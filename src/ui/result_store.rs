//! The result of a query tab, as an entity of its own.
//!
//! A streaming query appends rows to this many times a second. Keeping the
//! rows here, rather than in the workspace, means an arriving chunk only
//! notifies the views that show a result (the results grid and the status bar)
//! instead of everything that observes the workspace.

use crate::commands::QueryPhase;
use crate::grid_edit::exec::{SaveControl, SaveReport};
use crate::grid_edit::RowRef;
use crate::ui::model::{EditInfo, QueryResult, ResultSource, SortDirection};
use crate::ui::pending::{self, Pending, ViewRows};
use serde_json::Value;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// A fresh id for each set of columns: anything keyed on "which result is
/// this" (grid scroll, selection, column widths, the sort cache) compares these.
pub fn next_generation() -> u64 {
    static GEN: AtomicU64 = AtomicU64::new(1);
    GEN.fetch_add(1, Ordering::Relaxed)
}

#[derive(Clone)]
pub struct QueryProgress {
    pub phase: QueryPhase,
    started: Instant,
    elapsed: Option<Duration>,
}

impl QueryProgress {
    pub fn elapsed(&self) -> Duration {
        self.elapsed.unwrap_or_else(|| self.started.elapsed())
    }

    pub fn set_phase(&mut self, phase: QueryPhase) {
        if !phase.running() && self.elapsed.is_none() {
            self.elapsed = Some(self.started.elapsed());
        }
        self.phase = phase;
    }
}

pub fn format_elapsed(elapsed: Duration) -> String {
    if elapsed.as_secs() < 10 {
        // Truncate to tenths so 9.99 seconds still displays as 9.9s.
        format!("{:.1}s", (elapsed.as_millis() / 100) as f64 / 10.0)
    } else {
        format!("{}s", elapsed.as_secs())
    }
}

pub struct ResultStore {
    pub result: Option<QueryResult>,
    pub progress: Option<QueryProgress>,
    pub sort_col: Option<usize>,
    pub sort_dir: SortDirection,
    /// Set when the statement is a plain read of one table with a primary
    /// key, which makes its cells editable.
    pub edit_info: Option<EditInfo>,
    /// Why the result can't be edited, when that is known.
    pub edit_blocked: Option<String>,
    /// The result generation `edit_info` / `edit_blocked` were worked out for.
    pub edit_checked: u64,
    /// Where the result came from (set when the query runs).
    pub source: Option<ResultSource>,
    /// Edits, deletions and new rows waiting to be saved.
    pub pending: Pending,
    /// A save in progress.
    pub saving: Option<Arc<SaveControl>>,
    /// The last save failed: what went wrong, and the row it was on.
    pub save_error: Option<(String, Option<RowRef>)>,
    /// Rows are still arriving.
    pub loading: bool,
}

