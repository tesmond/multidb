use crate::{
    commands::EventEmitter,
    ipc::{self, IpcRequest, UserEvent},
    startup_profile::StartupProfiler,
    state::AppState,
};
use anyhow::{Context, Result};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    sync::{Arc, OnceLock},
};
use tao::{
    dpi::LogicalSize,
    event::{Event, WindowEvent},
    event_loop::{ControlFlow, EventLoopBuilder},
    window::{Icon, WindowBuilder},
};
use wry::{
    http::{header::CONTENT_TYPE, Request, Response},
    WebViewBuilder,
};

const APP_URL: &str = "multidb://localhost/index.html";
include!(concat!(env!("OUT_DIR"), "/embedded_assets.rs"));

pub fn run() -> Result<()> {
    let profiler = StartupProfiler::from_env();
    profiler.mark("run_start");

    install_platform_edit_menu();
    profiler.mark("platform_edit_menu_installed");

    sqlx::any::install_default_drivers();
    profiler.mark("sqlx_drivers_installed");

    let runtime = Arc::new(OnceLock::new());
    profiler.mark("tokio_runtime_deferred");

    let state = Arc::new(AppState::default());

    let dist_dir = frontend_dist_dir();
    profiler.mark_with(
        "frontend_dist_resolved",
        serde_json::json!({
            "path": dist_dir.as_ref().map(|path| path.display().to_string()),
            "embeddedAssets": EMBEDDED_ASSETS.len(),
        }),
    );
    let mut event_loop_builder = EventLoopBuilder::<UserEvent>::with_user_event();
    let event_loop = event_loop_builder.build();
    profiler.mark("event_loop_built");
    let proxy = event_loop.create_proxy();

    let mut window_builder = WindowBuilder::new()
        .with_title("multidb")
        .with_inner_size(LogicalSize::new(1280.0, 800.0))
        .with_min_inner_size(LogicalSize::new(900.0, 600.0))
        .with_background_color((18, 18, 23, 255));
    if let Some(icon) = app_icon() {
        window_builder = window_builder.with_window_icon(Some(icon));
    }
    let window = window_builder
        .build(&event_loop)
        .context("failed to create window")?;
    profiler.mark("window_built");

    let protocol_root = dist_dir.clone();
    let protocol_profiler = profiler.clone();
    let ipc_state = state.clone();
    let ipc_proxy = proxy.clone();
    let profile_proxy = proxy.clone();
    let ipc_profiler = profiler.clone();
    let ipc_runtime = runtime.clone();
    let webview_builder = WebViewBuilder::new()
        .with_custom_protocol(
            "multidb".into(),
            move |_webview_id, request| match asset_response(
                protocol_root.as_deref(),
                request,
                &protocol_profiler,
            ) {
                Ok(response) => response.map(Into::into),
                Err(err) => Response::builder()
                    .status(500)
                    .header(CONTENT_TYPE, "text/plain")
                    .body(err.to_string().into_bytes())
                    .unwrap()
                    .map(Into::into),
            },
        )
        .with_ipc_handler(move |request| {
            let body = request.body().clone();
            let Ok(message) = serde_json::from_str::<IpcRequest>(&body) else {
                let _ = ipc_proxy.send_event(UserEvent::Script(ipc::reject_script(
                    "",
                    "invalid IPC message",
                )));
                return;
            };

            if message.command == "__startup_profile" {
                let profiler = ipc_profiler.clone();
                match profiler.write_report(message.args) {
                    Ok(path) if !path.as_os_str().is_empty() => {
                        eprintln!("startup profile written to {}", path.display());
                    }
                    Ok(_) => {}
                    Err(err) => eprintln!("failed to write startup profile: {err}"),
                }

                if profiler.should_exit_after_profile() {
                    let _ = profile_proxy.send_event(UserEvent::Exit);
                }
                return;
            }

            let state = ipc_state.clone();
            let command_proxy = ipc_proxy.clone();
            let event_proxy = ipc_proxy.clone();
            let emitter: EventEmitter = Arc::new(move |event_name, payload| {
                let _ = event_proxy
                    .send_event(UserEvent::Script(ipc::emit_script(event_name, &payload)));
            });
            let runtime_profiler = ipc_profiler.clone();
            let runtime = ipc_runtime.get_or_init(|| {
                let runtime = build_runtime();
                runtime_profiler.mark("tokio_runtime_built");
                runtime
            });

            runtime.spawn(async move {
                let id = message.id;
                let script =
                    match ipc::dispatch(state, emitter, message.command, message.args).await {
                        Ok(value) => ipc::resolve_script(&id, &value),
                        Err(err) => ipc::reject_script(&id, &err),
                    };
                let _ = command_proxy.send_event(UserEvent::Script(script));
            });
        })
        .with_url(APP_URL)
        .with_background_color((18, 18, 23, 255))
        .with_clipboard(true)
        .with_hotkeys_zoom(false)
        .with_devtools(cfg!(debug_assertions));
    profiler.mark("webview_builder_configured");

    #[cfg(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    ))]
    let webview = webview_builder
        .build(&window)
        .context("failed to create webview")?;

    #[cfg(not(any(
        target_os = "windows",
        target_os = "macos",
        target_os = "ios",
        target_os = "android"
    )))]
    let webview = {
        use tao::platform::unix::WindowExtUnix;
        use wry::WebViewBuilderExtUnix;
        let vbox = window
            .default_vbox()
            .ok_or_else(|| anyhow::anyhow!("failed to get GTK window container"))?;
        webview_builder
            .build_gtk(vbox)
            .context("failed to create webview")?
    };
    profiler.mark("webview_built");

    profiler.mark("event_loop_starting");
    event_loop.run(move |event, _, control_flow| {
        let _window = &window;
        *control_flow = ControlFlow::Wait;

        match event {
            Event::UserEvent(UserEvent::Script(script)) => {
                if let Err(err) = webview.evaluate_script(&script) {
                    eprintln!("failed to evaluate webview script: {err}");
                }
            }
            Event::UserEvent(UserEvent::Exit) => {
                *control_flow = ControlFlow::Exit;
            }
            Event::WindowEvent {
                event: WindowEvent::CloseRequested,
                ..
            } => {
                *control_flow = ControlFlow::Exit;
            }
            _ => {}
        }
    });
}

