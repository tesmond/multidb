//! Colours and sizing constants lifted from the old CSS custom properties
//! (`App.svelte :root`) plus the CodeMirror One Dark palette.

use gpui::{px, Hsla, Pixels};
pub use gpui::Rgba;

/// `#rrggbb` → colour.
pub const fn hex(rgb: u32) -> Rgba {
    Rgba {
        r: ((rgb >> 16) & 0xff) as f32 / 255.0,
        g: ((rgb >> 8) & 0xff) as f32 / 255.0,
        b: (rgb & 0xff) as f32 / 255.0,
        a: 1.0,
    }
}

/// `rgba(r, g, b, a)`.
pub const fn rgba8(r: u8, g: u8, b: u8, a: f32) -> Rgba {
    Rgba { r: r as f32 / 255.0, g: g as f32 / 255.0, b: b as f32 / 255.0, a }
}

/// `#rrggbbaa`.
pub const fn hexa(rgba: u32) -> Rgba {
    Rgba {
        r: ((rgba >> 24) & 0xff) as f32 / 255.0,
        g: ((rgba >> 16) & 0xff) as f32 / 255.0,
        b: ((rgba >> 8) & 0xff) as f32 / 255.0,
        a: (rgba & 0xff) as f32 / 255.0,
    }
}

pub fn with_alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a: c.a * a, ..c }
}

pub fn hsla(c: Rgba) -> Hsla {
    c.into()
}

// :root palette
pub const BG: Rgba = hex(0x12121a);
pub const BG_PANEL: Rgba = hex(0x1a1a24);
pub const BG_SURFACE: Rgba = hex(0x1e1e2e);
pub const BG_TOOLBAR: Rgba = hex(0x16161f);
pub const BG_EDITOR: Rgba = hex(0x0f0f17);
pub const BG_INPUT: Rgba = hex(0x1e1e2e);
pub const BG_HOVER: Rgba = rgba8(255, 255, 255, 0.04);
pub const BG_SELECTED: Rgba = rgba8(99, 102, 241, 0.15);
pub const BG_BADGE: Rgba = hex(0x252536);
pub const BG_ROW_ALT: Rgba = rgba8(255, 255, 255, 0.02);
pub const BORDER: Rgba = hex(0x2e2e40);
pub const BORDER_SUBTLE: Rgba = hex(0x1e1e2d);
pub const TEXT: Rgba = hex(0xe2e2f0);
pub const TEXT_MUTED: Rgba = hex(0x888898);
pub const TEXT_DIM: Rgba = hex(0xaaaabc);
pub const ACCENT: Rgba = hex(0x6366f1);
pub const ACCENT_HOVER: Rgba = hex(0x818cf8);
pub const SUCCESS: Rgba = hex(0x34d399);
pub const ERROR: Rgba = hex(0xf87171);
pub const WHITE: Rgba = hex(0xffffff);
pub const SAVE_BLUE: Rgba = hex(0x336fc8);
pub const TRANSPARENT: Rgba = Rgba { r: 0.0, g: 0.0, b: 0.0, a: 0.0 };

// Results grid canvas colours
pub const GRID_SEL: Rgba = rgba8(70, 130, 255, 0.28);
pub const GRID_ROW_SEL: Rgba = rgba8(70, 130, 255, 0.13);
pub const GRID_ROWNUM_SEL: Rgba = rgba8(70, 130, 255, 0.45);
pub const GRID_DIRTY: Rgba = rgba8(255, 200, 50, 0.15);
pub const GRID_BORDER_SEL: Rgba = rgba8(80, 140, 255, 0.85);
pub const GRID_BORDER_DIRTY: Rgba = rgba8(255, 200, 50, 0.7);
pub const AMBER: Rgba = rgba8(255, 200, 50, 1.0);

// One Dark (CodeMirror)
pub mod one_dark {
    use super::{hex, hexa, Rgba};
    pub const CHALKY: Rgba = hex(0xe5c07b);
    pub const CORAL: Rgba = hex(0xe06c75);
    pub const CYAN: Rgba = hex(0x56b6c2);
    pub const IVORY: Rgba = hex(0xabb2bf);
    pub const STONE: Rgba = hex(0x7d8799);
    pub const MALIBU: Rgba = hex(0x61afef);
    pub const SAGE: Rgba = hex(0x98c379);
    pub const WHISKEY: Rgba = hex(0xd19a66);
    pub const VIOLET: Rgba = hex(0xc678dd);
    pub const DARK_BACKGROUND: Rgba = hex(0x21252b);
    pub const HIGHLIGHT_BACKGROUND: Rgba = hex(0x2c313a);
    pub const BACKGROUND: Rgba = hex(0x282c34);
    pub const TOOLTIP_BACKGROUND: Rgba = hex(0x353a42);
    pub const SELECTION: Rgba = hex(0x3e4451);
    pub const CURSOR: Rgba = hex(0x528bff);
    pub const ACTIVE_LINE: Rgba = hexa(0x6699ff0b);
    pub const SELECTION_MATCH: Rgba = hexa(0xaafe661a);
    pub const MATCHING_BRACKET: Rgba = hexa(0xbad0f847);
    pub const SEARCH_MATCH: Rgba = hexa(0x72a1ff59);
    pub const SEARCH_MATCH_OUTLINE: Rgba = hex(0x457dff);
    pub const SEARCH_MATCH_SELECTED: Rgba = hexa(0x6199ff2f);
    pub const PLACEHOLDER: Rgba = hex(0x888888);
    pub const LINT_ERROR: Rgba = hex(0xff1111);
    pub const LINT_BORDER: Rgba = hex(0xdd1111);
}

/// Fonts. `.SystemUIFont` resolves to the macOS system font, which is what
/// `-apple-system` rendered in WebKit.
pub const UI_FONT: &str = ".SystemUIFont";
pub const MONO_FONTS: &[&str] = &["JetBrains Mono", "Fira Code", "Cascadia Code", "Menlo"];

pub fn scaled(size: f32, scale: f32) -> Pixels {
    px(size * scale)
}
