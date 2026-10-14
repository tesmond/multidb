//! Editor panel: the toolbar (connection selector, run/stop/save) and the SQL
//! editor, plus the switch that shows whichever panel the active tab needs.

use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::{shadow, Scale, TextExt};
use crate::ui::icons::{icon, Icon};
use crate::ui::model::TabId;
use crate::ui::workspace::{TabKind, Workspace};
use gpui::{
    deferred, div, point, prelude::*, px, AnyElement, Context, CursorStyle, FontWeight, MouseButton, MouseMoveEvent,
    SharedString, Window,
};

/// What the empty choice of the default-schema dropdown reads.
const NO_SCHEMA: &str = "— no default schema —";

/// Which toolbar dropdown `render_toolbar_select` draws.
struct ToolbarSelect {
    id: &'static str,
    /// Its open state: the tab it is open on and the highlighted option.
    state: fn(&mut Workspace) -> &mut Option<(TabId, usize)>,
    /// The other toolbar dropdown's, closed when this one opens.
    other: fn(&mut Workspace) -> &mut Option<(TabId, usize)>,
    min_w: f32,
}

fn conn_select_state(ws: &mut Workspace) -> &mut Option<(TabId, usize)> {
    &mut ws.conn_select_open
}

fn schema_select_state(ws: &mut Workspace) -> &mut Option<(TabId, usize)> {
    &mut ws.schema_select_open
}

impl Workspace {
    // ─── Editor panel ───────────────────────────────────────────────────────

