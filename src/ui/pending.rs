//! Changes queued in a result grid before they are saved: cell edits, rows
//! marked for deletion and new rows, plus the order the grid shows rows in
//! once new rows are placed among them.
//!
//! Rows are identified by [`RowRef`]: an index into the result's data (never
//! a position on screen, which moves when the grid is sorted), or the id of a
//! new row.

use crate::grid_edit::{Cell, RowRef};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::sync::Arc;

/// A row added in the grid and not yet saved.
#[derive(Clone, Debug, PartialEq)]
pub struct NewRow {
    pub id: u64,
    /// The row it sits below; `None` for the end of the grid.
    pub anchor: Option<RowRef>,
    /// One per result column; `Default` until something is entered.
    pub cells: Vec<Cell>,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Pending {
    /// data row → column → new value (`Value` or `Null`).
    pub edits: BTreeMap<usize, BTreeMap<usize, Cell>>,
    /// Data rows marked for deletion.
    pub deletes: BTreeSet<usize>,
    /// New rows, in the order they were added.
    pub new_rows: Vec<NewRow>,
    /// Rows inserted by an earlier save, kept where they were added rather
    /// than sorted in with the rest: (data row, the row it sits below).
    pub pins: Vec<(usize, Option<RowRef>)>,
    next_id: u64,
    /// Bumped on every change, for caches of the row order.
    pub rev: u64,
}

/// The text a value is edited as. Unlike the grid's display text, numbers
/// keep every digit.
pub fn edit_text(value: &Value) -> String {
    crate::grid_edit::exact_text(value).unwrap_or_default()
}

impl Pending {
    fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// Nothing queued; rows saved earlier placed as in `pins`.
    pub fn with_pins(pins: Vec<(usize, Option<RowRef>)>) -> Pending {
        Pending { pins, ..Pending::default() }
    }

    pub fn is_empty(&self) -> bool {
        self.edits.is_empty() && self.deletes.is_empty() && self.new_rows.is_empty()
    }

    /// How many changes a save would make: edited cells of rows that are not
    /// being deleted, new rows and deleted rows.
    pub fn count(&self) -> usize {
        let edited: usize = self.edits.iter().filter(|(r, _)| !self.deletes.contains(r)).map(|(_, c)| c.len()).sum();
        edited + self.new_rows.len() + self.deletes.len()
    }

    pub fn new_row(&self, id: u64) -> Option<&NewRow> {
        self.new_rows.iter().find(|r| r.id == id)
    }

    pub fn is_deleted(&self, row: RowRef) -> bool {
        matches!(row, RowRef::Data(i) if self.deletes.contains(&i))
    }

    /// A cell's queued value: an edit of a loaded row, or any cell of a new row.
    pub fn cell(&self, row: RowRef, col: usize) -> Option<&Cell> {
        match row {
            RowRef::Data(i) => self.edits.get(&i).and_then(|c| c.get(&col)),
            RowRef::New(id) => self.new_row(id).and_then(|r| r.cells.get(col)),
        }
    }

    /// Whether a cell differs from what was loaded.
    pub fn is_dirty(&self, row: RowRef, col: usize) -> bool {
        match row {
            RowRef::Data(i) => self.edits.get(&i).is_some_and(|c| c.contains_key(&col)),
            RowRef::New(_) => true,
        }
    }

    /// Give a cell a value. `original` is the loaded value (for loaded rows):
    /// setting a cell back to it removes the edit.
    pub fn set_cell(&mut self, row: RowRef, col: usize, cell: Cell, original: Option<&Value>) {
        match row {
            RowRef::Data(i) => {
                let same = match (&cell, original) {
                    (Cell::Null, Some(Value::Null)) => true,
                    (Cell::Value(t), Some(v)) if !v.is_null() => *t == edit_text(v),
                    _ => false,
                };
                if same {
                    self.revert_cell(row, col);
                    return;
                }
                if matches!(cell, Cell::Default) {
                    return;
                }
                self.edits.entry(i).or_default().insert(col, cell);
            }
            RowRef::New(id) => {
                let Some(r) = self.new_rows.iter_mut().find(|r| r.id == id) else { return };
                if let Some(c) = r.cells.get_mut(col) {
                    *c = cell;
                }
            }
        }
        self.touch();
    }

