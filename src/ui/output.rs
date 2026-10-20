//! The output panel (results grid, messages, history, saved queries), as a
//! view of its own.
//!
//! What the panel shows of the application — which tab is active, which
//! result that tab holds, the font scale — is read from [`Workspace`]; the
//! result itself lives in a [`ResultStore`] entity the panel observes. All of
//! the grid's own state (scroll, selection, column widths, an open cell editor,
//! a drag in progress) lives here. A streaming chunk, a scroll or a selection
//! therefore redraws this view and nothing else.

use crate::grid_edit::{Cell, RowRef};
use crate::ui::grid::{self, Bar, CellSel, EditOverlay, GridBody, GridState, Jump, ScrollDrag};
use crate::ui::pending::{edit_text, ViewRows};
use crate::ui::js;
use crate::ui::model::{OutputTab, QueryResult, SortDirection, TabId};
use crate::ui::render::*;
use crate::ui::result_store::ResultStore;
use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{shadow, Scale, TextExt};
use crate::ui::icons::{icon, Icon};
use crate::ui::workspace::Workspace;
use crate::models::{QueryRecord, SavedQuery};
use gpui::{
    anchored, deferred, div, point, prelude::*, px, AnyElement, App, ClipboardItem, Context, Corner, CursorStyle, Entity,
    FontWeight, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent,
    SharedString, Subscription, WeakEntity, Window,
};
use std::sync::Arc;

#[derive(Clone)]
pub enum OutputMenu {
    History { pos: Point<Pixels> },
    Saved { pos: Point<Pixels>, id: i64, title: String },
    /// A results cell: (row, column) where it was opened.
    GridCell { pos: Point<Pixels>, row: RowRef, col: usize },
    /// The row-number gutter: the rows it acts on.
    GridRows { pos: Point<Pixels>, rows: Vec<RowRef>, clicked: RowRef },
    /// Empty grid space or the header.
    Grid { pos: Point<Pixels> },
}

/// What a cached row order was built from: (generation, rev, sort order,
/// pending changes, rows).
type ViewKey = (u64, u64, usize, u64, usize);

/// What the panel reads of the workspace. Copied out whenever the workspace
/// changes; cheap, since the heavy parts are `Arc`s or entity handles.
#[derive(Clone)]
pub struct OutputCtx {
    pub tab_id: TabId,
    /// Whether the active tab is a query tab (the others have no results).
    pub is_sql: bool,
    pub store: Option<Entity<ResultStore>>,
    /// The connection whose history and saved queries are listed.
    pub conn_id: String,
    pub output_tab: OutputTab,
    /// Bumped when the lists may have changed behind the panel's back (a query
    /// finished, one was saved or renamed).
    pub lists_rev: u64,
    pub scale: f32,
    pub mono: SharedString,
}

impl Default for OutputCtx {
    fn default() -> Self {
        OutputCtx {
            tab_id: String::new(),
            is_sql: false,
            store: None,
            conn_id: String::new(),
            output_tab: OutputTab::Results,
            lists_rev: 0,
            scale: 1.0,
            mono: "Menlo".into(),
        }
    }
}

pub struct OutputPanel {
    ws: WeakEntity<Workspace>,
    ctx: OutputCtx,
    pub grid: GridState,
    history: Vec<QueryRecord>,
    saved: Vec<SavedQuery>,
    menu: Option<OutputMenu>,
    /// What the lists were last loaded for.
    list_key: (OutputTab, String, u64),
    store_sub: Option<Subscription>,
    view_cache: Option<(ViewKey, ViewRows)>,
    _ws_sub: Subscription,
}

impl OutputPanel {
    pub fn new(ws: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(ws, |this, ws, cx| {
            let ctx = ws.read(cx).output_ctx();
            this.set_ctx(ctx, cx);
            cx.notify();
        });
        // The workspace is still being built when this runs, so it cannot be
        // read yet; it hands over its state with `set_ctx` when it is ready.
        let panel = OutputPanel {
            ws: ws.downgrade(),
            ctx: OutputCtx::default(),
            grid: GridState::default(),
            history: Vec::new(),
            saved: Vec::new(),
            menu: None,
            list_key: (OutputTab::Results, String::new(), 0),
            store_sub: None,
            view_cache: None,
            _ws_sub: sub,
        };
        panel
    }

