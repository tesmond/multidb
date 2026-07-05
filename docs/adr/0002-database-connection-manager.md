# ADR 0002: Database Connection Manager

## Status

Accepted

## Context

Users need to inspect active server-side database connections from the navigator and terminate a selected connection when it is safe and permitted by the database.

The app already has typed right-pane tabs for non-query database views. Database connection management should follow that model instead of becoming a transient dialog, because connection state is live operational data and benefits from a persistent tab.

## Decision

- Add a database-level navigator context-menu action labeled "Show Connections".
- Open a right-pane tab with kind `databaseConnections`.
- Only one connection-manager tab exists per database connection; reopening the action focuses the existing tab.
- The tab polls while mounted every 10 seconds and refreshes immediately after a terminate attempt.
- The tab shows connection id, user, database, client, state, opened time, last-active time, and most recent command when the driver exposes those details.
- Termination requires a confirmation dialog.
- Postgres uses `pg_stat_activity` and `pg_terminate_backend`.
- MySQL uses `SHOW FULL PROCESSLIST` and `KILL`.
- SQLite shows an unsupported state because it does not expose server-side database sessions.
- The management session itself must not be terminable from the UI.

## Consequences

- Operators can manage active database sessions without leaving the database workspace.
- Backend commands now expose connection listing and termination as explicit IPC operations.
- MySQL opened time may be unknown because the standard process list does not reliably expose connection start time.
- Permission errors from the database are surfaced in the tab and status bar.
