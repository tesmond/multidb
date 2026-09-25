//! The navigator tree, flattened into rows.
//!
//! The navigator used to build every open node of the tree as nested elements
//! on every frame — and a frame happens on each keystroke, each caret blink and
//! each hover — so with hundreds of tables across a score of connections the
//! window spent its time laying out rows nobody could see.
//!
//! Instead the tree is walked into a flat list of [`Row`]s. A row is a handful
//! of indices into the connection's schema (no strings, nothing cloned) plus
//! its indent, the gap above it and its height, which are all known up front
//! because every row kind has a fixed height. From the scroll offset,
//! [`visible_range`] picks the rows inside the viewport; only those become
//! elements, and two spacers stand in for the rest, so the scrollable extent —
//! and with it the scrollbar — is exactly what the whole tree would measure.
//!
//! The geometry reproduces the nested layout it replaces: each level's
//! `pl`/`ml` becomes the row's indent and each section's `mt(2px)` becomes the
//! gap above its first row.

use crate::models::{Schema, SchemaTree, Table};
use crate::ui::model::{display_structure, ActiveConnection, ServerGroup};
use crate::ui::nav_index::{Matches, NONE};
use std::collections::HashSet;
use std::fmt::Write;
use std::ops::Range;

/// Views or indexes: the two plain lists under a schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Leaf {
    Views,
    Indexes,
}

/// What stands in for a connection's schema when there is none to show.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Info {
    Loading,
    Error,
    NotLoaded,
}

/// One line of the tree. `conn` indexes the connections slice the rows were
/// built from and `group` the server groups; `db`, `schema`, `table` index the
/// connection's schema tree, with [`NONE`] where that level does not exist.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RowKind {
    Group { group: usize, count: usize, open: bool },
    Conn { conn: usize, group: Option<usize>, open: bool },
    Info { conn: usize, info: Info },
    Database { conn: usize, db: u32, open: bool },
    Schema { conn: usize, db: u32, schema: u32, open: bool },
    Tables { conn: usize, db: u32, schema: u32, count: usize, open: bool },
    Table { conn: usize, db: u32, schema: u32, table: u32, open: bool },
    Column { conn: usize, db: u32, schema: u32, table: u32, column: u32 },
    LeafSection { conn: usize, db: u32, schema: u32, leaf: Leaf, count: usize, open: bool },
    LeafItem { conn: usize, db: u32, schema: u32, leaf: Leaf, item: u32 },
    /// The 2px of padding below an open connection's children.
    Gap,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Row {
    pub kind: RowKind,
    /// Distance from the left of the tree.
    pub indent: f32,
    /// Margin above the row.
    pub gap: f32,
    /// The row's own height.
    pub height: f32,
}

impl Row {
    /// Vertical space the row takes, margin included.
    pub fn extent(&self) -> f32 {
        self.gap + self.height
    }
}

/// Line heights at the current font scale (`line-height: normal` of 13, 12
/// and 11px text), from which every row height follows.
#[derive(Clone, Copy, Debug)]
pub struct Metrics {
    pub lh13: f32,
    pub lh12: f32,
    pub lh11: f32,
}

impl Metrics {
    /// Group and connection rows: 13px text, 5px padding.
    pub fn header(&self) -> f32 {
        self.lh13 + 10.0
    }
    /// Database and schema labels: 13px text, 3px padding.
    pub fn section(&self) -> f32 {
        self.lh13 + 6.0
    }
    /// "Tables"/"Views"/"Indexes" headers, tables and leaf items: 12px, 3px.
    pub fn item(&self) -> f32 {
        self.lh12 + 6.0
    }
    /// Columns: 11px text, 2px padding.
    pub fn column(&self) -> f32 {
        self.lh11 + 4.0
    }
    /// "Loading schema…" and friends: 12px text, 6px padding.
    pub fn info(&self) -> f32 {
        self.lh12 + 12.0
    }
}

