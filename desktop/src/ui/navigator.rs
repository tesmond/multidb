//! Connection navigator (`Navigator.svelte`): header menus, table filter,
//! server groups, connection tree, drag and drop, context menus.

use crate::ui::dialogs::{self, Btn};
use crate::ui::model::format_bytes;
use crate::ui::nav_index::NONE;
use crate::ui::nav_rows::{self, Info, Leaf, Row, RowKind};
use crate::ui::sql_text;
use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::scroll;
use crate::ui::widgets::spaced_text::spaced_text;
use crate::ui::widgets::{overlay, separator, shadow, Scale, TextExt};
use crate::ui::workspace::{DragKind, NavDrag, NavMenu, Workspace};
use gpui::{
    deferred, div, point, prelude::*, px, AnyElement, App, Context, CursorStyle, Div, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, SharedString, Stateful, Window,
};

const CHEVRON_OPEN: &str = "▾";
const CHEVRON_CLOSED: &str = "▸";

/// Padding above the first row of the tree.
const NAV_PAD_TOP: f32 = 4.0;
/// Room left below the last row.
const NAV_PAD_BOTTOM: f32 = 40.0;
/// Rows drawn beyond each edge of the viewport, so a small scroll has its
/// rows already laid out.
const NAV_OVERDRAW: f32 = 200.0;

fn nav_metrics(s: Scale) -> nav_rows::Metrics {
    nav_rows::Metrics { lh13: s.lh_f(13.0), lh12: s.lh_f(12.0), lh11: s.lh_f(11.0) }
}

fn leaf_icon(leaf: Leaf) -> &'static str {
    match leaf {
        Leaf::Views => "👁",
        Leaf::Indexes => "⚡",
    }
}

pub fn swatch_color(tab_color: &str) -> Option<Rgba> {
    dialogs::parse_hex(tab_color)
}

fn clamp_menu(pos: Point<Pixels>, kind: &str, window: &Window) -> Point<Pixels> {
    let pad = 8.0;
    let w = 220.0;
    let h = match kind {
        "database" => 318.0,
        "databaseNode" => 50.0,
        "dropConfirm" => 120.0,
        _ => 165.0,
    };
    let vw: f32 = window.viewport_size().width.into();
    let vh: f32 = window.viewport_size().height.into();
    let max_x = (vw - w - pad).max(pad);
    let max_y = (vh - h - pad).max(pad);
    point(px(f32::from(pos.x).min(max_x).max(pad)), px(f32::from(pos.y).min(max_y).max(pad)))
}

/// `<select>` drawn like WebKit's styled menulist-button.
#[allow(clippy::too_many_arguments)]
pub fn native_select(
    id: &'static str,
    s: Scale,
    label: &str,
    open: bool,
    options: Vec<(&'static str, &'static str)>,
    selected: &str,
    on_toggle: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static,
    on_pick: impl Fn(&String, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let lh = s.lh_f(13.0);
    let on_pick = std::rc::Rc::new(on_pick);
    let mut root = div().relative().w_full().child(
        div()
            .id(id)
            .w_full()
            .h(px(lh + 14.0 + 2.0))
            .pl(px(10.))
            .pr(px(10.))
            .flex()
            .items_center()
            .justify_between()
            .bg(theme::BG_INPUT)
            .border_1()
            .border_color(if open { theme::ACCENT } else { theme::BORDER })
            .rounded(px(4.))
            .t(s, 13.0)
            .text_color(theme::TEXT)
            .cursor_default()
            .on_click(on_toggle)
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().child(label.to_string()))
            .child(select_arrows(s)),
    );
    if open {
        let mut menu = div()
            .id(SharedString::from(format!("{id}-menu")))
            .occlude()
            .absolute()
            .top(px(lh + 16.0 + 2.0))
            .left_0()
            .min_w_full()
            .py(px(5.))
            .px(px(5.))
            .bg(theme::hex(0x2a2a2c))
            .border_1()
            .border_color(theme::rgba8(255, 255, 255, 0.12))
            .rounded(px(8.))
            .shadow(vec![shadow(0.0, 8.0, 24.0, 0.0, theme::rgba8(0, 0, 0, 0.35))]);
        for (value, text) in options {
            let v = value.to_string();
            let pick = on_pick.clone();
            let is_sel = value == selected;
            menu = menu.child(
                div()
                    .id(SharedString::from(format!("{id}-{value}")))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .rounded(px(4.))
                    .t(s, 13.0)
                    .text_color(theme::WHITE)
                    .hover(|st| st.bg(theme::hex(0x0a64d8)))
                    .child(div().w(px(12.)).child(if is_sel { "✓" } else { "" }))
                    .child(text)
                    .on_click(move |_, w, cx| pick(&v, w, cx)),
            );
        }
        root = root.child(deferred(menu).with_priority(5));
    }
    root.into_any_element()
}

/// The up/down chevrons WebKit paints on styled `<select>` elements.
pub fn select_arrows(s: Scale) -> Div {
    div()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(2.))
        .w(px(8.))
        .flex_shrink_0()
        .child(div().text_size(px(7.0 * s.0)).line_height(px(5.)).child("▲"))
        .child(div().text_size(px(7.0 * s.0)).line_height(px(5.)).child("▼"))
}

impl Workspace {
    fn nav_icon_btn(&self, id: impl Into<SharedString>, s: Scale, glyph: &'static str) -> Stateful<Div> {
        div()
            .id(gpui::ElementId::Name(id.into()))
            .px(px(4.))
            .py(px(2.))
            .text_size(s.fs(12.0))
            .line_height(s.fs(12.0))
            .font_weight(FontWeight::NORMAL)
            .text_color(theme::TEXT_MUTED)
            .cursor_pointer()
            .child(glyph)
    }

    fn toggle_conn(&mut self, conn_id: &str, cx: &mut Context<Self>) {
        let now_open = if self.expanded.contains(conn_id) {
            self.expanded.remove(conn_id);
            false
        } else {
            self.expanded.insert(conn_id.to_string());
            true
        };
        if now_open && self.connection(conn_id).is_some_and(|c| c.schema.is_none()) {
            self.ensure_schema(conn_id, cx);
        }
        self.selected_conn_id = conn_id.to_string();
        cx.notify();
    }

    fn toggle_key(&mut self, key: &str, cx: &mut Context<Self>) {
        if !self.expanded_tables.remove(key) {
            self.expanded_tables.insert(key.to_string());
        }
        cx.notify();
    }

    fn toggle_group(&mut self, id: &str, cx: &mut Context<Self>) {
        let open = if self.expanded_groups.remove(id) {
            false
        } else {
            self.expanded_groups.insert(id.to_string());
            true
        };
        self.active_server_group_id = if open { id.to_string() } else { String::new() };
        cx.notify();
    }

