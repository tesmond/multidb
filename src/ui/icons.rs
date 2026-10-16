//! The app's SVG icons (the "Duotone" set: 1.5px outlines with a 35% tint
//! fill) and the asset source that serves them to gpui.
//!
//! Every icon is drawn on a 16×16 grid in a single colour; gpui renders an
//! SVG as an alpha mask, so the colour comes from the `text_color` passed to
//! [`icon`] and the tint's opacity is preserved.

use gpui::{prelude::*, svg, AssetSource, Hsla, Pixels, SharedString, Svg};
use std::borrow::Cow;

macro_rules! icons {
    ($($variant:ident => $file:literal,)*) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq)]
        pub enum Icon {
            $($variant,)*
        }

        impl Icon {
            pub const ALL: &'static [Icon] = &[$(Icon::$variant,)*];

            /// Asset path the icon is served under.
            pub fn path(self) -> &'static str {
                match self {
                    $(Icon::$variant => concat!("multidb/icons/", $file, ".svg"),)*
                }
            }

            fn bytes(self) -> &'static [u8] {
                match self {
                    $(Icon::$variant => include_bytes!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/icons/", $file, ".svg")),)*
                }
            }
        }
    };
}

icons! {
    Connection => "connection",
    Database => "database",
    Schema => "schema",
    Table => "table",
    View => "view",
    Group => "group",
    Key => "key",
    Index => "index",
    Settings => "settings",
    Edit => "edit",
    Close => "close",
    Run => "run",
    Stop => "stop",
    Save => "save",
    Format => "format",
    Check => "check",
    ChevronRight => "chevron-right",
    ChevronDown => "chevron-down",
    ChevronLeft => "chevron-left",
    SortAsc => "sort-asc",
    SortDesc => "sort-desc",
    CaretUp => "caret-up",
    CaretDown => "caret-down",
}

/// A square icon of `size`, painted in `color`. gpui's `svg()` does not
/// inherit the parent's text colour, so hover colours need `group_hover`.
pub fn icon(icon: Icon, size: Pixels, color: impl Into<Hsla>) -> Svg {
    svg().path(icon.path()).size(size).flex_shrink_0().text_color(color)
}

/// Serves the bundled icons; registered with `Application::with_assets`.
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> anyhow::Result<Option<Cow<'static, [u8]>>> {
        Ok(Icon::ALL.iter().find(|i| i.path() == path).map(|i| Cow::Borrowed(i.bytes())))
    }

    fn list(&self, path: &str) -> anyhow::Result<Vec<SharedString>> {
        Ok(Icon::ALL.iter().map(|i| i.path()).filter(|p| p.starts_with(path)).map(SharedString::from).collect())
    }
}
