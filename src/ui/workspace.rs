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
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// How often the status bar refreshes an export's progress.
const EXPORT_TICK: Duration = Duration::from_millis(150);
/// How long "Saved …" / "Export failed …" stays in the status bar.
const EXPORT_RESULT_LINGER: Duration = Duration::from_secs(10);

/// Shared between a CSV export's writer thread and the UI task watching it.
#[derive(Default)]
struct ExportProgress {
    /// The file has been chosen and writing has begun.
    started: AtomicBool,
    /// Data rows written so far.
    written: AtomicUsize,
    /// The chosen file's name, for the status text.
    name: Mutex<String>,
}

/// `1234567` → `"1,234,567"`.
pub fn group_digits(n: usize) -> String {
    let digits = n.to_string();
    let mut out = String::with_capacity(digits.len() + digits.len() / 3);
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(ch);
    }
    out
}

pub struct SqlTab {
    pub database_name: String,
    /// The schema unqualified table names resolve to; empty for none, in
    /// which case tables must be written schema-qualified.
    pub default_schema: String,
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

/// The hover card for a column that is part of one or more indexes.
#[derive(Clone)]
pub struct NavIndexTip {
    /// Which column row raised it, so only that row can take it down again.
    pub key: String,
    /// Where the pointer was when it arrived on the bolt.
    pub pos: Point<Pixels>,
    pub column: String,
    /// The indexes the column belongs to.
    pub indexes: Vec<crate::models::TableIndex>,
}

#[derive(Clone)]
pub enum NavMenu {
    Table { pos: Point<Pixels>, conn_id: String, table: String, schema: Option<String>, database: Option<String> },
    /// A view in the tree: quick `SELECT *` and copy name.
    View { pos: Point<Pixels>, conn_id: String, view: String, schema: Option<String>, database: Option<String> },
    Database { pos: Point<Pixels>, conn_id: String },
    /// A database or schema node inside a connection's tree.
    DatabaseNode { pos: Point<Pixels>, conn_id: String, scope: StorageScope },
    DropConfirm { pos: Point<Pixels>, conn_id: String, table: String, schema: Option<String> },
    /// A server group: edit or delete it.
    Group { pos: Point<Pixels>, group_id: String },
    /// Confirmation before deleting a server group and its connections.
    GroupDeleteConfirm { pos: Point<Pixels>, group_id: String },
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
    /// The tab a query status belongs to, and the text it set. While
    /// `status` still holds that text, other tabs don't show it: a new tab
    /// shouldn't report the row count of a query run somewhere else. Any
    /// other status replaces the text and so applies everywhere again.
    pub status_scope: Option<(TabId, SharedString)>,
    /// A CSV export's progress or outcome. The status bar shows this in
    /// preference to everything else while it is set.
    pub export_status: Option<SharedString>,
    /// An export is writing its file; a second one waits for it to finish.
    export_running: bool,
    /// Bumped per export, so clearing a finished export's message later
    /// doesn't wipe out a newer export's.
    export_seq: u64,
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
    /// Which select in the settings menu has its list open.
    pub font_select_open: Option<FontSelect>,
    /// Installed font families: last session's list from the cache file at
    /// first, replaced whenever a background refresh finds a different one.
    pub font_families: Option<Arc<FontLists>>,
    /// The background listing of installed fonts, while one is running.
    font_refresh: Option<Task<()>>,
    pub nav_filter: Entity<TextInput>,
    pub font_scale_input: Entity<TextInput>,
    pub nav_menu: Option<NavMenu>,
    pub nav_index_tip: Option<NavIndexTip>,
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
    /// The Help > About dialog is showing.
    pub about_open: bool,
    /// The Help > Check for Updates dialog (`None` = closed).
    pub app_update: Option<crate::ui::update_dialog::UpdateState>,
    pub terminate_confirm: Option<(TabId, DatabaseConnection)>,
    /// A long cell value opened in full from its eye button.
    pub cell_popup: Option<crate::ui::grid::CellPopup>,
    /// Scroll handle for the cell popup's text.
    pub cell_popup_scroll: ScrollHandle,
    /// A long error opened in full from its info button: (title, text).
    pub error_popup: Option<(String, String)>,
    /// Scroll handle for the error popup's text.
    pub error_popup_scroll: ScrollHandle,
    /// The full text behind a shortened status line, paired with the status
    /// it belongs to. The status bar offers it only while `status` still
    /// holds that line, so any later status retires it without bookkeeping.
    pub status_detail: Option<(SharedString, String)>,
    /// When the background schema refresh last tried each connection, so a
    /// connection whose refresh fails is not retried on every tick.
    pub schema_refresh_attempts: std::collections::HashMap<String, std::time::Instant>,
    /// Schema refreshes in flight, by connection: (refresh number, handle that
    /// stops it). Clicking the connection's spinner aborts it.
    schema_refresh_tasks: std::collections::HashMap<String, (u64, tokio::task::AbortHandle)>,
    schema_refresh_seq: u64,
    /// The connection whose refresh spinner the pointer is over; it shows
    /// as a stop button then.
    pub hover_schema_spinner: Option<String>,
    /// A busy tab the person asked to close, waiting on "are you sure?".
    pub close_tab_confirm: Option<TabId>,
    /// Something that would discard unsaved grid changes, waiting on "are you sure?".
    pub discard_confirm: Option<crate::ui::grid_save::DiscardAction>,
    /// A save to a table that can't roll back, waiting on "are you sure?".
    pub nontx_confirm: Option<TabId>,
    /// A save that found rows changed since they were loaded.
    pub save_conflicts: Option<crate::ui::grid_save::ConflictPrompt>,
    pub conflicts_scroll: ScrollHandle,
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
    /// The default-schema dropdown: which tab's is open, and the highlighted row.
    pub schema_select_open: Option<(TabId, usize)>,
    /// The Format button's style menu, open on this tab.
    pub format_style_open: Option<TabId>,
    /// The "SQL Template" submenu of a table's context menu is showing.
    pub nav_template_open: bool,

