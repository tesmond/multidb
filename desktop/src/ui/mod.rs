//! Native GPUI front end (replaces the Svelte/WebView UI).

pub mod diagram;
pub mod color_panel;
pub mod dialogs;
pub mod editor;
pub mod grid;
pub mod js;
pub mod metrics;
pub mod model;
pub mod nav_index;
mod navigator;
mod output;
pub mod relationship;
mod render;
pub mod runtime;
pub mod sessions;
pub mod sql;
pub mod sql_text;
pub mod storage;
mod tabbar;
pub mod textfmt;
pub mod theme;
pub mod widgets;
pub mod workspace;

use gpui::{
    actions, px, size, App, AppContext, Application, Bounds, KeyBinding, Menu, MenuItem, OsAction, TitlebarOptions,
    WindowBounds, WindowOptions,
};
use workspace::Workspace;

actions!(multidb, [Quit, MenuUndo, MenuRedo, MenuCut, MenuCopy, MenuPaste, MenuSelectAll]);

/// First installed family from the old editor stack
/// (`'JetBrains Mono','Fira Code','Cascadia Code',monospace`), unless
/// `MULTIDB_MONO_FONT` names one.
pub fn pick_mono_family(cx: &App) -> String {
    if let Ok(family) = std::env::var("MULTIDB_MONO_FONT") {
        if !family.trim().is_empty() {
            return family;
        }
    }
    let names = cx.text_system().all_font_names();
    for candidate in ["JetBrains Mono", "Fira Code", "Cascadia Code"] {
        if names.iter().any(|n| n == candidate) {
            return candidate.to_string();
        }
    }
    // WebKit's generic `monospace` on macOS.
    "Courier".to_string()
}

pub fn run() -> anyhow::Result<()> {
    rustls::crypto::aws_lc_rs::default_provider()
        .install_default()
        .map_err(|_| anyhow::anyhow!("failed to install rustls aws-lc crypto provider"))?;
    sqlx::any::install_default_drivers();

    Application::new().run(|cx: &mut App| {
        let mono = pick_mono_family(cx);
        metrics::init(cx, &mono);
        widgets::text_input::bind_keys(cx);
        editor::bind_keys(cx);
        render::bind_keys(cx);
        dialogs::bind_keys(cx);
        cx.on_action(|_: &Quit, cx| cx.quit());
        cx.bind_keys([KeyBinding::new("cmd-q", Quit, None)]);
        cx.set_menus(vec![
            Menu { name: "multidb".into(), items: vec![MenuItem::action("Quit multidb", Quit)] },
            Menu {
                name: "Edit".into(),
                items: vec![
                    MenuItem::os_action("Undo", MenuUndo, OsAction::Undo),
                    MenuItem::os_action("Redo", MenuRedo, OsAction::Redo),
                    MenuItem::separator(),
                    MenuItem::os_action("Cut", MenuCut, OsAction::Cut),
                    MenuItem::os_action("Copy", MenuCopy, OsAction::Copy),
                    MenuItem::os_action("Paste", MenuPaste, OsAction::Paste),
                    MenuItem::separator(),
                    MenuItem::os_action("Select All", MenuSelectAll, OsAction::SelectAll),
                ],
            },
        ]);

        let bounds = Bounds::centered(None, size(px(1280.0), px(800.0)), cx);
        let window = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    window_min_size: Some(size(px(900.0), px(600.0))),
                    titlebar: Some(TitlebarOptions { title: Some("multidb".into()), appears_transparent: false, traffic_light_position: None }),
                    app_id: Some("com.multidb.app".into()),
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Workspace::new(window, cx)),
            )
            .expect("failed to open window");
        window
            .update(cx, |ws, window, cx| {
                window.focus(&ws.focus_handle);
                cx.activate(true);
            })
            .ok();
        cx.on_window_closed(|cx| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
    });
    Ok(())
}
