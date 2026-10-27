//! Application state that used to live in Svelte stores (`stores/appStore.ts`)
//! and `localStorage`.

use crate::models::{ConnectionConfig, SchemaTree};
use crate::ui::relationship::Point as LayoutPoint;
use gpui::SharedString;
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
    /// When the schema shown was last fetched from the database (from the
    /// schema cache, or a refresh this session); drives the background refresh.
    pub schema_refreshed_at: Option<std::time::SystemTime>,
}

impl ActiveConnection {
    pub fn new(config: ConnectionConfig) -> Self {
        ActiveConnection { config, schema: None, schema_loading: false, schema_error: None, schema_refreshed_at: None }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct ServerGroup {
    pub id: String,
    pub title: String,
    pub connection_ids: Vec<String>,
    /// Tab colour for the group's connections that have no colour of their
    /// own; empty for none.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tab_color: String,
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

    fn locate(&self, index: usize) -> Option<(usize, usize)> {
        if index >= self.len {
            return None;
        }
        let chunk = match self.starts.binary_search(&index) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        Some((chunk, index - self.starts[chunk]))
    }

    /// Replace one row. Only the chunk holding it is copied, and only if a
    /// snapshot still shares it.
    pub fn set(&mut self, index: usize, row: Vec<Value>) -> bool {
        let Some((chunk, at)) = self.locate(index) else { return false };
        Arc::make_mut(&mut self.chunks[chunk])[at] = row;
        true
    }

    /// Remove the rows at `indices`; the rest keep their order and move up.
    /// Only chunks that lose a row are copied.
    pub fn remove(&mut self, indices: &std::collections::BTreeSet<usize>) {
        if indices.is_empty() {
            return;
        }
        let mut chunks = Vec::with_capacity(self.chunks.len());
        for (c, chunk) in std::mem::take(&mut self.chunks).into_iter().enumerate() {
            let start = self.starts[c];
            let end = start + chunk.len();
            if indices.range(start..end).next().is_none() {
                chunks.push(chunk);
                continue;
            }
            let kept: Vec<Vec<Value>> = chunk
                .iter()
                .enumerate()
                .filter(|(i, _)| !indices.contains(&(start + i)))
                .map(|(_, r)| r.clone())
                .collect();
            if !kept.is_empty() {
                chunks.push(Arc::new(kept));
            }
        }
        self.starts.clear();
        self.len = 0;
        self.chunks.clear();
        for chunk in chunks {
            self.starts.push(self.len);
            self.len += chunk.len();
            self.chunks.push(chunk);
        }
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
    /// Bumped when rows of this result are changed in place (a save patched,
    /// removed or added rows), so anything derived from the rows (the sort
    /// order) is recomputed without treating it as a new result.
    pub rev: u64,
}

/// What makes a result editable: the table it reads and how its columns map
/// onto the table's.
#[derive(Clone, Debug, PartialEq)]
pub struct EditInfo {
    pub driver: crate::grid_edit::Driver,
    pub meta: Arc<crate::grid_edit::meta::TableMeta>,
    pub layout: crate::grid_edit::query::Layout,
}

impl EditInfo {
    /// The table column a result column shows, if any.
    pub fn column(&self, col: usize) -> Option<&crate::grid_edit::meta::ColumnMeta> {
        self.layout.sources.get(col).copied().flatten().and_then(|i| self.meta.columns.get(i))
    }

    /// Why a result column can't be edited; `None` if it can.
    pub fn readonly_reason(&self, col: usize) -> Option<&str> {
        match self.layout.readonly.get(col) {
            Some(Some(reason)) => Some(reason.as_str()),
            Some(None) => None,
            None => Some("unknown column"),
        }
    }

    pub fn nullable(&self, col: usize) -> bool {
        self.column(col).is_some_and(|c| c.nullable)
    }
}

/// Where a result came from: what to read the table's details against, and
/// where to save. Recorded when the query runs, so changing the editor text,
/// connection or default schema afterwards cannot redirect a save.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct ResultSource {
    pub sql: String,
    pub conn_id: String,
    pub driver: String,
    pub database: String,
    pub default_schema: String,
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

// ─── Persisted UI state (was localStorage) ───────────────────────────────────

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UiSettings {
    pub server_groups: Vec<ServerGroup>,
    pub font_scale_percent: Option<f64>,
    /// UI font family; `None` is the system font.
    pub ui_font_family: Option<String>,
    /// UI base font size in px; `None` is [`DEFAULT_FONT_SIZE`].
    pub ui_font_size: Option<f64>,
    /// SQL editor font family; `None` picks the first installed coding font.
    pub editor_font_family: Option<String>,
    /// SQL editor font size in px; `None` is [`DEFAULT_FONT_SIZE`].
    pub editor_font_size: Option<f64>,
    pub connection_order: Vec<String>,
    /// Style the Format button and generated SQL use.
    pub sql_style: crate::ui::sql::format::SqlStyle,
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

    pub fn ui_font_size(&self) -> f64 {
        clamp_font_size(self.ui_font_size.unwrap_or(DEFAULT_FONT_SIZE))
    }

    pub fn editor_font_size(&self) -> f64 {
        clamp_font_size(self.editor_font_size.unwrap_or(DEFAULT_FONT_SIZE))
    }
}

/// The base size the UI and editor were laid out at; the size settings are
/// relative to it.
pub const DEFAULT_FONT_SIZE: f64 = 13.0;
/// Sizes offered by the font size selects.
pub const FONT_SIZES: [u32; 15] = [8, 9, 10, 11, 12, 13, 14, 15, 16, 18, 20, 22, 24, 28, 32];

/// Installed font families, as the font selects list them.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FontLists {
    /// Every family, sorted.
    pub all: Vec<SharedString>,
    /// The monospaced ones, for the editor font select.
    pub mono: Vec<SharedString>,
}

#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
struct FontListsFile {
    all: Vec<String>,
    mono: Vec<String>,
}

fn font_cache_path() -> PathBuf {
    settings_path().with_file_name("font-cache.json")
}

impl FontLists {
    /// The list saved by the last refresh, so the selects have something to
    /// show at once while the installed fonts are listed again.
    pub fn load_cached() -> Option<Self> {
        let bytes = std::fs::read(font_cache_path()).ok()?;
        let file: FontListsFile = serde_json::from_slice(&bytes).ok()?;
        if file.all.is_empty() {
            return None;
        }
        Some(FontLists {
            all: file.all.into_iter().map(SharedString::from).collect(),
            mono: file.mono.into_iter().map(SharedString::from).collect(),
        })
    }

    pub fn save_cache(&self) {
        let path = font_cache_path();
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let file = FontListsFile {
            all: self.all.iter().map(|s| s.to_string()).collect(),
            mono: self.mono.iter().map(|s| s.to_string()).collect(),
        };
        if let Ok(json) = serde_json::to_vec(&file) {
            let tmp = path.with_extension("tmp");
            if std::fs::write(&tmp, json).is_ok() {
                let _ = std::fs::rename(tmp, path);
            }
        }
    }
}

pub fn clamp_font_size(v: f64) -> f64 {
    if !v.is_finite() {
        return DEFAULT_FONT_SIZE;
    }
    v.round().clamp(8.0, 32.0)
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
    fn rows_set_and_remove_keep_order_and_share_untouched_chunks() {
        let mut rows = Rows::default();
        rows.push_chunk((0..3).map(|i| vec![Value::from(i)]).collect());
        rows.push_chunk((3..6).map(|i| vec![Value::from(i)]).collect());
        let snapshot = rows.clone();
        assert!(rows.set(4, vec![Value::from(40)]));
        assert!(!rows.set(6, vec![]));
        assert_eq!(snapshot.get(4).unwrap()[0], Value::from(4), "a snapshot is not changed");
        assert!(Arc::ptr_eq(&rows.chunks[0], &snapshot.chunks[0]), "the untouched chunk is shared");
        rows.remove(&[0, 1, 2, 5].into());
        let left: Vec<Value> = rows.iter().map(|r| r[0].clone()).collect();
        assert_eq!(left, vec![Value::from(3), Value::from(40)]);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows.get(1).unwrap()[0], Value::from(40));
        rows.push_chunk(vec![vec![Value::from(7)]]);
        assert_eq!(rows.get(2).unwrap()[0], Value::from(7));
    }

    #[test]
    fn bytes_format() {
        assert_eq!(format_bytes(Some(512)), "512 B");
        assert_eq!(format_bytes(Some(1536)), "1.5 KB");
        assert_eq!(format_bytes(Some(245760)), "240 KB");
        assert_eq!(format_bytes(None), "");
    }
}
