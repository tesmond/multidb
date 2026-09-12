//! Modal dialogs: connection manager, import, save/edit query title, new
//! server group, terminate confirmation.

use crate::commands;
use crate::models::ConnectionConfig;
use crate::ui::model::{new_id, ActiveConnection};
use crate::ui::runtime;
use crate::ui::theme::{self, hsla, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{shadow, Scale, TextExt};
use crate::ui::workspace::Workspace;
use gpui::{
    div, prelude::*, px, AnyElement, App, ClickEvent, Context, Entity, FontWeight, MouseButton, SharedString,
    Subscription, Window,
};

pub fn nav_filter_look(scale: f32) -> InputLook {
    let fs = 12.0 * scale;
    let lh = crate::ui::metrics::line_height_normal(fs);
    InputLook {
        font_size: fs,
        line_height: lh,
        height: 28.0,
        pad_x: (8.0, 50.0),
        ..InputLook::dialog(scale)
    }
}

pub fn font_scale_look(scale: f32) -> InputLook {
    let fs = 12.0 * scale;
    let lh = crate::ui::metrics::line_height_normal(fs);
    InputLook { font_size: fs, line_height: lh, height: 26.0, pad_x: (6.0, 6.0), ..InputLook::dialog(scale) }
}

pub fn save_title_look(scale: f32) -> InputLook {
    let mut l = InputLook::dialog(scale);
    l.pad_x = (12.0, 12.0);
    l.height = l.line_height + 16.0 + 2.0;
    l.focus_ring = Some((3.0, theme::rgba8(88, 166, 255, 0.1)));
    l
}

pub fn rescale_inputs(ws: &mut Workspace, scale: f32, cx: &mut Context<Workspace>) {
    ws.nav_filter.update(cx, |i, cx| {
        i.look = nav_filter_look(scale);
        cx.notify()
    });
    ws.font_scale_input.update(cx, |i, cx| {
        i.look = font_scale_look(scale);
        cx.notify()
    });
    if let Some(d) = &ws.conn_dialog {
        for input in d.inputs() {
            input.update(cx, |i, cx| {
                let masked = i.masked;
                let number = i.number;
                i.look = InputLook::dialog(scale);
                i.masked = masked;
                i.number = number;
                cx.notify()
            });
        }
    }
}

fn input(cx: &mut App, scale: f32, text: &str, placeholder: &str) -> Entity<TextInput> {
    let text = text.to_string();
    let placeholder = placeholder.to_string();
    cx.new(|cx| {
        let mut t = TextInput::new(cx, InputLook::dialog(scale)).with_placeholder(placeholder);
        t.set_text(text, cx);
        t
    })
}

// ─── Server group ───────────────────────────────────────────────────────────

pub struct ServerGroupDialog {
    pub input: Entity<TextInput>,
    pub error: String,
    _sub: Subscription,
}

impl ServerGroupDialog {
    pub fn new(scale: f32, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let input = input(cx, scale, "", "Production");
        let sub = cx.subscribe_in(&input, window, |ws, _i, ev: &InputEvent, _w, cx| {
            if matches!(ev, InputEvent::Submit) {
                ws.save_server_group(cx);
            }
        });
        input.update(cx, |i, _| i.focus(window));
        ServerGroupDialog { input, error: String::new(), _sub: sub }
    }
}

// ─── Save / edit title ──────────────────────────────────────────────────────

pub enum TitleDialogKind {
    Save { conn_id: String, sql: String },
    Edit { id: i64, current: String },
}

pub struct TitleDialog {
    pub kind: TitleDialogKind,
    pub input: Entity<TextInput>,
    _sub: Subscription,
}

impl TitleDialog {
    pub fn new(kind: TitleDialogKind, initial: &str, scale: f32, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let placeholder = match kind {
            TitleDialogKind::Save { .. } => "e.g., Get all active users",
            TitleDialogKind::Edit { .. } => "",
        };
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, save_title_look(scale)).with_placeholder(placeholder);
            t.set_text(initial.to_string(), cx);
            t
        });
        let sub = cx.subscribe_in(&input, window, |ws, _i, ev: &InputEvent, _w, cx| match ev {
            InputEvent::Submit => ws.title_dialog_save(cx),
            InputEvent::Cancel => {
                ws.title_dialog = None;
                cx.notify();
            }
            InputEvent::Changed => cx.notify(),
            _ => {}
        });
        input.update(cx, |i, _| i.focus(window));
        TitleDialog { kind, input, _sub: sub }
    }
}

// ─── Import ─────────────────────────────────────────────────────────────────

pub struct ImportDialog {
    pub conn_id: String,
    pub import_type: String,
    pub path: Entity<TextInput>,
    pub error: String,
    pub importing: bool,
    pub select_open: bool,
}

