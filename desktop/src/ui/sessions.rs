//! Database connections tab (`DatabaseConnections.svelte`).

use crate::commands;
use crate::models::DatabaseConnection;
use crate::ui::dialogs;
use crate::ui::runtime;
use crate::ui::theme;
use crate::ui::widgets::{overlay, shadow, TextExt};
use crate::ui::workspace::{Tab, TabKind, Workspace};
use gpui::{div, prelude::*, px, AnyElement, Context, FontWeight, SharedString, Task, Window};
use std::time::Duration;

pub struct SessionsState {
    pub rows: Vec<DatabaseConnection>,
    pub loading: bool,
    pub error: String,
    pub last_updated: String,
    pub terminating: String,
    _poll: Option<Task<()>>,
}

fn local_time_string() -> String {
    chrono::Local::now().format("%H:%M:%S").to_string()
}

impl Workspace {
    pub fn show_db_connections(&mut self, conn_id: &str, _window: &mut Window, cx: &mut Context<Self>) {
        let Some(conn) = self.connection(conn_id) else {
            self.status = "Connection not found.".into();
            cx.notify();
            return;
        };
        let name = conn.config.name.clone();
        self.selected_conn_id = conn_id.to_string();
        if let Some(t) = self.tabs.iter().find(|t| t.conn_id == conn_id && matches!(t.kind, TabKind::Sessions(_))) {
            let id = t.id.clone();
            self.set_active_tab(id, cx);
            return;
        }
        let id = crate::ui::model::new_id();
        let tab_id = id.clone();
        let poll = cx.spawn(async move |this, cx| loop {
            if this.update(cx, |this, cx| this.refresh_sessions(&tab_id, cx)).is_err() {
                break;
            }
            cx.background_executor().timer(Duration::from_secs(10)).await;
        });
        self.tabs.push(Tab {
            id: id.clone(),
            title: format!("{name} Connections"),
            conn_id: conn_id.to_string(),
            manually_renamed: false,
            kind: TabKind::Sessions(SessionsState {
                rows: Vec::new(),
                loading: false,
                error: String::new(),
                last_updated: String::new(),
                terminating: String::new(),
                _poll: Some(poll),
            }),
        });
        self.set_active_tab(id, cx);
    }