    /// Put a cell back as it was loaded (or to DEFAULT in a new row).
    pub fn revert_cell(&mut self, row: RowRef, col: usize) {
        match row {
            RowRef::Data(i) => {
                if let Some(cols) = self.edits.get_mut(&i) {
                    cols.remove(&col);
                    if cols.is_empty() {
                        self.edits.remove(&i);
                    }
                }
            }
            RowRef::New(id) => {
                if let Some(c) = self.new_rows.iter_mut().find(|r| r.id == id).and_then(|r| r.cells.get_mut(col)) {
                    *c = Cell::Default;
                }
            }
        }
        self.touch();
    }

    /// Add a new row below `anchor` (or at the end); returns it.
    pub fn insert_below(&mut self, anchor: Option<RowRef>, columns: usize) -> RowRef {
        self.next_id += 1;
        let id = self.next_id;
        self.new_rows.push(NewRow { id, anchor, cells: vec![Cell::Default; columns] });
        self.touch();
        RowRef::New(id)
    }

    /// Mark rows for deletion. New rows are simply removed; rows that were
    /// placed below them move up to take their place.
    pub fn delete(&mut self, rows: &[RowRef]) {
        for &row in rows {
            match row {
                RowRef::Data(i) => {
                    self.deletes.insert(i);
                }
                RowRef::New(id) => {
                    let Some(pos) = self.new_rows.iter().position(|r| r.id == id) else { continue };
                    let removed = self.new_rows.remove(pos);
                    for r in &mut self.new_rows {
                        if r.anchor == Some(row) {
                            r.anchor = removed.anchor;
                        }
                    }
                }
            }
        }
        self.touch();
    }

    /// Take the deletion mark off rows; their edits come back with them.
    pub fn undelete(&mut self, rows: &[RowRef]) {
        for row in rows {
            if let RowRef::Data(i) = row {
                self.deletes.remove(i);
            }
        }
        self.touch();
    }

    /// Discard every queued change (rows already saved keep their place).
    pub fn clear(&mut self) {
        self.edits.clear();
        self.deletes.clear();
        self.new_rows.clear();
        self.touch();
    }