impl ImportDialog {
    pub fn new(conn_id: String, scale: f32, _window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let path = cx.new(|cx| {
            let mut t = TextInput::new(cx, InputLook::dialog(scale)).with_placeholder("No file selected");
            t.readonly = true;
            t
        });
        ImportDialog { conn_id, import_type: "zipped-sql".into(), path, error: String::new(), importing: false, select_open: false }
    }
}

// ─── Connection dialog ──────────────────────────────────────────────────────

pub const DEFAULT_TAB_COLOR: &str = "#6366f1";

pub struct ConnectionDialog {
    pub editing: Option<ConnectionConfig>,
    pub id: String,
    pub driver: String,
    pub auth_mode: String,
    pub has_saved_password: bool,
    pub tab_text_black: bool,
    pub use_kube: bool,
    pub name: Entity<TextInput>,
    pub tab_color: Entity<TextInput>,
    pub host: Entity<TextInput>,
    pub port: Entity<TextInput>,
    pub username: Entity<TextInput>,
    pub password: Entity<TextInput>,
    pub aws_region: Entity<TextInput>,
    pub aws_profile: Entity<TextInput>,
    pub ssl_ca_path: Entity<TextInput>,
    pub database: Entity<TextInput>,
    pub dsn: String,
    pub kubectl_path: Entity<TextInput>,
    pub kube_context: Entity<TextInput>,
    pub kube_namespace: Entity<TextInput>,
    pub kube_resource: Entity<TextInput>,
    pub kube_local_port: Entity<TextInput>,
    pub kube_remote_port: Entity<TextInput>,
    pub testing: bool,
    pub stopping: bool,
    pub active_test_id: String,
    pub stop_requested: bool,
    pub saving: bool,
    pub test_result: String,
    pub test_error: String,
    pub open_select: Option<&'static str>,
    _subs: Vec<Subscription>,
}

fn is_aws_iam(mode: &str) -> bool {
    mode.chars().filter(|c| !matches!(c, '_' | ' ' | '-')).collect::<String>().to_lowercase() == "awsiam"
}

fn port_text(p: i32) -> String {
    p.to_string()
}

impl ConnectionDialog {
    pub fn new(editing: Option<ConnectionConfig>, scale: f32, window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let f = editing.clone().unwrap_or_else(|| ConnectionConfig {
            id: new_id(),
            driver: "mysql".into(),
            host: "localhost".into(),
            port: 3306,
            auth_mode: "password".into(),
            ..Default::default()
        });
        let num = |cx: &mut App, v: i32| {
            let e = input(cx, scale, &port_text(v), "");
            e.update(cx, |i, _| i.number = Some((1.0, 65535.0, 1.0)));
            e
        };
        let password = input(cx, scale, &f.password, "");
        password.update(cx, |i, _| i.masked = true);
        let name = input(cx, scale, &f.name, "My Database");
        let mut d = ConnectionDialog {
            editing: editing.clone(),
            id: f.id.clone(),
            driver: f.driver.clone(),
            auth_mode: if is_aws_iam(&f.auth_mode) { "awsIam".into() } else if f.auth_mode.is_empty() { "password".into() } else { f.auth_mode.clone() },
            has_saved_password: f.has_saved_password,
            tab_text_black: f.tab_text_black,
            use_kube: f.use_kube_port_forward,
            tab_color: input(cx, scale, &f.tab_color, "Default"),
            host: input(cx, scale, &f.host, "localhost"),
            port: num(cx, f.port),
            username: input(cx, scale, &f.username, ""),
            password,
            aws_region: input(cx, scale, &f.aws_region, "us-east-1"),
            aws_profile: input(cx, scale, &f.aws_profile, "default"),
            ssl_ca_path: input(cx, scale, &f.ssl_ca_path, "Optional PEM bundle"),
            database: input(cx, scale, &f.database, ""),
            dsn: f.dsn.clone(),
            kubectl_path: input(cx, scale, &f.kubectl_path, "kubectl (use PATH)"),
            kube_context: input(cx, scale, &f.kube_context, "my-cluster"),
            kube_namespace: input(cx, scale, &f.kube_namespace, "default"),
            kube_resource: input(cx, scale, &f.kube_resource, "service/postgres"),
            kube_local_port: num(cx, f.kube_local_port),
            kube_remote_port: num(cx, f.kube_remote_port),
            name,
            testing: false,
            stopping: false,
            active_test_id: String::new(),
            stop_requested: false,
            saving: false,
            test_result: String::new(),
            test_error: String::new(),
            open_select: None,
            _subs: Vec::new(),
        };
        d.update_database_placeholder(cx);
        for i in d.inputs() {
            d._subs.push(cx.subscribe_in(&i, window, |_ws, _i, ev: &InputEvent, _w, cx| {
                if matches!(ev, InputEvent::Changed) {
                    cx.notify();
                }
            }));
        }
        let _ = ActiveConnection::new;
        d
    }

