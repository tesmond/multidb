import { invoke } from "@tauri-apps/api/core";

export function BackupTable(connId, tableName, schemaName) {
  return invoke("backup_table", { connId, tableName, schemaName });
}

export function CancelQuery(queryId) {
  return invoke("cancel_query", { queryId });
}

export function ClearQueryHistory() {
  return invoke("clear_query_history");
}

export function ClearQueryHistoryByConnID(connId) {
  return invoke("clear_query_history_by_conn_id", { connId });
}

export function DeleteSavedQuery(id) {
  return invoke("delete_saved_query", { id });
}

export function Disconnect(id) {
  return invoke("disconnect", { id });
}

export function DropTable(connId, tableName, schemaName) {
  return invoke("drop_table", { connId, tableName, schemaName });
}

export function ExecuteQuery(connId, queryId, query, maxRows) {
  return invoke("execute_query", { connId, queryId, query, maxRows });
}

export function ExecuteQueryStreamed(connId, queryId, query, maxRows) {
  return invoke("execute_query_streamed", { connId, queryId, query, maxRows });
}

export function GetQueryHistory(limit) {
  return invoke("get_query_history", { limit });
}

export function GetQueryHistoryByConnID(connId, limit) {
  return invoke("get_query_history_by_conn_id", { connId, limit });
}

export function GetSavedQueries(connId) {
  return invoke("get_saved_queries", { connId });
}

export function GetSchema(connId) {
  return invoke("get_schema", { connId });
}

export function GetTablePrimaryKeys(connId, driver, schemaName, tableName) {
  return invoke("get_table_primary_keys", {
    connId,
    driver,
    schemaName,
    tableName,
  });
}

export function ImportTable(connId, importType, sourcePath) {
  return invoke("import_table", { connId, importType, sourcePath });
}

export function ListConnections() {
  return invoke("list_connections");
}

export function ListSavedConnections() {
  return invoke("list_saved_connections");
}

export function LoadSchema(connId) {
  return invoke("load_schema", { connId });
}

export function SaveAndConnect(cfg) {
  return invoke("save_and_connect", { cfg });
}

export function SaveCSV(csvContent, defaultFilename) {
  return invoke("save_csv", { csvContent, defaultFilename });
}

export function SaveFile(path, data, perm) {
  return invoke("save_file", { path, data: Array.from(data), perm });
}

export function SaveQuery(connId, title, query) {
  return invoke("save_query", { connId, title, query });
}

export function SaveSchema(connId, schemaJson, hash) {
  return invoke("save_schema", { connId, schemaJson, hash });
}

export function SelectImportFile(importType) {
  return invoke("select_import_file", { importType });
}

export function SelectSqliteFile() {
  return invoke("select_sqlite_file");
}

export function TestConnection(cfg) {
  return invoke("test_connection", { cfg });
}

export function UpdateSavedQueryTitle(id, newTitle) {
  return invoke("update_saved_query_title", { id, newTitle });
}
