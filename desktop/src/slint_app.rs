use crate::{
    commands::{self, EventEmitter},
    editor::{CompletionCandidate, SqlEditorBuffer},
    models::{
        ConnectionConfig, QueryRecord, QueryStreamChunk, QueryStreamDone, QueryStreamMeta,
        SavedQuery, SchemaTree,
    },
    state::AppState,
};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use slint::{Color, ComponentHandle, Model, ModelRc, ModelTracker, SharedString, VecModel};
use std::{
    any::Any,
    rc::Rc,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::runtime::Runtime;

slint::include_modules!();

const SERVER_GROUPS_KEY: &str = "server_groups_v1";
const CONNECTION_ORDER_KEY: &str = "connection_order_v1";
const FONT_SCALE_KEY: &str = "font_scale_percent_v1";

#[derive(Clone)]
struct AppController {
    ui: slint::Weak<MainWindow>,
    runtime: Arc<Runtime>,
    app_state: Arc<AppState>,
    state: Arc<Mutex<UiState>>,
}

#[derive(Default)]
struct UiState {
    connections: Vec<ConnectionConfig>,
    server_groups: Vec<ServerGroup>,
    editing_conn_id: String,
    selected_conn_id: String,
    nav_nodes: Vec<NavNode>,
    tabs: Vec<QueryTab>,
    active_tab_id: String,
    output_tab: String,
    messages: String,
    status: String,
    history: Vec<QueryRecord>,
    saved_queries: Vec<SavedQuery>,
    schema_words: Vec<String>,
    font_scale_percent: i32,
}

struct QueryTab {
    id: String,
    title: String,
    conn_id: String,
    query_id: String,
    running: bool,
    editor: SqlEditorBuffer,
    columns: Vec<String>,
    column_types: Vec<String>,
    rows: Arc<Vec<Vec<Value>>>,
    rows_affected: i64,
    duration: i64,
    error: String,
}

struct NavNode {
    id: String,
    label: String,
    kind: String,
    depth: i32,
    selected: bool,
}

#[derive(Clone, Default, Serialize, Deserialize)]
struct ServerGroup {
    id: String,
    name: String,
    expanded: bool,
    #[serde(default)]
    connection_ids: Vec<String>,
}

impl AppController {
    fn new(ui: &MainWindow, runtime: Arc<Runtime>) -> Self {
        let mut state = UiState {
            output_tab: "results".to_string(),
            status: "Ready".to_string(),
            font_scale_percent: 100,
            ..UiState::default()
        };
        state.tabs.push(QueryTab::new("tab-1", "Query 1"));
        state.active_tab_id = "tab-1".to_string();

        Self {
            ui: ui.as_weak(),
            runtime,
            app_state: Arc::new(AppState::default()),
            state: Arc::new(Mutex::new(state)),
        }
    }

    fn install_callbacks(&self, ui: &MainWindow) {
        let controller = self.clone();
        ui.on_new_tab(move || controller.add_tab(None));

        let controller = self.clone();
        ui.on_new_connection(move || {
            if let Some(ui) = controller.ui.upgrade() {
                controller.clear_connection_dialog(&ui);
                ui.set_connection_dialog_open(true);
            }
        });

        let controller = self.clone();
        ui.on_edit_connection(move |id| controller.edit_connection(id.to_string()));

        let controller = self.clone();
        ui.on_test_connection(move |id| controller.test_connection(id.to_string()));

        let controller = self.clone();
        ui.on_remove_connection(move |id| controller.remove_connection(id.to_string()));

        let controller = self.clone();
        ui.on_close_connection_dialog(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_connection_dialog_open(false);
            }
        });

        let controller = self.clone();
        ui.on_save_connection_dialog(move || controller.save_connection_from_dialog());

        let controller = self.clone();
        ui.on_test_connection_dialog(move || controller.test_connection_from_dialog());

        let controller = self.clone();
        ui.on_pick_sqlite_file(move || controller.pick_sqlite_file());

        let controller = self.clone();
        ui.on_new_server_group(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_dialog_group_name(ss(""));
                ui.set_server_group_dialog_open(true);
            }
        });

        let controller = self.clone();
        ui.on_close_server_group_dialog(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_server_group_dialog_open(false);
            }
        });

        let controller = self.clone();
        ui.on_save_server_group(move || controller.save_server_group_from_dialog());

        let controller = self.clone();
        ui.on_open_settings(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_settings_dialog_open(true);
            }
        });

        let controller = self.clone();
        ui.on_close_settings(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_settings_dialog_open(false);
            }
        });

        let controller = self.clone();
        ui.on_increase_font(move || controller.adjust_font_scale(10));

        let controller = self.clone();
        ui.on_decrease_font(move || controller.adjust_font_scale(-10));

        let controller = self.clone();
        ui.on_editor_text_changed(move |text| controller.set_editor_text(text.to_string()));

        let controller = self.clone();
        ui.on_select_connection(move |id| controller.select_connection(id.to_string()));

        let controller = self.clone();
        ui.on_select_nav_node(move |id| controller.select_nav_node(id.to_string()));

        let controller = self.clone();
        ui.on_select_tab(move |id| controller.select_tab(id.to_string()));

        let controller = self.clone();
        ui.on_close_tab(move |id| controller.close_tab(id.to_string()));

        let controller = self.clone();
        ui.on_reorder_tab(move |id, index| controller.reorder_tab(id.to_string(), index as usize));

        let controller = self.clone();
        ui.on_drag_connection(move |id, y| controller.reorder_connection_drag(id.to_string(), y));

        let controller = self.clone();
        ui.on_editor_key_pressed(move |text, ctrl, shift, alt, meta| {
            controller.handle_editor_key(text.to_string(), ctrl, shift, alt, meta)
        });

        let controller = self.clone();
        ui.on_execute_query(move || controller.execute_active_query());

        let controller = self.clone();
        ui.on_cancel_query(move || controller.cancel_active_query());

        let controller = self.clone();
        ui.on_save_query(move || controller.save_active_query());

        let controller = self.clone();
        ui.on_set_output_tab(move |tab| controller.set_output_tab(tab.to_string()));

        let controller = self.clone();
        ui.on_open_history(move |id| controller.open_history_query(id as i64));

        let controller = self.clone();
        ui.on_open_saved(move |id| controller.open_saved_query(id as i64));

        let controller = self.clone();
        ui.on_refresh_schema(move |id| controller.load_schema(id.to_string()));

        let controller = self.clone();
        ui.on_backup_nav_node(move |id| controller.backup_nav_node(id.to_string()));

        let controller = self.clone();
        ui.on_drop_nav_node(move |id| controller.drop_nav_node(id.to_string()));

        let controller = self.clone();
        ui.on_copy_nav_node_name(move |id| controller.copy_nav_node_name(id.to_string()));
    }

    fn start(&self) {
        self.sync();
        self.load_ui_preferences();
        self.load_saved_connections();
    }

    fn add_tab(&self, sql: Option<String>) {
        let conn_id = {
            let state = self.state.lock().expect("ui state");
            state.selected_conn_id.clone()
        };
        self.add_tab_for_connection(sql, conn_id, None);
    }

    fn add_tab_for_connection(&self, sql: Option<String>, conn_id: String, title: Option<String>) {
        {
            let mut state = self.state.lock().expect("ui state");
            let number = state.tabs.len() + 1;
            let id = format!("tab-{}", now_millis());
            let title = title.unwrap_or_else(|| format!("Query {number}"));
            let mut tab = QueryTab::new(&id, &title);
            tab.conn_id = conn_id.clone();
            if let Some(sql) = sql {
                tab.editor.set_text(&sql);
            }
            if !conn_id.is_empty() {
                state.selected_conn_id = conn_id;
            }
            state.active_tab_id = id;
            state.tabs.push(tab);
            state.status = "New query tab".to_string();
        }
        self.sync();
    }

    fn select_tab(&self, id: String) {
        {
            let mut state = self.state.lock().expect("ui state");
            if state.tabs.iter().any(|tab| tab.id == id) {
                state.active_tab_id = id;
            }
        }
        self.sync();
    }

    fn close_tab(&self, id: String) {
        {
            let mut state = self.state.lock().expect("ui state");
            if state.tabs.len() == 1 {
                return;
            }
            let active = state.active_tab_id == id;
            state.tabs.retain(|tab| tab.id != id);
            if active {
                state.active_tab_id = state
                    .tabs
                    .last()
                    .map(|tab| tab.id.clone())
                    .unwrap_or_default();
            }
        }
        self.sync();
    }

    fn reorder_tab(&self, id: String, target_index: usize) {
        {
            let mut state = self.state.lock().expect("ui state");
            let Some(from) = state.tabs.iter().position(|tab| tab.id == id) else {
                return;
            };
            let tab = state.tabs.remove(from);
            let to = target_index.min(state.tabs.len());
            state.tabs.insert(to, tab);
        }
        self.sync();
    }

    fn reorder_connection_drag(&self, id: String, y: i32) {
        {
            let mut state = self.state.lock().expect("ui state");
            let Some(from) = state.connections.iter().position(|conn| conn.id == id) else {
                return;
            };
            let target = (y / 32).max(0) as usize;
            let conn = state.connections.remove(from);
            let to = target.min(state.connections.len());
            state.connections.insert(to, conn);
        }
        self.persist_connection_order();
        self.sync();
    }

    fn select_connection(&self, id: String) {
        {
            let mut state = self.state.lock().expect("ui state");
            state.selected_conn_id = id.clone();
            state.status = format!("Selected connection {id}");
            if let Some(tab) = active_tab_mut(&mut state) {
                tab.conn_id = id.clone();
            }
        }
        self.sync();
        self.load_schema(id.clone());
        self.load_history_and_saved(id);
    }

    fn select_nav_node(&self, id: String) {
        let sql = {
            let mut state = self.state.lock().expect("ui state");
            for node in &mut state.nav_nodes {
                node.selected = node.id == id;
            }
            if let Some(rest) = id.strip_prefix("table:") {
                let name = rest.rsplit(':').next().unwrap_or(rest);
                Some(format!("SELECT * FROM {name};"))
            } else {
                None
            }
        };
        if let Some(sql) = sql {
            self.add_tab(Some(sql));
        } else {
            self.sync();
        }
    }

    fn backup_nav_node(&self, id: String) {
        let Some((conn_id, schema_name, table_name)) = self.table_parts_from_nav_node(&id) else {
            self.set_status("No table selected for backup");
            return;
        };
        self.set_status(&format!("Backing up {table_name}"));
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result =
                commands::backup_table(app_state, conn_id, table_name.clone(), schema_name).await;
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(_) => controller.set_status(&format!("Backed up {table_name}")),
                Err(err) => controller.set_status(&format!("Backup failed: {err}")),
            });
        });
    }

    fn drop_nav_node(&self, id: String) {
        let Some((conn_id, schema_name, table_name)) = self.table_parts_from_nav_node(&id) else {
            self.set_status("No table selected to drop");
            return;
        };
        self.set_status(&format!("Dropping {table_name}"));
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result =
                commands::drop_table(app_state, conn_id.clone(), table_name.clone(), schema_name)
                    .await;
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(_) => {
                    controller.set_status(&format!("Dropped {table_name}"));
                    controller.load_schema(conn_id);
                }
                Err(err) => controller.set_status(&format!("Drop failed: {err}")),
            });
        });
    }

    fn copy_nav_node_name(&self, id: String) {
        if let Some((_, schema_name, table_name)) = self.table_parts_from_nav_node(&id) {
            let name = if schema_name.is_empty() {
                table_name
            } else {
                format!("{schema_name}.{table_name}")
            };
            self.set_status(&format!("Table name: {name}"));
        }
    }

    fn table_parts_from_nav_node(&self, id: &str) -> Option<(String, String, String)> {
        let conn_id = {
            let state = self.state.lock().expect("ui state");
            state.selected_conn_id.clone()
        };
        if conn_id.is_empty() {
            return None;
        }
        let rest = id.strip_prefix("table:")?;
        let mut parts = rest.split(':').collect::<Vec<_>>();
        if parts.len() >= 2 {
            let table_name = parts.pop()?.to_string();
            let schema_name = parts.pop().unwrap_or_default().to_string();
            Some((conn_id, schema_name, table_name))
        } else {
            Some((
                conn_id,
                String::new(),
                rest.trim_start_matches(':').to_string(),
            ))
        }
    }

    fn handle_editor_key(
        &self,
        text: String,
        ctrl: bool,
        _shift: bool,
        _alt: bool,
        meta: bool,
    ) -> bool {
        let command = ctrl || meta;
        if command {
            match text.as_str() {
                "\n" | "\r" => {
                    self.execute_active_query();
                    return true;
                }
                "s" | "S" => {
                    self.save_active_query();
                    return true;
                }
                "." => {
                    self.cancel_active_query();
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    fn with_active_editor(&self, f: impl FnOnce(&mut SqlEditorBuffer)) {
        {
            let mut state = self.state.lock().expect("ui state");
            if let Some(tab) = active_tab_mut(&mut state) {
                f(&mut tab.editor);
                tab.error.clear();
            }
        }
        self.sync();
    }

    fn set_editor_text(&self, text: String) {
        {
            let mut state = self.state.lock().expect("ui state");
            if let Some(tab) = active_tab_mut(&mut state) {
                if tab.editor.text() != text {
                    tab.editor.set_text(&text);
                    tab.error.clear();
                }
            }
        }
        self.sync();
    }

    fn execute_active_query(&self) {
        let Some((conn_id, query_id, sql)) = ({
            let mut state = self.state.lock().expect("ui state");
            let selected_conn = state.selected_conn_id.clone();
            let Some(tab) = active_tab_mut(&mut state) else {
                return;
            };
            let sql = tab.editor.text().trim().to_string();
            if sql.is_empty() {
                state.status = "Nothing to run".to_string();
                drop(state);
                self.sync();
                return;
            }
            let conn_id = if tab.conn_id.is_empty() {
                selected_conn
            } else {
                tab.conn_id.clone()
            };
            if conn_id.is_empty() {
                state.status = "No connection selected".to_string();
                drop(state);
                self.sync();
                return;
            }
            let query_id = format!("query-{}", now_millis());
            tab.conn_id = conn_id.clone();
            tab.query_id = query_id.clone();
            tab.running = true;
            tab.columns.clear();
            tab.column_types.clear();
            tab.rows = Arc::new(Vec::new());
            tab.error.clear();
            state.output_tab = "results".to_string();
            state.status = "Running query".to_string();
            Some((conn_id, query_id, sql))
        }) else {
            return;
        };

        self.sync();

        let runtime_state = self.app_state.clone();
        let controller = self.clone();
        let emitter: EventEmitter = Arc::new(move |event, payload| {
            controller.handle_query_event(event, payload);
        });
        self.runtime.spawn(async move {
            let result =
                commands::execute_query_streamed(emitter, runtime_state, conn_id, query_id, sql, 0)
                    .await;
            if let Err(err) = result {
                eprintln!("query failed: {err}");
            }
        });
    }

    fn handle_query_event(&self, event: &str, payload: Value) {
        let event = event.to_string();
        let controller = self.clone();
        let _ = slint::invoke_from_event_loop(move || {
            match event.as_str() {
                "query:meta" => {
                    if let Ok(meta) = serde_json::from_value::<QueryStreamMeta>(payload) {
                        controller.apply_query_meta(meta);
                    }
                }
                "query:chunk" => {
                    if let Ok(chunk) = serde_json::from_value::<QueryStreamChunk>(payload) {
                        controller.apply_query_chunk(chunk);
                    }
                }
                "query:done" => {
                    if let Ok(done) = serde_json::from_value::<QueryStreamDone>(payload) {
                        controller.apply_query_done(done);
                    }
                }
                _ => {}
            }
            controller.sync();
        });
    }

    fn apply_query_meta(&self, meta: QueryStreamMeta) {
        let mut state = self.state.lock().expect("ui state");
        if let Some(tab) = tab_by_query_id_mut(&mut state, &meta.query_id) {
            tab.columns = meta.columns;
            tab.column_types = meta.column_types;
            tab.rows = Arc::new(Vec::new());
        }
    }

    fn apply_query_chunk(&self, chunk: QueryStreamChunk) {
        let mut state = self.state.lock().expect("ui state");
        if let Some(tab) = tab_by_query_id_mut(&mut state, &chunk.query_id) {
            Arc::make_mut(&mut tab.rows).extend(chunk.rows);
            state.status = format!("Loading... {} rows", tab.rows.len());
        }
    }

    fn apply_query_done(&self, done: QueryStreamDone) {
        let mut state = self.state.lock().expect("ui state");
        if let Some(tab) = tab_by_query_id_mut(&mut state, &done.query_id) {
            tab.running = false;
            tab.query_id.clear();
            tab.rows_affected = done.rows_affected;
            tab.duration = done.duration;
            tab.error = done.error.clone();
            if done.error.is_empty() {
                if tab.columns.is_empty() {
                    state.status = format!(
                        "{} row(s) affected - {}ms",
                        done.rows_affected, done.duration
                    );
                } else {
                    state.status = format!("{} rows - {}ms", done.total_rows, done.duration);
                }
            } else {
                state.output_tab = "messages".to_string();
                state.status = format!("Error: {}", done.error);
                state.messages = done.error;
            }
        }
    }

    fn cancel_active_query(&self) {
        let query_id = {
            let mut state = self.state.lock().expect("ui state");
            let Some(tab) = active_tab_mut(&mut state) else {
                return;
            };
            if tab.query_id.is_empty() {
                return;
            }
            let query_id = tab.query_id.clone();
            tab.running = false;
            tab.query_id.clear();
            let _ = tab;
            state.status = "Query cancellation requested".to_string();
            query_id
        };
        self.sync();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let _ = commands::cancel_query(app_state, query_id).await;
        });
    }

    fn save_active_query(&self) {
        let Some((conn_id, title, sql)) = ({
            let state = self.state.lock().expect("ui state");
            let Some(tab) = active_tab(&state) else {
                return;
            };
            let sql = tab.editor.text().trim().to_string();
            let conn_id = if tab.conn_id.is_empty() {
                state.selected_conn_id.clone()
            } else {
                tab.conn_id.clone()
            };
            if sql.is_empty() || conn_id.is_empty() {
                drop(state);
                self.set_status("Cannot save without SQL and a connection");
                return;
            }
            Some((conn_id, tab.title.clone(), sql))
        }) else {
            return;
        };
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result = commands::save_query(app_state, conn_id.clone(), title, sql).await;
            let _ = slint::invoke_from_event_loop(move || {
                match result {
                    Ok(_) => controller.set_status("Query saved"),
                    Err(err) => controller.set_status(&format!("Error saving query: {err}")),
                }
                controller.load_history_and_saved(conn_id);
            });
        });
    }

    fn open_history_query(&self, id: i64) {
        let item = {
            let state = self.state.lock().expect("ui state");
            state.history.iter().find(|item| item.id == id).cloned()
        };
        if let Some(item) = item {
            self.add_tab_for_connection(
                Some(item.query.clone()),
                item.conn_id,
                Some(first_line(&item.query).to_string()),
            );
        } else {
            self.set_status("History item not found");
        }
    }

    fn open_saved_query(&self, id: i64) {
        let item = {
            let state = self.state.lock().expect("ui state");
            state
                .saved_queries
                .iter()
                .find(|item| item.id == id)
                .cloned()
        };
        if let Some(item) = item {
            self.add_tab_for_connection(Some(item.query), item.conn_id, Some(item.title));
        } else {
            self.set_status("Saved query not found");
        }
    }

    fn save_connection_from_dialog(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let cfg = self.connection_config_from_dialog(&ui);
        let name = cfg.name.clone();
        if name.trim().is_empty() {
            self.set_status("Connection name is required");
            return;
        }

        ui.set_connection_dialog_open(false);
        let app_state = self.app_state.clone();
        let controller = self.clone();
        self.runtime.spawn(async move {
            let result = commands::save_and_connect(app_state, cfg).await;
            let _ = slint::invoke_from_event_loop(move || {
                match result {
                    Ok(_) => controller.set_status("Connection saved"),
                    Err(err) => controller.set_status(&format!("Error saving connection: {err}")),
                }
                controller.load_saved_connections();
            });
        });
    }

    fn test_connection_from_dialog(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let cfg = self.connection_config_from_dialog(&ui);
        self.test_connection_config(cfg);
    }

    fn edit_connection(&self, id: String) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let cfg = {
            let mut state = self.state.lock().expect("ui state");
            state.editing_conn_id = id.clone();
            state.connections.iter().find(|conn| conn.id == id).cloned()
        };
        let Some(cfg) = cfg else {
            self.set_status("Connection not found");
            return;
        };
        ui.set_dialog_name(ss(&cfg.name));
        ui.set_dialog_driver(ss(&cfg.driver));
        ui.set_dialog_host(ss(&cfg.host));
        ui.set_dialog_port(ss(&port_to_text(cfg.port)));
        ui.set_dialog_username(ss(&cfg.username));
        ui.set_dialog_password(ss(&cfg.password));
        ui.set_dialog_database(ss(&cfg.database));
        ui.set_dialog_dsn(ss(&cfg.dsn));
        ui.set_dialog_tab_color(ss(&cfg.tab_color));
        ui.set_dialog_kube_context(ss(&cfg.kube_context));
        ui.set_dialog_kube_namespace(ss(&cfg.kube_namespace));
        ui.set_dialog_kube_resource(ss(&cfg.kube_resource));
        ui.set_dialog_kube_local_port(ss(&port_to_text(cfg.kube_local_port)));
        ui.set_dialog_kube_remote_port(ss(&port_to_text(cfg.kube_remote_port)));
        ui.set_connection_dialog_open(true);
    }

    fn test_connection(&self, id: String) {
        let cfg = {
            let state = self.state.lock().expect("ui state");
            state.connections.iter().find(|conn| conn.id == id).cloned()
        };
        let Some(cfg) = cfg else {
            self.set_status("Connection not found");
            return;
        };
        self.test_connection_config(cfg);
    }

    fn test_connection_config(&self, cfg: ConnectionConfig) {
        self.set_status("Testing connection");
        let name = cfg.name.clone();
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result = commands::test_connection(app_state, cfg).await;
            let _ = slint::invoke_from_event_loop(move || match result {
                Ok(_) => controller.set_status(&format!("Connection to {name} succeeded")),
                Err(err) => controller.set_status(&format!("Connection failed: {err}")),
            });
        });
    }

    fn remove_connection(&self, id: String) {
        self.set_status("Removing connection");
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result = commands::disconnect(app_state, id.clone()).await;
            let _ = slint::invoke_from_event_loop(move || {
                match result {
                    Ok(_) => controller.set_status("Connection removed"),
                    Err(err) => controller.set_status(&format!("Remove connection error: {err}")),
                }
                {
                    let mut state = controller.state.lock().expect("ui state");
                    state.connections.retain(|conn| conn.id != id);
                    for group in &mut state.server_groups {
                        group.connection_ids.retain(|conn_id| conn_id != &id);
                    }
                    if state.selected_conn_id == id {
                        state.selected_conn_id.clear();
                    }
                }
                controller.persist_server_groups();
                controller.sync();
            });
        });
    }

    fn pick_sqlite_file(&self) {
        match commands::select_sqlite_file() {
            Ok(path) => {
                if let Some(ui) = self.ui.upgrade() {
                    ui.set_dialog_database(ss(&path));
                    ui.set_dialog_driver(ss("sqlite"));
                }
            }
            Err(err) => self.set_status(&format!("SQLite file selection failed: {err}")),
        }
    }

    fn clear_connection_dialog(&self, ui: &MainWindow) {
        {
            let mut state = self.state.lock().expect("ui state");
            state.editing_conn_id.clear();
        }
        ui.set_dialog_name(ss(""));
        ui.set_dialog_driver(ss("postgres"));
        ui.set_dialog_host(ss("localhost"));
        ui.set_dialog_port(ss("5432"));
        ui.set_dialog_username(ss(""));
        ui.set_dialog_password(ss(""));
        ui.set_dialog_database(ss(""));
        ui.set_dialog_dsn(ss(""));
        ui.set_dialog_tab_color(ss("#6366f1"));
        ui.set_dialog_kube_context(ss(""));
        ui.set_dialog_kube_namespace(ss(""));
        ui.set_dialog_kube_resource(ss(""));
        ui.set_dialog_kube_local_port(ss(""));
        ui.set_dialog_kube_remote_port(ss(""));
    }

    fn connection_config_from_dialog(&self, ui: &MainWindow) -> ConnectionConfig {
        let editing_id = {
            let state = self.state.lock().expect("ui state");
            state.editing_conn_id.clone()
        };
        let driver = ui.get_dialog_driver().to_string();
        let driver = if driver.trim().is_empty() {
            "postgres".to_string()
        } else {
            driver
        };
        let tab_color = ui.get_dialog_tab_color().to_string();
        let kube_context = ui.get_dialog_kube_context().to_string();
        let kube_namespace = ui.get_dialog_kube_namespace().to_string();
        let kube_resource = ui.get_dialog_kube_resource().to_string();
        let kube_local_port = parse_i32(&ui.get_dialog_kube_local_port());
        let kube_remote_port = parse_i32(&ui.get_dialog_kube_remote_port());
        ConnectionConfig {
            id: if editing_id.is_empty() {
                format!("conn-{}", now_millis())
            } else {
                editing_id
            },
            name: ui.get_dialog_name().to_string(),
            driver: driver.clone(),
            tab_color: if tab_color.trim().is_empty() {
                "#6366f1".to_string()
            } else {
                tab_color
            },
            tab_text_black: false,
            host: ui.get_dialog_host().to_string(),
            port: parse_port(&ui.get_dialog_port(), &driver),
            username: ui.get_dialog_username().to_string(),
            password: ui.get_dialog_password().to_string(),
            database: ui.get_dialog_database().to_string(),
            dsn: ui.get_dialog_dsn().to_string(),
            use_kube_port_forward: !kube_context.trim().is_empty()
                || !kube_namespace.trim().is_empty()
                || !kube_resource.trim().is_empty()
                || kube_local_port > 0
                || kube_remote_port > 0,
            kube_context,
            kube_namespace,
            kube_resource,
            kube_local_port,
            kube_remote_port,
        }
    }

    fn load_saved_connections(&self) {
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result = commands::list_saved_connections(app_state.clone()).await;
            let prefs = app_state.store().await.ok();
            let order = if let Some(store) = &prefs {
                store
                    .get_ui_preference(CONNECTION_ORDER_KEY)
                    .await
                    .ok()
                    .flatten()
            } else {
                None
            };
            let _ = slint::invoke_from_event_loop(move || {
                match result {
                    Ok(mut connections) => {
                        apply_connection_order(&mut connections, order.as_deref());
                        let mut state = controller.state.lock().expect("ui state");
                        if state.selected_conn_id.is_empty() {
                            state.selected_conn_id = connections
                                .first()
                                .map(|conn| conn.id.clone())
                                .unwrap_or_default();
                        }
                        state.connections = connections;
                        state.status = "Connections loaded".to_string();
                    }
                    Err(err) => controller.set_status(&format!("Error loading connections: {err}")),
                }
                controller.sync();
            });
        });
    }

    fn load_ui_preferences(&self) {
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let prefs = app_state.store().await.ok();
            let groups = if let Some(store) = &prefs {
                store
                    .get_ui_preference(SERVER_GROUPS_KEY)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|value| serde_json::from_str::<Vec<ServerGroup>>(&value).ok())
                    .unwrap_or_default()
            } else {
                Vec::new()
            };
            let font_scale = if let Some(store) = &prefs {
                store
                    .get_ui_preference(FONT_SCALE_KEY)
                    .await
                    .ok()
                    .flatten()
                    .and_then(|value| value.parse::<i32>().ok())
                    .unwrap_or(100)
            } else {
                100
            };
            let _ = slint::invoke_from_event_loop(move || {
                let mut state = controller.state.lock().expect("ui state");
                state.server_groups = groups;
                state.font_scale_percent = font_scale.clamp(50, 250);
                drop(state);
                controller.sync();
            });
        });
    }

    fn load_schema(&self, conn_id: String) {
        if conn_id.is_empty() {
            return;
        }
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let result = commands::get_schema(app_state, conn_id.clone()).await;
            let _ = slint::invoke_from_event_loop(move || {
                match result {
                    Ok(schema) => controller.apply_schema(conn_id, schema),
                    Err(err) => controller.set_status(&format!("Error loading schema: {err}")),
                }
                controller.sync();
            });
        });
    }

    fn apply_schema(&self, conn_id: String, schema: SchemaTree) {
        let mut nodes = Vec::new();
        let mut words = Vec::new();
        flatten_schema("", &schema, &mut nodes, &mut words);
        let mut state = self.state.lock().expect("ui state");
        state.nav_nodes = nodes;
        state.schema_words = words;
        state.status = format!("Schema loaded for {conn_id}");
    }

    fn load_history_and_saved(&self, conn_id: String) {
        let controller = self.clone();
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            let history =
                commands::get_query_history_by_conn_id(app_state.clone(), conn_id.clone(), 100)
                    .await;
            let saved = commands::get_saved_queries(app_state, conn_id).await;
            let _ = slint::invoke_from_event_loop(move || {
                let mut state = controller.state.lock().expect("ui state");
                if let Ok(history) = history {
                    state.history = history;
                }
                if let Ok(saved) = saved {
                    state.saved_queries = saved;
                }
                drop(state);
                controller.sync();
            });
        });
    }

    fn persist_connection_order(&self) {
        let order = {
            let state = self.state.lock().expect("ui state");
            state
                .connections
                .iter()
                .map(|conn| conn.id.clone())
                .collect::<Vec<_>>()
        };
        let Ok(value) = serde_json::to_string(&order) else {
            return;
        };
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            if let Ok(store) = app_state.store().await {
                let _ = store.set_ui_preference(CONNECTION_ORDER_KEY, &value).await;
            }
        });
    }

    fn persist_server_groups(&self) {
        let groups = {
            let state = self.state.lock().expect("ui state");
            state.server_groups.clone()
        };
        let Ok(value) = serde_json::to_string(&groups) else {
            return;
        };
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            if let Ok(store) = app_state.store().await {
                let _ = store.set_ui_preference(SERVER_GROUPS_KEY, &value).await;
            }
        });
    }

    fn persist_font_scale(&self) {
        let font_scale = {
            let state = self.state.lock().expect("ui state");
            state.font_scale_percent
        };
        let app_state = self.app_state.clone();
        self.runtime.spawn(async move {
            if let Ok(store) = app_state.store().await {
                let _ = store
                    .set_ui_preference(FONT_SCALE_KEY, &font_scale.to_string())
                    .await;
            }
        });
    }

    fn save_server_group_from_dialog(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let name = ui.get_dialog_group_name().to_string();
        if name.trim().is_empty() {
            self.set_status("Group name is required");
            return;
        }
        {
            let mut state = self.state.lock().expect("ui state");
            state.server_groups.push(ServerGroup {
                id: format!("group-{}", now_millis()),
                name,
                expanded: true,
                connection_ids: Vec::new(),
            });
            state.status = "Server group created".to_string();
        }
        ui.set_dialog_group_name(ss(""));
        ui.set_server_group_dialog_open(false);
        self.persist_server_groups();
        self.sync();
    }

    fn adjust_font_scale(&self, delta: i32) {
        {
            let mut state = self.state.lock().expect("ui state");
            state.font_scale_percent = (state.font_scale_percent + delta).clamp(50, 250);
            state.status = format!("Editor font size: {}%", state.font_scale_percent);
        }
        self.persist_font_scale();
        self.sync();
    }

    fn set_output_tab(&self, tab: String) {
        {
            let mut state = self.state.lock().expect("ui state");
            state.output_tab = tab;
        }
        self.sync();
    }

    fn set_status(&self, message: &str) {
        {
            let mut state = self.state.lock().expect("ui state");
            state.status = message.to_string();
        }
        self.sync();
    }

    fn sync(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let state = self.state.lock().expect("ui state");

        ui.set_connections(model(
            state
                .connections
                .iter()
                .map(|conn| ConnectionView {
                    id: ss(&conn.id),
                    name: ss(&conn.name),
                    driver: ss(&conn.driver),
                    color: ss(&conn.tab_color),
                    selected: conn.id == state.selected_conn_id,
                    connected: true,
                    group_name: ss(group_name(&state.server_groups, &conn.id)),
                })
                .collect(),
        ));

        ui.set_server_groups(model(
            state
                .server_groups
                .iter()
                .map(|group| ServerGroupView {
                    id: ss(&group.id),
                    name: ss(&group.name),
                    expanded: group.expanded,
                })
                .collect(),
        ));

        ui.set_nav_nodes(model(
            state
                .nav_nodes
                .iter()
                .map(|node| NavNodeView {
                    id: ss(&node.id),
                    label: ss(&node.label),
                    kind: ss(&node.kind),
                    depth: node.depth,
                    expanded: true,
                    selected: node.selected,
                })
                .collect(),
        ));

        ui.set_tabs(model(
            state
                .tabs
                .iter()
                .map(|tab| {
                    let connection = state.connections.iter().find(|conn| conn.id == tab.conn_id);
                    let tab_color = connection
                        .map(|conn| conn.tab_color.as_str())
                        .unwrap_or_default();
                    let tab_text_black =
                        connection.map(|conn| conn.tab_text_black).unwrap_or(false);
                    TabView {
                        id: ss(&tab.id),
                        title: ss(&tab.title),
                        active: tab.id == state.active_tab_id,
                        running: tab.running,
                        dirty: tab.editor.dirty(),
                        connection_name: ss(connection_name(&state.connections, &tab.conn_id)),
                        has_custom_color: !tab_color.trim().is_empty(),
                        tab_color: color_from_hex(tab_color, Color::from_rgb_u8(99, 102, 241)),
                        tab_text_color: if tab_text_black {
                            Color::from_rgb_u8(0, 0, 0)
                        } else {
                            Color::from_rgb_u8(238, 242, 255)
                        },
                    }
                })
                .collect(),
        ));

        let (editor_text, completions, columns, column_types, rows, messages, running) =
            if let Some(tab) = active_tab(&state) {
                let dialect = state
                    .connections
                    .iter()
                    .find(|conn| conn.id == tab.conn_id)
                    .map(|conn| conn.driver.as_str())
                    .unwrap_or("postgres");
                (
                    tab.editor.text(),
                    tab.editor.completions(&state.schema_words, dialect),
                    tab.columns.clone(),
                    tab.column_types.clone(),
                    tab.rows.clone(),
                    if tab.error.is_empty() {
                        state.messages.clone()
                    } else {
                        format!("Error: {}", tab.error)
                    },
                    tab.running,
                )
            } else {
                (
                    String::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Arc::new(Vec::new()),
                    state.messages.clone(),
                    false,
                )
            };

        ui.set_editor_text(ss(&editor_text));
        ui.set_completions(model(
            completions
                .into_iter()
                .enumerate()
                .map(
                    |(idx, item): (usize, CompletionCandidate)| CompletionItemView {
                        label: ss(&item.label),
                        detail: ss(&item.detail),
                        replacement: ss(&item.replacement),
                        selected: idx == 0,
                    },
                )
                .collect(),
        ));
        ui.set_result_columns(model(
            columns
                .iter()
                .enumerate()
                .map(|(idx, name)| ResultColumnView {
                    name: ss(name),
                    type_name: ss(column_types
                        .get(idx)
                        .map(String::as_str)
                        .unwrap_or_default()),
                    width: 160,
                    sort: ss(""),
                })
                .collect(),
        ));
        ui.set_result_rows(ModelRc::from(Rc::new(ResultRowsModel { rows })));
        ui.set_active_query_running(running);
        ui.set_history(model(
            state
                .history
                .iter()
                .map(|item| HistoryItemView {
                    id: item.id as i32,
                    title: ss(first_line(&item.query)),
                    conn_id: ss(&item.conn_id),
                    detail: ss(&format!(
                        "{} rows - {}ms - {}",
                        item.result_count, item.duration, item.created_at
                    )),
                })
                .collect(),
        ));
        ui.set_saved_queries(model(
            state
                .saved_queries
                .iter()
                .map(|item| SavedQueryView {
                    id: item.id as i32,
                    title: ss(&item.title),
                    query: ss(&item.query),
                    detail: ss(first_line(&item.query)),
                })
                .collect(),
        ));
        ui.set_output_tab(ss(&state.output_tab));
        ui.set_messages(ss(&messages));
        ui.set_status_text(ss(&state.status));
        ui.set_font_scale_percent(state.font_scale_percent);
    }
}