    pub hovered: Option<SharedString>,
    _subs: Vec<Subscription>,
    pub(crate) edit_check: Option<Task<()>>,
}

/// The selects in the settings menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSelect {
    UiFamily,
    UiSize,
    EditorFamily,
    EditorSize,
}

/// Scale of the UI: the zoom percentage times the chosen UI font size
/// relative to the size the layout was designed at.
pub fn scale_of(settings: &UiSettings) -> f32 {
    (settings.font_scale() / 100.0 * settings.ui_font_size() / DEFAULT_FONT_SIZE) as f32
}

/// Scale of the SQL editor: the zoom percentage times the chosen editor size.
pub fn editor_scale_of(settings: &UiSettings) -> f32 {
    (settings.font_scale() / 100.0 * settings.editor_font_size() / DEFAULT_FONT_SIZE) as f32
}

/// The editor family in effect: the user's pick, or the first installed
/// coding font.
pub fn mono_family_of(settings: &UiSettings, cx: &App) -> SharedString {
    match settings.editor_font_family.as_deref().map(str::trim) {
        Some(f) if !f.is_empty() => f.to_string().into(),
        _ => crate::ui::pick_mono_family(cx).into(),
    }
}

/// Installed font families, sorted, without the hidden `.`-prefixed system
/// ones: every family, and the ones whose `i` and `W` are equally wide.
/// Loads every font to measure it, so it runs off the UI thread.
pub fn list_font_families(ts: &gpui::TextSystem) -> FontLists {
    let mut names: Vec<String> = ts.all_font_names().into_iter().filter(|n| !n.starts_with('.') && !n.trim().is_empty()).collect();
    names.sort_by_key(|n| n.to_lowercase());
    names.dedup();
    let mut mono = Vec::new();
    for n in &names {
        let id = ts.resolve_font(&gpui::font(n.clone()));
        let w = |c: char| ts.advance(id, gpui::px(100.), c).map(|s| f32::from(s.width)).ok();
        if let (Some(a), Some(b)) = (w('i'), w('W')) {
            if a > 0.0 && (a - b).abs() < 0.5 {
                mono.push(SharedString::from(n.clone()));
            }
        }
    }
    FontLists { all: names.into_iter().map(SharedString::from).collect(), mono }
}

impl Workspace {
    pub fn new(window: &mut Window, cx: &mut Context<Self>) -> Self {
        let settings = UiSettings::load();
        crate::ui::sql::format::set_default_style(settings.sql_style);
        let scale = scale_of(&settings);
        crate::ui::theme::set_ui_font(settings.ui_font_family.as_deref());
        let mono_family = mono_family_of(&settings, cx);
        crate::ui::metrics::init(cx, mono_family.as_ref());
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
            status_scope: None,
            export_status: None,
            export_running: false,
            export_seq: 0,
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
            font_select_open: None,
            font_families: FontLists::load_cached().map(Arc::new),
            font_refresh: None,
            nav_filter,
            font_scale_input,
            nav_menu: None,
            nav_index_tip: None,
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
            about_open: false,
            app_update: None,
            terminate_confirm: None,
            cell_popup: None,
            cell_popup_scroll: ScrollHandle::new(),
            error_popup: None,
            error_popup_scroll: ScrollHandle::new(),
            status_detail: None,
            schema_refresh_attempts: std::collections::HashMap::new(),
            schema_refresh_tasks: std::collections::HashMap::new(),
            schema_refresh_seq: 0,
            hover_schema_spinner: None,
            close_tab_confirm: None,
            discard_confirm: None,
            nontx_confirm: None,
            save_conflicts: None,
            conflicts_scroll: ScrollHandle::new(),
            conn_dialog_scroll: ScrollHandle::new(),
            nav_indexes: std::collections::HashMap::new(),
            nav_matches: None,
            nav_rows_cache: None,
            nav_filter_task: None,
            nav_generation: 0,
            nav_index_pending: std::collections::HashMap::new(),
            conn_select_open: None,
            schema_select_open: None,
            format_style_open: None,
            nav_template_open: false,
            hovered: None,
            settings,
            _subs: subs,
            edit_check: None,
        };
        let first = ws.make_sql_tab(String::new(), String::new(), cx);
        ws.active_tab_id = first.id.clone();
        ws.tabs.push(first);
        let ctx = ws.output_ctx();
        ws.output.update(cx, |o, cx| o.set_ctx(ctx, cx));
        ws.sync_font_scale_input(cx);
        ws.load_saved_connections(cx);
        ws.start_background_schema_refresh(cx);
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

    pub fn editor_scale(&self) -> f32 {
        editor_scale_of(&self.settings)
    }

    // ─── Tabs ───────────────────────────────────────────────────────────────

    pub fn make_sql_tab(&mut self, conn_id: String, database_name: String, cx: &mut Context<Self>) -> Tab {
        let scale = self.editor_scale();
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
                default_schema: String::new(),
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
        self.connection(conn_id)?;
        let color = self.effective_tab_color(conn_id);
        if color.is_empty() {
            return None;
        }
        Some(crate::ui::dialogs::parse_hex(&color).unwrap_or(crate::ui::theme::BG_SURFACE))
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

    /// Whether the tab is in the middle of something closing it would stop:
    /// a query (fetching rows, building an index, …) or a session being
    /// terminated.
    pub fn tab_busy(&self, id: &str) -> bool {
        self.tab(id).is_some_and(|t| match &t.kind {
            TabKind::Sql(s) => s.running,
            TabKind::Sessions(s) => !s.terminating.is_empty(),
            _ => false,
        })
    }

    /// The tab's close button: a busy tab asks first.
    pub fn request_close_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        if !self.tabs_with_changes(None, cx).iter().all(|t| t != id) {
            self.guard_discard(crate::ui::grid_save::DiscardAction::CloseTabs(vec![id.to_string()]), cx);
        } else if self.tab_busy(id) {
            self.close_tab_confirm = Some(id.to_string());
            cx.notify();
        } else {
            self.remove_tab(id, cx);
        }
    }

