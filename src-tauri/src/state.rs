use crate::{connections::ConnectionManager, history::HistoryStore};
use anyhow::{anyhow, Result};
use std::{collections::HashMap, path::PathBuf};
use tokio::sync::{Mutex, RwLock};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct AppState {
    pub connections: ConnectionManager,
    pub store: RwLock<Option<HistoryStore>>,
    pub query_cancels: Mutex<HashMap<String, CancellationToken>>,
}

impl AppState {
    pub async fn initialise(&self) -> Result<()> {
        let store = HistoryStore::open(history_db_path()).await?;
        let saved = store.list_saved_connections().await.unwrap_or_default();

        for cfg in saved {
            let _ = self.connections.connect(cfg).await;
        }

        *self.store.write().await = Some(store);
        Ok(())
    }

    pub async fn store(&self) -> Result<HistoryStore> {
        self.store
            .read()
            .await
            .clone()
            .ok_or_else(|| anyhow!("store not initialised"))
    }

    pub async fn get_pool_or_reconnect(&self, conn_id: &str) -> Result<sqlx::AnyPool> {
        match self.connections.get_pool(conn_id).await {
            Ok(pool) => Ok(pool),
            Err(original) => {
                let store = self.store().await?;
                let saved = store.list_saved_connections().await?;
                let cfg = saved
                    .into_iter()
                    .find(|cfg| cfg.id == conn_id)
                    .ok_or_else(|| anyhow!("connection {conn_id:?} not found"))?;
                self.connections.connect(cfg).await?;
                self.connections
                    .get_pool(conn_id)
                    .await
                    .map_err(|err| anyhow!("{original}; reconnect failed: {err}"))
            }
        }
    }

    pub async fn get_pg_pool_or_reconnect(&self, conn_id: &str) -> Result<sqlx::PgPool> {
        match self.connections.get_pg_pool(conn_id).await {
            Ok(pool) => Ok(pool),
            Err(original) => {
                let store = self.store().await?;
                let saved = store.list_saved_connections().await?;
                let cfg = saved
                    .into_iter()
                    .find(|cfg| cfg.id == conn_id)
                    .ok_or_else(|| anyhow!("connection {conn_id:?} not found"))?;
                self.connections.connect(cfg).await?;
                self.connections
                    .get_pg_pool(conn_id)
                    .await
                    .map_err(|err| anyhow!("{original}; reconnect failed: {err}"))
            }
        }
    }
}

fn history_db_path() -> PathBuf {
    let base = dirs::config_dir().unwrap_or_else(std::env::temp_dir);
    base.join("multidb").join("history.db")
}
