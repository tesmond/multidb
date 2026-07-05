use crate::{
    backup,
    models::{
        ConnectionConfig, DatabaseConnection, ExecuteResult, QueryRecord, QueryStreamChunk,
        QueryStreamDone, QueryStreamMeta, SavedQuery, SchemaCacheEntry, SchemaTree,
    },
    queries, schema,
    state::AppState,
};
use anyhow::{anyhow, Context, Result};
use futures_util::StreamExt;
use serde::Serialize;
use sqlx::{Column, Row, TypeInfo};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tokio_util::sync::CancellationToken;

pub type CommandResult<T> = std::result::Result<T, String>;
pub type EventEmitter = Arc<dyn Fn(&str, serde_json::Value) + Send + Sync + 'static>;

fn command_err(err: impl Into<anyhow::Error>) -> String {
    let err: anyhow::Error = err.into();
    err.chain()
        .map(|cause| cause.to_string())
        .collect::<Vec<_>>()
        .join(": ")
}

pub async fn save_and_connect(state: Arc<AppState>, cfg: ConnectionConfig) -> CommandResult<()> {
    state
        .connections
        .connect(cfg.clone())
        .await
        .map_err(command_err)?;
    let store = state.store().await.map_err(command_err)?;
    store.save_connection(&cfg).await.map_err(command_err)?;
    Ok(())
}

pub async fn test_connection(state: Arc<AppState>, cfg: ConnectionConfig) -> CommandResult<()> {
    state
        .connections
        .test_connection(cfg)
        .await
        .map_err(command_err)
}

pub async fn disconnect(state: Arc<AppState>, id: String) -> CommandResult<()> {
    let result = state.connections.disconnect(&id).await;
    let store = state.store().await.map_err(command_err)?;
    let _ = store.delete_connection(&id).await;
    let _ = store.delete_schema(&id).await;
    result.map_err(command_err)
}

pub async fn list_connections(state: Arc<AppState>) -> CommandResult<Vec<ConnectionConfig>> {
    Ok(state.connections.list_connections().await)
}

pub async fn list_saved_connections(state: Arc<AppState>) -> CommandResult<Vec<ConnectionConfig>> {
    let store = state.store().await.map_err(command_err)?;
    store.list_saved_connections().await.map_err(command_err)
}

pub async fn execute_query(
    state: Arc<AppState>,
    conn_id: String,
    query_id: String,
    query: String,
    max_rows: i64,
) -> CommandResult<ExecuteResult> {
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;
    let cancel = CancellationToken::new();
    state
        .query_cancels
        .lock()
        .await
        .insert(query_id.clone(), cancel.clone());

    let result = if cfg.driver == "postgres" {
        let pool = state
            .get_pg_pool_or_reconnect(&conn_id)
            .await
            .map_err(command_err)?;
        queries::execute_postgres(&pool, &query, max_rows, cancel).await
    } else if cfg.driver == "mysql" {
        let pool = state
            .get_mysql_pool_or_reconnect(&conn_id)
            .await
            .map_err(command_err)?;
        queries::execute_mysql(&pool, &query, max_rows, cancel).await
    } else {
        let pool = state
            .get_pool_or_reconnect(&conn_id)
            .await
            .map_err(command_err)?;
        queries::execute(&pool, &query, max_rows, cancel).await
    };
    state.query_cancels.lock().await.remove(&query_id);

    if let Ok(store) = state.store().await {
        let _ = store
            .add_query_history(QueryRecord {
                conn_id,
                query,
                duration: result.duration,
                result_count: result.rows.len() as i64,
                error: result.error.clone(),
                ..QueryRecord::default()
            })
            .await;
    }

    Ok(result)
}

pub async fn cancel_query(state: Arc<AppState>, query_id: String) -> CommandResult<()> {
    if let Some(cancel) = state.query_cancels.lock().await.get(&query_id) {
        cancel.cancel();
    }
    Ok(())
}

pub async fn list_database_connections(
    state: Arc<AppState>,
    conn_id: String,
) -> CommandResult<Vec<DatabaseConnection>> {
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;

    match cfg.driver.as_str() {
        "postgres" => list_postgres_connections(state, conn_id)
            .await
            .map_err(command_err),
        "mysql" => list_mysql_connections(state, conn_id)
            .await
            .map_err(command_err),
        "sqlite" => Ok(Vec::new()),
        other => Err(format!(
            "unsupported driver for connection management: {other}"
        )),
    }
}