    /// Forget everything, including where saved rows were placed (a new result).
    pub fn reset(&mut self) {
        let rev = self.rev;
        *self = Pending::default();
        self.rev = rev.wrapping_add(1);
    }
}

// ─── Row order ──────────────────────────────────────────────────────────────

/// The rows of a grid in the order shown.
#[derive(Clone, Debug, PartialEq)]
pub enum ViewRows {
    /// Data rows in their own order.
    Plain(usize),
    /// Data rows in sorted order.
    Sorted(Arc<Vec<usize>>),
    /// With new or pinned rows placed among them.
    Custom(Arc<Vec<RowRef>>),
}

impl Default for ViewRows {
    fn default() -> Self {
        ViewRows::Plain(0)
    }
}

impl ViewRows {
    pub fn len(&self) -> usize {
        match self {
            ViewRows::Plain(n) => *n,
            ViewRows::Sorted(v) => v.len(),
            ViewRows::Custom(v) => v.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, display: usize) -> Option<RowRef> {
        match self {
            ViewRows::Plain(n) => (display < *n).then_some(RowRef::Data(display)),
            ViewRows::Sorted(v) => v.get(display).map(|&i| RowRef::Data(i)),
            ViewRows::Custom(v) => v.get(display).copied(),
        }
    }

    /// Where a row is shown. Linear in the number of rows.
    pub fn position(&self, row: RowRef) -> Option<usize> {
        match (self, row) {
            (ViewRows::Plain(n), RowRef::Data(i)) => (i < *n).then_some(i),
            (ViewRows::Sorted(v), RowRef::Data(i)) => v.iter().position(|&x| x == i),
            (ViewRows::Custom(v), r) => v.iter().position(|&x| x == r),
            _ => None,
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = RowRef> + '_ {
        (0..self.len()).filter_map(move |i| self.get(i))
    }
}

/// The order to show `data_len` data rows in, given their sorted order (if
/// sorted) and the new and pinned rows in `pending`. A row placed below
/// another follows it wherever the sort puts it; several placed below the
/// same row show the most recent first. A row whose anchor no longer exists
/// goes at the end.
pub fn build_view(data_len: usize, sorted: Option<Arc<Vec<usize>>>, pending: &Pending) -> ViewRows {
    if pending.new_rows.is_empty() && pending.pins.is_empty() {
        return match sorted {
            Some(s) if s.len() == data_len => ViewRows::Sorted(s),
            _ => ViewRows::Plain(data_len),
        };
    }
    let pinned_data: HashSet<usize> = pending.pins.iter().map(|(i, _)| *i).filter(|&i| i < data_len).collect();
    let mut placed: Vec<(RowRef, Option<RowRef>)> = Vec::new();
    for (i, anchor) in &pending.pins {
        if *i < data_len {
            placed.push((RowRef::Data(*i), *anchor));
        }
    }
    for r in &pending.new_rows {
        placed.push((RowRef::New(r.id), r.anchor));
    }
    let exists = |r: &RowRef| match r {
        RowRef::Data(i) => *i < data_len,
        RowRef::New(id) => pending.new_rows.iter().any(|n| n.id == *id),
    };
    let mut children: HashMap<RowRef, Vec<RowRef>> = HashMap::new();
    let mut orphans: Vec<RowRef> = Vec::new();
    for (row, anchor) in &placed {
        match anchor {
            Some(a) if exists(a) && a != row => children.entry(*a).or_default().push(*row),
            _ => orphans.push(*row),
        }
    }
    let mut out: Vec<RowRef> = Vec::with_capacity(data_len + pending.new_rows.len());
    let mut emitted: HashSet<RowRef> = HashSet::new();
    let mut emit = |start: RowRef, out: &mut Vec<RowRef>| {
        // Depth first; the most recently placed child comes directly below.
        let mut stack = vec![start];
        while let Some(r) = stack.pop() {
            if !emitted.insert(r) {
                continue;
            }
            out.push(r);
            if let Some(kids) = children.get(&r) {
                // Pushed oldest first so the newest is popped first.
                for k in kids {
                    stack.push(*k);
                }
            }
        }
    };
    let base: Box<dyn Iterator<Item = usize>> = match &sorted {
        Some(s) if s.len() == data_len => Box::new(s.iter().copied()),
        _ => Box::new(0..data_len),
    };
    for i in base {
        if pinned_data.contains(&i) {
            continue;
        }
        emit(RowRef::Data(i), &mut out);
    }
    for r in orphans {
        emit(r, &mut out);
    }
    // Anything caught in an anchor cycle still has to be shown.
    for (row, _) in &placed {
        emit(*row, &mut out);
    }
    ViewRows::Custom(Arc::new(out))
}

/// After a save: data rows `deleted` are removed from the result, and
/// `inserted` new rows become data rows (in the order given, appended after
/// the existing `data_len` rows). Returns the new pins: every pinned or
/// inserted row anchored to whatever is now shown directly above it, so
/// nothing moves on screen.
pub fn pins_after_save(view: &ViewRows, pending: &Pending, deleted: &BTreeSet<usize>, inserted: &[u64], data_len: usize) -> Vec<(usize, Option<RowRef>)> {
    // Old data index → new data index, with the deleted rows gone.
    let new_index = |i: usize| -> Option<usize> {
        if deleted.contains(&i) {
            return None;
        }
        Some(i - deleted.range(..i).count())
    };
    let kept = data_len - deleted.iter().filter(|&&i| i < data_len).count();
    let inserted_index: HashMap<u64, usize> = inserted.iter().enumerate().map(|(n, id)| (*id, kept + n)).collect();
    let map = |r: RowRef| -> Option<RowRef> {
        match r {
            RowRef::Data(i) => new_index(i).map(RowRef::Data),
            RowRef::New(id) => inserted_index.get(&id).map(|&i| RowRef::Data(i)),
        }
    };
    let pinned: HashSet<RowRef> = pending
        .pins
        .iter()
        .map(|(i, _)| RowRef::Data(*i))
        .chain(inserted.iter().map(|id| RowRef::New(*id)))
        .collect();
    let mut out = Vec::new();
    let mut above: Option<RowRef> = None;
    for row in view.iter() {
        let Some(now) = map(row) else { continue };
        if pinned.contains(&row) {
            if let RowRef::Data(i) = now {
                out.push((i, above));
            }
        }
        above = Some(now);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn v(s: &str) -> Cell {
        Cell::Value(s.into())
    }

    #[test]
    fn p01_editing_back_to_the_original_is_not_a_change() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(0), 1, v("Bob"), Some(&json!("Ann")));
        assert!(p.is_dirty(RowRef::Data(0), 1));
        assert_eq!(p.count(), 1);
        p.set_cell(RowRef::Data(0), 1, v("Ann"), Some(&json!("Ann")));
        assert!(!p.is_dirty(RowRef::Data(0), 1));
        assert!(p.edits.is_empty(), "a row with no edits left drops out");
        // Numbers compare by their exact text.
        p.set_cell(RowRef::Data(0), 2, v("10"), Some(&json!(10)));
        assert!(p.is_empty());
        p.set_cell(RowRef::Data(0), 2, v("10.0"), Some(&json!(10)));
        assert!(!p.is_empty());
    }

    #[test]
    fn p02_p04_p05_empty_text_null_and_back() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(0), 0, v(""), Some(&json!("x")));
        assert_eq!(p.cell(RowRef::Data(0), 0), Some(&v("")), "empty is an empty string, not NULL");
        p.set_cell(RowRef::Data(1), 0, Cell::Null, Some(&Value::Null));
        assert!(p.cell(RowRef::Data(1), 0).is_none(), "NULL on a NULL cell is no change");
        p.set_cell(RowRef::Data(2), 0, Cell::Null, Some(&json!("x")));
        assert_eq!(p.cell(RowRef::Data(2), 0), Some(&Cell::Null));
        p.set_cell(RowRef::Data(2), 0, v("y"), Some(&json!("x")));
        assert_eq!(p.cell(RowRef::Data(2), 0), Some(&v("y")));
        p.revert_cell(RowRef::Data(2), 0);
        assert!(p.cell(RowRef::Data(2), 0).is_none());
        // An empty string on a NULL cell is a change.
        p.set_cell(RowRef::Data(3), 0, v(""), Some(&Value::Null));
        assert_eq!(p.cell(RowRef::Data(3), 0), Some(&v("")));
    }

