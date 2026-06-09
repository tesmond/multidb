mod backup;
mod commands;
mod connections;
mod desktop;
mod history;
mod ipc;
mod models;
mod password_vault;
mod queries;
mod schema;
mod startup_profile;
mod state;

pub fn run() {
    desktop::run().expect("failed to run multidb");
}
