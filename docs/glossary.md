# Glossary

## Relationship Viewer

A read-only diagram tab that displays tables, columns, and foreign-key relationships for one database connection.

## Relationship Diagram Tab

A right-pane tab variant with kind `relationshipDiagram`, distinct from a SQL editor tab.

## Schema Snapshot

The cached schema payload loaded from the backend for a connection. It is the source of truth for tables, columns, and foreign-key relationships.

## Relationship

A foreign-key constraint represented as a directional edge between source and referenced tables, preserving constraint identity.

## Column Pair

One ordered mapping from a source foreign-key column to a referenced target column within a relationship.

## Composite Foreign Key

A foreign-key constraint composed of multiple ordered column pairs that must be treated as one relationship.

## Default Layout

The deterministic auto-layout generated from the current schema graph before any user repositioning.

## Manual Layout

The user-adjusted table positions persisted locally for a connection and schema hash.

## Schema Hash

The identifier used to determine whether a persisted manual layout still applies to the current schema snapshot.

## Neighbor-Preserving Filter

A filter mode that keeps matching tables visible along with their directly related neighbors to preserve graph context.

## Relationship Inspector

The stable details area in the diagram tab that shows metadata for a selected relationship.

## Loopback Edge

A self-referencing relationship rendered as an edge that leaves and returns to the same table.

## Database Connection Manager

A right-pane tab that lists server-side sessions for one database connection and allows eligible sessions to be terminated.

## Database Connections Tab

A right-pane tab variant with kind `databaseConnections`, distinct from SQL editor and relationship diagram tabs.

## Managed Database Session

One server-side backend process or process-list row reported by a database engine for the selected connection.

## Terminate Connection

An operator action that asks the database engine to close a selected managed database session after confirmation.
