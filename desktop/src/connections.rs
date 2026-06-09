use crate::models::ConnectionConfig;
use anyhow::{anyhow, Context, Result};
use sqlx::{
    any::AnyPoolOptions, mysql::MySqlPoolOptions, postgres::PgPoolOptions, AnyPool, MySqlPool,
    PgPool,
};
use std::{
    collections::HashMap,
    net::{TcpStream, ToSocketAddrs},
    path::Path,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::sync::RwLock;
use url::Url;

struct ManagedConnection {
    pool: AnyPool,
    pg_pool: Option<PgPool>,
    mysql_pool: Option<MySqlPool>,
    config: ConnectionConfig,
    port_forward: Option<Child>,
}

#[derive(Default, Clone)]
pub struct ConnectionManager {
    inner: Arc<RwLock<HashMap<String, ManagedConnection>>>,
}

impl ConnectionManager {
    pub async fn connect(&self, cfg: ConnectionConfig) -> Result<()> {
        let mut effective = cfg.clone();
        let mut port_forward = None;

        if cfg.use_kube_port_forward {
            let child = start_port_forward(&cfg)?;
            wait_for_local_port(cfg.kube_local_port, Duration::from_secs(30))?;
            effective.host = "127.0.0.1".to_string();
            effective.port = cfg.kube_local_port;
            port_forward = Some(child);
        }

        let dsn = build_dsn(&effective)?;
        let connection_result = async {
            let pool = AnyPoolOptions::new()
                .max_connections(10)
                .min_connections(0)
                .connect(&dsn)
                .await
                .with_context(|| format!("connect {}", cfg.name))?;

            validate_connection_access(&pool)
                .await
                .with_context(|| format!("validate {}", cfg.name))?;

            let pg_pool = if effective.driver == "postgres" {
                Some(
                    PgPoolOptions::new()
                        .max_connections(10)
                        .min_connections(0)
                        .connect(&dsn)
                        .await
                        .with_context(|| format!("connect postgres {}", cfg.name))?,
                )
            } else {
                None
            };
            let mysql_pool = if effective.driver == "mysql" {
                Some(
                    MySqlPoolOptions::new()
                        .max_connections(10)
                        .min_connections(0)
                        .connect(&dsn)
                        .await
                        .with_context(|| format!("connect mysql {}", cfg.name))?,
                )
            } else {
                None
            };

            Result::<(AnyPool, Option<PgPool>, Option<MySqlPool>)>::Ok((pool, pg_pool, mysql_pool))
        }
        .await;

        let (pool, pg_pool, mysql_pool) = match connection_result {
            Ok(resources) => resources,
            Err(err) => {
                kill_port_forward(&mut port_forward);
                return Err(err);
            }
        };

        let mut inner = self.inner.write().await;
        if let Some(mut old) = inner.remove(&cfg.id) {
            old.pool.close().await;
            if let Some(pg_pool) = old.pg_pool {
                pg_pool.close().await;
            }
            if let Some(mysql_pool) = old.mysql_pool {
                mysql_pool.close().await;
            }
            kill_port_forward(&mut old.port_forward);
        }
        inner.insert(
            cfg.id.clone(),
            ManagedConnection {
                pool,
                pg_pool,
                mysql_pool,
                config: cfg,
                port_forward,
            },
        );
        Ok(())
    }

    pub async fn test_connection(&self, cfg: ConnectionConfig) -> Result<()> {
        let mut effective = cfg.clone();
        let mut port_forward = None;

        if cfg.use_kube_port_forward {
            let child = start_port_forward(&cfg)?;
            wait_for_local_port(cfg.kube_local_port, Duration::from_secs(30))?;
            effective.host = "127.0.0.1".to_string();
            effective.port = cfg.kube_local_port;
            port_forward = Some(child);
        }

        let dsn = build_dsn(&effective)?;
        let test_result = match AnyPoolOptions::new()
            .max_connections(1)
            .connect(&dsn)
            .await
            .with_context(|| format!("test connection {}", cfg.name))
        {
            Ok(pool) => {
                let result = validate_connection_access(&pool)
                    .await
                    .with_context(|| format!("validate connection {}", cfg.name));
                pool.close().await;
                result
            }
            Err(err) => Err(err),
        };
        kill_port_forward(&mut port_forward);
        test_result
    }

    pub async fn disconnect(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        let mut conn = inner
            .remove(id)
            .ok_or_else(|| anyhow!("connection {id:?} not found"))?;
        conn.pool.close().await;
        if let Some(pg_pool) = conn.pg_pool {
            pg_pool.close().await;
        }
        if let Some(mysql_pool) = conn.mysql_pool {
            mysql_pool.close().await;
        }
        kill_port_forward(&mut conn.port_forward);
        Ok(())
    }

    pub async fn get_pool(&self, id: &str) -> Result<AnyPool> {
        let inner = self.inner.read().await;
        inner
            .get(id)
            .map(|conn| conn.pool.clone())
            .ok_or_else(|| anyhow!("connection {id:?} not found"))
    }

    pub async fn get_pg_pool(&self, id: &str) -> Result<PgPool> {
        let inner = self.inner.read().await;
        inner
            .get(id)
            .and_then(|conn| conn.pg_pool.clone())
            .ok_or_else(|| anyhow!("postgres connection {id:?} not found"))
    }

    pub async fn get_mysql_pool(&self, id: &str) -> Result<MySqlPool> {
        let inner = self.inner.read().await;
        inner
            .get(id)
            .and_then(|conn| conn.mysql_pool.clone())
            .ok_or_else(|| anyhow!("mysql connection {id:?} not found"))
    }

    pub async fn get_config(&self, id: &str) -> Option<ConnectionConfig> {
        let inner = self.inner.read().await;
        inner.get(id).map(|conn| conn.config.clone())
    }

    pub async fn list_connections(&self) -> Vec<ConnectionConfig> {
        let inner = self.inner.read().await;
        inner
            .values()
            .map(|conn| {
                let mut cfg = conn.config.clone();
                cfg.password.clear();
                cfg
            })
            .collect()
    }
}

pub fn build_dsn(cfg: &ConnectionConfig) -> Result<String> {
    if !cfg.dsn.trim().is_empty() {
        return Ok(cfg.dsn.clone());
    }

    match cfg.driver.as_str() {
        "mysql" => {
            let mut url = Url::parse("mysql://localhost").expect("static mysql url");
            url.set_host(Some(&cfg.host))?;
            url.set_port(Some(cfg.port as u16))
                .map_err(|_| anyhow!("invalid mysql port {}", cfg.port))?;
            url.set_path(&cfg.database);
            if !cfg.username.is_empty() {
                url.set_username(&cfg.username)
                    .map_err(|_| anyhow!("invalid mysql username"))?;
                url.set_password(Some(&cfg.password))
                    .map_err(|_| anyhow!("invalid mysql password"))?;
            }
            url.query_pairs_mut().append_pair("ssl-mode", "PREFERRED");
            Ok(url.to_string())
        }
        "postgres" => {
            let mut url = Url::parse("postgres://localhost").expect("static postgres url");
            url.set_host(Some(&cfg.host))?;
            url.set_port(Some(cfg.port as u16))
                .map_err(|_| anyhow!("invalid postgres port {}", cfg.port))?;
            url.set_path(&cfg.database);
            if !cfg.username.is_empty() {
                url.set_username(&cfg.username)
                    .map_err(|_| anyhow!("invalid postgres username"))?;
                url.set_password(Some(&cfg.password))
                    .map_err(|_| anyhow!("invalid postgres password"))?;
            }
            url.query_pairs_mut().append_pair("sslmode", "prefer");
            Ok(url.to_string())
        }
        "sqlite" => {
            let path = cfg.database.trim();
            if path.is_empty() {
                return Err(anyhow!("sqlite database path is required"));
            }
            if path == ":memory:" || path.starts_with("sqlite:") {
                Ok(path.to_string())
            } else {
                Ok(build_sqlite_file_dsn(path))
            }
        }
        other => Err(anyhow!("unsupported driver: {other}")),
    }
}

async fn validate_connection_access(pool: &AnyPool) -> Result<()> {
    let probe: i64 = sqlx::query_scalar("SELECT 1")
        .fetch_one(pool)
        .await
        .context("run validation query")?;
    if probe != 1 {
        return Err(anyhow!("validation query returned unexpected result"));
    }
    Ok(())
}

fn build_sqlite_file_dsn(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    if normalized.starts_with('/') {
        format!("sqlite://{normalized}")
    } else if Path::new(path).is_absolute() || looks_like_windows_absolute_path(&normalized) {
        format!("sqlite:///{normalized}")
    } else {
        format!("sqlite://{normalized}")
    }
}

fn looks_like_windows_absolute_path(path: &str) -> bool {
    let bytes = path.as_bytes();
    bytes.len() > 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() && bytes[2] == b'/'
}

fn start_port_forward(cfg: &ConnectionConfig) -> Result<Child> {
    let mut args = Vec::new();
    if !cfg.kube_context.is_empty() {
        args.push(format!("--context={}", cfg.kube_context));
    }
    args.push("port-forward".to_string());
    if !cfg.kube_namespace.is_empty() {
        args.push("-n".to_string());
        args.push(cfg.kube_namespace.clone());
    }
    args.push(cfg.kube_resource.clone());
    args.push(format!("{}:{}", cfg.kube_local_port, cfg.kube_remote_port));

    Command::new("kubectl")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("kubectl port-forward")
}

fn wait_for_local_port(port: i32, timeout: Duration) -> Result<()> {
    let addr = format!("127.0.0.1:{port}");
    let addrs: Vec<_> = addr.to_socket_addrs()?.collect();
    let deadline = Instant::now() + timeout;

    while Instant::now() < deadline {
        if addrs
            .iter()
            .any(|addr| TcpStream::connect_timeout(addr, Duration::from_secs(1)).is_ok())
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(300));
    }

    Err(anyhow!(
        "port {port} not ready after {}s",
        timeout.as_secs()
    ))
}

