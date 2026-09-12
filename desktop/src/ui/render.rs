//! Root layout (`App.svelte`): navigator | splitter | main area, status bar,
//! plus window-level overlays (menus, dialogs).

use crate::ui::dialogs::{self, Btn};
use crate::ui::theme;
use crate::ui::widgets::{overlay, shadow, Scale, TextExt};
use crate::ui::workspace::{PaneDrag, Workspace};
use gpui::{
    actions, div, prelude::*, px, AnyElement, App, Context, CursorStyle, KeyBinding, MouseButton, MouseMoveEvent,
    MouseUpEvent, Window,
};

actions!(workspace, [CloseOverlay, GridCopy, GridSelectAll, GridUp, GridDown, GridLeft, GridRight, GridPageUp, GridPageDown,
    GridExtendUp, GridExtendDown, GridExtendLeft, GridExtendRight, GridExtendPageUp, GridExtendPageDown, GridEscape]);

pub fn bind_keys(cx: &mut App) {
    let g = Some("ResultsGrid");
    cx.bind_keys([
        KeyBinding::new("escape", CloseOverlay, Some("Workspace")),
        KeyBinding::new("cmd-c", GridCopy, g),
        KeyBinding::new("ctrl-c", GridCopy, g),
        KeyBinding::new("cmd-a", GridSelectAll, g),
        KeyBinding::new("ctrl-a", GridSelectAll, g),
        KeyBinding::new("up", GridUp, g),
        KeyBinding::new("down", GridDown, g),
        KeyBinding::new("left", GridLeft, g),
        KeyBinding::new("right", GridRight, g),
        KeyBinding::new("pageup", GridPageUp, g),
        KeyBinding::new("pagedown", GridPageDown, g),
        KeyBinding::new("shift-up", GridExtendUp, g),
        KeyBinding::new("shift-down", GridExtendDown, g),
        KeyBinding::new("shift-left", GridExtendLeft, g),
        KeyBinding::new("shift-right", GridExtendRight, g),
        KeyBinding::new("shift-pageup", GridExtendPageUp, g),
        KeyBinding::new("shift-pagedown", GridExtendPageDown, g),
        KeyBinding::new("escape", GridEscape, g),
    ]);
}

impl Workspace {
    fn on_root_mouse_move(&mut self, e: &MouseMoveEvent, window: &mut Window, cx: &mut Context<Self>) {
        match self.pane_drag {
            Some(PaneDrag::Nav) => {
                self.nav_width = f32::from(e.position.x).clamp(160.0, 500.0);
                cx.notify();
            }
            Some(PaneDrag::Split) => {
                if self.main_area_height > 0.0 {
                    let toolbar_h = 32.0;
                    let ratio = (f32::from(e.position.y) - self.main_area_top - toolbar_h) / (self.main_area_height - toolbar_h);
                    self.editor_ratio = ratio.clamp(0.15, 0.85);
                    cx.notify();
                }
            }
            None => {}
        }
        self.on_nav_drag_move(e, window, cx);
        self.on_tab_drag_move(e, window, cx);
        self.on_grid_drag_move(e, window, cx);
    }

    fn on_root_mouse_up(&mut self, e: &MouseUpEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.pane_drag.take().is_some() {
            cx.notify();
        }
        self.on_nav_drag_end(e, window, cx);
        self.on_tab_drag_end(e, window, cx);
        self.on_grid_drag_end(e, window, cx);
    }

    fn close_overlay(&mut self, _: &CloseOverlay, _: &mut Window, cx: &mut Context<Self>) {
        if self.import_dialog.is_some() {
            self.import_dialog = None;
        } else if self.title_dialog.is_some() {
            self.title_dialog = None;
        }
        self.tab_menu = None;
        self.output_menu = None;
        self.conn_select_open = None;
        cx.notify();
    }

