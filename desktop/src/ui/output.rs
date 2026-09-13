//! Editor panel (toolbar + SQL editor), output panel (results grid,
//! messages, history, saved queries) and the status bar.

use crate::ui::grid::{self, Bar, CellSel, EditOverlay, GridBody, Jump, ScrollDrag};
use crate::ui::js;
use crate::ui::model::{OutputTab, QueryResult, SortDirection};
use crate::ui::render::*;
use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{shadow, Scale, TextExt};
use crate::ui::workspace::{OutputMenu, TabKind, Workspace};
use gpui::{
    deferred, div, point, prelude::*, px, AnyElement, Context, CursorStyle, FontWeight, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent, SharedString, Window,
};

impl Workspace {
    // ─── Editor panel ───────────────────────────────────────────────────────

    pub fn render_active_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let Some(tab) = self.active_tab() else { return div().into_any_element() };
        match &tab.kind {
            TabKind::Sql(_) => self.render_sql_panel(window, cx),
            TabKind::Diagram(_) => self.render_diagram(window, cx),
            TabKind::Sessions(_) => self.render_sessions(window, cx),
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

    // ─── Output panel ───────────────────────────────────────────────────────

    pub fn render_output_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let pending = self.active_tab().and_then(|t| t.sql()).map(|s| s.pending_edits.len()).unwrap_or(0);
        let mut tabs = div()
            .flex()
            .items_center()
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_PANEL)
            .flex_shrink_0();
        for (key, label) in OutputTab::ALL {
            let active = self.output_tab == key;
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
                        this.output_tab = key;
                        this.refresh_output_lists(cx);
                        cx.notify();
                    }))
                    .child(label),
            );
        }
        if self.output_tab == OutputTab::Results && pending > 0 {
            tabs = tabs.child(div().flex_1()).child(
                div()
                    .id("save-edits")
                    .mr(px(8.))
                    .px(px(12.))
                    .py(px(3.))
                    .bg(theme::rgba8(255, 200, 50, 0.15))
                    .border_1()
                    .border_color(theme::rgba8(255, 200, 50, 0.6))
                    .rounded(px(4.))
                    .text_color(theme::AMBER)
                    .t(s, 11.0)
                    .font_weight(FontWeight::SEMIBOLD)
                    .whitespace_nowrap()
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::rgba8(255, 200, 50, 0.25)).border_color(theme::rgba8(255, 200, 50, 0.9)))
                    .on_click(cx.listener(|this, _, _, cx| this.save_edits(cx)))
                    .child(format!("Save Changes ({pending})")),
            );
        }
        let content: AnyElement = match self.output_tab {
            OutputTab::Results => self.render_results(window, cx),
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
            .into_any_element()
    }

    fn render_messages(&mut self, s: Scale, cx: &mut Context<Self>) -> AnyElement {
        let result = self.active_tab().and_then(|t| t.sql()).and_then(|t| t.result.clone());
        let msg = match result {
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
                this.output_menu = Some(OutputMenu::History { pos: e.position });
                cx.notify();
            }));
        let mono = self.mono_family.clone();
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
                            .on_click(cx.listener(move |this, _, _, cx| this.use_query(query.clone(), conn.clone(), cx)))
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
                        this.output_menu = Some(OutputMenu::Saved { pos: e.position, id, title: title.clone() });
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
                            .on_click(cx.listener(move |this, _, _, cx| this.use_query(query.clone(), conn.clone(), cx)))
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

    pub fn render_output_overlays(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let menu = self.output_menu.clone()?;
        let s = Scale(self.scale());
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
                        this.output_menu = None;
                        this.clear_history(false, cx);
                    })))
                    .child(item("clear-all-hist", "Clear all history").on_click(cx.listener(|this, _, _, cx| {
                        this.output_menu = None;
                        this.clear_history(true, cx);
                    }))),
            ),
            OutputMenu::Saved { pos, id, title } => (
                pos,
                div()
                    .child(item("edit-title", "Edit title").on_click(cx.listener(move |this, _, window, cx| {
                        this.output_menu = None;
                        let d = crate::ui::dialogs::TitleDialog::new(
                            crate::ui::dialogs::TitleDialogKind::Edit { id, current: title.clone() },
                            &title,
                            this.scale(),
                            window,
                            cx,
                        );
                        this.title_dialog = Some(d);
                        cx.notify();
                    })))
                    .child(item("delete-saved", "Delete").on_click(cx.listener(move |this, _, _, cx| {
                        this.output_menu = None;
                        let state = crate::ui::runtime::state();
                        let fut = crate::ui::runtime::spawn(async move { crate::commands::delete_saved_query(state, id).await });
                        cx.spawn(async move |this, cx| {
                            let _ = fut.await;
                            this.update(cx, |this, cx| this.load_saved_queries(cx)).ok();
                        })
                        .detach();
                        cx.notify();
                    }))),
            ),
        };
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("ctx-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.output_menu = None;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .id("ctx-menu")
                        .occlude()
                        .absolute()
                        .left(pos.x)
                        .top(pos.y)
                        .min_w(px(180.))
                        .py(px(4.))
                        .bg(theme::BG_PANEL)
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(4.))
                        .shadow(vec![shadow(0.0, 4.0, 12.0, 0.0, theme::rgba8(0, 0, 0, 0.3))])
                        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                        .child(body),
                )
                .into_any_element(),
        )
    }

    fn clear_history(&mut self, all: bool, cx: &mut Context<Self>) {
        let conn = self.output_conn_id();
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

    // ─── Status bar ─────────────────────────────────────────────────────────

    pub fn render_status_bar(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let result = self.active_tab().and_then(|t| t.sql()).and_then(|t| t.result.clone());
        let mut bar = div()
            .flex()
            .items_center()
            .gap(px(6.))
            .h(px(24.))
            .px(px(12.))
            .bg(theme::BG_TOOLBAR)
            .border_t_1()
            .border_color(theme::BORDER)
            .t(s, 11.0)
            .text_color(theme::TEXT_MUTED)
            .flex_shrink_0()
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(self.status.clone()));
        if let Some(r) = result {
            let sep = || div().opacity(0.4).child("|");
            bar = bar
                .child(sep())
                .child(div().text_color(theme::TEXT_DIM).child(format!("{} rows", r.rows.len())))
                .child(sep())
                .child(div().text_color(theme::TEXT_DIM).child(format!("{}ms", r.duration)))
                .child(sep())
                .child(
                    div()
                        .id("export-btn")
                        .ml(px(4.))
                        .px(px(8.))
                        .py(px(2.))
                        .rounded(px(3.))
                        .bg(theme::ACCENT)
                        .text_color(theme::WHITE)
                        .t(s, 11.0)
                        .cursor_pointer()
                        .hover(|st| st.bg(theme::ACCENT_HOVER))
                        .on_click(cx.listener(|this, _, _, cx| this.export_csv(cx)))
                        .child("Export data"),
                );
        }
        bar.into_any_element()
    }

    // ─── Results grid ───────────────────────────────────────────────────────

    fn render_results(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let Some(tab) = self.active_tab() else { return div().into_any_element() };
        let tab_id = tab.id.clone();
        let Some(sql) = tab.sql() else {
            return empty_state(s, theme::TEXT_MUTED, "No results.".into());
        };
        let Some(result) = sql.result.clone() else {
            return empty_state(s, theme::TEXT_MUTED, "Run a query to see results here.".into());
        };
        if !result.error.is_empty() {
            return self.render_error_box(s, result.error.clone(), cx);
        }
        if result.columns.is_empty() {
            return empty_state(s, theme::TEXT_MUTED, format!("Query executed. {} row(s) affected in {}ms.", result.rows_affected, result.duration));
        }
        let sort = sql.sort_col.map(|c| (c, sql.sort_dir));
        let pending = sql.pending_edits.clone();
        let editable = sql.edit_info.as_ref().is_some_and(|e| !e.primary_key_cols.is_empty());
        if self.grid.tab_id != tab_id || self.grid.generation != result.generation {
            self.grid.on_result_changed(&tab_id, result.generation, &result.columns, true, s.0);
        }
        self.grid.font_scale = s.0;
        self.grid.sync_measurements(&result, cx);
        let total = result.rows.len();
        let rnw = grid::row_num_width(total, s.0);
        let sort_index = self.grid.sort_index(&result, sort);
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
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().text_ellipsis().child(name2))
                    .when_some(sorted, |d, dir| {
                        d.child(div().flex_shrink_0().opacity(0.7).t(s, 10.0).child(if dir == SortDirection::Asc { "▲" } else { "▼" }))
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
                                    if let Some(r) = this.active_result() {
                                        let cols = r.columns.clone();
                                        this.grid.auto_fit(i, &cols, cx);
                                    }
                                    cx.notify();
                                    return;
                                }
                                this.grid.did_resize = false;
                                let w = this.grid.col_widths.get(i).copied().unwrap_or(100.0);
                                this.grid.resizing = Some((i, e.position.x, w));
                            }))
                            .on_click(|_, _, cx| cx.stop_propagation()),
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

        let ws = cx.entity().downgrade();
        let body = GridBody {
            result: result.clone(),
            sort_index,
            col_widths: self.grid.col_widths.clone(),
            scale: s.0,
            scroll: (self.grid.scroll_x, self.grid.scroll_y),
            sel: self.grid.sel,
            selected_rows: self.grid.selected_rows.clone(),
            hovered_row: self.grid.hovered_row,
            active_bar: self.grid.scroll_drag.map(|d| d.bar).or(self.grid.hovered_bar),
            pending,
            editing: self.grid.edit.as_ref().map(|e| (e.row_data_idx, e.col)),
            on_bounds: Box::new(move |b, cx| {
                ws.update(cx, |ws, _| ws.grid.body_bounds = Some(b)).ok();
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
                this.grid.scroll_y -= f32::from(d.y);
                this.clamp_grid_scroll();
                if let Some(ed) = &mut this.grid.edit {
                    let _ = ed;
                }
                cx.notify();
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| {
                let Some(b) = this.grid.body_bounds else { return };
                let total = this.active_result().map(|r| r.rows.len()).unwrap_or(0);
                let local = point(e.position.x - b.left(), e.position.y - b.top());
                let rh = grid::row_height(this.grid.font_scale);
                let row = ((f32::from(local.y) + this.grid.scroll_y) / rh).floor();
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
                let p = this.grid_page();
                this.grid_move(-p, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &GridPageDown, _, cx| {
                let p = this.grid_page();
                this.grid_move(p, 0, false, cx)
            }))
            .on_action(cx.listener(|this, _: &GridExtendUp, _, cx| this.grid_move(-1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendDown, _, cx| this.grid_move(1, 0, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendLeft, _, cx| this.grid_move(0, -1, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendRight, _, cx| this.grid_move(0, 1, true, cx)))
            .on_action(cx.listener(|this, _: &GridExtendPageUp, _, cx| {
                let p = this.grid_page();
                this.grid_move(-p, 0, true, cx)
            }))
            .on_action(cx.listener(|this, _: &GridExtendPageDown, _, cx| {
                let p = this.grid_page();
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
            let tip_pos = self.header_cell_bottom_left(col).unwrap_or(pos);
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
                                gpui::Font { weight: FontWeight::SEMIBOLD, ..gpui::font(theme::UI_FONT) },
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
                gpui::anchored().position(edit.origin).child(div().w(px(edit.width)).h(px(rh)).child(edit.input.clone())),
            ).with_priority(3));
        }
        let _ = window;
        wrap.into_any_element()
    }

    fn header_cell_bottom_left(&self, col: usize) -> Option<Point<Pixels>> {
        let b = self.grid.body_bounds?;
        let total = self.active_result()?.rows.len();
        let rnw = grid::row_num_width(total, self.grid.font_scale);
        let left: f32 = self.grid.col_widths[..col].iter().sum::<f32>() + rnw - self.grid.scroll_x;
        Some(point(b.left() + px(left), b.top() + px(4.)))
    }

    pub fn active_result(&self) -> Option<QueryResult> {
        self.active_tab().and_then(|t| t.sql()).and_then(|s| s.result.clone())
    }

    fn clamp_grid_scroll(&mut self) {
        let total = self.active_result().map(|r| r.rows.len()).unwrap_or(0);
        let (w, h) = self.grid_view_size();
        self.grid.clamp_scroll(total, w, h);
    }

    fn grid_view_size(&self) -> (f32, f32) {
        if self.grid.body_bounds.is_none() {
            return (800.0, 300.0);
        }
        let total = self.active_result().map(|r| r.rows.len()).unwrap_or(0);
        let geom = self.grid.geom(total);
        (geom.view_w, geom.view_h)
    }

    fn grid_page(&self) -> isize {
        let (_, h) = self.grid_view_size();
        (h / grid::row_height(self.grid.font_scale)).floor().max(1.0) as isize
    }

    fn toggle_sort(&mut self, col: usize, cx: &mut Context<Self>) {
        if let Some(s) = self.active_tab_mut().and_then(|t| t.sql_mut()) {
            let dir = if s.sort_col == Some(col) {
                if s.sort_dir == SortDirection::Asc { SortDirection::Desc } else { SortDirection::Asc }
            } else {
                SortDirection::Asc
            };
            s.sort_col = Some(col);
            s.sort_dir = dir;
        }
        cx.notify();
    }

    fn grid_move(&mut self, dr: isize, dc: isize, extend: bool, cx: &mut Context<Self>) {
        let Some(r) = self.active_result() else { return };
        let view = self.grid_view_size();
        self.grid.move_selection(dr, dc, extend, r.rows.len(), r.columns.len(), view);
        cx.notify();
    }

    /// cmd/ctrl with an arrow, and Home/End: jump to the first or last row or
    /// column. With `extend` the anchor stays put, so shift-cmd-down selects
    /// everything from the cursor down to the last row.
    fn grid_jump(&mut self, row: Option<Jump>, col: Option<Jump>, extend: bool, cx: &mut Context<Self>) {
        let Some(r) = self.active_result() else { return };
        let (total, cols) = (r.rows.len(), r.columns.len());
        if total == 0 || cols == 0 {
            return;
        }
        let view = self.grid_view_size();
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
        let Some(r) = self.active_result() else { return };
        let sort = self.active_tab().and_then(|t| t.sql()).and_then(|s| s.sort_col.map(|c| (c, s.sort_dir)));
        if let Some(text) = self.grid.copy_text(&r, sort) {
            self.copy_to_clipboard(text, cx);
        }
    }

    fn grid_select_all(&mut self, cx: &mut Context<Self>) {
        let Some(r) = self.active_result() else { return };
        if (self.grid.sel.is_some() || !self.grid.selected_rows.is_empty()) && !r.rows.is_empty() && !r.columns.is_empty() {
            let total = r.rows.len();
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
        let Some(r) = self.active_result() else { return };
        if let Some(f) = &self.grid.focus {
            window.focus(f);
        }
        let total = r.rows.len();
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
        let anchor = if e.modifiers.shift { self.grid.last_selected.unwrap_or(hit) } else { hit };
        self.grid.selecting = true;
        self.grid.sel_anchor = Some(anchor);
        self.grid.sel = Some(CellSel { r0: anchor.0, c0: anchor.1, r1: hit.0, c1: hit.1 });
        self.grid.selected_rows.clear();
        self.grid.row_anchor = None;
        self.grid.last_selected = Some(hit);
        cx.notify();
    }

    /// A press on a scrollbar: grab the thumb, or page towards a click on the
    /// track, the way a native scrollbar does.
    fn on_scrollbar_press(&mut self, bar: Bar, local: Point<Pixels>, total: usize, cx: &mut Context<Self>) {
        let geom = self.grid.geom(total);
        let pos = geom.pos_on(bar, local);
        let (start, len, scroll, page) = match bar {
            Bar::Vertical => {
                let (s, l) = geom.v_thumb(self.grid.scroll_y);
                (s, l, self.grid.scroll_y, geom.view_h)
            }
            Bar::Horizontal => {
                let (s, l) = geom.h_thumb(self.grid.scroll_x);
                (s, l, self.grid.scroll_x, geom.view_w)
            }
        };
        if pos >= start && pos < start + len {
            self.grid.scroll_drag = Some(ScrollDrag { bar, grab: pos - start });
        } else {
            // Track click: one page towards the pointer.
            let delta = if pos < start { -page } else { page };
            match bar {
                Bar::Vertical => self.grid.scroll_y = scroll + delta,
                Bar::Horizontal => self.grid.scroll_x = scroll + delta,
            }
            self.clamp_grid_scroll();
        }
        self.grid.hovered_bar = Some(bar);
        cx.notify();
    }

    fn on_grid_double_click(&mut self, local: Point<Pixels>, editable: bool, r: &QueryResult, window: &mut Window, cx: &mut Context<Self>) {
        let total = r.rows.len();
        let Some(hit) = self.grid.cell_at(local, total) else { return };
        let sort = self.active_tab().and_then(|t| t.sql()).and_then(|s| s.sort_col.map(|c| (c, s.sort_dir)));
        let idx = self.grid.sort_index(r, sort);
        let data_idx = idx.as_ref().and_then(|i| i.get(hit.0).copied()).unwrap_or(hit.0);
        let Some(row) = r.rows.get(data_idx) else { return };
        if editable {
            let pending = self.active_tab().and_then(|t| t.sql()).map(|s| s.pending_edits.clone()).unwrap_or_default();
            let current = crate::ui::model::pending_get(&pending, data_idx, hit.1).cloned().unwrap_or_else(|| match &row[hit.1] {
                serde_json::Value::Null => String::new(),
                v => js::value_to_string(v),
            });
            let s = self.grid.font_scale;
            let rh = grid::row_height(s);
            let rnw = grid::row_num_width(total, s);
            let b = self.grid.body_bounds.unwrap();
            let x: f32 = rnw - self.grid.scroll_x + self.grid.col_widths[..hit.1].iter().sum::<f32>();
            let y: f32 = hit.0 as f32 * rh - self.grid.scroll_y;
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
            let input = cx.new(|cx| {
                let mut t = TextInput::new(cx, look);
                t.set_text(current, cx);
                t
            });
            cx.subscribe_in(&input, window, |this, _i, ev: &InputEvent, _w, cx| match ev {
                InputEvent::Submit | InputEvent::Blur => this.commit_cell_edit(cx),
                InputEvent::Cancel => {
                    this.grid.edit = None;
                    cx.notify();
                }
                _ => {}
            })
            .detach();
            input.update(cx, |i, _| i.focus(window));
            self.grid.edit = Some(EditOverlay {
                row: hit.0,
                col: hit.1,
                row_data_idx: data_idx,
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
        let text = crate::ui::textfmt::format_value_for_clipboard(&row[hit.1], ty, "NULL");
        self.copy_to_clipboard(text, cx);
        self.grid.sel = Some(CellSel { r0: hit.0, c0: hit.1, r1: hit.0, c1: hit.1 });
        self.grid.last_selected = Some(hit);
        cx.notify();
    }

    fn commit_cell_edit(&mut self, cx: &mut Context<Self>) {
        let Some(edit) = self.grid.edit.take() else { return };
        let value = edit.input.read(cx).text().to_string();
        let Some(r) = self.active_result() else { return };
        let original = r.rows.get(edit.row_data_idx).and_then(|row| row.get(edit.col)).map(|v| match v {
            serde_json::Value::Null => String::new(),
            v => js::value_to_string(v),
        });
        if let Some(s) = self.active_tab_mut().and_then(|t| t.sql_mut()) {
            let row_pos = s.pending_edits.iter().position(|(r, _)| *r == edit.row_data_idx);
            if Some(&value) == original.as_ref() {
                if let Some(p) = row_pos {
                    s.pending_edits[p].1.retain(|(c, _)| *c != edit.col);
                    if s.pending_edits[p].1.is_empty() {
                        s.pending_edits.remove(p);
                    }
                }
            } else {
                match row_pos {
                    Some(p) => {
                        let cols = &mut s.pending_edits[p].1;
                        match cols.iter_mut().find(|(c, _)| *c == edit.col) {
                            Some(e) => e.1 = value,
                            None => cols.push((edit.col, value)),
                        }
                    }
                    None => s.pending_edits.push((edit.row_data_idx, vec![(edit.col, value)])),
                }
            }
        }
        cx.notify();
    }

    pub fn on_grid_drag_move(&mut self, e: &MouseMoveEvent, _w: &mut Window, cx: &mut Context<Self>) {
        if let Some(drag) = self.grid.scroll_drag {
            let Some(b) = self.grid.body_bounds else { return };
            let total = self.active_result().map(|r| r.rows.len()).unwrap_or(0);
            let geom = self.grid.geom(total);
            let local = point(e.position.x - b.left(), e.position.y - b.top());
            let pos = geom.pos_on(drag.bar, local) - drag.grab;
            match drag.bar {
                Bar::Vertical => self.grid.scroll_y = geom.scroll_for_v_thumb(pos),
                Bar::Horizontal => self.grid.scroll_x = geom.scroll_for_h_thumb(pos),
            }
            self.clamp_grid_scroll();
            cx.notify();
            return;
        }
        // Hover state for the thumb (`::-webkit-scrollbar-thumb:hover`).
        let hovered = self.grid.body_bounds.and_then(|b| {
            let total = self.active_result().map(|r| r.rows.len()).unwrap_or(0);
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
            if let Some(w) = self.grid.col_widths.get_mut(idx) {
                *w = (start_w + delta).max(50.0);
            }
            cx.notify();
            return;
        }
        if !self.grid.selecting || !e.dragging() {
            return;
        }
        let Some(b) = self.grid.body_bounds else { return };
        let Some(r) = self.active_result() else { return };
        let total = r.rows.len();
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
            self.grid.scroll_y += vy;
            self.clamp_grid_scroll();
        }
        if let (Some(hit), Some(anchor)) = (self.grid.cell_at(local, total), self.grid.sel_anchor) {
            self.grid.sel = Some(CellSel { r0: anchor.0, c0: anchor.1, r1: hit.0, c1: hit.1 });
        }
        cx.notify();
    }

    pub fn on_grid_drag_end(&mut self, _e: &MouseUpEvent, _w: &mut Window, cx: &mut Context<Self>) {
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