impl QueryTab {
    fn new(id: &str, title: &str) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            conn_id: String::new(),
            query_id: String::new(),
            running: false,
            editor: SqlEditorBuffer::new(""),
            columns: Vec::new(),
            column_types: Vec::new(),
            rows: Arc::new(Vec::new()),
            rows_affected: 0,
            duration: 0,
            error: String::new(),
        }
    }
}

pub fn run() -> Result<()> {
    let runtime = Arc::new(Runtime::new()?);
    let ui = MainWindow::new()?;
    let controller = AppController::new(&ui, runtime);
    controller.install_callbacks(&ui);
    controller.start();
    ui.run()?;
    Ok(())
}

fn model<T: Clone + 'static>(items: Vec<T>) -> ModelRc<T> {
    ModelRc::from(Rc::new(VecModel::from(items)))
}

struct ResultRowsModel {
    rows: Arc<Vec<Vec<Value>>>,
}

impl Model for ResultRowsModel {
    type Data = ResultRowView;

    fn row_count(&self) -> usize {
        self.rows.len()
    }

    fn row_data(&self, row: usize) -> Option<Self::Data> {
        self.rows.get(row).map(|row| ResultRowView {
            cells: model(row.iter().map(value_to_cell_ref).collect()),
            selected: false,
        })
    }

