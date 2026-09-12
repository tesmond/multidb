//! The navigator's filter index.
//!
//! "Filter tables or columns" used to walk every database, schema, table and
//! column of every connection — lower-casing each name as it went — and it did
//! that again on every frame, so a large schema stalled the window on each
//! keystroke. Instead each connection's schema is flattened once, when it
//! loads, into lower-cased names plus a `BTreeMap` of them; searching that is
//! then cheap enough to run once per keystroke, on a background thread.

use crate::models::SchemaTree;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Bound;
use std::sync::Arc;

/// Where a table sits in a schema tree: `(database, schema, table)` positions,
/// with [`NONE`] where that level does not exist (MySQL without databases,
/// SQLite). They index the snapshot the index was built from, which is why
/// [`SchemaIndex::generation`] is checked before results are used.
pub type TableKey = (u32, u32, u32);
pub const NONE: u32 = u32::MAX;

struct IndexedTable {
    key: TableKey,
    /// Lower-cased table name.
    name: Box<str>,
    /// Lower-cased column names, joined by `\n`.
    columns: Box<str>,
}

/// One connection's searchable names.
pub struct SchemaIndex {
    pub conn_id: String,
    /// Bumped whenever the connection's schema is replaced, so a search that
    /// was in flight over the old one can be discarded.
    pub generation: u64,
    tables: Vec<IndexedTable>,
    /// Lower-cased table and column names to the tables that carry them, so a
    /// filter that is the start of a name is a range scan rather than a walk.
    by_name: BTreeMap<Box<str>, Vec<u32>>,
}

impl SchemaIndex {
    pub fn build(conn_id: &str, generation: u64, schema: &SchemaTree) -> Self {
        let mut tables: Vec<IndexedTable> = Vec::new();
        let mut push = |db: u32, sc: u32, ti: u32, t: &crate::models::Table| {
            let mut columns = String::new();
            for c in &t.columns {
                if !columns.is_empty() {
                    columns.push('\n');
                }
                columns.push_str(&c.name.to_lowercase());
            }
            tables.push(IndexedTable {
                key: (db, sc, ti),
                name: t.name.to_lowercase().into_boxed_str(),
                columns: columns.into_boxed_str(),
            });
        };
        if !schema.databases.is_empty() {
            for (di, d) in schema.databases.iter().enumerate() {
                for (si, sc) in d.schemas.iter().enumerate() {
                    for (ti, t) in sc.tables.iter().enumerate() {
                        push(di as u32, si as u32, ti as u32, t);
                    }
                }
            }
        } else if !schema.schemas.is_empty() {
            for (si, sc) in schema.schemas.iter().enumerate() {
                for (ti, t) in sc.tables.iter().enumerate() {
                    push(NONE, si as u32, ti as u32, t);
                }
            }
        } else {
            for (ti, t) in schema.tables.iter().enumerate() {
                push(NONE, NONE, ti as u32, t);
            }
        }

        let mut by_name: BTreeMap<Box<str>, Vec<u32>> = BTreeMap::new();
        for (slot, t) in tables.iter().enumerate() {
            by_name.entry(t.name.clone()).or_default().push(slot as u32);
            for c in t.columns.split('\n').filter(|c| !c.is_empty()) {
                by_name.entry(c.into()).or_default().push(slot as u32);
            }
        }
        SchemaIndex { conn_id: conn_id.to_string(), generation, tables, by_name }
    }

    pub fn table_count(&self) -> usize {
        self.tables.len()
    }

    /// Slots whose table or column name begins with `filter`, straight out of
    /// the ordered map.
    fn prefix_slots(&self, filter: &str, out: &mut HashSet<u32>) {
        for (name, slots) in self.by_name.range::<str, _>((Bound::Included(filter), Bound::Unbounded)) {
            if !name.starts_with(filter) {
                break;
            }
            out.extend(slots.iter().copied());
        }
    }
}

/// What survives the filter, by connection and by table.
#[derive(Default, Debug)]
pub struct Matches {
    pub filter: String,
    /// Connection ids with at least one matching table.
    pub conns: HashSet<String>,
    /// Matching tables per connection.
    pub tables: HashMap<String, HashSet<TableKey>>,
    /// The generation each connection's index was at, so stale results are
    /// ignored rather than mapped onto a schema that has since changed.
    pub generations: HashMap<String, u64>,
}

impl Matches {
    pub fn conn_matches(&self, conn_id: &str) -> bool {
        self.conns.contains(conn_id)
    }

    pub fn table_matches(&self, conn_id: &str, key: TableKey) -> bool {
        self.tables.get(conn_id).is_some_and(|t| t.contains(&key))
    }
}

