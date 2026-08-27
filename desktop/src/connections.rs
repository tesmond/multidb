use crate::{ipc_diagnostics, models::ConnectionConfig};
use anyhow::{anyhow, Context, Result};
use aws_config::{BehaviorVersion, Region};
use aws_sdk_rds::auth_token::{AuthTokenGenerator, Config as AuthTokenConfig};
use sqlx::{
    any::AnyPoolOptions,
    mysql::{MySqlConnectOptions, MySqlPoolOptions, MySqlSslMode},
    postgres::PgPoolOptions,
    AnyPool, ConnectOptions, MySqlPool, PgPool,
};
use std::{
    collections::{HashMap, VecDeque},
    ffi::{OsStr, OsString},
    future::Future,
    io::{self, Read},
    net::{SocketAddr, TcpListener},
    path::{Path, PathBuf},
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex as StdMutex,
    },
    time::{Duration, Instant},
};
use tokio::net::TcpStream;
use tokio::sync::RwLock;
use tokio::time::{sleep, timeout};
use tokio_util::sync::CancellationToken;
use url::Url;

struct ManagedConnection {
    pool: Option<AnyPool>,
    pg_pool: Option<PgPool>,
    mysql_pool: Option<MySqlPool>,
    config: ConnectionConfig,
    port_forward: Option<PortForwardProcess>,
    connected_at: Instant,
}

struct PortForwardProcess {
    child: Child,
    stderr: Arc<StdMutex<VecDeque<u8>>>,
    stderr_done: Arc<AtomicBool>,
}

impl Drop for PortForwardProcess {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl PortForwardProcess {
    fn capture(mut child: Child) -> Result<Self> {
        const STDERR_LIMIT: usize = 16 * 1024;
        let mut pipe = child
            .stderr
            .take()
            .ok_or_else(|| anyhow!("kubectl stderr was not captured"))?;
        let stderr = Arc::new(StdMutex::new(VecDeque::with_capacity(STDERR_LIMIT)));
        let thread_buffer = stderr.clone();
        let stderr_done = Arc::new(AtomicBool::new(false));
        let thread_done = stderr_done.clone();
        std::thread::spawn(move || {
            let mut chunk = [0_u8; 1024];
            while let Ok(count) = pipe.read(&mut chunk) {
                if count == 0 {
                    break;
                }
                let mut buffer = thread_buffer.lock().unwrap_or_else(|err| err.into_inner());
                buffer.extend(&chunk[..count]);
                while buffer.len() > STDERR_LIMIT {
                    buffer.pop_front();
                }
            }
            thread_done.store(true, Ordering::Release);
        });
        Ok(Self {
            child,
            stderr,
            stderr_done,
        })
    }

    async fn wait_for_stderr(&self) {
        let _ = timeout(Duration::from_millis(100), async {
            while !self.stderr_done.load(Ordering::Acquire) {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await;
    }

    fn stderr_text(&self) -> String {
        let mut buffer = self.stderr.lock().unwrap_or_else(|err| err.into_inner());
        String::from_utf8_lossy(buffer.make_contiguous())
            .trim()
            .to_string()
    }
}

const DEFAULT_MAX_CONNECTIONS: u32 = 10;
const IAM_MAX_CONNECTIONS: u32 = 1;
const AWS_IAM_REFRESH_AGE: Duration = Duration::from_secs(14 * 60);
const AWS_SDK_LOAD_TIMEOUT: Duration = Duration::from_secs(10);
const AWS_IAM_TOKEN_TIMEOUT: Duration = Duration::from_secs(10);
const MYSQL_CONNECT_TIMEOUT: Duration = Duration::from_secs(60);
const MYSQL_CONNECT_RETRY_DELAY: Duration = Duration::from_millis(500);
const CONNECTION_VALIDATION_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Default, Clone)]
pub struct ConnectionManager {
    inner: Arc<RwLock<HashMap<String, ManagedConnection>>>,
}

impl ConnectionManager {
    pub async fn connect(&self, cfg: ConnectionConfig) -> Result<()> {
        let (effective, mut port_forward) = prepare_runtime_connection_config(&cfg, None).await?;
        let connection_result = async {
            let max_connections = max_connections_for(&effective);
            let (pool, pg_pool, mysql_pool) = if effective.driver == "mysql" {
                let mysql_pool = connect_mysql_pool(&effective, max_connections, &cfg.name).await?;
                (None, None, Some(mysql_pool))
            } else {
                let dsn = build_dsn(&effective)?;
                let pool = AnyPoolOptions::new()
                    .max_connections(max_connections)
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
                            .max_connections(max_connections)
                            .min_connections(0)
                            .connect(&dsn)
                            .await
                            .with_context(|| format!("connect postgres {}", cfg.name))?,
                    )
                } else {
                    None
                };
                (Some(pool), pg_pool, None)
            };

