//! The status bar. It shows the workspace's status line and, for a query tab,
//! the row count and duration of its result — including the running count of a
//! result that is still streaming in, which it reads from the result store so
//! the workspace is not disturbed for every batch of rows.

use crate::ui::result_store::{format_elapsed, ResultStore};
use crate::ui::theme;
use crate::ui::widgets::{Scale, TextExt};
use crate::ui::workspace::Workspace;
use gpui::{div, prelude::*, px, Context, Entity, SharedString, Subscription, Task, WeakEntity, Window};

use std::time::Duration;

pub struct StatusBar {
    ws: WeakEntity<Workspace>,
    store: Option<Entity<ResultStore>>,
    store_sub: Option<Subscription>,
    _ws_sub: Subscription,
    timer: Option<Task<()>>,
}

impl StatusBar {
    pub fn new(ws: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(ws, |_, _, cx| cx.notify());
        StatusBar { ws: ws.downgrade(), store: None, store_sub: None, _ws_sub: sub, timer: None }
    }

    /// Follow the active tab's result store.
    fn follow(&mut self, store: Option<Entity<ResultStore>>, cx: &mut Context<Self>) {
        let same = match (&self.store, &store) {
            (Some(a), Some(b)) => a.entity_id() == b.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.timer = None;
            self.store_sub = store.as_ref().map(|s| cx.observe(s, |_, _, cx| cx.notify()));
            self.store = store;
        }
    }

    fn update_timer(&mut self, cx: &mut Context<Self>) {
        let running = self.store.as_ref().is_some_and(|store| {
            store.read(cx).progress.as_ref().is_some_and(|p| p.phase.running())
        });
        if !running {
            self.timer = None;
        } else if self.timer.is_none() {
            self.timer = Some(cx.spawn(async move |this, cx| {
                loop {
                    let delay = this.update(cx, |this, cx| {
                        let progress = this.store.as_ref()?.read(cx).progress.as_ref()?;
                        if !progress.phase.running() { return None; }
                        let elapsed = progress.elapsed();
                        let interval = if elapsed.as_secs() < 10 { 100 } else { 1000 };
                        Some(Duration::from_millis(interval - (elapsed.as_millis() % interval as u128) as u64))
                    }).ok().flatten();
                    let Some(delay) = delay else { break };
                    cx.background_executor().timer(delay).await;
                    if this.update(cx, |_, cx| cx.notify()).is_err() { break; }
                }
            }));
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else { return div().into_any_element() };
        let (status, export_status, store, scale) = {
            let w = ws.read(cx);
            (w.visible_status(), w.export_status.clone(), w.active_store(), w.scale())
        };
        self.follow(store, cx);
        self.update_timer(cx);
        let s = Scale(scale);
        let (rows, duration, progress, error) = match self.store.as_ref().map(|st| st.read(cx)) {
            Some(st) => (
                st.result.as_ref().map(|r| r.rows.len()),
                st.result.as_ref().map_or(0, |r| r.duration),
                st.progress.clone(),
                st.result.as_ref().map(|r| r.error.clone()).unwrap_or_default(),
            ),
            None => (None, 0, None, String::new()),
        };
        // An export in progress (or its outcome) outranks the query status.
        let text: SharedString = if let Some(export) = export_status {
            export
        } else if let Some(progress) = &progress {
            let detail = if error.is_empty() { String::new() } else { format!(": {error}") };
            format!("{} · {}{detail}", progress.phase.label(), format_elapsed(progress.elapsed())).into()
        } else {
            status
        };
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
            .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(text));
        if let Some(n) = rows {
            let sep = || div().opacity(0.4).child("|");
            bar = bar
                .child(sep())
                .child(div().text_color(theme::TEXT_DIM).child(format!("{n} rows")))
                .child(sep())
                .child(div().text_color(theme::TEXT_DIM).child(format!("{duration}ms")))
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
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.ws.update(cx, |ws, cx| ws.export_csv(cx)).ok();
                        }))
                        .child("Export data"),
                );
        }
        bar.into_any_element()
    }
}
