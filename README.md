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
  - Find/replace panel and go-to-line (`⌘F`, `⌘G`, `⌥⌘G`)
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

- Rust stable (`rustup` toolchain, 1.85 or newer)
- `make`
- macOS: the Command Line Tools are enough (`xcode-select --install`)
- Linux: the GPUI build dependencies

  ```bash
  sudo apt-get install -y build-essential pkg-config libasound2-dev \
      libfontconfig-dev libwayland-dev libxkbcommon-x11-dev libssl-dev \
      libzstd-dev libvulkan1 libgit2-dev
  ```

  A Vulkan-capable driver is required at runtime.
- Windows: the MSVC toolchain (`rustup default stable-msvc`) and the Windows SDK
- Optional, per workflow: `kubectl` for Kubernetes port-forwarded connections

## Build

From the repository root:

```bash
make build     # release build (+ MultiDB.app on macOS)
make dev       # debug build and run
make check     # cargo check
make test      # unit tests
```

`make build` produces `desktop/target/release/multidb`; on macOS it also writes
`desktop/target/release/MultiDB.app` and a zip beside it. Plain cargo works
just as well:

```bash
cargo build --release --manifest-path desktop/Cargo.toml
```

### Metal shaders on macOS

GPUI renders through Metal, and its shaders are normally compiled at build time
with `xcrun metal` — a tool that ships with Xcode but **not** with the Command
Line Tools. So that a Command Line Tools install is enough, this crate enables
GPUI's `runtime_shaders` feature by default, which compiles the shaders when the
app starts (a few milliseconds at launch).

With Xcode installed you can build the shaders ahead of time instead:

```bash
make build CARGO_FLAGS=--no-default-features
```

### Fonts

The SQL editor asks for JetBrains Mono, then Fira Code, then Cascadia Code, and
falls back to the system fixed-width font. Set `MULTIDB_MONO_FONT` to pick a
different family:

```bash
MULTIDB_MONO_FONT="SF Mono" make dev
```

## Quick Mac OS install

The macOS download is not currently signed as I do not have an Apple developer account. So here are some quick instructions:

1. Download the latest Mac OS release zip file.
2. Unzip the download inside the `Downloads` folder.
3. At the terminal run `sudo xattr -cr ~/Download/MultiDB.app` and enter your password.
4. Now run `mv ~/Download/MultiDB.app /Applications/MultiDB.app`

MultiDB should be accessible.

## Testing And Checks

```bash
make check   # cargo check
make test    # unit tests (SQL tokenizer, completion, lint, value formatting,
             # column sizing, diagram layout, connection helpers)
```

## Data Storage

Application metadata is stored in a local SQLite database (`history.db`) under the user config directory in `multidb/`.

Stored data includes:

- Saved connections
- Query history
- Saved queries
- Cached schema snapshots

UI preferences (font scale, server groups, connection order, saved diagram
layouts) live beside it in `ui-settings.json`. To run against a throwaway
profile, start the app with a different `HOME`:

```bash
HOME=/tmp/multidb-demo CFFIXED_USER_HOME=/tmp/multidb-demo \
    cargo run --manifest-path desktop/Cargo.toml
```

## App Icons

Desktop icon assets are committed for packaging:

- macOS icon: `build/appicon.icns` (built from `build/icon.iconset/`)
- Windows icon: `build/windows/icon.ico`, embedded by `desktop/app.rc`
- PNG icon: `build/icon.png`