/// Search every index for `filter` (already lower-cased and trimmed). Pure and
/// allocation-light, so it can run on a background thread.
pub fn search(indexes: &[Arc<SchemaIndex>], filter: &str) -> Matches {
    let mut m = Matches { filter: filter.to_string(), ..Default::default() };
    if filter.is_empty() {
        return m;
    }
    for index in indexes {
        // Names that start with the filter come out of the map directly; the
        // rest have to be looked at, but they are already lower-cased.
        let mut slots: HashSet<u32> = HashSet::new();
        index.prefix_slots(filter, &mut slots);
        for (slot, t) in index.tables.iter().enumerate() {
            let slot = slot as u32;
            if slots.contains(&slot) {
                continue;
            }
            if t.name.contains(filter) || t.columns.contains(filter) {
                slots.insert(slot);
            }
        }
        if slots.is_empty() {
            continue;
        }
        let keys: HashSet<TableKey> = slots.iter().map(|&s| index.tables[s as usize].key).collect();
        m.conns.insert(index.conn_id.clone());
        m.tables.insert(index.conn_id.clone(), keys);
        m.generations.insert(index.conn_id.clone(), index.generation);
    }
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Column, Schema, Table};

    fn table(name: &str, columns: &[&str]) -> Table {
        Table {
            name: name.into(),
            table_type: "table".into(),
            size_bytes: None,
            columns: columns
                .iter()
                .map(|c| Column { name: (*c).into(), ..Default::default() })
                .collect(),
        }
    }

    fn tree() -> SchemaTree {
        SchemaTree {
            schemas: vec![Schema {
                name: "public".into(),
                size_bytes: None,
                tables: vec![
                    table("Orders", &["id", "customer_id", "placed_at"]),
                    table("customers", &["id", "email"]),
                    table("order_items", &["order_id", "sku"]),
                ],
                views: Vec::new(),
                indexes: Vec::new(),
            }],
            ..Default::default()
        }
    }

    fn found(m: &Matches) -> Vec<TableKey> {
        let mut v: Vec<TableKey> = m.tables.get("c1").cloned().unwrap_or_default().into_iter().collect();
        v.sort();
        v
    }

    #[test]
    fn matches_table_names_regardless_of_case() {
        let idx = Arc::new(SchemaIndex::build("c1", 1, &tree()));
        // "Orders" and "order_items" both contain "order".
        assert_eq!(found(&search(&[idx.clone()], "order")), vec![(NONE, 0, 0), (NONE, 0, 2)]);
        assert_eq!(found(&search(&[idx], "ORDERS".to_lowercase().as_str())), vec![(NONE, 0, 0)]);
    }

    #[test]
    fn matches_column_names_too() {
        let idx = Arc::new(SchemaIndex::build("c1", 1, &tree()));
        // Only customers has an "email" column.
        assert_eq!(found(&search(&[idx.clone()], "email")), vec![(NONE, 0, 1)]);
        // "customer_id" is a column of Orders, and "customers" is a table.
        assert_eq!(found(&search(&[idx], "customer")), vec![(NONE, 0, 0), (NONE, 0, 1)]);
    }

    #[test]
    fn a_filter_that_matches_nothing_leaves_the_connection_out() {
        let idx = Arc::new(SchemaIndex::build("c1", 1, &tree()));
        let m = search(&[idx], "nothing_here");
        assert!(m.conns.is_empty());
        assert!(!m.conn_matches("c1"));
    }

    #[test]
    fn substrings_that_are_not_prefixes_still_match() {
        let idx = Arc::new(SchemaIndex::build("c1", 1, &tree()));
        // "_items" starts no name, so this is the scan rather than the map.
        assert_eq!(found(&search(&[idx], "_items")), vec![(NONE, 0, 2)]);
    }

    #[test]
    fn keys_follow_the_shape_of_the_tree() {
        // Flat (SQLite): no database, no schema.
        let flat = SchemaTree { tables: vec![table("t", &["a"])], ..Default::default() };
        let idx = Arc::new(SchemaIndex::build("c1", 1, &flat));
        assert_eq!(found(&search(&[idx], "t")), vec![(NONE, NONE, 0)]);

        // Postgres: databases, each with schemas.
        let nested = SchemaTree {
            databases: vec![crate::models::Database {
                name: "app".into(),
                schemas: vec![Schema { name: "public".into(), tables: vec![table("t", &["a"])], ..Default::default() }],
                ..Default::default()
            }],
            ..Default::default()
        };
        let idx = Arc::new(SchemaIndex::build("c1", 1, &nested));
        assert_eq!(found(&search(&[idx], "t")), vec![(0, 0, 0)]);
    }
}
