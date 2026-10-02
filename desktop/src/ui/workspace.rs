//! Root view: owns all application state (the old Svelte stores) and wires
//! UI events to backend commands.

use crate::commands::{self, StreamEvent};
use crate::models::{ConnectionConfig, DatabaseConnection, SchemaTree};
use crate::ui::editor::{EditorEvent, SqlEditor};
use crate::ui::output::{OutputCtx, OutputPanel};
use crate::ui::result_store::{Finished, ResultStore};
use crate::ui::statusbar::StatusBar;
use crate::ui::model::*;
use crate::ui::runtime;
use crate::ui::sql::schema::{db_schema_for, DbSchema};
use crate::ui::sql_text;
use crate::ui::widgets::text_input::{InputEvent, TextInput};
use crate::ui::{diagram::DiagramState, dialogs, sessions::SessionsState, storage::{StorageScope, StorageState}};
use gpui::{
    prelude::*, App, AsyncApp, ClipboardItem, Context, Entity, FocusHandle, Focusable, Pixels, Point,
    ScrollHandle, SharedString, Subscription, Task, WeakEntity, Window,
};
use std::sync::Arc;
use std::time::Duration;

pub struct SqlTab {
    pub database_name: String,
    pub editor: Entity<SqlEditor>,
    /// The tab's result, sort order and pending cell edits.
    pub store: Entity<ResultStore>,
    pub running: bool,
    pub query_id: String,
    _sub: Subscription,
}

pub enum TabKind {
    Sql(SqlTab),
    Diagram(DiagramState),
    Sessions(SessionsState),
    Storage(StorageState),
}

pub struct Tab {
    pub id: TabId,
    pub title: String,
    pub conn_id: String,
    pub manually_renamed: bool,
    pub kind: TabKind,
}

impl Tab {
    pub fn sql(&self) -> Option<&SqlTab> {
        match &self.kind {
            TabKind::Sql(t) => Some(t),
            _ => None,
        }
    }
    pub fn sql_mut(&mut self) -> Option<&mut SqlTab> {
        match &mut self.kind {
            TabKind::Sql(t) => Some(t),
            _ => None,
        }
    }
}

#[derive(Clone)]
pub enum PaneDrag {
    Nav,
    Split,
}

#[derive(Clone)]
pub struct TabDrag {
    pub index: usize,
    pub start_x: Pixels,
    pub active: bool,
    pub drop_index: Option<usize>,
    pub indicator_x: Pixels,
}

#[derive(Clone)]
pub enum NavMenu {
    Table { pos: Point<Pixels>, conn_id: String, table: String, schema: Option<String>, database: Option<String> },
    Database { pos: Point<Pixels>, conn_id: String },
    /// A database or schema node inside a connection's tree.
    DatabaseNode { pos: Point<Pixels>, conn_id: String, scope: StorageScope },
    DropConfirm { pos: Point<Pixels>, conn_id: String, table: String, schema: Option<String> },
}

#[derive(Clone, PartialEq)]
pub enum DragKind {
    Conn(String),
    Group(String),
}

#[derive(Clone)]
pub struct NavDrag {
    pub kind: DragKind,
    pub start: Point<Pixels>,
    pub active: bool,
    pub target_conn: Option<(String, Option<String>)>,
    pub target_group: Option<String>,
    pub after: bool,
}

pub struct Workspace {
    pub focus_handle: FocusHandle,
    pub connections: Vec<ActiveConnection>,
    pub selected_conn_id: String,
    pub settings: UiSettings,
    pub active_server_group_id: String,
    pub tabs: Vec<Tab>,
    pub active_tab_id: TabId,
    pub output_tab: OutputTab,
    /// Bumped when history or saved queries may have changed, so the output
    /// panel reloads whichever list it is showing.
    pub lists_rev: u64,
    pub output: Entity<OutputPanel>,
    pub statusbar: Entity<StatusBar>,
    pub status: SharedString,
    pub mono_family: SharedString,

    // Layout
    pub nav_width: f32,
    pub editor_ratio: f32,
    pub pane_drag: Option<PaneDrag>,
    pub main_area_top: f32,
    pub main_area_height: f32,

    // Tab bar
    pub tabbar: Entity<crate::ui::tabbar::TabBar>,
    /// Bounds of navigator rows (`conn:<id>` / `group:<id>`), used to decide
    /// whether a drag drops above or below a row.
    pub nav_row_bounds: std::collections::HashMap<String, gpui::Bounds<Pixels>>,
    /// Connection row under the pointer (the old `.conn-row:hover .row-actions`
    /// rule). Tracked in state rather than with `group_hover` so the row's
    /// layout is identical in prepaint and paint.
    pub hover_conn: Option<String>,

    // Navigator
    pub expanded: std::collections::HashSet<String>,
    pub expanded_tables: std::collections::HashSet<String>,
    pub expanded_groups: std::collections::HashSet<String>,
    pub add_menu_open: bool,
    pub settings_open: bool,
    pub nav_filter: Entity<TextInput>,
    pub font_scale_input: Entity<TextInput>,
    pub nav_menu: Option<NavMenu>,
    pub nav_drag: Option<NavDrag>,
    pub suppress_nav_click: bool,
    pub testing_conn_id: Option<String>,
    pub active_test_id: String,
    pub stop_requested: bool,
    pub nav_scroll: ScrollHandle,
    pub last_filter_load_key: String,

    // Dialogs
    pub server_group_dialog: Option<dialogs::ServerGroupDialog>,
    pub conn_dialog: Option<dialogs::ConnectionDialog>,
    pub import_dialog: Option<dialogs::ImportDialog>,
    pub title_dialog: Option<dialogs::TitleDialog>,
    pub terminate_confirm: Option<(TabId, DatabaseConnection)>,
    /// A long cell value opened in full from its eye button.
    pub cell_popup: Option<crate::ui::grid::CellPopup>,
    /// Scroll handle for the cell popup's text.
    pub cell_popup_scroll: ScrollHandle,
    /// Scroll handle for the connection dialog's fields, which do not fit on a
    /// short screen.
    pub conn_dialog_scroll: ScrollHandle,
    /// Per-connection search index for the navigator filter, rebuilt when a
    /// schema loads.
    pub nav_indexes: std::collections::HashMap<String, Arc<crate::ui::nav_index::SchemaIndex>>,
    /// Result of the last filter search, or None when not filtering.
    pub nav_matches: Option<Arc<crate::ui::nav_index::Matches>>,
    /// The navigator's flattened rows from the last frame that changed them.
    pub nav_rows_cache: Option<crate::ui::navigator::NavRowsCache>,
    /// The debounce + background search in flight.
    nav_filter_task: Option<Task<()>>,
    /// Bumped whenever a schema is replaced, so stale searches are dropped.
    nav_generation: u64,
    /// Connections whose index is being rebuilt off the UI thread, with the
    /// generation that rebuild will carry; a result for an older generation is
    /// thrown away.
    nav_index_pending: std::collections::HashMap<String, u64>,