    pub fn refresh_sessions(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(tab) = self.tab(tab_id) else { return };
        let conn_id = tab.conn_id.clone();
        let unsupported = self.connection(&conn_id).is_some_and(|c| c.config.driver == "sqlite");
        let Some(TabKind::Sessions(st)) = self.tab_mut(tab_id).map(|t| &mut t.kind) else { return };
        if conn_id.is_empty() || unsupported {
            st.rows.clear();
            return;
        }
        st.loading = true;
        st.error.clear();
        cx.notify();
        let state = runtime::state();
        let fut = runtime::spawn(async move { commands::list_database_connections(state, conn_id).await });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                let mut status = None;
                if let Some(TabKind::Sessions(st)) = this.tab_mut(&tab_id).map(|t| &mut t.kind) {
                    match res {
                        Ok(rows) => {
                            st.rows = rows;
                            st.last_updated = local_time_string();
                        }
                        Err(e) => {
                            st.error = e.clone();
                            status = Some(format!("Connection list error: {e}"));
                        }
                    }
                    st.loading = false;
                }
                if let Some(s) = status {
                    this.status = s.into();
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn terminate_session(&mut self, tab_id: &str, row: DatabaseConnection, cx: &mut Context<Self>) {
        let Some(conn_id) = self.tab(tab_id).map(|t| t.conn_id.clone()) else { return };
        if let Some(TabKind::Sessions(st)) = self.tab_mut(tab_id).map(|t| &mut t.kind) {
            st.terminating = row.id.clone();
        }
        let state = runtime::state();
        let rid = row.id.clone();
        let fut = runtime::spawn(async move { commands::terminate_database_connection(state, conn_id, rid).await });
        let tab_id = tab_id.to_string();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                match &res {
                    Ok(()) => this.status = format!("Terminated database connection {}", row.id).into(),
                    Err(e) => {
                        this.status = format!("Terminate connection error: {e}").into();
                        if let Some(TabKind::Sessions(st)) = this.tab_mut(&tab_id).map(|t| &mut t.kind) {
                            st.error = e.clone();
                        }
                    }
                }
                if let Some(TabKind::Sessions(st)) = this.tab_mut(&tab_id).map(|t| &mut t.kind) {
                    st.terminating.clear();
                }
                this.refresh_sessions(&tab_id, cx);
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn render_sessions(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = crate::ui::widgets::Scale(self.scale());
        let tab_id = self.active_tab_id.clone();
        let tab = self.active_tab().unwrap();
        let conn = self.connection(&tab.conn_id).cloned();
        let unsupported = conn.as_ref().is_some_and(|c| c.config.driver == "sqlite");
        let TabKind::Sessions(st) = &tab.kind else { return div().into_any_element() };
        let (loading, error, updated, rows, terminating) = (st.loading, st.error.clone(), st.last_updated.clone(), st.rows.clone(), st.terminating.clone());
        let btn = |id: SharedString, label: String, disabled: bool| {
            div()
                .id(id)
                .border_1()
                .border_color(theme::BORDER)
                .rounded(px(4.))
                .bg(theme::BG_INPUT)
                .text_color(theme::TEXT)
                .t(s, 12.0)
                .px(px(10.))
                .py(px(5.))
                .when(disabled, |d| d.opacity(0.5))
                .when(!disabled, |d| d.cursor_pointer().hover(|st| st.border_color(theme::ACCENT)))
                .child(label)
        };
        let t1 = tab_id.clone();
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_TOOLBAR)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .min_w(px(0.))
                    .child(div().t(s, 14.0).font_weight(FontWeight::SEMIBOLD).child("Database Connections"))
                    .child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().child(conn.as_ref().map(|c| c.config.name.clone()).unwrap_or_else(|| "Unknown connection".into()))),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .flex_shrink_0()
                    .when(!updated.is_empty(), |d| d.child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().child(format!("Updated {updated}"))))
                    .child({
                        let b = btn("sess-refresh".into(), if loading { "Refreshing...".into() } else { "Refresh".into() }, loading || unsupported);
                        if loading || unsupported { b } else { b.on_click(cx.listener(move |this, _, _, cx| this.refresh_sessions(&t1, cx))) }
                    }),
            );
        let mut view = div().flex().flex_col().size_full().min_w(px(0.)).bg(theme::BG_EDITOR).text_color(theme::TEXT).child(toolbar);
        if unsupported {
            view = view.child(div().p(px(14.)).text_color(theme::TEXT_MUTED).child(
                "SQLite uses local file handles rather than server-side sessions, so there are no database connections to manage.",
            ));
            return view.into_any_element();
        }
        if !error.is_empty() {
            view = view.child(div().p(px(14.)).text_color(theme::ERROR).border_b_1().border_color(theme::BORDER).child(error));
        }
        let headers = ["ID", "User", "Database", "Client", "State", "Opened", "Last Active", "Most Recent Command", "Action"];
        let cell = |text: String, header: bool, action: bool| {
            let d = div()
                .px(px(8.))
                .py(px(7.))
                .border_b_1()
                .border_color(theme::BORDER_SUBTLE)
                .overflow_hidden()
                .whitespace_nowrap()
                .text_ellipsis();
            let d = if action { d.w(px(108.)).flex_shrink_0().flex().justify_end() } else { d.flex_1().min_w(px(0.)) };
            if header {
                d.bg(theme::BG_SURFACE).text_color(theme::TEXT_MUTED).font_weight(FontWeight::SEMIBOLD).child(text)
            } else {
                d.child(text)
            }
        };
        let mut table = div().id("sess-table").flex_1().min_h(px(0.)).overflow_y_scroll().t(s, 12.0);
        let mut head = div().flex();
        for (i, h) in headers.iter().enumerate() {
            head = head.child(cell(h.to_string(), true, i == 8).when(i == 8, |d| d.flex().justify_end()));
        }
        table = table.child(head);
        if rows.is_empty() && !loading {
            table = table.child(div().p(px(14.)).text_color(theme::TEXT_MUTED).border_b_1().border_color(theme::BORDER_SUBTLE).child("No active database connections found."));
        } else {
            for (i, r) in rows.iter().enumerate() {
                let or_unknown = |v: &str, d: &str| if v.is_empty() { d.to_string() } else { v.to_string() };
                let cmd = if r.most_recent_command.trim().is_empty() { "No recent command".to_string() } else { r.most_recent_command.trim().to_string() };
                let row = r.clone();
                let t2 = tab_id.clone();
                let disabled = !r.can_terminate || terminating == r.id;
                let term = div()
                    .id(SharedString::from(format!("term-{}", r.id)))
                    .border_1()
                    .border_color(theme::BORDER)
                    .rounded(px(4.))
                    .bg(theme::BG_INPUT)
                    .text_color(theme::ERROR)
                    .px(px(10.))
                    .py(px(5.))
                    .child(if terminating == r.id { "Closing..." } else { "Terminate" })
                    .when(disabled, |d| d.opacity(0.5))
                    .when(!disabled, |d| {
                        d.cursor_pointer()
                            .hover(|st| st.bg(theme::rgba8(248, 113, 113, 0.1)).border_color(theme::ERROR))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.terminate_confirm = Some((t2.clone(), row.clone()));
                                cx.notify();
                            }))
                    });
                table = table.child(
                    div()
                        .flex()
                        .when(i % 2 == 1, |d| d.bg(theme::BG_ROW_ALT))
                        .child(cell(r.id.clone(), false, false).font_family("Menlo"))
                        .child(cell(or_unknown(&r.user, "Unknown"), false, false))
                        .child(cell(or_unknown(&r.database, "Unknown"), false, false))
                        .child(cell(or_unknown(&r.client, "Local"), false, false))
                        .child(cell(or_unknown(&r.state, "Unknown"), false, false))
                        .child(cell(or_unknown(&r.opened_at, "Unknown"), false, false))
                        .child(cell(or_unknown(&r.last_active_at, "Unknown"), false, false))
                        .child(cell(cmd, false, false))
                        .child(div().w(px(108.)).flex_shrink_0().px(px(8.)).py(px(7.)).border_b_1().border_color(theme::BORDER_SUBTLE).flex().justify_end().child(term)),
                );
            }
        }
        view.child(table).into_any_element()
    }

    pub fn render_terminate_confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (tab_id, row) = self.terminate_confirm.clone()?;
        let s = crate::ui::widgets::Scale(self.scale());
        let user = if row.user.is_empty() { "unknown user".to_string() } else { row.user.clone() };
        let r2 = row.clone();
        Some(
            overlay(0.6)
                .id("term-overlay")
                .occlude()
                .child(
                    div()
                        .w(px(420.))
                        .bg(theme::BG_SURFACE)
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(8.))
                        .shadow(vec![shadow(0.0, 8.0, 32.0, 0.0, theme::rgba8(0, 0, 0, 0.5))])
                        .child(div().px(px(18.)).py(px(16.)).border_b_1().border_color(theme::BORDER).child(div().t(s, 15.0).font_weight(FontWeight::BOLD).child("Terminate Connection?")))
                        .child(div().px(px(18.)).py(px(16.)).text_color(theme::TEXT_DIM).child(format!(
                            "Close connection {} for {}? Any running command on that connection may fail.",
                            row.id, user
                        )))
                        .child(
                            div()
                                .flex()
                                .justify_end()
                                .gap(px(8.))
                                .px(px(18.))
                                .py(px(16.))
                                .border_t_1()
                                .border_color(theme::BORDER)
                                .child(dialogs::button("term-cancel", s, "Cancel", dialogs::Btn::Secondary, false, cx.listener(|this, _, _, cx| {
                                    this.terminate_confirm = None;
                                    cx.notify();
                                })))
                                .child(
                                    div()
                                        .id("term-go")
                                        .border_1()
                                        .border_color(theme::rgba8(248, 113, 113, 0.45))
                                        .bg(theme::rgba8(248, 113, 113, 0.16))
                                        .text_color(theme::ERROR)
                                        .rounded(px(4.))
                                        .t(s, 12.0)
                                        .px(px(10.))
                                        .py(px(5.))
                                        .cursor_pointer()
                                        .hover(|st| st.border_color(theme::ACCENT))
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.terminate_confirm = None;
                                            this.terminate_session(&tab_id, r2.clone(), cx);
                                        }))
                                        .child("Terminate"),
                                ),
                        ),
                )
                .into_any_element(),
        )
    }
}
