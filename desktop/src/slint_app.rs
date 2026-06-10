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
use serde_json::Value;
use slint::{ComponentHandle, ModelRc, SharedString, VecModel};
use std::{
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
    rows: Vec<Vec<Value>>,
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

impl AppController {
    fn new(ui: &MainWindow, runtime: Arc<Runtime>) -> Self {
        let mut state = UiState {
            output_tab: "results".to_string(),
            status: "Ready".to_string(),
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
                ui.set_connection_dialog_open(true);
            }
        });

        let controller = self.clone();
        ui.on_close_connection_dialog(move || {
            if let Some(ui) = controller.ui.upgrade() {
                ui.set_connection_dialog_open(false);
            }
        });

        let controller = self.clone();
        ui.on_save_connection_dialog(move || controller.save_connection_from_dialog());

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
        ui.on_editor_focus(move || controller.set_status("Editor focused"));

        let controller = self.clone();
        ui.on_execute_query(move || controller.execute_active_query());

        let controller = self.clone();
        ui.on_cancel_query(move || controller.cancel_active_query());

        let controller = self.clone();
        ui.on_save_query(move || controller.save_active_query());

        let controller = self.clone();
        ui.on_set_output_tab(move |tab| controller.set_output_tab(tab.to_string()));

        let controller = self.clone();
        ui.on_refresh_schema(move |id| controller.load_schema(id.to_string()));
    }

    fn start(&self) {
        self.sync();
        self.load_saved_connections();
    }

    fn add_tab(&self, sql: Option<String>) {
        {
            let mut state = self.state.lock().expect("ui state");
            let number = state.tabs.len() + 1;
            let id = format!("tab-{}", now_millis());
            let mut tab = QueryTab::new(&id, &format!("Query {number}"));
            tab.conn_id = state.selected_conn_id.clone();
            if let Some(sql) = sql {
                tab.editor.set_text(&sql);
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
                Some(format!("SELECT * FROM {name} LIMIT 100;"))
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

    fn handle_editor_key(
        &self,
        text: String,
        ctrl: bool,
        shift: bool,
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
                "a" | "A" => {
                    self.with_active_editor(|editor| editor.select_all());
                    return true;
                }
                "z" | "Z" if shift => {
                    self.with_active_editor(|editor| editor.redo());
                    return true;
                }
                "z" | "Z" => {
                    self.with_active_editor(|editor| editor.undo());
                    return true;
                }
                _ => {}
            }
        }

        let mut handled = true;
        match text.as_str() {
            "\u{8}" | "Backspace" => self.with_active_editor(|editor| editor.backspace()),
            "\u{7f}" | "Delete" => self.with_active_editor(|editor| editor.delete_forward()),
            "\n" | "\r" | "Enter" => self.with_active_editor(|editor| editor.insert_text("\n")),
            "\t" | "Tab" => self.with_active_editor(|editor| editor.insert_text("    ")),
            "\u{f700}" | "ArrowUp" => self.with_active_editor(|editor| editor.move_up()),
            "\u{f701}" | "ArrowDown" => self.with_active_editor(|editor| editor.move_down()),
            "\u{f702}" | "ArrowLeft" => self.with_active_editor(|editor| editor.move_left()),
            "\u{f703}" | "ArrowRight" => self.with_active_editor(|editor| editor.move_right()),
            "" => handled = false,
            _ if text.chars().all(|ch| !ch.is_control()) => {
                self.with_active_editor(|editor| editor.insert_text(&text))
            }
            _ => handled = false,
        }
        handled
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
            tab.rows.clear();
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
            let result = commands::execute_query_streamed(
                emitter,
                runtime_state,
                conn_id,
                query_id,
                sql,
                1_000_000,
            )
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
            tab.rows.clear();
        }
    }

    fn apply_query_chunk(&self, chunk: QueryStreamChunk) {
        let mut state = self.state.lock().expect("ui state");
        if let Some(tab) = tab_by_query_id_mut(&mut state, &chunk.query_id) {
            tab.rows.extend(chunk.rows);
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

    fn save_connection_from_dialog(&self) {
        let Some(ui) = self.ui.upgrade() else {
            return;
        };
        let name = ui.get_dialog_name().to_string();
        let driver = ui.get_dialog_driver().to_string();
        let host = ui.get_dialog_host().to_string();
        let database = ui.get_dialog_database().to_string();
        if name.trim().is_empty() {
            self.set_status("Connection name is required");
            return;
        }
        let cfg = ConnectionConfig {
            id: format!("conn-{}", now_millis()),
            name,
            driver: if driver.is_empty() {
                "postgres".to_string()
            } else {
                driver
            },
            tab_color: "#6366f1".to_string(),
            tab_text_black: false,
            host,
            port: 0,
            username: String::new(),
            password: String::new(),
            database,
            dsn: String::new(),
            use_kube_port_forward: false,
            kube_context: String::new(),
            kube_namespace: String::new(),
            kube_resource: String::new(),
            kube_local_port: 0,
            kube_remote_port: 0,
        };

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
                        state.connections = connections;
                        state.status = "Connections loaded".to_string();
                    }
                    Err(err) => controller.set_status(&format!("Error loading connections: {err}")),
                }
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
                .map(|tab| TabView {
                    id: ss(&tab.id),
                    title: ss(&tab.title),
                    active: tab.id == state.active_tab_id,
                    running: tab.running,
                    dirty: tab.editor.dirty(),
                    connection_name: ss(connection_name(&state.connections, &tab.conn_id)),
                })
                .collect(),
        ));

        let (lines, completions, columns, column_types, rows, messages) =
            if let Some(tab) = active_tab(&state) {
                let dialect = state
                    .connections
                    .iter()
                    .find(|conn| conn.id == tab.conn_id)
                    .map(|conn| conn.driver.as_str())
                    .unwrap_or("postgres");
                (
                    tab.editor.visible_lines(240),
                    tab.editor.completions(&state.schema_words, dialect),
                    tab.columns.clone(),
                    tab.column_types.clone(),
                    tab.rows.clone(),
                    if tab.error.is_empty() {
                        state.messages.clone()
                    } else {
                        format!("Error: {}", tab.error)
                    },
                )
            } else {
                (
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    Vec::new(),
                    state.messages.clone(),
                )
            };

        ui.set_editor_lines(model(
            lines
                .into_iter()
                .map(|line| EditorLineView {
                    number: line.number,
                    text: ss(&line.text),
                    class_name: ss(&line.class_name),
                    active: line.active,
                })
                .collect(),
        ));
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
        ui.set_result_rows(model(
            rows.into_iter()
                .take(10_000)
                .map(|row| ResultRowView {
                    cells: model(row.into_iter().map(value_to_cell).collect()),
                    selected: false,
                })
                .collect(),
        ));
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
            editor: SqlEditorBuffer::new("SELECT 1;"),
            columns: Vec::new(),
            column_types: Vec::new(),
            rows: Vec::new(),
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

