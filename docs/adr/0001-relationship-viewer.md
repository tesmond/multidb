# ADR 0001: Database Relationship Viewer

## Status

Accepted

## Context

Multidb currently exposes schema metadata through the backend schema loader and renders SQL editor tabs in the right-hand pane. The product needs a relationship viewer that displays tables, columns, and foreign-key links for a single database connection.

The current schema payload does not include foreign-key relationship metadata, and the current tab model assumes every tab is a SQL editor tab.

## Decision

### Scope and entry point

- The relationship viewer is scoped to one selected connection and its database.
- The viewer opens from a connection-level context-menu action in the navigator labeled "Show relationships".
- The action ensures schema is loaded before opening the viewer.
- The viewer opens in the right-hand pane as a new tab kind.
- Only one relationship-viewer tab may exist per connection. Reopening the action focuses the existing tab.

### Data source and backend contract

- The backend schema-loading path is the source of truth for relationship data.
- Schema extraction must be extended to include foreign-key metadata for MySQL, Postgres, and SQLite.
- Relationship data is cached with the schema snapshot.
- Multi-schema databases are shown on one canvas using schema-qualified table identities.
- Views are excluded from the relationship viewer in v1. Only base tables appear.

### Relationship model

- Relationships are represented as foreign-key constraints, not just table-to-table adjacency.
- Each relationship preserves constraint identity.
- Links are anchored to the specific source and referenced columns.
- Composite foreign keys are represented as a single relationship with ordered column pairs.
- Self-referencing foreign keys render as loopback edges.
- Direction is meaningful and should be visually indicated subtly.

### Tab model

- The frontend tab model should be generalized into typed variants.
- At minimum, the app should support `sql` tabs and `relationshipDiagram` tabs.
- The existing shared tab strip remains in place.

### Viewer interaction model

- V1 is read-only. It does not edit or create relationships.
- The diagram supports pan, zoom, and dragging table cards.
- Table cards show all columns by default.
- Primary-key and foreign-key columns are visually marked.
- Clicking a defined table action or table header can open a SQL query tab for that table.
- Pointer-first spatial interaction is acceptable in v1, but filter, zoom controls, reset, and open-table actions must remain keyboard accessible.

### Layout and persistence

- The default display is a deterministic auto-layout derived from the schema graph.
- The default layout should use a layered graph approach with deterministic ordering and cluster packing.
- Relationship lines should use orthogonal or stepped routing.
- Manual table positions are persisted locally on the frontend.
- Persisted layout is keyed per connection and schema hash.
- Filtering must never rewrite saved coordinates.
- The tab includes a reset control that discards saved custom positions and restores the default deterministic layout.

### Filtering and refresh behavior

- The viewer includes a name filter in v1.
- Filtering shows matching tables plus their directly related neighbors for context.
- Hidden tables retain their positions while filtered out.
- If the underlying schema changes, the open diagram refreshes to the latest schema.
- Existing positions are preserved where possible for surviving tables.
- New tables receive auto-placed positions.
- Removed tables and stale links are dropped.

### Relationship inspection

- Constraint names are not shown persistently on edges in v1.
- Hover highlights the relationship and participating columns.
- Clicking a relationship reveals stable detail in a panel or inspector area within the tab.
- The inspector should include constraint name, source table and columns, target table and columns, and referential actions when available.

### Empty and large-schema behavior

- If a schema has no foreign-key relationships, the viewer still renders all tables.
- The tab shows an inline notice such as "No foreign-key relationships found".
- The initial performance target is roughly 100 to 150 tables with typical column counts.
- Larger schemas are best-effort and expected to rely on filtering.

### Rendering strategy

- Table nodes render as HTML cards.
- Relationship edges render as SVG overlay paths.

## Consequences

### Benefits

- The relationship viewer stays aligned with existing schema caching and refresh flows.
- The tab system becomes extensible for future non-SQL views.
- The viewer remains useful for both relationship-heavy and relationship-free schemas.
- Persisted layout rewards user effort without introducing backend persistence complexity.

### Costs

- Backend schema extraction becomes more complex across three drivers.
- The frontend must support typed tab rendering and a new diagram surface.
- A deterministic graph layout and edge routing engine must be implemented or integrated.

### Constraints for implementation

- The diagram should not open against stale or missing schema data.
- The data model must preserve constraint-level and column-level fidelity.
- Layout persistence must remain resilient to schema evolution through schema-hash invalidation and merge behavior.