pub async fn terminate_database_connection(
    state: Arc<AppState>,
    conn_id: String,
    connection_id: String,
) -> CommandResult<()> {
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;

    match cfg.driver.as_str() {
        "postgres" => terminate_postgres_connection(state, conn_id, connection_id)
            .await
            .map_err(command_err),
        "mysql" => terminate_mysql_connection(state, conn_id, connection_id)
            .await
            .map_err(command_err),
        "sqlite" => Err("SQLite does not expose server-side database connections".to_string()),
        other => Err(format!(
            "unsupported driver for connection management: {other}"
        )),
    }
}

pub async fn execute_query_streamed(
    emitter: EventEmitter,
    state: Arc<AppState>,
    conn_id: String,
    query_id: String,
    query: String,
    max_rows: i64,
) -> CommandResult<()> {
    let cfg = match state.get_config_or_saved(&conn_id).await {
        Ok(cfg) => cfg,
        Err(err) => {
            emit_done(
                &emitter,
                QueryStreamDone {
                    query_id,
                    total_rows: 0,
                    rows_affected: 0,
                    duration: 0,
                    error: err.to_string(),
                },
            )
            .map_err(command_err)?;
            return Ok(());
        }
    };

    let max_rows = if max_rows <= 0 {
        10_000_000
    } else {
        max_rows as usize
    };
    let cancel = CancellationToken::new();
    state
        .query_cancels
        .lock()
        .await
        .insert(query_id.clone(), cancel.clone());

    let start = Instant::now();
    let done = if cfg.driver == "postgres" {
        match state.get_pg_pool_or_reconnect(&conn_id).await {
            Ok(pool) => {
                if queries::looks_like_row_returning_query(&query) {
                    stream_postgres_rows(
                        &emitter,
                        &pool,
                        &query_id,
                        &query,
                        max_rows,
                        cancel.clone(),
                        start,
                    )
                    .await
                } else {
                    let result = queries::execute_postgres_non_query(&pool, &query).await;
                    Ok(QueryStreamDone {
                        query_id: query_id.clone(),
                        total_rows: 0,
                        rows_affected: result.rows_affected,
                        duration: result.duration,
                        error: result.error,
                    })
                }
            }
            Err(err) => Err(err),
        }
    } else if cfg.driver == "mysql" {
        match state.get_mysql_pool_or_reconnect(&conn_id).await {
            Ok(pool) => {
                if queries::looks_like_row_returning_query(&query) {
                    stream_mysql_rows(
                        &emitter,
                        &pool,
                        &query_id,
                        &query,
                        max_rows,
                        cancel.clone(),
                        start,
                    )
                    .await
                } else {
                    let result = queries::execute_mysql_non_query(&pool, &query).await;
                    Ok(QueryStreamDone {
                        query_id: query_id.clone(),
                        total_rows: 0,
                        rows_affected: result.rows_affected,
                        duration: result.duration,
                        error: result.error,
                    })
                }
            }
            Err(err) => Err(err),
        }
    } else {
        match state.get_pool_or_reconnect(&conn_id).await {
            Ok(pool) => {
                if queries::looks_like_row_returning_query(&query) {
                    stream_rows(
                        &emitter,
                        &pool,
                        &query_id,
                        &query,
                        max_rows,
                        cancel.clone(),
                        start,
                    )
                    .await
                } else {
                    let result = queries::execute_non_query(&pool, &query).await;
                    Ok(QueryStreamDone {
                        query_id: query_id.clone(),
                        total_rows: 0,
                        rows_affected: result.rows_affected,
                        duration: result.duration,
                        error: result.error,
                    })
                }
            }
            Err(err) => Err(err),
        }
    };

    state.query_cancels.lock().await.remove(&query_id);

    let done = match done {
        Ok(done) => done,
        Err(err) => QueryStreamDone {
            query_id: query_id.clone(),
            total_rows: 0,
            rows_affected: 0,
            duration: queries::elapsed_ms(start),
            error: err.to_string(),
        },
    };

    if let Ok(store) = state.store().await {
        let _ = store
            .add_query_history(QueryRecord {
                conn_id,
                query,
                duration: done.duration,
                result_count: done.total_rows as i64,
                error: done.error.clone(),
                ..QueryRecord::default()
            })
            .await;
    }

    emit_done(&emitter, done).map_err(command_err)?;
    Ok(())
}

