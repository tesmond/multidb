use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    pub id: String,
    pub name: String,
    pub driver: String,
    pub tab_color: String,
    pub tab_text_black: bool,
    pub host: String,
    pub port: i32,
    pub username: String,
    pub password: String,
    pub database: String,
    pub dsn: String,
    pub use_kube_port_forward: bool,
    pub kube_context: String,
    pub kube_namespace: String,
    pub kube_resource: String,
    pub kube_local_port: i32,
    pub kube_remote_port: i32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ExecuteResult {
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub rows_affected: i64,
    pub duration: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Column {
    pub name: String,
    #[serde(rename = "type")]
    pub column_type: String,
    pub nullable: bool,
    pub default: String,
    pub key: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipTableRef {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_name: String,
    pub table_name: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RelationshipColumnPair {
    pub source_column: String,
    pub target_column: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Relationship {
    pub constraint_name: String,
    pub source_table: RelationshipTableRef,
    pub target_table: RelationshipTableRef,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub column_pairs: Vec<RelationshipColumnPair>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_update: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_delete: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Table {
    pub name: String,
    #[serde(rename = "type")]
    pub table_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub columns: Vec<Column>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Schema {
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    pub tables: Vec<Table>,
    pub views: Vec<Table>,
    pub indexes: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaTree {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<i64>,
    pub tables: Vec<Table>,
    pub views: Vec<Table>,
    pub indexes: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub relationships: Vec<Relationship>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub schemas: Vec<Schema>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SchemaCacheEntry {
    pub schema_json: String,
    pub last_refreshed_at: String,
    pub hash: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryRecord {
    pub id: i64,
    pub conn_id: String,
    pub query: String,
    pub duration: i64,
    pub result_count: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedQuery {
    pub id: i64,
    pub conn_id: String,
    pub title: String,
    pub query: String,
    pub created_at: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DatabaseConnection {
    pub id: String,
    pub user: String,
    pub database: String,
    pub client: String,
    pub state: String,
    pub opened_at: String,
    pub last_active_at: String,
    pub most_recent_command: String,
    pub can_terminate: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStreamMeta {
    pub query_id: String,
    pub columns: Vec<String>,
    pub column_types: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStreamChunk {
    pub query_id: String,
    pub rows: Vec<Vec<Value>>,
    pub offset: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct QueryStreamDone {
    pub query_id: String,
    pub total_rows: usize,
    pub rows_affected: i64,
    pub duration: i64,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub error: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TableBackupPayload {
    pub driver: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub schema_name: String,
    pub table_name: String,
    pub create_sql: String,
    pub indexes_sql: Vec<String>,
    pub columns: Vec<String>,
    pub rows: Vec<serde_json::Map<String, Value>>,
    pub created_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TableBackupArchive {
    pub version: i32,
    pub table: TableBackupPayload,
}