/// Everything that decides which rows exist.
pub struct TreeState<'a> {
    pub connections: &'a [ActiveConnection],
    pub groups: &'a [ServerGroup],
    pub expanded: &'a HashSet<String>,
    pub expanded_tables: &'a HashSet<String>,
    pub expanded_groups: &'a HashSet<String>,
    /// The filter results to show, or `None` when not filtering.
    pub matches: Option<&'a Matches>,
}

/// Each nesting level of the old layout indents by 16px.
const STEP: f32 = 16.0;
/// Sections were separated by a 2px top margin.
const SECTION_GAP: f32 = 2.0;
/// Open connections had 2px of bottom padding.
const CONN_TAIL: f32 = 2.0;

// ─── Schema tree accessors ─────────────────────────────────────────────────

pub fn schemas_of(tree: &SchemaTree, db: u32) -> &[Schema] {
    if db == NONE {
        &tree.schemas
    } else {
        tree.databases.get(db as usize).map(|d| d.schemas.as_slice()).unwrap_or(&[])
    }
}

pub fn schema_of(tree: &SchemaTree, db: u32, schema: u32) -> Option<&Schema> {
    if schema == NONE {
        None
    } else {
        schemas_of(tree, db).get(schema as usize)
    }
}

pub fn tables_of(tree: &SchemaTree, db: u32, schema: u32) -> &[Table] {
    if schema == NONE {
        &tree.tables
    } else {
        schema_of(tree, db, schema).map(|s| s.tables.as_slice()).unwrap_or(&[])
    }
}

pub fn views_of(tree: &SchemaTree, db: u32, schema: u32) -> &[Table] {
    if schema == NONE {
        &tree.views
    } else {
        schema_of(tree, db, schema).map(|s| s.views.as_slice()).unwrap_or(&[])
    }
}

pub fn indexes_of(tree: &SchemaTree, db: u32, schema: u32) -> &[String] {
    if schema == NONE {
        &tree.indexes
    } else {
        schema_of(tree, db, schema).map(|s| s.indexes.as_slice()).unwrap_or(&[])
    }
}

pub fn leaf_count(tree: &SchemaTree, db: u32, schema: u32, leaf: Leaf) -> usize {
    match leaf {
        Leaf::Views => views_of(tree, db, schema).len(),
        Leaf::Indexes => indexes_of(tree, db, schema).len(),
    }
}

pub fn leaf_name(tree: &SchemaTree, db: u32, schema: u32, leaf: Leaf, item: u32) -> &str {
    match leaf {
        Leaf::Views => views_of(tree, db, schema).get(item as usize).map(|v| v.name.as_str()).unwrap_or(""),
        Leaf::Indexes => indexes_of(tree, db, schema).get(item as usize).map(|s| s.as_str()).unwrap_or(""),
    }
}

/// `navigatorSchemaGroups`: the database levels a connection's tree shows, as
/// database positions ([`NONE`] for the connection's own, unnamed or
/// configured database). Empty when the tree is flat (MySQL without schemas,
/// SQLite).
pub fn schema_groups(conn: &ActiveConnection) -> Vec<u32> {
    let Some(s) = &conn.schema else { return Vec::new() };
    if conn.config.driver == "postgres" {
        if !s.databases.is_empty() {
            return (0..s.databases.len() as u32).collect();
        }
        if !conn.config.database.trim().is_empty() && !s.schemas.is_empty() {
            return vec![NONE];
        }
        return Vec::new();
    }
    if !s.schemas.is_empty() {
        vec![NONE]
    } else {
        Vec::new()
    }
}

/// The label of a database level: the database's name, the configured
/// database for a Postgres tree without databases, and otherwise empty (no
/// database row at all).
pub fn database_name(conn: &ActiveConnection, db: u32) -> String {
    if db != NONE {
        return conn
            .schema
            .as_ref()
            .and_then(|s| s.databases.get(db as usize))
            .map(|d| d.name.clone())
            .unwrap_or_default();
    }
    if conn.config.driver == "postgres" {
        conn.config.database.trim().to_string()
    } else {
        String::new()
    }
}