    pub fn inputs(&self) -> Vec<Entity<TextInput>> {
        vec![
            self.name.clone(),
            self.tab_color.clone(),
            self.host.clone(),
            self.port.clone(),
            self.username.clone(),
            self.password.clone(),
            self.aws_region.clone(),
            self.aws_profile.clone(),
            self.ssl_ca_path.clone(),
            self.database.clone(),
            self.kubectl_path.clone(),
            self.kube_context.clone(),
            self.kube_namespace.clone(),
            self.kube_resource.clone(),
            self.kube_local_port.clone(),
            self.kube_remote_port.clone(),
        ]
    }

    pub fn update_database_placeholder(&self, cx: &mut App) {
        let ph = if self.driver == "sqlite" {
            "C:\\\\path\\\\to\\\\file.db or :memory:"
        } else {
            "Optional (leave blank to browse all databases)"
        };
        self.database.update(cx, |i, _| i.set_placeholder(ph));
    }

    pub fn aws_iam_selected(&self) -> bool {
        self.driver == "mysql" && is_aws_iam(&self.auth_mode)
    }

    pub fn form(&self, cx: &App) -> ConnectionConfig {
        let t = |e: &Entity<TextInput>| e.read(cx).text().to_string();
        let n = |e: &Entity<TextInput>| e.read(cx).text().trim().parse::<i32>().unwrap_or(0);
        ConnectionConfig {
            id: self.id.clone(),
            name: t(&self.name),
            driver: self.driver.clone(),
            tab_color: t(&self.tab_color),
            tab_text_black: self.tab_text_black,
            host: t(&self.host),
            port: n(&self.port),
            username: t(&self.username),
            password: t(&self.password),
            has_saved_password: self.has_saved_password,
            auth_mode: self.auth_mode.clone(),
            database: t(&self.database),
            dsn: self.dsn.clone(),
            aws_region: t(&self.aws_region),
            aws_profile: t(&self.aws_profile),
            ssl_ca_path: t(&self.ssl_ca_path),
            use_kube_port_forward: self.use_kube,
            kubectl_path: t(&self.kubectl_path),
            kube_context: t(&self.kube_context),
            kube_namespace: t(&self.kube_namespace),
            kube_resource: t(&self.kube_resource),
            kube_local_port: n(&self.kube_local_port),
            kube_remote_port: n(&self.kube_remote_port),
        }
    }

    pub fn validate(&self, require_name: bool, cx: &App) -> String {
        let f = self.form(cx);
        if require_name && f.name.trim().is_empty() {
            return "Name is required".into();
        }
        if f.driver == "sqlite" && f.database.trim().is_empty() {
            return "SQLite database path is required".into();
        }
        if f.driver != "sqlite" {
            if f.host.trim().is_empty() {
                return "Host is required".into();
            }
            if f.username.trim().is_empty() {
                return "Username is required".into();
            }
        }
        if self.aws_iam_selected() {
            if f.aws_region.trim().is_empty() {
                return "AWS region is required for IAM authentication".into();
            }
            if f.use_kube_port_forward {
                return "AWS IAM authentication is not supported with Kubernetes port forwarding".into();
            }
        }
        String::new()
    }
}

pub fn driver_default_port(driver: &str) -> i32 {
    match driver {
        "mysql" => 3306,
        "postgres" => 5432,
        _ => 0,
    }
}

pub fn normalize_hex_color(value: &str) -> String {
    let v = value.trim();
    if v.len() == 7 && v.starts_with('#') && v[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        v.to_lowercase()
    } else {
        String::new()
    }
}

pub fn parse_hex(value: &str) -> Option<Rgba> {
    let n = normalize_hex_color(value);
    if n.is_empty() {
        return None;
    }
    u32::from_str_radix(&n[1..], 16).ok().map(theme::hex)
}

fn connection_target_changed(prev: Option<&ConnectionConfig>, next: &ConnectionConfig) -> bool {
    let Some(p) = prev else { return true };
    p.driver != next.driver
        || p.host != next.host
        || p.port != next.port
        || p.username != next.username
        || p.password != next.password
        || p.auth_mode != next.auth_mode
        || p.database != next.database
        || p.dsn != next.dsn
        || p.aws_region != next.aws_region
        || p.aws_profile != next.aws_profile
        || p.ssl_ca_path != next.ssl_ca_path
        || p.use_kube_port_forward != next.use_kube_port_forward
        || p.kubectl_path != next.kubectl_path
        || p.kube_context != next.kube_context
        || p.kube_namespace != next.kube_namespace
        || p.kube_resource != next.kube_resource
        || p.kube_local_port != next.kube_local_port
        || p.kube_remote_port != next.kube_remote_port
}