async fn list_postgres_connections(
    state: Arc<AppState>,
    conn_id: String,
) -> Result<Vec<DatabaseConnection>> {
    let pool = state.get_pg_pool_or_reconnect(&conn_id).await?;
    let rows = sqlx::query(
        r#"
        SELECT
            pid::text AS id,
            COALESCE(usename, '') AS user_name,
            COALESCE(datname, '') AS database_name,
            COALESCE(client_addr::text, '') AS client,
            COALESCE(state, '') AS state,
            COALESCE(to_char(backend_start, 'YYYY-MM-DD HH24:MI:SS TZ'), '') AS opened_at,
            COALESCE(to_char(state_change, 'YYYY-MM-DD HH24:MI:SS TZ'), '') AS last_active_at,
            COALESCE(query, '') AS most_recent_command,
            pid <> pg_backend_pid() AS can_terminate
        FROM pg_stat_activity
        WHERE datname = current_database()
        ORDER BY backend_start DESC NULLS LAST
        "#,
    )
    .fetch_all(&pool)
    .await?;

    Ok(rows
        .into_iter()
        .map(|row| DatabaseConnection {
            id: row.try_get("id").unwrap_or_default(),
            user: row.try_get("user_name").unwrap_or_default(),
            database: row.try_get("database_name").unwrap_or_default(),
            client: row.try_get("client").unwrap_or_default(),
            state: row.try_get("state").unwrap_or_default(),
            opened_at: row.try_get("opened_at").unwrap_or_default(),
            last_active_at: row.try_get("last_active_at").unwrap_or_default(),
            most_recent_command: row.try_get("most_recent_command").unwrap_or_default(),
            can_terminate: row.try_get("can_terminate").unwrap_or(false),
        })
        .collect())
}

async fn list_mysql_connections(
    state: Arc<AppState>,
    conn_id: String,
) -> Result<Vec<DatabaseConnection>> {
    let pool = state.get_mysql_pool_or_reconnect(&conn_id).await?;
    let current_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
        .fetch_one(&pool)
        .await?;
    let rows = sqlx::query("SHOW FULL PROCESSLIST")
        .fetch_all(&pool)
        .await?;

    Ok(rows
        .into_iter()
        .map(|row| {
            let id = row.try_get::<u64, _>("Id").unwrap_or_default();
            let command = row.try_get::<String, _>("Info").unwrap_or_default();
            let state = row.try_get::<String, _>("State").unwrap_or_default();
            let command_type = row.try_get::<String, _>("Command").unwrap_or_default();
            DatabaseConnection {
                id: id.to_string(),
                user: row.try_get("User").unwrap_or_default(),
                database: row.try_get("db").unwrap_or_default(),
                client: row.try_get("Host").unwrap_or_default(),
                state: if state.is_empty() {
                    command_type
                } else {
                    state
                },
                opened_at: String::new(),
                last_active_at: row
                    .try_get::<i64, _>("Time")
                    .map(|seconds| format!("{seconds}s ago"))
                    .unwrap_or_default(),
                most_recent_command: command,
                can_terminate: id != current_id,
            }
        })
        .collect())
}

async fn terminate_postgres_connection(
    state: Arc<AppState>,
    conn_id: String,
    connection_id: String,
) -> Result<()> {
    let pid: i32 = connection_id
        .parse()
        .with_context(|| format!("invalid postgres connection id {connection_id:?}"))?;
    let pool = state.get_pg_pool_or_reconnect(&conn_id).await?;
    let terminated: bool = sqlx::query_scalar(
        "SELECT CASE WHEN $1 = pg_backend_pid() THEN false ELSE pg_terminate_backend($1) END",
    )
    .bind(pid)
    .fetch_one(&pool)
    .await?;

    if terminated {
        Ok(())
    } else {
        Err(anyhow!("postgres connection {pid} was not terminated"))
    }
}

async fn terminate_mysql_connection(
    state: Arc<AppState>,
    conn_id: String,
    connection_id: String,
) -> Result<()> {
    let target_id: u64 = connection_id
        .parse()
        .with_context(|| format!("invalid mysql connection id {connection_id:?}"))?;
    let pool = state.get_mysql_pool_or_reconnect(&conn_id).await?;
    let current_id: u64 = sqlx::query_scalar("SELECT CONNECTION_ID()")
        .fetch_one(&pool)
        .await?;
    if target_id == current_id {
        return Err(anyhow!(
            "refusing to terminate the current management session"
        ));
    }

    sqlx::query(&format!("KILL {target_id}"))
        .execute(&pool)
        .await?;
    Ok(())
}

