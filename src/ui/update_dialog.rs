//! Help > "Check for Updates…" dialog. Drives `crate::updater`: check GitHub for
//! a newer release, confirm, download (with progress), install and relaunch.

use crate::ui::dialogs::{self, Btn};
use crate::ui::runtime;
use crate::ui::theme;
use crate::ui::widgets::{overlay, Scale, TextExt};
use crate::ui::workspace::Workspace;
use crate::ui::CheckForUpdates;
use crate::updater::{self, Release};
use gpui::{div, prelude::*, px, relative, AnyElement, Context, FontWeight, MouseButton, Window};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

const NOTES_LIMIT: usize = 700;

#[derive(Clone)]
pub enum UpdateState {
    /// This build can't update itself (dev build, non-macOS).
    Unavailable(String),
    Checking,
    UpToDate,
    Available(Release),
    Downloading { release: Release, received: u64, cancel: Arc<AtomicBool> },
    Installing(Release),
    Failed { message: String, release: Option<Release> },
}

impl UpdateState {
    /// A download or install is underway; the dialog can't simply be dismissed.
    fn busy(&self) -> bool {
        matches!(self, UpdateState::Downloading { .. } | UpdateState::Installing(_))
    }
}

fn megabytes(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_048_576.0)
}

fn short_notes(notes: &str) -> String {
    let notes = notes.trim();
    if notes.chars().count() <= NOTES_LIMIT {
        return notes.to_string();
    }
    let cut: String = notes.chars().take(NOTES_LIMIT).collect();
    format!("{}…", cut.trim_end())
}

impl Workspace {
    /// Close the update dialog. A download in progress is cancelled instead (it
    /// closes the dialog once it stops); an install can't be interrupted.
    /// Returns whether a dialog was open (so Escape knows it handled the key).
    pub fn close_update_dialog(&mut self) -> bool {
        match &self.app_update {
            None => false,
            Some(UpdateState::Downloading { cancel, .. }) => {
                cancel.store(true, Ordering::Relaxed);
                true
            }
            Some(state) if state.busy() => true,
            Some(_) => {
                self.app_update = None;
                true
            }
        }
    }

