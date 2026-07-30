use crate::{
    models::{ConnectionConfig, QueryRecord, SavedQuery, SchemaCacheEntry},
    password_vault,
};
use anyhow::{anyhow, Context, Result};
use sqlx::{
    sqlite::{SqliteConnectOptions, SqlitePoolOptions},
    Row, SqlitePool,
};
use std::{path::Path, str::FromStr};

#[derive(Clone)]
pub struct HistoryStore {
    pool: SqlitePool,
}

impl HistoryStore {
    pub async fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let options =
            SqliteConnectOptions::from_str(&path.to_string_lossy())?.create_if_missing(true);
        let pool = SqlitePoolOptions::new()
            .max_connections(5)
            .connect_with(options)
            .await
            .context("open history store")?;
        let store = Self { pool };
        store.migrate().await?;
        store.migrate_connection_passwords_to_keychain().await?;
        Ok(store)
    }

    async fn migrate(&self) -> Result<()> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS query_history (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conn_id TEXT NOT NULL,
                query TEXT NOT NULL,
                duration_ms INTEGER NOT NULL DEFAULT 0,
                result_count INTEGER NOT NULL DEFAULT 0,
                error TEXT NOT NULL DEFAULT '',
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            CREATE TABLE IF NOT EXISTS saved_connections (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                driver TEXT NOT NULL,
                tab_color TEXT NOT NULL DEFAULT '',
                tab_text_black INTEGER NOT NULL DEFAULT 0,
                host TEXT NOT NULL DEFAULT '',
                port INTEGER NOT NULL DEFAULT 0,
                username TEXT NOT NULL DEFAULT '',
                has_keychain_password INTEGER NOT NULL DEFAULT 0,
                auth_mode TEXT NOT NULL DEFAULT 'password',
                database TEXT NOT NULL DEFAULT '',
                dsn TEXT NOT NULL DEFAULT '',
                aws_region TEXT NOT NULL DEFAULT '',
                aws_profile TEXT NOT NULL DEFAULT '',
                ssl_ca_path TEXT NOT NULL DEFAULT '',
                use_kube_port_forward INTEGER NOT NULL DEFAULT 0,
                kube_context TEXT NOT NULL DEFAULT '',
                kube_namespace TEXT NOT NULL DEFAULT '',
                kube_resource TEXT NOT NULL DEFAULT '',
                kube_local_port INTEGER NOT NULL DEFAULT 0,
                kube_remote_port INTEGER NOT NULL DEFAULT 0
            );
            CREATE TABLE IF NOT EXISTS schema_cache (
                conn_id TEXT PRIMARY KEY,
                schema_json TEXT NOT NULL,
                last_refreshed_at TEXT NOT NULL,
                hash TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS saved_queries (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                conn_id TEXT NOT NULL,
                title TEXT NOT NULL,
                query TEXT NOT NULL,
                created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
            );
            "#,
        )
        .execute(&self.pool)
        .await?;

        for statement in [
            "ALTER TABLE query_history ADD COLUMN result_count INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE saved_connections ADD COLUMN tab_color TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN tab_text_black INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE saved_connections ADD COLUMN has_keychain_password INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE saved_connections ADD COLUMN auth_mode TEXT NOT NULL DEFAULT 'password'",
            "ALTER TABLE saved_connections ADD COLUMN aws_region TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN aws_profile TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN ssl_ca_path TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN use_kube_port_forward INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE saved_connections ADD COLUMN kube_context TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN kube_namespace TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN kube_resource TEXT NOT NULL DEFAULT ''",
            "ALTER TABLE saved_connections ADD COLUMN kube_local_port INTEGER NOT NULL DEFAULT 0",
            "ALTER TABLE saved_connections ADD COLUMN kube_remote_port INTEGER NOT NULL DEFAULT 0",
        ] {
            if let Err(err) = sqlx::query(statement).execute(&self.pool).await {
                let message = err.to_string();
                if !message.contains("duplicate column name") && !message.contains("already exists") {
                    return Err(err.into());
                }
            }
        }

        Ok(())
    }

    pub async fn add_query_history(&self, rec: QueryRecord) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO query_history (conn_id, query, duration_ms, result_count, error, created_at)
            VALUES (?, ?, ?, ?, ?, COALESCE(NULLIF(?, ''), datetime('now')))
            "#,
        )
        .bind(rec.conn_id)
        .bind(rec.query)
        .bind(rec.duration)
        .bind(rec.result_count)
        .bind(rec.error)
        .bind(rec.created_at)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn get_query_history(&self, limit: i64) -> Result<Vec<QueryRecord>> {
        let limit = if limit <= 0 { 100 } else { limit };
        self.query_history(
            "SELECT id, conn_id, query, duration_ms, result_count, error, created_at FROM query_history ORDER BY id DESC LIMIT ?",
            None,
            limit,
        )
        .await
    }

    pub async fn get_query_history_by_conn_id(
        &self,
        conn_id: &str,
        limit: i64,
    ) -> Result<Vec<QueryRecord>> {
        let limit = if limit <= 0 { 100 } else { limit };
        self.query_history(
            "SELECT id, conn_id, query, duration_ms, result_count, error, created_at FROM query_history WHERE conn_id = ? ORDER BY id DESC LIMIT ?",
            Some(conn_id),
            limit,
        )
        .await
    }

    async fn query_history(
        &self,
        sql: &str,
        conn_id: Option<&str>,
        limit: i64,
    ) -> Result<Vec<QueryRecord>> {
        let mut query = sqlx::query_as::<_, (i64, String, String, i64, i64, String, String)>(sql);
        if let Some(conn_id) = conn_id {
            query = query.bind(conn_id);
        }
        let rows = query.bind(limit).fetch_all(&self.pool).await?;
        Ok(rows
            .into_iter()
            .map(
                |(id, conn_id, query, duration, result_count, error, created_at)| QueryRecord {
                    id,
                    conn_id,
                    query,
                    duration,
                    result_count,
                    error,
                    created_at,
                },
            )
            .collect())
    }

    pub async fn clear_query_history(&self) -> Result<()> {
        sqlx::query("DELETE FROM query_history")
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn clear_query_history_by_conn_id(&self, conn_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM query_history WHERE conn_id = ?")
            .bind(conn_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_connection(&self, cfg: &ConnectionConfig) -> Result<()> {
        let has_keychain_password = if cfg.uses_aws_iam_auth() {
            password_vault::delete_connection_password(&cfg.id)?;
            false
        } else if cfg.password.is_empty() && cfg.has_saved_password {
            true
        } else {
            password_vault::save_connection_password(&cfg.id, &cfg.password)?;
            !cfg.password.is_empty()
        };

        sqlx::query(
            r#"
            INSERT INTO saved_connections (
                id, name, driver, tab_color, tab_text_black, host, port, username, has_keychain_password, auth_mode, database, dsn,
                aws_region, aws_profile, ssl_ca_path,
                use_kube_port_forward, kube_context, kube_namespace, kube_resource, kube_local_port, kube_remote_port
            )
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            ON CONFLICT(id) DO UPDATE SET
                name=excluded.name, driver=excluded.driver, tab_color=excluded.tab_color,
                tab_text_black=excluded.tab_text_black, host=excluded.host, port=excluded.port,
                username=excluded.username, has_keychain_password=excluded.has_keychain_password,
                auth_mode=excluded.auth_mode, database=excluded.database,
                dsn=excluded.dsn, aws_region=excluded.aws_region, aws_profile=excluded.aws_profile,
                ssl_ca_path=excluded.ssl_ca_path, use_kube_port_forward=excluded.use_kube_port_forward,
                kube_context=excluded.kube_context, kube_namespace=excluded.kube_namespace,
                kube_resource=excluded.kube_resource, kube_local_port=excluded.kube_local_port,
                kube_remote_port=excluded.kube_remote_port
            "#,
        )
        .bind(&cfg.id)
        .bind(&cfg.name)
        .bind(&cfg.driver)
        .bind(&cfg.tab_color)
        .bind(cfg.tab_text_black as i64)
        .bind(&cfg.host)
        .bind(cfg.port as i64)
        .bind(&cfg.username)
        .bind(has_keychain_password as i64)
        .bind(&cfg.auth_mode)
        .bind(&cfg.database)
        .bind(&cfg.dsn)
        .bind(&cfg.aws_region)
        .bind(&cfg.aws_profile)
        .bind(&cfg.ssl_ca_path)
        .bind(cfg.use_kube_port_forward as i64)
        .bind(&cfg.kube_context)
        .bind(&cfg.kube_namespace)
        .bind(&cfg.kube_resource)
        .bind(cfg.kube_local_port as i64)
        .bind(cfg.kube_remote_port as i64)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    pub async fn delete_connection(&self, id: &str) -> Result<()> {
        sqlx::query("DELETE FROM saved_connections WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        password_vault::delete_connection_password(id)?;
        Ok(())
    }

    pub async fn list_saved_connections(&self) -> Result<Vec<ConnectionConfig>> {
        let rows = sqlx::query(
            r#"
             SELECT id, name, driver, tab_color, tab_text_black, host, port, username, has_keychain_password, auth_mode, database, dsn,
                 aws_region, aws_profile, ssl_ca_path,
                   use_kube_port_forward, kube_context, kube_namespace, kube_resource, kube_local_port, kube_remote_port
            FROM saved_connections
            ORDER BY name
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let mut out = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get("name")?;
            let has_keychain_password = row.try_get::<i64, _>("has_keychain_password")? != 0;

            out.push(ConnectionConfig {
                id: row.try_get("id")?,
                name,
                driver: row.try_get("driver")?,
                tab_color: row.try_get("tab_color")?,
                tab_text_black: row.try_get::<i64, _>("tab_text_black")? != 0,
                host: row.try_get("host")?,
                port: row.try_get::<i64, _>("port")? as i32,
                username: row.try_get("username")?,
                password: String::new(),
                has_saved_password: has_keychain_password,
                auth_mode: row.try_get("auth_mode")?,
                database: row.try_get("database")?,
                dsn: row.try_get("dsn")?,
                aws_region: row.try_get("aws_region")?,
                aws_profile: row.try_get("aws_profile")?,
                ssl_ca_path: row.try_get("ssl_ca_path")?,
                use_kube_port_forward: row.try_get::<i64, _>("use_kube_port_forward")? != 0,
                kube_context: row.try_get("kube_context")?,
                kube_namespace: row.try_get("kube_namespace")?,
                kube_resource: row.try_get("kube_resource")?,
                kube_local_port: row.try_get::<i64, _>("kube_local_port")? as i32,
                kube_remote_port: row.try_get::<i64, _>("kube_remote_port")? as i32,
            });
        }

        Ok(out)
    }

    pub async fn load_saved_connection(&self, id: &str) -> Result<ConnectionConfig> {
        let mut matches = self
            .list_saved_connections()
            .await?
            .into_iter()
            .filter(|cfg| cfg.id == id);
        let mut cfg = matches
            .next()
            .ok_or_else(|| anyhow!("connection {id:?} not found"))?;

        if cfg.has_saved_password {
            cfg.password = password_vault::load_connection_password(id)?.ok_or_else(|| {
                anyhow!(
                    "Saved password for connection \"{}\" ({id}) is missing from the OS keychain",
                    cfg.name
                )
            })?;
        }

        Ok(cfg)
    }

    async fn migrate_connection_passwords_to_keychain(&self) -> Result<()> {
        if self.column_exists("saved_connections", "password").await? {
            let rows =
                sqlx::query("SELECT id, password FROM saved_connections WHERE password <> ''")
                    .fetch_all(&self.pool)
                    .await?;

            for row in rows {
                let id: String = row.try_get("id")?;
                let legacy_password: String = row.try_get("password")?;
                if legacy_password.is_empty() {
                    continue;
                }
                password_vault::save_connection_password(&id, &legacy_password)?;
                self.set_has_keychain_password(&id, true).await?;
            }

            self.drop_saved_connection_password_column().await?;
        }

        Ok(())
    }

    async fn set_has_keychain_password(&self, id: &str, has_keychain_password: bool) -> Result<()> {
        sqlx::query("UPDATE saved_connections SET has_keychain_password = ? WHERE id = ?")
            .bind(has_keychain_password as i64)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    async fn column_exists(&self, table: &str, column: &str) -> Result<bool> {
        let pragma = format!("PRAGMA table_info({table})");
        let rows = sqlx::query(&pragma).fetch_all(&self.pool).await?;
        for row in rows {
            let name: String = row.try_get("name")?;
            if name == column {
                return Ok(true);
            }
        }
        Ok(false)
    }

    async fn drop_saved_connection_password_column(&self) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query("DROP TABLE IF EXISTS saved_connections_without_password")
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            CREATE TABLE saved_connections_without_password (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                driver TEXT NOT NULL,
                tab_color TEXT NOT NULL DEFAULT '',
                tab_text_black INTEGER NOT NULL DEFAULT 0,
                host TEXT NOT NULL DEFAULT '',
                port INTEGER NOT NULL DEFAULT 0,
                username TEXT NOT NULL DEFAULT '',
                has_keychain_password INTEGER NOT NULL DEFAULT 0,
                auth_mode TEXT NOT NULL DEFAULT 'password',
                database TEXT NOT NULL DEFAULT '',
                dsn TEXT NOT NULL DEFAULT '',
                aws_region TEXT NOT NULL DEFAULT '',
                aws_profile TEXT NOT NULL DEFAULT '',
                ssl_ca_path TEXT NOT NULL DEFAULT '',
                use_kube_port_forward INTEGER NOT NULL DEFAULT 0,
                kube_context TEXT NOT NULL DEFAULT '',
                kube_namespace TEXT NOT NULL DEFAULT '',
                kube_resource TEXT NOT NULL DEFAULT '',
                kube_local_port INTEGER NOT NULL DEFAULT 0,
                kube_remote_port INTEGER NOT NULL DEFAULT 0
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO saved_connections_without_password (
                id, name, driver, tab_color, tab_text_black, host, port, username,
                has_keychain_password, auth_mode, database, dsn, aws_region, aws_profile, ssl_ca_path, use_kube_port_forward,
                kube_context, kube_namespace, kube_resource, kube_local_port, kube_remote_port
            )
            SELECT
                id, name, driver, tab_color, tab_text_black, host, port, username,
                has_keychain_password, COALESCE(auth_mode, 'password'), database, dsn,
                COALESCE(aws_region, ''), COALESCE(aws_profile, ''), COALESCE(ssl_ca_path, ''), use_kube_port_forward,
                kube_context, kube_namespace, kube_resource, kube_local_port, kube_remote_port
            FROM saved_connections
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("DROP TABLE saved_connections")
            .execute(&mut *tx)
            .await?;
        sqlx::query("ALTER TABLE saved_connections_without_password RENAME TO saved_connections")
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn save_schema(&self, conn_id: &str, schema_json: &str, hash: &str) -> Result<()> {
        let mut tx = self.pool.begin().await?;

        sqlx::query(
            r#"
            CREATE TEMP TABLE IF NOT EXISTS schema_cache_refresh (
                conn_id TEXT PRIMARY KEY,
                schema_json TEXT NOT NULL,
                last_refreshed_at TEXT NOT NULL,
                hash TEXT NOT NULL
            )
            "#,
        )
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM schema_cache_refresh WHERE conn_id = ?")
            .bind(conn_id)
            .execute(&mut *tx)
            .await?;

        sqlx::query(
            r#"
            INSERT INTO schema_cache_refresh (conn_id, schema_json, last_refreshed_at, hash)
            VALUES (?, ?, datetime('now'), ?)
            "#,
        )
        .bind(conn_id)
        .bind(schema_json)
        .bind(hash)
        .execute(&mut *tx)
        .await?;

        sqlx::query(
            r#"
            INSERT INTO schema_cache (conn_id, schema_json, last_refreshed_at, hash)
            SELECT conn_id, schema_json, last_refreshed_at, hash
            FROM schema_cache_refresh
            WHERE conn_id = ?
            ON CONFLICT(conn_id) DO UPDATE SET
                schema_json=excluded.schema_json,
                last_refreshed_at=excluded.last_refreshed_at,
                hash=excluded.hash
            "#,
        )
        .bind(conn_id)
        .execute(&mut *tx)
        .await?;

        sqlx::query("DELETE FROM schema_cache_refresh WHERE conn_id = ?")
            .bind(conn_id)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(())
    }

    pub async fn load_schema(&self, conn_id: &str) -> Result<SchemaCacheEntry> {
        let (schema_json, last_refreshed_at, hash) = sqlx::query_as::<_, (String, String, String)>(
            "SELECT schema_json, last_refreshed_at, hash FROM schema_cache WHERE conn_id = ?",
        )
        .bind(conn_id)
        .fetch_one(&self.pool)
        .await?;
        Ok(SchemaCacheEntry {
            schema_json,
            last_refreshed_at,
            hash,
        })
    }

    pub async fn delete_schema(&self, conn_id: &str) -> Result<()> {
        sqlx::query("DELETE FROM schema_cache WHERE conn_id = ?")
            .bind(conn_id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn save_query(&self, rec: SavedQuery) -> Result<SavedQuery> {
        let created_at = if rec.created_at.is_empty() {
            chrono_like_now()
        } else {
            rec.created_at
        };
        let result = sqlx::query(
            "INSERT INTO saved_queries (conn_id, title, query, created_at) VALUES (?, ?, ?, ?)",
        )
        .bind(&rec.conn_id)
        .bind(&rec.title)
        .bind(&rec.query)
        .bind(&created_at)
        .execute(&self.pool)
        .await?;

        Ok(SavedQuery {
            id: result.last_insert_rowid(),
            created_at,
            ..rec
        })
    }

    pub async fn get_saved_queries(&self, conn_id: &str) -> Result<Vec<SavedQuery>> {
        let rows = sqlx::query_as::<_, (i64, String, String, String, String)>(
            "SELECT id, conn_id, title, query, created_at FROM saved_queries WHERE conn_id = ? ORDER BY id DESC",
        )
        .bind(conn_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows
            .into_iter()
            .map(|(id, conn_id, title, query, created_at)| SavedQuery {
                id,
                conn_id,
                title,
                query,
                created_at,
            })
            .collect())
    }

    pub async fn delete_saved_query(&self, id: i64) -> Result<()> {
        sqlx::query("DELETE FROM saved_queries WHERE id = ?")
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn update_saved_query_title(&self, id: i64, title: &str) -> Result<()> {
        sqlx::query("UPDATE saved_queries SET title = ? WHERE id = ?")
            .bind(title)
            .bind(id)
            .execute(&self.pool)
            .await?;
        Ok(())
    }
}

fn chrono_like_now() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or_default();
    seconds.to_string()
}
