//! Normalised schema model used by completion and lint (port of the
//! `DbSchema` types in `lib/sqlComplete.ts`).

use crate::models::SchemaTree;

#[derive(Debug, Clone, PartialEq)]
pub struct ColInfo {
    pub name: String,
    pub col_type: String,
    pub key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct TableInfo {
    pub name: String,
    pub columns: Vec<ColInfo>,
    pub is_view: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SchemaInfo {
    pub name: String,
    pub tables: Vec<TableInfo>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct DbSchema {
    pub driver: String,
    /// Some(..) when the connection exposes schemas (Postgres/MySQL).
    pub schemas: Option<Vec<SchemaInfo>>,
    pub tables: Vec<TableInfo>,
    pub views: Vec<TableInfo>,
}

impl DbSchema {
    pub fn all_flat_tables(&self) -> impl Iterator<Item = &TableInfo> {
        self.tables.iter().chain(self.views.iter())
    }
}

fn cols(t: &crate::models::Table) -> Vec<ColInfo> {
    t.columns
        .iter()
        .map(|c| ColInfo { name: c.name.clone(), col_type: c.column_type.clone(), key: c.key.clone() })
        .collect()
}

/// `getConnectionDbSchema` from SqlEditor.svelte.
pub fn db_schema_for(tree: &SchemaTree, driver: &str, database_name: &str) -> DbSchema {
    let driver = if driver.is_empty() { "postgres" } else { driver };
    let database = tree.databases.iter().find(|d| d.name == database_name);
    let schemas = match database {
        Some(db) => &db.schemas,
        None => &tree.schemas,
    };
    if !schemas.is_empty() {
        return DbSchema {
            driver: driver.to_string(),
            schemas: Some(
                schemas
                    .iter()
                    .map(|s| SchemaInfo {
                        name: s.name.clone(),
                        tables: s
                            .tables
                            .iter()
                            .map(|t| TableInfo { name: t.name.clone(), columns: cols(t), is_view: false })
                            .chain(s.views.iter().map(|t| TableInfo { name: t.name.clone(), columns: cols(t), is_view: true }))
                            .collect(),
                    })
                    .collect(),
            ),
            tables: Vec::new(),
            views: Vec::new(),
        };
    }
    DbSchema {
        driver: driver.to_string(),
        schemas: None,
        tables: tree
            .tables
            .iter()
            .map(|t| TableInfo { name: t.name.clone(), columns: cols(t), is_view: false })
            .collect(),
        views: tree
            .views
            .iter()
            .map(|t| TableInfo { name: t.name.clone(), columns: cols(t), is_view: true })
            .collect(),
    }
}

/// SQLNamespace for lang-sql's schema completion: nested levels of names.
#[derive(Debug, Clone, Default)]
pub struct Namespace {
    /// Ordered (name, child) entries; a child with `columns` is a table.
    pub entries: Vec<(String, NamespaceNode)>,
}

#[derive(Debug, Clone)]
pub enum NamespaceNode {
    Columns(Vec<String>),
    Nested(Namespace),
}

impl Namespace {
    fn set(&mut self, name: &str, node: NamespaceNode) {
        if let Some(entry) = self.entries.iter_mut().find(|(n, _)| n == name) {
            entry.1 = node;
        } else {
            self.entries.push((name.to_string(), node));
        }
    }
    fn has(&self, name: &str) -> bool {
        self.entries.iter().any(|(n, _)| n == name)
    }
}

/// `buildSqlNamespace` from sqlComplete.ts.
pub fn build_namespace(db: &DbSchema) -> Namespace {
    let mut ns = Namespace::default();
    match &db.schemas {
        Some(schemas) if !schemas.is_empty() => {
            for schema in schemas {
                let mut schema_ns = Namespace::default();
                for t in &schema.tables {
                    let cols: Vec<String> = t.columns.iter().map(|c| c.name.clone()).collect();
                    schema_ns.set(&t.name, NamespaceNode::Columns(cols.clone()));
                    if !ns.has(&t.name) {
                        ns.set(&t.name, NamespaceNode::Columns(cols));
                    }
                }
                ns.set(&schema.name, NamespaceNode::Nested(schema_ns));
            }
        }
        _ => {
            for t in db.all_flat_tables() {
                ns.set(&t.name, NamespaceNode::Columns(t.columns.iter().map(|c| c.name.clone()).collect()));
            }
        }
    }
    ns
}