    pub fn check_for_updates(&mut self, _: &CheckForUpdates, _: &mut Window, cx: &mut Context<Self>) {
        if self.app_update.as_ref().is_some_and(|s| s.busy() || matches!(s, UpdateState::Available(_))) {
            cx.notify();
            return;
        }
        if let Some(reason) = updater::unavailable_reason() {
            self.app_update = Some(UpdateState::Unavailable(reason));
            cx.notify();
            return;
        }
        self.app_update = Some(UpdateState::Checking);
        cx.notify();
        let fut = runtime::spawn_blocking(updater::check);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                // The user may have dismissed the dialog while we waited.
                if !matches!(this.app_update, Some(UpdateState::Checking)) {
                    return;
                }
                this.app_update = Some(match res {
                    Ok(Some(release)) => UpdateState::Available(release),
                    Ok(None) => UpdateState::UpToDate,
                    Err(e) => UpdateState::Failed { message: format!("{e:#}"), release: None },
                });
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    fn start_update(&mut self, release: Release, cx: &mut Context<Self>) {
        let cancel = Arc::new(AtomicBool::new(false));
        self.app_update = Some(UpdateState::Downloading { release: release.clone(), received: 0, cancel: cancel.clone() });
        cx.notify();

        let dir = updater::staging_dir();
        let part = updater::part_path(&dir, &release);
        let done = Arc::new(AtomicBool::new(false));

        // Progress ticker: the download streams into `part`, so its size is the progress.
        let ticker_done = done.clone();
        cx.spawn(async move |this, cx| {
            while !ticker_done.load(Ordering::Relaxed) {
                cx.background_executor().timer(Duration::from_millis(250)).await;
                let size = std::fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
                let alive = this
                    .update(cx, |this, cx| match &mut this.app_update {
                        Some(UpdateState::Downloading { received, .. }) => {
                            *received = size;
                            cx.notify();
                            true
                        }
                        _ => false,
                    })
                    .unwrap_or(false);
                if !alive {
                    break;
                }
            }
        })
        .detach();

        let rel = release.clone();
        let work = dir.clone();
        let download = runtime::spawn_blocking(move || updater::download(&rel, &work, &cancel));
        cx.spawn(async move |this, cx| {
            let downloaded = download.await;
            done.store(true, Ordering::Relaxed);

            let result = match downloaded {
                Ok(zip) => {
                    this.update(cx, |this, cx| {
                        this.app_update = Some(UpdateState::Installing(release.clone()));
                        cx.notify();
                    })
                    .ok();
                    let work = dir.clone();
                    runtime::spawn_blocking(move || updater::install(&zip, &work)).await
                }
                Err(e) => Err(e),
            };

            this.update(cx, |this, cx| match result {
                Ok(bundle) => {
                    updater::relaunch(&bundle, &dir);
                    cx.quit();
                }
                Err(e) => {
                    let _ = std::fs::remove_dir_all(&dir);
                    if e.downcast_ref::<updater::Cancelled>().is_some() {
                        this.app_update = None;
                        cx.notify();
                        return;
                    }
                    this.app_update = Some(UpdateState::Failed { message: format!("{e:#}"), release: Some(release) });
                    cx.notify();
                }
            })
            .ok();
        })
        .detach();
    }

    pub fn render_update_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let state = self.app_update.clone()?;
        let s = Scale(self.scale());
        let current = crate::ui::about::VERSION;

        let text = |msg: String| div().t(s, 13.0).text_color(theme::TEXT_DIM).child(msg);
        let close = |id: &'static str| {
            dialogs::button(
                id,
                s,
                "Close",
                Btn::Secondary,
                false,
                cx.listener(|this, _, _, cx| {
                    this.close_update_dialog();
                    cx.notify();
                }),
            )
        };
        let github = |id: &'static str, url: String| {
            dialogs::button(id, s, "View on GitHub", Btn::Secondary, false, move |_, _, cx| cx.open_url(&url))
        };

        let (title, body, buttons): (&str, Vec<AnyElement>, Vec<AnyElement>) = match &state {
            UpdateState::Unavailable(reason) => ("Updates unavailable", vec![text(reason.clone()).into_any_element()], vec![close("upd-close")]),
            UpdateState::Checking => ("Check for Updates", vec![text("Checking for updates…".into()).into_any_element()], vec![close("upd-close")]),
            UpdateState::UpToDate => (
                "You're up to date",
                vec![text(format!("multidb {current} is the latest version.")).into_any_element()],
                vec![close("upd-close")],
            ),
            UpdateState::Available(release) => {
                let mut body = vec![
                    div()
                        .t(s, 13.0)
                        .text_color(theme::TEXT)
                        .font_weight(FontWeight::MEDIUM)
                        .child(format!("multidb {} is available. You have {current}.", release.version))
                        .into_any_element(),
                    text("multidb will restart to finish updating. Save anything you need first: unsaved tabs may be lost.".into()).into_any_element(),
                ];
                let notes = short_notes(&release.notes);
                if !notes.is_empty() {
                    body.push(
                        div()
                            .p(px(10.))
                            .rounded(px(6.))
                            .border_1()
                            .border_color(theme::BORDER)
                            .bg(theme::BG_INPUT)
                            .t(s, 12.0)
                            .text_color(theme::TEXT_MUTED)
                            .child(notes)
                            .into_any_element(),
                    );
                }
                let rel = release.clone();
                (
                    "Update available",
                    body,
                    vec![
                        close("upd-cancel"),
                        github("upd-github", release.page_url.clone()),
                        dialogs::button(
                            "upd-go",
                            s,
                            "Update & Restart",
                            Btn::Primary,
                            false,
                            cx.listener(move |this, _, _, cx| this.start_update(rel.clone(), cx)),
                        ),
                    ],
                )
            }
            UpdateState::Downloading { release, received, cancel } => {
                let total = release.zip_size;
                let frac = if total > 0 { (*received as f32 / total as f32).clamp(0.0, 1.0) } else { 0.0 };
                let cancel = cancel.clone();
                let label = if total > 0 {
                    format!("Downloading multidb {}: {} of {}", release.version, megabytes(*received), megabytes(total))
                } else {
                    format!("Downloading multidb {}…", release.version)
                };
                (
                    "Updating",
                    vec![
                        text(label).into_any_element(),
                        div()
                            .h(px(6.))
                            .w_full()
                            .rounded(px(3.))
                            .bg(theme::BG_INPUT)
                            .child(div().h_full().w(relative(frac)).rounded(px(3.)).bg(theme::ACCENT))
                            .into_any_element(),
                    ],
                    vec![dialogs::button(
                        "upd-cancel-download",
                        s,
                        "Cancel",
                        Btn::Secondary,
                        false,
                        cx.listener(move |_, _, _, cx| {
                            cancel.store(true, Ordering::Relaxed);
                            cx.notify();
                        }),
                    )],
                )
            }
            UpdateState::Installing(release) => (
                "Updating",
                vec![
                    text(format!("Installing multidb {}…", release.version)).into_any_element(),
                    text("If macOS asks for your password, it's so multidb can replace itself in its install folder.".into()).into_any_element(),
                ],
                vec![],
            ),
            UpdateState::Failed { message, release } => {
                let url = release.as_ref().map(|r| r.page_url.clone()).unwrap_or_else(updater::releases_page);
                (
                    "Update failed",
                    vec![
                        div().t(s, 13.0).text_color(theme::ERROR).child(message.clone()).into_any_element(),
                        text("Your installed version has not been changed.".into()).into_any_element(),
                    ],
                    vec![close("upd-close"), github("upd-github", url)],
                )
            }
        };

        let busy = state.busy();
        let mut footer = div().flex().justify_end().gap(px(8.)).px(px(18.)).py(px(14.)).border_t_1().border_color(theme::BORDER);
        let has_footer = !buttons.is_empty();
        for b in buttons {
            footer = footer.child(b);
        }

        let mut modal = dialogs::modal_box(440.0)
            .id("update-dialog")
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(div().px(px(18.)).py(px(14.)).border_b_1().border_color(theme::BORDER).child(div().t(s, 15.0).font_weight(FontWeight::BOLD).child(title)))
            .child(div().flex().flex_col().gap(px(10.)).px(px(18.)).py(px(16.)).children(body));
        if has_footer {
            modal = modal.child(footer);
        }

        Some(
            overlay(0.5)
                .id("update-overlay")
                .occlude()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        if !busy {
                            this.close_update_dialog();
                            cx.notify();
                        }
                    }),
                )
                .child(modal)
                .into_any_element(),
        )
    }
}