    fn open_table_query(&mut self, conn_id: &str, table: &str, schema: Option<&str>, database: Option<&str>, cx: &mut Context<Self>) {
        let driver = self.connection(conn_id).map(|c| c.config.driver.clone()).unwrap_or_else(|| "postgres".into());
        let q = sql_text::qualify_table(&driver, table, schema);
        let sql = format!("SELECT * FROM {q} LIMIT 100;");
        self.open_query_tab_with_sql(conn_id, database.unwrap_or(""), &sql, Some(table.to_string()), cx);
        self.selected_conn_id = conn_id.to_string();
    }

    fn qualified_name(&self, conn_id: &str, table: &str, schema: Option<&str>) -> String {
        let driver = self.connection(conn_id).map(|c| c.config.driver.clone()).unwrap_or_else(|| "postgres".into());
        sql_text::qualify_table(&driver, table, schema)
    }

    // ─── Drag and drop ──────────────────────────────────────────────────────

    pub fn on_nav_drag_move(&mut self, e: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &mut self.nav_drag else { return };
        if !e.dragging() {
            return;
        }
        if !d.active {
            let dx = (e.position.x - d.start.x).abs();
            let dy = (e.position.y - d.start.y).abs();
            if dx.max(dy) < px(4.) {
                return;
            }
            d.active = true;
            self.suppress_nav_click = true;
        }
        self.autoscroll_nav_drag(e.position.y);
        cx.notify();
    }

    /// Dragging near the top or bottom edge of the tree scrolls it, so a
    /// connection can be dropped on a row that is currently scrolled out of
    /// view (and therefore, now that off-screen rows are not drawn, has no
    /// element to hover yet).
    fn autoscroll_nav_drag(&mut self, y: Pixels) {
        const EDGE: f32 = 28.0;
        const STEP: f32 = 12.0;
        let b = self.nav_scroll.bounds();
        let (top, bottom) = (f32::from(b.top()), f32::from(b.bottom()));
        if bottom - top <= 2.0 * EDGE {
            return;
        }
        let y = f32::from(y);
        let delta = if y < top + EDGE && y >= top - EDGE {
            -STEP
        } else if y > bottom - EDGE && y <= bottom + EDGE {
            STEP
        } else {
            return;
        };
        let max = f32::from(self.nav_scroll.max_offset().height).max(0.0);
        let mut offset = self.nav_scroll.offset();
        // gpui offsets run negative as the content scrolls up.
        let next = (-f32::from(offset.y) + delta).clamp(0.0, max);
        offset.y = px(-next);
        self.nav_scroll.set_offset(offset);
    }

    pub fn on_nav_drag_end(&mut self, _e: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = self.nav_drag.take() else { return };
        if !d.active {
            return;
        }
        match &d.kind {
            DragKind::Conn(id) => {
                if let Some((target, group)) = &d.target_conn {
                    if target != id {
                        self.move_connection(id, Some(target), Some(d.after), group.as_deref());
                        match group {
                            Some(g) => {
                                self.expanded_groups.insert(g.clone());
                                self.active_server_group_id = g.clone();
                            }
                            None => self.active_server_group_id.clear(),
                        }
                    }
                } else if let Some(g) = &d.target_group {
                    self.add_connection_to_group(id, g);
                    self.expanded_groups.insert(g.clone());
                    self.active_server_group_id = g.clone();
                } else {
                    self.move_connection(id, None, None, None);
                    self.active_server_group_id.clear();
                }
            }
            DragKind::Group(id) => {
                if let Some(g) = &d.target_group {
                    if g != id {
                        self.move_server_group(id, g, d.after);
                    }
                }
            }
        }
        cx.notify();
    }

    fn drag_over_conn(&mut self, conn_id: &str, group: Option<String>, bounds_mid: Pixels, y: Pixels) {
        if let Some(d) = &mut self.nav_drag {
            if !d.active {
                return;
            }
            d.target_group = None;
            d.target_conn = None;
            if let DragKind::Conn(id) = &d.kind {
                if id != conn_id {
                    d.after = y > bounds_mid;
                    d.target_conn = Some((conn_id.to_string(), group));
                }
            }
        }
    }

    fn drag_over_group(&mut self, group_id: &str, bounds_mid: Pixels, y: Pixels) {
        if let Some(d) = &mut self.nav_drag {
            if !d.active {
                return;
            }
            d.target_group = None;
            d.target_conn = None;
            match &d.kind {
                DragKind::Group(id) if id != group_id => {
                    d.after = y > bounds_mid;
                    d.target_group = Some(group_id.to_string());
                }
                DragKind::Conn(_) => d.target_group = Some(group_id.to_string()),
                _ => {}
            }
        }
    }

    // ─── Rendering ──────────────────────────────────────────────────────────

