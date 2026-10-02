//! The status bar. It shows the workspace's status line and, for a query tab,
//! the row count and duration of its result — including the running count of a
//! result that is still streaming in, which it reads from the result store so
//! the workspace is not disturbed for every batch of rows.

use crate::ui::result_store::ResultStore;
use crate::ui::theme;
use crate::ui::widgets::{Scale, TextExt};
use crate::ui::workspace::Workspace;
use gpui::{div, prelude::*, px, Context, Entity, SharedString, Subscription, WeakEntity, Window};

pub struct StatusBar {
    ws: WeakEntity<Workspace>,
    store: Option<Entity<ResultStore>>,
    store_sub: Option<Subscription>,
    _ws_sub: Subscription,
}

impl StatusBar {
    pub fn new(ws: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        let sub = cx.observe(ws, |_, _, cx| cx.notify());
        StatusBar { ws: ws.downgrade(), store: None, store_sub: None, _ws_sub: sub }
    }

    /// Follow the active tab's result store.
    fn follow(&mut self, store: Option<Entity<ResultStore>>, cx: &mut Context<Self>) {
        let same = match (&self.store, &store) {
            (Some(a), Some(b)) => a.entity_id() == b.entity_id(),
            (None, None) => true,
            _ => false,
        };
        if !same {
            self.store_sub = store.as_ref().map(|s| cx.observe(s, |_, _, cx| cx.notify()));
            self.store = store;
        }
    }
}

impl Render for StatusBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else { return div().into_any_element() };
        let (status, store, scale) = {
            let w = ws.read(cx);
            (w.status.clone(), w.active_store(), w.scale())
        };
        self.follow(store, cx);
        let s = Scale(scale);
        let (rows, duration, loading) = match self.store.as_ref().map(|st| st.read(cx)) {
            Some(st) => match &st.result {
                Some(r) => (Some(r.rows.len()), r.duration, st.loading),
                None => (None, 0, st.loading),
            },
            None => (None, 0, false),
        };
        let text: SharedString = match (loading, rows) {
            (true, Some(n)) => format!("Loading… {n} rows").into(),
            _ => status,
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