// ─── Expansion keys ────────────────────────────────────────────────────────
//
// These are the keys `expanded_tables` has always used, so what was open stays
// open.

pub fn database_key(conn_id: &str, dbname: &str) -> String {
    format!("{conn_id}-database-{dbname}")
}

pub fn schema_key(conn_id: &str, schema: &str) -> String {
    format!("{conn_id}-schema-{schema}")
}

pub fn tables_key(conn_id: &str, schema: Option<&str>) -> String {
    match schema {
        Some(sc) => format!("{conn_id}-{sc}-tables"),
        None => format!("{conn_id}-tables"),
    }
}

pub fn leaf_key(conn_id: &str, schema: Option<&str>, leaf: Leaf) -> String {
    let what = match leaf {
        Leaf::Views => "views",
        Leaf::Indexes => "indexes",
    };
    match schema {
        Some(sc) => format!("{conn_id}-{sc}-{what}"),
        None => format!("{conn_id}-{what}"),
    }
}

pub fn table_key(conn_id: &str, schema: Option<&str>, table: &str) -> String {
    match schema {
        Some(sc) => format!("{conn_id}-{sc}-t-{table}"),
        None => format!("{conn_id}-t-{table}"),
    }
}

// ─── Building ──────────────────────────────────────────────────────────────

struct Builder<'a> {
    st: &'a TreeState<'a>,
    m: Metrics,
    rows: Vec<Row>,
    /// Reused for expansion-key lookups so walking thousands of tables does
    /// not allocate a key per table.
    key: String,
}

