//! Font metrics that reproduce WebKit's `line-height: normal` so text boxes
//! have exactly the heights the CSS layout produced.

use crate::ui::theme::{rgba8, Rgba};
use gpui::{font, px, App};
use std::sync::atomic::{AtomicU32, Ordering};

/// Ascent / descent / leading of the UI font per 1px of font size.
static UI_ASCENT: AtomicU32 = AtomicU32::new(0);
static UI_DESCENT: AtomicU32 = AtomicU32::new(0);
static MONO_ASCENT: AtomicU32 = AtomicU32::new(0);
static MONO_DESCENT: AtomicU32 = AtomicU32::new(0);
/// Metrics of the generic `monospace` family (WebKit's fixed-pitch default),
/// used by the completion tooltip.
static GENERIC_ASCENT: AtomicU32 = AtomicU32::new(0);
static GENERIC_DESCENT: AtomicU32 = AtomicU32::new(0);

/// `::placeholder` colour WebKit used on the dark inputs.
pub const PLACEHOLDER: Rgba = rgba8(117, 117, 117, 1.0);
/// Native text selection colours (macOS light appearance, blue accent).
pub const SELECTION_FOCUSED: Rgba = rgba8(179, 215, 255, 1.0);
pub const SELECTION_UNFOCUSED: Rgba = rgba8(220, 220, 220, 1.0);

fn store(a: &AtomicU32, v: f32) {
    a.store(v.to_bits(), Ordering::Relaxed);
}
fn load(a: &AtomicU32, default: f32) -> f32 {
    let bits = a.load(Ordering::Relaxed);
    if bits == 0 {
        default
    } else {
        f32::from_bits(bits)
    }
}

pub fn init(cx: &App, mono_family: &str) {
    let ts = cx.text_system();
    let id = ts.resolve_font(&font(crate::ui::theme::UI_FONT));
    store(&UI_ASCENT, f32::from(ts.ascent(id, px(1000.))) / 1000.0);
    store(&UI_DESCENT, f32::from(ts.descent(id, px(1000.))).abs() / 1000.0);
    let id = ts.resolve_font(&font(mono_family.to_string()));
    store(&MONO_ASCENT, f32::from(ts.ascent(id, px(1000.))) / 1000.0);
    store(&MONO_DESCENT, f32::from(ts.descent(id, px(1000.))).abs() / 1000.0);
    let id = ts.resolve_font(&font(crate::ui::editor::popup::GENERIC_MONO));
    store(&GENERIC_ASCENT, f32::from(ts.ascent(id, px(1000.))) / 1000.0);
    store(&GENERIC_DESCENT, f32::from(ts.descent(id, px(1000.))).abs() / 1000.0);
}

pub fn ui_ascent(font_size: f32) -> f32 {
    load(&UI_ASCENT, 0.952) * font_size
}

pub fn ui_descent(font_size: f32) -> f32 {
    load(&UI_DESCENT, 0.241) * font_size
}

pub fn mono_ascent(font_size: f32) -> f32 {
    load(&MONO_ASCENT, 0.928) * font_size
}

pub fn mono_descent(font_size: f32) -> f32 {
    load(&MONO_DESCENT, 0.236) * font_size
}

/// Ascent/descent of the generic `monospace` family.
pub fn generic_ascent(font_size: f32) -> f32 {
    load(&GENERIC_ASCENT, 0.832) * font_size
}

pub fn generic_descent(font_size: f32) -> f32 {
    load(&GENERIC_DESCENT, 0.3) * font_size
}

/// WebKit: `lroundf(ascent) + lroundf(descent) + lroundf(lineGap)`.
pub fn line_height_normal(font_size: f32) -> f32 {
    let a = load(&UI_ASCENT, 0.952) * font_size;
    let d = load(&UI_DESCENT, 0.241) * font_size;
    a.round() + d.round()
}

/// Content height of a WebKit text input's inner editor (the same line box a
/// block would get; the accessibility rect reports one pixel more).
pub fn input_inner_height(font_size: f32) -> f32 {
    line_height_normal(font_size)
}

pub fn mono_line_height_normal(font_size: f32) -> f32 {
    let a = load(&MONO_ASCENT, 0.928) * font_size;
    let d = load(&MONO_DESCENT, 0.236) * font_size;
    a.round() + d.round()
}
