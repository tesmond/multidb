//! The result of a query tab, as an entity of its own.
//!
//! A streaming query appends rows to this many times a second. Keeping the
//! rows here, rather than in the workspace, means an arriving chunk only
//! notifies the views that show a result (the results grid and the status bar)
//! instead of everything that observes the workspace.

use crate::ui::model::{EditInfo, PendingEdits, QueryResult, SortDirection};
use serde_json::Value;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

/// A fresh id for each set of columns: anything keyed on "which result is
/// this" (grid scroll, selection, column widths, the sort cache) compares these.
pub fn next_generation() -> u64 {
    static GEN: AtomicU64 = AtomicU64::new(1);
    GEN.fetch_add(1, Ordering::Relaxed)
}

pub struct ResultStore {
    pub result: Option<QueryResult>,
    pub sort_col: Option<usize>,
    pub sort_dir: SortDirection,
    /// Set when the statement is a simple SELECT on a table with a primary
    /// key, which makes its cells editable.
    pub edit_info: Option<EditInfo>,
    pub pending_edits: PendingEdits,
    /// Rows are still arriving.
    pub loading: bool,
}

impl Default for ResultStore {
    fn default() -> Self {
        ResultStore {
            result: None,
            sort_col: None,
            sort_dir: SortDirection::Asc,
            edit_info: None,
            pending_edits: Vec::new(),
            loading: false,
        }
    }
}

/// What a finished statement reports.
pub struct Finished {
    pub rows_affected: i64,
    pub duration: i64,
    pub error: String,
}

impl ResultStore {
    pub fn sort(&self) -> Option<(usize, SortDirection)> {
        self.sort_col.map(|c| (c, self.sort_dir))
    }

    pub fn row_count(&self) -> usize {
        self.result.as_ref().map_or(0, |r| r.rows.len())
    }

    fn reset_view_state(&mut self) {
        self.sort_col = None;
        self.sort_dir = SortDirection::Asc;
        self.pending_edits.clear();
    }

    /// Forget the result (a statement is about to run).
    pub fn clear(&mut self) {
        self.result = None;
        self.edit_info = None;
        self.reset_view_state();
        self.loading = false;
    }

    /// The first rows are on their way: a new, empty result with these columns.
    pub fn begin(&mut self, columns: Vec<String>, column_types: Vec<String>) -> u64 {
        let generation = next_generation();
        self.result = Some(QueryResult {
            columns: Arc::new(columns),
            column_types: Arc::new(column_types),
            generation,
            ..Default::default()
        });
        self.reset_view_state();
        self.loading = true;
        generation
    }

    /// Add a batch of rows; returns how many there are now.
    pub fn append(&mut self, rows: Vec<Vec<Value>>) -> usize {
        match &mut self.result {
            Some(r) => {
                r.rows.push_chunk(rows);
                r.rows.len()
            }
            None => 0,
        }
    }

    /// The statement is done. A statement that returned no rows never began a
    /// result, so one is made for its summary.
    pub fn finish(&mut self, done: Finished) {
        let r = self.result.get_or_insert_with(|| QueryResult { generation: next_generation(), ..Default::default() });
        r.rows_affected = done.rows_affected;
        r.duration = done.duration;
        r.error = done.error;
        self.loading = false;
    }

    /// The statement failed before any row arrived.
    pub fn fail(&mut self, error: String) {
        self.result = Some(QueryResult { error, generation: next_generation(), ..Default::default() });
        self.reset_view_state();
        self.loading = false;
    }

    /// A copy for a duplicated tab.
    pub fn duplicate(&self) -> ResultStore {
        ResultStore {
            result: self.result.clone(),
            sort_col: self.sort_col,
            sort_dir: self.sort_dir,
            edit_info: self.edit_info.clone(),
            pending_edits: self.pending_edits.clone(),
            loading: false,
        }
    }
}
