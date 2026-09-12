mod backup;
mod commands;
mod connections;
mod history;
mod ipc_diagnostics;
mod models;
mod password_vault;
mod queries;
mod schema;
mod state;
pub mod ui;

pub fn run() {
    if let Err(e) = ui::run() {
        eprintln!("multidb: {e:#}");
        std::process::exit(1);
    }
}
