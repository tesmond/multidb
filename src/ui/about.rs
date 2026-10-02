//! "About multidb" dialog: application icon, version (read from Cargo.toml at build
//! time) and a short description. Opened from the Help menu (or F1).

use crate::ui::dialogs::{self, Btn};
use crate::ui::theme;
use crate::ui::widgets::{overlay, Scale, TextExt};
use crate::ui::workspace::Workspace;
use crate::ui::About;
use gpui::{div, img, prelude::*, px, AnyElement, Context, FontWeight, Image, ImageFormat, MouseButton, Window};
use std::sync::{Arc, OnceLock};

/// Version from `Cargo.toml`; the release workflow tags with the same value.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
const DESCRIPTION: &str = env!("CARGO_PKG_DESCRIPTION");
const APP_ICON_PNG: &[u8] = include_bytes!("../../icons/icon_256x256@2x.png");

fn app_icon() -> Arc<Image> {
    static APP_ICON: OnceLock<Arc<Image>> = OnceLock::new();
    APP_ICON.get_or_init(|| Arc::new(Image::from_bytes(ImageFormat::Png, APP_ICON_PNG.to_vec()))).clone()
}

impl Workspace {
    pub fn show_about(&mut self, _: &About, _: &mut Window, cx: &mut Context<Self>) {
        self.about_open = true;
        cx.notify();
    }

    pub fn render_about_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.about_open {
            return None;
        }
        let s = Scale(self.scale());

        let version_pill = div()
            .px(px(12.))
            .py(px(3.))
            .rounded(px(12.))
            .border_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_INPUT)
            .t(s, 12.0)
            .font_weight(FontWeight::MEDIUM)
            .text_color(theme::ACCENT)
            .child(format!("Version {VERSION}"));

        let modal = dialogs::modal_box(380.0)
            .id("about-dialog")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(
                div()
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap(px(10.))
                    .px(px(32.))
                    .pt(px(32.))
                    .pb(px(24.))
                    .child(img(app_icon()).w(px(140.)).h(px(140.)))
                    .child(
                        div()
                            .mt(px(6.))
                            .t(s, 24.0)
                            .font_weight(FontWeight::BOLD)
                            .text_color(theme::TEXT)
                            .child("multidb"),
                    )
                    .child(version_pill)
                    .child(
                        div()
                            .mt(px(4.))
                            .text_center()
                            .t(s, 12.0)
                            .text_color(theme::TEXT_MUTED)
                            .child(DESCRIPTION),
                    ),
            )
            .child(
                div()
                    .flex()
                    .justify_center()
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(dialogs::button(
                        "about-close",
                        s,
                        "Close",
                        Btn::Primary,
                        false,
                        cx.listener(|this, _, _, cx| {
                            this.about_open = false;
                            cx.notify();
                        }),
                    )),
            );

        Some(
            overlay(0.5)
                .id("about-overlay")
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(|this, _, _, cx| {
                        this.about_open = false;
                        cx.notify();
                    }),
                )
                .child(modal)
                .into_any_element(),
        )
    }
}
