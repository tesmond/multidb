//! Connection navigator (`Navigator.svelte`): header menus, table filter,
//! server groups, connection tree, drag and drop, context menus.

use crate::models::Table;
use crate::ui::nav_index::{Matches, NONE};
use crate::ui::dialogs::{self, Btn};
use crate::ui::model::{display_structure, format_bytes, ActiveConnection};
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

/// Tables of one schema that survive the filter. With no filter that is all of
/// them; otherwise it is a lookup into the set `nav_index` produced off the UI
/// thread, rather than a walk over every name.
fn filter_tables<'a>(
    tables: &'a [Table],
    matches: Option<&Matches>,
    conn_id: &str,
    db: u32,
    schema: u32,
) -> Vec<&'a Table> {
    let Some(m) = matches else { return tables.iter().collect() };
    tables
        .iter()
        .enumerate()
        .filter(|(i, _)| m.table_matches(conn_id, (db, schema, *i as u32)))
        .map(|(_, t)| t)
        .collect()
}

fn conn_matches(conn: &ActiveConnection, matches: Option<&Matches>) -> bool {
    match matches {
        None => true,
        Some(m) => m.conn_matches(&conn.config.id),
    }
}

/// `navigatorSchemaGroups`, as positions into the connection's schema tree so
/// that rendering borrows it rather than cloning every table on every frame.
/// The `u32` is the database index, or [`NONE`] when the tree has none.
fn schema_groups(conn: &ActiveConnection) -> Vec<(String, u32)> {
    let Some(s) = &conn.schema else { return Vec::new() };
    if conn.config.driver == "postgres" {
        if !s.databases.is_empty() {
            return s.databases.iter().enumerate().map(|(i, d)| (d.name.clone(), i as u32)).collect();
        }
        let db = conn.config.database.trim();
        if !db.is_empty() && !s.schemas.is_empty() {
            return vec![(db.to_string(), NONE)];
        }
        return Vec::new();
    }
    if !s.schemas.is_empty() {
        vec![(String::new(), NONE)]
    } else {
        Vec::new()
    }
}

pub fn swatch_color(tab_color: &str) -> Option<Rgba> {
    dialogs::parse_hex(tab_color)
}

