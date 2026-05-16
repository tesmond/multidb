# multidb

![image](mascot.png)


`multidb` is a desktop SQL client. It is designed for working across multiple database engines with one UI for querying,
schema browsing, table backup/import workflows, and query history.

I used pgadmin and MySQL Workbench for years, they both randomly crashed, or if I had to reset my VPN all the DB connections were broken and I would need to restart the app. Each load time took tens of seconds, when all I needed was to run a quick SQL query to validate data or debug an issue. Not only were they slow to load and took up lots of memory they also started moving away from my use case of running SQL queries and exporting data for quick reports or debugging. Even exploring the space I struggled to find any useful alternative. DBeaver is probably the best, but it still is sluggish to load and with all its functionality the query window is a little less intuitive to access.

So I thought I would finally roll my own. This is a simple SQL explorer, massively paired down compared to pgamin, DBeaver or MySQL Workbench, but it loads fast, uses little memory and can handle displaying large tables quickly.

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
	- Per-connection tab color + optional black tab text
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
	- Expandable navigation tree for schemas/tables/views/indexes
	- Context menu actions (view table, refresh schema, backup, drop, import)
- Backup and import:
	- Table backup generation
	- Import from zipped SQL and pg_dump formats
- Query productivity:
	- Per-connection query history
	- Saved queries with title editing
	- Quick re-open of saved/history queries into new tabs
- Persistence:
	- Local SQLite metadata store for saved connections, history, saved queries, and schema cache

## Tech Stack

- Desktop framework: Wails v2
- Backend: Go
- Frontend: Svelte + TypeScript + Vite
- Editor: CodeMirror 6
- Database drivers:
	- `github.com/go-sql-driver/mysql`
	- `github.com/jackc/pgx/v5`
	- `modernc.org/sqlite`

## Project Structure

- `main.go`: Wails app bootstrap and runtime options
- `app.go`: Main backend API bound to the frontend
- `backend/connections`: Connection manager and DSN logic
- `backend/queries`: Query execution and streaming logic
- `backend/schema`: Schema inspection
- `backend/history`: Local persistence (history.db)
- `frontend/src`: Svelte UI components and stores

## Prerequisites

- Go (module targets Go `1.25.x`)
- Node.js and npm
- Wails CLI v2
- Optional tools based on workflow:
	- `kubectl` for Kubernetes port-forwarded connections
	- `pg_restore` / PostgreSQL client tools for pg_dump import flows

## Development

Install frontend dependencies:

```bash
cd frontend
npm install
cd ..
```

Run in live development mode:

```bash
wails dev
```

This starts the desktop app with Vite hot reload for frontend updates.

## Build

Create a production desktop build:

```bash
wails build
```

## Testing

Run backend tests:

```bash
go test ./backend/...
```

Run frontend checks/tests from `frontend/` as needed:

```bash
npm run check
```

## Data Storage

Application metadata is stored in a local SQLite database (`history.db`) under the
user config directory in `multidb/`.

Stored data includes:

- Saved connections
- Query history
- Saved queries
- Cached schema snapshots

## App Icons

Desktop icon assets are committed for packaging:

- macOS icon: `resources/appicon.icns`
- Windows icon: `resources/appicon.ico`