fn kill_port_forward(child: &mut Option<Child>) {
    if let Some(child) = child.as_mut() {
        let _ = child.kill();
        let _ = child.wait();
    }
    *child = None;
}

#[cfg(test)]
mod tests {
    use super::{build_dsn, build_sqlite_file_dsn, validate_connection_access};
    use crate::models::ConnectionConfig;
    use sqlx::any::AnyPoolOptions;

    #[test]
    fn sqlite_requires_database_path() {
        let cfg = ConnectionConfig {
            driver: "sqlite".to_string(),
            host: "localhost".to_string(),
            ..ConnectionConfig::default()
        };

        let err = build_dsn(&cfg).expect_err("sqlite config without database should fail");
        assert!(err.to_string().contains("sqlite database path is required"));
    }

    #[test]
    fn sqlite_relative_path_uses_standard_dsn() {
        assert_eq!(build_sqlite_file_dsn("data/app.db"), "sqlite://data/app.db");
    }

    #[test]
    fn sqlite_windows_absolute_path_uses_triple_slash_dsn() {
        assert_eq!(
            build_sqlite_file_dsn(r"C:\Users\tesmo\data\app.db"),
            "sqlite:///C:/Users/tesmo/data/app.db"
        );
    }

    #[tokio::test]
    async fn validation_query_runs_successfully() {
        sqlx::any::install_default_drivers();

        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .expect("create in-memory sqlite pool");

        validate_connection_access(&pool)
            .await
            .expect("validation query should succeed");

        pool.close().await;
    }
}