fn value_to_cell(value: Value) -> ResultCellView {
    ResultCellView {
        text: ss(&match value {
            Value::Null => "NULL".to_string(),
            Value::String(value) => value,
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
        for table in &schema_node.tables {
            let qualified = format!("{}.{}", schema_node.name, table.name);
            words.push(qualified.clone());
            nodes.push(NavNode {
                id: format!("table:{}:{}", schema_node.name, table.name),
                label: table.name.clone(),
                kind: "table".to_string(),
                depth: 1,
                selected: false,
            });
            for column in &table.columns {
                words.push(format!("{qualified}.{}", column.name));
                nodes.push(NavNode {
                    id: format!("column:{}:{}:{}", schema_node.name, table.name, column.name),
                    label: format!("{} : {}", column.name, column.column_type),
                    kind: "column".to_string(),
                    depth: 2,
                    selected: false,
                });
            }
        }
        for view in &schema_node.views {
            let qualified = format!("{}.{}", schema_node.name, view.name);
            words.push(qualified);
            nodes.push(NavNode {
                id: format!("table:{}:{}", schema_node.name, view.name),
                label: format!("{} (view)", view.name),
                kind: "view".to_string(),
                depth: 1,
                selected: false,
            });
        }
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
            depth: 0,
            selected: false,
        });
        for column in &table.columns {
            words.push(format!("{label}.{}", column.name));
            nodes.push(NavNode {
                id: format!("column::{label}:{}", column.name),
                label: format!("{} : {}", column.name, column.column_type),
                kind: "column".to_string(),
                depth: 1,
                selected: false,
            });
        }
    }

    for view in &schema.views {
        words.push(view.name.clone());
        nodes.push(NavNode {
            id: format!("table::{}", view.name),
            label: format!("{} (view)", view.name),
            kind: "view".to_string(),
            depth: 0,
            selected: false,
        });
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
