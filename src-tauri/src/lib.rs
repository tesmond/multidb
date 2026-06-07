mod backup;
mod commands;
mod connections;
mod history;
mod models;
mod password_vault;
mod queries;
mod schema;
mod state;

use state::AppState;
use tauri::Manager;

pub fn run() {
    sqlx::any::install_default_drivers();

    tauri::Builder::default()
        .manage(AppState::default())
        .setup(|app| {
            let state = app.state::<AppState>();
            tauri::async_runtime::block_on(async move {
                state.initialise().await.map_err(|err| {
                    eprintln!("startup failed: {err}");
                    err
                })
            })?;
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::save_and_connect,
            commands::test_connection,
            commands::disconnect,
            commands::list_connections,
            commands::list_saved_connections,
            commands::execute_query,
            commands::execute_query_streamed,
            commands::cancel_query,
            commands::get_table_primary_keys,
            commands::get_schema,
            commands::load_schema,
            commands::save_schema,
            commands::backup_table,
            commands::drop_table,
            commands::select_import_file,
            commands::select_sqlite_file,
            commands::import_table,
            commands::save_csv,
            commands::save_file,
            commands::get_query_history,
            commands::get_query_history_by_conn_id,
            commands::clear_query_history,
            commands::clear_query_history_by_conn_id,
            commands::save_query,
            commands::get_saved_queries,
            commands::delete_saved_query,
            commands::update_saved_query_title,
        ])
        .run(tauri::generate_context!())
        .expect("failed to run multidb");
}
