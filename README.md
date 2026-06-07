# multidb

![image](mascot.png)

`multidb` is a desktop SQL client for working across multiple database engines with one UI for querying, schema browsing, table backup/import workflows, and query history.

I used pgAdmin and MySQL Workbench for years, and both could be slow to load or fragile after VPN resets. `multidb` is intentionally much smaller than tools like pgAdmin, DBeaver, or MySQL Workbench: it focuses on fast startup, low memory use, quick SQL execution, exporting data, and browsing schemas without a lot of surrounding weight.

## Screenshot

![image](screenshot.png)

## Features

- Multi-database connectivity:
  - MySQL
  - PostgreSQL
  - SQLite
- Connection management:
  - Save, edit, test, and remove connections
  - Optional Kubernetes port-forward support for database access
  - Per-connection tab color and optional black tab text
  - Connection color swatch shown in the left navigator
- SQL editor experience:
  - CodeMirror-based SQL editor
  - Connection-aware SQL dialect switching
  - Schema-driven SQL autocomplete
  - Query cancellation support
- Query execution:
  - Streamed query results for large datasets
  - Virtualized results grid for performance
  - Column sorting and copy/select behavior
  - CSV export support
- Schema explorer:
  - Expandable navigation tree for schemas, tables, views, indexes, and columns
  - Context menu actions for viewing tables, refreshing schema, backups, drops, and imports
- Backup and import:
  - Table backup generation
  - Import from zipped SQL backup archives and PostgreSQL dump files
- Query productivity:
  - Per-connection query history
  - Saved queries with title editing
  - Quick re-open of saved/history queries into new tabs
- Persistence:
  - Local SQLite metadata store for saved connections, history, saved queries, and schema cache

## Tech Stack

- Desktop framework: Tauri 2
- Backend: Rust
- Frontend: Svelte + TypeScript + Vite
- Editor: CodeMirror 6
- Database layer:
  - `sqlx` with MySQL, PostgreSQL, and SQLite support
  - Local metadata stored in SQLite

## Project Structure

- `src-tauri`: Tauri app, Rust backend, command handlers, and packaging config
- `src-tauri/src/connections.rs`: connection manager, DSN logic, and Kubernetes port-forwarding
- `src-tauri/src/queries.rs`: query execution, cancellation, result conversion, and non-query handling
- `src-tauri/src/schema.rs`: schema and primary-key inspection
- `src-tauri/src/history.rs`: local metadata persistence in `history.db`
- `src-tauri/src/backup.rs`: table backup, import, pg_dump import, and drop workflows
- `frontend/src`: Svelte UI components and stores
- `src-tauri/gen/`: Tauri-generated frontend bindings (auto-generated during build)
- `docs/rust-tauri-migration-plan.md`: migration inventory and follow-up hardening plan

## Prerequisites

- Rust stable
- Node.js and npm
- Platform dependencies for Tauri 2
- Optional tools based on workflow:
  - `kubectl` for Kubernetes port-forwarded connections

## Development

Install dependencies:

```bash
npm install
cd frontend
npm install
cd ..
```

Run the desktop app in development mode:

```bash
npm run dev
```

This starts the Tauri desktop app and the Vite dev server for frontend updates.

## Build

Create a production desktop build:

```bash
npm run build
```

## Testing And Checks

Run Rust checks:

```bash
npm run rust:check
```

Run frontend checks:

```bash
npm run frontend:check
```

Build the frontend:

```bash
npm run frontend:build
```

## Data Storage

Application metadata is stored in a local SQLite database (`history.db`) under the user config directory in `multidb/`.

Stored data includes:

- Saved connections
- Query history
- Saved queries
- Cached schema snapshots

## App Icons

Desktop icon assets are committed for packaging:

- macOS icon: `build/appicon.icns`
- Windows icon: `build/appicon.ico`
- PNG icon: `build/icon.png`