    #[test]
    fn p07_p09_p10_new_rows_sit_below_their_anchor_newest_first() {
        let mut p = Pending::default();
        let a = p.insert_below(Some(RowRef::Data(4)), 2);
        let b = p.insert_below(Some(RowRef::Data(4)), 2);
        let view = build_view(8, None, &p);
        let shown: Vec<RowRef> = view.iter().collect();
        assert_eq!(&shown[4..8], &[RowRef::Data(4), b, a, RowRef::Data(5)]);
        assert_eq!(view.len(), 10);
        assert_eq!(p.new_row(match a { RowRef::New(id) => id, _ => 0 }).unwrap().cells, vec![Cell::Default, Cell::Default]);
        // Into an empty result.
        let mut p = Pending::default();
        let c = p.insert_below(None, 3);
        assert_eq!(build_view(0, None, &p).iter().collect::<Vec<_>>(), vec![c]);
    }

    #[test]
    fn p08_new_rows_follow_their_anchor_through_a_sort() {
        let mut p = Pending::default();
        let n = p.insert_below(Some(RowRef::Data(0)), 1);
        let sorted = Arc::new(vec![2, 1, 0]);
        let shown: Vec<RowRef> = build_view(3, Some(sorted), &p).iter().collect();
        assert_eq!(shown, vec![RowRef::Data(2), RowRef::Data(1), RowRef::Data(0), n]);
        // A row inserted below a new row goes below that one.
        let m = p.insert_below(Some(n), 1);
        let shown: Vec<RowRef> = build_view(3, None, &p).iter().collect();
        assert_eq!(shown, vec![RowRef::Data(0), n, m, RowRef::Data(1), RowRef::Data(2)]);
    }

    #[test]
    fn p11_deleting_a_new_row_removes_it_and_closes_the_gap() {
        let mut p = Pending::default();
        let n = p.insert_below(Some(RowRef::Data(0)), 1);
        let m = p.insert_below(Some(n), 1);
        p.delete(&[n]);
        assert!(p.new_rows.iter().all(|r| RowRef::New(r.id) != n));
        assert!(p.deletes.is_empty(), "nothing to delete in the database");
        let shown: Vec<RowRef> = build_view(2, None, &p).iter().collect();
        assert_eq!(shown, vec![RowRef::Data(0), m, RowRef::Data(1)]);
    }