    pub fn render_navigator(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        // The tree follows the search *results*, not the live text: between a
        // keystroke and the debounced search there are no results for the new
        // text yet, and treating that gap as "filtering with nothing excluded"
        // force-expanded every connection, schema and table at once.
        let nav_matches = self.effective_nav_matches(cx);
        let has_filter_text = !self.nav_filter.read(cx).text().is_empty();

        // Header
        let title = spaced_text(
            "CONNECTIONS",
            gpui::Font { weight: FontWeight::SEMIBOLD, ..gpui::font(theme::UI_FONT) },
            11.0 * s.0,
            s.lh_f(11.0),
            theme::TEXT_MUTED,
            0.55 * s.0,
        );
        let add_btn = self
            .nav_icon_btn("nav-add", s, "+")
            .size(px(22.))
            .p_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|st| st.bg(theme::BG_HOVER).text_color(theme::TEXT))
            .on_click(cx.listener(|this, _, _, cx| {
                this.add_menu_open = !this.add_menu_open;
                this.settings_open = false;
                cx.notify();
            }));
        let settings_btn = self
            .nav_icon_btn("nav-settings", s, "⚙")
            .size(px(22.))
            .p_0()
            .flex()
            .items_center()
            .justify_center()
            .rounded(px(4.))
            .hover(|st| st.bg(theme::BG_HOVER).text_color(theme::TEXT))
            .on_click(cx.listener(|this, _, _, cx| {
                this.settings_open = !this.settings_open;
                this.add_menu_open = false;
                cx.notify();
            }));
        let mut header = div()
            .id("nav-header")
            .relative()
            .flex()
            .items_center()
            .justify_between()
            .px(px(12.))
            .py(px(8.))
            .border_b_1()
            .border_color(theme::BORDER)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().flex().items_center().gap(px(6.)).min_w(px(0.)).child(title).child(add_btn))
            .child(settings_btn);
        if self.add_menu_open {
            let item = |id: &'static str, label: &'static str| {
                div()
                    .id(id)
                    .w_full()
                    .px(px(12.))
                    .py(px(8.))
                    .t(s, 12.0)
                    .text_color(theme::TEXT)
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::BG_HOVER))
                    .child(label)
            };
            header = header.child(deferred(
                div()
                    .id("add-menu")
                    .occlude()
                    .absolute()
                    .top(px(8.0 + 22.0 + 8.0 + 1.0 + 4.0))
                    .left(px(12.))
                    .min_w(px(170.))
                    .bg(theme::BG_SURFACE)
                    .border_1()
                    .border_color(theme::BORDER)
                    .rounded(px(4.))
                    .shadow(vec![shadow(0.0, 4.0, 16.0, 0.0, theme::rgba8(0, 0, 0, 0.4))])
                    .overflow_hidden()
                    .child(item("add-conn", "New Connection").on_click(cx.listener(|this, _, window, cx| {
                        this.add_menu_open = false;
                        this.open_connection_dialog(None, window, cx);
                    })))
                    .child(item("add-group", "New Server Group").on_click(cx.listener(|this, _, window, cx| {
                        this.add_menu_open = false;
                        this.server_group_dialog = Some(dialogs::ServerGroupDialog::new(this.scale(), window, cx));
                        cx.notify();
                    }))),
            ).with_priority(2));
        }
        if self.settings_open {
            let scale_btn = |id: &'static str, glyph: &'static str| {
                div()
                    .id(id)
                    .size(px(26.))
                    .flex()
                    .items_center()
                    .justify_center()
                    .border_1()
                    .border_color(theme::BORDER)
                    .rounded(px(4.))
                    .bg(theme::BG_INPUT)
                    .text_size(s.fs(12.0))
                    .line_height(s.fs(12.0))
                    .text_color(theme::TEXT_MUTED)
                    .cursor_pointer()
                    .hover(|st| st.text_color(theme::TEXT))
                    .child(glyph)
            };
            header = header.child(deferred(
                div()
                    .id("settings-menu")
                    .occlude()
                    .absolute()
                    .top(px(8.0 + 22.0 + 8.0 + 1.0 + 4.0))
                    .right(px(8.))
                    .w(px(220.))
                    .p(px(10.))
                    .bg(theme::BG_SURFACE)
                    .border_1()
                    .border_color(theme::BORDER)
                    .rounded(px(4.))
                    .shadow(vec![shadow(0.0, 4.0, 16.0, 0.0, theme::rgba8(0, 0, 0, 0.4))])
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(8.))
                            .t(s, 12.0)
                            .text_color(theme::TEXT_MUTED)
                            .child("Font size")
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(6.))
                                    .child(scale_btn("scale-dec", "−").on_click(cx.listener(|this, _, _, cx| {
                                        let v = this.settings.font_scale() - 10.0;
                                        this.set_font_scale(v, cx);
                                    })))
                                    .child(div().flex_1().min_w(px(0.)).child(self.font_scale_input.clone()))
                                    .child(div().text_color(theme::TEXT_MUTED).child("%"))
                                    .child(scale_btn("scale-inc", "+").on_click(cx.listener(|this, _, _, cx| {
                                        let v = this.settings.font_scale() + 10.0;
                                        this.set_font_scale(v, cx);
                                    }))),
                            ),
                    ),
            ).with_priority(2));
        }

        // Filter
        let filter_box = div()
            .relative()
            .p(px(8.))
            .border_b_1()
            .border_color(theme::BORDER)
            .child(self.nav_filter.clone())
            .when(has_filter_text, |d| {
                d.child(
                    div()
                        .id("filter-clear")
                        .absolute()
                        .right(px(14.))
                        .top(px(8.0 + 14.0))
                        .mt(px(-(s.lh_f(11.0) + 4.0) / 2.0))
                        .px(px(4.))
                        .py(px(2.))
                        .t(s, 11.0)
                        .text_color(theme::TEXT_MUTED)
                        .cursor_pointer()
                        .hover(|st| st.text_color(theme::TEXT))
                        .on_click(cx.listener(|this, _, _, cx| this.clear_nav_filter(cx)))
                        .child("Clear"),
                )
            });

        // Tree. Flattened into rows (see `nav_rows`), and only the rows that
        // fall inside the scrolled viewport become elements; a spacer above
        // and below them keeps the content exactly as tall as the whole tree,
        // so the scrollbar and wheel scrolling behave as if it were all there.
        let rows = nav_rows::build(
            &nav_rows::TreeState {
                connections: &self.connections,
                groups: &self.settings.server_groups,
                expanded: &self.expanded,
                expanded_tables: &self.expanded_tables,
                expanded_groups: &self.expanded_groups,
                matches: nav_matches.as_deref(),
            },
            nav_metrics(s),
        );
        let min_width = rows.iter().map(|r| self.estimate_row_width(r, s)).fold(0.0_f32, f32::max);
        // Last frame's viewport. Scrolling notifies the view, so the next
        // render sees the new offset; before the first layout there is no
        // viewport yet, so assume the window's height.
        let view_h = {
            let h = f32::from(self.nav_scroll.bounds().size.height);
            if h > 0.5 { h } else { f32::from(window.viewport_size().height) }
        };
        // The offset is last frame's too: when the tree has just got shorter
        // (a collapse, a narrower filter) gpui will clamp it during layout, so
        // clamp it here the same way or this frame would draw rows below the
        // new end and leave the viewport empty.
        let content_h = NAV_PAD_TOP + nav_rows::total_height(&rows) + NAV_PAD_BOTTOM;
        let max_scroll = (content_h - view_h).max(0.0);
        let scroll_top = ((-f32::from(self.nav_scroll.offset().y)).min(max_scroll) - NAV_PAD_TOP).max(0.0);
        let (range, before, after) = nav_rows::visible_range(&rows, scroll_top, view_h, NAV_OVERDRAW);
        let mut list = div().flex().flex_col().min_w(px(min_width));
        if before > 0.0 {
            list = list.child(div().flex_shrink_0().h(px(before)));
        }
        for row in &rows[range] {
            list = list.child(self.render_nav_row(row, s, cx));
        }
        if after > 0.0 {
            list = list.child(div().flex_shrink_0().h(px(after)));
        }
        let mut items: Vec<AnyElement> = vec![list.into_any_element()];
        if self.connections.is_empty() {
            items.push(
                div()
                    .px(px(16.))
                    .py(px(20.))
                    .flex()
                    .flex_col()
                    .items_center()
                    .child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).mb(px(8.)).child("No connections."))
                    .child(
                        div()
                            .id("empty-new-conn")
                            .t(s, 12.0)
                            .text_color(theme::ACCENT)
                            .cursor_pointer()
                            .px(px(6.))
                            .py(px(1.))
                            .hover(|st| st.underline())
                            .on_click(cx.listener(|this, _, window, cx| {
                                this.add_menu_open = false;
                                this.open_connection_dialog(None, window, cx);
                            }))
                            .child("+ New Connection"),
                    )
                    .into_any_element(),
            );
        }

        // A flex row holding one column, aligned to the start: see
        // `scroll::scroll_body` for why both of those are load-bearing.
        let content = scroll::scroll_body(div().id("nav-content"))
            .flex_1()
            .min_h(px(0.))
            .overflow_scroll()
            .track_scroll(&self.nav_scroll)
            // One child holding the rows. Its padding lives here rather than on
            // the scroll container so that it counts towards the content size.
            // The right padding keeps the rows clear of the vertical scrollbar,
            // which is drawn over the pane's edge rather than taking layout
            // space.
            .child(
                scroll::scroll_content(div())
                    .pt(px(NAV_PAD_TOP))
                    .pb(px(NAV_PAD_BOTTOM))
                    .pr(px(if f32::from(self.nav_scroll.max_offset().height) > 0.5 { scroll::SCROLLBAR } else { 0.0 }))
                    .children(items),
            );

        // The tree scrolls, so it gets a scrollbar over its right edge.
        let nav_scroll = self.nav_scroll.clone();
        let scrolling = self.scrollable("nav", &nav_scroll, content.into_any_element(), cx);

        div()
            .flex()
            .flex_col()
            .h_full()
            .overflow_hidden()
            .bg(theme::BG_PANEL)
            .border_r_1()
            .border_color(theme::BORDER)
            .min_w(px(0.))
            .child(header)
            .child(filter_box)
            .child(scrolling.flex_1().min_h(px(0.)))
            .into_any_element()
    }

    /// One row of the flattened tree, positioned as the nested layout had it:
    /// indented by `ml`, separated by `mt`, and exactly `height` tall so the
    /// spacers standing in for the rows that are not drawn add up.
    fn render_nav_row(&mut self, row: &Row, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let h = row.height;
        let body = match row.kind {
            RowKind::Group { group, count, open } => self.render_group_row(group, count, open, h, s, cx),
            RowKind::Conn { conn, group, open } => self.render_conn_row(conn, group, open, h, s, cx),
            RowKind::Info { conn, info } => {
                let c = &self.connections[conn];
                let (text, color) = match info {
                    Info::Loading => ("Loading schema…".to_string(), theme::TEXT_MUTED),
                    Info::Error => (c.schema_error.clone().unwrap_or_default(), theme::ERROR),
                    Info::NotLoaded => ("Click to load schema".to_string(), theme::TEXT_MUTED),
                };
                nav_info(s, &text, color).h(px(h)).whitespace_nowrap().into_any_element()
            }
            RowKind::Database { conn, db, open } => self.render_database_row(conn, db, open, h, s, cx),
            RowKind::Schema { conn, db, schema, open } => self.render_schema_row(conn, db, schema, open, h, s, cx),
            RowKind::Tables { conn, db, schema, count, open } => {
                let (conn_id, sc_name) = self.row_names(conn, db, schema);
                let key = nav_rows::tables_key(&conn_id, sc_name.as_deref());
                self.section_header(key, "Tables", count, open, h, s, cx)
            }
            RowKind::Table { conn, db, schema, table, open } => self.render_table_row(conn, db, schema, table, open, h, s, cx),
            RowKind::Column { conn, db, schema, table, column } => {
                let tree = self.connections[conn].schema.clone();
                match tree.as_deref().and_then(|t| nav_rows::tables_of(t, db, schema).get(table as usize)).and_then(|t| t.columns.get(column as usize)) {
                    None => div().into_any_element(),
                    Some(c) => div()
                    .h(px(h))
                    .flex()
                    .items_center()
                    .gap(px(6.))
                    .px(px(8.))
                    .py(px(2.))
                    .t(s, 11.0)
                    .text_color(theme::TEXT_MUTED)
                    .min_w_full()
                    .child(div().flex_shrink_0().whitespace_nowrap().child(c.name.clone()))
                    .child(div().flex_1().min_w(px(12.)))
                    .when(c.key == "PRI", |d| d.child(div().w(px(14.)).flex_shrink_0().child("🔑")))
                    .child(div().flex_shrink_0().opacity(0.6).italic().child(c.column_type.clone()))
                    .into_any_element(),
                }
            }
            RowKind::LeafSection { conn, db, schema, leaf, count, open } => {
                let (conn_id, sc_name) = self.row_names(conn, db, schema);
                let key = nav_rows::leaf_key(&conn_id, sc_name.as_deref(), leaf);
                let label = match leaf {
                    Leaf::Views => "Views",
                    Leaf::Indexes => "Indexes",
                };
                self.section_header(key, label, count, open, h, s, cx)
            }
            RowKind::LeafItem { conn, db, schema, leaf, item } => {
                let name = self.connections[conn]
                    .schema
                    .as_ref()
                    .map(|t| nav_rows::leaf_name(t, db, schema, leaf, item).to_string())
                    .unwrap_or_default();
                div()
                    .h(px(h))
                    .flex()
                    .items_center()
                    .gap(px(4.))
                    .pl(px(24.))
                    .pr(px(8.))
                    .py(px(3.))
                    .t(s, 12.0)
                    .text_color(theme::TEXT)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::BG_HOVER))
                    .child(div().flex_shrink_0().child(leaf_icon(leaf)))
                    .child(name)
                    .into_any_element()
            }
            RowKind::Gap => div().into_any_element(),
        };
        div()
            .flex_shrink_0()
            .mt(px(row.gap))
            .ml(px(row.indent))
            .h(px(h))
            .flex()
            .flex_col()
            .child(body)
            .into_any_element()
    }

    /// The connection id and, below a schema level, the schema's name: the
    /// two things the expansion keys are made of.
    fn row_names(&self, conn: usize, db: u32, schema: u32) -> (String, Option<String>) {
        let c = &self.connections[conn];
        let sc = c.schema.as_ref().and_then(|t| nav_rows::schema_of(t, db, schema)).map(|s| s.name.clone());
        (c.config.id.clone(), sc)
    }

    /// A rough width for a row, so that the horizontal extent of the tree
    /// does not depend on which rows happen to be drawn. Deliberately on the
    /// low side: a row that is on screen is measured for real anyway, and an
    /// estimate that ran long would conjure a scrollbar out of nothing.
    fn estimate_row_width(&self, row: &Row, s: Scale) -> f32 {
        let chars = |text: &str, size: f32| text.chars().count() as f32 * size * s.0 * 0.5;
        let text = match row.kind {
            RowKind::Table { conn, db, schema, table, .. } => self.connections[conn]
                .schema
                .as_ref()
                .and_then(|t| nav_rows::tables_of(t, db, schema).get(table as usize))
                .map(|t| chars(&t.name, 12.0) + 40.0),
            RowKind::Column { conn, db, schema, table, column } => self.connections[conn]
                .schema
                .as_ref()
                .and_then(|t| nav_rows::tables_of(t, db, schema).get(table as usize))
                .and_then(|t| t.columns.get(column as usize))
                .map(|c| chars(&c.name, 11.0) + chars(&c.column_type, 11.0) + 40.0),
            RowKind::Conn { conn, .. } => Some(chars(&self.connections[conn].config.name, 13.0) + 80.0),
            _ => None,
        };
        text.map_or(0.0, |w| row.indent + w)
    }

    fn render_group_row(&mut self, gi: usize, count: usize, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let Some(g) = self.settings.server_groups.get(gi) else { return div().into_any_element() };
        let (id, title) = (g.id.clone(), g.title.clone());
        let active = self.active_server_group_id == id;
        let drag = self.nav_drag.clone();
        let is_target = drag.as_ref().is_some_and(|d| d.active && d.target_group.as_deref() == Some(&id));
        let show_line = is_target && matches!(drag.as_ref().map(|d| &d.kind), Some(DragKind::Group(_)));
        let after = drag.as_ref().is_some_and(|d| d.after);
        let dragging = drag.as_ref().is_some_and(|d| d.active && d.kind == DragKind::Group(id.clone()));
        let (i1, i2, i3) = (id.clone(), id.clone(), id.clone());
        let row = div()
            .id(SharedString::from(format!("group-{id}")))
            .relative()
            .h(px(h))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(5.))
            .t(s, 13.0)
            .text_color(theme::TEXT)
            .whitespace_nowrap()
            .cursor(CursorStyle::OpenHand)
            .when(active, |d| d.bg(theme::BG_HOVER))
            .hover(|st| st.bg(theme::BG_HOVER))
            .when(dragging, |d| d.opacity(0.6))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, _cx| {
                this.nav_drag = Some(NavDrag { kind: DragKind::Group(i1.clone()), start: e.position, active: false, target_conn: None, target_group: None, after: false });
            }))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _w, cx| {
                if this.nav_drag.as_ref().is_some_and(|d| d.active) {
                    let mid = this.nav_row_mid(&format!("group:{i3}"), e.position.y);
                    this.drag_over_group(&i3, mid, e.position.y);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if std::mem::take(&mut this.suppress_nav_click) {
                    return;
                }
                this.toggle_group(&i2, cx);
            }))
            .child(chevron(s, open))
            .child(div().flex_shrink_0().child("▣"))
            .child(div().flex_shrink_0().child(title))
            .child(div().font_weight(FontWeight::NORMAL).opacity(0.7).child(format!("({count})")))
            .when(show_line, |d| d.child(drop_line(after)));
        record_row(row.into_any_element(), format!("group:{id}"), cx)
    }

    fn render_conn_row(&mut self, ci: usize, group: Option<usize>, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let conn = self.connections[ci].clone();
        let group = group.and_then(|g| self.settings.server_groups.get(g)).map(|g| g.id.clone());
        let id = conn.config.id.clone();
        let selected = self.selected_conn_id == id;
        let drag = self.nav_drag.clone();
        let is_target = drag.as_ref().is_some_and(|d| d.active && d.target_conn.as_ref().is_some_and(|(c, _)| *c == id));
        let after = drag.as_ref().is_some_and(|d| d.after);
        let dragging = drag.as_ref().is_some_and(|d| d.active && d.kind == DragKind::Conn(id.clone()));
        let swatch = swatch_color(&conn.config.tab_color);
        let size_label = format_bytes(conn.schema.as_ref().and_then(|s| s.size_bytes));
        let group_name = SharedString::from(format!("conn-{id}"));
        let row_hovered = self.hover_conn.as_deref() == Some(id.as_str());
        let (i1, i2, i3, i4, i5, i6) = (id.clone(), id.clone(), id.clone(), id.clone(), id.clone(), id.clone());
        let g1 = group.clone();
        let cfg = conn.config.clone();
        let label = div()
            .id(SharedString::from(format!("conn-{}-{}", id, group.clone().unwrap_or_else(|| "root".into()))))
            .group(group_name.clone())
            .relative()
            .h(px(h))
            .flex()
            .items_center()
            .gap(px(4.))
            .pr(px(8.))
            .py(px(5.))
            .pl(px(if group.is_some() { 24. } else { 8. }))
            .t(s, 13.0)
            .text_color(theme::TEXT)
            .cursor(CursorStyle::OpenHand)
            .hover(|st| st.bg(theme::BG_HOVER))
            .when(selected, |d| d.bg(theme::BG_SELECTED))
            .when(dragging, |d| d.opacity(0.6))
            .on_hover(cx.listener({
                let id = id.clone();
                move |this, hovered: &bool, _w, cx| {
                    if *hovered {
                        this.hover_conn = Some(id.clone());
                    } else if this.hover_conn.as_deref() == Some(id.as_str()) {
                        this.hover_conn = None;
                    }
                    cx.notify();
                }
            }))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, _cx| {
                this.nav_drag = Some(NavDrag { kind: DragKind::Conn(i1.clone()), start: e.position, active: false, target_conn: None, target_group: None, after: false });
            }))
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _w, cx| {
                if this.nav_drag.as_ref().is_some_and(|d| d.active) {
                    let mid = this.nav_row_mid(&format!("conn:{i2}"), e.position.y);
                    this.drag_over_conn(&i2, g1.clone(), mid, e.position.y);
                    cx.notify();
                }
            }))
            .on_click(cx.listener(move |this, _, _, cx| {
                if std::mem::take(&mut this.suppress_nav_click) {
                    return;
                }
                this.toggle_conn(&i3, cx);
            }))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let pos = clamp_menu(e.position, "database", window);
                this.nav_menu = Some(NavMenu::Database { pos, conn_id: i4.clone() });
                cx.notify();
            }))
            .child(chevron(s, open))
            .child(div().flex_shrink_0().child("🔌"))
            .child(
                div()
                    .size(px(10.))
                    .flex_shrink_0()
                    .rounded(px(2.))
                    .border_1()
                    .border_color(if swatch.is_some() { theme::rgba8(255, 255, 255, 0.35) } else { theme::BORDER })
                    .when_some(swatch, |d, c| d.bg(c)),
            )
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(6.))
                    .font_weight(FontWeight::MEDIUM)
                    .flex_shrink_0()
                    .whitespace_nowrap()
                    .child(div().flex_shrink_0().child(cfg.name.clone()))
                    .when(!size_label.is_empty(), |d| {
                        d.child(div().flex_shrink_0().t(s, 11.0).font_weight(FontWeight::NORMAL).text_color(theme::TEXT_MUTED).opacity(0.72).child(size_label.clone()))
                    }),
            )
            .child(
                div()
                    .t(s, 10.0)
                    .px(px(5.))
                    .py(px(1.))
                    .rounded(px(3.))
                    .bg(theme::BG_BADGE)
                    .text_color(theme::TEXT_MUTED)
                    .child(cfg.driver.clone()),
            )
            .child(
                div()
                    .when(!row_hovered, |d| d.hidden())
                    .when(row_hovered, |d| d.flex())
                    .gap(px(2.))
                    .items_center()
                    .child(self.nav_icon_btn(format!("edit-{id}"), s, "✏️").hover(|st| st.text_color(theme::TEXT)).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        let cfg = this.connection(&i5).map(|c| c.config.clone());
                        this.open_connection_dialog(cfg, window, cx);
                    })))
                    .child(self.nav_icon_btn(format!("disc-{id}"), s, "✕").hover(|st| st.text_color(theme::TEXT)).on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).on_click(cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.disconnect_conn(&i6, cx);
                    }))),
            )
            .when(is_target, |d| d.child(drop_line(after)));

        record_row(label.into_any_element(), format!("conn:{id}"), cx)
    }

    fn render_database_row(&mut self, ci: usize, db: u32, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let conn = &self.connections[ci];
        let id = conn.config.id.clone();
        let dbname = nav_rows::database_name(conn, db);
        let db_key = nav_rows::database_key(&id, &dbname);
        let k = db_key.clone();
        // A database the tree holds several of narrows to that database; the
        // connection's only database is all of it.
        let scope = crate::ui::storage::StorageScope {
            database: (db != NONE).then(|| dbname.clone()),
            schema: None,
            label: dbname.clone(),
        };
        self.section_label(format!("db-{db_key}"), s, open, true, cx.listener(move |this, _, _, cx| this.toggle_key(&k, cx)))
            .h(px(h))
            .whitespace_nowrap()
            .on_mouse_down(MouseButton::Right, self.storage_menu_listener(&id, scope, cx))
            .child(div().flex_shrink_0().child("🗄"))
            .child(div().flex_shrink_0().child(dbname))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_schema_row(&mut self, ci: usize, db: u32, schema: u32, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let conn = &self.connections[ci];
        let id = conn.config.id.clone();
        let dbname = nav_rows::database_name(conn, db);
        let Some(sc) = conn.schema.as_ref().and_then(|t| nav_rows::schema_of(t, db, schema)) else {
            return div().into_any_element();
        };
        let name = sc.name.clone();
        let size = format_bytes(sc.size_bytes);
        let schema_key = nav_rows::schema_key(&id, &name);
        let scope = crate::ui::storage::StorageScope {
            database: (db != NONE).then(|| dbname.clone()),
            schema: Some(name.clone()),
            label: if dbname.is_empty() { name.clone() } else { format!("{dbname}.{name}") },
        };
        let sk = schema_key.clone();
        self.section_label(format!("s-{schema_key}"), s, open, true, cx.listener(move |this, _, _, cx| this.toggle_key(&sk, cx)))
            .h(px(h))
            .whitespace_nowrap()
            .on_mouse_down(MouseButton::Right, self.storage_menu_listener(&id, scope, cx))
            .child(div().flex_shrink_0().child("🗂"))
            .child(div().flex_shrink_0().child(name))
            .when(!size.is_empty(), |d| d.child(size_label(s, &size)))
            .into_any_element()
    }

    /// The "Tables", "Views" and "Indexes" headers.
    #[allow(clippy::too_many_arguments)]
    fn section_header(&mut self, key: String, label: &'static str, count: usize, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let k = key.clone();
        div()
            .id(SharedString::from(format!("sec-{key}")))
            .h(px(h))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(3.))
            .t(s, 12.0)
            .text_color(theme::TEXT_MUTED)
            .font_weight(FontWeight::MEDIUM)
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|st| st.text_color(theme::TEXT))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(&k, cx)))
            .child(chevron(s, open))
            .child(label)
            .child(div().font_weight(FontWeight::NORMAL).opacity(0.7).child(format!("({count})")))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_table_row(&mut self, ci: usize, db: u32, schema: u32, table: u32, open: bool, h: f32, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let conn = &self.connections[ci];
        let conn_id = conn.config.id.clone();
        let dbname = nav_rows::database_name(conn, db);
        let Some(tree) = conn.schema.clone() else { return div().into_any_element() };
        let sc_name = nav_rows::schema_of(&tree, db, schema).map(|s| s.name.clone());
        let Some(t) = nav_rows::tables_of(&tree, db, schema).get(table as usize) else {
            return div().into_any_element();
        };
        let tkey = nav_rows::table_key(&conn_id, sc_name.as_deref(), &t.name);
        let size = format_bytes(t.size_bytes);
        // Flat trees have no database level to name.
        let database = if dbname.is_empty() || schema == NONE { None } else { Some(dbname) };
        let (tk, cid, tname, sc2, db2) = (tkey.clone(), conn_id, t.name.clone(), sc_name, database);
        div()
            .id(SharedString::from(format!("tbl-{tkey}")))
            .h(px(h))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(3.))
            .t(s, 12.0)
            .text_color(theme::TEXT)
            .whitespace_nowrap()
            .cursor_pointer()
            .hover(|st| st.bg(theme::BG_HOVER))
            .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(&tk, cx)))
            .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                cx.stop_propagation();
                let pos = clamp_menu(e.position, "table", window);
                this.nav_menu = Some(NavMenu::Table { pos, conn_id: cid.clone(), table: tname.clone(), schema: sc2.clone(), database: db2.clone() });
                cx.notify();
            }))
            .child(chevron(s, open))
            .child(div().flex_shrink_0().child("📋"))
            .child(div().flex_shrink_0().child(t.name.clone()))
            .when(!size.is_empty(), |d| d.child(size_label(s, &size)))
            .into_any_element()
    }
    /// Right-click on a database or schema node: the storage menu for it.
    fn storage_menu_listener(
        &self,
        conn_id: &str,
        scope: crate::ui::storage::StorageScope,
        cx: &mut Context<Self>,
    ) -> impl Fn(&MouseDownEvent, &mut Window, &mut App) + 'static + use<> {
        let conn_id = conn_id.to_string();
        let view = cx.entity().downgrade();
        move |e: &MouseDownEvent, window: &mut Window, cx: &mut App| {
            cx.stop_propagation();
            let pos = clamp_menu(e.position, "databaseNode", window);
            view.update(cx, |this, cx| {
                this.nav_menu = Some(NavMenu::DatabaseNode { pos, conn_id: conn_id.clone(), scope: scope.clone() });
                cx.notify();
            })
            .ok();
        }
    }

    fn section_label(&self, id: String, s: Scale, open: bool, schema_node: bool, on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut App) + 'static) -> Stateful<Div> {
        let _ = schema_node;
        div()
            .id(SharedString::from(id))
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(3.))
            .t(s, 13.0)
            .text_color(theme::TEXT)
            .font_weight(FontWeight::MEDIUM)
            .cursor_pointer()
            .on_click(on_click)
            .child(chevron(s, open))
    }

    // ─── Overlays ───────────────────────────────────────────────────────────

    pub fn render_nav_overlays(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let s = Scale(self.scale());
        let mut layer = div().absolute().top_0().left_0().size_full();
        let mut any = false;
        if let Some(menu) = self.nav_menu.clone() {
            any = true;
            let item = |id: String, label: String, danger: bool| {
                div()
                    .id(SharedString::from(id))
                    .w_full()
                    .px(px(16.))
                    .py(px(8.))
                    .t(s, 13.0)
                    .text_color(if danger { theme::ERROR } else { theme::TEXT })
                    .cursor_pointer()
                    .when(danger, |d| d.hover(|st| st.bg(theme::rgba8(248, 113, 113, 0.1))))
                    .when(!danger, |d| d.hover(|st| st.bg(theme::BG_HOVER)))
                    .child(label)
            };
            let (pos, body): (Point<Pixels>, Div) = match menu.clone() {
                NavMenu::Table { pos, conn_id, table, schema, database } => {
                    let (c1, t1, s1, d1) = (conn_id.clone(), table.clone(), schema.clone(), database.clone());
                    let (c2, t2, s2) = (conn_id.clone(), table.clone(), schema.clone());
                    let (c3, t3, s3) = (conn_id.clone(), table.clone(), schema.clone());
                    let (c4, t4, s4) = (conn_id.clone(), table.clone(), schema.clone());
                    (
                        pos,
                        div()
                            .child(item("m-view".into(), "View Data (SELECT * LIMIT 100)".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.open_table_query(&c1, &t1, s1.as_deref(), d1.as_deref(), cx);
                            })))
                            .child(item("m-copy".into(), "Copy Name".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                let q = this.qualified_name(&c2, &t2, s2.as_deref());
                                this.copy_to_clipboard(q, cx);
                                cx.notify();
                            })))
                            .child(item("m-backup".into(), "Backup Table...".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.backup_table(&c3, &t3, s3.clone(), cx);
                            })))
                            .child(separator(3.0))
                            .child(item("m-drop".into(), "Drop Table...".into(), true).on_click(cx.listener(move |this, _, window, cx| {
                                let pos = clamp_menu(pos, "dropConfirm", window);
                                this.nav_menu = Some(NavMenu::DropConfirm { pos, conn_id: c4.clone(), table: t4.clone(), schema: s4.clone() });
                                cx.notify();
                            }))),
                    )
                }
                NavMenu::Database { pos, conn_id } => {
                    let testing = self.testing_conn_id.as_deref() == Some(&conn_id);
                    let ids: Vec<String> = (0..9).map(|_| conn_id.clone()).collect();
                    let [a, b, c, d, e, f, g, h, i]: [String; 9] = ids.try_into().unwrap();
                    (
                        pos,
                        div()
                            .child(item("m-query".into(), "Query".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                if this.connection(&a).is_some() {
                                    let id = this.add_tab(a.clone(), String::new(), cx);
                                    this.selected_conn_id = a.clone();
                                    this.set_active_tab(id, cx);
                                } else {
                                    this.status = "Connection not found.".into();
                                }
                                cx.notify();
                            })))
                            .child(separator(3.0))
                            .child(if testing {
                                item("m-cancel-test".into(), "Cancel Test".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                    this.nav_menu = None;
                                    this.cancel_test(cx);
                                    cx.notify();
                                }))
                            } else {
                                item("m-test".into(), "Test Connection".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                    this.nav_menu = None;
                                    this.test_connection(&b, cx);
                                }))
                            })
                            .child(separator(3.0))
                            .child(item("m-edit".into(), "Edit Connection...".into(), false).on_click(cx.listener(move |this, _, window, cx| {
                                this.nav_menu = None;
                                let cfg = this.connection(&h).map(|c| c.config.clone());
                                this.open_connection_dialog(cfg, window, cx);
                            })))
                            .child(separator(3.0))
                            .child(item("m-refresh".into(), "Refresh Schema".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.status = "Refreshing schema…".into();
                                let t = this.refresh_schema(&c, cx);
                                cx.spawn(async move |this, cx| {
                                    t.await;
                                    this.update(cx, |this, cx| {
                                        this.status = "Schema refreshed".into();
                                        cx.notify();
                                    })
                                    .ok();
                                })
                                .detach();
                            })))
                            .child(item("m-rel".into(), "Show Relationships".into(), false).on_click(cx.listener(move |this, _, window, cx| {
                                this.nav_menu = None;
                                this.show_relationships(&d, window, cx);
                            })))
                            .child(item("m-conns".into(), "Show Connections".into(), false).on_click(cx.listener(move |this, _, window, cx| {
                                this.nav_menu = None;
                                this.show_db_connections(&e, window, cx);
                            })))
                            .child(item("m-storage".into(), "Visualise Storage".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.show_storage(&i, crate::ui::storage::StorageScope::default(), cx);
                            })))
                            .child(item("m-import".into(), "Import...".into(), false).on_click(cx.listener(move |this, _, window, cx| {
                                this.nav_menu = None;
                                this.open_import(&f, window, cx);
                            })))
                            .child(separator(3.0))
                            .child(item("m-remove".into(), "Remove Connection".into(), true).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.disconnect_conn(&g, cx);
                            }))),
                    )
                }
                NavMenu::DatabaseNode { pos, conn_id, scope } => (
                    pos,
                    div().child(item("m-node-storage".into(), "Visualise Storage".into(), false).on_click(cx.listener(move |this, _, _, cx| {
                        this.nav_menu = None;
                        this.show_storage(&conn_id, scope.clone(), cx);
                    }))),
                ),
                NavMenu::DropConfirm { pos, conn_id, table, schema } => {
                    let label = match &schema {
                        Some(sc) if !sc.is_empty() => format!("Drop {sc}.{table}?"),
                        _ => format!("Drop {table}?"),
                    };
                    (
                        pos,
                        div()
                            .child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).px(px(16.)).py(px(8.)).border_b_1().border_color(theme::BORDER).child(label))
                            .child(item("m-drop-yes".into(), "Yes, drop table".into(), true).on_click(cx.listener(move |this, _, _, cx| {
                                this.nav_menu = None;
                                this.drop_table(&conn_id, &table, schema.clone(), cx);
                            })))
                            .child(item("m-drop-no".into(), "No, cancel".into(), false).on_click(cx.listener(|this, _, _, cx| {
                                this.close_floating();
                                cx.notify();
                            }))),
                    )
                }
            };
            layer = layer
                .child(
                    div()
                        .id("nav-menu-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.close_floating();
                            cx.notify();
                        }))
                        .on_mouse_down(MouseButton::Right, cx.listener(|this, _, _, cx| {
                            this.close_floating();
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("nav-menu")
                        .occlude()
                        .absolute()
                        .left(pos.x)
                        .top(pos.y)
                        .min_w(px(180.))
                        .bg(theme::BG_SURFACE)
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(4.))
                        .shadow(vec![shadow(0.0, 4.0, 16.0, 0.0, theme::rgba8(0, 0, 0, 0.4))])
                        .overflow_hidden()
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                );
        } else if self.add_menu_open || self.settings_open {
            any = true;
            layer = layer.child(
                div()
                    .id("nav-float-backdrop")
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                        this.close_floating();
                        cx.notify();
                    })),
            );
        }
        if let Some(d) = &self.server_group_dialog {
            any = true;
            let error = d.error.clone();
            let input = d.input.clone();
            layer = layer.child(
                overlay(0.6).child(
                    dialogs::modal_box(420.0)
                        .id("group-modal")
                        .occlude()
                        .child(dialogs::modal_header(s, "New Server Group", cx.listener(|this, _, _, cx| {
                            this.server_group_dialog = None;
                            cx.notify();
                        })))
                        .child(
                            div()
                                .p(px(20.))
                                .flex()
                                .flex_col()
                                .gap(px(12.))
                                .child(div().flex().flex_col().gap(px(4.)).child(dialogs::field_label(s, "Group Title")).child(input))
                                .when(!error.is_empty(), |d| d.child(div().t(s, 12.0).text_color(theme::ERROR).child(error.clone()))),
                        )
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .px(px(20.))
                                .py(px(16.))
                                .border_t_1()
                                .border_color(theme::BORDER)
                                .child(dialogs::button("group-cancel", s, "Cancel", Btn::Secondary, false, cx.listener(|this, _, _, cx| {
                                    this.server_group_dialog = None;
                                    cx.notify();
                                })))
                                .child(dialogs::button("group-create", s, "Create Group", Btn::Primary, false, cx.listener(|this, _, _, cx| this.save_server_group(cx)))),
                        ),
                ),
            );
        }
        any.then(|| layer.into_any_element())
    }

    fn backup_table(&mut self, conn_id: &str, table: &str, schema: Option<String>, cx: &mut Context<Self>) {
        let state = crate::ui::runtime::state();
        let (c, t, sc) = (conn_id.to_string(), table.to_string(), schema.clone().unwrap_or_default());
        let fut = crate::ui::runtime::spawn(async move { crate::commands::backup_table(state, c, t, sc).await });
        let q = self.qualified_name(conn_id, table, schema.as_deref());
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                this.status = match res {
                    Ok(_) => format!("Backed up {q}").into(),
                    Err(e) => e.into(),
                };
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn drop_table(&mut self, conn_id: &str, table: &str, schema: Option<String>, cx: &mut Context<Self>) {
        let state = crate::ui::runtime::state();
        let (c, t, sc) = (conn_id.to_string(), table.to_string(), schema.clone().unwrap_or_default());
        let fut = crate::ui::runtime::spawn(async move { crate::commands::drop_table(state, c, t, sc).await });
        let q = self.qualified_name(conn_id, table, schema.as_deref());
        let conn_id = conn_id.to_string();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            let task = this
                .update(cx, |this, cx| match res {
                    Ok(()) => {
                        this.status = format!("Dropped {q}").into();
                        cx.notify();
                        Some(this.refresh_schema(&conn_id, cx))
                    }
                    Err(e) => {
                        this.status = e.into();
                        cx.notify();
                        None
                    }
                })
                .ok()
                .flatten();
            if let Some(t) = task {
                t.await;
            }
        })
        .detach();
    }
}