    pub fn remove_tab(&mut self, id: &str, cx: &mut Context<Self>) {
        self.close_tabs_where(|_, t| t.id == id, cx);
        self.ensure_active_valid(cx);
        cx.notify();
    }

    /// Take every tab matching `close` (given its index and the tab) out of
    /// the list and release what it holds.
    fn close_tabs_where(&mut self, mut close: impl FnMut(usize, &Tab) -> bool, cx: &mut Context<Self>) {
        let (closed, kept): (Vec<(usize, Tab)>, Vec<(usize, Tab)>) =
            std::mem::take(&mut self.tabs).into_iter().enumerate().partition(|(i, t)| close(*i, t));
        self.tabs = kept.into_iter().map(|(_, t)| t).collect();
        for (_, tab) in closed {
            self.release_tab(tab, cx);
        }
    }

    /// Free a closed tab's resources now rather than whenever the last stray
    /// handle to them goes away: stop its query on the server (otherwise the
    /// database keeps streaming rows nobody will see) and empty its result
    /// store, so the rows are dropped even if an in-flight task still holds
    /// the store. Everything else the tab owns (editor, subscriptions, poll
    /// tasks) is dropped with it.
    fn release_tab(&mut self, tab: Tab, cx: &mut Context<Self>) {
        if self.conn_select_open.as_ref().is_some_and(|(id, _)| *id == tab.id) {
            self.conn_select_open = None;
        }
        if let TabKind::Sql(sql) = &tab.kind {
            if let Some(ctl) = sql.store.read(cx).saving.clone() {
                // A save still running for a closed tab is stopped and rolled back.
                drop(runtime::spawn(async move { ctl.stop().await }));
            }
            if sql.running && !sql.query_id.is_empty() {
                let state = runtime::state();
                let qid = sql.query_id.clone();
                // Spawned on the runtime, so it runs without being awaited.
                drop(runtime::spawn(async move { commands::cancel_query(state, qid).await }));
            }
            sql.store.update(cx, |st, cx| {
                *st = ResultStore::default();
                cx.notify();
            });
        }
        drop(tab);
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
        let default_schema = orig.default_schema.clone();
        let copy = orig.store.read(cx).duplicate();
        let title = format!("{} (Copy)", self.tabs[index].title);
        let conn_id = self.tabs[index].conn_id.clone();
        let mut dup = self.make_sql_tab(conn_id, database, cx);
        dup.title = title;
        if let Some(sql) = dup.sql_mut() {
            sql.default_schema = default_schema;
        }
        self.configure_editor(&mut dup, cx);
        if let Some(sql) = dup.sql_mut() {
            sql.editor.update(cx, |e, cx| e.set_text(&text, cx));
            // The copy starts idle: the query that is streaming into the
            // original is not streaming into it.
            sql.store.update(cx, |st, _| *st = copy);
        }
        self.tabs.insert(index + 1, dup);
        cx.notify();
    }

    /// Close the tabs `pick` selects (by index and tab), asking first if any
    /// of them have unsaved grid changes.
    fn close_tabs_guarded(&mut self, pick: impl Fn(usize, &Tab) -> bool, cx: &mut Context<Self>) {
        let ids: Vec<TabId> = self.tabs.iter().enumerate().filter(|(i, t)| pick(*i, t)).map(|(_, t)| t.id.clone()).collect();
        let changed = self.tabs_with_changes(None, cx);
        if ids.iter().any(|id| changed.contains(id)) {
            self.guard_discard(crate::ui::grid_save::DiscardAction::CloseTabs(ids), cx);
            return;
        }
        self.close_tabs_where(|i, t| pick(i, t), cx);
        self.ensure_active_valid(cx);
    }

    pub fn close_other_tabs(&mut self, id: &str, cx: &mut Context<Self>) {
        self.close_tabs_guarded(|_, t| t.id != id, cx);
    }