    #[test]
    fn p12_p13_delete_marks_keep_edits_for_an_undelete() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(1), 0, v("x"), Some(&json!("a")));
        p.delete(&[RowRef::Data(1)]);
        assert!(p.is_deleted(RowRef::Data(1)));
        assert_eq!(p.count(), 1, "a deleted row's edits are not counted");
        p.undelete(&[RowRef::Data(1)]);
        assert_eq!(p.cell(RowRef::Data(1), 0), Some(&v("x")));
        assert_eq!(p.count(), 1);
    }

    #[test]
    fn p19_clear_discards_everything_queued() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(1), 0, v("x"), Some(&json!("a")));
        p.delete(&[RowRef::Data(2)]);
        p.insert_below(None, 1);
        p.pins.push((5, None));
        assert_eq!(p.count(), 3);
        p.clear();
        assert!(p.is_empty());
        assert_eq!(p.pins, vec![(5, None)], "rows saved earlier stay where they are");
    }

    #[test]
    fn now_is_a_change_that_can_be_reverted() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(0), 2, Cell::Now, Some(&json!("2024-01-01")));
        assert_eq!(p.cell(RowRef::Data(0), 2), Some(&Cell::Now));
        assert_eq!(p.count(), 1);
        // Even on a NULL cell, NOW() is a change.
        p.set_cell(RowRef::Data(1), 2, Cell::Now, Some(&Value::Null));
        assert_eq!(p.count(), 2);
        p.revert_cell(RowRef::Data(0), 2);
        assert_eq!(p.count(), 1);
        let n = p.insert_below(None, 3);
        p.set_cell(n, 2, Cell::Now, None);
        assert_eq!(p.cell(n, 2), Some(&Cell::Now));
    }

    #[test]
    fn p20_reverting_a_new_rows_cell_means_default() {
        let mut p = Pending::default();
        let n = p.insert_below(None, 2);
        p.set_cell(n, 1, v("x"), None);
        p.set_cell(n, 0, Cell::Null, None);
        assert_eq!(p.cell(n, 1), Some(&v("x")));
        p.revert_cell(n, 1);
        assert_eq!(p.cell(n, 1), Some(&Cell::Default));
        assert_eq!(p.cell(n, 0), Some(&Cell::Null));
    }

    #[test]
    fn p21_edits_belong_to_rows_not_screen_positions() {
        let mut p = Pending::default();
        p.set_cell(RowRef::Data(2), 0, v("x"), Some(&json!("a")));
        let sorted = build_view(3, Some(Arc::new(vec![2, 0, 1])), &p);
        assert_eq!(sorted.get(0), Some(RowRef::Data(2)));
        assert!(p.is_dirty(sorted.get(0).unwrap(), 0));
        assert!(!p.is_dirty(sorted.get(2).unwrap(), 0));
    }

    #[test]
    fn pins_keep_saved_rows_in_place() {
        // Rows 0..4; delete row 1; new row placed below row 2.
        let mut p = Pending::default();
        let n = p.insert_below(Some(RowRef::Data(2)), 1);
        p.delete(&[RowRef::Data(1)]);
        let view = build_view(4, None, &p);
        let deleted: BTreeSet<usize> = [1].into();
        let id = match n { RowRef::New(id) => id, _ => unreachable!() };
        let pins = pins_after_save(&view, &p, &deleted, &[id], 4);
        // Rows left: 0, 2→1, 3→2, and the new row at data index 3, below old row 2.
        assert_eq!(pins, vec![(3, Some(RowRef::Data(1)))]);
        let after = Pending { pins, ..Default::default() };
        let shown: Vec<RowRef> = build_view(4, None, &after).iter().collect();
        assert_eq!(shown, vec![RowRef::Data(0), RowRef::Data(1), RowRef::Data(3), RowRef::Data(2)]);
    }

    #[test]
    fn a_pinned_rows_anchor_being_deleted_keeps_it_in_place() {
        let mut p = Pending { pins: vec![(3, Some(RowRef::Data(1)))], ..Default::default() };
        // Shown: 0, 1, 3(pinned), 2. Delete row 1.
        p.delete(&[RowRef::Data(1)]);
        let view = build_view(4, None, &p);
        let deleted: BTreeSet<usize> = [1].into();
        let pins = pins_after_save(&view, &p, &deleted, &[], 4);
        assert_eq!(pins, vec![(2, Some(RowRef::Data(0)))]);
        let shown: Vec<RowRef> = build_view(3, None, &Pending { pins, ..Default::default() }).iter().collect();
        assert_eq!(shown, vec![RowRef::Data(0), RowRef::Data(2), RowRef::Data(1)]);
    }

    #[test]
    fn view_position_and_lookup_agree() {
        let mut p = Pending::default();
        p.insert_below(Some(RowRef::Data(1)), 1);
        p.insert_below(None, 1);
        let view = build_view(5, Some(Arc::new(vec![4, 3, 2, 1, 0])), &p);
        for i in 0..view.len() {
            assert_eq!(view.position(view.get(i).unwrap()), Some(i));
        }
        assert_eq!(view.len(), 7);
        assert!(view.get(7).is_none());
    }

    #[test]
    fn edit_text_keeps_big_numbers_exact() {
        let big: Value = serde_json::from_str("9007199254740993").unwrap();
        assert_eq!(edit_text(&big), "9007199254740993");
        assert_eq!(edit_text(&Value::Null), "");
        assert_eq!(edit_text(&json!(true)), "true");
    }
}
