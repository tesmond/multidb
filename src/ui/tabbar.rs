//! Query tab bar (tabs, rename, drag reorder, overflow scroll, context menu).
//!
//! The bar is its own view. The application state it shows (the tab list, the
//! active tab, connection colours, the font scale) stays on [`Workspace`] and is
//! read from there; everything that only the bar cares about — the inline
//! rename box, a drag in progress, the scroll position, the context menu and
//! the painted bounds — lives here, so dragging a tab or opening its menu does
//! not redraw anything else.

use crate::ui::model::TabId;
use crate::ui::theme::{self, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{separator, shadow, Scale, TextExt};
use crate::ui::workspace::{TabDrag, Workspace};
use gpui::{
    anchored, deferred, div, point, prelude::*, px, Animation, AnimationExt, AnyElement, Bounds, Context, Corner,
    Entity, MouseButton, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollHandle, Subscription, WeakEntity, Window,
};
use std::time::Duration;

/// `contrast-color()`: white or black, whichever contrasts more (WCAG).
pub fn contrast_color(c: Rgba) -> Rgba {
    let lin = |v: f32| if v <= 0.04045 { v / 12.92 } else { ((v + 0.055) / 1.055).powf(2.4) };
    let l = 0.2126 * lin(c.r) + 0.7152 * lin(c.g) + 0.0722 * lin(c.b);
    let with_white = 1.05 / (l + 0.05);
    let with_black = (l + 0.05) / 0.05;
    if with_white >= with_black {
        theme::WHITE
    } else {
        theme::hex(0x000000)
    }
}

/// What the bar needs to know about one tab, copied out of the workspace for
/// each render.
#[derive(Clone)]
pub struct TabInfo {
    pub id: TabId,
    pub title: String,
    pub running: bool,
    /// The connection's custom tab colour, if it has one.
    pub custom: Option<Rgba>,
}

pub struct TabBar {
    ws: WeakEntity<Workspace>,
    /// Tab ids in display order, as of the last render; drags refer to tabs by
    /// position in this list.
    order: Vec<TabId>,
    scale: f32,
    edit: Option<(TabId, Entity<TextInput>)>,
    drag: Option<TabDrag>,
    scroll: ScrollHandle,
    menu: Option<(TabId, Point<Pixels>)>,
    bounds: Vec<Bounds<Pixels>>,
    _sub: Subscription,
}

impl TabBar {
    pub fn new(ws: &Entity<Workspace>, cx: &mut Context<Self>) -> Self {
        // Until the application state is split by concern, any change to it
        // may change a tab (title, colour, running spinner, which is active).
        let sub = cx.observe(ws, |_, _, cx| cx.notify());
        TabBar {
            ws: ws.downgrade(),
            order: Vec::new(),
            scale: 1.0,
            edit: None,
            drag: None,
            scroll: ScrollHandle::new(),
            menu: None,
            bounds: Vec::new(),
            _sub: sub,
        }
    }

    /// Run `f` against the workspace.
    fn with_ws<R>(&self, cx: &mut Context<Self>, f: impl FnOnce(&mut Workspace, &mut Context<Workspace>) -> R) -> Option<R> {
        self.ws.update(cx, f).ok()
    }

    /// Escape and clicks elsewhere close the menu.
    pub fn close_menu(&mut self, cx: &mut Context<Self>) {
        if self.menu.take().is_some() {
            cx.notify();
        }
    }

    pub fn start_rename(&mut self, tab_id: &str, title: String, window: &mut Window, cx: &mut Context<Self>) {
        let s = self.scale;
        let lh = crate::ui::metrics::line_height_normal(12.0 * s);
        let look = InputLook {
            font_size: 12.0 * s,
            line_height: lh,
            height: lh + 4.0 + 2.0,
            pad_x: (4.0, 4.0),
            radius: 2.0,
            bg: theme::TRANSPARENT,
            border: theme::ACCENT,
            focus_border: theme::ACCENT,
            ..InputLook::dialog(s)
        };
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, look);
            t.set_text(title, cx);
            t.select_all_text(cx);
            t
        });
        let id = tab_id.to_string();
        cx.subscribe_in(&input, window, move |this, input, ev: &InputEvent, _w, cx| match ev {
            InputEvent::Submit | InputEvent::Blur => {
                if this.edit.as_ref().is_some_and(|(t, _)| *t == id) {
                    let text = input.read(cx).text().trim().to_string();
                    this.edit = None;
                    if !text.is_empty() {
                        let id = id.clone();
                        this.with_ws(cx, |ws, cx| {
                            ws.rename_tab(&id, text);
                            cx.notify();
                        });
                    }
                    cx.notify();
                }
            }
            InputEvent::Cancel => {
                this.edit = None;
                cx.notify();
            }
            _ => {}
        })
        .detach();
        input.update(cx, |i, _| i.focus(window));
        self.edit = Some((tab_id.to_string(), input));
        cx.notify();
    }

    pub fn on_drag_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(drag) = &mut self.drag else { return };
        if !e.dragging() {
            return;
        }
        if !drag.active {
            if (e.position.x - drag.start_x).abs() < px(3.) {
                return;
            }
            drag.active = true;
        }
        // updateDropTarget: insertion index from tab midpoints.
        let bounds = &self.bounds;
        let bar_left = bounds.first().map(|b| b.left()).unwrap_or(px(0.)) - self.scroll.offset().x;
        let mut insert_at = bounds.len();
        let mut indicator = bounds.last().map(|b| b.right() - bar_left).unwrap_or(px(0.));
        for (i, b) in bounds.iter().enumerate() {
            let mid = b.left() + b.size.width / 2.;
            if e.position.x < mid {
                insert_at = i;
                indicator = b.left() - bar_left;
                break;
            }
        }
        drag.drop_index = Some(insert_at);
        drag.indicator_x = indicator;
        cx.notify();
    }

    pub fn on_drag_end(&mut self, _e: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some(drag) = self.drag.take() else { return };
        if drag.active {
            if let Some(to) = drag.drop_index {
                let from = drag.index;
                if from != to {
                    let adjusted = if from < to { to - 1 } else { to };
                    self.with_ws(cx, |ws, cx| {
                        ws.reorder_tabs(from, adjusted);
                        cx.notify();
                    });
                }
            }
        }
        cx.notify();
    }

    fn render_tab(&mut self, i: usize, info: &TabInfo, active: bool, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale);
        let id = info.id.clone();
        let custom = info.custom;
        let running = info.running;
        let dragging = self.drag.as_ref().is_some_and(|d| d.active && d.index == i);
        let editing = self.edit.as_ref().filter(|(t, _)| *t == info.id).map(|(_, e)| e.clone());
        let title = info.title.clone();
        let (fg, bg, border) = match (custom, active) {
            (Some(c), true) => (contrast_color(c), Some(c), contrast_color(c)),
            (Some(c), false) => (contrast_color(c), Some(c), theme::TRANSPARENT),
            (None, true) => (theme::TEXT, Some(theme::BG_SURFACE), theme::ACCENT),
            (None, false) => (theme::TEXT_MUTED, None, theme::TRANSPARENT),
        };
        let group = gpui::SharedString::from(format!("tab-{id}"));
        let close_color = if custom.is_some() { fg } else { theme::TEXT_MUTED };
        let mut el = div()
            .id(gpui::ElementId::Name(format!("tab-{id}").into()))
            .group(group.clone())
            .flex()
            .items_center()
            .gap(px(6.))
            .px(px(14.))
            .pt(px(6.))
            .pb(px(6.))
            .border_b_2()
            .border_color(border)
            .text_color(fg)
            .t(s, 12.0)
            .whitespace_nowrap()
            .min_w(px(80.))
            .flex_shrink_0()
            .cursor_pointer()
            .when_some(bg, |d, c| d.bg(c))
            .when(dragging, |d| d.opacity(0.5));
        el = match custom {
            Some(c) => el.hover(move |st| st.bg(brighten(c, 1.03))),
            None if !active => el.hover(|st| st.text_color(theme::TEXT).bg(theme::BG_HOVER)),
            None => el,
        };
        el = el
            .on_mouse_down(
                MouseButton::Left,
                cx.listener({
                    let id = id.clone();
                    move |this, e: &gpui::MouseDownEvent, _, _cx| {
                        let index = this.order.iter().position(|t| *t == id).unwrap_or(0);
                        this.drag = Some(TabDrag { index, start_x: e.position.x, active: false, drop_index: None, indicator_x: px(0.) });
                    }
                }),
            )
            .on_click(cx.listener({
                let id = id.clone();
                move |this, _, _, cx| {
                    if this.edit.as_ref().is_some_and(|(t, _)| *t == id) {
                        return;
                    }
                    let id = id.clone();
                    this.with_ws(cx, |ws, cx| ws.set_active_tab(id, cx));
                }
            }))
            .on_mouse_down(
                MouseButton::Right,
                cx.listener({
                    let id = id.clone();
                    move |this, e: &gpui::MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        this.menu = Some((id.clone(), e.position));
                        cx.notify();
                    }
                }),
            );
        el = match editing {
            Some(input) => el.child(div().flex_1().min_w(px(0.)).text_color(fg).child(input)),
            None => el.child(div().flex_1().child(title)),
        };
        if running {
            let fs = 13.0 * s.0;
            let lh = s.lh_f(13.0);
            el = el.child(
                crate::ui::widgets::spinner::spinner(fs, lh, fg).with_animation(
                    gpui::ElementId::Name(format!("spin-{id}").into()),
                    Animation::new(Duration::from_secs(1)).repeat(),
                    |spinner, delta| spinner.angle(delta),
                ),
            );
        }
        let close_group = group.clone();
        el = el.child(
            div()
                .id(gpui::ElementId::Name(format!("tab-close-{id}").into()))
                .t(s, 11.0)
                .px(px(2.))
                .rounded(px(2.))
                .text_color(close_color)
                .opacity(0.)
                .group_hover(close_group, |st| st.opacity(0.7))
                .hover(move |st| {
                    let st = st.opacity(1.0);
                    if custom.is_some() { st.bg(theme::rgba8(0, 0, 0, 0.12)) } else { st.text_color(theme::TEXT).bg(theme::BG_HOVER) }
                })
                .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .on_click(cx.listener({
                    let id = id.clone();
                    move |this, _, _, cx| {
                        cx.stop_propagation();
                        let id = id.clone();
                        this.with_ws(cx, |ws, cx| ws.remove_tab(&id, cx));
                    }
                }))
                .child("✕"),
        );
        let this = cx.entity().downgrade();
        el.into_any_element().map_bounds(i, this)
    }

    fn scroll_tabs(&mut self, dir: f32) {
        let width: f32 = self.scroll.bounds().size.width.into();
        let step = (width * 0.65).floor().max(180.0);
        let max: f32 = self.scroll.max_offset().width.into();
        let cur: f32 = self.scroll.offset().x.into();
        let next = (cur - dir * step).clamp(-max, 0.0);
        self.scroll.set_offset(point(px(next), px(0.)));
    }

    fn render_menu(&mut self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let (tab_id, pos) = self.menu.clone()?;
        let s = Scale(self.scale);
        let item = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .w_full()
                .px(px(16.))
                .py(px(8.))
                .t(s, 13.0)
                .text_color(theme::TEXT)
                .cursor_pointer()
                .hover(|st| st.bg(theme::BG_HOVER))
                .child(label)
        };
        let (t1, t2, t3, t4, t5) = (tab_id.clone(), tab_id.clone(), tab_id.clone(), tab_id.clone(), tab_id.clone());
        let menu = div()
            .id("tab-menu")
            .occlude()
            .min_w(px(160.))
            .bg(theme::BG_SURFACE)
            .border_1()
            .border_color(theme::BORDER)
            .rounded(px(4.))
            .shadow(vec![shadow(0.0, 4.0, 16.0, 0.0, theme::rgba8(0, 0, 0, 0.4))])
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_mouse_down_out(cx.listener(|this, _, _, cx| this.close_menu(cx)))
            .child(item("tm-rename", "Rename Tab").on_click(cx.listener(move |this, _, window, cx| {
                this.menu = None;
                let title = this.with_ws(cx, |ws, _| ws.tab(&t1).map(|t| t.title.clone())).flatten().unwrap_or_default();
                this.start_rename(&t1, title, window, cx);
            })))
            .child(item("tm-dup", "Duplicate Tab").on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.with_ws(cx, |ws, cx| ws.duplicate_tab(&t2, cx));
                cx.notify();
            })))
            .child(separator(3.0))
            .child(item("tm-others", "Close Other Tabs").on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.with_ws(cx, |ws, cx| {
                    ws.close_other_tabs(&t3, cx);
                    cx.notify();
                });
                cx.notify();
            })))
            .child(item("tm-right", "Close Tabs to the Right").on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.with_ws(cx, |ws, cx| {
                    ws.close_tabs_right(&t4, cx);
                    cx.notify();
                });
                cx.notify();
            })))
            .child(item("tm-left", "Close Tabs to the Left").on_click(cx.listener(move |this, _, _, cx| {
                this.menu = None;
                this.with_ws(cx, |ws, cx| {
                    ws.close_tabs_left(&t5, cx);
                    cx.notify();
                });
                cx.notify();
            })));
        Some(deferred(anchored().position(pos).anchor(Corner::TopLeft).child(menu)).with_priority(3).into_any_element())
    }

    fn record_bounds(&mut self, index: usize, bounds: Bounds<Pixels>) {
        if self.bounds.len() <= index {
            self.bounds.resize(index + 1, Bounds::default());
        }
        self.bounds[index] = bounds;
        self.bounds.truncate(self.order.len());
    }
}