    pub fn close_tabs_right(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.close_tabs_guarded(|j, _| j > i, cx);
        }
    }

    pub fn close_tabs_left(&mut self, id: &str, cx: &mut Context<Self>) {
        if let Some(i) = self.tabs.iter().position(|t| t.id == id) {
            self.close_tabs_guarded(|j, _| j < i, cx);
        }
    }

    pub fn reorder_tabs(&mut self, from: usize, to: usize) {
        if from < self.tabs.len() {
            let t = self.tabs.remove(from);
            let to = to.min(self.tabs.len());
            self.tabs.insert(to, t);
        }
    }

    pub fn close_tabs_for_conn(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        self.close_tabs_where(|_, t| t.conn_id == conn_id, cx);
        self.ensure_active_valid(cx);
    }

    /// Lay out the tab's SQL (or just its selection) in the Mozilla style.
    pub fn format_tab_sql(&mut self, id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(t) = self.tab(id).and_then(|t| t.sql()) {
            let editor = t.editor.clone();
            editor.update(cx, |e, cx| e.format_sql(cx));
            editor.update(cx, |e, _| e.focus(window));
        }
    }

    /// Make `style` the default for the Format button and generated SQL.
    pub fn set_sql_style(&mut self, style: crate::ui::sql::format::SqlStyle, cx: &mut Context<Self>) {
        crate::ui::sql::format::set_default_style(style);
        if self.settings.sql_style != style {
            self.settings.sql_style = style;
            self.settings.save();
        }
        cx.notify();
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
                sql.default_schema.clear();
            }
        }
        self.configure_editors(cx);
        self.schedule_edit_check(cx);
        cx.notify();
    }

    pub fn open_query_tab_with_sql(&mut self, conn_id: &str, database: &str, sql: &str, title: Option<String>, cx: &mut Context<Self>) {
        self.open_query_tab_in_schema(conn_id, database, "", sql, title, cx);
    }

    /// `open_query_tab_with_sql` with the new tab's default schema set, so
    /// `sql` can name tables in that schema without qualifying them.
    pub fn open_query_tab_in_schema(
        &mut self,
        conn_id: &str,
        database: &str,
        default_schema: &str,
        sql: &str,
        title: Option<String>,
        cx: &mut Context<Self>,
    ) {
        let id = self.add_tab(conn_id.to_string(), database.to_string(), cx);
        if !default_schema.is_empty() {
            self.set_tab_default_schema(&id, default_schema.to_string(), cx);
        }
        self.set_tab_sql(&id, sql, cx);
        if let Some(title) = title {
            if let Some(t) = self.tab_mut(&id) {
                t.title = title;
            }
        }
        self.set_active_tab(id, cx);
    }

    pub fn set_tab_default_schema(&mut self, id: &str, schema: String, cx: &mut Context<Self>) {
        let Some(index) = self.tabs.iter().position(|t| t.id == id) else { return };
        let Some(sql) = self.tabs[index].sql_mut() else { return };
        if sql.default_schema == schema {
            return;
        }
        sql.default_schema = schema;
        let mut tab = self.tabs.remove(index);
        self.configure_editor(&mut tab, cx);
        self.tabs.insert(index, tab);
        // Whether the result is editable depends on which table it came from.
        if self.active_tab_id == id {
            self.schedule_edit_check(cx);
        }
        cx.notify();
    }

    /// The schemas a tab's default-schema dropdown offers: those of the
    /// database the tab runs against (for Postgres, the connection's own
    /// database when the tab names none). Empty for engines without schemas
    /// and before the connection's schema has loaded.
    pub fn schema_names(&self, conn_id: &str, database_name: &str) -> Vec<String> {
        let Some(conn) = self.connection(conn_id) else { return Vec::new() };
        if conn.config.driver == "sqlite" {
            return Vec::new();
        }
        let Some(tree) = conn.schema.as_ref() else { return Vec::new() };
        let configured = conn.config.database.trim();
        let database = tree
            .databases
            .iter()
            .find(|d| d.name == database_name)
            .or_else(|| if database_name.is_empty() { tree.databases.iter().find(|d| d.name == configured) } else { None });
        let schemas = match database {
            Some(db) => &db.schemas,
            None => &tree.schemas,
        };
        schemas.iter().map(|s| s.name.clone()).collect()
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
        let db = self.db_schema(&conn_id, &sql.database_name).map(|mut db| {
            if !sql.default_schema.is_empty() {
                db.default_schema = Some(sql.default_schema.clone());
            }
            db
        });
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
        // Whether a result can be edited depends on the query that produced
        // it, not on what the editor holds now.
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
                let refreshed_at = parse_sqlite_utc(&entry.last_refreshed_at);
                let parsed = cx.background_spawn(async move { serde_json::from_str::<SchemaTree>(&json).ok() }).await;
                if let Some(tree) = parsed {
                    this.update(cx, |this, cx| {
                        if let Some(c) = this.connection_mut(&conn_id) {
                            if c.schema_loading || c.schema.is_some() { return; }
                            c.schema = Some(Arc::new(tree));
                            c.schema_refreshed_at = refreshed_at;
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
        self.refresh_schema_with(conn_id, false, cx)
    }

    /// `background`: a refresh nobody asked for. Its failure leaves the schema
    /// on screen as it was, without an error line.
    fn refresh_schema_with(&mut self, conn_id: &str, background: bool, cx: &mut Context<Self>) -> Task<()> {
        let Some(conn) = self.connection_mut(conn_id) else { return Task::ready(()) };
        if conn.schema_loading {
            return Task::ready(());
        }
        let previous_schema = conn.schema.clone();
        conn.schema_loading = true;
        conn.schema_error = None;
        cx.notify();
        let state = runtime::state();
        let id = conn_id.to_string();
        let (progress, mut previews) = futures::channel::mpsc::unbounded();
        let (fut, abort) = runtime::spawn_abortable(async move {
            commands::get_schema_with_progress(state, id, progress).await.map(Arc::new)
        });
        self.schema_refresh_seq += 1;
        let seq = self.schema_refresh_seq;
        self.schema_refresh_tasks.insert(conn_id.to_string(), (seq, abort));
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            use futures::StreamExt;
            while let Some(tree) = previews.next().await {
                // A refresh keeps the last complete snapshot visible. On first
                // load, publish table names while the details arrive.
                if previous_schema.is_none() {
                    this.update(cx, |this, cx| {
                        if let Some(c) = this.connection_mut(&conn_id) {
                            c.schema = Some(Arc::new(tree));
                        }
                        this.reindex_schema(&conn_id, cx);
                        cx.notify();
                    }).ok();
                }
            }
            let res = fut.await;
            let successful_schema = res.as_ref().and_then(|r| r.as_ref().ok()).cloned();
            this.update(cx, |this, cx| {
                if this.schema_refresh_tasks.get(&conn_id).is_some_and(|(s, _)| *s == seq) {
                    this.schema_refresh_tasks.remove(&conn_id);
                }
                if this.hover_schema_spinner.as_deref() == Some(conn_id.as_str()) {
                    // The spinner is gone, so it will not report the pointer leaving.
                    this.hover_schema_spinner = None;
                }
                if let Some(c) = this.connection_mut(&conn_id) {
                    c.schema_loading = false;
                    match res {
                        // Stopped from the spinner: back to what was shown.
                        None => c.schema = previous_schema,
                        Some(Ok(tree)) => {
                            c.schema = Some(tree);
                            c.schema_refreshed_at = Some(std::time::SystemTime::now());
                        }
                        Some(Err(e)) => {
                            c.schema = previous_schema;
                            if !background {
                                c.schema_error = Some(e);
                            }
                        },
                    }
                }
                this.reindex_schema(&conn_id, cx);
                this.configure_editors(cx);
                cx.notify();
            })
            .ok();
            if let Some(tree) = successful_schema {
                // Persist only complete snapshots, after publishing to the UI.
                runtime::spawn(async move {
                    if let Ok(json) = serde_json::to_string(tree.as_ref()) {
                        let _ = commands::save_schema(runtime::state(), conn_id, json, String::new()).await;
                    }
                }).await;
            }
        })
    }

    /// Stop the connection's schema refresh (its spinner was clicked). The
    /// refresh task resolves as cancelled and restores the previous schema.
    pub fn stop_schema_refresh(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        if let Some((_, abort)) = self.schema_refresh_tasks.remove(conn_id) {
            abort.abort();
            // Not retried in the background straight away either.
            self.schema_refresh_attempts.insert(conn_id.to_string(), std::time::Instant::now());
            let name = self.connection(conn_id).map(|c| c.config.name.clone()).unwrap_or_default();
            self.status = format!("Schema refresh for {name} stopped").into();
        }
        self.hover_schema_spinner = None;
        cx.notify();
    }

    /// Refresh, in the background, schemas that have not been fetched for
    /// `SCHEMA_MAX_AGE`. Only connected connections are refreshed, so this
    /// never opens a connection (or an SSO sign-in) the person has not.
    fn start_background_schema_refresh(&mut self, cx: &mut Context<Self>) {
        cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(SCHEMA_REFRESH_TICK).await;
            let Ok(due) = this.update(cx, |this, _| this.stale_schema_ids()) else { break };
            for id in due {
                if this
                    .update(cx, |this, _| this.schema_refresh_attempts.insert(id.clone(), std::time::Instant::now()))
                    .is_err()
                {
                    return;
                }
                let state = runtime::state();
                let check_id = id.clone();
                let allowed =
                    runtime::spawn(async move { commands::schema_background_refresh_allowed(state, check_id).await }).await;
                if !allowed {
                    continue;
                }
                let task = this.update(cx, |this, cx| this.refresh_schema_with(&id, true, cx)).ok();
                // One at a time: no need to load several databases at once.
                if let Some(task) = task {
                    task.await;
                }
            }
        })
        .detach();
    }

    /// Connections showing a schema older than `SCHEMA_MAX_AGE` that are not
    /// already refreshing and were not tried recently.
    fn stale_schema_ids(&self) -> Vec<String> {
        let now = std::time::SystemTime::now();
        self.connections
            .iter()
            .filter(|c| c.schema.is_some() && !c.schema_loading)
            .filter(|c| {
                c.schema_refreshed_at
                    .is_none_or(|at| now.duration_since(at).is_ok_and(|age| age > SCHEMA_MAX_AGE))
            })
            .filter(|c| {
                self.schema_refresh_attempts
                    .get(&c.config.id)
                    .is_none_or(|at| at.elapsed() > SCHEMA_RETRY_AFTER)
            })
            .map(|c| c.config.id.clone())
            .collect()
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
        if !self.tabs_with_changes(Some(id), cx).is_empty() {
            self.guard_discard(crate::ui::grid_save::DiscardAction::Disconnect(id.to_string()), cx);
            return;
        }
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
                        } else if crate::ui::dialogs::is_long_error(&e) {
                            // Too long for the status bar: say so, and keep
                            // the whole message behind the info button.
                            let short: SharedString = format!("Connection to {} failed: error", cfg.name).into();
                            this.status_detail = Some((short.clone(), e));
                            short
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

    pub fn add_server_group(&mut self, title: &str, tab_color: &str) -> String {
        let id = new_id();
        let t = title.trim();
        self.settings.server_groups.push(ServerGroup {
            id: id.clone(),
            title: if t.is_empty() { "Server Group".into() } else { t.into() },
            connection_ids: Vec::new(),
            tab_color: tab_color.trim().to_string(),
        });
        self.settings.save();
        self.active_server_group_id = id.clone();
        id
    }

    pub fn update_server_group(&mut self, group_id: &str, title: &str, tab_color: &str) {
        let Some(g) = self.settings.server_groups.iter_mut().find(|g| g.id == group_id) else { return };
        let t = title.trim();
        if !t.is_empty() {
            g.title = t.to_string();
        }
        g.tab_color = tab_color.trim().to_string();
        self.settings.save();
    }

    /// Removes the group and deletes every connection in it, just as
    /// "Remove Connection" would one at a time.
    pub fn delete_server_group(&mut self, group_id: &str, cx: &mut Context<Self>) {
        let Some(pos) = self.settings.server_groups.iter().position(|g| g.id == group_id) else { return };
        let changed = self.settings.server_groups[pos].connection_ids.iter().any(|c| !self.tabs_with_changes(Some(c), cx).is_empty());
        if changed {
            self.guard_discard(crate::ui::grid_save::DiscardAction::DeleteGroup(group_id.to_string()), cx);
            return;
        }
        let group = self.settings.server_groups.remove(pos);
        self.settings.save();
        self.expanded_groups.remove(group_id);
        if self.active_server_group_id == group_id {
            self.active_server_group_id.clear();
        }
        let mut removed = 0;
        for conn_id in &group.connection_ids {
            if self.connection(conn_id).is_some() {
                self.disconnect_conn(conn_id, cx);
                removed += 1;
            }
        }
        self.status = match removed {
            0 => format!("Deleted server group {}", group.title),
            1 => format!("Deleted server group {} and 1 connection", group.title),
            n => format!("Deleted server group {} and {n} connections", group.title),
        }
        .into();
        cx.notify();
    }

    /// The server group a connection belongs to, if any.
    pub fn group_of(&self, conn_id: &str) -> Option<&ServerGroup> {
        self.settings.server_groups.iter().find(|g| g.connection_ids.iter().any(|c| c == conn_id))
    }

    /// The tab colour a connection's tabs get: its own, else its server
    /// group's, else none (empty).
    pub fn effective_tab_color(&self, conn_id: &str) -> String {
        let own = self.connection(conn_id).map(|c| c.config.tab_color.trim()).unwrap_or("");
        if !own.is_empty() {
            return own.to_string();
        }
        self.group_of(conn_id).map(|g| g.tab_color.trim().to_string()).unwrap_or_default()
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
        self.apply_fonts(cx);
    }

    /// Push the font settings out to everything that draws text.
    fn apply_fonts(&mut self, cx: &mut Context<Self>) {
        crate::ui::theme::set_ui_font(self.settings.ui_font_family.as_deref());
        self.mono_family = mono_family_of(&self.settings, cx);
        crate::ui::metrics::init(cx, self.mono_family.as_ref());
        let scale = self.scale();
        let editor_scale = self.editor_scale();
        let mono = self.mono_family.clone();
        for t in &self.tabs {
            if let Some(sql) = t.sql() {
                sql.editor.update(cx, |e, cx| {
                    e.set_mono_family(mono.clone(), cx);
                    e.set_font_scale(editor_scale, cx);
                });
            }
        }
        dialogs::rescale_inputs(self, scale, cx);
        self.nav_rows_cache = None;
        let ctx = self.output_ctx();
        self.output.update(cx, |o, cx| o.set_ctx(ctx, cx));
        cx.notify();
    }

    /// `None` returns to the default family.
    pub fn set_ui_font_family(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.settings.ui_font_family = family.filter(|f| !f.trim().is_empty());
        self.settings.save();
        self.apply_fonts(cx);
    }

    pub fn set_ui_font_size(&mut self, size: f64, cx: &mut Context<Self>) {
        let size = clamp_font_size(size);
        self.settings.ui_font_size = (size != DEFAULT_FONT_SIZE).then_some(size);
        self.settings.save();
        self.apply_fonts(cx);
    }

    /// `None` returns to the default family.
    pub fn set_editor_font_family(&mut self, family: Option<String>, cx: &mut Context<Self>) {
        self.settings.editor_font_family = family.filter(|f| !f.trim().is_empty());
        self.settings.save();
        self.apply_fonts(cx);
    }

    pub fn set_editor_font_size(&mut self, size: f64, cx: &mut Context<Self>) {
        let size = clamp_font_size(size);
        self.settings.editor_font_size = (size != DEFAULT_FONT_SIZE).then_some(size);
        self.settings.save();
        self.apply_fonts(cx);
    }

    /// Open or close one of the settings menu's selects.
    pub fn toggle_font_select(&mut self, which: FontSelect, cx: &mut Context<Self>) {
        if self.font_select_open == Some(which) {
            self.font_select_open = None;
        } else {
            if matches!(which, FontSelect::UiFamily | FontSelect::EditorFamily) && self.font_families.is_none() {
                self.refresh_font_families(cx);
            }
            self.font_select_open = Some(which);
        }
        cx.notify();
    }

    /// List the installed fonts again in the background. Whatever list is
    /// already there stays usable meanwhile; if the new one differs (fonts
    /// installed or removed since) it replaces it and goes to the cache.
    pub fn refresh_font_families(&mut self, cx: &mut Context<Self>) {
        if self.font_refresh.is_some() {
            return;
        }
        let ts = cx.text_system().clone();
        self.font_refresh = Some(cx.spawn(async move |this, cx| {
            let lists = cx.background_spawn(async move { list_font_families(&ts) }).await;
            this.update(cx, |this, cx| {
                this.font_refresh = None;
                if this.font_families.as_deref() == Some(&lists) {
                    return;
                }
                let lists = Arc::new(lists);
                this.font_families = Some(lists.clone());
                cx.background_spawn(async move { lists.save_cache() }).detach();
                cx.notify();
            })
            .ok();
        }));
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
        // Running again replaces the result, and with it any unsaved changes.
        if sql_tab.store.read(cx).has_pending() {
            self.guard_discard(crate::ui::grid_save::DiscardAction::RunQuery(tab_id.to_string()), cx);
            return;
        }
        if sql_tab.store.read(cx).is_saving() {
            self.set_tab_status(tab_id, "Wait for the save to finish, or stop it");
            cx.notify();
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
        let default_schema = sql_tab.default_schema.clone();
        let store = sql_tab.store.clone();
        let query_id = new_id();
        if let Some(t) = self.tab_mut(tab_id) {
            t.conn_id = conn_id.clone();
            if let Some(s) = t.sql_mut() {
                s.running = true;
                s.query_id = query_id.clone();
            }
        }
        let driver = self.connection(&conn_id).map(|c| c.config.driver.clone()).unwrap_or_default();
        let source = ResultSource { sql: sql.clone(), conn_id: conn_id.clone(), driver, database: database.clone(), default_schema: default_schema.clone() };
        store.update(cx, |st, cx| {
            st.start();
            st.source = Some(source);
            cx.notify();
        });
        self.set_tab_status(tab_id, "Running query…");
        self.output_tab = OutputTab::Results;
        cx.notify();

        let (tx, rx) = futures::channel::mpsc::unbounded::<StreamEvent>();
        let emitter: commands::EventEmitter = Arc::new(move |ev| {
            let _ = tx.unbounded_send(ev);
        });
        let state = runtime::state();
        let (c2, q2, s2, d2) = (conn_id.clone(), query_id.clone(), sql.clone(), database.clone());
        let call = runtime::spawn(async move {
            // No row cap: stream every row the query returns.
            commands::execute_query_streamed(emitter, state, c2, d2, default_schema, q2, s2, 0).await
        });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            stream_query(this, cx, tab_id, query_id, conn_id, sql, store, rx, call).await;
        })
        .detach();
    }

    /// A status line about one tab's query (see `status_scope`).
    pub fn set_tab_status(&mut self, tab_id: &str, text: impl Into<SharedString>) {
        let text = text.into();
        self.status = text.clone();
        self.status_scope = Some((tab_id.to_string(), text));
    }

    /// Open the full text of a long error in a popup with a Copy button.
    pub fn show_error_popup(&mut self, title: impl Into<String>, text: impl Into<String>, cx: &mut Context<Self>) {
        self.error_popup = Some((title.into(), text.into()));
        self.error_popup_scroll = ScrollHandle::new();
        cx.notify();
    }

    /// The full error behind the visible status line, when it was shortened.
    pub fn visible_status_detail(&self) -> Option<String> {
        let (status, detail) = self.status_detail.as_ref()?;
        (*status == self.visible_status()).then(|| detail.clone())
    }

    /// The status line as the active tab should see it.
    pub fn visible_status(&self) -> SharedString {
        match &self.status_scope {
            Some((tab, text)) if *text == self.status && *tab != self.active_tab_id => "Ready".into(),
            _ => self.status.clone(),
        }
    }

    pub fn cancel_query(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(qid) = self.tab(tab_id).and_then(|t| t.sql()).map(|s| s.query_id.clone()) else { return };
        if qid.is_empty() {
            return;
        }
        let state = runtime::state();
        let cancelled_id = qid.clone();
        let fut = runtime::spawn(async move { commands::cancel_query(state, qid).await });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            let _ = fut.await;
            this.update(cx, |this, cx| {
                if !this.tab(&tab_id).and_then(|t| t.sql()).is_some_and(|s| s.query_id == cancelled_id) {
                    return;
                }
                if let Some(s) = this.tab_mut(&tab_id).and_then(|t| t.sql_mut()) {
                    s.running = false;
                    s.query_id.clear();
                    s.store.update(cx, |st, cx| {
                        st.set_phase(commands::QueryPhase::Cancelled);
                        st.loading = false;
                        cx.notify();
                    });
                }
                this.set_tab_status(&tab_id, "Query cancelled");
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

    // ─── Misc ───────────────────────────────────────────────────────────────

    pub fn copy_to_clipboard(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    pub fn export_csv(&mut self, cx: &mut Context<Self>) {
        if self.export_running {
            return;
        }
        let Some(tab) = self.active_tab() else { return };
        let filename = crate::ui::textfmt::csv_export_filename(&tab.title);
        // Cloning the result only bumps the `Arc`s on its row chunks; no row
        // is copied. The rows are written straight to the file as they are
        // formatted, so the export never holds a second copy of the result or
        // the whole CSV text in memory, and nothing is built at all until a
        // file has been chosen. The snapshot is dropped when the write ends.
        let Some(result) = self.active_store().and_then(|s| s.read(cx).result.clone()) else { return };
        if result.columns.is_empty() {
            return;
        }
        let total = result.rows.len();
        // The writer thread counts rows as it goes; the UI samples the count a
        // few times a second rather than being woken for every row.
        let progress = Arc::new(ExportProgress::default());
        let shared = progress.clone();
        let fut = runtime::spawn_blocking(move || -> Result<Option<(String, u64)>, String> {
            let Some(path) = commands::pick_csv_path(filename) else { return Ok(None) };
            let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            if let Ok(mut n) = shared.name.lock() {
                *n = name.clone();
            }
            shared.started.store(true, Ordering::Relaxed);
            let file = std::fs::File::create(&path).map_err(|e| format!("{name}: {e}"))?;
            let mut out = std::io::BufWriter::with_capacity(256 * 1024, file);
            let rows = result.rows.iter().inspect(|_| {
                shared.written.fetch_add(1, Ordering::Relaxed);
            });
            crate::ui::textfmt::write_csv(&mut out, &result.columns, rows)
                .and_then(|_| std::io::Write::flush(&mut out))
                .map_err(|e| format!("{name}: {e}"))?;
            let bytes = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
            Ok(Some((name, bytes)))
        });
        self.export_running = true;
        self.export_seq += 1;
        let seq = self.export_seq;
        cx.spawn(async move |this, cx| {
            use futures::FutureExt;
            let mut fut = Box::pin(fut.fuse());
            let outcome = loop {
                let mut tick = Box::pin(cx.background_executor().timer(EXPORT_TICK).fuse());
                futures::select! {
                    res = fut => break res,
                    _ = tick => {
                        if !progress.started.load(Ordering::Relaxed) {
                            continue; // still in the save dialog
                        }
                        let written = progress.written.load(Ordering::Relaxed).min(total);
                        let name = progress.name.lock().map(|n| n.clone()).unwrap_or_default();
                        let pct = if total == 0 { 100 } else { written * 100 / total };
                        let text = format!(
                            "Exporting {name}… {pct}% ({} of {} rows)",
                            group_digits(written),
                            group_digits(total)
                        );
                        if this.update(cx, |this, cx| {
                            this.export_status = Some(text.into());
                            cx.notify();
                        }).is_err() {
                            return;
                        }
                    }
                }
            };
            let text: Option<SharedString> = match outcome {
                Ok(None) => None, // dialog cancelled
                Ok(Some((name, bytes))) => Some(
                    format!(
                        "✓ Saved {name} · {} rows · {}",
                        group_digits(total),
                        crate::ui::model::format_bytes(Some(bytes as i64))
                    )
                    .into(),
                ),
                Err(e) => Some(format!("Export failed: {e}").into()),
            };
            let shown = text.is_some();
            if this.update(cx, |this, cx| {
                this.export_running = false;
                this.export_status = text;
                cx.notify();
            }).is_err() {
                return;
            }
            // Leave the outcome up for a while, then give the bar back to the
            // query status — unless another export has replaced it since.
            if shown {
                cx.background_executor().timer(EXPORT_RESULT_LINGER).await;
                this.update(cx, |this, cx| {
                    if this.export_seq == seq && !this.export_running {
                        this.export_status = None;
                        cx.notify();
                    }
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn close_floating(&mut self) {
        // Escape on a save prompt is the safe answer: keep the changes, save nothing.
        self.discard_confirm = None;
        self.nontx_confirm = None;
        self.save_conflicts = None;
        self.nav_menu = None;
        self.nav_template_open = false;
        self.add_menu_open = false;
        self.settings_open = false;
        self.font_select_open = None;
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

/// A schema not refreshed for this long is refreshed in the background.
const SCHEMA_MAX_AGE: Duration = Duration::from_secs(6 * 60 * 60);
/// How often the background refresh looks for stale schemas.
const SCHEMA_REFRESH_TICK: Duration = Duration::from_secs(5 * 60);
/// A background refresh that failed (or was skipped as unsafe) waits this
/// long before it is tried again.
const SCHEMA_RETRY_AFTER: Duration = Duration::from_secs(30 * 60);

/// Parse SQLite's `datetime('now')` format (`YYYY-MM-DD HH:MM:SS`, UTC).
fn parse_sqlite_utc(text: &str) -> Option<std::time::SystemTime> {
    chrono::NaiveDateTime::parse_from_str(text.trim(), "%Y-%m-%d %H:%M:%S")
        .ok()
        .map(|t| t.and_utc().into())
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
        let abort = task.abort_handle();
        let out = tokio::select! {
            r = task => r.map_err(|e| format!("backend command task failed: {e}")).and_then(|r| r),
            _ = test_watchdog(&test_id, Duration::from_secs(65)) => {
                // Stop the test along with the wait for it, rather than leaving
                // it running (and an `aws sso login` open) behind the error.
                abort.abort();
                let stage = crate::ipc_diagnostics::get_test_connection_stage(&test_id).unwrap_or_else(|| "unknown".into());
                let checks = crate::ipc_diagnostics::recommended_checks_for_stage(&stage);
                Err(format!("test_connection timed out in backend watchdog after 65s (last stage: {stage}). Recommended next checks: {checks}"))
            }
        };
        crate::ipc_diagnostics::clear_test_connection_stage(&test_id);
        out
    })
}

/// Resolves once the test has spent `limit` working without an answer.
///
/// Time spent waiting on `aws sso login` does not count: that is a person
/// approving a sign-in in their browser, which has its own (longer) timeout.
/// Counting it let the watchdog abort the login — killing the `aws` process
/// before it wrote the new token — whenever approving took more than about a
/// minute, so a test never renewed an expired SSO session the way a connect does.
async fn test_watchdog(test_id: &str, limit: Duration) {
    const TICK: Duration = Duration::from_millis(250);
    let mut counted = Duration::ZERO;
    while counted < limit {
        tokio::time::sleep(TICK).await;
        let signing_in =
            crate::ipc_diagnostics::get_test_connection_stage(test_id).as_deref() == Some("aws_sso_login");
        if !signing_in {
            counted += TICK;
        }
    }
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
                        let current = this.update(cx, |this, _| {
                            this.tab(&tab_id).and_then(|t| t.sql()).is_some_and(|s| s.query_id == query_id)
                        }).unwrap_or(false);
                        if !current { return; }
                        store.update(cx, |st, cx| {
                            st.fail(e.clone());
                            cx.notify();
                        }).ok();
                        this.update(cx, |this, cx| {
                            if let Some(s) = this.tab_mut(&tab_id).and_then(|t| t.sql_mut()) {
                                s.running = false;
                                s.query_id.clear();
                            }
                            this.set_tab_status(&tab_id, format!("Error: {e}"));
                            this.output_tab = OutputTab::Messages;
                            cx.notify();
                        }).ok();
                        return;
                    }
                    continue;
                }
            }
        };
        // Ignore events from a cancelled run or a tab that has since run again.
        let current = this.update(cx, |this, _| {
            this.tab(&tab_id).and_then(|t| t.sql()).is_some_and(|s| s.query_id == query_id)
        }).unwrap_or(false);
        if !current { return; }
        // Take everything that has arrived, in order, and apply it in one go.
        let mut events = vec![first];
        while let Ok(ev) = rx.try_recv() {
            events.push(ev);
        }
        let mut chunks: Vec<Vec<Vec<serde_json::Value>>> = Vec::new();
        let mut done = None;
        for ev in events {
            match ev {
                StreamEvent::Progress { query_id: id, phase } if id == query_id => {
                    store.update(cx, |st, cx| {
                        st.set_phase(phase);
                        cx.notify();
                    }).ok();
                }
                StreamEvent::Meta(meta) if meta.query_id == query_id => {
                    store
                        .update(cx, |st, cx| {
                            st.begin(meta.columns, meta.column_types);
                            cx.notify();
                        })
                        .ok();
                    // The columns are known: whether they can be edited can be
                    // worked out while the rows stream in.
                    this.update(cx, |this, cx| this.schedule_edit_check(cx)).ok();
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
                this.set_tab_status(&tab_id, format!("Error: {error}"));
                this.output_tab = OutputTab::Messages;
            } else {
                let text = if columns_empty {
                    format!("{} row(s) affected · {}ms", done.rows_affected, done.duration)
                } else {
                    format!("{n} rows · {}ms", done.duration)
                };
                this.set_tab_status(&tab_id, text);
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