impl Default for ResultStore {
    fn default() -> Self {
        ResultStore {
            result: None,
            progress: None,
            sort_col: None,
            sort_dir: SortDirection::Asc,
            edit_info: None,
            edit_blocked: None,
            edit_checked: 0,
            source: None,
            pending: Pending::default(),
            saving: None,
            save_error: None,
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
        self.pending.reset();
        self.save_error = None;
    }

    /// Forget the result (a statement is about to run).
    pub fn clear(&mut self) {
        self.result = None;
        self.progress = None;
        self.edit_info = None;
        self.edit_blocked = None;
        self.edit_checked = 0;
        self.reset_view_state();
        self.loading = false;
    }

    /// There are changes waiting to be saved.
    pub fn has_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn is_saving(&self) -> bool {
        self.saving.is_some()
    }

    /// The rows in the order the grid shows them, given the sorted order of
    /// the data rows (if sorted).
    pub fn view(&self, sorted: Option<Arc<Vec<usize>>>) -> ViewRows {
        let n = self.row_count();
        pending::build_view(n, sorted, &self.pending)
    }

    /// Fold a successful save into the result without running the query
    /// again: updated rows take their stored values, deleted rows go, new
    /// rows become ordinary rows where they were placed. `view` is the order
    /// the grid showed when the save started.
    pub fn apply_saved(&mut self, report: &SaveReport, view: &ViewRows) {
        let Some(result) = &mut self.result else { return };
        for (row, values) in &report.updated {
            if let RowRef::Data(i) = row {
                result.rows.set(*i, values.clone());
            }
        }
        let deleted: std::collections::BTreeSet<usize> =
            report.deleted.iter().filter_map(|r| if let RowRef::Data(i) = r { Some(*i) } else { None }).collect();
        let inserted_ids: Vec<u64> = report.inserted.iter().filter_map(|(r, _)| if let RowRef::New(id) = r { Some(*id) } else { None }).collect();
        let data_len = result.rows.len();
        let pins = pending::pins_after_save(view, &self.pending, &deleted, &inserted_ids, data_len);
        result.rows.remove(&deleted);
        let new_rows: Vec<Vec<serde_json::Value>> = report.inserted.iter().map(|(_, v)| v.clone()).collect();
        result.rows.push_chunk(new_rows);
        result.rev = result.rev.wrapping_add(1);
        self.pending.clear();
        self.pending.pins = pins;
        self.save_error = None;
    }

    pub fn start(&mut self) {
        self.clear();
        self.progress = Some(QueryProgress { phase: QueryPhase::Received, started: Instant::now(), elapsed: None });
    }

    pub fn set_phase(&mut self, phase: QueryPhase) {
        if let Some(progress) = &mut self.progress {
            progress.set_phase(phase);
        }
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
        self.set_phase(QueryPhase::Streaming);
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
        let phase = if done.error.is_empty() {
            QueryPhase::Completed
        } else if done.error == "query cancelled" {
            QueryPhase::Cancelled
        } else {
            QueryPhase::Failed
        };
        r.error = done.error;
        self.set_phase(phase);
        self.loading = false;
    }

    /// The statement failed before any row arrived.
    pub fn fail(&mut self, error: String) {
        self.set_phase(QueryPhase::Failed);
        self.result = Some(QueryResult { error, generation: next_generation(), ..Default::default() });
        self.reset_view_state();
        self.loading = false;
    }

    /// A copy for a duplicated tab.
    pub fn duplicate(&self) -> ResultStore {
        ResultStore {
            result: self.result.clone(),
            progress: self.progress.clone().map(|mut p| {
                if p.phase.running() { p.set_phase(QueryPhase::Cancelled); }
                p
            }),
            sort_col: self.sort_col,
            sort_dir: self.sort_dir,
            edit_info: self.edit_info.clone(),
            edit_blocked: self.edit_blocked.clone(),
            edit_checked: self.edit_checked,
            source: self.source.clone(),
            // Unsaved changes stay with the tab they were made in; a copy
            // that could save them a second time would only conflict.
            pending: Pending::with_pins(self.pending.pins.clone()),
            saving: None,
            save_error: None,
            loading: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn elapsed_switches_from_tenths_to_whole_seconds() {
        for (millis, expected) in [(0, "0.0s"), (100, "0.1s"), (1250, "1.2s"), (9999, "9.9s"), (10000, "10s"), (10999, "10s"), (11000, "11s")] {
            assert_eq!(format_elapsed(Duration::from_millis(millis)), expected);
        }
    }

    #[test]
    fn progress_tracks_execution_and_freezes_when_finished() {
        let mut store = ResultStore::default();
        store.start();
        store.set_phase(QueryPhase::Waiting);
        assert!(store.progress.as_ref().unwrap().phase.running());
        store.begin(vec!["value".into()], vec!["integer".into()]);
        assert_eq!(store.progress.as_ref().unwrap().phase, QueryPhase::Streaming);
        store.finish(Finished { rows_affected: 0, duration: 12, error: String::new() });
        let progress = store.progress.as_ref().unwrap();
        assert_eq!(progress.phase, QueryPhase::Completed);
        assert_eq!(progress.elapsed(), progress.elapsed.unwrap());
        store.start();
        assert_eq!(store.progress.as_ref().unwrap().phase, QueryPhase::Received);
        assert!(store.result.is_none());
        store.fail("connection failed".into());
        assert_eq!(store.progress.as_ref().unwrap().phase, QueryPhase::Failed);
        assert!(store.progress.as_ref().unwrap().elapsed.is_some());
    }
}