fn infer_connection_name(path: &str) -> String {
    let filename = path.rsplit(['/', '\\']).next().unwrap_or(path).trim();
    let lower = filename.to_lowercase();
    for ext in [".sqlite", ".sqlite3", ".db", ".db3"] {
        if lower.ends_with(ext) && filename.len() > ext.len() {
            return filename[..filename.len() - ext.len()].to_string();
        }
    }
    filename.to_string()
}

// ─── Workspace actions for dialogs ──────────────────────────────────────────

impl Workspace {
    pub fn save_server_group(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.server_group_dialog else { return };
        let title = d.input.read(cx).text().trim().to_string();
        if title.is_empty() {
            d.error = "Title is required".into();
            cx.notify();
            return;
        }
        let id = self.add_server_group(&title);
        self.expanded_groups.insert(id);
        self.server_group_dialog = None;
        cx.notify();
    }

    pub fn title_dialog_save(&mut self, cx: &mut Context<Self>) {
        let Some(d) = self.title_dialog.take() else { return };
        let title = d.input.read(cx).text().to_string();
        match d.kind {
            TitleDialogKind::Save { conn_id, sql } => {
                if title.trim().is_empty() {
                    self.title_dialog = Some(TitleDialog { kind: TitleDialogKind::Save { conn_id, sql }, ..d });
                    return;
                }
                self.save_query(conn_id, title, sql, cx);
            }
            TitleDialogKind::Edit { id, current } => {
                let trimmed = title.trim().to_string();
                if trimmed.is_empty() {
                    self.title_dialog = Some(TitleDialog { kind: TitleDialogKind::Edit { id, current }, ..d });
                    return;
                }
                if trimmed != current {
                    let state = runtime::state();
                    let fut = runtime::spawn(async move { commands::update_saved_query_title(state, id, trimmed).await });
                    cx.spawn(async move |this, cx| {
                        let _ = fut.await;
                        this.update(cx, |this, cx| this.load_saved_queries(cx)).ok();
                    })
                    .detach();
                }
            }
        }
        cx.notify();
    }

    pub fn conn_dialog_driver_changed(&mut self, driver: &str, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        d.driver = driver.to_string();
        let port = driver_default_port(driver).to_string();
        d.port.update(cx, |i, cx| i.set_text(port, cx));
        if driver != "mysql" {
            d.auth_mode = "password".into();
            d.password.update(cx, |i, cx| i.set_text("", cx));
            d.has_saved_password = false;
        }
        d.update_database_placeholder(cx);
        cx.notify();
    }

    pub fn conn_dialog_auth_changed(&mut self, mode: &str, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        d.auth_mode = if is_aws_iam(mode) { "awsIam".into() } else { "password".into() };
        if d.aws_iam_selected() {
            d.password.update(cx, |i, cx| i.set_text("", cx));
            d.has_saved_password = false;
            d.use_kube = false;
        }
        cx.notify();
    }

    pub fn conn_dialog_toggle_kube(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        if d.aws_iam_selected() {
            return;
        }
        d.use_kube = !d.use_kube;
        if d.use_kube {
            let port = d.port.read(cx).text().to_string();
            let port_n: i32 = port.trim().parse().unwrap_or(0);
            let remote: i32 = d.kube_remote_port.read(cx).text().trim().parse().unwrap_or(0);
            let local: i32 = d.kube_local_port.read(cx).text().trim().parse().unwrap_or(0);
            if remote == 0 {
                d.kube_remote_port.update(cx, |i, cx| i.set_text(port_n.to_string(), cx));
            }
            if local == 0 {
                d.kube_local_port.update(cx, |i, cx| i.set_text(port_n.to_string(), cx));
            }
        }
        cx.notify();
    }

