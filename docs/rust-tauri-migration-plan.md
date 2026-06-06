# Rust + Tauri Migration Plan

## Existing Functionality To Preserve

- Multi-database connections for MySQL, PostgreSQL, and SQLite.
- Saved connection management with test, edit, reconnect, delete, tab colors, and optional Kubernetes port-forwarding.
- SQL execution with cancellation, streamed result chunks, affected row counts for non-query statements, and query history.
- Schema browsing for schemas, tables, views, indexes, columns, table sizes, and primary keys.
- Schema cache persistence per connection.
- Table workflows: backup to zipped JSON SQL archive, import zipped backups, import PostgreSQL dumps, and drop tables.
- Query productivity: saved queries, title updates, delete, per-connection history, and clear history.
- Native desktop file dialogs for CSV export, backup output, and import file selection.

## New Architecture

- `src-tauri/`: Rust/Tauri desktop shell and backend commands.
- `src-tauri/src/connections.rs`: active pool management, DSN construction, reconnect, and Kubernetes port-forward processes.
- `src-tauri/src/queries.rs`: query execution, JSON value conversion, cancellation, and non-query execution.
- `src-tauri/src/schema.rs`: engine-specific schema and primary-key inspection.
- `src-tauri/src/history.rs`: local SQLite metadata store for saved connections, history, saved queries, and schema cache.
- `src-tauri/src/backup.rs`: backup/import/drop table workflows.
- `src-tauri/src/commands.rs`: Tauri command boundary and streamed query events.
- `frontend/wailsjs/...`: compatibility shims that map the previous Wails API calls to Tauri `invoke` and `listen`.

## Migration Steps Completed In This Rewrite

- Added Tauri v2 configuration and Rust crate under `src-tauri`.
- Ported the Go backend API to Rust modules with Tauri commands.
- Kept the existing Svelte frontend and replaced Wails bindings with Tauri compatibility wrappers.
- Updated npm scripts so `npm run dev` and `npm run build` target Tauri from the repository root.
- Preserved persisted `history.db` table names and columns, with added Kubernetes connection columns.

## Follow-Up Hardening

- Add integration tests with local temporary SQLite databases for query execution, schema inspection, backup, import, and history persistence.
- Add containerized MySQL/PostgreSQL test fixtures for driver-specific schema and backup paths.
- Consider replacing plain saved passwords with OS keychain storage through a Tauri plugin.
- Remove the old Go/Wails source once the Rust path has been validated against real databases.