fn clamp_menu(pos: Point<Pixels>, kind: &str, window: &Window) -> Point<Pixels> {
    let pad = 8.0;
    let w = 220.0;
    let h = match kind {
        "database" => 285.0,
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
        cx.notify();
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
        let filter = self.filter_text(cx);
        let filtering = !filter.is_empty();
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.nav_filter.update(cx, |i, cx| i.set_text("", cx));
                            this.last_filter_load_key.clear();
                            cx.notify();
                        }))
                        .child("Clear"),
                )
            });

        // Tree. `filtering` means the filter box has text; `matches` is what
        // the background search last found for it.
        let matches = self.nav_matches.clone();
        let matches = matches.as_deref();
        let mut items: Vec<AnyElement> = Vec::new();
        let conns = self.connections.clone();
        let groups = self.settings.server_groups.clone();
        let structure = display_structure(&conns, &groups);
        for (g, gconns) in &structure.groups {
            let matching: Vec<&&ActiveConnection> = gconns.iter().filter(|c| conn_matches(c, matches)).collect();
            if !filtering || !matching.is_empty() {
                items.push(self.render_group_label(g.id.clone(), g.title.clone(), matching.len(), filtering, s, cx));
            }
            if self.expanded_groups.contains(&g.id) || filtering {
                for c in matching {
                    items.push(self.render_conn(c, Some(g.id.clone()), &filter, s, window, cx));
                }
            }
        }
        for c in &structure.ungrouped {
            if conn_matches(c, matches) {
                items.push(self.render_conn(c, None, &filter, s, window, cx));
            }
        }
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
            .on_mouse_move(cx.listener(|this, _e: &MouseMoveEvent, _, _cx| {
                // Hovering empty space: dropping there moves to the end.
                if let Some(d) = &mut this.nav_drag {
                    if d.active {
                        let _ = d;
                    }
                }
            }))
            // One child holding the rows. Its padding lives here rather than on
            // the scroll container so that it counts towards the content size.
            // The right padding keeps the rows clear of the vertical scrollbar,
            // which is drawn over the pane's edge rather than taking layout
            // space.
            .child(
                scroll::scroll_content(div())
                    .pt(px(4.))
                    .pb(px(40.))
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

    fn render_group_label(&mut self, id: String, title: String, count: usize, filtering: bool, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let open = filtering || self.expanded_groups.contains(&id);
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
            .flex()
            .items_center()
            .gap(px(4.))
            .px(px(8.))
            .py(px(5.))
            .t(s, 13.0)
            .text_color(theme::TEXT)
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

    fn render_conn(&mut self, conn: &ActiveConnection, group: Option<String>, filter: &str, s: Scale, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let id = conn.config.id.clone();
        let filtering = !filter.is_empty();
        let open = filtering || self.expanded.contains(&id);
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

        let label = record_row(label.into_any_element(), format!("conn:{id}"), cx);
        let mut node = div().flex().flex_col().child(label);
        if open {
            node = node.child(self.render_conn_children(conn, filter, s, window, cx));
        }
        node.into_any_element()
    }

    fn render_conn_children(&mut self, conn: &ActiveConnection, filter: &str, s: Scale, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let filtering = !filter.is_empty();
        let id = conn.config.id.clone();
        // Section margins collapsed in the old block layout: 2px between
        // sections and 2px after the last one.
        let mut children = div().flex().flex_col().pl(px(16.)).pb(px(2.));
        if conn.schema_loading {
            return children.child(nav_info(s, "Loading schema…", theme::TEXT_MUTED)).into_any_element();
        }
        if let Some(err) = &conn.schema_error {
            return children.child(nav_info(s, err, theme::ERROR)).into_any_element();
        }
        let Some(schema) = conn.schema.clone() else {
            return children.child(nav_info(s, "Click to load schema", theme::TEXT_MUTED)).into_any_element();
        };
        let matches = self.nav_matches.clone();
        let matches = matches.as_deref();
        let groups = schema_groups(conn);
        if !groups.is_empty() {
            for (dbname, db_idx) in groups {
                // Borrowed from the connection's Arc'd tree — no cloning.
                let schemas: &[crate::models::Schema] =
                    if db_idx == NONE { &schema.schemas } else { &schema.databases[db_idx as usize].schemas };
                let db_key = format!("{id}-database-{dbname}");
                let db_open = dbname.is_empty() || filtering || self.expanded_tables.contains(&db_key);
                if !dbname.is_empty() {
                    let k = db_key.clone();
                    children = children.child(
                        div().mt(px(2.)).ml(px(16.)).child(
                            self.section_label(format!("db-{db_key}"), s, filtering || self.expanded_tables.contains(&db_key), true, cx.listener(move |this, _, _, cx| this.toggle_key(&k, cx)))
                                .child(div().flex_shrink_0().child("🗄"))
                                .child(div().flex_shrink_0().child(dbname.clone())),
                        ),
                    );
                }
                if db_open {
                    let mut inner = div().flex().flex_col();
                    let nested = !dbname.is_empty();
                    if nested {
                        inner = inner.pl(px(16.));
                    }
                    for (sc_idx, sc) in schemas.iter().enumerate() {
                        let schema_key = format!("{id}-schema-{}", sc.name);
                        let tables_key = format!("{id}-{}-tables", sc.name);
                        let tables = filter_tables(&sc.tables, matches, &id, db_idx, sc_idx as u32);
                        if filtering && tables.is_empty() {
                            continue;
                        }
                        let sk = schema_key.clone();
                        let sopen = filtering || self.expanded_tables.contains(&schema_key);
                        let size = format_bytes(sc.size_bytes);
                        let mut section = div().mt(px(2.)).when(nested, |d| d.ml(px(16.))).when(!nested, |d| d.ml(px(16.))).child(
                            self.section_label(format!("s-{schema_key}"), s, sopen, true, cx.listener(move |this, _, _, cx| this.toggle_key(&sk, cx)))
                                .child(div().flex_shrink_0().child("🗂"))
                                .child(div().flex_shrink_0().child(sc.name.clone()))
                                .when(!size.is_empty(), |d| d.child(size_label(s, &size))),
                        );
                        if sopen {
                            let mut sch = div().flex().flex_col().pl(px(16.));
                            sch = sch.child(self.render_tables_section(&id, &tables_key, Some(sc.name.clone()), if dbname.is_empty() { None } else { Some(dbname.clone()) }, &tables, filtering, s, cx));
                            if !filtering && !sc.views.is_empty() {
                                sch = sch.child(self.render_leaf_section(&format!("{id}-{}-views", sc.name), "Views", "👁", sc.views.iter().map(|v| v.name.clone()).collect(), s, cx));
                            }
                            if !filtering && !sc.indexes.is_empty() {
                                sch = sch.child(self.render_leaf_section(&format!("{id}-{}-indexes", sc.name), "Indexes", "⚡", sc.indexes.clone(), s, cx));
                            }
                            section = section.child(sch);
                        }
                        inner = inner.child(section);
                    }
                    children = children.child(inner);
                }
            }
            return children.into_any_element();
        }
        // Flat (MySQL without schemas / SQLite)
        let tables_key = format!("{id}-tables");
        let tables = filter_tables(&schema.tables, matches, &id, NONE, NONE);
        if !filtering || !tables.is_empty() {
            children = children.child(self.render_tables_section(&id, &tables_key, None, None, &tables, filtering, s, cx));
        }
        if !filtering && !schema.views.is_empty() {
            children = children.child(self.render_leaf_section(&format!("{id}-views"), "Views", "👁", schema.views.iter().map(|v| v.name.clone()).collect(), s, cx));
        }
        if !filtering && !schema.indexes.is_empty() {
            children = children.child(self.render_leaf_section(&format!("{id}-indexes"), "Indexes", "⚡", schema.indexes.clone(), s, cx));
        }
        children.into_any_element()
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

    #[allow(clippy::too_many_arguments)]
    fn render_tables_section(
        &mut self,
        conn_id: &str,
        key: &str,
        schema: Option<String>,
        database: Option<String>,
        tables: &[&Table],
        filtering: bool,
        s: Scale,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let open = filtering || self.expanded_tables.contains(key);
        let k = key.to_string();
        let mut section = div().mt(px(2.)).ml(px(16.)).flex().flex_col().child(
            div()
                .id(SharedString::from(format!("sec-{key}")))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(3.))
                .t(s, 12.0)
                .text_color(theme::TEXT_MUTED)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|st| st.text_color(theme::TEXT))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(&k, cx)))
                .child(chevron(s, open))
                .child("Tables")
                .child(div().font_weight(FontWeight::NORMAL).opacity(0.7).child(format!("({})", tables.len()))),
        );
        if open {
            for t in tables {
                let tkey = match &schema {
                    Some(sc) => format!("{conn_id}-{sc}-t-{}", t.name),
                    None => format!("{conn_id}-t-{}", t.name),
                };
                let topen = self.expanded_tables.contains(&tkey);
                let size = format_bytes(t.size_bytes);
                let (tk, cid, tname, sc2, db2) = (tkey.clone(), conn_id.to_string(), t.name.clone(), schema.clone(), database.clone());
                section = section.child(
                    div()
                        .id(SharedString::from(format!("tbl-{tkey}")))
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .ml(px(16.))
                        .px(px(8.))
                        .py(px(3.))
                        .t(s, 12.0)
                        .text_color(theme::TEXT)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::BG_HOVER))
                        .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(&tk, cx)))
                        .on_mouse_down(MouseButton::Right, cx.listener(move |this, e: &MouseDownEvent, window, cx| {
                            cx.stop_propagation();
                            let pos = clamp_menu(e.position, "table", window);
                            this.nav_menu = Some(NavMenu::Table { pos, conn_id: cid.clone(), table: tname.clone(), schema: sc2.clone(), database: db2.clone() });
                            cx.notify();
                        }))
                        .child(chevron(s, topen))
                        .child(div().flex_shrink_0().child("📋"))
                        .child(div().flex_shrink_0().child(t.name.clone()))
                        .when(!size.is_empty(), |d| d.child(size_label(s, &size))),
                );
                if topen {
                    let mut cols = div().ml(px(16.)).pl(px(24.)).flex().flex_col();
                    for c in &t.columns {
                        cols = cols.child(
                            div()
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
                                .child(div().flex_shrink_0().opacity(0.6).italic().child(c.column_type.clone())),
                        );
                    }
                    section = section.child(cols);
                }
            }
        }
        section.into_any_element()
    }

    fn render_leaf_section(&mut self, key: &str, label: &'static str, icon: &'static str, names: Vec<String>, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let open = self.expanded_tables.contains(key);
        let k = key.to_string();
        let mut section = div().mt(px(2.)).ml(px(16.)).flex().flex_col().child(
            div()
                .id(SharedString::from(format!("sec-{key}")))
                .flex()
                .items_center()
                .gap(px(4.))
                .px(px(8.))
                .py(px(3.))
                .t(s, 12.0)
                .text_color(theme::TEXT_MUTED)
                .font_weight(FontWeight::MEDIUM)
                .cursor_pointer()
                .hover(|st| st.text_color(theme::TEXT))
                .on_click(cx.listener(move |this, _, _, cx| this.toggle_key(&k, cx)))
                .child(chevron(s, open))
                .child(label)
                .child(div().font_weight(FontWeight::NORMAL).opacity(0.7).child(format!("({})", names.len()))),
        );
        if open {
            for n in names {
                section = section.child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(4.))
                        .ml(px(16.))
                        .pl(px(24.))
                        .pr(px(8.))
                        .py(px(3.))
                        .t(s, 12.0)
                        .text_color(theme::TEXT)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::BG_HOVER))
                        .child(div().flex_shrink_0().child(icon))
                        .child(n),
                );
            }
        }
        section.into_any_element()
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
                    let ids: Vec<String> = (0..8).map(|_| conn_id.clone()).collect();
                    let [a, b, c, d, e, f, g, _h]: [String; 8] = ids.try_into().unwrap();
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
