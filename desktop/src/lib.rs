#![allow(dead_code)]

mod backup;
mod commands;
mod connections;
mod editor;
mod history;
mod models;
mod password_vault;
mod queries;
mod schema;
mod slint_app;
mod state;

pub fn run() {
    slint_app::run().expect("failed to run multidb");
}