    pub fn conn_dialog_browse_sqlite(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.conn_dialog {
            d.test_result.clear();
            d.test_error.clear();
        }
        let fut = runtime::spawn_blocking(commands::select_sqlite_file);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                let Some(d) = &mut this.conn_dialog else { return };
                match res {
                    Ok(path) if !path.is_empty() => {
                        d.database.update(cx, |i, cx| i.set_text(path.clone(), cx));
                        if d.name.read(cx).text().trim().is_empty() {
                            let n = infer_connection_name(&path);
                            d.name.update(cx, |i, cx| i.set_text(n, cx));
                        }
                    }
                    Ok(_) => {}
                    Err(e) => d.test_error = e,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn conn_dialog_browse_kubectl(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &mut self.conn_dialog {
            d.test_result.clear();
            d.test_error.clear();
        }
        let fut = runtime::spawn_blocking(commands::select_kubectl_executable);
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                let Some(d) = &mut this.conn_dialog else { return };
                match res {
                    Ok(path) if !path.is_empty() => d.kubectl_path.update(cx, |i, cx| i.set_text(path, cx)),
                    Ok(_) => {}
                    Err(e) => d.test_error = e,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn conn_dialog_test(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        d.test_result.clear();
        d.test_error.clear();
        let err = d.validate(false, cx);
        if !err.is_empty() {
            d.test_error = err;
            cx.notify();
            return;
        }
        d.testing = true;
        d.stopping = false;
        d.stop_requested = false;
        let test_id = new_id();
        d.active_test_id = test_id.clone();
        let cfg = d.form(cx);
        cx.notify();
        let fut = crate::ui::workspace::run_test_connection(cfg, test_id.clone());
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                let Some(d) = &mut this.conn_dialog else { return };
                match res {
                    Ok(()) => d.test_result = "Connection successful!".into(),
                    Err(e) => d.test_error = if d.stop_requested { "Connection test cancelled".into() } else { e },
                }
                if d.active_test_id == test_id {
                    d.active_test_id.clear();
                    d.testing = false;
                    d.stopping = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn conn_dialog_stop_test(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        if d.active_test_id.is_empty() || d.stopping {
            return;
        }
        d.stopping = true;
        d.stop_requested = true;
        let id = d.active_test_id.clone();
        let state = runtime::state();
        let fut = runtime::spawn(async move { commands::cancel_test_connection(state, id).await });
        cx.notify();
        cx.spawn(async move |this, cx| {
            if let Err(e) = fut.await {
                this.update(cx, |this, cx| {
                    if let Some(d) = &mut this.conn_dialog {
                        d.test_error = e;
                        d.stopping = false;
                    }
                    cx.notify();
                })
                .ok();
            }
        })
        .detach();
    }

    pub fn conn_dialog_save(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.conn_dialog else { return };
        let err = d.validate(true, cx);
        if !err.is_empty() {
            d.test_error = err;
            cx.notify();
            return;
        }
        d.saving = true;
        d.test_error.clear();
        let form = d.form(cx);
        let previous = d.editing.clone();
        let is_new = previous.is_none();
        cx.notify();
        let state = runtime::state();
        let f2 = form.clone();
        let fut = runtime::spawn(async move { commands::save_and_connect(state, f2).await });
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        let refresh = connection_target_changed(previous.as_ref(), &form);
                        if let Some(c) = this.connections.iter_mut().find(|c| c.config.id == form.id) {
                            c.config = form.clone();
                            if refresh {
                                c.schema = None;
                                c.schema_loading = false;
                                c.schema_error = None;
                            }
                        } else {
                            this.connections.push(ActiveConnection::new(form.clone()));
                        }
                        this.persist_connection_order();
                        if this.selected_conn_id.is_empty() {
                            this.selected_conn_id = form.id.clone();
                        }
                        if is_new && !this.active_server_group_id.is_empty() {
                            let g = this.active_server_group_id.clone();
                            this.add_connection_to_group(&form.id, &g);
                        }
                        this.status = format!("Saved {}", form.name).into();
                        this.conn_dialog = None;
                        this.configure_editors(cx);
                    }
                    Err(e) => {
                        if let Some(d) = &mut this.conn_dialog {
                            d.test_error = e;
                        }
                    }
                }
                if let Some(d) = &mut this.conn_dialog {
                    d.saving = false;
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn conn_dialog_close(&mut self, cx: &mut Context<Self>) {
        if let Some(d) = &self.conn_dialog {
            if !d.active_test_id.is_empty() {
                let state = runtime::state();
                let id = d.active_test_id.clone();
                runtime::runtime().spawn(async move {
                    let _ = commands::cancel_test_connection(state, id).await;
                });
            }
        }
        self.conn_dialog = None;
        cx.notify();
    }

    pub fn import_browse(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.import_dialog else { return };
        d.error.clear();
        let kind = d.import_type.clone();
        let fut = runtime::spawn_blocking(move || commands::select_import_file(kind));
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                let Some(d) = &mut this.import_dialog else { return };
                match res {
                    Ok(p) if !p.is_empty() => d.path.update(cx, |i, cx| i.set_text(p, cx)),
                    Ok(_) => {}
                    Err(e) => d.error = e,
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }

    pub fn import_run(&mut self, cx: &mut Context<Self>) {
        let Some(d) = &mut self.import_dialog else { return };
        d.error.clear();
        let path = d.path.read(cx).text().to_string();
        if path.is_empty() {
            d.error = "Please choose a file to import.".into();
            cx.notify();
            return;
        }
        d.importing = true;
        let (conn, kind) = (d.conn_id.clone(), d.import_type.clone());
        let state = runtime::state();
        let c2 = conn.clone();
        let fut = runtime::spawn(async move { commands::import_table(state, c2, kind, path).await });
        cx.notify();
        cx.spawn(async move |this, cx| {
            let res = fut.await;
            this.update(cx, |this, cx| {
                match res {
                    Ok(()) => {
                        this.status = "Import completed".into();
                        this.request_schema_refresh(&conn, cx);
                        this.import_dialog = None;
                    }
                    Err(e) => {
                        if let Some(d) = &mut this.import_dialog {
                            d.error = e;
                            d.importing = false;
                        }
                    }
                }
                cx.notify();
            })
            .ok();
        })
        .detach();
    }
}

// ─── Shared modal rendering pieces ──────────────────────────────────────────

/// `.modal-header` with an h2 title and the ✕ close button.
pub fn modal_header(s: Scale, title: &str, on_close: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static) -> AnyElement {
    div()
        .flex()
        .items_center()
        .justify_between()
        .px(px(20.))
        .py(px(16.))
        .border_b_1()
        .border_color(theme::BORDER)
        .child(div().t(s, 16.0).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT).child(title.to_string()))
        .child(
            div()
                .id("modal-close")
                .px(px(8.))
                .py(px(4.))
                .t(s, 16.0)
                .text_color(theme::TEXT_MUTED)
                .cursor_pointer()
                .hover(|st| st.text_color(theme::TEXT))
                .on_click(on_close)
                .child("✕"),
        )
        .into_any_element()
}

pub fn modal_box(width: f32) -> gpui::Div {
    div()
        .w(px(width))
        .bg(theme::BG_SURFACE)
        .border_1()
        .border_color(theme::BORDER)
        .rounded(px(8.))
        .shadow(vec![shadow(0.0, 8.0, 32.0, 0.0, theme::rgba8(0, 0, 0, 0.5))])
        .text_color(theme::TEXT)
}

pub fn field_label(s: Scale, text: &str) -> gpui::Div {
    div().t(s, 12.0).text_color(theme::TEXT_MUTED).child(text.to_string())
}

#[derive(Clone, Copy, PartialEq)]
pub enum Btn {
    Primary,
    Secondary,
    Stop,
}

pub fn button(
    id: impl Into<SharedString>,
    s: Scale,
    label: &str,
    kind: Btn,
    disabled: bool,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let (bg, fg, border) = match kind {
        Btn::Primary => (theme::ACCENT, theme::WHITE, theme::ACCENT),
        Btn::Secondary => (theme::BG_INPUT, theme::TEXT, theme::BORDER),
        Btn::Stop => (theme::TRANSPARENT, theme::ERROR, theme::ERROR),
    };
    let mut b = div()
        .id(gpui::ElementId::Name(id.into()))
        .px(px(16.))
        .py(px(7.))
        .rounded(px(4.))
        .border_1()
        .border_color(border)
        .bg(bg)
        .text_color(fg)
        .t(s, 13.0)
        .flex()
        .items_center()
        .justify_center()
        .child(label.to_string());
    if disabled {
        b = b.opacity(0.5);
    } else {
        b = match kind {
            Btn::Primary => b.hover(|st| st.bg(theme::ACCENT_HOVER)),
            Btn::Secondary => b.hover(|st| st.border_color(theme::ACCENT)),
            Btn::Stop => b.hover(|st| st.bg(theme::rgba8(248, 113, 113, 0.12))),
        };
        b = b.cursor_pointer().on_click(on_click);
    }
    b.on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation()).into_any_element()
}

pub fn color_of(c: Rgba) -> gpui::Hsla {
    hsla(c)
}

// ─── Connection dialog rendering ────────────────────────────────────────────

impl Workspace {
    pub fn render_conn_dialog(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let d = self.conn_dialog.as_ref()?;
        let s = Scale(self.scale());
        let title = if d.editing.is_some() { "Edit Connection" } else { "New Connection" };
        let driver = d.driver.clone();
        let aws = d.aws_iam_selected();
        let field = |label: &str, input: Entity<TextInput>| {
            div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.)).child(field_label(s, label)).child(input)
        };
        let two = |a: gpui::Div, b: gpui::Div| div().flex().gap(px(12.)).child(a).child(b);
        let help = |parts: Vec<(&'static str, bool)>| {
            let mut row = div().flex().flex_wrap().t(s, 11.0).line_height(px(11.0 * s.0 * 1.4)).text_color(theme::TEXT_MUTED);
            for (text, code) in parts {
                row = row.child(if code {
                    div().font_family("SF Mono").text_color(theme::TEXT).child(text)
                } else {
                    div().child(text)
                });
            }
            row
        };
        let color_text = d.tab_color.read(cx).text().to_string();
        let picker = parse_hex(&color_text).unwrap_or_else(|| parse_hex(DEFAULT_TAB_COLOR).unwrap());
        let driver_label = match driver.as_str() {
            "postgres" => "PostgreSQL",
            "sqlite" => "SQLite",
            _ => "MySQL",
        };
        let auth_label = if is_aws_iam(&d.auth_mode) { "AWS IAM" } else { "Password" };
        let open = d.open_select;
        let auth_mode = d.auth_mode.clone();

        let mut body = div().p(px(20.)).flex().flex_col().gap(px(12.));
        body = body.child(two(
            field("Connection Name", d.name.clone()),
            div()
                .flex()
                .flex_col()
                .gap(px(6.))
                .flex_1()
                .min_w(px(0.))
                .child(field_label(s, "Tab Colour"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(px(8.))
                        .child(div().flex_1().min_w(px(0.)).child(d.tab_color.clone()))
                        .child(
                            div()
                                .id("color-well")
                                .w(px(36.))
                                .h(px(32.))
                                .flex_shrink_0()
                                .rounded(px(4.))
                                .border_1()
                                .border_color(theme::hex(0x767676))
                                .bg(theme::hex(0xefefef))
                                .p(px(4.))
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, window, cx| this.open_color_panel(window, cx)))
                                .child(div().size_full().rounded(px(2.)).bg(picker).border_1().border_color(theme::hex(0x777777))),
                        )
                        .child(
                            div()
                                .id("clear-color")
                                .h(px(32.))
                                .px(px(10.))
                                .flex()
                                .items_center()
                                .border_1()
                                .border_color(theme::BORDER)
                                .rounded(px(4.))
                                .bg(theme::BG_INPUT)
                                .text_color(theme::TEXT)
                                .t(s, 12.0)
                                .cursor_pointer()
                                .hover(|st| st.border_color(theme::ACCENT))
                                .on_click(cx.listener(|this, _, _, cx| {
                                    if let Some(d) = &this.conn_dialog {
                                        d.tab_color.update(cx, |i, cx| i.set_text("", cx));
                                    }
                                    cx.notify();
                                }))
                                .child("Clear"),
                        ),
                ),
        ));
        let driver_select = crate::ui::navigator::native_select(
            "driver-select",
            s,
            driver_label,
            open == Some("driver"),
            vec![("mysql", "MySQL"), ("postgres", "PostgreSQL"), ("sqlite", "SQLite")],
            &driver,
            cx.listener(|this, _, _, cx| {
                if let Some(d) = &mut this.conn_dialog {
                    d.open_select = if d.open_select == Some("driver") { None } else { Some("driver") };
                }
                cx.notify();
            }),
            cx.listener(|this, v: &String, _, cx| {
                if let Some(d) = &mut this.conn_dialog {
                    d.open_select = None;
                }
                let v = v.clone();
                this.conn_dialog_driver_changed(&v, cx);
            }),
        );
        let auth = if driver == "mysql" {
            div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.)).child(field_label(s, "Authentication")).child(crate::ui::navigator::native_select(
                "auth-select",
                s,
                auth_label,
                open == Some("auth"),
                vec![("password", "Password"), ("awsIam", "AWS IAM")],
                &auth_mode,
                cx.listener(|this, _, _, cx| {
                    if let Some(d) = &mut this.conn_dialog {
                        d.open_select = if d.open_select == Some("auth") { None } else { Some("auth") };
                    }
                    cx.notify();
                }),
                cx.listener(|this, v: &String, _, cx| {
                    if let Some(d) = &mut this.conn_dialog {
                        d.open_select = None;
                    }
                    let v = v.clone();
                    this.conn_dialog_auth_changed(&v, cx);
                }),
            ))
        } else {
            div().flex_1()
        };
        body = body.child(two(div().flex().flex_col().gap(px(4.)).flex_1().min_w(px(0.)).child(field_label(s, "Driver")).child(driver_select), auth));

        if driver != "sqlite" {
            body = body.child(two(field("Host", d.host.clone()), field("Port", d.port.clone())));
            if aws {
                body = body
                    .child(two(field("AWS Region", d.aws_region.clone()), field("AWS Profile", d.aws_profile.clone())))
                    .child(field("TLS CA Bundle Path", d.ssl_ca_path.clone()))
                    .child(field("Database User", d.username.clone()));
            } else {
                body = body.child(two(field("Username", d.username.clone()), field("Password", d.password.clone())));
            }
            body = body.child(field("Database (optional)", d.database.clone()));
        } else {
            body = body.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(field_label(s, "Database File Path"))
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.))
                            .child(div().flex_1().min_w(px(0.)).child(d.database.clone()))
                            .child(button("browse-sqlite", s, "Browse…", Btn::Secondary, false, cx.listener(|this, _, _, cx| this.conn_dialog_browse_sqlite(cx)))),
                    )
                    .child(help(vec![
                        ("Choose an existing SQLite database file, or enter ", false),
                        (":memory:", true),
                        (" for an in-memory database.", false),
                    ])),
            );
        }
        if driver != "sqlite" {
            let checked = d.use_kube;
            body = body.child(
                div()
                    .id("kube-toggle")
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .t(s, 12.0)
                    .text_color(theme::TEXT_MUTED)
                    .cursor_pointer()
                    .when(aws, |d| d.opacity(1.0))
                    .on_click(cx.listener(|this, _, _, cx| this.conn_dialog_toggle_kube(cx)))
                    .child(checkbox(checked, aws))
                    .child("Use Kubernetes port forwarding"),
            );
            if checked {
                body = body.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .p(px(12.))
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(4.))
                        .bg(theme::BG_INPUT)
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .gap(px(4.))
                                .child(field_label(s, "kubectl Executable"))
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap(px(8.))
                                        .child(div().flex_1().min_w(px(0.)).child(d.kubectl_path.clone()))
                                        .child(button("browse-kubectl", s, "Browse…", Btn::Secondary, false, cx.listener(|this, _, _, cx| this.conn_dialog_browse_kubectl(cx)))),
                                )
                                .child(help(vec![("Leave blank to find ", false), ("kubectl", true), (" using the application PATH.", false)])),
                        )
                        .child(two(field("Context", d.kube_context.clone()), field("Namespace", d.kube_namespace.clone())))
                        .child(field("Target", d.kube_resource.clone()))
                        .child(two(field("Local Port", d.kube_local_port.clone()), field("Remote Port", d.kube_remote_port.clone()))),
                );
            }
        }
        if !d.test_result.is_empty() {
            body = body.child(div().t(s, 12.0).text_color(theme::SUCCESS).child(d.test_result.clone()));
        }
        if !d.test_error.is_empty() {
            body = body.child(div().t(s, 12.0).text_color(theme::ERROR).child(d.test_error.clone()));
        }
        let (testing, stopping, saving) = (d.testing, d.stopping, d.saving);
        let footer = div()
            .flex()
            .justify_end()
            .gap(px(8.))
            .px(px(20.))
            .py(px(16.))
            .border_t_1()
            .border_color(theme::BORDER)
            .child(if testing {
                button("stop-test", s, if stopping { "Stopping…" } else { "Stop Test" }, Btn::Stop, stopping, cx.listener(|this, _, _, cx| this.conn_dialog_stop_test(cx)))
            } else {
                button("test-conn", s, "Test Connection", Btn::Secondary, false, cx.listener(|this, _, _, cx| this.conn_dialog_test(cx)))
            })
            .child(button("save-conn", s, if saving { "Saving…" } else { "Save" }, Btn::Primary, saving || testing, cx.listener(|this, _, _, cx| this.conn_dialog_save(cx))));
        Some(
            crate::ui::widgets::overlay(0.6)
                .id("conn-overlay")
                .occlude()
                .child(
                    modal_box(640.0)
                        .id("conn-modal")
                        .child(modal_header(s, title, cx.listener(|this, _, _, cx| this.conn_dialog_close(cx))))
                        .child(body)
                        .child(footer),
                )
                .into_any_element(),
        )
    }

    pub fn open_color_panel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(d) = &self.conn_dialog else { return };
        let current = parse_hex(&d.tab_color.read(cx).text().to_string()).unwrap_or_else(|| parse_hex(DEFAULT_TAB_COLOR).unwrap());
        crate::ui::color_panel::show(current);
        cx.spawn_in(window, async move |this, cx| {
            let mut last: Option<String> = None;
            let mut first = true;
            loop {
                cx.background_executor().timer(std::time::Duration::from_millis(100)).await;
                let (visible, hex) = crate::ui::color_panel::poll();
                if first {
                    // The panel starts on the current colour; only report changes.
                    last = hex.clone();
                    first = false;
                } else if hex.is_some() && hex != last {
                    last = hex.clone();
                    let hex = hex.unwrap();
                    let ok = this
                        .update(cx, |this, cx| {
                            let Some(d) = &this.conn_dialog else { return false };
                            d.tab_color.update(cx, |i, cx| i.set_text(hex.clone(), cx));
                            cx.notify();
                            true
                        })
                        .unwrap_or(false);
                    if !ok {
                        crate::ui::color_panel::close();
                        break;
                    }
                }
                let open = this.update(cx, |this, _| this.conn_dialog.is_some()).unwrap_or(false);
                if !open {
                    crate::ui::color_panel::close();
                    break;
                }
                if !visible {
                    break;
                }
            }
        })
        .detach();
    }
}

/// Native (light appearance) macOS checkbox as rendered by WebKit.
pub fn checkbox(checked: bool, disabled: bool) -> AnyElement {
    let base = div()
        .size(px(14.))
        .flex_shrink_0()
        .rounded(px(3.))
        .flex()
        .items_center()
        .justify_center()
        .when(disabled, |d| d.opacity(0.5));
    if checked {
        base.bg(theme::hex(0x0a82ff))
            .child(div().text_size(px(11.)).line_height(px(11.)).text_color(theme::WHITE).font_weight(FontWeight::BOLD).child("✓"))
            .into_any_element()
    } else {
        base.bg(theme::WHITE).border_1().border_color(theme::hex(0xb4b4b4)).into_any_element()
    }
}