impl Builder<'_> {
    fn push(&mut self, kind: RowKind, indent: f32, gap: f32, height: f32) {
        self.rows.push(Row { kind, indent, gap, height });
    }

    fn is_open(&mut self, key: impl FnOnce(&mut String)) -> bool {
        self.key.clear();
        key(&mut self.key);
        self.st.expanded_tables.contains(self.key.as_str())
    }

    fn filtering(&self) -> bool {
        self.st.matches.is_some()
    }

    fn conn(&mut self, ci: usize, group: Option<usize>) {
        let st = self.st;
        let conn = &st.connections[ci];
        let id = conn.config.id.as_str();
        let open = self.filtering() || st.expanded.contains(id);
        self.push(RowKind::Conn { conn: ci, group, open }, 0.0, 0.0, self.m.header());
        if !open {
            return;
        }
        let base = STEP;
        if conn.schema_loading {
            self.push(RowKind::Info { conn: ci, info: Info::Loading }, base, 0.0, self.m.info());
        } else if conn.schema_error.is_some() {
            self.push(RowKind::Info { conn: ci, info: Info::Error }, base, 0.0, self.m.info());
        } else if let Some(schema) = conn.schema.clone() {
            self.conn_children(ci, &schema, base);
        } else {
            self.push(RowKind::Info { conn: ci, info: Info::NotLoaded }, base, 0.0, self.m.info());
        }
        self.push(RowKind::Gap, 0.0, 0.0, CONN_TAIL);
    }

    fn conn_children(&mut self, ci: usize, schema: &SchemaTree, base: f32) {
        let st = self.st;
        let conn = &st.connections[ci];
        let filtering = self.filtering();
        let groups = schema_groups(conn);
        if groups.is_empty() {
            // Flat: the tables section sits straight under the connection.
            self.tables_section(ci, schema, NONE, NONE, base + STEP);
            if !filtering {
                self.leaf_section(ci, schema, NONE, NONE, Leaf::Views, base + STEP);
                self.leaf_section(ci, schema, NONE, NONE, Leaf::Indexes, base + STEP);
            }
            return;
        }
        for db in groups {
            let dbname = database_name(conn, db);
            let nested = !dbname.is_empty();
            let db_open = !nested
                || filtering
                || self.is_open(|k| {
                    let _ = write!(k, "{}-database-{dbname}", conn.config.id);
                });
            if nested {
                self.push(RowKind::Database { conn: ci, db, open: db_open }, base + STEP, SECTION_GAP, self.m.section());
            }
            if !db_open {
                continue;
            }
            let inner = if nested { base + STEP } else { base };
            for (si, sc) in schemas_of(schema, db).iter().enumerate() {
                let si = si as u32;
                let count = self.matching_tables(ci, schema, db, si).len();
                if filtering && count == 0 {
                    continue;
                }
                let sopen = filtering
                    || self.is_open(|k| {
                        let _ = write!(k, "{}-schema-{}", conn.config.id, sc.name);
                    });
                let schema_indent = inner + STEP;
                self.push(RowKind::Schema { conn: ci, db, schema: si, open: sopen }, schema_indent, SECTION_GAP, self.m.section());
                if !sopen {
                    continue;
                }
                let section_indent = schema_indent + 2.0 * STEP;
                self.tables_section(ci, schema, db, si, section_indent);
                if !filtering {
                    self.leaf_section(ci, schema, db, si, Leaf::Views, section_indent);
                    self.leaf_section(ci, schema, db, si, Leaf::Indexes, section_indent);
                }
            }
        }
    }

    /// Positions of the tables of one schema that survive the filter.
    fn matching_tables(&self, ci: usize, schema: &SchemaTree, db: u32, sc: u32) -> Vec<u32> {
        let tables = tables_of(schema, db, sc);
        match self.st.matches {
            None => (0..tables.len() as u32).collect(),
            Some(m) => {
                let id = self.st.connections[ci].config.id.as_str();
                (0..tables.len() as u32).filter(|&i| m.table_matches(id, (db, sc, i))).collect()
            }
        }
    }

    fn tables_section(&mut self, ci: usize, schema: &SchemaTree, db: u32, sc: u32, indent: f32) {
        let st = self.st;
        let conn = &st.connections[ci];
        let filtering = self.filtering();
        let visible = self.matching_tables(ci, schema, db, sc);
        // A flat tree drops the whole section when nothing in it matches; a
        // schema with no match was already skipped by the caller.
        if filtering && visible.is_empty() {
            return;
        }
        let sc_name = schema_of(schema, db, sc).map(|s| s.name.as_str());
        let open = filtering
            || self.is_open(|k| {
                let _ = match sc_name {
                    Some(s) => write!(k, "{}-{s}-tables", conn.config.id),
                    None => write!(k, "{}-tables", conn.config.id),
                };
            });
        self.push(
            RowKind::Tables { conn: ci, db, schema: sc, count: visible.len(), open },
            indent,
            SECTION_GAP,
            self.m.item(),
        );
        if !open {
            return;
        }
        let tables = tables_of(schema, db, sc);
        for ti in visible {
            let t = &tables[ti as usize];
            let topen = self.is_open(|k| {
                let _ = match sc_name {
                    Some(s) => write!(k, "{}-{s}-t-{}", conn.config.id, t.name),
                    None => write!(k, "{}-t-{}", conn.config.id, t.name),
                };
            });
            self.push(RowKind::Table { conn: ci, db, schema: sc, table: ti, open: topen }, indent + STEP, 0.0, self.m.item());
            if topen {
                for c in 0..t.columns.len() as u32 {
                    self.push(
                        RowKind::Column { conn: ci, db, schema: sc, table: ti, column: c },
                        // `ml(16px) pl(24px)` under the section header.
                        indent + STEP + 24.0,
                        0.0,
                        self.m.column(),
                    );
                }
            }
        }
    }

    fn leaf_section(&mut self, ci: usize, schema: &SchemaTree, db: u32, sc: u32, leaf: Leaf, indent: f32) {
        let count = leaf_count(schema, db, sc, leaf);
        if count == 0 {
            return;
        }
        let conn = &self.st.connections[ci];
        let key = leaf_key(&conn.config.id, schema_of(schema, db, sc).map(|s| s.name.as_str()), leaf);
        let open = self.st.expanded_tables.contains(&key);
        self.push(RowKind::LeafSection { conn: ci, db, schema: sc, leaf, count, open }, indent, SECTION_GAP, self.m.item());
        if open {
            for item in 0..count as u32 {
                self.push(RowKind::LeafItem { conn: ci, db, schema: sc, leaf, item }, indent + STEP, 0.0, self.m.item());
            }
        }
    }
}

