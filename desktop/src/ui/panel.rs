//! Editor panel: the toolbar (connection selector, run/stop/save) and the SQL
//! editor, plus the switch that shows whichever panel the active tab needs.

use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::{shadow, Scale, TextExt};
use crate::ui::workspace::{TabKind, Workspace};
use gpui::{
    deferred, div, point, prelude::*, px, AnyElement, Context, CursorStyle, FontWeight, MouseButton, MouseMoveEvent,
    SharedString, Window,
};

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

        let btn = |id: &'static str, label: &'static str, bg: Rgba, border: Rgba| {
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
        if running {
            toolbar = toolbar.child(
                btn("btn-stop", "⏹ Stop", theme::ERROR, theme::ERROR)
                    .cursor_pointer()
                    .hover(|st| st.opacity(0.85))
                    .on_click(cx.listener(move |this, _, _, cx| this.cancel_query(&t1, cx))),
            );
        } else {
            toolbar = toolbar
                .child(
                    btn("btn-run", "▶ Run", theme::ACCENT, theme::ACCENT)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::ACCENT_HOVER))
                        .on_click(cx.listener(move |this, _, _, cx| this.run_query(&t2, cx))),
                )
                .child({
                    let b = btn("btn-save", "💾 Save", theme::SAVE_BLUE, theme::TRANSPARENT);
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
        let tid = tab_id.to_string();
        let current = self.tab(tab_id).map(|t| t.conn_id.clone()).unwrap_or_default();
        let current_index = options.iter().position(|(v, _)| *v == current).unwrap_or(0);
        let n_options = options.len();
        let mut root = div().relative().flex().min_w(px(180.)).flex_shrink_0().child(
            div()
                .id("conn-select")
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
                    if this.conn_select_open.is_some() {
                        this.conn_select_open = None;
                    } else if n_options > 0 {
                        this.conn_select_open = Some((tid.clone(), current_index));
                    }
                    cx.notify();
                }))
                .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(label.to_string()))
                .child(div().flex_shrink_0().opacity(0.8).text_size(s.fs(14.0)).line_height(s.fs(14.0)).child("▾")),
        );
        if let Some(active) = open {
            let mut menu = div()
                .id("conn-select-menu")
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
                        .id(SharedString::from(format!("opt-{i}")))
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
                            if let Some((_, a)) = &mut this.conn_select_open {
                                if *a != i {
                                    *a = i;
                                    cx.notify();
                                }
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.conn_select_open = None;
                            this.set_tab_conn(&tid, v.clone(), cx);
                        }))
                        .child(div().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(text))
                        .when(selected, |d| d.child(div().flex_shrink_0().text_color(theme::ACCENT).t(s, 11.0).child("✓"))),
                );
            }
            root = root
                .child(
                    deferred(
                        div()
                            .id("conn-select-backdrop")
                            .absolute()
                            .top(px(-2000.))
                            .left(px(-2000.))
                            .w(px(8000.))
                            .h(px(8000.))
                            .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                                this.conn_select_open = None;
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