    fn model_tracker(&self) -> &dyn ModelTracker {
        &()
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

fn ss(value: &str) -> SharedString {
    SharedString::from(value)
}

fn active_tab(state: &UiState) -> Option<&QueryTab> {
    state.tabs.iter().find(|tab| tab.id == state.active_tab_id)
}

fn active_tab_mut(state: &mut UiState) -> Option<&mut QueryTab> {
    let active_id = state.active_tab_id.clone();
    state.tabs.iter_mut().find(|tab| tab.id == active_id)
}

fn tab_by_query_id_mut<'a>(state: &'a mut UiState, query_id: &str) -> Option<&'a mut QueryTab> {
    state.tabs.iter_mut().find(|tab| tab.query_id == query_id)
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default()
}

fn connection_name<'a>(connections: &'a [ConnectionConfig], id: &str) -> &'a str {
    connections
        .iter()
        .find(|conn| conn.id == id)
        .map(|conn| conn.name.as_str())
        .unwrap_or("")
}

fn group_name<'a>(groups: &'a [ServerGroup], conn_id: &str) -> &'a str {
    groups
        .iter()
        .find(|group| group.connection_ids.iter().any(|id| id == conn_id))
        .map(|group| group.name.as_str())
        .unwrap_or("")
}

fn parse_port(value: &SharedString, driver: &str) -> i32 {
    let parsed = parse_i32(value);
    if parsed > 0 {
        return parsed;
    }
    match driver {
        "postgres" => 5432,
        "mysql" => 3306,
        _ => 0,
    }
}