    fn with_ws<R>(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>) -> R) -> Option<R> {
        self.ws.update(cx, f).ok()
    }

    /// Take in the workspace's current state.
    pub fn set_ctx(&mut self, ctx: OutputCtx, cx: &mut Context<Self>) {
        let same_store = match (&self.ctx.store, &ctx.store) {
            (Some(a), Some(b)) => a.entity_id() == b.entity_id(),
            (None, None) => true,
            _ => false,
        };
        let tab_changed = self.ctx.tab_id != ctx.tab_id;
        if ctx.scale != self.ctx.scale && self.grid.font_scale > 0.0 {
            let old = self.grid.font_scale;
            self.grid.rescale(ctx.scale, old);
        }
        if !same_store {
            self.store_sub = ctx.store.as_ref().map(|s| {
                cx.observe(s, |this, store, cx| {
                    // A result that was cleared (a statement is about to run)
                    // resets the grid, widths included, as a new result does.
                    if store.read(cx).result.is_none() && this.grid.generation != 0 {
                        let scale = this.ctx.scale;
                        this.grid.on_result_changed(&this.ctx.tab_id.clone(), 0, &Arc::new(Vec::new()), false, scale);
                    }
                    cx.notify();
                })
            });
        }
        if tab_changed || !same_store {
            let (cols, generation, has) = match ctx.store.as_ref().and_then(|s| s.read(cx).result.clone()) {
                Some(r) => (r.columns.clone(), r.generation, true),
                None => (Arc::new(Vec::new()), 0, false),
            };
            self.grid.on_result_changed(&ctx.tab_id, generation, &cols, has, ctx.scale);
        }
        let key = (ctx.output_tab, ctx.conn_id.clone(), ctx.lists_rev);
        let reload = key != self.list_key;
        self.list_key = key;
        let tab = ctx.output_tab;
        self.ctx = ctx;
        if reload {
            match tab {
                OutputTab::History => self.load_history(cx),
                OutputTab::Saved => self.load_saved(cx),
                _ => {}
            }
        }
    }

    pub fn close_overlays(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    fn result(&self, cx: &App) -> Option<QueryResult> {
        self.ctx.store.as_ref()?.read(cx).result.clone()
    }

    fn sort(&self, cx: &App) -> Option<(usize, SortDirection)> {
        self.ctx.store.as_ref()?.read(cx).sort()
    }

    /// Rows shown: the result's rows plus any new ones.
    fn total_rows(&self, cx: &App) -> usize {
        self.ctx.store.as_ref().map_or(0, |s| {
            let st = s.read(cx);
            st.row_count() + st.pending.new_rows.len()
        })
    }

    /// The rows in the order shown (sort, then new and pinned rows placed).
    fn view_for(&mut self, r: &QueryResult, cx: &App) -> ViewRows {
        let sort = self.sort(cx);
        let sorted = self.grid.sort_index(r, sort);
        let Some(store) = self.ctx.store.clone() else { return ViewRows::Plain(r.rows.len()) };
        let st = store.read(cx);
        let key = (r.generation, r.rev, sorted.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize), st.pending.rev, r.rows.len());
        if let Some((k, v)) = &self.view_cache {
            if *k == key {
                return v.clone();
            }
        }
        let v = st.view(sorted);
        self.view_cache = Some((key, v.clone()));
        v
    }

    /// The rows in the order shown right now.
    pub fn current_view(&mut self, cx: &mut Context<Self>) -> ViewRows {
        match self.result(cx) {
            Some(r) => self.view_for(&r, cx),
            None => ViewRows::default(),
        }
    }

    /// A save went through: row positions may have moved, so drop the
    /// selection and any open editor.
    pub fn after_save(&mut self, cx: &mut Context<Self>) {
        self.grid.clear_selection();
        self.grid.edit = None;
        self.view_cache = None;
        self.menu = None;
        self.clamp_grid_scroll(cx);
        cx.notify();
    }

    /// Select and scroll to a row (the one a save failed on).
    pub fn reveal_row(&mut self, row: RowRef, cx: &mut Context<Self>) {
        let view = self.current_view(cx);
        if let Some(pos) = view.position(row) {
            let total = view.len();
            let size = self.grid_view_size(cx);
            self.grid.clear_selection();
            self.grid.selected_rows.insert(pos);
            self.grid.row_anchor = Some(pos);
            self.grid.ensure_row_visible(pos, total, size);
        }
        cx.notify();
    }

    // ─── Lists ──────────────────────────────────────────────────────────────

    fn load_history(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.ctx.conn_id.clone();
        if conn_id.is_empty() {
            self.history.clear();
            return;
        }
        let state = crate::ui::runtime::state();
        let fut = crate::ui::runtime::spawn(async move { crate::commands::get_query_history_by_conn_id(state, conn_id, 50).await });
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                this.history = res.unwrap_or_default();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn load_saved(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.ctx.conn_id.clone();
        if conn_id.is_empty() {
            self.saved.clear();
            return;
        }
        let state = crate::ui::runtime::state();
        let fut = crate::ui::runtime::spawn(async move { crate::commands::get_saved_queries(state, conn_id).await });
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                this.saved = res.unwrap_or_default();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn clear_history(&mut self, all: bool, cx: &mut Context<Self>) {
        let conn = self.ctx.conn_id.clone();
        let state = crate::ui::runtime::state();
        let fut = crate::ui::runtime::spawn(async move {
            if all || conn.is_empty() {
                crate::commands::clear_query_history(state).await
            } else {
                crate::commands::clear_query_history_by_conn_id(state, conn).await
            }
        });
        cx.spawn(async move |this, cx| {
            let _ = fut.await;
            this.update(cx, |this, cx| {
                this.history.clear();
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn delete_saved(&mut self, id: i64, cx: &mut Context<Self>) {
        let state = crate::ui::runtime::state();
        let fut = crate::ui::runtime::spawn(async move { crate::commands::delete_saved_query(state, id).await });
        cx.spawn(async move |this, cx| {
            let _ = fut.await;
            this.update(cx, |this, cx| this.load_saved(cx)).ok();
        })
        .detach();
    }

    fn copy_to_clipboard(&self, text: String, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    // ─── Panel ──────────────────────────────────────────────────────────────

    fn render_messages(&mut self, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let msg = match self.result(cx) {
            Some(r) if !r.error.is_empty() => self.render_error_box(s, r.error, cx),
            Some(r) => msg_box(
                s,
                theme::SUCCESS,
                None,
                format!("Query OK — {} row(s) affected, {} row(s) returned in {}ms", r.rows_affected, r.rows.len(), r.duration),
            ),
            None => msg_box(s, theme::TEXT_MUTED, None, "No messages.".into()),
        };
        div().id("messages").size_full().overflow_y_scroll().px(px(12.)).py(px(8.)).child(msg).into_any_element()
    }

    fn render_error_box(&mut self, s: Scale, error: String, cx: &mut Context<Self>) -> AnyElement {
        let display = format!("ERROR: {error}");
        let input = cx.new(|cx| {
            let mut input = TextInput::new(cx, InputLook {
                font_size: 12.0 * s.0,
                line_height: crate::ui::metrics::line_height_normal(12.0 * s.0),
                height: 26.0 * s.0,
                pad_x: (0.0, 6.0),
                radius: 0.0,
                border_width: 0.0,
                text: theme::ERROR,
                bg: theme::rgba8(0, 0, 0, 0.0),
                focus_bg: None,
                border: theme::rgba8(0, 0, 0, 0.0),
                focus_border: theme::rgba8(0, 0, 0, 0.0),
                focus_ring: None,
                ..InputLook::dialog(s.0)
            });
            input.readonly = true;
            input.set_text(display, cx);
            input
        });
        let copy_error = error.clone();

        div()
            .flex()
            .items_center()
            .gap(px(6.))
            .t(s, 12.0)
            .px(px(10.))
            .py(px(4.))
            .rounded(px(4.))
            .bg(theme::rgba8(255, 80, 80, 0.08))
            .child(div().flex_1().min_w(px(0.)).child(input))
            .child(
                div()
                    .id("copy-error-message")
                    .flex_shrink_0()
                    .w(px(26.))
                    .h(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .rounded(px(3.))
                    .text_color(theme::ERROR)
                    .cursor_pointer()
                    .hover(|style| style.bg(theme::rgba8(255, 80, 80, 0.16)))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.copy_to_clipboard(copy_error.clone(), cx);
                    }))
                    .child(
                        div()
                            .relative()
                            .w(px(14.))
                            .h(px(14.))
                            .child(
                                div()
                                    .absolute()
                                    .left(px(1.))
                                    .top(px(1.))
                                    .w(px(9.))
                                    .h(px(9.))
                                    .border_1()
                                    .border_color(theme::ERROR)
                                    .rounded(px(1.)),
                            )
                            .child(
                                div()
                                    .absolute()
                                    .left(px(4.))
                                    .top(px(4.))
                                    .w(px(9.))
                                    .h(px(9.))
                                    .border_1()
                                    .border_color(theme::ERROR)
                                    .rounded(px(1.))
                                    .bg(theme::BG_PANEL),
                            ),
                    ),
            )
            .into_any_element()
    }

    fn render_history(&mut self, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div()
            .id("history-list")
            .size_full()
            .overflow_y_scroll()
            .px(px(12.))
            .py(px(8.))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                this.menu = Some(OutputMenu::History { pos: e.position });
                cx.notify();
            }));
        let mono = self.ctx.mono.clone();
        for rec in self.history.clone() {
            let q = if js::char_len(&rec.query) > 120 { format!("{}…", js::slice_chars(&rec.query, 120)) } else { rec.query.clone() };
            let (query, conn) = (rec.query.clone(), rec.conn_id.clone());
            let group = SharedString::from(format!("hist-{}", rec.id));
            list = list.child(
                div()
                    .px(px(8.))
                    .py(px(6.))
                    .border_b_1()
                    .border_color(theme::BORDER_SUBTLE)
                    .child(
                        div()
                            .id(SharedString::from(format!("hq-{}", rec.id)))
                            .group(group.clone())
                            .cursor_pointer()
                            .text_color(theme::TEXT)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (q, c) = (query.clone(), conn.clone());
                                this.with_ws(cx, |ws, cx| ws.use_query(q, c, cx));
                            }))
                            .child(
                                div()
                                    .font_family(mono.clone())
                                    .text_size(s.fs(11.0))
                                    .line_height(px(crate::ui::metrics::mono_line_height_normal(11.0 * s.0)))
                                    .group_hover(group, |st| st.text_color(theme::ACCENT))
                                    .child(q),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .gap(px(12.))
                            .t(s, 10.0)
                            .text_color(theme::TEXT_MUTED)
                            .mt(px(2.))
                            .child(rec.created_at.clone())
                            .child(format!("{}ms", rec.duration))
                            .child(format!("{} rows", rec.result_count))
                            .when(!rec.error.is_empty(), |d| d.child(div().text_color(theme::ERROR).font_weight(FontWeight::SEMIBOLD).child("ERR"))),
                    ),
            );
        }
        if self.history.is_empty() {
            list = list.child(msg_box(s, theme::TEXT_MUTED, None, "No query history for this connection yet.".into()));
        }
        list.into_any_element()
    }

    fn render_saved(&mut self, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let mut list = div().id("saved-list").size_full().overflow_y_scroll().px(px(12.)).py(px(8.));
        for sq in self.saved.clone() {
            let preview = if js::char_len(&sq.query) > 100 { format!("{}…", js::slice_chars(&sq.query, 100)) } else { sq.query.clone() };
            let (query, conn, id, title) = (sq.query.clone(), sq.conn_id.clone(), sq.id, sq.title.clone());
            list = list.child(
                div()
                    .id(SharedString::from(format!("sv-{}", sq.id)))
                    .p(px(8.))
                    .border_b_1()
                    .border_color(theme::BORDER_SUBTLE)
                    .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu = Some(OutputMenu::Saved { pos: e.position, id, title: title.clone() });
                        cx.notify();
                    }))
                    .child(
                        div()
                            .id(SharedString::from(format!("svt-{}", sq.id)))
                            .cursor_pointer()
                            .text_color(theme::TEXT)
                            .font_weight(FontWeight::MEDIUM)
                            .t(s, 12.0)
                            .hover(|st| st.text_color(theme::ACCENT))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let (q, c) = (query.clone(), conn.clone());
                                this.with_ws(cx, |ws, cx| ws.use_query(q, c, cx));
                            }))
                            .child(sq.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(2.))
                            .t(s, 10.0)
                            .text_color(theme::TEXT_MUTED)
                            .mt(px(4.))
                            .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(preview))
                            .child(div().whitespace_nowrap().overflow_hidden().text_ellipsis().child(js_locale_string(&sq.created_at))),
                    ),
            );
        }
        if self.saved.is_empty() {
            list = list.child(msg_box(s, theme::TEXT_MUTED, None, "No saved queries for this connection yet.".into()));
        }
        list.into_any_element()
    }

    /// The history / saved-query context menu.
    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.menu.clone()?;
        let s = Scale(self.ctx.scale);
        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .w_full()
                .px(px(14.))
                .py(px(7.))
                .t(s, 12.0)
                .text_color(theme::TEXT)
                .cursor_pointer()
                .hover(|st| st.bg(theme::BG_HOVER))
                .child(label)
        };
        let (pos, body) = match menu {
            OutputMenu::History { pos } => (
                pos,
                div()
                    .child(item("clear-conn-hist", "Clear connection history").on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        this.clear_history(false, cx);
                        cx.notify();
                    })))
                    .child(item("clear-all-hist", "Clear all history").on_click(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        this.clear_history(true, cx);
                        cx.notify();
                    }))),
            ),
            OutputMenu::Saved { pos, id, title } => (
                pos,
                div()
                    .child(item("edit-title", "Edit title").on_click(cx.listener(move |this, _, window, cx| {
                        this.menu = None;
                        let title = title.clone();
                        this.with_ws(cx, |ws, cx| ws.open_title_edit(id, title, window, cx));
                        cx.notify();
                    })))
                    .child(item("delete-saved", "Delete").on_click(cx.listener(move |this, _, _, cx| {
                        this.menu = None;
                        this.delete_saved(id, cx);
                        cx.notify();
                    }))),
            ),
            OutputMenu::GridCell { pos, .. } | OutputMenu::GridRows { pos, .. } | OutputMenu::Grid { pos } => (pos, self.grid_menu_body(&menu, s, cx)),
        };
        let menu = div()
            .id("ctx-menu")
            .occlude()
            .min_w(px(180.))
            .py(px(4.))
            .bg(theme::BG_PANEL)
            .border_1()
            .border_color(theme::BORDER)
            .rounded(px(4.))
            .shadow(vec![shadow(0.0, 4.0, 12.0, 0.0, theme::rgba8(0, 0, 0, 0.3))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_overlays(cx)))
            .child(body);
        Some(deferred(anchored().position(pos).anchor(Corner::TopLeft).child(menu)).with_priority(3).into_any_element())
    }

    // ─── Editing from the grid's context menus ──────────────────────────────

    /// The rows a row-number menu acts on: every highlighted row if the
    /// clicked one is highlighted, otherwise just the clicked one (which then
    /// becomes the highlight).
    fn rows_for_menu(&mut self, display_row: usize, view: &ViewRows) -> Vec<RowRef> {
        if !self.grid.selected_rows.contains(&display_row) {
            self.grid.selected_rows.clear();
            self.grid.selected_rows.insert(display_row);
            self.grid.row_anchor = Some(display_row);
            self.grid.sel = None;
        }
        self.grid.selected_rows.iter().filter_map(|&r| view.get(r)).collect()
    }

    fn on_grid_right_click(&mut self, e: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.grid.body_bounds else { return };
        let Some(r) = self.result(cx) else { return };
        if self.grid.edit.is_some() {
            self.commit_cell_edit(cx);
        }
        if let Some(f) = &self.grid.focus {
            window.focus(f);
        }
        let total = self.total_rows(cx);
        let local = point(e.position.x - b.left(), e.position.y - b.top());
        let geom = self.grid.geom(total);
        if geom.bar_at(local).is_some() || f32::from(local.x) >= geom.view_w || f32::from(local.y) >= geom.view_h {
            return;
        }
        let view = self.view_for(&r, cx);
        let pos = e.position;
        self.menu = if let Some(row) = self.grid.row_at(local, total) {
            let rows = self.rows_for_menu(row, &view);
            let Some(clicked) = view.get(row) else { return };
            Some(OutputMenu::GridRows { pos, rows, clicked })
        } else if let Some(hit) = self.grid.cell_at(local, total) {
            let Some(row_ref) = view.get(hit.0) else { return };
            // A right-click outside the cell selection moves it there; the
            // row-number highlight is left alone.
            let inside = self.grid.sel.is_some_and(|sel| {
                let (r0, r1, c0, c1) = sel.norm();
                (r0..=r1).contains(&hit.0) && (c0..=c1).contains(&hit.1)
            });
            if !inside && self.grid.selected_rows.is_empty() {
                self.grid.sel = Some(CellSel { r0: hit.0, c0: hit.1, r1: hit.0, c1: hit.1 });
                self.grid.last_selected = Some(hit);
            }
            Some(OutputMenu::GridCell { pos, row: row_ref, col: hit.1 })
        } else {
            Some(OutputMenu::Grid { pos })
        };
        cx.notify();
    }

    /// The cells "Set to NULL" / "Set to NOW()" apply to from a menu opened on (`row`, `col`):
    /// the cell selection, when it is within that one column and includes the
    /// cell; otherwise just the cell. Cells that can't be changed are skipped.
    fn cell_targets(&mut self, row: RowRef, col: usize, cx: &App) -> Vec<(RowRef, usize)> {
        let Some(r) = self.result(cx) else { return vec![] };
        let view = self.view_for(&r, cx);
        let mut cells = vec![(row, col)];
        if let (Some(sel), Some(pos)) = (self.grid.sel, view.position(row)) {
            let (r0, r1, c0, c1) = sel.norm();
            if c0 == col && c1 == col && (r0..=r1).contains(&pos) {
                cells = (r0..=r1).filter_map(|d| view.get(d)).map(|rr| (rr, col)).collect();
            }
        }
        cells.retain(|(rr, c)| self.cell_lock(*rr, *c, cx).is_none());
        cells
    }

    fn update_pending(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut crate::ui::pending::Pending, &QueryResult)) {
        let Some(store) = self.ctx.store.clone() else { return };
        store.update(cx, |st, cx| {
            let Some(result) = st.result.clone() else { return };
            if st.is_saving() || st.edit_info.is_none() {
                return;
            }
            f(&mut st.pending, &result);
            st.save_error = None;
            cx.notify();
        });
        self.menu = None;
        cx.notify();
    }

    fn set_null(&mut self, cells: Vec<(RowRef, usize)>, cx: &mut Context<Self>) {
        self.update_pending(cx, |p, r| {
            for (row, col) in cells {
                let original = match row {
                    RowRef::Data(i) => r.rows.get(i).and_then(|v| v.get(col)).cloned(),
                    RowRef::New(_) => None,
                };
                p.set_cell(row, col, Cell::Null, original.as_ref());
            }
        });
    }

    fn set_now(&mut self, cells: Vec<(RowRef, usize)>, cx: &mut Context<Self>) {
        self.update_pending(cx, |p, _| {
            for (row, col) in cells {
                p.set_cell(row, col, Cell::Now, None);
            }
        });
    }

    fn revert(&mut self, row: RowRef, col: usize, cx: &mut Context<Self>) {
        self.update_pending(cx, |p, _| p.revert_cell(row, col));
    }

    fn delete_rows(&mut self, rows: Vec<RowRef>, cx: &mut Context<Self>) {
        let removes_new = rows.iter().any(|r| matches!(r, RowRef::New(_)));
        self.update_pending(cx, |p, _| p.delete(&rows));
        if removes_new {
            // New rows are gone outright, so positions below them moved up.
            self.grid.clear_selection();
        }
        self.clamp_grid_scroll(cx);
    }

    fn undelete_rows(&mut self, rows: Vec<RowRef>, cx: &mut Context<Self>) {
        self.update_pending(cx, |p, _| p.undelete(&rows));
    }

    fn insert_row(&mut self, anchor: Option<RowRef>, cx: &mut Context<Self>) {
        let mut added = None;
        self.update_pending(cx, |p, r| added = Some(p.insert_below(anchor, r.columns.len())));
        let Some(added) = added else { return };
        // Select the new row and bring it into view.
        let view = self.current_view(cx);
        if let Some(pos) = view.position(added) {
            let size = self.grid_view_size(cx);
            self.grid.clear_selection();
            self.grid.selected_rows.insert(pos);
            self.grid.row_anchor = Some(pos);
            self.grid.ensure_row_visible(pos, view.len(), size);
        }
        cx.notify();
    }

    fn grid_menu_body(&mut self, menu: &OutputMenu, s: Scale, cx: &mut Context<Self>) -> gpui::Div {
        let item = |id: &'static str, label: String, blocked: Option<String>| {
            let enabled = blocked.is_none();
            div()
                .id(id)
                .w_full()
                .px(px(14.))
                .py(px(6.))
                .t(s, 12.0)
                .text_color(if enabled { theme::TEXT } else { theme::TEXT_MUTED })
                .when(enabled, |d| d.cursor_pointer().hover(|st| st.bg(theme::BG_HOVER)))
                .child(label)
                .when_some(blocked, |d, why| d.child(div().t(s, 10.5).text_color(theme::TEXT_MUTED).child(why)))
        };
        let separator = || div().my(px(4.)).h(px(1.)).bg(theme::BORDER_SUBTLE);
        let rows_lock = self.rows_lock(cx);
        let mut body = div();
        match menu.clone() {
            OutputMenu::GridCell { row, col, .. } => {
                let st = self.ctx.store.as_ref().map(|s| s.read(cx));
                let nullable = st.and_then(|st| st.edit_info.as_ref()).is_some_and(|i| i.nullable(col));
                let queued = st.and_then(|st| st.pending.cell(row, col).cloned());
                let deleted = st.is_some_and(|st| st.pending.is_deleted(row));
                let lock = self.cell_lock(row, col, cx);
                let targets = self.cell_targets(row, col, cx);
                let null_block = lock.clone().or_else(|| (!nullable).then(|| "the column is NOT NULL".to_string()));
                let null_label = if targets.len() > 1 { format!("Set {} cells to NULL", targets.len()) } else { "Set to NULL".to_string() };
                let now_targets = targets.clone();
                body = body.child(item("grid-null", null_label, null_block.clone()).when(null_block.is_none(), |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.set_null(targets.clone(), cx)))
                }));
                // Date and time columns can take the database's current time.
                let info = st.and_then(|st| st.edit_info.clone());
                let temporal = info
                    .as_ref()
                    .and_then(|i| i.column(col).map(|c| (i.driver, c)))
                    .is_some_and(|(d, c)| crate::grid_edit::now_sql(d, &c.data_type).is_some());
                if temporal {
                    let is_key = info.as_ref().is_some_and(|i| i.layout.key.contains(&col));
                    let now_block = lock.clone().or_else(|| is_key.then(|| "the row is found by this key column, so it needs a known value".to_string()));
                    let now_label = if now_targets.len() > 1 { format!("Set {} cells to NOW()", now_targets.len()) } else { "Set to NOW()".to_string() };
                    body = body.child(item("grid-now", now_label, now_block.clone()).when(now_block.is_none(), |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.set_now(now_targets.clone(), cx)))
                    }));
                }
                let is_new = matches!(row, RowRef::New(_));
                let changed = match (&queued, is_new) {
                    (Some(Cell::Default), true) | (None, _) => false,
                    (Some(_), _) => true,
                };
                let revert_block = lock.clone().filter(|_| !deleted).or_else(|| (!changed).then(|| "nothing to undo".to_string()));
                let revert_label = if is_new { "Reset to DEFAULT" } else { "Revert Cell" };
                body = body.child(item("grid-revert", revert_label.to_string(), revert_block.clone()).when(revert_block.is_none(), |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.revert(row, col, cx)))
                }));
                body = body.child(separator());
                if deleted {
                    body = body.child(item("grid-undelete-row", "Undelete Row".into(), rows_lock.clone()).when(rows_lock.is_none(), |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.undelete_rows(vec![row], cx)))
                    }));
                } else {
                    body = body.child(item("grid-delete-row", "Delete Row".into(), rows_lock.clone()).when(rows_lock.is_none(), |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.delete_rows(vec![row], cx)))
                    }));
                }
                body = body.child(item("grid-insert-row", "Insert Row Below".into(), rows_lock.clone()).when(rows_lock.is_none(), |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.insert_row(Some(row), cx)))
                }));
            }
            OutputMenu::GridRows { rows, clicked, .. } => {
                let st = self.ctx.store.as_ref().map(|s| s.read(cx));
                let (marked, unmarked): (Vec<RowRef>, Vec<RowRef>) = rows.iter().partition(|r| st.is_some_and(|st| st.pending.is_deleted(**r)));
                let n = |k: usize| if k == 1 { "1 Row".to_string() } else { format!("{k} Rows") };
                if !unmarked.is_empty() {
                    let label = format!("Delete {}", n(unmarked.len()));
                    body = body.child(item("grid-delete-rows", label, rows_lock.clone()).when(rows_lock.is_none(), |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.delete_rows(unmarked.clone(), cx)))
                    }));
                }
                if !marked.is_empty() {
                    let label = format!("Undelete {}", n(marked.len()));
                    body = body.child(item("grid-undelete-rows", label, rows_lock.clone()).when(rows_lock.is_none(), |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| this.undelete_rows(marked.clone(), cx)))
                    }));
                }
                let anchor = Some(clicked);
                body = body.child(separator()).child(item("grid-insert-row", "Insert Row Below".into(), rows_lock.clone()).when(rows_lock.is_none(), |d| {
                    d.on_click(cx.listener(move |this, _, _, cx| this.insert_row(anchor, cx)))
                }));
            }
            OutputMenu::Grid { .. } => {
                body = body.child(item("grid-insert-row", "Insert Row".into(), rows_lock.clone()).when(rows_lock.is_none(), |d| {
                    d.on_click(cx.listener(|this, _, _, cx| this.insert_row(None, cx)))
                }));
            }
            _ => {}
        }
        body
    }

    // ─── Results grid ───────────────────────────────────────────────────────

    fn render_results(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.ctx.scale);
        let tab_id = self.ctx.tab_id.clone();
        if !self.ctx.is_sql {
            return empty_state(s, theme::TEXT_MUTED, "No results.".into());
        }
        let Some(store) = self.ctx.store.clone() else {
            return empty_state(s, theme::TEXT_MUTED, "No results.".into());
        };
        let (result, sort, pending, editable, error_row) = {
            let st = store.read(cx);
            let Some(result) = st.result.clone() else {
                return empty_state(s, theme::TEXT_MUTED, "Run a query to see results here.".into());
            };
            (
                result,
                st.sort(),
                Arc::new(st.pending.clone()),
                st.edit_info.is_some() && !st.is_saving(),
                st.save_error.as_ref().and_then(|(_, r)| *r),
            )
        };
        if !result.error.is_empty() {
            return self.render_error_box(s, result.error.clone(), cx);
        }
        if result.columns.is_empty() {
            return empty_state(s, theme::TEXT_MUTED, format!("Query executed. {} row(s) affected in {}ms.", result.rows_affected, result.duration));
        }
        if self.grid.tab_id != tab_id || self.grid.generation != result.generation {
            self.grid.on_result_changed(&tab_id, result.generation, &result.columns, true, s.0);
        }
        self.grid.font_scale = s.0;
        let loading = store.read(cx).loading;
        self.grid.loading = loading;
        if self.grid.sync_measurements(&result, loading, cx) && !loading {
            cx.notify();
        }
        let view = self.view_for(&result, cx);
        let total = view.len();
        let rnw = grid::row_num_width(total, s.0);
        if let Some(req) = self.grid.take_sort_request() {
            let snapshot = result.clone();
            cx.spawn(async move |this, cx| {
                let idx = cx.background_spawn(async move { grid::compute_sort(&snapshot, req.col, req.dir) }).await;
                this.update(cx, |this, cx| {
                    this.grid.finish_sort(req, idx);
                    cx.notify();
                })
                .ok();
            })
            .detach();
        }
        if self.grid.focus.is_none() {
            self.grid.focus = Some(cx.focus_handle());
        }
        let focus = self.grid.focus.clone().unwrap();

        // Header
        let mut cells = div().flex();
        for (i, name) in result.columns.iter().enumerate() {
            let w = self.grid.col_widths.get(i).copied().unwrap_or(100.0);
            let sorted = sort.filter(|(c, _)| *c == i).map(|(_, d)| d);
            let name2 = name.clone();
            cells = cells.child(
                div()
                    .id(SharedString::from(format!("hc-{i}")))
                    .relative()
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .px(px(10.))
                    .py(px(6.))
                    .w(px(w))
                    .min_w(px(w))
                    .flex_shrink_0()
                    .border_r_1()
                    .border_color(theme::BORDER)
                    .whitespace_nowrap()
                    .overflow_hidden()
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::BG_HOVER).text_color(theme::TEXT))
                    .on_hover(cx.listener(move |this, hovered: &bool, window, cx| {
                        if *hovered {
                            let pos = window.mouse_position();
                            this.grid.tooltip = Some((i, pos));
                        } else if this.grid.tooltip.is_some_and(|(c, _)| c == i) {
                            this.grid.tooltip = None;
                        }
                        cx.notify();
                    }))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if std::mem::take(&mut this.grid.did_resize) {
                            return;
                        }
                        this.toggle_sort(i, cx);
                    }))
                    .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu = Some(OutputMenu::Grid { pos: e.position });
                        cx.notify();
                    }))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().child(name2))
                    .when_some(sorted, |d, dir| {
                        d.child(div().flex_shrink_0().opacity(0.7).child(icon(if dir == SortDirection::Asc { Icon::SortAsc } else { Icon::SortDesc }, s.fs(11.0), theme::TEXT_DIM)))
                    })
                    .child(
                        div()
                            .id(SharedString::from(format!("rh-{i}")))
                            .absolute()
                            .right_0()
                            .top_0()
                            .bottom_0()
                            .w(px(6.))
                            .cursor(CursorStyle::ResizeLeftRight)
                            .hover(|st| st.bg(theme::with_alpha(theme::ACCENT, 0.5)))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                                cx.stop_propagation();
                                if e.click_count >= 2 {
                                    if let Some(r) = this.result(cx) {
                                        let cols = r.columns.clone();
                                        this.grid.measure_all(&r, cx);
                                        this.grid.auto_fit(i, &cols, cx);
                                    }
                                    cx.notify();
                                    return;
                                }
                                this.grid.did_resize = false;
                                let w = this.grid.col_widths.get(i).copied().unwrap_or(100.0);
                                this.grid.resizing = Some((i, e.position.x, w));
                            }))
                            .on_click(cx.listener(|this, _, _, cx| {
                                // A click ends here rather than at the root, so
                                // the resize has to be finished here too —
                                // otherwise the column stays glued to the
                                // pointer after the button comes up. Stopping
                                // propagation is what keeps the header from
                                // treating the release as a sort.
                                this.grid.resizing = None;
                                this.grid.did_resize = false;
                                cx.stop_propagation();
                            })),
                    ),
            );
        }
        let scroll_x = self.grid.scroll_x;
        let header = div()
            .flex()
            .bg(theme::BG_PANEL)
            .border_b_2()
            .border_color(theme::BORDER)
            .flex_shrink_0()
            .t(s, 12.0)
            .font_weight(FontWeight::SEMIBOLD)
            .text_color(theme::TEXT_MUTED)
            .child(
                div()
                    .w(px(rnw))
                    .min_w(px(rnw))
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .px(px(8.))
                    .py(px(6.))
                    .t(s, 11.0)
                    .text_color(theme::TEXT_MUTED)
                    .border_r_1()
                    .border_color(theme::BORDER)
                    .child("#"),
            )
            .child(div().flex_1().overflow_hidden().child(div().relative().left(px(-scroll_x)).child(cells)));

        let panel = cx.entity().downgrade();
        let body = GridBody {
            result: result.clone(),
            view: view.clone(),
            col_widths: self.grid.col_widths.clone(),
            scale: s.0,
            scroll: (self.grid.scroll_x, self.grid.scroll_y),
            sel: self.grid.sel,
            selected_rows: self.grid.selected_rows.clone(),
            hovered_row: self.grid.hovered_row,
            active_bar: self.grid.scroll_drag.map(|d| d.bar).or(self.grid.hovered_bar),
            pending,
            editing: self.grid.edit.as_ref().map(|e| (e.row_ref, e.col)),
            error_row,
            on_bounds: Box::new(move |b, cx| {
                panel.update(cx, |p, _| p.grid.body_bounds = Some(b)).ok();
            }),
        };
        let body_el = div()
            .id("grid-body")
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .cursor(CursorStyle::Crosshair)
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                let d = e.delta.pixel_delta(px(20.));
                this.grid.scroll_x -= f32::from(d.x);
                this.grid.scroll_y -= f64::from(f32::from(d.y));
                this.clamp_grid_scroll(cx);
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some(b) = this.grid.body_bounds else { return };
                let total = this.total_rows(cx);
                let local = point(e.position.x - b.left(), e.position.y - b.top());
                let rh = grid::row_height(this.grid.font_scale);
                let row = ((f64::from(f32::from(local.y)) + this.grid.scroll_y) / f64::from(rh)).floor();
                let next = (row >= 0.0 && (row as usize) < total && b.contains(&e.position)).then_some(row as usize);
                if next != this.grid.hovered_row {
                    this.grid.hovered_row = next;
                    cx.notify();
                }
            }))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if !*hovered && this.grid.hovered_row.is_some() {
                    this.grid.hovered_row = None;
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                this.on_grid_mouse_down(e, editable, window, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(|this, e: &MouseDownEvent, window, cx| {
                this.on_grid_right_click(e, window, cx);
            }))
            .child(body);

        let mut wrap = div()
            .id("grid-wrap")
            .key_context("ResultsGrid")
            .track_focus(&focus)
            .flex()
            .flex_col()
            .size_full()
            .min_h(px(0.))
            .overflow_hidden()
            .on_action(cx.listener(|this, _: &GridCopy, _, cx| this.grid_copy(cx)))
            .on_action(cx.listener(|this, _: &GridSelectAll, _, cx| this.grid_select_all(cx)))
            .on_action(cx.listener(|this, _: &GridUp, _, cx| this.grid_move(-1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridDown, _, cx| this.grid_move(1, 0, false, cx)))
            .on_action(cx.listener(|this, _: &GridLeft, _, cx| this.grid_move(0, -1, false, cx)))
            .on_action(cx.listener(|this, _: &GridRight, _, cx| this.grid_move(0, 1, false, cx)))
            .on_action(cx.listener(|this, _: &GridPageUp, _, cx| {
                let p = this.grid_page(cx);
                this.grid_move(-p, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &GridPageDown, _, cx| {
                let p = this.grid_page(cx);
                this.grid_move(p, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &GridExtendUp, _, cx| this.grid_move(-1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendDown, _, cx| this.grid_move(1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendLeft, _, cx| this.grid_move(0, -1, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendRight, _, cx| this.grid_move(0, 1, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendPageUp, _, cx| {
                let p = this.grid_page(cx);
                this.grid_move(-p, 0, true, cx)
            }))
            .on_action(cx.listener(|this, _: &GridExtendPageDown, _, cx| {
                let p = this.grid_page(cx);
                this.grid_move(p, 0, true, cx)
            }))
            .on_action(cx.listener(|this, _: &GridFirstRow, _, cx| this.grid_jump(Some(Jump::First), None, false, cx)))
            .on_action(cx.listener(|this, _: &GridLastRow, _, cx| this.grid_jump(Some(Jump::Last), None, false, cx)))
            .on_action(cx.listener(|this, _: &GridFirstCol, _, cx| this.grid_jump(None, Some(Jump::First), false, cx)))
            .on_action(cx.listener(|this, _: &GridLastCol, _, cx| this.grid_jump(None, Some(Jump::Last), false, cx)))
            .on_action(cx.listener(|this, _: &GridExtendFirstRow, _, cx| this.grid_jump(Some(Jump::First), None, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendLastRow, _, cx| this.grid_jump(Some(Jump::Last), None, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendFirstCol, _, cx| this.grid_jump(None, Some(Jump::First), true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendLastCol, _, cx| this.grid_jump(None, Some(Jump::Last), true, cx)))
            .on_action(cx.listener(|this, _: &GridEscape, _, cx| {
                this.grid.clear_selection();
                cx.notify();
            }))
            .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                if this.grid.sel.is_some() || !this.grid.selected_rows.is_empty() {
                    this.grid.clear_selection();
                    cx.notify();
                }
            }))
            .child(header)
            .child(body_el);

        if let Some((col, pos)) = self.grid.tooltip {
            let ty = result.column_types.get(col).cloned().unwrap_or_default();
            let max_len = self.grid.max_len.get(col).copied().unwrap_or(0);
            let tip_pos = self.header_cell_bottom_left(col, total).unwrap_or(pos);
            wrap = wrap.child(deferred(
                gpui::anchored().position(tip_pos).child(
                    div()
                        .bg(theme::BG_PANEL)
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(4.))
                        .px(px(10.))
                        .py(px(6.))
                        .flex()
                        .flex_col()
                        .gap(px(2.))
                        .shadow(vec![shadow(0.0, 4.0, 12.0, 0.0, theme::rgba8(0, 0, 0, 0.4))])
                        .when(!ty.is_empty(), |d| {
                            d.child(crate::ui::widgets::spaced_text::spaced_text(
                                ty.to_uppercase(),
                                gpui::Font { weight: FontWeight::SEMIBOLD, ..gpui::font(theme::ui_font()) },
                                11.0 * s.0,
                                s.lh_f(11.0),
                                theme::ACCENT,
                                0.04 * 11.0 * s.0,
                            ))
                        })
                        .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).child(format!("max length: {max_len}"))),
                ),
            ).with_priority(2));
        }
        if let Some(edit) = &self.grid.edit {
            let rh = grid::row_height(s.0);
            wrap = wrap.child(deferred(
                gpui::anchored().position(edit.origin).child(
                    div()
                        .id("cell-editor")
                        // The editor floats over the grid; without this a click
                        // in it also lands on the cell underneath, which takes
                        // focus (committing and closing the editor) or, on a
                        // double click, reopens it with the selection lost.
                        .occlude()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .on_mouse_down(MouseButton::Right, |_, _, cx| cx.stop_propagation())
                        .w(px(edit.width))
                        .h(px(rh))
                        .child(edit.input.clone()),
                ),
            ).with_priority(3));
        }
        wrap.into_any_element()
    }

    fn header_cell_bottom_left(&self, col: usize, total: usize) -> Option<Point<Pixels>> {
        let b = self.grid.body_bounds?;
        let rnw = grid::row_num_width(total, self.grid.font_scale);
        let left: f32 = self.grid.col_widths[..col].iter().sum::<f32>() + rnw - self.grid.scroll_x;
        Some(point(b.left() + px(left), b.top() + px(4.)))
    }

    fn clamp_grid_scroll(&mut self, cx: &App) {
        let total = self.total_rows(cx);
        let (w, h) = self.grid_view_size(cx);
        self.grid.clamp_scroll(total, w, h);
    }

    fn grid_view_size(&self, cx: &App) -> (f32, f32) {
        if self.grid.body_bounds.is_none() {
            return (800.0, 300.0);
        }
        let geom = self.grid.geom(self.total_rows(cx));
        (geom.view_w, geom.view_h)
    }

    fn grid_page(&self, cx: &App) -> isize {
        let (_, h) = self.grid_view_size(cx);
        (h / grid::row_height(self.grid.font_scale)).floor().max(1.0) as isize
    }

    fn toggle_sort(&mut self, col: usize, cx: &mut Context<Self>) {
        if let Some(store) = self.ctx.store.clone() {
            store.update(cx, |st, cx| {
                let dir = if st.sort_col == Some(col) {
                    if st.sort_dir == SortDirection::Asc { SortDirection::Desc } else { SortDirection::Asc }
                } else {
                    SortDirection::Asc
                };
                st.sort_col = Some(col);
                st.sort_dir = dir;
                cx.notify();
            });
        }
        cx.notify();
    }

    fn grid_move(&mut self, dr: isize, dc: isize, extend: bool, cx: &mut Context<Self>) {
        let Some(r) = self.result(cx) else { return };
        let view = self.grid_view_size(cx);
        let total = self.total_rows(cx);
        self.grid.move_selection(dr, dc, extend, total, r.columns.len(), view);
        cx.notify();
    }

    /// cmd/ctrl with an arrow, and Home/End: jump to the first or last row or
    /// column. With `extend` the anchor stays put, so shift-cmd-down selects
    /// everything from the cursor down to the last row.
    fn grid_jump(&mut self, row: Option<Jump>, col: Option<Jump>, extend: bool, cx: &mut Context<Self>) {
        let Some(r) = self.result(cx) else { return };
        let (total, cols) = (self.total_rows(cx), r.columns.len());
        if total == 0 || cols == 0 {
            return;
        }
        let view = self.grid_view_size(cx);
        // Whole rows are selected (the row-number gutter), so keep selecting
        // whole rows rather than switching to a cell range.
        if self.grid.sel.is_none() && !self.grid.selected_rows.is_empty() {
            if let Some(jump) = row {
                let target = match jump {
                    Jump::First => 0,
                    Jump::Last => total - 1,
                };
                let g = &mut self.grid;
                let anchor = g.row_anchor.or(g.last_selected.map(|s| s.0)).unwrap_or(target);
                if extend {
                    for i in anchor.min(target)..=anchor.max(target) {
                        g.selected_rows.insert(i);
                    }
                } else {
                    g.selected_rows.clear();
                    g.selected_rows.insert(target);
                    g.row_anchor = Some(target);
                }
                let col = g.last_selected.map(|s| s.1).unwrap_or(0);
                g.last_selected = Some((target, col));
                g.ensure_row_visible(target, total, view);
                cx.notify();
                return;
            }
        }
        self.grid.jump_selection(row, col, extend, total, cols, view);
        cx.notify();
    }

    fn grid_copy(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.result(cx) else { return };
        let view = self.view_for(&r, cx);
        let Some(store) = self.ctx.store.clone() else { return };
        let pending = store.read(cx).pending.clone();
        if let Some(text) = self.grid.copy_text(&r, &view, &pending) {
            self.copy_to_clipboard(text, cx);
        }
    }

    fn grid_select_all(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.result(cx) else { return };
        let total = self.total_rows(cx);
        if (self.grid.sel.is_some() || !self.grid.selected_rows.is_empty()) && total > 0 && !r.columns.is_empty() {
            let cols = r.columns.len();
            self.grid.sel = Some(CellSel { r0: 0, c0: 0, r1: total - 1, c1: cols - 1 });
            self.grid.selected_rows.clear();
            self.grid.row_anchor = Some(0);
            self.grid.last_selected = Some((total - 1, cols - 1));
            cx.notify();
        }
    }

    fn on_grid_mouse_down(&mut self, e: &MouseDownEvent, editable: bool, window: &mut Window, cx: &mut Context<Self>) {
        let Some(b) = self.grid.body_bounds else { return };
        let Some(r) = self.result(cx) else { return };
        if let Some(f) = &self.grid.focus {
            window.focus(f);
        }
        let total = self.total_rows(cx);
        let local = point(e.position.x - b.left(), e.position.y - b.top());
        let geom = self.grid.geom(total);
        if let Some(bar) = geom.bar_at(local) {
            self.on_scrollbar_press(bar, local, total, cx);
            return;
        }
        if f32::from(local.x) >= geom.view_w || f32::from(local.y) >= geom.view_h {
            // The corner square between the two scrollbars.
            return;
        }
        if e.click_count >= 2 {
            self.on_grid_double_click(local, editable, &r, window, cx);
            return;
        }
        if let Some(row) = self.grid.row_at(local, total) {
            let toggle = e.modifiers.platform || e.modifiers.control;
            let range = e.modifiers.shift;
            let g = &mut self.grid;
            if range {
                let anchor = g.row_anchor.or(g.last_selected.map(|x| x.0)).unwrap_or(row);
                if !toggle {
                    g.selected_rows.clear();
                }
                for i in anchor.min(row)..=anchor.max(row) {
                    g.selected_rows.insert(i);
                }
                g.row_anchor = Some(anchor);
            } else if toggle {
                if !g.selected_rows.remove(&row) {
                    g.selected_rows.insert(row);
                }
                g.row_anchor = Some(row);
            } else {
                g.selected_rows.clear();
                g.selected_rows.insert(row);
                g.row_anchor = Some(row);
            }
            g.sel = None;
            g.last_selected = Some((row, r.columns.len().saturating_sub(1)));
            cx.notify();
            return;
        }
        let Some(hit) = self.grid.cell_at(local, total) else {
            self.grid.clear_selection();
            cx.notify();
            return;
        };
        // The eye button on a value too long to show opens it in full.
        if self.grid.eye_at(hit.0, hit.1, total).contains(&local) {
            if let Some(text) = self.cell_text(&r, hit.0, hit.1, cx) {
                if js::char_len(&text) > grid::EXPAND_CHARS {
                    let column = r.columns.get(hit.1).cloned().unwrap_or_default();
                    let popup = grid::CellPopup { column, row: hit.0 + 1, text };
                    self.with_ws(cx, |ws, cx| {
                        ws.cell_popup = Some(popup);
                        cx.notify();
                    });
                    cx.notify();
                    return;
                }
            }
        }
        let anchor = if e.modifiers.shift { self.grid.last_selected.unwrap_or(hit) } else { hit };
        self.grid.selecting = true;
        self.grid.sel_anchor = Some(anchor);
        self.grid.sel = Some(CellSel { r0: anchor.0, c0: anchor.1, r1: hit.0, c1: hit.1 });
        self.grid.selected_rows.clear();
        self.grid.row_anchor = None;
        self.grid.last_selected = Some(hit);
        cx.notify();
    }

    /// The text a cell shows, following the current order and any queued change.
    fn cell_text(&mut self, r: &QueryResult, row: usize, col: usize, cx: &App) -> Option<String> {
        let view = self.view_for(r, cx);
        let row_ref = view.get(row)?;
        let store = self.ctx.store.clone()?;
        let st = store.read(cx);
        match st.pending.cell(row_ref, col) {
            Some(Cell::Value(t)) => return Some(t.clone()),
            Some(_) => return None,
            None => {}
        }
        let RowRef::Data(i) = row_ref else { return None };
        match r.rows.get(i)?.get(col)? {
            serde_json::Value::Null => None,
            v => Some(js::value_to_string(v)),
        }
    }

    /// Why a cell can't be changed right now; `None` when it can.
    fn cell_lock(&self, row: RowRef, col: usize, cx: &App) -> Option<String> {
        let Some(store) = self.ctx.store.as_ref() else { return Some("there is no result".into()) };
        let st = store.read(cx);
        if st.is_saving() {
            return Some("a save is in progress".into());
        }
        let Some(info) = &st.edit_info else {
            return Some(st.edit_blocked.clone().map(|r| format!("read-only: {r}")).unwrap_or_else(|| "this result can't be edited".into()));
        };
        if st.pending.is_deleted(row) {
            return Some("the row is marked for deletion".into());
        }
        if let Some(reason) = info.readonly_reason(col) {
            return Some(reason.to_string());
        }
        if let RowRef::Data(i) = row {
            if let Some(serde_json::Value::String(v)) = st.result.as_ref().and_then(|r| r.rows.get(i)).and_then(|r| r.get(col)) {
                if v.starts_with("<unsupported") {
                    return Some("this value can't be shown as text".into());
                }
            }
        }
        None
    }

    /// Whether rows can be inserted or deleted right now; the reason if not.
    fn rows_lock(&self, cx: &App) -> Option<String> {
        let store = self.ctx.store.as_ref()?;
        let st = store.read(cx);
        if st.is_saving() {
            return Some("a save is in progress".into());
        }
        if st.edit_info.is_none() {
            return Some(st.edit_blocked.clone().map(|r| format!("read-only: {r}")).unwrap_or_else(|| "this result can't be edited".into()));
        }
        None
    }

    /// A press on a scrollbar: grab the thumb, or page towards a click on the
    /// track, the way a native scrollbar does.
    fn on_scrollbar_press(&mut self, bar: Bar, local: Point<Pixels>, total: usize, cx: &mut Context<Self>) {
        let geom = self.grid.geom(total);
        let pos = geom.pos_on(bar, local);
        let (start, len, scroll, page) = match bar {
            Bar::Vertical => {
                let (s, l) = geom.v_thumb(self.grid.scroll_y as f32);
                (s, l, self.grid.scroll_y, f64::from(geom.view_h))
            }
            Bar::Horizontal => {
                let (s, l) = geom.h_thumb(self.grid.scroll_x);
                (s, l, f64::from(self.grid.scroll_x), f64::from(geom.view_w))
            }
        };
        if pos >= start && pos < start + len {
            self.grid.scroll_drag = Some(ScrollDrag { bar, grab: pos - start });
        } else {
            // Track click: one page towards the pointer.
            let delta = if pos < start { -page } else { page };
            match bar {
                Bar::Vertical => self.grid.scroll_y = scroll + delta,
                Bar::Horizontal => self.grid.scroll_x = (scroll + delta) as f32,
            }
            self.clamp_grid_scroll(cx);
        }
        self.grid.hovered_bar = Some(bar);
        cx.notify();
    }

    fn on_grid_double_click(&mut self, local: Point<Pixels>, editable: bool, r: &QueryResult, window: &mut Window, cx: &mut Context<Self>) {
        let total = self.total_rows(cx);
        let Some(hit) = self.grid.cell_at(local, total) else { return };
        let view = self.view_for(r, cx);
        let Some(row_ref) = view.get(hit.0) else { return };
        let loaded = match row_ref {
            RowRef::Data(i) => r.rows.get(i).and_then(|row| row.get(hit.1)).cloned(),
            RowRef::New(_) => None,
        };
        if editable && self.cell_lock(row_ref, hit.1, cx).is_none() {
            let queued = self.ctx.store.as_ref().and_then(|s| s.read(cx).pending.cell(row_ref, hit.1).cloned());
            let current = match queued {
                Some(Cell::Value(t)) => t,
                Some(_) => String::new(),
                None => loaded.as_ref().map(edit_text).unwrap_or_default(),
            };
            let s = self.grid.font_scale;
            let rh = grid::row_height(s);
            let rnw = grid::row_num_width(total, s);
            let b = self.grid.body_bounds.unwrap();
            let x: f32 = rnw - self.grid.scroll_x + self.grid.col_widths[..hit.1].iter().sum::<f32>();
            let y: f32 = (hit.0 as f64 * f64::from(rh) - self.grid.scroll_y) as f32;
            let w = self.grid.col_widths.get(hit.1).copied().unwrap_or(100.0);
            let look = InputLook {
                font_size: 12.0 * s,
                line_height: crate::ui::metrics::line_height_normal(12.0 * s),
                height: rh,
                pad_x: (8.0, 8.0),
                radius: 0.0,
                border_width: 2.0,
                bg: theme::BG_PANEL,
                focus_bg: Some(theme::rgba8(255, 200, 50, 0.06)),
                border: theme::rgba8(255, 200, 50, 0.9),
                focus_border: theme::AMBER,
                ..InputLook::dialog(s)
            };
            // The value opens fully selected, so Cmd+C copies it straight
            // away and typing replaces it. After a stray click collapses the
            // selection, a double click or Cmd+A selects it all again.
            let initial = current.clone();
            let input = cx.new(|cx| {
                let mut t = TextInput::new(cx, look);
                t.double_click_selects_all = true;
                t.blur_on_click_outside = true;
                t.set_text(current, cx);
                t.select_all_text(cx);
                t
            });
            cx.subscribe_in(&input, window, |this, _i, ev: &InputEvent, window, cx| match ev {
                // Enter and Escape hand the keyboard back to the grid, so the
                // arrows, Cmd+C and a double click carry on from the cell.
                // A click elsewhere (Blur) leaves focus wherever it went.
                InputEvent::Submit => {
                    this.commit_cell_edit(cx);
                    if let Some(f) = &this.grid.focus {
                        window.focus(f);
                    }
                }
                InputEvent::Blur => this.commit_cell_edit(cx),
                InputEvent::Cancel => {
                    this.grid.edit = None;
                    if let Some(f) = &this.grid.focus {
                        window.focus(f);
                    }
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            input.update(cx, |i, _| i.focus(window));
            // This runs on mouse-down, which then bubbles up to the grid
            // container. gpui focuses any `track_focus` element it bubbles
            // through unless the default is prevented, so without this the
            // grid takes focus straight back: the editor looks active but the
            // keyboard (Escape, Cmd+C, Cmd+A, typing) still goes to the grid.
            window.prevent_default();
            self.grid.edit = Some(EditOverlay {
                row: hit.0,
                col: hit.1,
                row_ref,
                initial,
                input,
                origin: point(b.left() + px(x), b.top() + px(y)),
                width: w,
            });
            self.grid.sel = Some(CellSel { r0: hit.0, c0: hit.1, r1: hit.0, c1: hit.1 });
            self.grid.last_selected = Some(hit);
            cx.notify();
            return;
        }
        let ty = r.column_types.get(hit.1).map(String::as_str);
        let text = match (row_ref, self.ctx.store.as_ref().and_then(|s| s.read(cx).pending.cell(row_ref, hit.1).cloned())) {
            (_, Some(Cell::Value(t))) => t,
            (_, Some(_)) | (RowRef::New(_), None) => "NULL".to_string(),
            (RowRef::Data(_), None) => crate::ui::textfmt::format_value_for_clipboard(loaded.as_ref().unwrap_or(&serde_json::Value::Null), ty, "NULL"),
        };
        self.copy_to_clipboard(text, cx);
        self.grid.sel = Some(CellSel { r0: hit.0, c0: hit.1, r1: hit.0, c1: hit.1 });
        self.grid.last_selected = Some(hit);
        cx.notify();
    }

    fn commit_cell_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.grid.edit.take() else { return };
        let value = edit.input.read(cx).text().to_string();
        // Closing the editor without changing its text changes nothing.
        if value == edit.initial || self.cell_lock(edit.row_ref, edit.col, cx).is_some() {
            cx.notify();
            return;
        }
        let Some(r) = self.result(cx) else { return };
        let original = match edit.row_ref {
            RowRef::Data(i) => r.rows.get(i).and_then(|row| row.get(edit.col)).cloned(),
            RowRef::New(_) => None,
        };
        if let Some(store) = self.ctx.store.clone() {
            store.update(cx, |s, cx| {
                s.pending.set_cell(edit.row_ref, edit.col, Cell::Value(value), original.as_ref());
                s.save_error = None;
                cx.notify();
            });
        }
        cx.notify();
    }

    /// The pointer moved anywhere in the window: follow a scrollbar drag, a
    /// column resize or a cell-range selection that is in progress.
    pub fn on_drag_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if !self.ctx.is_sql {
            return;
        }
        if let Some(drag) = self.grid.scroll_drag {
            let Some(b) = self.grid.body_bounds else { return };
            let total = self.total_rows(cx);
            let geom = self.grid.geom(total);
            let local = point(e.position.x - b.left(), e.position.y - b.top());
            let pos = geom.pos_on(drag.bar, local) - drag.grab;
            match drag.bar {
                Bar::Vertical => self.grid.scroll_y = f64::from(geom.scroll_for_v_thumb(pos)),
                Bar::Horizontal => self.grid.scroll_x = geom.scroll_for_h_thumb(pos),
            }
            self.clamp_grid_scroll(cx);
            cx.notify();
            return;
        }
        // Hover state for the thumb (`::-webkit-scrollbar-thumb:hover`).
        let hovered = self.grid.body_bounds.and_then(|b| {
            let total = self.total_rows(cx);
            self.grid.geom(total).bar_at(point(e.position.x - b.left(), e.position.y - b.top()))
        });
        if hovered != self.grid.hovered_bar {
            self.grid.hovered_bar = hovered;
            cx.notify();
        }
        if let Some((idx, start_x, start_w)) = self.grid.resizing {
            let delta: f32 = (e.position.x - start_x).into();
            if delta.abs() > 2.0 {
                self.grid.did_resize = true;
            }
            let min = 50.0 * self.grid.font_scale;
            if let Some(w) = self.grid.col_widths.get_mut(idx) {
                // Live resize: the column follows the pointer and stays where
                // the button came up.
                *w = (start_w + delta).max(min);
            }
            if let Some(u) = self.grid.user_sized.get_mut(idx) {
                *u = true;
            }
            cx.notify();
            return;
        }
        if !self.grid.selecting || !e.dragging() {
            return;
        }
        let Some(b) = self.grid.body_bounds else { return };
        let total = self.total_rows(cx);
        // Edge auto-scroll (EDGE_ZONE 50px, up to 15px per event).
        let local = point(e.position.x - b.left(), e.position.y - b.top());
        let speed = |pos: f32, size: f32| {
            if pos < 50.0 {
                -(1.0 - pos / 50.0).min(1.0) * 15.0
            } else if pos > size - 50.0 {
                (1.0 - (size - pos) / 50.0).min(1.0) * 15.0
            } else {
                0.0
            }
        };
        let vx = speed(local.x.into(), b.size.width.into());
        let vy = speed(local.y.into(), b.size.height.into());
        if vx != 0.0 || vy != 0.0 {
            self.grid.scroll_x += vx;
            self.grid.scroll_y += f64::from(vy);
            self.clamp_grid_scroll(cx);
        }
        if let (Some(hit), Some(anchor)) = (self.grid.cell_at(local, total), self.grid.sel_anchor) {
            self.grid.sel = Some(CellSel { r0: anchor.0, c0: anchor.1, r1: hit.0, c1: hit.1 });
        }
        cx.notify();
    }

    pub fn on_drag_end(&mut self, _e: &MouseUpEvent, cx: &mut Context<Self>) {
        if self.grid.scroll_drag.take().is_some() {
            cx.notify();
        }
        if self.grid.resizing.take().is_some() {
            self.grid.did_resize = false;
            cx.notify();
        }
        if self.grid.selecting {
            if let Some(s) = self.grid.sel {
                self.grid.last_selected = Some((s.r1, s.c1));
            }
            self.grid.selecting = false;
            self.grid.sel_anchor = None;
            cx.notify();
        }
    }
}

impl OutputPanel {
    /// Save / Clear / Open as SQL while changes are queued; progress and Stop
    /// while saving; why the result is read-only when it is.
    fn render_edit_bar(&mut self, s: Scale, cx: &mut Context<Self>) -> Option<AnyElement> {
        let store = self.ctx.store.clone()?;
        let st = store.read(cx);
        st.result.as_ref().filter(|r| r.error.is_empty() && !r.columns.is_empty())?;
        let amber = |id: &'static str, label: String, enabled: bool| {
            div()
                .id(id)
                .px(px(12.))
                .py(px(3.))
                .rounded(px(4.))
                .border_1()
                .t(s, 11.0)
                .font_weight(FontWeight::SEMIBOLD)
                .whitespace_nowrap()
                .when(enabled, |d| {
                    d.bg(theme::rgba8(255, 200, 50, 0.15))
                        .border_color(theme::rgba8(255, 200, 50, 0.6))
                        .text_color(theme::AMBER)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::rgba8(255, 200, 50, 0.25)).border_color(theme::rgba8(255, 200, 50, 0.9)))
                })
                .when(!enabled, |d| d.border_color(theme::BORDER).text_color(theme::TEXT_MUTED))
                .child(label)
        };
        let plain = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .px(px(10.))
                .py(px(3.))
                .rounded(px(4.))
                .border_1()
                .border_color(theme::BORDER)
                .t(s, 11.0)
                .text_color(theme::TEXT_DIM)
                .whitespace_nowrap()
                .cursor_pointer()
                .hover(|st| st.bg(theme::BG_HOVER).text_color(theme::TEXT))
                .child(label)
        };
        let mut bar = div().flex().items_center().gap(px(6.)).mr(px(8.));
        if let Some(ctl) = st.saving.clone() {
            let done = ctl.done.load(std::sync::atomic::Ordering::Relaxed);
            let total = ctl.total.load(std::sync::atomic::Ordering::Relaxed);
            let committing = ctl.committing();
            let text = if committing {
                "Committing…".to_string()
            } else if total > 1 {
                format!("Saving {} / {}…", crate::ui::workspace::group_digits(done.min(total)), crate::ui::workspace::group_digits(total))
            } else {
                "Saving…".to_string()
            };
            bar = bar.child(div().t(s, 11.0).text_color(theme::TEXT_DIM).whitespace_nowrap().child(text));
            let stop = div()
                .id("stop-save")
                .px(px(10.))
                .py(px(3.))
                .rounded(px(4.))
                .border_1()
                .t(s, 11.0)
                .whitespace_nowrap()
                .when(!committing, |d| {
                    d.border_color(theme::ERROR).text_color(theme::ERROR).cursor_pointer().hover(|st| st.bg(theme::rgba8(255, 80, 80, 0.12))).on_click(
                        cx.listener(|this, _, _, cx| {
                            let tab = this.ctx.tab_id.clone();
                            this.with_ws(cx, |ws, cx| ws.stop_save(&tab, cx));
                        }),
                    )
                })
                .when(committing, |d| d.border_color(theme::BORDER).text_color(theme::TEXT_MUTED))
                .child("Stop");
            return Some(bar.child(stop).into_any_element());
        }
        let count = st.pending.count();
        if count > 0 {
            let loading = st.loading;
            let tab = self.ctx.tab_id.clone();
            bar = bar
                .child(plain("clear-edits", "Clear").on_click(cx.listener(move |this, _, _, cx| {
                    let tab = tab.clone();
                    this.with_ws(cx, |ws, cx| ws.guard_discard(crate::ui::grid_save::DiscardAction::Clear(tab), cx));
                })))
                .child(plain("edits-as-sql", "Open as SQL").on_click(cx.listener(|this, _, _, cx| {
                    this.with_ws(cx, |ws, cx| ws.open_edits_as_sql(cx));
                })));
            let label = if loading { "Loading rows…".to_string() } else { format!("Save Changes ({count})") };
            let save = amber("save-edits", label, !loading);
            let save = if loading {
                save
            } else {
                save.on_click(cx.listener(|this, _, _, cx| {
                    if this.grid.edit.is_some() {
                        this.commit_cell_edit(cx);
                    }
                    let view = this.current_view(cx);
                    this.with_ws(cx, |ws, cx| ws.save_edits(view, cx));
                }))
            };
            return Some(bar.child(save).into_any_element());
        }
        if let Some(reason) = &st.edit_blocked {
            let text = format!("Read-only: {reason}");
            return Some(bar.child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().overflow_hidden().text_ellipsis().max_w(px(420.)).child(text)).into_any_element());
        }
        None
    }
}

impl Render for OutputPanel {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = Scale(self.ctx.scale);
        let output_tab = self.ctx.output_tab;
        let mut tabs = div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_PANEL)
            .flex_shrink_0();
        for (key, label) in OutputTab::ALL {
            let active = output_tab == key;
            tabs = tabs.child(
                div()
                    .id(SharedString::from(format!("out-{label}")))
                    .px(px(16.))
                    .pt(px(6.))
                    .pb(px(6.))
                    .border_b_2()
                    .border_color(if active { theme::ACCENT } else { theme::TRANSPARENT })
                    .text_color(if active { theme::TEXT } else { theme::TEXT_MUTED })
                    .t(s, 12.0)
                    .cursor_pointer()
                    .hover(|st| st.text_color(theme::TEXT))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.with_ws(cx, |ws, cx| {
                            ws.output_tab = key;
                            cx.notify();
                        });
                    }))
                    .child(label),
            );
        }
        if output_tab == OutputTab::Results {
            if let Some(bar) = self.render_edit_bar(s, cx) {
                tabs = tabs.child(div().flex_1()).child(bar);
            }
        }
        let content: AnyElement = match output_tab {
            OutputTab::Results => self.render_results(cx),
            OutputTab::Messages => self.render_messages(s, cx),
            OutputTab::History => self.render_history(s, cx),
            OutputTab::Saved => self.render_saved(s, cx),
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .overflow_hidden()
            .child(tabs)
            .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(content))
            .children(self.render_menu(cx))
    }
}