    pub conn_select_open: Option<(TabId, usize)>,

    pub hovered: Option<SharedString>,
    _subs: Vec<Subscription>,
    edit_check: Option<Task<()>>,
    edit_check_debounce: Option<Task<()>>,
}

pub fn scale_of(settings: &UiSettings) -> f32 {
    (settings.font_scale() / 100.0) as f32
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = UiSettings::load();
        let scale = scale_of(&settings);
        let nav_filter = cx.new(|cx| {
            TextInput::new(cx, dialogs::nav_filter_look(scale)).with_placeholder("Filter tables or columns")
        });
        let font_scale_input = cx.new(|cx| {
            let mut t = TextInput::new(cx, dialogs::font_scale_look(scale));
            t.number = Some((50.0, 250.0, 10.0));
            t
        });
        let mut subs = Vec::new();
        subs.push(cx.subscribe_in(&nav_filter, window, |this, input, ev: &InputEvent, _w, cx| {
            if matches!(ev, InputEvent::Changed) {
                let text = input.read(cx).text().to_string();
                this.on_filter_changed(text, cx);
            }
        }));
        subs.push(cx.subscribe_in(&font_scale_input, window, |this, input, ev: &InputEvent, _w, cx| match ev {
            InputEvent::Submit | InputEvent::Blur => {
                let v: f64 = input.read(cx).text().trim().parse().unwrap_or(f64::NAN);
                this.set_font_scale(v, cx);
            }
            _ => {}
        }));
        let mono_family: SharedString = crate::ui::pick_mono_family(cx).into();
        let me = cx.entity();
        let tabbar = cx.new(|cx| crate::ui::tabbar::TabBar::new(&me, cx));
        let statusbar = cx.new(|cx| StatusBar::new(&me, cx));
        let output = cx.new(|cx| OutputPanel::new(&me, cx));
        let mut ws = Workspace {
            focus_handle: cx.focus_handle(),
            connections: Vec::new(),
            selected_conn_id: String::new(),
            active_server_group_id: String::new(),
            tabs: Vec::new(),
            active_tab_id: String::new(),
            output_tab: OutputTab::Results,
            lists_rev: 0,
            output,
            statusbar,
            status: "Ready".into(),
            mono_family,
            nav_width: 240.0,
            editor_ratio: 0.55,
            pane_drag: None,
            main_area_top: 0.0,
            main_area_height: 0.0,
            tabbar,
            nav_row_bounds: std::collections::HashMap::new(),
            hover_conn: None,
            expanded: Default::default(),
            expanded_tables: Default::default(),
            expanded_groups: Default::default(),
            add_menu_open: false,
            settings_open: false,
            nav_filter,
            font_scale_input,
            nav_menu: None,
            nav_drag: None,
            suppress_nav_click: false,
            testing_conn_id: None,
            active_test_id: String::new(),
            stop_requested: false,
            nav_scroll: ScrollHandle::new(),
            last_filter_load_key: String::new(),
            server_group_dialog: None,
            conn_dialog: None,
            import_dialog: None,
            title_dialog: None,
            terminate_confirm: None,
            cell_popup: None,
            cell_popup_scroll: ScrollHandle::new(),
            conn_dialog_scroll: ScrollHandle::new(),
            nav_indexes: std::collections::HashMap::new(),
            nav_matches: None,
            nav_rows_cache: None,
            nav_filter_task: None,
            nav_generation: 0,
            nav_index_pending: std::collections::HashMap::new(),
            conn_select_open: None,
            hovered: None,
            settings,
            _subs: subs,
            edit_check: None,
            edit_check_debounce: None,
        };
        let first = ws.make_sql_tab(String::new(), String::new(), cx);
        ws.active_tab_id = first.id.clone();
        ws.tabs.push(first);
        let ctx = ws.output_ctx();
        ws.output.update(cx, |o, cx| o.set_ctx(ctx, cx));
        ws.sync_font_scale_input(cx);
        ws.load_saved_connections(cx);
        // Trackpad pinch, which gpui does not deliver itself (see `ui::pinch`).
        // Steps that arrive together are combined into one zoom, so a fast
        // pinch costs one frame rather than one per step.
        if let Some(mut steps) = crate::ui::pinch::install() {
            cx.spawn_in(window, async move |this, cx| {
                use futures::StreamExt;
                while let Some(first) = steps.next().await {
                    let mut batch = vec![first];
                    while let Ok(m) = steps.try_recv() {
                        batch.push(m);
                    }
                    let factor = crate::ui::pinch::factor(batch);
                    if this.update_in(cx, |this, window, cx| this.diagram_pinch(factor, window, cx)).is_err() {
                        break;
                    }
                }
            })
            .detach();
        }
        ws
    }

    pub fn scale(&self) -> f32 {
        scale_of(&self.settings)
    }

    // ─── Tabs ───────────────────────────────────────────────────────────────

    pub fn make_sql_tab(&mut self, conn_id: String, database_name: String, cx: &mut Context<Self>) -> Tab {
        let scale = self.scale();
        let mono = self.mono_family.clone();
        let editor = cx.new(|cx| SqlEditor::new("", scale, mono, cx));
        let id = new_id();
        let tab_id = id.clone();
        let sub = cx.subscribe(&editor, move |this, _editor, ev: &EditorEvent, cx| match ev {
            EditorEvent::Changed => {
                this.on_sql_changed(&tab_id, cx);
            }
            EditorEvent::Run => this.run_query(&tab_id, cx),
        });
        let store = cx.new(|_| ResultStore::default());
        let mut tab = Tab {
            id,
            title: "Query".into(),
            conn_id,
            manually_renamed: false,
            kind: TabKind::Sql(SqlTab {
                database_name,
                editor,
                store,
                running: false,
                query_id: String::new(),
                _sub: sub,
            }),
        };
        self.configure_editor(&mut tab, cx);
        tab
    }

    /// A copy of what the tab bar shows for each tab.
    pub fn tab_infos(&self) -> Vec<crate::ui::tabbar::TabInfo> {
        self.tabs
            .iter()
            .map(|t| crate::ui::tabbar::TabInfo {
                id: t.id.clone(),
                title: t.title.clone(),
                running: matches!(&t.kind, TabKind::Sql(s) if s.running),
                custom: self.tab_custom_color(&t.conn_id),
            })
            .collect()
    }

    pub fn tab_custom_color(&self, conn_id: &str) -> Option<crate::ui::theme::Rgba> {
        let c = self.connection(conn_id)?;
        if c.config.tab_color.is_empty() {
            return None;
        }
        Some(crate::ui::dialogs::parse_hex(&c.config.tab_color).unwrap_or(crate::ui::theme::BG_SURFACE))
    }

    /// The result store of the active tab, if it is a query tab.
    pub fn active_store(&self) -> Option<Entity<ResultStore>> {
        self.active_tab().and_then(|t| t.sql()).map(|s| s.store.clone())
    }

    /// What the output panel reads of the workspace.
    pub fn output_ctx(&self) -> OutputCtx {
        OutputCtx {
            tab_id: self.active_tab_id.clone(),
            is_sql: self.active_tab().is_none_or(|t| t.sql().is_some()),
            store: self.active_store(),
            conn_id: self.output_conn_id(),
            output_tab: self.output_tab,
            lists_rev: self.lists_rev,
            scale: self.scale(),
            mono: self.mono_family.clone(),
        }
    }

    /// Open the saved-query title dialog in edit mode.
    pub fn open_title_edit(&mut self, id: i64, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let d = dialogs::TitleDialog::new(
            dialogs::TitleDialogKind::Edit { id, current: title.clone() },
            &title,
            self.scale(),
            window,
            cx,
        );
        self.title_dialog = Some(d);
        cx.notify();
    }

    pub fn active_tab(&self) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == self.active_tab_id)
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut Tab> {
        let id = self.active_tab_id.clone();
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn tab_mut(&mut self, id: &str) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn tab(&self, id: &str) -> Option<&Tab> {
        self.tabs.iter().find(|t| t.id == id)
    }

    pub fn set_active_tab(&mut self, id: TabId, cx: &mut Context<Self>) {
        if self.active_tab_id == id {
            return;
        }
        self.active_tab_id = id;
        self.on_active_tab_changed(cx);
        cx.notify();
    }

    /// The results grid is shared between tabs; like the old component it
    /// resets sort, selection and pending edits whenever its result changes.
    pub fn on_active_tab_changed(&mut self, cx: &mut Context<Self>) {
        self.schedule_edit_check(cx);
        let stale_sessions = self.active_tab().is_some_and(
            |t| matches!(&t.kind, TabKind::Sessions(st) if !st.last_updated.is_empty()),
        );
        if stale_sessions {
            let id = self.active_tab_id.clone();
            self.refresh_sessions(&id, cx);
        }
    }

    pub fn ensure_active_valid(&mut self, cx: &mut Context<Self>) {
        if self.tabs.is_empty() {
            let t = self.make_sql_tab(String::new(), String::new(), cx);
            self.tabs.push(t);
        }
        if !self.tabs.iter().any(|t| t.id == self.active_tab_id) {
            self.active_tab_id = self.tabs[0].id.clone();
            self.on_active_tab_changed(cx);
        }
    }

    pub fn add_tab(&mut self, conn_id: String, database_name: String, cx: &mut Context<Self>) -> TabId {
        let t = self.make_sql_tab(conn_id, database_name, cx);
        let id = t.id.clone();
        self.tabs.push(t);
        id
    }

    pub fn new_query_tab(&mut self, cx: &mut Context<Self>) {
        let id = self.add_tab(self.selected_conn_id.clone(), String::new(), cx);
        self.set_active_tab(id, cx);
    }

    pub fn remove_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tabs.retain(|t| t.id != id);
        self.ensure_active_valid(cx);
        cx.notify();
    }

    pub fn rename_tab(&mut self, id: &str, title: String) {
        if let Some(t) = self.tab_mut(id) {
            t.title = title;
            t.manually_renamed = true;
        }
    }

    pub fn duplicate_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else { return };
        let Some(orig) = self.tabs[index].sql() else { return };
        let text = orig.editor.read(cx).text().to_string();
        let database = orig.database_name.clone();
        let copy = orig.store.read(cx).duplicate();
        let title = format!("{} (Copy)", self.tabs[index].title);
        let conn_id = self.tabs[index].conn_id.clone();
        let mut dup = self.make_sql_tab(conn_id, database, cx);
        dup.title = title;
        if let Some(sql) = dup.sql_mut() {
            sql.editor.update(cx, |e, cx| e.set_text(&text, cx));
            // The copy starts idle: the query that is streaming into the
            // original is not streaming into it.
            sql.store.update(cx, |st, _| *st = copy);
        }
        self.tabs.insert(index + 1, dup);
        cx.notify();
    }

    pub fn close_other_tabs(&mut self, id: &str, cx: &mut Context<Self>) {
        self.tabs.retain(|t| t.id == id);
        self.ensure_active_valid(cx);
    }

    pub fn close_tabs_right(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.truncate(i + 1);
        }
        self.ensure_active_valid(cx);
    }

    pub fn close_tabs_left(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.tabs.drain(..i);
        }
        self.ensure_active_valid(cx);
    }

    pub fn reorder_tabs(&mut self, from: usize, to: usize) {
        if from < self.tabs.len() {
            let t = self.tabs.remove(from);
            let to = to.min(self.tabs.len());
            self.tabs.insert(to, t);
        }
    }

    pub fn close_tabs_for_conn(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        self.tabs.retain(|t| t.conn_id != conn_id);
        self.ensure_active_valid(cx);
    }

    pub fn set_tab_sql(&mut self, id: &str, sql: &str, cx: &mut Context<Self>) {
        if let Some(t) = self.tab(id).and_then(|t| t.sql()) {
            let editor = t.editor.clone();
            editor.update(cx, |e, cx| e.set_text(sql, cx));
        }
    }

    pub fn set_tab_conn(&mut self, id: &str, conn_id: String, cx: &mut Context<Self>) {
        if let Some(t) = self.tab_mut(id) {
            t.conn_id = conn_id;
            if let Some(sql) = t.sql_mut() {
                sql.database_name.clear();
            }
        }
        self.configure_editors(cx);
        self.schedule_edit_check(cx);
        cx.notify();
    }

    pub fn open_query_tab_with_sql(&mut self, conn_id: &str, database: &str, sql: &str, title: Option<String>, cx: &mut Context<Self>) {
        let id = self.add_tab(conn_id.to_string(), database.to_string(), cx);
        self.set_tab_sql(&id, sql, cx);
        if let Some(title) = title {
            if let Some(t) = self.tab_mut(&id) {
                t.title = title;
            }
        }
        self.set_active_tab(id, cx);
    }

    // ─── Editors ────────────────────────────────────────────────────────────

    pub fn db_schema(&self, conn_id: &str, database_name: &str) -> Option<DbSchema> {
        let conn = self.connections.iter().find(|c| c.config.id == conn_id)?;
        let schema = conn.schema.as_ref()?;
        Some(db_schema_for(schema, &conn.config.driver, database_name))
    }

    fn configure_editor(&self, tab: &mut Tab, cx: &mut Context<Self>) {
        let conn_id = tab.conn_id.clone();
        let Some(sql) = tab.sql_mut() else { return };
        let driver = self.connections.iter().find(|c| c.config.id == conn_id).map(|c| c.config.driver.clone()).unwrap_or_default();
        let db = self.db_schema(&conn_id, &sql.database_name);
        sql.editor.update(cx, |e, cx| e.configure(&driver, db, cx));
    }

    /// Mirror of `activeConnections.subscribe(() => reconfigure)`.
    pub fn configure_editors(&mut self, cx: &mut Context<Self>) {
        let mut tabs = std::mem::take(&mut self.tabs);
        for t in tabs.iter_mut() {
            self.configure_editor(t, cx);
        }
        self.tabs = tabs;
    }

    fn on_sql_changed(&mut self, _tab_id: &str, cx: &mut Context<Self>) {
        // The editability check is a database round trip. Run it once typing
        // pauses, and not at all while the statement still targets the table
        // whose primary key is already known.
        self.edit_check_debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(500)).await;
            this.update(cx, |this, cx| {
                let unchanged = this.active_tab().and_then(|t| t.sql()).is_some_and(|s| {
                    let sql = s.editor.read(cx).text().to_string();
                    match (&s.store.read(cx).edit_info, sql_text::parse_simple_select(&sql)) {
                        (Some(info), Some(p)) => info.table_name == p.table_name && info.schema_name == p.schema_name,
                        _ => false,
                    }
                });
                if !unchanged {
                    this.schedule_edit_check(cx);
                }
            })
            .ok();
        }));
        cx.notify();
    }

    // ─── Connections ────────────────────────────────────────────────────────

    fn load_saved_connections(&mut self, cx: &mut Context<Self>) {
        let state = runtime::state();
        let fut = runtime::spawn(async move { commands::list_saved_connections(state).await });
        cx.spawn(async move |this, cx| {
            if let Ok(saved) = fut.await {
                this.update(cx, |this, cx| {
                    if !saved.is_empty() {
                        let first = saved[0].id.clone();
                        let conns = saved.into_iter().map(ActiveConnection::new).collect();
                        this.connections = apply_connection_order(conns, &this.settings.connection_order);
                        this.persist_connection_order();
                        this.selected_conn_id = first;
                        this.configure_editors(cx);
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn persist_connection_order(&mut self) {
        self.settings.connection_order = self.connections.iter().map(|c| c.config.id.clone()).collect();
        self.settings.save();
    }

    pub fn connection(&self, id: &str) -> Option<&ActiveConnection> {
        self.connections.iter().find(|c| c.config.id == id)
    }

    fn connection_mut(&mut self, id: &str) -> Option<&mut ActiveConnection> {
        self.connections.iter_mut().find(|c| c.config.id == id)
    }

    pub fn load_cached_schema(&mut self, conn_id: &str, cx: &mut Context<Self>) -> Task<()> {
        let state = runtime::state();
        let id = conn_id.to_string();
        let fut = runtime::spawn(async move { commands::load_schema(state, id).await });
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            if let Ok(entry) = fut.await {
                if entry.schema_json.is_empty() {
                    return;
                }
                // A cached schema can be megabytes of JSON; decoding it on the
                // UI thread froze the window once per connection when the
                // filter pulled in every schema at once.
                let json = entry.schema_json;
                let parsed = cx.background_spawn(async move { serde_json::from_str::<SchemaTree>(&json).ok() }).await;
                if let Some(tree) = parsed {
                    this.update(cx, |this, cx| {
                        if let Some(c) = this.connection_mut(&conn_id) {
                            c.schema = Some(Arc::new(tree));
                        }
                        this.reindex_schema(&conn_id, cx);
                        this.configure_editors(cx);
                        cx.notify();
                    })
                    .ok();
                }
            }
        })
    }

    pub fn refresh_schema(&mut self, conn_id: &str, cx: &mut Context<Self>) -> Task<()> {
        let Some(conn) = self.connection_mut(conn_id) else { return Task::ready(()) };
        if conn.schema_loading {
            return Task::ready(());
        }
        conn.schema_loading = true;
        conn.schema_error = None;
        cx.notify();
        let state = runtime::state();
        let id = conn_id.to_string();
        let fut = runtime::spawn(async move {
            let tree = commands::get_schema(state.clone(), id.clone()).await;
            if let Ok(tree) = &tree {
                if let Ok(json) = serde_json::to_string(tree) {
                    let _ = commands::save_schema(state, id, json, String::new()).await;
                }
            }
            tree
        });
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                if let Some(c) = this.connection_mut(&conn_id) {
                    c.schema_loading = false;
                    match res {
                        Ok(tree) => c.schema = Some(Arc::new(tree)),
                        Err(e) => c.schema_error = Some(e),
                    }
                }
                this.reindex_schema(&conn_id, cx);
                this.configure_editors(cx);
                cx.notify();
            })
            .ok();
        })
    }

    /// Navigator expand: load cached schema, fetch fresh if none cached.
    pub fn ensure_schema(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        let load = self.load_cached_schema(conn_id, cx);
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            load.await;
            let task = this
                .update(cx, |this, cx| {
                    let has = this.connection(&conn_id).is_some_and(|c| c.schema.is_some());
                    (!has).then(|| this.refresh_schema(&conn_id, cx))
                })
                .ok()
                .flatten();
            if let Some(t) = task {
                t.await;
            }
        })
        .detach();
    }

    pub fn disconnect_conn(&mut self, id: &str, cx: &mut Context<Self>) {
        let state = runtime::state();
        let conn_id = id.to_string();
        let fut = runtime::spawn(async move { commands::disconnect(state, conn_id).await });
        let id = id.to_string();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| match res {
                Ok(()) => {
                    this.connections.retain(|c| c.config.id != id);
                    this.persist_connection_order();
                    this.remove_connection_from_groups(&id);
                    this.close_tabs_for_conn(&id, cx);
                    if this.selected_conn_id == id {
                        this.selected_conn_id.clear();
                    }
                    this.configure_editors(cx);
                    cx.notify();
                }
                Err(e) => {
                    this.status = format!("Disconnect error: {e}").into();
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn test_connection(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        let Some(conn) = self.connection(conn_id) else { return };
        let cfg = conn.config.clone();
        let test_id = new_id();
        self.testing_conn_id = Some(conn_id.to_string());
        self.active_test_id = test_id.clone();
        self.stop_requested = false;
        self.status = "Testing connection…".into();
        cx.notify();
        let fut = run_test_connection(cfg.clone(), test_id.clone());
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(()) => format!("Connection to {} succeeded ✓", cfg.name).into(),
                    Err(e) => {
                        if this.stop_requested {
                            "Connection test cancelled".into()
                        } else {
                            format!("Connection failed: {e}").into()
                        }
                    }
                };
                if this.active_test_id == test_id {
                    this.active_test_id.clear();
                }
                this.testing_conn_id = None;
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn cancel_test(&mut self, cx: &mut Context<Self>) {
        if self.active_test_id.is_empty() {
            return;
        }
        self.stop_requested = true;
        let state = runtime::state();
        let id = self.active_test_id.clone();
        let fut = runtime::spawn(async move { commands::cancel_test_connection(state, id).await });
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |this, cx| {
                    this.status = e.into();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    // ─── Server groups ──────────────────────────────────────────────────────

    pub fn add_server_group(&mut self, title: &str) -> String {
        let id = new_id();
        let t = title.trim();
        self.settings.server_groups.push(ServerGroup {
            id: id.clone(),
            title: if t.is_empty() { "Server Group".into() } else { t.into() },
            connection_ids: Vec::new(),
        });
        self.settings.save();
        self.active_server_group_id = id.clone();
        id
    }

    pub fn add_connection_to_group(&mut self, conn_id: &str, group_id: &str) {
        for g in self.settings.server_groups.iter_mut() {
            if g.id == group_id {
                if !g.connection_ids.iter().any(|c| c == conn_id) {
                    g.connection_ids.push(conn_id.to_string());
                }
            } else {
                g.connection_ids.retain(|c| c != conn_id);
            }
        }
        self.settings.save();
    }

    pub fn remove_connection_from_groups(&mut self, conn_id: &str) {
        for g in self.settings.server_groups.iter_mut() {
            g.connection_ids.retain(|c| c != conn_id);
        }
        self.settings.save();
    }

    pub fn move_connection(&mut self, conn_id: &str, target: Option<&str>, after: Option<bool>, group_id: Option<&str>) {
        let insert_at = |list_len: usize, target_idx: Option<usize>| -> usize {
            match (after, target_idx) {
                (None, _) | (_, None) => list_len,
                (Some(true), Some(i)) => i + 1,
                (Some(false), Some(i)) => i,
            }
        };
        if let Some(gid) = group_id {
            for g in self.settings.server_groups.iter_mut() {
                g.connection_ids.retain(|c| c != conn_id);
                if g.id == gid {
                    let idx = target.and_then(|t| g.connection_ids.iter().position(|c| c == t));
                    let at = insert_at(g.connection_ids.len(), idx);
                    g.connection_ids.insert(at, conn_id.to_string());
                }
            }
            self.settings.save();
            return;
        }
        self.remove_connection_from_groups(conn_id);
        let Some(pos) = self.connections.iter().position(|c| c.config.id == conn_id) else { return };
        let moved = self.connections.remove(pos);
        let idx = target.and_then(|t| self.connections.iter().position(|c| c.config.id == t));
        let at = insert_at(self.connections.len(), idx);
        self.connections.insert(at, moved);
        self.persist_connection_order();
    }

    pub fn move_server_group(&mut self, group_id: &str, target: &str, after: bool) {
        let groups = &mut self.settings.server_groups;
        let Some(pos) = groups.iter().position(|g| g.id == group_id) else { return };
        let moved = groups.remove(pos);
        match groups.iter().position(|g| g.id == target) {
            Some(i) => groups.insert(if after { i + 1 } else { i }, moved),
            None => groups.push(moved),
        }
        self.settings.save();
    }

    // ─── Font scale ─────────────────────────────────────────────────────────

    pub fn set_font_scale(&mut self, value: f64, cx: &mut Context<Self>) {
        self.settings.font_scale_percent = Some(clamp_font_scale(value));
        self.settings.save();
        self.sync_font_scale_input(cx);
        let scale = self.scale();
        for t in &self.tabs {
            if let Some(sql) = t.sql() {
                sql.editor.update(cx, |e, cx| e.set_font_scale(scale, cx));
            }
        }
        dialogs::rescale_inputs(self, scale, cx);
        cx.notify();
    }

    pub fn sync_font_scale_input(&mut self, cx: &mut Context<Self>) {
        let v = format!("{}", self.settings.font_scale() as i64);
        self.font_scale_input.update(cx, |i, cx| i.set_text(v, cx));
    }

    // ─── Navigator filter ───────────────────────────────────────────────────

    /// Re-index one connection after its schema changed. Lower-casing and
    /// mapping every table and column name is done off the UI thread; the
    /// index is swapped in when it is ready.
    pub fn reindex_schema(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        self.nav_generation += 1;
        let generation = self.nav_generation;
        let Some(schema) = self.connection(conn_id).and_then(|c| c.schema.clone()) else {
            self.nav_index_pending.remove(conn_id);
            self.nav_indexes.remove(conn_id);
            self.search_navigator_after(Self::REINDEX_COALESCE_MS, cx);
            return;
        };
        self.nav_index_pending.insert(conn_id.to_string(), generation);
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            let id = conn_id.clone();
            let index = cx
                .background_spawn(async move { crate::ui::nav_index::SchemaIndex::build(&id, generation, &schema) })
                .await;
            this.update(cx, |this, cx| {
                // A newer schema arrived while this one was being indexed.
                if this.nav_index_pending.get(&conn_id) != Some(&generation) {
                    return;
                }
                this.nav_index_pending.remove(&conn_id);
                this.nav_indexes.insert(conn_id, Arc::new(index));
                // Whatever was on screen was filtered against the old schema.
                this.search_navigator_after(Self::REINDEX_COALESCE_MS, cx);
            })
            .ok();
        })
        .detach();
    }

    /// Run the navigator filter: debounced, then off the UI thread, so typing
    /// stays responsive however large the schemas are.
    ///
    /// Long enough that an ordinary typing rhythm never triggers a search
    /// mid-word, short enough to feel immediate once the hands stop.
    pub const FILTER_DEBOUNCE_MS: u64 = 200;

    /// When schemas finish loading they arrive in a burst (one per
    /// connection). Re-searching after each would restart the typing debounce
    /// over and over; a short window gathers the burst into one search.
    const REINDEX_COALESCE_MS: u64 = 40;

    pub fn search_navigator(&mut self, cx: &mut Context<Self>) {
        self.search_navigator_after(Self::FILTER_DEBOUNCE_MS, cx);
    }

    fn search_navigator_after(&mut self, delay_ms: u64, cx: &mut Context<Self>) {
        let filter = self.filter_text(cx);
        if filter.is_empty() {
            self.nav_filter_task = None;
            self.last_filter_load_key.clear();
            if self.nav_matches.take().is_some() {
                cx.notify();
            }
            return;
        }
        let indexes: Vec<Arc<crate::ui::nav_index::SchemaIndex>> = self.nav_indexes.values().cloned().collect();
        // Assigning the new task drops the old one, which cancels it: a run of
        // keystrokes leaves exactly one timer, and the search happens once the
        // typing stops.
        self.nav_filter_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(delay_ms)).await;
            let matches = cx.background_spawn(async move { crate::ui::nav_index::search(&indexes, &filter) }).await;
            this.update(cx, |this, cx| {
                this.nav_matches = Some(Arc::new(matches));
                this.load_schemas_for_filter(cx);
                cx.notify();
            })
            .ok();
        }));
    }

    /// The filter's Clear button. It empties the box the way deleting the
    /// text does — as an edit that emits `Changed` — so it goes through the
    /// same `on_filter_changed` path as the keyboard; the search is also
    /// reset right here so the tree is restored in this same frame.
    pub fn clear_nav_filter(&mut self, cx: &mut Context<Self>) {
        self.nav_filter.update(cx, |i, cx| i.clear_as_edit(cx));
        self.search_navigator(cx);
        cx.notify();
    }

    /// The results the tree should be filtered by: none when the box is
    /// empty, whatever happens to be stored.
    pub fn effective_nav_matches(&self, cx: &App) -> Option<Arc<crate::ui::nav_index::Matches>> {
        if self.filter_text(cx).is_empty() {
            return None;
        }
        self.nav_matches.clone()
    }

    /// Filtering searches columns too, so it needs the schemas of connections
    /// that have not been expanded yet. Reading them is disk work, so it waits
    /// until the debounce has fired rather than happening per keystroke.
    fn load_schemas_for_filter(&mut self, cx: &mut Context<Self>) {
        let mut missing: Vec<String> =
            self.connections.iter().filter(|c| c.schema.is_none() && !c.schema_loading).map(|c| c.config.id.clone()).collect();
        missing.sort();
        let key = missing.join("|");
        if key.is_empty() || key == self.last_filter_load_key {
            return;
        }
        self.last_filter_load_key = key;
        for id in missing {
            self.load_cached_schema(&id, cx).detach();
        }
    }

    /// A keystroke in the filter box. This runs on the UI thread between the
    /// key going down and the character appearing, so it does nothing but
    /// restart the debounce; the input redraws itself, and the matching and
    /// schema loading wait for the pause in typing.
    fn on_filter_changed(&mut self, _text: String, cx: &mut Context<Self>) {
        self.search_navigator(cx);
    }

    pub fn filter_text(&self, cx: &App) -> String {
        self.nav_filter.read(cx).text().trim().to_lowercase()
    }

    // ─── Queries ────────────────────────────────────────────────────────────

    fn selected_or_all_sql(&self, tab: &SqlTab, cx: &App) -> String {
        let e = tab.editor.read(cx);
        let sel = e.selected_text().trim().to_string();
        if sel.is_empty() {
            e.text().to_string()
        } else {
            sel
        }
    }

    pub fn run_query(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tab(tab_id) else { return };
        let Some(sql_tab) = tab.sql() else { return };
        if sql_tab.running {
            return;
        }
        let sql = self.selected_or_all_sql(sql_tab, cx).trim().to_string();
        if sql.is_empty() {
            return;
        }
        let conn_id = if tab.conn_id.is_empty() { self.selected_conn_id.clone() } else { tab.conn_id.clone() };
        if conn_id.is_empty() {
            self.status = "No connection selected. Please connect to a database first.".into();
            cx.notify();
            return;
        }
        let database = sql_tab.database_name.clone();
        let store = sql_tab.store.clone();
        let query_id = new_id();
        if let Some(t) = self.tab_mut(tab_id) {
            t.conn_id = conn_id.clone();
            if let Some(s) = t.sql_mut() {
                s.running = true;
                s.query_id = query_id.clone();
            }
        }
        store.update(cx, |st, cx| {
            st.clear();
            cx.notify();
        });
        self.status = "Running query…".into();
        self.output_tab = OutputTab::Results;
        cx.notify();

        let (tx, rx) = futures::channel::mpsc::unbounded::<StreamEvent>();
        let emitter: commands::EventEmitter = Arc::new(move |ev| {
            let _ = tx.unbounded_send(ev);
        });
        let state = runtime::state();
        let (c2, q2, s2, d2) = (conn_id.clone(), query_id.clone(), sql.clone(), database.clone());
        let call = runtime::spawn(async move { commands::execute_query_streamed(emitter, state, c2, d2, q2, s2, 1_000_000).await });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            stream_query(this, cx, tab_id, query_id, conn_id, sql, store, rx, call).await;
        })
        .detach();
    }

    pub fn cancel_query(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(qid) = self.tab(tab_id).and_then(|t| t.sql()).map(|s| s.query_id.clone()) else { return };
        if qid.is_empty() {
            return;
        }
        let state = runtime::state();
        let fut = runtime::spawn(async move { commands::cancel_query(state, qid).await });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            let _ = fut.await;
            this.update(cx, |this, cx| {
                if let Some(s) = this.tab_mut(&tab_id).and_then(|t| t.sql_mut()) {
                    s.running = false;
                    s.query_id.clear();
                }
                this.status = "Query cancelled".into();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn open_save_query(&mut self, tab_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(tab) = self.tab(tab_id) else { return };
        let Some(s) = tab.sql() else { return };
        let sql = s.editor.read(cx).text().trim().to_string();
        if sql.is_empty() {
            self.status = "Cannot save empty query".into();
            cx.notify();
            return;
        }
        let conn_id = if tab.conn_id.is_empty() { self.selected_conn_id.clone() } else { tab.conn_id.clone() };
        if conn_id.is_empty() {
            self.status = "No connection selected. Please select a connection first.".into();
            cx.notify();
            return;
        }
        let d = dialogs::TitleDialog::new(dialogs::TitleDialogKind::Save { conn_id, sql }, "", self.scale(), window, cx);
        self.title_dialog = Some(d);
        cx.notify();
    }

    pub fn save_query(&mut self, conn_id: String, title: String, sql: String, cx: &mut Context<Self>) {
        let state = runtime::state();
        let t2 = title.clone();
        let fut = runtime::spawn(async move { commands::save_query(state, conn_id, t2, sql).await });
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(_) => {
                        this.status = format!("Query saved as \"{title}\"").into();
                        this.lists_rev += 1;
                    }
                    Err(e) => this.status = format!("Error saving query: {e}").into(),
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    // ─── Output lists ───────────────────────────────────────────────────────

    pub fn output_conn_id(&self) -> String {
        match self.active_tab() {
            Some(t) => t.conn_id.clone(),
            None => self.selected_conn_id.clone(),
        }
    }

    /// `useHistoryQuery` / `useSavedQuery`: reconnect if needed, then open a tab.
    pub fn use_query(&mut self, query: String, conn_id: String, cx: &mut Context<Self>) {
        if self.connection(&conn_id).is_some() {
            self.open_tab_for_query(query, conn_id, cx);
            return;
        }
        let state = runtime::state();
        let fut = runtime::spawn(async move {
            let saved = commands::list_saved_connections(state.clone()).await?;
            let cfg = saved.into_iter().find(|c| c.id == conn_id).ok_or_else(|| "not found".to_string())?;
            commands::save_and_connect(state, cfg.clone()).await?;
            Ok::<ConnectionConfig, String>(cfg)
        });
        cx.spawn(async move |this, cx| {
            if let Ok(cfg) = fut.await {
                this.update(cx, |this, cx| {
                    let id = cfg.id.clone();
                    this.connections.push(ActiveConnection::new(cfg));
                    this.persist_connection_order();
                    this.open_tab_for_query(query, id, cx);
                })
                .ok();
            }
        })
        .detach();
    }

    fn open_tab_for_query(&mut self, query: String, conn_id: String, cx: &mut Context<Self>) {
        self.selected_conn_id = conn_id.clone();
        let id = self.add_tab(conn_id, String::new(), cx);
        self.set_tab_sql(&id, &query, cx);
        self.set_active_tab(id, cx);
        self.configure_editors(cx);
    }

    // ─── Editability (simple SELECT with a primary key) ─────────────────────

    pub fn schedule_edit_check(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let Some(s) = tab.sql() else { return };
        let store = s.store.clone();
        let sql = s.editor.read(cx).text().to_string();
        let ok = store.read(cx).result.as_ref().is_some_and(|r| r.error.is_empty()) && !sql.is_empty();
        let clear = |store: &Entity<ResultStore>, cx: &mut Context<Self>| {
            if store.read(cx).edit_info.is_some() {
                store.update(cx, |st, cx| {
                    st.edit_info = None;
                    cx.notify();
                });
            }
        };
        if !ok {
            clear(&store, cx);
            return;
        }
        let Some(parsed) = sql_text::parse_simple_select(&sql) else {
            clear(&store, cx);
            return;
        };
        let conn_id = tab.conn_id.clone();
        let database = s.database_name.clone();
        let Some(conn) = self.connection(&conn_id) else {
            clear(&store, cx);
            return;
        };
        let driver = conn.config.driver.clone();
        let state = runtime::state();
        let p = parsed.clone();
        let fut = runtime::spawn(async move {
            commands::get_table_primary_keys(state, conn_id, driver, database, p.schema_name, p.table_name).await
        });
        self.edit_check = Some(cx.spawn(async move |_this, cx| {
            let res = fut.await;
            store
                .update(cx, |st, cx| {
                    st.edit_info = match res {
                        Ok(pk) if !pk.is_empty() => Some(EditInfo {
                            table_name: parsed.table_name,
                            schema_name: parsed.schema_name,
                            primary_key_cols: pk,
                        }),
                        _ => None,
                    };
                    cx.notify();
                })
                .ok();
        }));
    }

    pub fn save_edits(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let Some(s) = tab.sql() else { return };
        let st = s.store.read(cx);
        let (Some(info), Some(result)) = (&st.edit_info, &st.result) else { return };
        let driver = self.connection(&tab.conn_id).map(|c| c.config.driver.clone()).unwrap_or_else(|| "postgres".into());
        let rows = result.rows.to_vec();
        let sql = sql_text::generate_update_sql(
            &sql_text::EditTarget {
                driver: &driver,
                table_name: &info.table_name,
                schema_name: &info.schema_name,
                primary_key_cols: &info.primary_key_cols,
            },
            &result.columns,
            &rows,
            &st.pending_edits,
        );
        if sql.is_empty() {
            return;
        }
        let (conn, db) = (tab.conn_id.clone(), s.database_name.clone());
        let id = self.add_tab(conn, db, cx);
        self.set_tab_sql(&id, &sql, cx);
        self.set_active_tab(id, cx);
    }

    // ─── Misc ───────────────────────────────────────────────────────────────

    pub fn copy_to_clipboard(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn export_csv(&mut self, cx: &mut Context<Self>) {
        let Some(result) = self.active_store().and_then(|s| s.read(cx).result.clone()) else { return };
        let rows = result.rows.to_vec();
        let columns = result.columns.clone();
        let fut = runtime::spawn_blocking(move || {
            let csv = crate::ui::textfmt::build_csv(&columns, &rows, None);
            if csv.is_empty() {
                return Ok(());
            }
            commands::save_csv(csv, "query_results.csv".into())
        });
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |this, cx| {
                    this.status = format!("Export failed: {e}").into();
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn close_floating(&mut self) {
        self.nav_menu = None;
        self.add_menu_open = false;
        self.settings_open = false;
    }

    pub fn request_schema_refresh(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        self.refresh_schema(conn_id, cx).detach();
    }

    pub fn open_import(&mut self, conn_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        self.import_dialog = Some(dialogs::ImportDialog::new(conn_id.to_string(), self.scale(), window, cx));
        cx.notify();
    }

    pub fn open_connection_dialog(&mut self, editing: Option<ConnectionConfig>, window: &mut Window, cx: &mut Context<Self>) {
        self.conn_dialog = Some(dialogs::ConnectionDialog::new(editing, self.scale(), window, cx));
        cx.notify();
    }
}

impl Focusable for Workspace {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

/// `test_connection` with the 65s backend watchdog from the old IPC layer.
pub fn run_test_connection(cfg: ConnectionConfig, test_id: String) -> impl std::future::Future<Output = Result<(), String>> {
    let state = runtime::state();
    runtime::spawn(async move {
        let id = test_id.clone();
        let task = tokio::spawn(crate::ipc_diagnostics::with_request_context(id.clone(), {
            let state = state.clone();
            async move { commands::test_connection(state, cfg, id).await }
        }));
        let out = tokio::select! {
            r = task => r.map_err(|e| format!("backend command task failed: {e}")).and_then(|r| r),
            _ = tokio::time::sleep(Duration::from_secs(65)) => {
                let stage = crate::ipc_diagnostics::get_test_connection_stage(&test_id).unwrap_or_else(|| "unknown".into());
                let checks = crate::ipc_diagnostics::recommended_checks_for_stage(&stage);
                Err(format!("test_connection timed out in backend watchdog after 65s (last stage: {stage}). Recommended next checks: {checks}"))
            }
        };
        crate::ipc_diagnostics::clear_test_connection_stage(&test_id);
        out
    })
}

/// How often a streaming result is redrawn at most. Rows that arrive between
/// two draws are added together, so a fast query costs one redraw per frame
/// rather than one per batch the database driver happened to hand over.
const STREAM_FRAME: Duration = Duration::from_millis(16);

#[allow(clippy::too_many_arguments)]
async fn stream_query(
    this: WeakEntity<Workspace>,
    cx: &mut AsyncApp,
    tab_id: String,
    query_id: String,
    conn_id: String,
    sql: String,
    store: Entity<ResultStore>,
    mut rx: futures::channel::mpsc::UnboundedReceiver<StreamEvent>,
    call: impl std::future::Future<Output = Result<(), String>>,
) {
    use futures::{FutureExt, StreamExt};
    let mut call = Box::pin(call.fuse());
    let mut call_done = false;
    loop {
        let first = if call_done {
            match rx.next().await {
                Some(ev) => ev,
                None => break,
            }
        } else {
            futures::select! {
                ev = rx.next() => match ev { Some(ev) => ev, None => break },
                res = call => {
                    call_done = true;
                    if let Err(e) = res {
                        store.update(cx, |st, cx| {
                            st.fail(e.clone());
                            cx.notify();
                        }).ok();
                        this.update(cx, |this, cx| {
                            if let Some(s) = this.tab_mut(&tab_id).and_then(|t| t.sql_mut()) {
                                s.running = false;
                                s.query_id.clear();
                            }
                            this.status = format!("Error: {e}").into();
                            this.output_tab = OutputTab::Messages;
                            cx.notify();
                        }).ok();
                        return;
                    }
                    continue;
                }
            }
        };
        // Take everything that has arrived, in order, and apply it in one go.
        let mut events = vec![first];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let mut chunks: Vec<Vec<Vec<serde_json::Value>>> = Vec::new();
        let mut done = None;
        for ev in events {
            match ev {
                StreamEvent::Meta(meta) if meta.query_id == query_id => {
                    store
                        .update(cx, |st, cx| {
                            st.begin(meta.columns, meta.column_types);
                            cx.notify();
                        })
                        .ok();
                }
                StreamEvent::Chunk(chunk) if chunk.query_id == query_id => chunks.push(chunk.rows),
                StreamEvent::Done(d) if d.query_id == query_id => {
                    done = Some(d);
                    break;
                }
                _ => {}
            }
        }
        if !chunks.is_empty() {
            store
                .update(cx, |st, cx| {
                    for rows in chunks {
                        st.append(rows);
                    }
                    cx.notify();
                })
                .ok();
        }
        let Some(done) = done else {
            // More is coming; let the frame draw before taking the next batch.
            cx.background_executor().timer(STREAM_FRAME).await;
            continue;
        };
        let error = done.error.clone();
        let (columns_empty, n) = store
            .update(cx, |st, cx| {
                st.finish(Finished { rows_affected: done.rows_affected, duration: done.duration, error: error.clone() });
                cx.notify();
                (st.result.as_ref().is_none_or(|r| r.columns.is_empty()), st.row_count())
            })
            .unwrap_or((true, 0));
        this.update(cx, |this, cx| {
            if let Some(s) = this.tab_mut(&tab_id).and_then(|t| t.sql_mut()) {
                s.running = false;
                s.query_id.clear();
            }
            if !error.is_empty() {
                this.status = format!("Error: {error}").into();
                this.output_tab = OutputTab::Messages;
            } else {
                this.status = if columns_empty {
                    format!("{} row(s) affected · {}ms", done.rows_affected, done.duration).into()
                } else {
                    format!("{n} rows · {}ms", done.duration).into()
                };
                if sql_text::is_ddl(&sql) {
                    this.request_schema_refresh(&conn_id, cx);
                }
                if let Some(t) = this.tab_mut(&tab_id) {
                    if !t.manually_renamed {
                        if let Some(name) = sql_text::extract_first_table_name(&sql) {
                            t.title = name;
                        }
                    }
                }
            }
            this.schedule_edit_check(cx);
            this.lists_rev += 1;
            cx.notify();
        })
        .ok();
        return;
    }
}

impl Workspace {
    /// Wrap a `ScrollHandle` list with visible scrollbars: the bars sit over
    /// the list's right and bottom edges, the thumb can be dragged, and a press
    /// on the track pages towards it.
    /// The wrapper is returned unsized: the scrolling child keeps whatever
    /// bounds it had (a flex item, or its own `max_h`), because that bound is
    /// what makes it scroll at all. Callers add the sizing they need.
    pub fn scrollable(
        &mut self,
        id: &'static str,
        handle: &ScrollHandle,
        content: gpui::AnyElement,
        cx: &mut Context<Self>,
    ) -> gpui::Div {
        use gpui_component::scroll::{ScrollableElement, ScrollbarAxis};
        // The kit's scrollbar follows the handle itself (thumb drag, paging,
        // fade-out, repaint when the content's size changes), so all this
        // does is lay it over the content.
        let _ = (id, &cx);
        gpui::div()
            .relative()
            .flex()
            .flex_col()
            .child(content)
            .scrollbar(handle, ScrollbarAxis::Both)
    }
}

