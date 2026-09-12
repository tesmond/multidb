//! Native colour picker. WebKit's `<input type="color">` opens the shared
//! `NSColorPanel` on macOS; we do the same and poll it from the foreground
//! executor (AppKit objects must only be touched on the main thread).

use gpui::Rgba;

#[allow(dead_code)]
fn to_hex(r: f64, g: f64, b: f64) -> String {
    let c = |v: f64| (v.clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", c(r), c(g), c(b))
}

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use objc2::MainThreadMarker;
    use objc2_app_kit::{NSColor, NSColorPanel, NSColorSpace};

    pub fn show(current: Rgba) {
        let Some(mtm) = MainThreadMarker::new() else { return };
        {
            let panel = NSColorPanel::sharedColorPanel(mtm);
            panel.setShowsAlpha(false);
            let color = NSColor::colorWithSRGBRed_green_blue_alpha(current.r as f64, current.g as f64, current.b as f64, 1.0);
            panel.setColor(&color);
            panel.makeKeyAndOrderFront(None);
        }
    }

    /// Returns `(visible, hex)` for the shared panel.
    pub fn poll() -> (bool, Option<String>) {
        let Some(mtm) = MainThreadMarker::new() else { return (false, None) };
        {
            let panel = NSColorPanel::sharedColorPanel(mtm);
            let visible = panel.isVisible();
            let color = panel.color();
            let srgb = NSColorSpace::sRGBColorSpace();
            let hex = color
                .colorUsingColorSpace(&srgb)
                .map(|c| to_hex(c.redComponent(), c.greenComponent(), c.blueComponent()));
            (visible, hex)
        }
    }

    pub fn close() {
        let Some(mtm) = MainThreadMarker::new() else { return };
        {
            NSColorPanel::sharedColorPanel(mtm).orderOut(None);
        }
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::*;

    pub fn show(_current: Rgba) {}

    pub fn poll() -> (bool, Option<String>) {
        (false, None)
    }

    pub fn close() {}
}

pub use imp::{close, poll, show};
