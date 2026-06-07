use crate::{
    backup,
    models::{
        ConnectionConfig, ExecuteResult, QueryRecord, QueryStreamChunk, QueryStreamDone,
        QueryStreamMeta, SavedQuery, SchemaCacheEntry, SchemaTree,
    },
    queries, schema,
    state::AppState,
};
use anyhow::Result;
use futures_util::StreamExt;
use sqlx::{Column, Row, TypeInfo};
use std::{path::PathBuf, time::Instant};
use tauri::{AppHandle, Emitter, State};
use tokio_util::sync::CancellationToken;

type CommandResult<T> = std::result::Result<T, String>;

fn command_err(err: impl std::fmt::Display) -> String {
    err.to_string()
}

#[tauri::command]
pub async fn save_and_connect(
    state: State<'_, AppState>,
    cfg: ConnectionConfig,
) -> CommandResult<()> {
    state
        .connections
        .connect(cfg.clone())
        .await
        .map_err(command_err)?;
    let store = state.store().await.map_err(command_err)?;
    store.save_connection(&cfg).await.map_err(command_err)?;
    Ok(())
}

#[tauri::command]
pub async fn test_connection(
    state: State<'_, AppState>,
    cfg: ConnectionConfig,
) -> CommandResult<()> {
    state
        .connections
        .test_connection(cfg)
        .await
        .map_err(command_err)
}

#[tauri::command]
pub async fn disconnect(state: State<'_, AppState>, id: String) -> CommandResult<()> {
    let result = state.connections.disconnect(&id).await;
    let store = state.store().await.map_err(command_err)?;
    let _ = store.delete_connection(&id).await;
    let _ = store.delete_schema(&id).await;
    result.map_err(command_err)
}

#[tauri::command]
pub async fn list_connections(state: State<'_, AppState>) -> CommandResult<Vec<ConnectionConfig>> {
    Ok(state.connections.list_connections().await)
}

#[tauri::command]
pub async fn list_saved_connections(
    state: State<'_, AppState>,
) -> CommandResult<Vec<ConnectionConfig>> {
    let store = state.store().await.map_err(command_err)?;
    store.list_saved_connections().await.map_err(command_err)
}