#[cfg(target_os = "macos")]
fn install_platform_edit_menu() {
    macos::install_edit_menu();
}

#[cfg(not(target_os = "macos"))]
fn install_platform_edit_menu() {}

#[cfg(target_os = "macos")]
mod macos {
    use objc2_app_kit::{NSApplication, NSEventModifierFlags, NSMenu, NSMenuItem};
    use objc2_foundation::{MainThreadMarker, NSSelectorFromString, NSString};

    pub fn install_edit_menu() {
        let Some(mtm) = MainThreadMarker::new() else {
            return;
        };

        let app = NSApplication::sharedApplication(mtm);
        let main_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str(""));
        let app_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("multidb"));
        let edit_menu = NSMenu::initWithTitle(mtm.alloc(), &NSString::from_str("Edit"));

        let app_item = NSMenuItem::new(mtm);
        app_item.setTitle(&NSString::from_str("multidb"));
        app_item.setSubmenu(Some(&app_menu));
        main_menu.addItem(&app_item);

        app_menu.addItem(&menu_item(
            mtm,
            "Quit multidb",
            "terminate:",
            "q",
            NSEventModifierFlags::Command,
        ));

        let edit_item = NSMenuItem::new(mtm);
        edit_item.setTitle(&NSString::from_str("Edit"));
        edit_item.setSubmenu(Some(&edit_menu));
        main_menu.addItem(&edit_item);

        edit_menu.addItem(&menu_item(
            mtm,
            "Undo",
            "undo:",
            "z",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&menu_item(
            mtm,
            "Redo",
            "redo:",
            "z",
            NSEventModifierFlags::Command | NSEventModifierFlags::Shift,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&menu_item(
            mtm,
            "Cut",
            "cut:",
            "x",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&menu_item(
            mtm,
            "Copy",
            "copy:",
            "c",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&menu_item(
            mtm,
            "Paste",
            "paste:",
            "v",
            NSEventModifierFlags::Command,
        ));
        edit_menu.addItem(&NSMenuItem::separatorItem(mtm));
        edit_menu.addItem(&menu_item(
            mtm,
            "Select All",
            "selectAll:",
            "a",
            NSEventModifierFlags::Command,
        ));

        app.setMainMenu(Some(&main_menu));
    }

    fn menu_item(
        mtm: MainThreadMarker,
        title: &str,
        action: &str,
        key: &str,
        modifiers: NSEventModifierFlags,
    ) -> objc2::rc::Retained<NSMenuItem> {
        let item = unsafe {
            NSMenuItem::initWithTitle_action_keyEquivalent(
                mtm.alloc(),
                &NSString::from_str(title),
                Some(NSSelectorFromString(&NSString::from_str(action))),
                &NSString::from_str(key),
            )
        };
        item.setKeyEquivalentModifierMask(modifiers);
        item
    }
}

fn build_runtime() -> tokio::runtime::Runtime {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("multidb-worker")
        .build()
        .expect("failed to start async runtime")
}

fn app_icon() -> Option<Icon> {
    let image =
        image::load_from_memory(include_bytes!("../../build/icon.iconset/icon_256x256.png"))
            .ok()?
            .into_rgba8();
    let (width, height) = image.dimensions();
    Icon::from_rgba(image.into_raw(), width, height).ok()
}

fn frontend_dist_dir() -> Option<PathBuf> {
    if !EMBEDDED_ASSETS.is_empty() {
        return None;
    }

    let mut candidates = Vec::new();
    if let Ok(path) = std::env::var("MULTIDB_DIST_DIR") {
        candidates.push(PathBuf::from(path));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("frontend").join("dist"));
            candidates.push(dir.join("dist"));
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd.join("frontend").join("dist"));
        candidates.push(cwd.join("dist"));
    }
    candidates.push(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../frontend/dist"));

    candidates
        .into_iter()
        .find(|path| path.join("index.html").is_file())
}

fn asset_response(
    root: Option<&Path>,
    request: Request<Vec<u8>>,
    profiler: &StartupProfiler,
) -> Result<Response<Vec<u8>>> {
    let uri_path = request.uri().path();
    let relative = asset_path(uri_path);

    if let Some(bytes) = embedded_asset(relative) {
        profiler.mark_asset(relative, "embedded", bytes.len());
        return Response::builder()
            .header(CONTENT_TYPE, content_type_from_name(relative))
            .body(bytes.to_vec())
            .map_err(Into::into);
    }

    if relative_needs_app_fallback(relative) {
        if let Some(bytes) = embedded_asset("index.html") {
            profiler.mark_asset("index.html", "embedded-fallback", bytes.len());
            return Response::builder()
                .header(CONTENT_TYPE, "text/html; charset=utf-8")
                .body(bytes.to_vec())
                .map_err(Into::into);
        }
    }

    let Some(root) = root else {
        return not_found();
    };

    let root = root.canonicalize()?;
    let path = root.join(relative);
    if let Ok(path) = path.canonicalize() {
        if path.starts_with(&root) && path.is_file() {
            let bytes = std::fs::read(&path)?;
            profiler.mark_asset(relative, "filesystem", bytes.len());
            return Response::builder()
                .header(CONTENT_TYPE, content_type(&path))
                .body(bytes)
                .map_err(Into::into);
        }
    }

    let index = root.join("index.html");
    if relative_needs_app_fallback(relative) && index.is_file() {
        let bytes = std::fs::read(&index).context("read frontend index.html")?;
        profiler.mark_asset("index.html", "filesystem-fallback", bytes.len());
        return Response::builder()
            .header(CONTENT_TYPE, "text/html; charset=utf-8")
            .body(bytes)
            .map_err(Into::into);
    }

    not_found()
}

fn embedded_asset(path: &str) -> Option<&'static [u8]> {
    EMBEDDED_ASSETS
        .iter()
        .find_map(|(name, bytes)| (*name == path).then_some(*bytes))
}

fn asset_path(uri_path: &str) -> &str {
    let relative = uri_path.trim_start_matches('/');
    if relative.is_empty() {
        "index.html"
    } else {
        relative
    }
}

fn relative_needs_app_fallback(relative: &str) -> bool {
    !relative.contains('.') && !relative.starts_with("assets/")
}

fn not_found() -> Result<Response<Vec<u8>>> {
    Response::builder()
        .status(404)
        .header(CONTENT_TYPE, "text/plain")
        .body(b"not found".to_vec())
        .map_err(Into::into)
}

fn content_type(path: &Path) -> &'static str {
    content_type_from_name(path.extension().and_then(|ext| ext.to_str()).unwrap_or(""))
}

fn content_type_from_name(name: &str) -> &'static str {
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or(name);

    match extension {
        "html" => "text/html; charset=utf-8",
        "js" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "wasm" => "application/wasm",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "map" => "application/json",
        _ => "application/octet-stream",
    }
}

#[allow(dead_code)]
fn _keep_value_send_sync(_: Value) {}