    pub fn render_active_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(tab) = self.active_tab() else { return div().into_any_element() };
        match &tab.kind {
            TabKind::Sql(_) => self.render_sql_panel(window, cx),
            TabKind::Diagram(_) => self.render_diagram(window, cx),
            TabKind::Sessions(_) => self.render_sessions(window, cx),
            TabKind::Storage(_) => self.render_storage(window, cx),
        }
    }

    fn render_sql_panel(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let tab = self.active_tab().unwrap();
        let tab_id = tab.id.clone();
        let sql = tab.sql().unwrap();
        let running = sql.running;
        let editor = sql.editor.clone();
        let empty = editor.read(cx).text().trim().is_empty();
        let conn_id = tab.conn_id.clone();
        let mut options = vec![(String::new(), "— select connection —".to_string())];
        options.extend(crate::ui::model::ordered_connection_options(&self.connections, &self.settings.server_groups));
        let open = self.conn_select_open.as_ref().filter(|(t, _)| *t == tab_id).map(|(_, a)| *a);
        let label = options.iter().find(|(v, _)| *v == conn_id).map(|(_, l)| l.clone()).unwrap_or_else(|| "— select connection —".into());
        let has_selection = options.iter().any(|(v, _)| *v == conn_id);

        let btn = |id: &'static str, glyph: Icon, label: &'static str, bg: Rgba, border: Rgba| {
            div()
                .id(id)
                .px(px(14.))
                .py(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_color(border)
                .bg(bg)
                .text_color(theme::WHITE)
                .t(s, 12.0)
                .font_weight(FontWeight::MEDIUM)
                .flex_shrink_0()
                .flex()
                .items_center()
                .gap(px(6.))
                .child(icon(glyph, s.fs(12.0), theme::WHITE))
                .child(label)
        };
        let t1 = tab_id.clone();
        let t2 = tab_id.clone();
        let t3 = tab_id.clone();
        let t4 = tab_id.clone();
        let mut toolbar = div()
            .flex()
            .items_center()
            .gap(px(8.))
            .px(px(10.))
            .py(px(6.))
            .bg(theme::BG_PANEL)
            .border_b_1()
            .border_color(theme::BORDER)
            .flex_shrink_0()
            .child(self.render_conn_select(&tab_id, &label, has_selection, open, options, s, cx));
        if let Some(schema_select) = self.render_schema_select(&tab_id, s, cx) {
            toolbar = toolbar.child(schema_select);
        }
        if running {
            toolbar = toolbar.child(
                btn("btn-stop", Icon::Stop, "Stop", theme::ERROR, theme::ERROR)
                    .cursor_pointer()
                    .hover(|st| st.opacity(0.85))
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_query(&t1, cx))),
            );
        } else {
            toolbar = toolbar
                .child(
                    btn("btn-run", Icon::Run, "Run", theme::ACCENT, theme::ACCENT)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::ACCENT_HOVER))
                        .on_click(cx.listener(move |this, _, _, cx| this.run_query(&t2, cx))),
                )
                .child({
                    let b = btn("btn-save", Icon::Save, "Save", theme::SAVE_BLUE, theme::TRANSPARENT);
                    if empty {
                        b.opacity(0.5).cursor(CursorStyle::OperationNotAllowed)
                    } else {
                        b.cursor_pointer()
                            .hover(|st| st.opacity(0.9))
                            .on_click(cx.listener(move |this, _, window, cx| this.open_save_query(&t3, window, cx)))
                    }
                });
        }
        let _ = t4;
        div()
            .flex()
            .flex_col()
            .size_full()
            .child(toolbar)
            .child(div().flex_1().min_h(px(0.)).overflow_hidden().child(editor))
            .into_any_element()
    }

    #[allow(clippy::too_many_arguments)]
    fn render_conn_select(
        &mut self,
        tab_id: &str,
        label: &str,
        has_selection: bool,
        open: Option<usize>,
        options: Vec<(String, String)>,
        s: Scale,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let _ = has_selection;
        let current = self.tab(tab_id).map(|t| t.conn_id.clone()).unwrap_or_default();
        self.render_toolbar_select(
            ToolbarSelect { id: "conn-select", state: conn_select_state, other: schema_select_state, min_w: 180.0 },
            tab_id,
            label,
            &current,
            open,
            options,
            |this, tab_id, value, cx| this.set_tab_conn(tab_id, value, cx),
            s,
            cx,
        )
    }

    /// The default-schema dropdown: the schema unqualified table names in the
    /// tab's SQL resolve to, or none.
    fn render_schema_select(&mut self, tab_id: &str, s: Scale, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab = self.tab(tab_id)?;
        let sql = tab.sql()?;
        let conn = self.connection(&tab.conn_id)?;
        if conn.config.driver == "sqlite" {
            return None;
        }
        let current = sql.default_schema.clone();
        let mut names = self.schema_names(&tab.conn_id, &sql.database_name);
        // Keep a selection the loaded schema does not list (yet) visible.
        if !current.is_empty() && !names.contains(&current) {
            names.insert(0, current.clone());
        }
        let mut options = vec![(String::new(), NO_SCHEMA.to_string())];
        options.extend(names.into_iter().map(|n| (n.clone(), n)));
        let label = if current.is_empty() { NO_SCHEMA.to_string() } else { current.clone() };
        let open = self.schema_select_open.as_ref().filter(|(t, _)| *t == tab_id).map(|(_, a)| *a);
        Some(self.render_toolbar_select(
            ToolbarSelect { id: "schema-select", state: schema_select_state, other: conn_select_state, min_w: 150.0 },
            tab_id,
            &label,
            &current,
            open,
            options,
            |this, tab_id, value, cx| this.set_tab_default_schema(tab_id, value, cx),
            s,
            cx,
        ))
    }

    /// A toolbar dropdown: a button showing `label` and, while open, a menu of
    /// `options` (value, text) with `current` ticked.
    #[allow(clippy::too_many_arguments)]
    fn render_toolbar_select(
        &mut self,
        sel: ToolbarSelect,
        tab_id: &str,
        label: &str,
        current: &str,
        open: Option<usize>,
        options: Vec<(String, String)>,
        on_pick: fn(&mut Workspace, &str, String, &mut Context<Workspace>),
        s: Scale,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let ToolbarSelect { id, state, other, min_w } = sel;
        let tid = tab_id.to_string();
        let current_index = options.iter().position(|(v, _)| v == current).unwrap_or(0);
        let n_options = options.len();
        let mut root = div().relative().flex().min_w(px(min_w)).flex_shrink_0().child(
            div()
                .id(id)
                .w_full()
                .flex()
                .items_center()
                .justify_between()
                .gap(px(12.))
                .px(px(10.))
                .py(px(5.))
                .rounded(px(4.))
                .border_1()
                .border_color(if open.is_some() { theme::ACCENT } else { theme::BORDER })
                .bg(theme::BG_INPUT)
                .text_color(theme::TEXT)
                .t(s, 12.0)
                .cursor_pointer()
                .hover(|st| st.border_color(theme::ACCENT))
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener(move |this, _, _, cx| {
                    *other(this) = None;
                    if state(this).is_some() {
                        *state(this) = None;
                    } else if n_options > 0 {
                        *state(this) = Some((tid.clone(), current_index));
                        // The schema list comes from the connection's schema tree.
                        if id == "schema-select" {
                            let conn_id = this.tab(&tid).map(|t| t.conn_id.clone()).unwrap_or_default();
                            if this.connection(&conn_id).is_some_and(|c| c.schema.is_none()) {
                                this.ensure_schema(&conn_id, cx);
                            }
                        }
                    }
                    cx.notify();
                }))
                .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.to_string()))
                .child(div().flex_shrink_0().opacity(0.8).child(icon(Icon::ChevronDown, s.fs(12.0), theme::TEXT))),
        );
        if let Some(active) = open {
            let mut menu = div()
                .id(SharedString::from(format!("{id}-menu")))
                .occlude()
                .absolute()
                .left_0()
                .top(px(5.0 + 5.0 + 2.0 + s.lh_f(12.0) + 4.0))
                .min_w_full()
                .p(px(4.))
                .border_1()
                .border_color(theme::BORDER)
                .rounded(px(8.))
                .bg(theme::BG_PANEL)
                .shadow(vec![shadow(0.0, 14.0, 40.0, 0.0, theme::rgba8(0, 0, 0, 0.32))])
                .max_h(px(260.))
                .overflow_y_scroll()
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation());
            for (i, (value, text)) in options.into_iter().enumerate() {
                let selected = value == current;
                let tid = tab_id.to_string();
                let v = value.clone();
                menu = menu.child(
                    div()
                        .id(SharedString::from(format!("{id}-opt-{i}")))
                        .w_full()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(12.))
                        .px(px(10.))
                        .py(px(8.))
                        .rounded(px(6.))
                        .t(s, 12.0)
                        .text_color(theme::TEXT)
                        .cursor_pointer()
                        .when(i == active, |d| d.bg(theme::BG_HOVER))
                        .when(selected, |d| {
                            d.bg(theme::rgba8(255, 255, 255, 0.08))
                                .shadow(vec![gpui::BoxShadow { color: theme::hsla(theme::ACCENT), offset: point(px(0.), px(0.)), blur_radius: px(0.), spread_radius: px(1.) }])
                        })
                        .on_mouse_move(cx.listener(move |this, _: &MouseMoveEvent, _, cx| {
                            if let Some((_, a)) = state(this) {
                                if *a != i {
                                    *a = i;
                                    cx.notify();
                                }
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            *state(this) = None;
                            on_pick(this, &tid, v.clone(), cx);
                            cx.notify();
                        }))
                        .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(text))
                        .when(selected, |d| d.child(icon(Icon::Check, s.fs(12.0), theme::ACCENT))),
                );
            }
            root = root
                .child(
                    deferred(
                        div()
                            .id(SharedString::from(format!("{id}-backdrop")))
                            .absolute()
                            .top(px(-2000.))
                            .left(px(-2000.))
                            .w(px(8000.))
                            .h(px(8000.))
                            .on_mouse_down(MouseButton::Left, cx.listener(move |this, _, _, cx| {
                                *state(this) = None;
                                cx.notify();
                            })),
                    )
                    .with_priority(3),
                )
                .child(deferred(menu).with_priority(4));
        }
        root.into_any_element()
    }
}
