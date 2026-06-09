use crate::{
    commands::{self, EventEmitter},
    models::ConnectionConfig,
    state::AppState,
};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;
use std::sync::Arc;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IpcRequest {
    pub id: String,
    pub command: String,
    #[serde(default)]
    pub args: Value,
}

#[derive(Debug, Clone)]
pub enum UserEvent {
    Script(String),
    Exit,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CfgArgs {
    cfg: ConnectionConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IdArgs {
    id: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnIdArgs {
    conn_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DisconnectArgs {
    id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct QueryArgs {
    conn_id: String,
    query_id: String,
    query: String,
    max_rows: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CancelArgs {
    query_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrimaryKeyArgs {
    conn_id: String,
    driver: String,
    schema_name: String,
    table_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveSchemaArgs {
    conn_id: String,
    schema_json: String,
    hash: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct TableArgs {
    conn_id: String,
    table_name: String,
    schema_name: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SelectImportArgs {
    import_type: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImportArgs {
    conn_id: String,
    import_type: String,
    source_path: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveCsvArgs {
    csv_content: String,
    default_filename: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveFileArgs {
    path: String,
    data: Vec<u8>,
    perm: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct LimitArgs {
    limit: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ConnLimitArgs {
    conn_id: String,
    limit: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SaveQueryArgs {
    conn_id: String,
    title: String,
    query: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateSavedQueryTitleArgs {
    id: i64,
    new_title: String,
}

pub async fn dispatch(
    state: Arc<AppState>,
    emitter: EventEmitter,
    command: String,
    args: Value,
) -> Result<Value, String> {
    match command.as_str() {
        "save_and_connect" => {
            let args: CfgArgs = parse(args)?;
            to_value(commands::save_and_connect(state, args.cfg).await?)
        }
        "test_connection" => {
            let args: CfgArgs = parse(args)?;
            to_value(commands::test_connection(state, args.cfg).await?)
        }
        "disconnect" => {
            let args: DisconnectArgs = parse(args)?;
            to_value(commands::disconnect(state, args.id).await?)
        }
        "list_connections" => to_value(commands::list_connections(state).await?),
        "list_saved_connections" => to_value(commands::list_saved_connections(state).await?),
        "execute_query" => {
            let args: QueryArgs = parse(args)?;
            to_value(
                commands::execute_query(
                    state,
                    args.conn_id,
                    args.query_id,
                    args.query,
                    args.max_rows,
                )
                .await?,
            )
        }
        "execute_query_streamed" => {
            let args: QueryArgs = parse(args)?;
            to_value(
                commands::execute_query_streamed(
                    emitter,
                    state,
                    args.conn_id,
                    args.query_id,
                    args.query,
                    args.max_rows,
                )
                .await?,
            )
        }
        "cancel_query" => {
            let args: CancelArgs = parse(args)?;
            to_value(commands::cancel_query(state, args.query_id).await?)
        }
        "get_table_primary_keys" => {
            let args: PrimaryKeyArgs = parse(args)?;
            to_value(
                commands::get_table_primary_keys(
                    state,
                    args.conn_id,
                    args.driver,
                    args.schema_name,
                    args.table_name,
                )
                .await?,
            )
        }
        "get_schema" => {
            let args: ConnIdArgs = parse(args)?;
            to_value(commands::get_schema(state, args.conn_id).await?)
        }
        "load_schema" => {
            let args: ConnIdArgs = parse(args)?;
            to_value(commands::load_schema(state, args.conn_id).await?)
        }
        "save_schema" => {
            let args: SaveSchemaArgs = parse(args)?;
            to_value(commands::save_schema(state, args.conn_id, args.schema_json, args.hash).await?)
        }
        "backup_table" => {
            let args: TableArgs = parse(args)?;
            to_value(
                commands::backup_table(state, args.conn_id, args.table_name, args.schema_name)
                    .await?,
            )
        }
        "drop_table" => {
            let args: TableArgs = parse(args)?;
            to_value(
                commands::drop_table(state, args.conn_id, args.table_name, args.schema_name)
                    .await?,
            )
        }
        "select_import_file" => {
            let args: SelectImportArgs = parse(args)?;
            to_value(commands::select_import_file(args.import_type)?)
        }
        "select_sqlite_file" => to_value(commands::select_sqlite_file()?),
        "import_table" => {
            let args: ImportArgs = parse(args)?;
            to_value(
                commands::import_table(state, args.conn_id, args.import_type, args.source_path)
                    .await?,
            )
        }
        "save_csv" => {
            let args: SaveCsvArgs = parse(args)?;
            to_value(commands::save_csv(args.csv_content, args.default_filename)?)
        }
        "save_file" => {
            let args: SaveFileArgs = parse(args)?;
            to_value(commands::save_file(args.path, args.data, args.perm)?)
        }
        "get_query_history" => {
            let args: LimitArgs = parse(args)?;
            to_value(commands::get_query_history(state, args.limit).await?)
        }
        "get_query_history_by_conn_id" => {
            let args: ConnLimitArgs = parse(args)?;
            to_value(commands::get_query_history_by_conn_id(state, args.conn_id, args.limit).await?)
        }
        "clear_query_history" => to_value(commands::clear_query_history(state).await?),
        "clear_query_history_by_conn_id" => {
            let args: ConnIdArgs = parse(args)?;
            to_value(commands::clear_query_history_by_conn_id(state, args.conn_id).await?)
        }
        "save_query" => {
            let args: SaveQueryArgs = parse(args)?;
            to_value(commands::save_query(state, args.conn_id, args.title, args.query).await?)
        }
        "get_saved_queries" => {
            let args: ConnIdArgs = parse(args)?;
            to_value(commands::get_saved_queries(state, args.conn_id).await?)
        }
        "delete_saved_query" => {
            let args: IdArgs = parse(args)?;
            to_value(commands::delete_saved_query(state, args.id).await?)
        }
        "update_saved_query_title" => {
            let args: UpdateSavedQueryTitleArgs = parse(args)?;
            to_value(commands::update_saved_query_title(state, args.id, args.new_title).await?)
        }
        other => Err(format!("unknown command {other}")),
    }
}

fn parse<T: DeserializeOwned>(args: Value) -> Result<T, String> {
    serde_json::from_value(args).map_err(|err| err.to_string())
}

fn to_value<T: Serialize>(value: T) -> Result<Value, String> {
    serde_json::to_value(value).map_err(|err| err.to_string())
}

pub fn resolve_script(id: &str, value: &Value) -> String {
    let id = js_arg(id);
    let value = serde_json::to_string(value).unwrap_or_else(|_| "null".to_string());
    format!("window.__MULTIDB__ && window.__MULTIDB__.resolve({id}, {value});")
}

pub fn reject_script(id: &str, error: &str) -> String {
    let id = js_arg(id);
    let error = js_arg(error);
    format!("window.__MULTIDB__ && window.__MULTIDB__.reject({id}, {error});")
}

pub fn emit_script(event_name: &str, payload: &Value) -> String {
    let event_name = js_arg(event_name);
    let payload = serde_json::to_string(payload).unwrap_or_else(|_| "null".to_string());
    format!("window.__MULTIDB__ && window.__MULTIDB__.emit({event_name}, {payload});")
}

fn js_arg(value: &str) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| "\"\"".to_string())
}