/// Walk the tree into rows, in the order and with the geometry the nested
/// layout had.
pub fn build(st: &TreeState, m: Metrics) -> Vec<Row> {
    let mut b = Builder { st, m, rows: Vec::new(), key: String::new() };
    let filtering = st.matches.is_some();
    let conn_matches = |c: &ActiveConnection| st.matches.is_none_or(|m| m.conn_matches(&c.config.id));
    let index_of = |c: &ActiveConnection| st.connections.iter().position(|x| std::ptr::eq(x, c)).unwrap_or(0);
    let structure = display_structure(st.connections, st.groups);
    for (gi, (g, gconns)) in structure.groups.iter().enumerate() {
        let matching: Vec<usize> = gconns.iter().copied().filter(|&c| conn_matches(c)).map(index_of).collect();
        let open = filtering || st.expanded_groups.contains(&g.id);
        if !filtering || !matching.is_empty() {
            b.push(RowKind::Group { group: gi, count: matching.len(), open }, 0.0, 0.0, m.header());
        }
        if open {
            for ci in matching {
                b.conn(ci, Some(gi));
            }
        }
    }
    for &c in &structure.ungrouped {
        if conn_matches(c) {
            b.conn(index_of(c), None);
        }
    }
    b.rows
}

/// Total height of the rows.
pub fn total_height(rows: &[Row]) -> f32 {
    rows.iter().map(Row::extent).sum()
}

