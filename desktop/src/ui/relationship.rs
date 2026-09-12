//! Relationship diagram graph + deterministic layout (port of
//! `lib/relationshipDiagram.ts`).

use super::js::locale_compare;
use crate::models::{Relationship, SchemaTree, Table};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};

pub const HEADER_HEIGHT: f32 = 40.0;
pub const ROW_HEIGHT: f32 = 24.0;
const MIN_WIDTH: f32 = 220.0;
const WIDTH_PER_CHAR: f32 = 7.0;
const COLUMN_PADDING: f32 = 56.0;
const CLUSTER_GAP_X: f32 = 180.0;
const LAYER_GAP_X: f32 = 280.0;
const NODE_GAP_Y: f32 = 48.0;

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramColumn {
    pub id: String,
    pub name: String,
    pub column_type: String,
    pub key: String,
    pub is_primary_key: bool,
    pub is_foreign_key: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramTable {
    pub id: String,
    pub schema_name: String,
    pub table_name: String,
    pub title: String,
    pub width: f32,
    pub height: f32,
    pub columns: Vec<DiagramColumn>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DiagramEdge {
    pub id: String,
    pub constraint_name: String,
    pub source_table_id: String,
    pub target_table_id: String,
    pub source_column_ids: Vec<String>,
    pub target_column_ids: Vec<String>,
    pub on_update: String,
    pub on_delete: String,
    pub is_self_referential: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct DiagramGraph {
    pub tables: Vec<DiagramTable>,
    pub edges: Vec<DiagramEdge>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct Point {
    pub x: f32,
    pub y: f32,
}

pub type Layout = HashMap<String, Point>;

fn make_table_id(schema: &str, table: &str) -> String {
    if schema.is_empty() {
        table.to_string()
    } else {
        format!("{schema}.{table}")
    }
}

fn make_column_id(table_id: &str, column: &str) -> String {
    format!("{table_id}.{column}")
}

pub fn build_graph(tree: &SchemaTree) -> DiagramGraph {
    let fk_columns = collect_foreign_key_columns(&tree.relationships);
    let mut tables: Vec<DiagramTable> = collect_tables(tree)
        .into_iter()
        .map(|(schema, table)| make_table_node(&schema, table, &fk_columns))
        .collect();
    tables.sort_by(|a, b| locale_compare(&a.id, &b.id));
    let mut edges: Vec<DiagramEdge> = tree.relationships.iter().map(make_edge).collect();
    edges.sort_by(|a, b| locale_compare(&a.id, &b.id));
    DiagramGraph { tables, edges }
}

fn collect_tables(tree: &SchemaTree) -> Vec<(String, &Table)> {
    if !tree.schemas.is_empty() {
        return tree
            .schemas
            .iter()
            .flat_map(|s| s.tables.iter().map(move |t| (s.name.clone(), t)))
            .collect();
    }
    tree.tables.iter().map(|t| (String::new(), t)).collect()
}

fn collect_foreign_key_columns(relationships: &[Relationship]) -> HashSet<String> {
    let mut out = HashSet::new();
    for rel in relationships {
        let table_id = make_table_id(&rel.source_table.schema_name, &rel.source_table.table_name);
        for pair in &rel.column_pairs {
            out.insert(make_column_id(&table_id, &pair.source_column));
        }
    }
    out
}

fn make_table_node(schema: &str, table: &Table, fk: &HashSet<String>) -> DiagramTable {
    let table_id = make_table_id(schema, &table.name);
    let columns: Vec<DiagramColumn> = table
        .columns
        .iter()
        .map(|c| {
            let id = make_column_id(&table_id, &c.name);
            DiagramColumn {
                is_foreign_key: fk.contains(&id),
                id,
                name: c.name.clone(),
                column_type: c.column_type.clone(),
                key: c.key.clone(),
                is_primary_key: c.key == "PRI",
            }
        })
        .collect();
    let title = if schema.is_empty() {
        table.name.clone()
    } else {
        format!("{schema}.{}", table.name)
    };
    let width = estimate_width(&title, &columns);
    DiagramTable {
        id: table_id,
        schema_name: schema.to_string(),
        table_name: table.name.clone(),
        title,
        width,
        height: HEADER_HEIGHT + columns.len() as f32 * ROW_HEIGHT,
        columns,
    }
}

fn utf16_len(s: &str) -> usize {
    s.encode_utf16().count()
}

fn estimate_width(header: &str, columns: &[DiagramColumn]) -> f32 {
    let widest = columns.iter().fold(utf16_len(header), |max, c| {
        max.max(utf16_len(&format!("{}: {}", c.name, c.column_type)))
    });
    MIN_WIDTH.max(widest as f32 * WIDTH_PER_CHAR + COLUMN_PADDING)
}

fn make_edge(rel: &Relationship) -> DiagramEdge {
    let source = make_table_id(&rel.source_table.schema_name, &rel.source_table.table_name);
    let target = make_table_id(&rel.target_table.schema_name, &rel.target_table.table_name);
    DiagramEdge {
        id: format!("{source}::{}", rel.constraint_name),
        constraint_name: rel.constraint_name.clone(),
        source_column_ids: rel
            .column_pairs
            .iter()
            .map(|p| make_column_id(&source, &p.source_column))
            .collect(),
        target_column_ids: rel
            .column_pairs
            .iter()
            .map(|p| make_column_id(&target, &p.target_column))
            .collect(),
        on_update: rel.on_update.clone(),
        on_delete: rel.on_delete.clone(),
        is_self_referential: source == target,
        source_table_id: source,
        target_table_id: target,
    }
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort_by(|a, b| locale_compare(a, b));
    v
}

pub fn default_layout(graph: &DiagramGraph) -> Layout {
    let table_ids: Vec<String> = graph.tables.iter().map(|t| t.id.clone()).collect();
    let by_id: HashMap<&str, &DiagramTable> = graph.tables.iter().map(|t| (t.id.as_str(), t)).collect();
    // Insertion-ordered sets mirror JS Set iteration order.
    let mut undirected: HashMap<String, Vec<String>> = HashMap::new();
    let mut parent_to_children: HashMap<String, Vec<String>> = HashMap::new();
    for id in &table_ids {
        undirected.insert(id.clone(), Vec::new());
        parent_to_children.insert(id.clone(), Vec::new());
    }
    let push_unique = |map: &mut HashMap<String, Vec<String>>, key: &str, value: &str| {
        if let Some(list) = map.get_mut(key) {
            if !list.iter().any(|v| v == value) {
                list.push(value.to_string());
            }
        }
    };
    for edge in &graph.edges {
        push_unique(&mut undirected, &edge.source_table_id, &edge.target_table_id);
        push_unique(&mut undirected, &edge.target_table_id, &edge.source_table_id);
        if edge.is_self_referential {
            continue;
        }
        push_unique(&mut parent_to_children, &edge.target_table_id, &edge.source_table_id);
    }

    let components = collect_components(&table_ids, &undirected);
    let mut layout = Layout::new();
    let mut cluster_x = 0.0f32;

    for component in components {
        let set: HashSet<&str> = component.iter().map(String::as_str).collect();
        let mut parents: HashMap<String, Vec<String>> = HashMap::new();
        let mut children: HashMap<String, Vec<String>> = HashMap::new();
        let mut indegree: HashMap<String, i32> = HashMap::new();
        for id in &component {
            parents.insert(id.clone(), Vec::new());
            children.insert(id.clone(), Vec::new());
            indegree.insert(id.clone(), 0);
        }
        for parent in &component {
            for child in parent_to_children.get(parent).cloned().unwrap_or_default() {
                if !set.contains(child.as_str()) {
                    continue;
                }
                push_unique(&mut children, parent, &child);
                push_unique(&mut parents, &child, parent);
                *indegree.entry(child.clone()).or_insert(0) += 1;
            }
        }
        let layers = assign_layers(&component, &children, &parents, &indegree);
        let mut layer_ids: Vec<i32> = component.iter().map(|id| *layers.get(id).unwrap_or(&0)).collect();
        layer_ids.sort_unstable();
        layer_ids.dedup();
        let mut component_width = 0.0f32;
        for layer in layer_ids {
            let layer_tables = sorted(
                component
                    .iter()
                    .filter(|id| *layers.get(*id).unwrap_or(&0) == layer)
                    .cloned()
                    .collect(),
            );
            let mut current_y = 0.0f32;
            let mut widest = 0.0f32;
            for id in layer_tables {
                let Some(table) = by_id.get(id.as_str()) else { continue };
                layout.insert(
                    id.clone(),
                    Point { x: cluster_x + layer as f32 * LAYER_GAP_X, y: current_y },
                );
                widest = widest.max(table.width);
                current_y += table.height + NODE_GAP_Y;
            }
            component_width = component_width.max(layer as f32 * LAYER_GAP_X + widest);
        }
        cluster_x += component_width + CLUSTER_GAP_X;
    }
    layout
}

fn collect_components(table_ids: &[String], adjacency: &HashMap<String, Vec<String>>) -> Vec<Vec<String>> {
    let mut visited: HashSet<String> = HashSet::new();
    let mut components = Vec::new();
    for id in sorted(table_ids.to_vec()) {
        if visited.contains(&id) {
            continue;
        }
        let mut stack = vec![id.clone()];
        let mut component = Vec::new();
        visited.insert(id.clone());
        while let Some(current) = stack.pop() {
            component.push(current.clone());
            let neighbors = sorted(adjacency.get(&current).cloned().unwrap_or_default());
            for n in neighbors {
                if visited.insert(n.clone()) {
                    stack.push(n);
                }
            }
        }
        components.push(sorted(component));
    }
    components.sort_by(|a, b| locale_compare(&a[0], &b[0]));
    components
}

fn assign_layers(
    component: &[String],
    children: &HashMap<String, Vec<String>>,
    parents: &HashMap<String, Vec<String>>,
    indegree: &HashMap<String, i32>,
) -> HashMap<String, i32> {
    let mut queue: VecDeque<String> = sorted(
        component
            .iter()
            .filter(|id| *indegree.get(*id).unwrap_or(&0) == 0)
            .cloned()
            .collect(),
    )
    .into();
    let mut remaining = indegree.clone();
    let mut layers: HashMap<String, i32> = HashMap::new();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(current) = queue.pop_front() {
        seen.insert(current.clone());
        let parent_layers: Vec<i32> = parents
            .get(&current)
            .map(|ps| ps.iter().map(|p| *layers.get(p).unwrap_or(&0)).collect())
            .unwrap_or_default();
        let layer = parent_layers.iter().max().map(|m| m + 1).unwrap_or(0);
        layers.insert(current.clone(), layer);
        for child in sorted(children.get(&current).cloned().unwrap_or_default()) {
            let next = remaining.get(&child).copied().unwrap_or(0) - 1;
            remaining.insert(child.clone(), next);
            if next == 0 {
                queue.push_back(child);
            }
        }
        let mut v: Vec<String> = queue.drain(..).collect();
        v.sort_by(|a, b| locale_compare(a, b));
        queue = v.into();
    }
    for id in sorted(component.to_vec()) {
        if seen.contains(&id) {
            continue;
        }
        let parent_layers: Vec<i32> = parents
            .get(&id)
            .map(|ps| ps.iter().filter_map(|p| layers.get(p).copied()).collect())
            .unwrap_or_default();
        let layer = parent_layers.iter().max().map(|m| m + 1).unwrap_or(0);
        layers.insert(id, layer);
    }
    layers
}

pub fn filter_graph(graph: &DiagramGraph, filter: &str) -> DiagramGraph {
    let normalized = filter.trim().to_lowercase();
    if normalized.is_empty() {
        return graph.clone();
    }
    let matching: HashSet<&str> = graph
        .tables
        .iter()
        .filter(|t| {
            t.title.to_lowercase().contains(&normalized)
                || t.table_name.to_lowercase().contains(&normalized)
                || t.columns.iter().any(|c| c.name.to_lowercase().contains(&normalized))
        })
        .map(|t| t.id.as_str())
        .collect();
    let mut visible: HashSet<&str> = matching.clone();
    for e in &graph.edges {
        if matching.contains(e.source_table_id.as_str()) || matching.contains(e.target_table_id.as_str()) {
            visible.insert(&e.source_table_id);
            visible.insert(&e.target_table_id);
        }
    }
    DiagramGraph {
        tables: graph.tables.iter().filter(|t| visible.contains(t.id.as_str())).cloned().collect(),
        edges: graph
            .edges
            .iter()
            .filter(|e| visible.contains(e.source_table_id.as_str()) && visible.contains(e.target_table_id.as_str()))
            .cloned()
            .collect(),
    }
}

pub fn merge_layout(graph: &DiagramGraph, default: &Layout, persisted: Option<&Layout>) -> Layout {
    graph
        .tables
        .iter()
        .map(|t| {
            let p = persisted
                .and_then(|l| l.get(&t.id))
                .or_else(|| default.get(&t.id))
                .copied()
                .unwrap_or_default();
            (t.id.clone(), p)
        })
        .collect()
}

pub fn effective_selected_edge_id(edges: &[DiagramEdge], selected: &str) -> String {
    if edges.iter().any(|e| e.id == selected) {
        selected.to_string()
    } else {
        edges.first().map(|e| e.id.clone()).unwrap_or_default()
    }
}

pub fn query_sql(driver: &str, schema: &str, table: &str) -> String {
    let quoted = super::sql_text::quote_identifier(table, driver);
    let qualified = if schema.is_empty() {
        quoted
    } else {
        format!("{}.{quoted}", super::sql_text::quote_identifier(schema, driver))
    };
    format!("SELECT * FROM {qualified} LIMIT 100;")
}

/// FNV-1a over the UTF-16 code units of a stable (key-sorted) JSON encoding.
pub fn schema_hash(tree: &SchemaTree) -> String {
    let value = serde_json::to_value(tree).unwrap_or(Value::Null);
    let canonical = stable_stringify(&value);
    let mut hash: u32 = 2166136261;
    for unit in canonical.encode_utf16() {
        hash ^= unit as u32;
        hash = hash.wrapping_mul(16777619);
    }
    format!("{hash:08x}")
}

fn stable_stringify(value: &Value) -> String {
    match value {
        Value::Null => "null".into(),
        Value::Bool(_) | Value::String(_) => serde_json::to_string(value).unwrap_or_default(),
        Value::Number(_) => super::js::value_to_string(value),
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(stable_stringify).collect::<Vec<_>>().join(",")
        ),
        Value::Object(map) => {
            let mut entries: Vec<(&String, &Value)> = map.iter().collect();
            entries.sort_by(|a, b| locale_compare(a.0, b.0));
            format!(
                "{{{}}}",
                entries
                    .iter()
                    .map(|(k, v)| format!("{}:{}", serde_json::to_string(k).unwrap_or_default(), stable_stringify(v)))
                    .collect::<Vec<_>>()
                    .join(",")
            )
        }
    }
}

/// Persisted manual layouts keyed by `connId::schemaHash`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LayoutStore(pub BTreeMap<String, HashMap<String, Point>>);

impl LayoutStore {
    pub fn key(conn_id: &str, hash: &str) -> String {
        format!("{conn_id}::{hash}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Column, RelationshipColumnPair, RelationshipTableRef};

    fn table(name: &str, cols: &[(&str, &str)]) -> Table {
        Table {
            name: name.into(),
            table_type: "table".into(),
            size_bytes: None,
            columns: cols
                .iter()
                .map(|(n, k)| Column { name: (*n).into(), column_type: "int".into(), nullable: false, default: String::new(), key: (*k).into() })
                .collect(),
        }
    }

    fn rel(name: &str, src: &str, sc: &str, dst: &str, dc: &str) -> Relationship {
        Relationship {
            constraint_name: name.into(),
            source_table: RelationshipTableRef { schema_name: String::new(), table_name: src.into() },
            target_table: RelationshipTableRef { schema_name: String::new(), table_name: dst.into() },
            column_pairs: vec![RelationshipColumnPair { source_column: sc.into(), target_column: dc.into() }],
            on_update: String::new(),
            on_delete: "CASCADE".into(),
        }
    }

    #[test]
    fn layers_parents_before_children() {
        let tree = SchemaTree {
            tables: vec![
                table("orders", &[("id", "PRI"), ("customer_id", "")]),
                table("customers", &[("id", "PRI")]),
                table("audit", &[("id", "PRI")]),
            ],
            relationships: vec![rel("fk_orders_customer", "orders", "customer_id", "customers", "id")],
            ..Default::default()
        };
        let graph = build_graph(&tree);
        assert_eq!(graph.tables.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(), ["audit", "customers", "orders"]);
        assert!(graph.tables[2].columns[1].is_foreign_key);
        let layout = default_layout(&graph);
        assert_eq!(layout["audit"], Point { x: 0.0, y: 0.0 });
        assert_eq!(layout["customers"], Point { x: 400.0, y: 0.0 });
        assert_eq!(layout["orders"], Point { x: 680.0, y: 0.0 });
        let filtered = filter_graph(&graph, "cust");
        assert_eq!(filtered.tables.len(), 2);
        assert_eq!(query_sql("mysql", "", "orders"), "SELECT * FROM `orders` LIMIT 100;");
    }
}
