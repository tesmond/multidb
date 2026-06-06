use crate::models::ConnectionConfig;
use anyhow::{anyhow, Context, Result};
use sqlx::{
    any::AnyPoolOptions, mysql::MySqlPoolOptions, postgres::PgPoolOptions, AnyPool, MySqlPool,
    PgPool,
};
use std::{
    collections::HashMap,
    net::{TcpStream, ToSocketAddrs},
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
        let pool = AnyPoolOptions::new()
            .max_connections(10)
            .min_connections(0)
            .connect(&dsn)
            .await
            .with_context(|| format!("connect {}", cfg.name))?;
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
        let pool = AnyPoolOptions::new()
            .max_connections(1)
            .connect(&dsn)
            .await
            .with_context(|| format!("test connection {}", cfg.name))?;
        pool.close().await;
        kill_port_forward(&mut port_forward);
        Ok(())
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
            let path = if cfg.database.trim().is_empty() {
                cfg.host.trim()
            } else {
                cfg.database.trim()
            };
            if path.is_empty() {
                return Err(anyhow!("sqlite database path is required"));
            }
            if path == ":memory:" || path.starts_with("sqlite:") {
                Ok(path.to_string())
            } else {
                Ok(format!("sqlite://{}", path.replace('\\', "/")))
            }
        }
        other => Err(anyhow!("unsupported driver: {other}")),
    }
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