impl Render for TabBar {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(ws) = self.ws.upgrade() else { return div().into_any_element() };
        let (infos, active_id, scale) = {
            let w = ws.read(cx);
            (w.tab_infos(), w.active_tab_id.clone(), w.scale())
        };
        self.scale = scale;
        self.order = infos.iter().map(|t| t.id.clone()).collect();
        if self.edit.as_ref().is_some_and(|(id, _)| !self.order.contains(id)) {
            self.edit = None;
        }
        let s = Scale(scale);
        let tabs: Vec<AnyElement> = infos
            .iter()
            .enumerate()
            .map(|(i, info)| {
                let active = info.id == active_id;
                self.render_tab(i, info, active, cx)
            })
            .collect();
        let overflow = self.scroll.max_offset().width > px(1.);
        let can_left = self.scroll.offset().x < px(-1.);
        let can_right = self.scroll.offset().x > -self.scroll.max_offset().width + px(1.);
        let indicator = self.drag.as_ref().filter(|d| d.active && d.drop_index.is_some()).map(|d| d.indicator_x);
        let scroll_btn = |id: &'static str, glyph: &'static str, enabled: bool| {
            div()
                .id(id)
                .size(px(26.))
                .ml(px(4.))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .border_1()
                .border_color(theme::BORDER)
                .rounded(px(4.))
                .bg(theme::BG_SURFACE)
                .text_color(theme::TEXT)
                .text_size(s.fs(11.0))
                .line_height(px(11.0 * s.0))
                .child(glyph)
                .when(!enabled, |d| d.opacity(0.45))
                .when(enabled, |d| d.cursor_pointer().hover(|st| st.border_color(theme::ACCENT).text_color(theme::ACCENT_HOVER)))
        };
        let bar = div()
            .flex()
            .items_center()
            .bg(theme::BG_TOOLBAR)
            .border_b_1()
            .border_color(theme::BORDER)
            .overflow_hidden()
            .flex_shrink_0()
            .min_w(px(0.))
            .child(
                div()
                    .id("tab-scroll")
                    .relative()
                    .flex_1()
                    .min_w(px(0.))
                    .flex()
                    .items_center()
                    .overflow_x_scroll()
                    .track_scroll(&self.scroll)
                    .children(tabs)
                    .when_some(indicator, |d, x| {
                        d.child(
                            div()
                                .absolute()
                                .top(px(8.))
                                .bottom(px(8.))
                                .left(x)
                                .w(px(6.))
                                .rounded(px(2.))
                                .bg(theme::WHITE)
                                .shadow(vec![shadow(0.0, 0.0, 0.0, 2.0, theme::rgba8(255, 255, 255, 0.05))]),
                        )
                    }),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .flex_shrink_0()
                    .border_l_1()
                    .border_color(theme::BORDER_SUBTLE)
                    .bg(theme::BG_TOOLBAR)
                    .when(overflow, |d| {
                        d.child(scroll_btn("tab-left", "◀", can_left).on_click(cx.listener(|this, _, _, cx| {
                            this.scroll_tabs(-1.0);
                            cx.notify();
                        })))
                        .child(scroll_btn("tab-right", "▶", can_right).on_click(cx.listener(|this, _, _, cx| {
                            this.scroll_tabs(1.0);
                            cx.notify();
                        })))
                    })
                    .child(
                        div()
                            .id("tab-add")
                            .px(px(12.))
                            .py(px(6.))
                            .t(s, 14.0)
                            .text_color(theme::TEXT_MUTED)
                            .cursor_pointer()
                            .hover(|st| st.text_color(theme::TEXT))
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.with_ws(cx, |ws, cx| ws.new_query_tab(cx));
                            }))
                            .child("+"),
                    ),
            );
        div().flex_shrink_0().min_w(px(0.)).child(bar).children(self.render_menu(cx)).into_any_element()
    }
}

fn brighten(c: Rgba, f: f32) -> Rgba {
    Rgba { r: (c.r * f).min(1.0), g: (c.g * f).min(1.0), b: (c.b * f).min(1.0), a: c.a }
}

/// Records each tab's painted bounds for drag-and-drop hit testing.
trait MapBounds {
    fn map_bounds(self, index: usize, bar: WeakEntity<TabBar>) -> AnyElement;
}

impl MapBounds for AnyElement {
    fn map_bounds(self, index: usize, bar: WeakEntity<TabBar>) -> AnyElement {
        div()
            .flex_shrink_0()
            .child(self)
            .on_children_prepainted(move |bounds, _w, cx| {
                if let Some(b) = bounds.first() {
                    let b = *b;
                    bar.update(cx, |bar, _| bar.record_bounds(index, b)).ok();
                }
            })
            .into_any_element()
    }
}