fn parse_i32(value: &SharedString) -> i32 {
    value.trim().parse::<i32>().unwrap_or_default()
}

fn port_to_text(port: i32) -> String {
    if port > 0 {
        port.to_string()
    } else {
        String::new()
    }
}

fn color_from_hex(value: &str, fallback: Color) -> Color {
    let value = value.trim().strip_prefix('#').unwrap_or(value.trim());
    if value.len() != 6 {
        return fallback;
    }
    let Ok(encoded) = u32::from_str_radix(value, 16) else {
        return fallback;
    };
    Color::from_rgb_u8(
        ((encoded >> 16) & 0xff) as u8,
        ((encoded >> 8) & 0xff) as u8,
        (encoded & 0xff) as u8,
    )
}

fn value_to_cell(value: Value) -> ResultCellView {
    value_to_cell_ref(&value)
}

fn value_to_cell_ref(value: &Value) -> ResultCellView {
    ResultCellView {
        text: ss(&match value {
            Value::Null => "NULL".to_string(),
            Value::String(value) => value.clone(),
            other => other.to_string(),
        }),
        pending: false,
    }
}

fn first_line(value: &str) -> &str {
    value.lines().next().unwrap_or(value)
}

fn flatten_schema(
    prefix: &str,
    schema: &SchemaTree,
    nodes: &mut Vec<NavNode>,
    words: &mut Vec<String>,
) {
    for schema_node in &schema.schemas {
        nodes.push(NavNode {
            id: format!("schema:{}", schema_node.name),
            label: schema_node.name.clone(),
            kind: "schema".to_string(),
            depth: 0,
            selected: false,
        });
        nodes.push(NavNode {
            id: format!("section:{}:tables", schema_node.name),
            label: format!("Tables ({})", schema_node.tables.len()),
            kind: "section".to_string(),
            depth: 1,
            selected: false,
        });
        for table in &schema_node.tables {
            let qualified = format!("{}.{}", schema_node.name, table.name);
            words.push(qualified.clone());
            nodes.push(NavNode {
                id: format!("table:{}:{}", schema_node.name, table.name),
                label: table.name.clone(),
                kind: "table".to_string(),
                depth: 2,
                selected: false,
            });
            for column in &table.columns {
                words.push(format!("{qualified}.{}", column.name));
                nodes.push(NavNode {
                    id: format!("column:{}:{}:{}", schema_node.name, table.name, column.name),
                    label: format!("{} : {}", column.name, column.column_type),
                    kind: "column".to_string(),
                    depth: 3,
                    selected: false,
                });
            }
        }
        nodes.push(NavNode {
            id: format!("section:{}:views", schema_node.name),
            label: format!("Views ({})", schema_node.views.len()),
            kind: "section".to_string(),
            depth: 1,
            selected: false,
        });
        for view in &schema_node.views {
            let qualified = format!("{}.{}", schema_node.name, view.name);
            words.push(qualified);
            nodes.push(NavNode {
                id: format!("table:{}:{}", schema_node.name, view.name),
                label: view.name.clone(),
                kind: "view".to_string(),
                depth: 2,
                selected: false,
            });
        }
        if !schema_node.indexes.is_empty() {
            nodes.push(NavNode {
                id: format!("section:{}:indexes", schema_node.name),
                label: format!("Indexes ({})", schema_node.indexes.len()),
                kind: "section".to_string(),
                depth: 1,
                selected: false,
            });
            for index in &schema_node.indexes {
                nodes.push(NavNode {
                    id: format!("index:{}:{}", schema_node.name, index),
                    label: index.clone(),
                    kind: "index".to_string(),
                    depth: 2,
                    selected: false,
                });
            }
        }
    }

    if !schema.tables.is_empty() {
        nodes.push(NavNode {
            id: "section::tables".to_string(),
            label: format!("Tables ({})", schema.tables.len()),
            kind: "section".to_string(),
            depth: 0,
            selected: false,
        });
    }
    for table in &schema.tables {
        let label = if prefix.is_empty() {
            table.name.clone()
        } else {
            format!("{prefix}.{}", table.name)
        };
        words.push(label.clone());
        nodes.push(NavNode {
            id: format!("table::{label}"),
            label: table.name.clone(),
            kind: "table".to_string(),
            depth: 1,
            selected: false,
        });
        for column in &table.columns {
            words.push(format!("{label}.{}", column.name));
            nodes.push(NavNode {
                id: format!("column::{label}:{}", column.name),
                label: format!("{} : {}", column.name, column.column_type),
                kind: "column".to_string(),
                depth: 2,
                selected: false,
            });
        }
    }

    if !schema.views.is_empty() {
        nodes.push(NavNode {
            id: "section::views".to_string(),
            label: format!("Views ({})", schema.views.len()),
            kind: "section".to_string(),
            depth: 0,
            selected: false,
        });
    }
    for view in &schema.views {
        words.push(view.name.clone());
        nodes.push(NavNode {
            id: format!("table::{}", view.name),
            label: view.name.clone(),
            kind: "view".to_string(),
            depth: 1,
            selected: false,
        });
    }

    if !schema.indexes.is_empty() {
        nodes.push(NavNode {
            id: "section::indexes".to_string(),
            label: format!("Indexes ({})", schema.indexes.len()),
            kind: "section".to_string(),
            depth: 0,
            selected: false,
        });
        for index in &schema.indexes {
            nodes.push(NavNode {
                id: format!("index::{}", index),
                label: index.clone(),
                kind: "index".to_string(),
                depth: 1,
                selected: false,
            });
        }
    }
}

