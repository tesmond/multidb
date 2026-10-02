//! Rendering for the search panel and the goto-line dialog.

use super::search;
use super::SqlEditor;
use crate::ui::theme;
use gpui::{
    div, linear_color_stop, linear_gradient, prelude::*, px, AnyElement, Context, MouseButton, Window,
};

/// `.cm-button`
fn button(
    id: &'static str,
    label: &'static str,
    font_size: f32,
    on_click: impl Fn(&mut SqlEditor, &mut Window, &mut Context<SqlEditor>) + 'static,
    cx: &mut Context<SqlEditor>,
) -> AnyElement {
    let fs = 0.7 * font_size;
    div()
        .id(id)
        .flex_shrink_0()
        .mr(px(0.6 * font_size))
        .my(px(0.2 * font_size))
        .px(px(1.0 * fs))
        .py(px(0.2 * fs))
        .rounded(px(1.))
        .border_1()
        .border_color(theme::hex(search::BUTTON_BORDER))
        .bg(linear_gradient(
            180.,
            linear_color_stop(theme::hex(search::BUTTON_TOP), 0.),
            linear_color_stop(theme::hex(search::BUTTON_BOTTOM), 1.),
        ))
        .text_size(px(fs))
        .line_height(px(crate::ui::metrics::line_height_normal(fs)))
        .text_color(theme::WHITE)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, window, cx| on_click(this, window, cx)))
        .child(label)
        .into_any_element()
}

/// `[name=close]`: `position: absolute; top: 0; bottom: 0; right: 4px`, so the
/// `×` is centred over the panel's full height.
fn close_button(id: &'static str, font_size: f32, cx: &mut Context<SqlEditor>) -> AnyElement {
    div()
        .id(id)
        .absolute()
        .top(px(0.))
        .bottom(px(0.))
        .right(px(4.))
        .flex()
        .items_center()
        .text_size(px(font_size))
        .line_height(px(crate::ui::metrics::line_height_normal(font_size)))
        .text_color(theme::WHITE)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(|this, _, window, cx| this.close_search(window, cx)))
        .child("×")
        .into_any_element()
}

/// `<label><input type=checkbox> text</label>` at 80% of the panel font.
fn checkbox(
    id: &'static str,
    label: &'static str,
    checked: bool,
    font_size: f32,
    option: &'static str,
    cx: &mut Context<SqlEditor>,
) -> AnyElement {
    let fs = 0.8 * font_size;
    div()
        .id(id)
        .flex()
        .items_center()
        .flex_shrink_0()
        .mr(px(0.6 * font_size))
        .my(px(0.2 * font_size))
        .text_size(px(fs))
        .line_height(px(crate::ui::metrics::line_height_normal(fs)))
        .text_color(theme::WHITE)
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
        .on_click(cx.listener(move |this, _, _, cx| this.toggle_search_option(option, cx)))
        .child(div().mr(px(0.2 * font_size)).child(crate::ui::dialogs::checkbox(checked, false)))
        .child(label)
        .into_any_element()
}

impl SqlEditor {
    /// The `.cm-panels-bottom` strip: the search panel, then the goto dialog.
    pub fn render_search_panels(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if self.search.is_none() && self.goto.is_none() {
            return None;
        }
        let font_size = 13.0 * self.font_scale;
        let mut panels = div()
            .flex()
            .flex_col()
            .w_full()
            .flex_shrink_0()
            // `.cm-panels.cm-panels-bottom { border-top: 2px solid black }`
            .border_t(px(2.))
            .border_color(theme::hex(0x000000))
            .bg(theme::hex(search::PANEL_BG))
            .text_color(theme::WHITE)
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .on_scroll_wheel(|_, _, cx| cx.stop_propagation());

        if let Some(panel) = &self.search {
            let (search_input, replace_input) = (panel.search_input.clone(), panel.replace_input.clone());
            let (case, re, word) = (panel.query.case_sensitive, panel.query.regexp, panel.query.whole_word);
            let width = search::default_input_width(font_size);
            let field = |input: gpui::Entity<crate::ui::widgets::text_input::TextInput>| {
                div().w(px(width)).flex_shrink_0().mr(px(0.6 * font_size)).my(px(0.2 * font_size)).child(input)
            };
            panels = panels.child(
                div()
                    .relative()
                    .w_full()
                    .pt(px(2.))
                    .px(px(6.))
                    .pb(px(4.))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .child(field(search_input))
                            .child(button("search-next", "next", font_size, |e, _, cx| e.find_next(cx), cx))
                            .child(button("search-prev", "previous", font_size, |e, _, cx| e.find_previous(cx), cx))
                            .child(button("search-all", "all", font_size, |e, _, cx| e.select_all_matches(cx), cx))
                            .child(checkbox("search-case", "match case", case, font_size, "case", cx))
                            .child(checkbox("search-re", "regexp", re, font_size, "re", cx))
                            .child(checkbox("search-word", "by word", word, font_size, "word", cx)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .child(field(replace_input))
                            .child(button("search-replace", "replace", font_size, |e, _, cx| e.replace_next(cx), cx))
                            .child(button("search-replace-all", "replace all", font_size, |e, _, cx| e.replace_all(cx), cx)),
                    )
                    .child(close_button("search-close", font_size, cx)),
            );
        }

        if let Some(dialog) = &self.goto {
            let input = dialog.input.clone();
            panels = panels.child(
                div()
                    .relative()
                    .w_full()
                    .flex()
                    .items_center()
                    .pt(px(2.))
                    .px(px(6.))
                    .pb(px(4.))
                    .text_size(px(font_size))
                    .child(
                        // `<label>Go to line: <input class=cm-textfield></label>`
                        // at 80%; the field is 70% of *that*.
                        div()
                            .flex()
                            .items_center()
                            .text_size(px(0.8 * font_size))
                            .line_height(px(crate::ui::metrics::line_height_normal(0.8 * font_size)))
                            .child("Go to line: ")
                            .child(
                                div()
                                    .w(px(search::default_input_width(0.8 * font_size)))
                                    .my(px(0.2 * font_size))
                                    .mr(px(0.6 * font_size))
                                    .child(input),
                            ),
                    )
                    .child(button("goto-go", "go", font_size, |e, window, cx| e.goto_line_apply(window, cx), cx))
                    .child(close_button("goto-close", font_size, cx)),
            );
        }
        Some(panels.into_any_element())
    }

    /// Ranges the editor element paints as `.cm-searchMatch`, and the selected
    /// one (`.cm-searchMatch-selected`).
    pub fn search_highlights(&self) -> (&[std::ops::Range<usize>], Option<std::ops::Range<usize>>) {
        match &self.search {
            Some(p) => {
                let sel = self.buffer.selection.range();
                let selected = p.matches.iter().find(|m| **m == sel).cloned();
                (&p.matches, selected)
            }
            None => (&[], None),
        }
    }
}
