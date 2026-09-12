//! Shared building blocks that reproduce the old CSS components.

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

/// Horizontal 1px rule (`.context-separator`).
pub fn separator(margin: f32) -> Div {
    div().h(px(1.)).bg(theme::BORDER).my(px(margin))
}