fn msg_box(s: Scale, color: Rgba, bg: Option<Rgba>, text: String) -> AnyElement {
    div()
        .t(s, 12.0)
        .px(px(10.))
        .py(px(6.))
        .rounded(px(4.))
        .text_color(color)
        .when_some(bg, |d, c| d.bg(c))
        .child(text)
        .into_any_element()
}

fn empty_state(s: Scale, color: Rgba, text: String) -> AnyElement {
    div()
        .size_full()
        .flex()
        .items_center()
        .justify_center()
        .t(s, 13.0)
        .text_color(color)
        .child(text)
        .into_any_element()
}

/// `new Date(createdAt).toLocaleString()` for SQLite `YYYY-MM-DD HH:MM:SS`
/// (en-GB style as rendered on the user's machine).
pub fn js_locale_string(created_at: &str) -> String {
    use chrono::{Local, NaiveDateTime, TimeZone};
    let parsed = NaiveDateTime::parse_from_str(created_at, "%Y-%m-%d %H:%M:%S")
        .or_else(|_| NaiveDateTime::parse_from_str(created_at, "%Y-%m-%dT%H:%M:%S"));
    match parsed {
        Ok(dt) => match Local.from_local_datetime(&dt).single() {
            Some(local) => local.format("%d/%m/%Y, %H:%M:%S").to_string(),
            None => "Invalid Date".into(),
        },
        Err(_) => "Invalid Date".into(),
    }
}
