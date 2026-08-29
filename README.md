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
  - Per-connection tab color with automatic contrasting tab text
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

- Desktop shell: WRY + Tao
- Backend: Rust
- Frontend: Svelte + TypeScript + Vite
- Editor: CodeMirror 6
- Database layer:
  - `sqlx` with MySQL, PostgreSQL, and SQLite support
  - Local metadata stored in SQLite

## Project Structure

- `desktop`: WRY/Tao desktop shell, Rust backend, command handlers, and IPC bridge
- `desktop/src/desktop.rs`: lightweight window/webview host and static asset protocol
- `desktop/src/ipc.rs`: JSON IPC dispatcher used by the frontend compatibility bindings
- `desktop/src/connections.rs`: connection manager, DSN logic, and Kubernetes port-forwarding
- `desktop/src/queries.rs`: query execution, cancellation, result conversion, and non-query handling
- `desktop/src/schema.rs`: schema and primary-key inspection
- `desktop/src/history.rs`: local metadata persistence in `history.db`
- `desktop/src/backup.rs`: table backup, import, pg_dump import, and drop workflows
- `frontend/src`: Svelte UI components and stores
- `frontend/desktop`: lightweight frontend bindings for the WRY IPC bridge

## Prerequisites

- Rust stable
- [Bun](https://bun.sh/)
- `make`
- Platform dependencies for WRY/WebKitGTK on Linux
- Optional tools based on workflow:
  - `kubectl` for Kubernetes port-forwarded connections

## Install and build

Install JavaScript dependencies once, from the repository root:

```bash
bun install
```

Then compile the production application:

```bash
make
```

`bun install` uses the root workspace definition and installs the frontend package too. Do not run an install command inside `frontend/`.

The build compiles the Svelte frontend first, then builds the Rust desktop executable at `desktop/target/release/multidb`. On macOS it also creates the application bundle.

For development, run:

```bash
make dev
```

## Running the macOS download

The macOS download is not currently notarized, so Gatekeeper may block it the first time it runs. Only override this warning if you downloaded MultiDB from a source you trust.

1. Unzip the download and move `MultiDB.app` to the `Applications` folder.
2. Try to open `MultiDB.app` once, then dismiss the warning.
3. Open **System Settings**, select **Privacy & Security**, and scroll down to **Security**.
4. Click **Open Anyway** beside the MultiDB warning.
5. Authenticate when prompted, then click **Open**.

macOS saves MultiDB as an exception, so later launches work normally. The **Open Anyway** button is available for about an hour after the blocked launch. See [Apple's instructions for opening an app from an unknown developer](https://support.apple.com/guide/mac-help/open-a-mac-app-from-an-unknown-developer-mh40616/mac).

If you get a "download is broken..." error instead run:
`sudo xattr -cr path/to/MultiDB.app` 

The application should then run as expected.

## Testing And Checks

Run both frontend and Rust checks with `make check`.

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
