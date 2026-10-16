//! Shared building blocks that reproduce the old CSS components.

pub mod scroll;
pub mod spaced_text;
pub mod spinner;
pub mod text_input;

use crate::ui::metrics::line_height_normal;
use crate::ui::theme::{self, hsla, Rgba};
use gpui::{div, prelude::*, px, AnyElement, BoxShadow, Div, FontWeight, Hsla, Pixels, SharedString};

/// Font sizing helper: `calc(Npx * var(--app-font-scale))` with WebKit's
/// `line-height: normal`.
#[derive(Clone, Copy, Debug)]
pub struct Scale(pub f32);

impl Scale {
    pub fn fs(self, n: f32) -> Pixels {
        px(n * self.0)
    }
    pub fn lh(self, n: f32) -> Pixels {
        px(line_height_normal(n * self.0))
    }
    pub fn lh_f(self, n: f32) -> f32 {
        line_height_normal(n * self.0)
    }
}

/// Apply a font size + normal line height to a div.
pub trait TextExt: Styled + Sized {
    fn t(self, s: Scale, size: f32) -> Self {
        self.text_size(s.fs(size)).line_height(s.lh(size))
    }
    fn t_rem(self, size_px: f32) -> Self {
        self.text_size(px(size_px)).line_height(px(line_height_normal(size_px)))
    }
    fn weight(self, w: u16) -> Self {
        self.font_weight(FontWeight(w as f32))
    }
}

impl<T: Styled> TextExt for T {}

pub fn shadow(x: f32, y: f32, blur: f32, spread: f32, color: Rgba) -> BoxShadow {
    BoxShadow {
        color: hsla(color),
        offset: gpui::point(px(x), px(y)),
        blur_radius: px(blur),
        spread_radius: px(spread),
    }
}

/// `.modal-overlay` backdrop.
pub fn overlay(alpha: f32) -> Div {
    div()
        .absolute()
        .top_0()
        .left_0()
        .size_full()
        .bg(theme::rgba8(0, 0, 0, alpha))
        .flex()
        .items_center()
        .justify_center()
}

pub fn text_color(c: Rgba) -> Hsla {
    hsla(c)
}

pub fn label(text: impl Into<SharedString>) -> AnyElement {
    div().child(text.into()).into_any_element()
}

/// The "add" button (new connection, new query tab): a bordered 22px square
/// with a plus, lighting up indigo on hover so it reads as clickable.
pub fn plus_button(id: &'static str, s: Scale) -> gpui::Stateful<Div> {
    let idle = theme::TEXT_DIM;
    let hover = theme::hex(0xc7d2fe);
    div()
        .id(id)
        .group(id)
        .size(px(22.))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(5.))
        .border_1()
        .border_color(theme::hex(0x3a3a52))
        .cursor_pointer()
        .hover(|st| st.border_color(theme::ACCENT_HOVER).bg(theme::with_alpha(theme::ACCENT, 0.12)))
        .child(
            crate::ui::icons::icon(crate::ui::icons::Icon::Plus, s.fs(14.0), idle)
                .group_hover(id, move |st| st.text_color(hover)),
        )
}

/// Horizontal 1px rule (`.context-separator`).
pub fn separator(margin: f32) -> Div {
    div().h(px(1.)).bg(theme::BORDER).my(px(margin))
}