async fn stream_mysql_rows(
    emitter: &EventEmitter,
    pool: &sqlx::MySqlPool,
    query_id: &str,
    query: &str,
    max_rows: usize,
    cancel: CancellationToken,
    start: Instant,
) -> Result<QueryStreamDone> {
    const FIRST_CHUNK_SIZE: usize = 500;
    const CHUNK_SIZE: usize = 50_000;

    let mut stream = sqlx::query(query).fetch(pool);
    let mut chunk = Vec::with_capacity(FIRST_CHUNK_SIZE);
    let mut total_rows = 0_usize;
    let mut first_flush = false;
    let mut emitted_meta = false;

    loop {
        if total_rows >= max_rows {
            break;
        }

        let next = tokio::select! {
            _ = cancel.cancelled() => {
                flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
                return Ok(QueryStreamDone {
                    query_id: query_id.to_string(),
                    total_rows,
                    rows_affected: 0,
                    duration: queries::elapsed_ms(start),
                    error: "query cancelled".to_string(),
                });
            }
            row = stream.next() => row,
        };

        let Some(row) = next else { break };
        let row = row?;

        if !emitted_meta {
            let columns = row
                .columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect();
            let column_types = row
                .columns()
                .iter()
                .map(|column| column.type_info().name().to_string())
                .collect();
            emit_payload(
                emitter,
                "query:meta",
                &QueryStreamMeta {
                    query_id: query_id.to_string(),
                    columns,
                    column_types,
                },
            )?;
            emitted_meta = true;
        }

        chunk.push(queries::mysql_row_to_json_values(&row)?);
        total_rows += 1;

        let limit = if first_flush {
            CHUNK_SIZE
        } else {
            FIRST_CHUNK_SIZE
        };
        if chunk.len() >= limit {
            flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

async fn stream_rows(
    emitter: &EventEmitter,
    pool: &sqlx::AnyPool,
    query_id: &str,
    query: &str,
    max_rows: usize,
    cancel: CancellationToken,
    start: Instant,
) -> Result<QueryStreamDone> {
    const FIRST_CHUNK_SIZE: usize = 500;
    const CHUNK_SIZE: usize = 50_000;

    let mut stream = sqlx::query(query).fetch(pool);
    let mut chunk = Vec::with_capacity(FIRST_CHUNK_SIZE);
    let mut total_rows = 0_usize;
    let mut first_flush = false;
    let mut emitted_meta = false;

    loop {
        if total_rows >= max_rows {
            break;
        }

        let next = tokio::select! {
            _ = cancel.cancelled() => {
                flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
                return Ok(QueryStreamDone {
                    query_id: query_id.to_string(),
                    total_rows,
                    rows_affected: 0,
                    duration: queries::elapsed_ms(start),
                    error: "query cancelled".to_string(),
                });
            }
            row = stream.next() => row,
        };

        let Some(row) = next else { break };
        let row = row?;

        if !emitted_meta {
            let columns = row
                .columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect();
            let column_types = row
                .columns()
                .iter()
                .map(|column| column.type_info().name().to_string())
                .collect();
            emit_payload(
                emitter,
                "query:meta",
                &QueryStreamMeta {
                    query_id: query_id.to_string(),
                    columns,
                    column_types,
                },
            )?;
            emitted_meta = true;
        }

        chunk.push(queries::row_to_json_values(&row)?);
        total_rows += 1;

        let limit = if first_flush {
            CHUNK_SIZE
        } else {
            FIRST_CHUNK_SIZE
        };
        if chunk.len() >= limit {
            flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

async fn stream_postgres_rows(
    emitter: &EventEmitter,
    pool: &sqlx::PgPool,
    query_id: &str,
    query: &str,
    max_rows: usize,
    cancel: CancellationToken,
    start: Instant,
) -> Result<QueryStreamDone> {
    const FIRST_CHUNK_SIZE: usize = 500;
    const CHUNK_SIZE: usize = 50_000;

    let mut stream = sqlx::query(query).fetch(pool);
    let mut chunk = Vec::with_capacity(FIRST_CHUNK_SIZE);
    let mut total_rows = 0_usize;
    let mut first_flush = false;
    let mut emitted_meta = false;

    loop {
        if total_rows >= max_rows {
            break;
        }

        let next = tokio::select! {
            _ = cancel.cancelled() => {
                flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
                return Ok(QueryStreamDone {
                    query_id: query_id.to_string(),
                    total_rows,
                    rows_affected: 0,
                    duration: queries::elapsed_ms(start),
                    error: "query cancelled".to_string(),
                });
            }
            row = stream.next() => row,
        };

        let Some(row) = next else { break };
        let row = row?;

        if !emitted_meta {
            let columns = row
                .columns()
                .iter()
                .map(|column| column.name().to_string())
                .collect();
            let column_types = row
                .columns()
                .iter()
                .map(|column| column.type_info().name().to_string())
                .collect();
            emit_payload(
                emitter,
                "query:meta",
                &QueryStreamMeta {
                    query_id: query_id.to_string(),
                    columns,
                    column_types,
                },
            )?;
            emitted_meta = true;
        }

        chunk.push(queries::pg_row_to_json_values(&row)?);
        total_rows += 1;

        let limit = if first_flush {
            CHUNK_SIZE
        } else {
            FIRST_CHUNK_SIZE
        };
        if chunk.len() >= limit {
            flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(emitter, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

fn flush_chunk(
    emitter: &EventEmitter,
    query_id: &str,
    chunk: &mut Vec<Vec<serde_json::Value>>,
    total_rows: usize,
) -> Result<()> {
    if chunk.is_empty() {
        return Ok(());
    }
    let rows = std::mem::take(chunk);
    let offset = total_rows.saturating_sub(rows.len());
    emit_payload(
        emitter,
        "query:chunk",
        &QueryStreamChunk {
            query_id: query_id.to_string(),
            rows,
            offset,
        },
    )?;
    Ok(())
}

fn emit_done(emitter: &EventEmitter, done: QueryStreamDone) -> Result<()> {
    emit_payload(emitter, "query:done", &done)?;
    Ok(())
}

fn emit_payload<T: Serialize>(emitter: &EventEmitter, event: &str, payload: &T) -> Result<()> {
    emitter(event, serde_json::to_value(payload)?);
    Ok(())
}

pub async fn get_table_primary_keys(
    state: Arc<AppState>,
    conn_id: String,
    driver: String,
    schema_name: String,
    table_name: String,
) -> CommandResult<Vec<String>> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    schema::get_primary_keys(&pool, &driver, &schema_name, &table_name)
        .await
        .map_err(command_err)
}

pub async fn get_schema(state: Arc<AppState>, conn_id: String) -> CommandResult<SchemaTree> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;
    schema::get_schema(&pool, &cfg.driver)
        .await
        .map_err(command_err)
}

pub async fn load_schema(state: Arc<AppState>, conn_id: String) -> CommandResult<SchemaCacheEntry> {
    let store = state.store().await.map_err(command_err)?;
    store.load_schema(&conn_id).await.map_err(command_err)
}

pub async fn save_schema(
    state: Arc<AppState>,
    conn_id: String,
    schema_json: String,
    hash: String,
) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store
        .save_schema(&conn_id, &schema_json, &hash)
        .await
        .map_err(command_err)
}

pub async fn backup_table(
    state: Arc<AppState>,
    conn_id: String,
    table_name: String,
    schema_name: String,
) -> CommandResult<()> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;
    let default_name = if schema_name.is_empty() {
        format!("{table_name}.zip")
    } else {
        format!("{schema_name}.{table_name}.zip")
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("Backup Table")
        .set_file_name(default_name)
        .add_filter("Zip Archive", &["zip"])
        .save_file()
    else {
        return Ok(());
    };
    backup::backup_table(&pool, &cfg, &table_name, &schema_name, &path)
        .await
        .map_err(command_err)
}

pub async fn drop_table(
    state: Arc<AppState>,
    conn_id: String,
    table_name: String,
    schema_name: String,
) -> CommandResult<()> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;
    backup::drop_table(&pool, &cfg, &table_name, &schema_name)
        .await
        .map_err(command_err)
}

pub fn select_import_file(import_type: String) -> CommandResult<String> {
    let mut dialog = rfd::FileDialog::new();
    dialog = if import_type == "pgdump" {
        dialog
            .set_title("Select PostgreSQL dump file")
            .add_filter("PostgreSQL Dump", &["sql", "dump", "pgdump", "gz"])
    } else {
        dialog
            .set_title("Select import file")
            .add_filter("Zip Archive", &["zip"])
    };
    Ok(dialog
        .pick_file()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default())
}

pub fn select_sqlite_file() -> CommandResult<String> {
    Ok(rfd::FileDialog::new()
        .set_title("Select SQLite database")
        .add_filter("SQLite Database", &["db", "sqlite", "sqlite3", "db3"])
        .pick_file()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default())
}

pub async fn import_table(
    state: Arc<AppState>,
    conn_id: String,
    import_type: String,
    source_path: String,
) -> CommandResult<()> {
    if source_path.is_empty() {
        return Err("source path not provided".to_string());
    }
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .get_config_or_saved(&conn_id)
        .await
        .map_err(command_err)?;
    backup::import_table(&pool, &cfg, &import_type, &PathBuf::from(source_path))
        .await
        .map_err(command_err)
}

pub fn save_csv(csv_content: String, default_filename: String) -> CommandResult<()> {
    let filename = if default_filename.is_empty() {
        "query_results.csv".to_string()
    } else {
        default_filename
    };
    let Some(path) = rfd::FileDialog::new()
        .set_title("Export CSV")
        .set_file_name(filename)
        .add_filter("CSV File", &["csv"])
        .save_file()
    else {
        return Ok(());
    };
    std::fs::write(&path, csv_content).map_err(command_err)
}

pub fn save_file(path: String, data: Vec<u8>, perm: u32) -> CommandResult<()> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    let path = PathBuf::from(path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(command_err)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, data).map_err(command_err)?;
    #[cfg(not(unix))]
    let _ = perm;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(perm))
            .map_err(command_err)?;
    }
    std::fs::rename(&tmp, &path).map_err(command_err)
}

pub async fn get_query_history(
    state: Arc<AppState>,
    limit: i64,
) -> CommandResult<Vec<QueryRecord>> {
    let store = state.store().await.map_err(command_err)?;
    store.get_query_history(limit).await.map_err(command_err)
}

pub async fn get_query_history_by_conn_id(
    state: Arc<AppState>,
    conn_id: String,
    limit: i64,
) -> CommandResult<Vec<QueryRecord>> {
    let store = state.store().await.map_err(command_err)?;
    store
        .get_query_history_by_conn_id(&conn_id, limit)
        .await
        .map_err(command_err)
}

pub async fn clear_query_history(state: Arc<AppState>) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store.clear_query_history().await.map_err(command_err)
}

pub async fn clear_query_history_by_conn_id(
    state: Arc<AppState>,
    conn_id: String,
) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store
        .clear_query_history_by_conn_id(&conn_id)
        .await
        .map_err(command_err)
}

pub async fn save_query(
    state: Arc<AppState>,
    conn_id: String,
    title: String,
    query: String,
) -> CommandResult<SavedQuery> {
    if conn_id.is_empty() {
        return Err("connection ID is required".to_string());
    }
    if title.is_empty() {
        return Err("title is required".to_string());
    }
    if query.is_empty() {
        return Err("query text is required".to_string());
    }
    let store = state.store().await.map_err(command_err)?;
    store
        .save_query(SavedQuery {
            conn_id,
            title,
            query,
            ..SavedQuery::default()
        })
        .await
        .map_err(command_err)
}

pub async fn get_saved_queries(
    state: Arc<AppState>,
    conn_id: String,
) -> CommandResult<Vec<SavedQuery>> {
    let store = state.store().await.map_err(command_err)?;
    store.get_saved_queries(&conn_id).await.map_err(command_err)
}

pub async fn delete_saved_query(state: Arc<AppState>, id: i64) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store.delete_saved_query(id).await.map_err(command_err)
}

pub async fn update_saved_query_title(
    state: Arc<AppState>,
    id: i64,
    new_title: String,
) -> CommandResult<()> {
    if new_title.is_empty() {
        return Err("title cannot be empty".to_string());
    }
    let store = state.store().await.map_err(command_err)?;
    store
        .update_saved_query_title(id, &new_title)
        .await
        .map_err(command_err)
}