#[tauri::command]
pub async fn execute_query(
    state: State<'_, AppState>,
    conn_id: String,
    query_id: String,
    query: String,
    max_rows: i64,
) -> CommandResult<ExecuteResult> {
    let cfg = state
        .connections
        .get_config(&conn_id)
        .await
        .ok_or_else(|| "config not found".to_string())?;
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

#[tauri::command]
pub async fn cancel_query(state: State<'_, AppState>, query_id: String) -> CommandResult<()> {
    if let Some(cancel) = state.query_cancels.lock().await.get(&query_id) {
        cancel.cancel();
    }
    Ok(())
}

#[tauri::command]
pub async fn execute_query_streamed(
    app: AppHandle,
    state: State<'_, AppState>,
    conn_id: String,
    query_id: String,
    query: String,
    max_rows: i64,
) -> CommandResult<()> {
    let cfg = match state.connections.get_config(&conn_id).await {
        Some(cfg) => cfg,
        None => {
            emit_done(
                &app,
                QueryStreamDone {
                    query_id,
                    total_rows: 0,
                    rows_affected: 0,
                    duration: 0,
                    error: "config not found".to_string(),
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
                        &app,
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
                        &app,
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
                        &app,
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

    emit_done(&app, done).map_err(command_err)?;
    Ok(())
}

async fn stream_mysql_rows(
    app: &AppHandle,
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
                flush_chunk(app, query_id, &mut chunk, total_rows)?;
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
            app.emit(
                "query:meta",
                QueryStreamMeta {
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
            flush_chunk(app, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(app, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

async fn stream_rows(
    app: &AppHandle,
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
                flush_chunk(app, query_id, &mut chunk, total_rows)?;
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
            app.emit(
                "query:meta",
                QueryStreamMeta {
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
            flush_chunk(app, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(app, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

async fn stream_postgres_rows(
    app: &AppHandle,
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
                flush_chunk(app, query_id, &mut chunk, total_rows)?;
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
            app.emit(
                "query:meta",
                QueryStreamMeta {
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
            flush_chunk(app, query_id, &mut chunk, total_rows)?;
            first_flush = true;
        }
    }

    flush_chunk(app, query_id, &mut chunk, total_rows)?;
    Ok(QueryStreamDone {
        query_id: query_id.to_string(),
        total_rows,
        rows_affected: 0,
        duration: queries::elapsed_ms(start),
        error: String::new(),
    })
}

fn flush_chunk(
    app: &AppHandle,
    query_id: &str,
    chunk: &mut Vec<Vec<serde_json::Value>>,
    total_rows: usize,
) -> Result<()> {
    if chunk.is_empty() {
        return Ok(());
    }
    let rows = std::mem::take(chunk);
    let offset = total_rows.saturating_sub(rows.len());
    app.emit(
        "query:chunk",
        QueryStreamChunk {
            query_id: query_id.to_string(),
            rows,
            offset,
        },
    )?;
    Ok(())
}

fn emit_done(app: &AppHandle, done: QueryStreamDone) -> Result<()> {
    app.emit("query:done", done)?;
    Ok(())
}

#[tauri::command]
pub async fn get_table_primary_keys(
    state: State<'_, AppState>,
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

#[tauri::command]
pub async fn get_schema(state: State<'_, AppState>, conn_id: String) -> CommandResult<SchemaTree> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .connections
        .get_config(&conn_id)
        .await
        .ok_or_else(|| "config not found".to_string())?;
    schema::get_schema(&pool, &cfg.driver)
        .await
        .map_err(command_err)
}

#[tauri::command]
pub async fn load_schema(
    state: State<'_, AppState>,
    conn_id: String,
) -> CommandResult<SchemaCacheEntry> {
    let store = state.store().await.map_err(command_err)?;
    store.load_schema(&conn_id).await.map_err(command_err)
}

#[tauri::command]
pub async fn save_schema(
    state: State<'_, AppState>,
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

#[tauri::command]
pub async fn backup_table(
    state: State<'_, AppState>,
    conn_id: String,
    table_name: String,
    schema_name: String,
) -> CommandResult<()> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .connections
        .get_config(&conn_id)
        .await
        .ok_or_else(|| "config not found".to_string())?;
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

#[tauri::command]
pub async fn drop_table(
    state: State<'_, AppState>,
    conn_id: String,
    table_name: String,
    schema_name: String,
) -> CommandResult<()> {
    let pool = state
        .get_pool_or_reconnect(&conn_id)
        .await
        .map_err(command_err)?;
    let cfg = state
        .connections
        .get_config(&conn_id)
        .await
        .ok_or_else(|| "config not found".to_string())?;
    backup::drop_table(&pool, &cfg, &table_name, &schema_name)
        .await
        .map_err(command_err)
}

#[tauri::command]
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

#[tauri::command]
pub fn select_sqlite_file() -> CommandResult<String> {
    Ok(rfd::FileDialog::new()
        .set_title("Select SQLite database")
        .add_filter("SQLite Database", &["db", "sqlite", "sqlite3", "db3"])
        .pick_file()
        .map(|path| path.to_string_lossy().to_string())
        .unwrap_or_default())
}

#[tauri::command]
pub async fn import_table(
    state: State<'_, AppState>,
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
        .connections
        .get_config(&conn_id)
        .await
        .ok_or_else(|| "config not found".to_string())?;
    backup::import_table(&pool, &cfg, &import_type, &PathBuf::from(source_path))
        .await
        .map_err(command_err)
}

#[tauri::command]
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

#[tauri::command]
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

#[tauri::command]
pub async fn get_query_history(
    state: State<'_, AppState>,
    limit: i64,
) -> CommandResult<Vec<QueryRecord>> {
    let store = state.store().await.map_err(command_err)?;
    store.get_query_history(limit).await.map_err(command_err)
}

#[tauri::command]
pub async fn get_query_history_by_conn_id(
    state: State<'_, AppState>,
    conn_id: String,
    limit: i64,
) -> CommandResult<Vec<QueryRecord>> {
    let store = state.store().await.map_err(command_err)?;
    store
        .get_query_history_by_conn_id(&conn_id, limit)
        .await
        .map_err(command_err)
}

#[tauri::command]
pub async fn clear_query_history(state: State<'_, AppState>) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store.clear_query_history().await.map_err(command_err)
}

#[tauri::command]
pub async fn clear_query_history_by_conn_id(
    state: State<'_, AppState>,
    conn_id: String,
) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store
        .clear_query_history_by_conn_id(&conn_id)
        .await
        .map_err(command_err)
}

#[tauri::command]
pub async fn save_query(
    state: State<'_, AppState>,
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

#[tauri::command]
pub async fn get_saved_queries(
    state: State<'_, AppState>,
    conn_id: String,
) -> CommandResult<Vec<SavedQuery>> {
    let store = state.store().await.map_err(command_err)?;
    store.get_saved_queries(&conn_id).await.map_err(command_err)
}

#[tauri::command]
pub async fn delete_saved_query(state: State<'_, AppState>, id: i64) -> CommandResult<()> {
    let store = state.store().await.map_err(command_err)?;
    store.delete_saved_query(id).await.map_err(command_err)
}

#[tauri::command]
pub async fn update_saved_query_title(
    state: State<'_, AppState>,
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