            Result::<(Option<AnyPool>, Option<PgPool>, Option<MySqlPool>)>::Ok((
                pool, pg_pool, mysql_pool,
            ))
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
            if let Some(pool) = old.pool {
                pool.close().await;
            }
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
                connected_at: Instant::now(),
            },
        );
        Ok(())
    }

    pub async fn test_connection(
        &self,
        cfg: ConnectionConfig,
        cancel: CancellationToken,
    ) -> Result<()> {
        ipc_diagnostics::set_test_connection_stage("prepare_runtime_connection_config");
        let (effective, mut port_forward) =
            prepare_runtime_connection_config(&cfg, Some(&cancel)).await?;
        let connection_test = async {
            if effective.driver == "mysql" {
                match connect_mysql_pool(&effective, IAM_MAX_CONNECTIONS, &cfg.name).await {
                    Ok(mysql_pool) => {
                        mysql_pool.close().await;
                        Ok(())
                    }
                    Err(err) => Err(err),
                }
            } else {
                let dsn = build_dsn(&effective)?;
                let stage = match effective.driver.as_str() {
                    "postgres" => "postgres_connect",
                    "sqlite" => "sqlite_connect",
                    _ => "validate_connection_access",
                };
                ipc_diagnostics::set_test_connection_stage(stage);
                match AnyPoolOptions::new()
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
                }
            }
        };
        let test_result = tokio::select! {
            _ = cancel.cancelled() => Err(anyhow!("connection test cancelled")),
            result = connection_test => result,
        };
        kill_port_forward(&mut port_forward);
        test_result
    }

    pub async fn disconnect(&self, id: &str) -> Result<()> {
        let mut inner = self.inner.write().await;
        let mut conn = inner
            .remove(id)
            .ok_or_else(|| anyhow!("connection {id:?} not found"))?;
        if let Some(pool) = conn.pool {
            pool.close().await;
        }
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
            .and_then(|conn| conn.pool.clone())
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

    pub async fn should_refresh_iam_connection(&self, id: &str) -> bool {
        let inner = self.inner.read().await;
        inner.get(id).is_some_and(|conn| {
            conn.config.uses_aws_iam_auth() && conn.connected_at.elapsed() >= AWS_IAM_REFRESH_AGE
        })
    }

    pub async fn list_connections(&self) -> Vec<ConnectionConfig> {
        let inner = self.inner.read().await;
        inner
            .values()
            .map(|conn| {
                let mut cfg = conn.config.clone();
                cfg.password.clear();
                cfg.has_saved_password = false;
                cfg
            })
            .collect()
    }
}

