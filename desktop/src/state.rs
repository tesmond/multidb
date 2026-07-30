use crate::{connections::ConnectionManager, history::HistoryStore, models::ConnectionConfig};
use anyhow::{anyhow, Result};
use std::{collections::HashMap, path::PathBuf};
use tokio::sync::{Mutex, OnceCell};
use tokio_util::sync::CancellationToken;

#[derive(Default)]
pub struct AppState {
    pub connections: ConnectionManager,
    pub store: OnceCell<HistoryStore>,
    pub query_cancels: Mutex<HashMap<String, CancellationToken>>,
}

impl AppState {
    pub async fn store(&self) -> Result<HistoryStore> {
        self.store
            .get_or_try_init(|| async { HistoryStore::open(history_db_path()).await })
            .await
            .cloned()
    }

    pub async fn get_config_or_saved(&self, conn_id: &str) -> Result<ConnectionConfig> {
        if let Some(cfg) = self.connections.get_config(conn_id).await {
            return Ok(cfg);
        }

        let store = self.store().await?;
        store.load_saved_connection(conn_id).await
    }

    pub async fn get_pool_or_reconnect(&self, conn_id: &str) -> Result<sqlx::AnyPool> {
        match self.connections.get_pool(conn_id).await {
            Ok(pool) => Ok(pool),
            Err(original) => {
                let store = self.store().await?;
                let cfg = store.load_saved_connection(conn_id).await?;
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
                let cfg = store.load_saved_connection(conn_id).await?;
                self.connections.connect(cfg).await?;
                self.connections
                    .get_pg_pool(conn_id)
                    .await
                    .map_err(|err| anyhow!("{original}; reconnect failed: {err}"))
            }
        }
    }

    pub async fn get_mysql_pool_or_reconnect(&self, conn_id: &str) -> Result<sqlx::MySqlPool> {
        match self.connections.get_mysql_pool(conn_id).await {
            Ok(pool) => Ok(pool),
            Err(original) => {
                let store = self.store().await?;
                let cfg = store.load_saved_connection(conn_id).await?;
                self.connections.connect(cfg).await?;
                self.connections
                    .get_mysql_pool(conn_id)
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