fn apply_connection_order(connections: &mut Vec<ConnectionConfig>, order_json: Option<&str>) {
    let Some(order_json) = order_json else {
        return;
    };
    let Ok(order) = serde_json::from_str::<Vec<String>>(order_json) else {
        return;
    };
    connections.sort_by_key(|conn| {
        order
            .iter()
            .position(|id| id == &conn.id)
            .unwrap_or(usize::MAX)
    });
}

#[allow(dead_code)]
fn _preference_keys() -> [&'static str; 3] {
    [SERVER_GROUPS_KEY, CONNECTION_ORDER_KEY, FONT_SCALE_KEY]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Column, Schema, Table};

    fn connection(id: &str, name: &str) -> ConnectionConfig {
        ConnectionConfig {
            id: id.to_string(),
            name: name.to_string(),
            driver: "postgres".to_string(),
            ..ConnectionConfig::default()
        }
    }

    #[test]
    fn connection_order_preference_reorders_saved_connections() {
        let mut connections = vec![
            connection("a", "Alpha"),
            connection("b", "Beta"),
            connection("c", "Gamma"),
        ];

        apply_connection_order(&mut connections, Some(r#"["c","a"]"#));

        let ids = connections
            .iter()
            .map(|conn| conn.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["c", "a", "b"]);
    }

    #[test]
    fn bad_connection_order_preference_keeps_default_order() {
        let mut connections = vec![connection("a", "Alpha"), connection("b", "Beta")];

        apply_connection_order(&mut connections, Some("not json"));

        let ids = connections
            .iter()
            .map(|conn| conn.id.as_str())
            .collect::<Vec<_>>();
        assert_eq!(ids, ["a", "b"]);
    }

    #[test]
    fn schema_flattening_creates_nav_nodes_and_completion_words() {
        let schema = SchemaTree {
            schemas: vec![Schema {
                name: "public".to_string(),
                tables: vec![Table {
                    name: "orders".to_string(),
                    columns: vec![Column {
                        name: "id".to_string(),
                        column_type: "int4".to_string(),
                        ..Column::default()
                    }],
                    ..Table::default()
                }],
                ..Schema::default()
            }],
            ..SchemaTree::default()
        };
        let mut nodes = Vec::new();
        let mut words = Vec::new();

        flatten_schema("", &schema, &mut nodes, &mut words);

        assert!(nodes.iter().any(|node| node.id == "schema:public"));
        assert!(nodes.iter().any(|node| node.id == "table:public:orders"));
        assert!(words.iter().any(|word| word == "public.orders.id"));
    }

    #[test]
    fn active_tab_lookup_tracks_state_selection() {
        let mut state = UiState::default();
        state.tabs.push(QueryTab::new("one", "One"));
        state.tabs.push(QueryTab::new("two", "Two"));
        state.active_tab_id = "two".to_string();

        active_tab_mut(&mut state)
            .expect("active tab")
            .editor
            .set_text("SELECT 2;");

        assert_eq!(active_tab(&state).unwrap().editor.text(), "SELECT 2;");
    }

    #[test]
    fn json_values_convert_to_visible_result_cells() {
        assert_eq!(value_to_cell(Value::Null).text.as_str(), "NULL");
        assert_eq!(
            value_to_cell(Value::String("hello".to_string()))
                .text
                .as_str(),
            "hello"
        );
        assert_eq!(value_to_cell(Value::Bool(true)).text.as_str(), "true");
    }
}
