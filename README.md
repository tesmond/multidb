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
  - Native GPU-rendered SQL editor
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

- UI: [GPUI](https://www.gpui.rs/) (GPU-accelerated Rust UI framework)
- Backend: Rust
- Database layer:
  - `sqlx` with MySQL, PostgreSQL, and SQLite support
  - Local metadata stored in SQLite

## Project Structure

- `desktop`: the whole application - GPUI front end, Rust backend, and command handlers
- `desktop/src/ui`: GPUI window, workspace layout, navigator, SQL editor, results grid, and dialogs
- `desktop/src/commands.rs`: command handlers the UI calls into
- `desktop/src/connections.rs`: connection manager, DSN logic, and Kubernetes port-forwarding
- `desktop/src/queries.rs`: query execution, cancellation, result conversion, and non-query handling
- `desktop/src/schema.rs`: schema and primary-key inspection
- `desktop/src/history.rs`: local metadata persistence in `history.db`
- `desktop/src/backup.rs`: table backup, import, pg_dump import, and drop workflows

## Prerequisites

- Rust stable
- `make`
- Platform dependencies for GPUI on Linux (Vulkan, Wayland/X11, fontconfig)
- Optional tools based on workflow:
  - `kubectl` for Kubernetes port-forwarded connections

## Install and build

Compile the production application from the repository root:

```bash
make
```

The build produces the Rust desktop executable at `desktop/target/release/multidb`. On macOS it also creates the application bundle.

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

Run `make check` for a type check and `make test` for the unit tests.

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