    pub fn render_root(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let nav_w = self.nav_width;
        let ratio = self.editor_ratio;
        let dragging = self.pane_drag.clone();
        let mut root = div()
            .id("root")
            .key_context("Workspace")
            .track_focus(&self.focus_handle)
            .on_action(cx.listener(Self::close_overlay))
            .size_full()
            .flex()
            .flex_col()
            .bg(theme::BG)
            .text_color(theme::TEXT)
            .font_family(theme::UI_FONT)
            .t(s, 13.0)
            .overflow_hidden()
            .on_mouse_move(cx.listener(Self::on_root_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_root_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_root_mouse_up))
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    // svelte:window on:click closes the tab menu / nav floating UI.
                    if this.tab_menu.take().is_some() {
                        cx.notify();
                    }
                }),
            );
        if let Some(d) = &dragging {
            root = root.cursor(match d {
                PaneDrag::Nav => CursorStyle::ResizeLeftRight,
                PaneDrag::Split => CursorStyle::ResizeUpDown,
            });
        }

        let workspace = div()
            .flex()
            .flex_1()
            .min_h(px(0.))
            .overflow_hidden()
            .child(
                div()
                    .w(px(nav_w))
                    .min_w(px(nav_w))
                    .flex_shrink_0()
                    .h_full()
                    .overflow_hidden()
                    .flex()
                    .flex_col()
                    .child(self.render_navigator(window, cx)),
            )
            .child(
                div()
                    .id("drag-handle-v")
                    .w(px(4.))
                    .h_full()
                    .flex_shrink_0()
                    .bg(if matches!(dragging, Some(PaneDrag::Nav)) { theme::BORDER } else { theme::BORDER })
                    .hover(|st| st.bg(theme::ACCENT))
                    .cursor(CursorStyle::ResizeLeftRight)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(|this, _, _, cx| {
                            this.pane_drag = Some(PaneDrag::Nav);
                            cx.stop_propagation();
                            cx.notify();
                        }),
                    ),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .overflow_hidden()
                    .min_w(px(0.))
                    .child(self.render_tab_bar(window, cx))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_1()
                            .overflow_hidden()
                            .min_h(px(0.))
                            .child(
                                div()
                                    .flex_grow()
                                    .flex_basis(px(0.))
                                    .flex_shrink_0()
                                    .min_h(px(80.))
                                    .overflow_hidden()
                                    .map(|d| d.flex_grow_ratio(ratio))
                                    .child(self.render_active_panel(window, cx)),
                            )
                            .child(
                                div()
                                    .id("drag-handle-h")
                                    .h(px(4.))
                                    .w_full()
                                    .flex_shrink_0()
                                    .bg(theme::BORDER)
                                    .hover(|st| st.bg(theme::ACCENT))
                                    .cursor(CursorStyle::ResizeUpDown)
                                    .on_mouse_down(
                                        MouseButton::Left,
                                        cx.listener(|this, _, _, cx| {
                                            this.pane_drag = Some(PaneDrag::Split);
                                            cx.stop_propagation();
                                            cx.notify();
                                        }),
                                    ),
                            )
                            .child(
                                div()
                                    .flex_basis(px(0.))
                                    .flex_shrink_0()
                                    .min_h(px(60.))
                                    .overflow_hidden()
                                    .map(|d| d.flex_grow_ratio(1.0 - ratio))
                                    .child(self.render_output_panel(window, cx)),
                            )
                            .on_children_prepainted({
                                let this = cx.entity().downgrade();
                                move |bounds, _w, cx| {
                                    if let Some(b) = bounds.first() {
                                        let top: f32 = b.top().into();
                                        let h: f32 = bounds.iter().map(|b| f32::from(b.size.height)).sum();
                                        this.update(cx, |ws, _| {
                                            ws.main_area_top = top - 31.0;
                                            ws.main_area_height = h + 31.0;
                                        })
                                        .ok();
                                    }
                                }
                            }),
                    ),
            );

        root = root.child(workspace).child(self.render_status_bar(window, cx));

        // Overlays in the same order as the old DOM (later = on top).
        if let Some(el) = self.render_conn_dialog(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_import_dialog(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_tab_menu(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_nav_overlays(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_output_overlays(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_title_dialog(window, cx) {
            root = root.child(el);
        }
        if let Some(el) = self.render_terminate_confirm(window, cx) {
            root = root.child(el);
        }
        root.into_any_element()
    }

    pub fn render_import_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.import_dialog.as_ref()?;
        let s = Scale(self.scale());
        let importing = d.importing;
        let has_path = !d.path.read(cx).text().is_empty();
        let import_type = d.import_type.clone();
        let label = if import_type == "pgdump" { "Import from pgdump" } else { "Import zipped sql" };
        let error = d.error.clone();
        let select_open = d.select_open;
        let path = d.path.clone();
        let modal = dialogs::modal_box(480.0)
            .id("import-modal")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialogs::modal_header(s, "Import Table", cx.listener(|this, _, _, cx| {
                this.import_dialog = None;
                cx.notify();
            })))
            .child(
                div()
                    .p(px(20.))
                    .flex()
                    .flex_col()
                    .gap(px(12.))
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(dialogs::field_label(s, "Import format"))
                            .child(crate::ui::navigator::native_select(
                                "import-type",
                                s,
                                label,
                                select_open,
                                vec![("zipped-sql", "Import zipped sql"), ("pgdump", "Import from pgdump")],
                                &import_type,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(d) = &mut this.import_dialog {
                                        d.select_open = !d.select_open;
                                    }
                                    cx.notify();
                                }),
                                cx.listener(|this, v: &String, _, cx| {
                                    if let Some(d) = &mut this.import_dialog {
                                        d.import_type = v.clone();
                                        d.select_open = false;
                                    }
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(4.))
                            .child(dialogs::field_label(s, "File"))
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap(px(8.))
                                    .child(dialogs::button("import-browse", s, "Browse...", Btn::Secondary, false, cx.listener(|this, _, _, cx| this.import_browse(cx))))
                                    .child(div().flex_1().min_w(px(0.)).child(path)),
                            ),
                    )
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
                    .child(dialogs::button("import-cancel", s, "Cancel", Btn::Secondary, importing, cx.listener(|this, _, _, cx| {
                        this.import_dialog = None;
                        cx.notify();
                    })))
                    .child(dialogs::button(
                        "import-go",
                        s,
                        if importing { "Importing…" } else { "Import" },
                        Btn::Primary,
                        importing || !has_path,
                        cx.listener(|this, _, _, cx| this.import_run(cx)),
                    )),
            );
        Some(
            overlay(0.6)
                .id("import-overlay")
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    // on:click|self closes the import dialog.
                    this.import_dialog = None;
                    cx.notify();
                }))
                .child(modal)
                .into_any_element(),
        )
    }

    pub fn render_title_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.title_dialog.as_ref()?;
        let s = Scale(self.scale());
        let heading = match d.kind {
            dialogs::TitleDialogKind::Save { .. } => "Save Query",
            dialogs::TitleDialogKind::Edit { .. } => "Edit Query Title",
        };
        let can_save = !d.input.read(cx).text().trim().is_empty();
        let input = d.input.clone();
        let small_btn = |id: &'static str, label: &'static str, primary: bool, disabled: bool| {
            let mut b = div()
                .id(id)
                .px(px(16.))
                .py(px(6.))
                .rounded(px(4.))
                .border_1()
                .t(s, 12.0)
                .font_weight(gpui::FontWeight::MEDIUM)
                .child(label);
            b = if primary {
                b.bg(theme::ACCENT).border_color(theme::ACCENT).text_color(theme::WHITE)
            } else {
                b.bg(theme::BG_HOVER).border_color(theme::BORDER).text_color(theme::TEXT)
            };
            if disabled {
                b = b.opacity(0.5);
            } else if primary {
                b = b.cursor_pointer().hover(|st| st.bg(theme::ACCENT_HOVER));
            } else {
                b = b.cursor_pointer().hover(|st| st.bg(theme::BORDER));
            }
            b
        };
        let dialog = div()
            .id("title-dialog")
            .occlude()
            .min_w(px(320.))
            .max_w(px(500.))
            .bg(theme::BG_PANEL)
            .border_1()
            .border_color(theme::BORDER)
            .rounded(px(8.))
            .shadow(vec![shadow(0.0, 10.0, 40.0, 0.0, theme::rgba8(0, 0, 0, 0.3))])
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .p(px(16.))
                    .border_b_1()
                    .border_color(theme::BORDER)
                    .child(div().t(s, 16.0).font_weight(gpui::FontWeight::SEMIBOLD).text_color(theme::TEXT).child(heading)),
            )
            .child(
                div()
                    .p(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(8.))
                    .child(crate::ui::widgets::spaced_text::spaced_text(
                        "QUERY TITLE",
                        gpui::Font { weight: gpui::FontWeight::MEDIUM, ..gpui::font(theme::UI_FONT) },
                        12.0 * s.0,
                        s.lh_f(12.0),
                        theme::TEXT_MUTED,
                        0.5,
                    ))
                    .child(input),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(16.))
                    .py(px(12.))
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(small_btn("title-cancel", "Cancel", false, false).on_click(cx.listener(|this, _, _, cx| {
                        this.title_dialog = None;
                        cx.notify();
                    })))
                    .child({
                        let b = small_btn("title-save", "Save", true, !can_save);
                        if can_save {
                            b.on_click(cx.listener(|this, _, _, cx| this.title_dialog_save(cx)))
                        } else {
                            b
                        }
                    }),
            );
        Some(
            div()
                .absolute()
                .top_0()
                .left_0()
                .size_full()
                .child(
                    div()
                        .id("title-backdrop")
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full()
                        .bg(theme::rgba8(0, 0, 0, 0.5))
                        .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                            this.title_dialog = None;
                            cx.notify();
                        })),
                )
                .child(div().absolute().top_0().left_0().size_full().flex().items_center().justify_center().child(dialog))
                .into_any_element(),
        )
    }
}

pub trait FlexRatio: Styled + Sized {
    fn flex_grow_ratio(mut self, ratio: f32) -> Self {
        self.style().flex_grow = Some(ratio);
        self
    }
}
impl<T: Styled> FlexRatio for T {}


impl Render for Workspace {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.render_root(window, cx)
    }
}