pub fn build_dsn(cfg: &ConnectionConfig) -> Result<String> {
    if !cfg.dsn.trim().is_empty() && !cfg.uses_aws_iam_auth() {
        return Ok(cfg.dsn.clone());
    }

    match cfg.driver.as_str() {
        "mysql" => Ok(build_mysql_connect_options(cfg)?.to_url_lossy().to_string()),
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

async fn prepare_runtime_connection_config(
    cfg: &ConnectionConfig,
    cancel: Option<&CancellationToken>,
) -> Result<(ConnectionConfig, Option<PortForwardProcess>)> {
    ipc_diagnostics::set_test_connection_stage("validate_connection_config");
    validate_connection_config(cfg)?;

    let mut effective = cfg.clone();
    let mut port_forward = None;

    if cfg.use_kube_port_forward {
        ipc_diagnostics::set_test_connection_stage("wait_for_local_port");
        let mut process = start_port_forward(cfg)?;
        wait_for_local_port(
            &mut process,
            cfg.kube_local_port,
            Duration::from_secs(30),
            cancel,
        )
        .await?;
        effective.host = "127.0.0.1".to_string();
        effective.port = cfg.kube_local_port;
        port_forward = Some(process);
    }

    if effective.uses_aws_iam_auth() {
        ipc_diagnostics::set_test_connection_stage("aws_generate_iam_token");
        effective.password = generate_mysql_aws_iam_token(&effective).await?;
        effective.has_saved_password = false;
        effective.dsn.clear();
    }

    Ok((effective, port_forward))
}

fn validate_connection_config(cfg: &ConnectionConfig) -> Result<()> {
    if cfg.uses_aws_iam_auth() {
        if !cfg.dsn.trim().is_empty() {
            return Err(anyhow!(
                "AWS IAM MySQL connections require host, port, username, and AWS region fields instead of a DSN"
            ));
        }
        if cfg.use_kube_port_forward {
            return Err(anyhow!(
                "AWS IAM MySQL authentication is not supported with Kubernetes port forwarding"
            ));
        }
        if cfg.host.trim().is_empty() {
            return Err(anyhow!("mysql host is required for AWS IAM authentication"));
        }
        if cfg.port <= 0 {
            return Err(anyhow!("mysql port is required for AWS IAM authentication"));
        }
        if cfg.username.trim().is_empty() {
            return Err(anyhow!(
                "mysql username is required for AWS IAM authentication"
            ));
        }
        if cfg.aws_region.trim().is_empty() {
            return Err(anyhow!("AWS region is required for AWS IAM authentication"));
        }
    }

    Ok(())
}

async fn generate_mysql_aws_iam_token(cfg: &ConnectionConfig) -> Result<String> {
    ipc_diagnostics::set_test_connection_stage("aws_sdk_load_config");
    let mut loader = aws_config::defaults(BehaviorVersion::latest())
        .region(Region::new(cfg.aws_region.trim().to_string()));

    if !cfg.aws_profile.trim().is_empty() {
        loader = loader.profile_name(cfg.aws_profile.trim().to_string());
    }

    let sdk_config = timeout_step(
        AWS_SDK_LOAD_TIMEOUT,
        format!(
            "load AWS SDK config for region {}{}",
            cfg.aws_region.trim(),
            if cfg.aws_profile.trim().is_empty() {
                String::new()
            } else {
                format!(" with profile {}", cfg.aws_profile.trim())
            }
        ),
        loader.load(),
    )
    .await?;
    let token_config = AuthTokenConfig::builder()
        .hostname(cfg.host.trim())
        .port(cfg.port as u64)
        .username(cfg.username.trim())
        .expires_in(900)
        .build()
        .map_err(|err| anyhow!("build AWS RDS auth token config: {err}"))?;

    let token = timeout_step(
        AWS_IAM_TOKEN_TIMEOUT,
        format!(
            "generate AWS RDS IAM auth token for {}@{}:{}",
            cfg.username.trim(),
            cfg.host.trim(),
            cfg.port
        ),
        AuthTokenGenerator::new(token_config).auth_token(&sdk_config),
    )
    .await?
    .map_err(|err| anyhow!("generate AWS RDS auth token: {err}"))?;

    Ok(token.to_string())
}

async fn connect_mysql_pool(
    cfg: &ConnectionConfig,
    max_connections: u32,
    connection_name: &str,
) -> Result<MySqlPool> {
    ipc_diagnostics::set_test_connection_stage("mysql_build_connect_options");
    let options = build_mysql_connect_options(cfg)?;
    let connection_details = mysql_connection_error_details(cfg);

    ipc_diagnostics::set_test_connection_stage("mysql_connect");
    let connect_started_at = Instant::now();
    let mysql_pool = timeout_step(
        MYSQL_CONNECT_TIMEOUT,
        format!("connect mysql {} ({})", connection_name, connection_details),
        connect_mysql_pool_with_retries(options, max_connections, MYSQL_CONNECT_TIMEOUT),
    )
    .await?
    .with_context(|| {
        format!(
            "connect mysql {} ({}, waited={})",
            connection_name,
            connection_details,
            format_duration_seconds(connect_started_at.elapsed())
        )
    })?;

    ipc_diagnostics::set_test_connection_stage("mysql_validate");
    let validate_started_at = Instant::now();
    timeout_step(
        CONNECTION_VALIDATION_TIMEOUT,
        format!("run validation query for mysql {}", connection_name),
        validate_mysql_connection_access(&mysql_pool),
    )
    .await?
    .with_context(|| {
        format!(
            "validate {} ({}, waited={})",
            connection_name,
            connection_details,
            format_duration_seconds(validate_started_at.elapsed())
        )
    })?;

    Ok(mysql_pool)
}

async fn connect_mysql_pool_with_retries(
    options: MySqlConnectOptions,
    max_connections: u32,
    connect_timeout: Duration,
) -> Result<MySqlPool, sqlx::Error> {
    let started_at = Instant::now();

    loop {
        let result = MySqlPoolOptions::new()
            .max_connections(max_connections)
            .min_connections(0)
            .acquire_timeout(connect_timeout)
            .connect_with(options.clone())
            .await;

        match result {
            Ok(pool) => return Ok(pool),
            Err(err)
                if is_retryable_mysql_connect_error(&err)
                    && started_at.elapsed() < connect_timeout =>
            {
                sleep(mysql_retry_delay(started_at.elapsed(), connect_timeout)).await;
            }
            Err(err) => return Err(err),
        }
    }
}

fn is_retryable_mysql_connect_error(err: &sqlx::Error) -> bool {
    matches!(err, sqlx::Error::Io(_))
}

fn mysql_retry_delay(elapsed: Duration, connect_timeout: Duration) -> Duration {
    connect_timeout
        .checked_sub(elapsed)
        .map(|remaining| remaining.min(MYSQL_CONNECT_RETRY_DELAY))
        .unwrap_or_default()
}

fn mysql_connection_error_details(cfg: &ConnectionConfig) -> String {
    format!(
        "host={}, username={}, port={}, password_length={}",
        cfg.host, cfg.username, cfg.port, cfg.password.len()
    )
}

fn format_duration_seconds(duration: Duration) -> String {
    format!("{:.3}s", duration.as_secs_f64())
}

async fn timeout_step<T, F>(duration: Duration, label: String, future: F) -> Result<T>
where
    F: Future<Output = T>,
{
    let started_at = Instant::now();
    match timeout(duration, future).await {
        Ok(result) => Ok(result),
        Err(_) => {
            let waited = format_duration_seconds(started_at.elapsed());
            let configured_timeout = format_duration_seconds(duration);
            let stage = ipc_diagnostics::current_test_connection_stage()
                .unwrap_or_else(|| "unknown".to_string());
            let checks = ipc_diagnostics::recommended_checks_for_stage(&stage);
            Err(anyhow!(
                "timed out after waiting {waited} while trying to {label} (configured timeout: {configured_timeout}, stage: {stage}). Recommended next checks: {checks}"
            ))
        }
    }
}

fn build_mysql_connect_options(cfg: &ConnectionConfig) -> Result<MySqlConnectOptions> {
    let port = u16::try_from(cfg.port).map_err(|_| anyhow!("invalid mysql port {}", cfg.port))?;
    let mut options = MySqlConnectOptions::new()
        .host(&cfg.host)
        .port(port)
        .username(&cfg.username)
        .password(&cfg.password);
    let database = cfg.database.trim();
    if !database.is_empty() {
        options = options.database(database);
    }

    let ssl_ca_path = resolve_ssl_ca_path(cfg.ssl_ca_path.trim())?;
    if cfg.uses_aws_iam_auth() {
        options = options.enable_cleartext_plugin(true);
        options = if let Some(path) = ssl_ca_path.as_deref() {
            options.ssl_mode(MySqlSslMode::VerifyIdentity).ssl_ca(path)
        } else {
            options.ssl_mode(MySqlSslMode::VerifyIdentity)
        };
    } else {
        options = if let Some(path) = ssl_ca_path.as_deref() {
            options.ssl_mode(MySqlSslMode::VerifyCa).ssl_ca(path)
        } else {
            options.ssl_mode(MySqlSslMode::Preferred)
        };
    }

    Ok(options)
}

fn resolve_ssl_ca_path(raw: &str) -> Result<Option<String>> {
    if raw.is_empty() {
        return Ok(None);
    }

    let expanded = expand_home_dir(raw)?;
    if !expanded.is_file() {
        return Err(anyhow!("TLS CA bundle not found at {}", expanded.display()));
    }

    Ok(Some(expanded.to_string_lossy().to_string()))
}

fn expand_home_dir(path: &str) -> Result<PathBuf> {
    if path == "~" || path.starts_with("~/") {
        let home = dirs::home_dir().ok_or_else(|| anyhow!("unable to resolve home directory"))?;
        let suffix = path.trim_start_matches('~').trim_start_matches('/');
        return Ok(home.join(suffix));
    }

    Ok(PathBuf::from(path))
}

fn max_connections_for(cfg: &ConnectionConfig) -> u32 {
    if cfg.uses_aws_iam_auth() {
        IAM_MAX_CONNECTIONS
    } else {
        DEFAULT_MAX_CONNECTIONS
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

async fn validate_mysql_connection_access(pool: &MySqlPool) -> Result<()> {
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

fn start_port_forward(cfg: &ConnectionConfig) -> Result<PortForwardProcess> {
    ensure_local_port_available(cfg.kube_local_port)?;
    let mut args = Vec::new();
    if !cfg.kube_context.is_empty() {
        args.push(format!("--context={}", cfg.kube_context));
    }
    args.push("port-forward".to_string());
    args.push("--address=127.0.0.1".to_string());
    if !cfg.kube_namespace.is_empty() {
        args.push("-n".to_string());
        args.push(cfg.kube_namespace.clone());
    }
    args.push(cfg.kube_resource.clone());
    args.push(format!("{}:{}", cfg.kube_local_port, cfg.kube_remote_port));

    let child = Command::new(kubectl_program(cfg)?)
        .args(args)
        .env("PATH", kubectl_child_path(cfg)?)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context("kubectl port-forward")?;
    PortForwardProcess::capture(child)
}

fn ensure_local_port_available(port: i32) -> Result<()> {
    ensure_local_port_available_with(port, |port_number| {
        TcpListener::bind(("127.0.0.1", port_number)).map(drop)
    })
}

fn ensure_local_port_available_with(
    port: i32,
    bind: impl FnOnce(u16) -> io::Result<()>,
) -> Result<()> {
    let port_number = u16::try_from(port)
        .ok()
        .filter(|port| *port > 0)
        .ok_or_else(|| anyhow!("invalid local port {port}"))?;
    bind(port_number).with_context(|| format!("local port {port} is already in use"))
}

fn kubectl_program(cfg: &ConnectionConfig) -> Result<PathBuf> {
    Ok(if cfg.kubectl_path.trim().is_empty() {
        PathBuf::from("kubectl")
    } else {
        expand_home_dir(cfg.kubectl_path.trim())?
    })
}

fn kubectl_child_path(cfg: &ConnectionConfig) -> Result<OsString> {
    kubectl_child_path_from(cfg, std::env::var_os("PATH").as_deref())
}

fn kubectl_child_path_from(
    cfg: &ConnectionConfig,
    current_path: Option<&OsStr>,
) -> Result<OsString> {
    let mut paths = Vec::new();
    let program = kubectl_program(cfg)?;
    if program.is_absolute() {
        if let Some(parent) = program.parent() {
            paths.push(parent.to_path_buf());
        }
    }
    if let Some(current_path) = current_path {
        paths.extend(std::env::split_paths(current_path));
    }
    if cfg!(target_os = "macos") {
        paths.push(PathBuf::from("/usr/local/bin"));
        paths.push(PathBuf::from("/opt/homebrew/bin"));
    }
    if let Some(home) = dirs::home_dir() {
        paths.push(home.join(".rd/bin"));
        paths.push(home.join(".local/bin"));
    }
    paths.dedup();
    std::env::join_paths(paths).context("build kubectl child PATH")
}

async fn wait_for_local_port(
    process: &mut PortForwardProcess,
    port: i32,
    wait_timeout: Duration,
    cancel: Option<&CancellationToken>,
) -> Result<()> {
    let addr: SocketAddr = format!("127.0.0.1:{port}").parse()?;
    let deadline = Instant::now() + wait_timeout;

    while Instant::now() < deadline {
        if let Some(status) = process
            .child
            .try_wait()
            .context("check kubectl port-forward")?
        {
            process.wait_for_stderr().await;
            let stderr = process.stderr_text();
            let detail = if stderr.is_empty() {
                format!("kubectl port-forward exited with {status}")
            } else {
                format!("kubectl port-forward exited with {status}: {stderr}")
            };
            return Err(anyhow!(detail));
        }
        let connect = timeout(Duration::from_millis(200), TcpStream::connect(addr));
        let connected = if let Some(cancel) = cancel {
            tokio::select! {
                _ = cancel.cancelled() => return Err(anyhow!("connection test cancelled")),
                result = connect => result.is_ok_and(|result| result.is_ok()),
            }
        } else {
            connect.await.is_ok_and(|result| result.is_ok())
        };
        if connected {
            return Ok(());
        }
        sleep(Duration::from_millis(100)).await;
    }

    let stderr = process.stderr_text();
    if stderr.is_empty() {
        Err(anyhow!(
            "port {port} not ready after {}s",
            wait_timeout.as_secs()
        ))
    } else {
        Err(anyhow!(
            "port {port} not ready after {}s: {stderr}",
            wait_timeout.as_secs()
        ))
    }
}

fn kill_port_forward(process: &mut Option<PortForwardProcess>) {
    *process = None;
}

#[cfg(test)]
mod tests {
    use super::{
        build_dsn, build_mysql_connect_options, build_sqlite_file_dsn,
        ensure_local_port_available_with, format_duration_seconds,
        is_retryable_mysql_connect_error, kubectl_child_path_from, kubectl_program,
        mysql_connection_error_details, mysql_retry_delay, timeout_step,
        validate_connection_access, validate_connection_config, wait_for_local_port,
        PortForwardProcess, MYSQL_CONNECT_RETRY_DELAY, MYSQL_CONNECT_TIMEOUT,
    };
    use crate::models::ConnectionConfig;
    use anyhow::anyhow;
    use sqlx::any::AnyPoolOptions;
    use sqlx::mysql::MySqlSslMode;
    use std::ffi::OsStr;
    use std::io;
    use std::process::{Command, Stdio};
    use std::time::{Duration, Instant};
    use tokio_util::sync::CancellationToken;

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

    #[test]
    fn kubectl_program_uses_path_lookup_when_unconfigured() {
        assert_eq!(
            kubectl_program(&ConnectionConfig::default()).expect("kubectl program"),
            std::path::PathBuf::from("kubectl")
        );
    }

    #[test]
    fn kubectl_program_uses_configured_executable() {
        let cfg = ConnectionConfig {
            kubectl_path: "/opt/tools/kubectl".to_string(),
            ..ConnectionConfig::default()
        };

        assert_eq!(
            kubectl_program(&cfg).expect("kubectl program"),
            std::path::PathBuf::from("/opt/tools/kubectl")
        );
    }

    #[test]
    fn kubectl_child_path_includes_configured_program_directory() {
        let cfg = ConnectionConfig {
            kubectl_path: "/opt/tools/kubectl".to_string(),
            ..ConnectionConfig::default()
        };
        let path = kubectl_child_path_from(&cfg, Some(OsStr::new("/usr/bin:/bin")))
            .expect("kubectl child PATH");
        let entries = std::env::split_paths(&path).collect::<Vec<_>>();

        assert!(entries.contains(&std::path::PathBuf::from("/opt/tools")));
        if cfg!(target_os = "macos") {
            assert!(entries.contains(&std::path::PathBuf::from("/usr/local/bin")));
            assert!(entries.contains(&std::path::PathBuf::from("/opt/homebrew/bin")));
        }
    }

    #[test]
    fn port_forward_rejects_a_local_port_that_is_already_in_use() {
        let err = ensure_local_port_available_with(18_105, |_| {
            Err(io::Error::new(io::ErrorKind::AddrInUse, "occupied"))
        })
        .expect_err("occupied port should fail");

        assert!(err.to_string().contains("already in use"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn port_forward_wait_reports_child_stderr_without_waiting_for_timeout() {
        let child = Command::new("/bin/sh")
            .args(["-c", "echo credential helper missing >&2; exit 7"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn failing child");
        let mut process = PortForwardProcess::capture(child).expect("capture child stderr");
        let started = Instant::now();

        let err = wait_for_local_port(&mut process, 1, Duration::from_secs(30), None)
            .await
            .expect_err("exited child should fail readiness");

        assert!(started.elapsed() < Duration::from_secs(2));
        assert!(err.to_string().contains("credential helper missing"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn port_forward_wait_can_be_cancelled() {
        let child = Command::new("/bin/sh")
            .args(["-c", "exec sleep 30"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn waiting child");
        let mut process = PortForwardProcess::capture(child).expect("capture child stderr");
        let cancel = CancellationToken::new();
        cancel.cancel();

        let err = wait_for_local_port(&mut process, 1, Duration::from_secs(30), Some(&cancel))
            .await
            .expect_err("cancelled wait should fail");

        assert!(err.to_string().contains("connection test cancelled"));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn port_forward_timeout_includes_buffered_stderr() {
        let child = Command::new("/bin/sh")
            .args(["-c", "echo waiting for pod >&2; exec sleep 30"])
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn waiting child");
        let mut process = PortForwardProcess::capture(child).expect("capture child stderr");

        let err = wait_for_local_port(&mut process, 1, Duration::from_millis(300), None)
            .await
            .expect_err("readiness should time out");

        assert!(err.to_string().contains("waiting for pod"));
    }

    #[test]
    fn mysql_iam_requires_region() {
        let cfg = ConnectionConfig {
            driver: "mysql".to_string(),
            host: "db.example.us-east-1.rds.amazonaws.com".to_string(),
            port: 3306,
            username: "app_user".to_string(),
            database: "app".to_string(),
            auth_mode: "awsIam".to_string(),
            ..ConnectionConfig::default()
        };

        let err = validate_connection_config(&cfg).expect_err("region should be required");
        assert!(err.to_string().contains("AWS region is required"));
    }

    #[test]
    fn mysql_password_auth_allows_empty_database() {
        let cfg = ConnectionConfig {
            driver: "mysql".to_string(),
            host: "localhost".to_string(),
            port: 3306,
            username: "app_user".to_string(),
            password: "secret".to_string(),
            ..ConnectionConfig::default()
        };

        validate_connection_config(&cfg).expect("database should be optional for password auth");
        let dsn = build_dsn(&cfg).expect("dsn should build without database");
        assert!(dsn.starts_with("mysql://app_user:"));
        assert!(!dsn.contains("@localhost:3306/"));
    }

    #[test]
    fn mysql_iam_auth_allows_empty_database() {
        let cfg = ConnectionConfig {
            driver: "mysql".to_string(),
            host: "db.example.eu-west-1.rds.amazonaws.com".to_string(),
            port: 3306,
            username: "app_user".to_string(),
            auth_mode: "awsIam".to_string(),
            aws_region: "eu-west-1".to_string(),
            ..ConnectionConfig::default()
        };

        validate_connection_config(&cfg).expect("database should be optional for IAM auth");
        let options =
            build_mysql_connect_options(&cfg).expect("options should build without database");
        assert_eq!(options.get_host(), cfg.host);
        assert!(options.get_database().is_none());
        assert!(options.get_socket().is_none());
    }

    #[test]
    fn mysql_connection_error_details_include_expected_password_detail() {
        let cfg = ConnectionConfig {
            driver: "mysql".to_string(),
            host: "db.example.com".to_string(),
            port: 3306,
            username: "app_user".to_string(),
            password: "super-secret".to_string(),
            ..ConnectionConfig::default()
        };

        let details = mysql_connection_error_details(&cfg);

        assert_eq!(
            details,
            "host=db.example.com, username=app_user, port=3306, password_length=12"
        );
        assert!(!details.contains("super-secret"));
    }

    #[test]
    fn mysql_connection_timeout_is_sixty_seconds() {
        assert_eq!(MYSQL_CONNECT_TIMEOUT, Duration::from_secs(60));
    }

    #[test]
    fn duration_format_includes_millisecond_precision() {
        assert_eq!(
            format_duration_seconds(Duration::from_millis(1_234)),
            "1.234s"
        );
    }

    #[test]
    fn mysql_io_connect_errors_are_retryable() {
        let err = sqlx::Error::Io(io::Error::new(
            io::ErrorKind::AddrNotAvailable,
            "failed to lookup address information",
        ));

        assert!(is_retryable_mysql_connect_error(&err));
    }

    #[test]
    fn mysql_database_connect_errors_are_not_retryable() {
        let err = sqlx::Error::InvalidArgument("bad connection settings".to_string());

        assert!(!is_retryable_mysql_connect_error(&err));
    }

    #[test]
    fn mysql_retry_delay_does_not_exceed_remaining_budget() {
        assert_eq!(
            mysql_retry_delay(Duration::from_secs(1), MYSQL_CONNECT_TIMEOUT),
            MYSQL_CONNECT_RETRY_DELAY
        );
        assert_eq!(
            mysql_retry_delay(
                MYSQL_CONNECT_TIMEOUT - Duration::from_millis(20),
                MYSQL_CONNECT_TIMEOUT
            ),
            Duration::from_millis(20)
        );
    }

    #[test]
    fn mysql_iam_enforces_strict_tls() {
        let cfg = ConnectionConfig {
            driver: "mysql".to_string(),
            host: "db.example.us-east-1.rds.amazonaws.com".to_string(),
            port: 3306,
            username: "app_user".to_string(),
            password: "token".to_string(),
            database: "app".to_string(),
            auth_mode: "awsIam".to_string(),
            aws_region: "us-east-1".to_string(),
            ..ConnectionConfig::default()
        };

        let options = build_mysql_connect_options(&cfg).expect("mysql options should build");
        assert!(matches!(
            options.get_ssl_mode(),
            MySqlSslMode::VerifyIdentity
        ));
        assert!(build_dsn(&cfg)
            .expect("dsn should build")
            .contains("ssl-mode=VERIFY_IDENTITY"));
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

    #[tokio::test]
    async fn timeout_step_returns_timeout_error() {
        let err = timeout_step(
            Duration::from_millis(1),
            "simulate stall".to_string(),
            async {
                tokio::time::sleep(Duration::from_millis(50)).await;
                Ok::<(), anyhow::Error>(())
            },
        )
        .await
        .expect_err("timeout should fail");

        assert!(err.to_string().contains("timed out after waiting "));
        assert!(err.to_string().contains("while trying to simulate stall"));
        assert!(err.to_string().contains("configured timeout: 0.001s"));
    }

    #[tokio::test]
    async fn timeout_step_preserves_underlying_error() {
        let result = timeout_step(Duration::from_secs(1), "fail fast".to_string(), async {
            Err::<(), anyhow::Error>(anyhow!("boom"))
        })
        .await
        .expect("timeout wrapper should return the inner result");

        let err = result.expect_err("underlying error should be returned");

        assert!(err.to_string().contains("boom"));
    }
}
