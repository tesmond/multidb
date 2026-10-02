//! Application state that used to live in Svelte stores (`stores/appStore.ts`)
//! and `localStorage`.

use crate::models::{ConnectionConfig, SchemaTree};
use crate::ui::relationship::Point as LayoutPoint;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;

pub type TabId = String;

pub fn new_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

#[derive(Clone)]
pub struct ActiveConnection {
    pub config: ConnectionConfig,
    pub schema: Option<Arc<SchemaTree>>,
    pub schema_loading: bool,
    pub schema_error: Option<String>,
}

impl ActiveConnection {
    pub fn new(config: ConnectionConfig) -> Self {
        ActiveConnection { config, schema: None, schema_loading: false, schema_error: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerGroup {
    pub id: String,
    pub title: String,
    pub connection_ids: Vec<String>,
}

/// Rows arrive in chunks while a query streams; storing the chunks behind
/// `Arc`s makes appending and snapshotting cheap for million-row results.
#[derive(Clone, Default)]
pub struct Rows {
    chunks: Vec<Arc<Vec<Vec<Value>>>>,
    starts: Vec<usize>,
    len: usize,
}

impl Rows {
    pub fn push_chunk(&mut self, rows: Vec<Vec<Value>>) {
        if rows.is_empty() {
            return;
        }
        self.starts.push(self.len);
        self.len += rows.len();
        self.chunks.push(Arc::new(rows));
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn get(&self, index: usize) -> Option<&Vec<Value>> {
        if index >= self.len {
            return None;
        }
        let chunk = match self.starts.binary_search(&index) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        self.chunks[chunk].get(index - self.starts[chunk])
    }

    pub fn iter(&self) -> impl Iterator<Item = &Vec<Value>> {
        self.chunks.iter().flat_map(|c| c.iter())
    }

    pub fn to_vec(&self) -> Vec<Vec<Value>> {
        self.iter().cloned().collect()
    }
}

#[derive(Clone, Default)]
pub struct QueryResult {
    pub columns: Arc<Vec<String>>,
    pub column_types: Arc<Vec<String>>,
    pub rows: Rows,
    pub rows_affected: i64,
    pub duration: i64,
    pub error: String,
    /// Bumped whenever a new result (column set) replaces the old one.
    pub generation: u64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EditInfo {
    pub table_name: String,
    pub schema_name: String,
    pub primary_key_cols: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OutputTab {
    Results,
    Messages,
    History,
    Saved,
}

impl OutputTab {
    pub const ALL: [(OutputTab, &'static str); 4] = [
        (OutputTab::Results, "Results"),
        (OutputTab::Messages, "Messages"),
        (OutputTab::History, "History"),
        (OutputTab::Saved, "Saved"),
    ];
}

/// Pending cell edits: row index → [(column index, new text)] in insertion order.
pub type PendingEdits = Vec<(usize, Vec<(usize, String)>)>;

pub fn pending_get(edits: &PendingEdits, row: usize, col: usize) -> Option<&String> {
    edits
        .iter()
        .find(|(r, _)| *r == row)
        .and_then(|(_, cols)| cols.iter().find(|(c, _)| *c == col).map(|(_, v)| v))
}

// ─── Persisted UI state (was localStorage) ───────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSettings {
    pub server_groups: Vec<ServerGroup>,
    pub font_scale_percent: Option<f64>,
    pub connection_order: Vec<String>,
    pub relationship_layouts: BTreeMap<String, HashMap<String, LayoutPoint>>,
}

fn settings_path() -> PathBuf {
    dirs::config_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("multidb")
        .join("ui-settings.json")
}

impl UiSettings {
    pub fn load() -> Self {
        std::fs::read(settings_path())
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }

    pub fn save(&self) {
        let path = settings_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(json) = serde_json::to_vec_pretty(self) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }

    pub fn font_scale(&self) -> f64 {
        clamp_font_scale(self.font_scale_percent.unwrap_or(100.0))
    }
}

pub fn clamp_font_scale(v: f64) -> f64 {
    if !v.is_finite() {
        return 100.0;
    }
    v.round().clamp(50.0, 250.0)
}

/// `buildConnectionDisplayStructure` from appStore.ts.
pub struct DisplayStructure<'a> {
    pub groups: Vec<(&'a ServerGroup, Vec<&'a ActiveConnection>)>,
    pub ungrouped: Vec<&'a ActiveConnection>,
}

pub fn display_structure<'a>(connections: &'a [ActiveConnection], groups: &'a [ServerGroup]) -> DisplayStructure<'a> {
    let mut included: std::collections::HashSet<&str> = Default::default();
    let groups_out = groups
        .iter()
        .map(|g| {
            let conns = g
                .connection_ids
                .iter()
                .filter_map(|id| {
                    let conn = connections.iter().find(|c| &c.config.id == id)?;
                    if !included.insert(id.as_str()) {
                        return None;
                    }
                    Some(conn)
                })
                .collect();
            (g, conns)
        })
        .collect();
    let ungrouped = connections.iter().filter(|c| !included.contains(c.config.id.as_str())).collect();
    DisplayStructure { groups: groups_out, ungrouped }
}

pub fn ordered_connection_options(connections: &[ActiveConnection], groups: &[ServerGroup]) -> Vec<(String, String)> {
    let s = display_structure(connections, groups);
    let mut out = Vec::new();
    for (g, conns) in &s.groups {
        for c in conns {
            out.push((c.config.id.clone(), format!("{} - {}", g.title, c.config.name)));
        }
    }
    for c in &s.ungrouped {
        out.push((c.config.id.clone(), c.config.name.clone()));
    }
    out
}

pub fn apply_connection_order(mut connections: Vec<ActiveConnection>, order: &[String]) -> Vec<ActiveConnection> {
    if order.is_empty() {
        return connections;
    }
    let rank = |id: &str| order.iter().position(|o| o == id);
    connections.sort_by(|a, b| match (rank(&a.config.id), rank(&b.config.id)) {
        (None, None) => std::cmp::Ordering::Equal,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (Some(_), None) => std::cmp::Ordering::Less,
        (Some(x), Some(y)) => x.cmp(&y),
    });
    connections
}

pub fn format_bytes(bytes: Option<i64>) -> String {
    let Some(bytes) = bytes.filter(|b| *b > 0) else { return String::new() };
    let units = ["B", "KB", "MB", "GB", "TB", "PB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < units.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    let precision = if value >= 10.0 || unit == 0 { 0 } else { 1 };
    format!("{} {}", js_to_fixed(value, precision), units[unit])
}

/// `Number.prototype.toFixed` (round-half-away-from-zero on the decimal value).
pub fn js_to_fixed(value: f64, digits: usize) -> String {
    format!("{:.*}", digits, value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_chunk_lookup() {
        let mut rows = Rows::default();
        rows.push_chunk(vec![vec![Value::from(1)], vec![Value::from(2)]]);
        rows.push_chunk(vec![vec![Value::from(3)]]);
        assert_eq!(rows.len(), 3);
        assert_eq!(rows.get(2).unwrap()[0], Value::from(3));
        assert_eq!(rows.get(1).unwrap()[0], Value::from(2));
        assert!(rows.get(3).is_none());
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(Some(512)), "512 B");
        assert_eq!(format_bytes(Some(1536)), "1.5 KB");
        assert_eq!(format_bytes(Some(245760)), "240 KB");
        assert_eq!(format_bytes(None), "");
    }
}