fn chevron(s: Scale, open: bool) -> Div {
    div()
        .w(px(10.))
        .flex_shrink_0()
        .opacity(0.5)
        .text_size(s.fs(10.0))
        .line_height(s.lh(10.0))
        .child(if open { CHEVRON_OPEN } else { CHEVRON_CLOSED })
}

fn size_label(s: Scale, text: &str) -> Div {
    div()
        .flex_shrink_0()
        .t(s, 11.0)
        .font_weight(FontWeight::NORMAL)
        .text_color(theme::TEXT_MUTED)
        .opacity(0.72)
        .child(text.to_string())
}

fn nav_info(s: Scale, text: &str, color: Rgba) -> Div {
    div().px(px(12.)).py(px(6.)).t(s, 12.0).text_color(color).child(text.to_string())
}

/// `box-shadow: inset 0 ±2px 0 0 var(--accent)` drop indicator.
fn drop_line(after: bool) -> Div {
    let d = div().absolute().left_0().right_0().h(px(2.)).bg(theme::ACCENT);
    if after {
        d.bottom_0()
    } else {
        d.top_0()
    }
}


/// Wraps a navigator row so its bounds are recorded each frame; drag and drop
/// needs the row midpoint to decide between dropping above and below it.
fn record_row(row: AnyElement, key: String, cx: &mut Context<Workspace>) -> AnyElement {
    let ws = cx.weak_entity();
    div()
        .child(row)
        .on_children_prepainted(move |bounds, _w, cx| {
            if let Some(b) = bounds.first().copied() {
                ws.update(cx, |ws, _| {
                    ws.nav_row_bounds.insert(key.clone(), b);
                })
                .ok();
            }
        })
        .into_any_element()
}

impl Workspace {
    /// Vertical midpoint of a recorded navigator row (falls back to `y`, which
    /// keeps the drop marker before the row).
    pub fn nav_row_mid(&self, key: &str, y: Pixels) -> Pixels {
        self.nav_row_bounds.get(key).map(|b| b.top() + b.size.height / 2.).unwrap_or(y)
    }
}