/// The rows to draw for a viewport `view_h` tall scrolled `scroll_top` down
/// (both relative to the first row), with `overdraw` extra above and below so
/// a scroll of a few lines never shows a gap before the next frame. Returns
/// the range and the heights of the spacers before and after it.
pub fn visible_range(rows: &[Row], scroll_top: f32, view_h: f32, overdraw: f32) -> (Range<usize>, f32, f32) {
    let lo = scroll_top - overdraw;
    let hi = scroll_top + view_h + overdraw;
    let mut y = 0.0;
    let mut start = rows.len();
    let mut before = 0.0;
    for (i, r) in rows.iter().enumerate() {
        let next = y + r.extent();
        if next > lo {
            start = i;
            before = y;
            break;
        }
        y = next;
    }
    if start == rows.len() {
        return (start..start, total_height(rows), 0.0);
    }
    let mut end = start;
    let mut y = before;
    while end < rows.len() && y < hi {
        y += rows[end].extent();
        end += 1;
    }
    let after: f32 = rows[end..].iter().map(Row::extent).sum();
    (start..end, before, after)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Column, ConnectionConfig, Database};
    use crate::ui::nav_index::{search, SchemaIndex};
    use std::sync::Arc;

    const M: Metrics = Metrics { lh13: 15.0, lh12: 14.0, lh11: 13.0 };

    fn table(name: &str, cols: &[&str]) -> Table {
        Table {
            name: name.into(),
            table_type: "table".into(),
            size_bytes: None,
            columns: cols.iter().map(|c| Column { name: (*c).into(), ..Default::default() }).collect(),
        }
    }

    fn conn(id: &str, driver: &str, database: &str, schema: SchemaTree) -> ActiveConnection {
        let mut c = ActiveConnection::new(ConnectionConfig {
            id: id.into(),
            name: id.into(),
            driver: driver.into(),
            database: database.into(),
            ..Default::default()
        });
        c.schema = Some(Arc::new(schema));
        c
    }

    fn pg(tables: Vec<Table>) -> SchemaTree {
        SchemaTree {
            databases: vec![Database {
                name: "app".into(),
                schemas: vec![Schema { name: "public".into(), tables, views: vec![table("v", &[])], ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        }
    }

    fn sqlite(tables: Vec<Table>) -> SchemaTree {
        SchemaTree { tables, ..Default::default() }
    }

    struct Fixture {
        conns: Vec<ActiveConnection>,
        groups: Vec<ServerGroup>,
        expanded: HashSet<String>,
        tables: HashSet<String>,
        egroups: HashSet<String>,
    }

    impl Fixture {
        fn new(conns: Vec<ActiveConnection>) -> Self {
            Fixture { conns, groups: Vec::new(), expanded: HashSet::new(), tables: HashSet::new(), egroups: HashSet::new() }
        }
        fn rows(&self, matches: Option<&Matches>) -> Vec<Row> {
            build(
                &TreeState {
                    connections: &self.conns,
                    groups: &self.groups,
                    expanded: &self.expanded,
                    expanded_tables: &self.tables,
                    expanded_groups: &self.egroups,
                    matches,
                },
                M,
            )
        }
    }

    fn kinds(rows: &[Row]) -> Vec<RowKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn a_collapsed_connection_is_one_row() {
        let f = Fixture::new(vec![conn("a", "sqlite", "", sqlite(vec![table("t", &["x"])]))]);
        assert_eq!(kinds(&f.rows(None)), vec![RowKind::Conn { conn: 0, group: None, open: false }]);
    }

    #[test]
    fn the_hierarchy_opens_level_by_level_with_the_old_indents() {
        let mut f = Fixture::new(vec![conn("a", "postgres", "", pg(vec![table("orders", &["id", "total"]), table("users", &[])]))]);
        f.expanded.insert("a".into());
        f.tables.insert(database_key("a", "app"));
        f.tables.insert(schema_key("a", "public"));
        f.tables.insert(tables_key("a", Some("public")));
        f.tables.insert(table_key("a", Some("public"), "orders"));
        let rows = f.rows(None);
        let got: Vec<(RowKind, f32, f32)> = rows.iter().map(|r| (r.kind, r.indent, r.gap)).collect();
        assert_eq!(
            got,
            vec![
                (RowKind::Conn { conn: 0, group: None, open: true }, 0.0, 0.0),
                (RowKind::Database { conn: 0, db: 0, open: true }, 32.0, 2.0),
                (RowKind::Schema { conn: 0, db: 0, schema: 0, open: true }, 48.0, 2.0),
                (RowKind::Tables { conn: 0, db: 0, schema: 0, count: 2, open: true }, 80.0, 2.0),
                (RowKind::Table { conn: 0, db: 0, schema: 0, table: 0, open: true }, 96.0, 0.0),
                (RowKind::Column { conn: 0, db: 0, schema: 0, table: 0, column: 0 }, 120.0, 0.0),
                (RowKind::Column { conn: 0, db: 0, schema: 0, table: 0, column: 1 }, 120.0, 0.0),
                (RowKind::Table { conn: 0, db: 0, schema: 0, table: 1, open: false }, 96.0, 0.0),
                (RowKind::LeafSection { conn: 0, db: 0, schema: 0, leaf: Leaf::Views, count: 1, open: false }, 80.0, 2.0),
                (RowKind::Gap, 0.0, 0.0),
            ]
        );
    }

    #[test]
    fn grouped_connections_only_show_when_their_group_is_open() {
        let mut f = Fixture::new(vec![conn("a", "sqlite", "", sqlite(vec![])), conn("b", "sqlite", "", sqlite(vec![]))]);
        f.groups = vec![ServerGroup { id: "g".into(), title: "G".into(), connection_ids: vec!["b".into()] }];
        assert_eq!(
            kinds(&f.rows(None)),
            vec![RowKind::Group { group: 0, count: 1, open: false }, RowKind::Conn { conn: 0, group: None, open: false }]
        );
        f.egroups.insert("g".into());
        assert_eq!(
            kinds(&f.rows(None)),
            vec![
                RowKind::Group { group: 0, count: 1, open: true },
                RowKind::Conn { conn: 1, group: Some(0), open: false },
                RowKind::Conn { conn: 0, group: None, open: false },
            ]
        );
    }

    #[test]
    fn filtering_opens_everything_and_keeps_only_matches() {
        let conns = vec![
            conn("a", "postgres", "", pg(vec![table("orders", &["id"]), table("users", &["email"])])),
            conn("b", "sqlite", "", sqlite(vec![table("logs", &["msg"])])),
        ];
        let f = Fixture::new(conns);
        let indexes: Vec<Arc<SchemaIndex>> =
            f.conns.iter().map(|c| Arc::new(SchemaIndex::build(&c.config.id, 1, c.schema.as_ref().unwrap()))).collect();
        let m = search(&indexes, "email");
        assert_eq!(
            kinds(&f.rows(Some(&m))),
            vec![
                RowKind::Conn { conn: 0, group: None, open: true },
                RowKind::Database { conn: 0, db: 0, open: true },
                RowKind::Schema { conn: 0, db: 0, schema: 0, open: true },
                RowKind::Tables { conn: 0, db: 0, schema: 0, count: 1, open: true },
                RowKind::Table { conn: 0, db: 0, schema: 0, table: 1, open: false },
                RowKind::Gap,
            ]
        );
    }

    #[test]
    fn connections_without_a_schema_show_why() {
        let mut c = ActiveConnection::new(ConnectionConfig { id: "a".into(), ..Default::default() });
        c.schema_loading = true;
        let mut f = Fixture::new(vec![c]);
        f.expanded.insert("a".into());
        assert_eq!(
            kinds(&f.rows(None)),
            vec![
                RowKind::Conn { conn: 0, group: None, open: true },
                RowKind::Info { conn: 0, info: Info::Loading },
                RowKind::Gap,
            ]
        );
    }

    fn uniform(n: usize, h: f32) -> Vec<Row> {
        (0..n).map(|_| Row { kind: RowKind::Gap, indent: 0.0, gap: 0.0, height: h }).collect()
    }

    #[test]
    fn only_the_rows_in_view_are_drawn_and_the_spacers_keep_the_height() {
        let rows = uniform(10_000, 20.0);
        let (range, before, after) = visible_range(&rows, 50_000.0, 400.0, 0.0);
        assert_eq!(range, 2500..2520);
        assert_eq!(before, 50_000.0);
        let drawn: f32 = rows[range].iter().map(Row::extent).sum();
        assert_eq!(before + drawn + after, total_height(&rows));
    }

    #[test]
    fn overdraw_extends_both_ways_and_clamps_at_the_ends() {
        let rows = uniform(100, 20.0);
        let (range, before, _) = visible_range(&rows, 0.0, 100.0, 40.0);
        assert_eq!((range, before), (0..7, 0.0));
        let (range, _, after) = visible_range(&rows, 1_900.0, 100.0, 40.0);
        assert_eq!((range, after), (93..100, 0.0));
    }

    #[test]
    fn a_partly_visible_row_is_drawn() {
        let rows = uniform(10, 20.0);
        let (range, before, _) = visible_range(&rows, 30.0, 15.0, 0.0);
        assert_eq!((range, before), (1..3, 20.0));
    }

    #[test]
    fn gaps_count_towards_the_extent() {
        let mut rows = uniform(3, 20.0);
        rows[1].gap = 2.0;
        assert_eq!(total_height(&rows), 62.0);
        let (range, before, _) = visible_range(&rows, 21.0, 1.0, 0.0);
        assert_eq!((range, before), (1..2, 20.0));
    }
}
