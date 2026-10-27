//! Editing results in place, workspace side: deciding whether a result can be
//! edited, saving queued changes in the background, the conflict and
//! "discard changes?" dialogs.

use crate::grid_edit::exec::{Conflict, SaveControl, SaveOutcome, SaveReport};
use crate::grid_edit::plan::{RowDelete, RowInsert, RowUpdate, SaveRequest, Target};
use crate::grid_edit::{query, Cell, Driver, RowRef};
use crate::ui::dialogs::{self, Btn};
use crate::ui::model::{EditInfo, OutputTab, ResultSource, TabId};
use crate::ui::pending::ViewRows;
use crate::ui::widgets::overlay;
use crate::ui::result_store::ResultStore;
use crate::ui::runtime;
use crate::ui::theme;
use crate::ui::widgets::{Scale, TextExt};
use crate::ui::workspace::{TabKind, Workspace};
use gpui::{div, prelude::*, px, AnyElement, Context, Entity, FontWeight, MouseButton, SharedString, Window};
use serde_json::Value;
use std::sync::Arc;
use std::time::Duration;

/// Seconds a save waits for a row lock before giving up.
pub const LOCK_TIMEOUT_SECS: u32 = 10;
/// Conflicting rows listed by name in the conflict dialog.
const CONFLICTS_SHOWN: usize = 50;

/// Something that would throw away queued changes, waiting on "are you sure?".
#[derive(Clone, Debug, PartialEq)]
pub enum DiscardAction {
    RunQuery(TabId),
    CloseTabs(Vec<TabId>),
    Disconnect(String),
    DeleteGroup(String),
    Clear(TabId),
    Quit,
}

/// Rows changed since they were loaded; the save is waiting on Override or Cancel.
#[derive(Clone)]
pub struct ConflictPrompt {
    pub tab_id: TabId,
    pub columns: Arc<Vec<String>>,
    /// The result columns of the primary key, to name each row by its key.
    pub key: Vec<usize>,
    pub conflicts: Vec<ConflictRow>,
}

#[derive(Clone)]
pub struct ConflictRow {
    pub conflict: Conflict,
    /// Where the row is shown (1-based), for people to find it.
    pub shown_at: Option<usize>,
    pub loaded: Vec<Value>,
    /// The values queued for it (empty for a row being deleted).
    pub yours: Vec<(usize, Cell)>,
}

/// Your side of one column of a conflicting row.
#[derive(Clone, Debug, PartialEq)]
pub enum Yours {
    /// You didn't edit this column.
    NotEdited,
    /// What you set it to.
    Value(String),
    /// You're deleting the row.
    Deleted,
}

/// One line of a conflicting row's table: a column with its value when
/// loaded, now in the database (`None` when they didn't change it) and yours.
#[derive(Clone, Debug, PartialEq)]
pub struct ConflictLine {
    pub col: usize,
    pub loaded: String,
    pub now: Option<String>,
    pub yours: Yours,
}

/// What kind of conflict a row is, for the badge in its header.
#[derive(Clone, Debug, PartialEq)]
pub enum ConflictKind {
    /// You both changed these columns.
    Both(Vec<usize>),
    /// They changed a row you're deleting.
    Deleting,
    /// They and you changed different columns.
    Separate,
}

/// The columns they changed, then any other columns you edited.
pub fn conflict_lines(row: &ConflictRow) -> Vec<ConflictLine> {
    let loaded = |c: usize| row.loaded.get(c).map(value_text).unwrap_or_default();
    let mut lines: Vec<ConflictLine> = row
        .conflict
        .changed
        .iter()
        .map(|&c| ConflictLine {
            col: c,
            loaded: loaded(c),
            now: Some(row.conflict.current.get(c).map(value_text).unwrap_or_default()),
            yours: if row.conflict.deleting {
                Yours::Deleted
            } else {
                row.yours.iter().find(|(col, _)| *col == c).map(|(_, v)| Yours::Value(cell_text(v))).unwrap_or(Yours::NotEdited)
            },
        })
        .collect();
    if !row.conflict.deleting {
        for (c, v) in &row.yours {
            if !row.conflict.changed.contains(c) {
                lines.push(ConflictLine { col: *c, loaded: loaded(*c), now: None, yours: Yours::Value(cell_text(v)) });
            }
        }
    }
    lines
}

pub fn conflict_kind(row: &ConflictRow) -> ConflictKind {
    if row.conflict.deleting {
        return ConflictKind::Deleting;
    }
    let both: Vec<usize> = row.conflict.changed.iter().copied().filter(|c| row.yours.iter().any(|(y, _)| y == c)).collect();
    if both.is_empty() { ConflictKind::Separate } else { ConflictKind::Both(both) }
}

/// The text of a value in the dialogs.
fn value_text(v: &Value) -> String {
    match v {
        Value::Null => "NULL".into(),
        v => crate::ui::js::value_to_string(v),
    }
}

fn cell_text(c: &Cell) -> String {
    match c {
        Cell::Value(t) => t.clone(),
        Cell::Null => "NULL".into(),
        Cell::Default => "DEFAULT".into(),
        Cell::Now => "NOW()".into(),
    }
}

fn short(text: String) -> String {
    if crate::ui::js::char_len(&text) > 60 {
        format!("{}…", crate::ui::js::slice_chars(&text, 60))
    } else {
        text
    }
}

impl Workspace {
    fn sql_store(&self, tab_id: &str) -> Option<Entity<ResultStore>> {
        self.tab(tab_id).and_then(|t| t.sql()).map(|s| s.store.clone())
    }

    /// Tabs with queued changes (including any being saved right now) on
    /// `conn_id`, or anywhere when `None`.
    pub fn tabs_with_changes(&self, conn_id: Option<&str>, cx: &gpui::App) -> Vec<TabId> {
        self.tabs
            .iter()
            .filter(|t| conn_id.is_none_or(|c| t.conn_id == c))
            .filter(|t| match &t.kind {
                TabKind::Sql(s) => s.store.read(cx).has_pending(),
                _ => false,
            })
            .map(|t| t.id.clone())
            .collect()
    }

    // ─── Can this result be edited? ─────────────────────────────────────────

    /// Work out whether the active tab's result can be edited, from the query
    /// that produced it (not whatever the editor holds now). Runs once per
    /// result.
    pub fn schedule_edit_check(&mut self, cx: &mut Context<Self>) {
        let Some(tab) = self.active_tab() else { return };
        let Some(s) = tab.sql() else { return };
        let store = s.store.clone();
        let st = store.read(cx);
        let Some(result) = st.result.clone() else { return };
        if !result.error.is_empty() || result.columns.is_empty() {
            return;
        }
        if st.edit_checked == result.generation {
            return;
        }
        let Some(source) = st.source.clone() else { return };
        let generation = result.generation;
        let set = move |store: &Entity<ResultStore>, info: Result<EditInfo, String>, cx: &mut gpui::App| {
            store.update(cx, |st, cx| {
                if st.result.as_ref().is_some_and(|r| r.generation == generation) {
                    st.edit_checked = generation;
                    match info {
                        Ok(info) => {
                            st.edit_info = Some(info);
                            st.edit_blocked = None;
                        }
                        Err(reason) => {
                            st.edit_info = None;
                            st.edit_blocked = Some(reason);
                        }
                    }
                    cx.notify();
                }
            });
        };
        let Some(driver) = Driver::from_name(&source.driver) else {
            set(&store, Err(format!("editing is not supported for {}", source.driver)), cx);
            return;
        };
        let parsed = match query::parse(&source.sql, driver) {
            Ok(p) => p,
            Err(reason) => {
                set(&store, Err(reason), cx);
                return;
            }
        };
        let schema = if !parsed.schema.is_empty() {
            parsed.schema.clone()
        } else if driver == Driver::Sqlite {
            String::new()
        } else {
            source.default_schema.clone()
        };
        let state = runtime::state();
        let (conn, db, table, drv) = (source.conn_id.clone(), source.database.clone(), parsed.table.clone(), source.driver.clone());
        let fut = runtime::spawn(async move { crate::commands::get_table_edit_meta(state, conn, drv, db, schema, table).await });
        let columns = result.columns.clone();
        self.edit_check = Some(cx.spawn(async move |_this, cx| {
            let meta = fut.await;
            let info = meta.and_then(|meta| {
                let layout = query::layout(driver, &parsed, &columns, &meta)?;
                Ok(EditInfo { driver, meta: Arc::new(meta), layout })
            });
            cx.update(|cx| set(&store, info, cx)).ok();
        }));
    }

    // ─── Saving ─────────────────────────────────────────────────────────────

    /// The Save button: check, then save the active tab's changes. `view`
    /// is the order the grid shows rows in; the button is inside the output
    /// panel, which is mid-update while it calls this, so it hands the order
    /// over rather than have the workspace ask the panel for it (a re-entrant
    /// update aborts the app).
    pub fn save_edits(&mut self, view: ViewRows, cx: &mut Context<Self>) {
        let tab_id = self.active_tab_id.clone();
        let Some(store) = self.sql_store(&tab_id) else { return };
        let st = store.read(cx);
        if st.loading || st.is_saving() || !st.has_pending() {
            return;
        }
        if let Some(info) = &st.edit_info {
            if !info.meta.transactional {
                self.nontx_confirm = Some(tab_id);
                cx.notify();
                return;
            }
        }
        self.start_save(&tab_id, true, Some(view), cx);
    }

    /// Build the save request from a tab's queued changes.
    fn save_request(&self, tab_id: &str, check_conflicts: bool, cx: &gpui::App) -> Result<(SaveRequest, ResultSource), String> {
        let store = self.sql_store(tab_id).ok_or("the tab is gone")?;
        let st = store.read(cx);
        let result = st.result.as_ref().ok_or("there is no result")?;
        let info = st.edit_info.as_ref().ok_or_else(|| st.edit_blocked.clone().unwrap_or_else(|| "this result can't be edited".into()))?;
        let source = st.source.clone().ok_or("the result's source is unknown")?;
        let p = &st.pending;
        let columns: Vec<Option<_>> = (0..result.columns.len()).map(|c| info.column(c).cloned()).collect();
        let row = |i: usize| result.rows.get(i).cloned().ok_or_else(|| format!("row {} is missing", i + 1));
        let mut updates = Vec::new();
        for (&i, cells) in &p.edits {
            if p.deletes.contains(&i) || cells.is_empty() {
                continue;
            }
            updates.push(RowUpdate { row: RowRef::Data(i), original: row(i)?, sets: cells.iter().map(|(c, v)| (*c, v.clone())).collect() });
        }
        let mut deletes = Vec::new();
        for &i in &p.deletes {
            deletes.push(RowDelete { row: RowRef::Data(i), original: row(i)? });
        }
        let inserts = p
            .new_rows
            .iter()
            .map(|r| RowInsert { row: RowRef::New(r.id), cells: r.cells.iter().cloned().enumerate().filter(|(_, c)| !matches!(c, Cell::Default)).collect() })
            .collect();
        let schema = if info.driver == Driver::Sqlite {
            query::parse(&source.sql, info.driver).map(|p| p.schema).unwrap_or_default()
        } else {
            info.meta.schema.clone()
        };
        let req = SaveRequest {
            target: Target { driver: info.driver, schema, table: info.meta.table.clone() },
            columns,
            key: info.layout.key.clone(),
            supports_returning: info.meta.supports_returning,
            updates,
            deletes,
            inserts,
            check_conflicts,
            lock_timeout_secs: LOCK_TIMEOUT_SECS,
        };
        Ok((req, source))
    }

    /// Save a tab's queued changes. `view` is the grid's row order, when the
    /// caller already has it; otherwise it is read from the output panel,
    /// which must not be mid-update (callers inside the panel pass it).
    pub fn start_save(&mut self, tab_id: &str, check_conflicts: bool, view: Option<ViewRows>, cx: &mut Context<Self>) {
        let Some(store) = self.sql_store(tab_id) else { return };
        let (req, source) = match self.save_request(tab_id, check_conflicts, cx) {
            Ok(r) => r,
            Err(e) => {
                self.show_error_popup("Can't save changes", e, cx);
                return;
            }
        };
        // The order rows are shown in now: new rows stay where they were placed.
        let view = match view {
            Some(v) => v,
            None => self.output.update(cx, |o, cx| o.current_view(cx)),
        };
        let generation = store.read(cx).result.as_ref().map(|r| r.generation);
        let ctl = SaveControl::new();
        store.update(cx, |st, cx| {
            st.saving = Some(ctl.clone());
            st.save_error = None;
            cx.notify();
        });
        self.set_tab_status(tab_id, "Saving changes…");
        cx.notify();
        let state = runtime::state();
        let fut = runtime::spawn(crate::commands::save_grid_changes(state, source.conn_id.clone(), source.database.clone(), req.clone(), ctl.clone()));
        let tab_id = tab_id.to_string();
        // Progress: redraw the counter a few times a second while it runs.
        let ticker_store = store.clone();
        let ticker_ctl = ctl.clone();
        cx.spawn(async move |_this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(150)).await;
            let running = ticker_store
                .update(cx, |st, cx| {
                    let same = st.saving.as_ref().is_some_and(|c| Arc::ptr_eq(c, &ticker_ctl));
                    if same {
                        cx.notify();
                    }
                    same
                })
                .unwrap_or(false);
            if !running {
                break;
            }
        })
        .detach();
        cx.spawn(async move |this, cx| {
            let outcome = fut.await;
            this.update(cx, |this, cx| this.finish_save(&tab_id, &store, generation, &req, &view, outcome, cx)).ok();
        })
        .detach();
    }

    #[allow(clippy::too_many_arguments)]
    fn finish_save(
        &mut self,
        tab_id: &str,
        store: &Entity<ResultStore>,
        generation: Option<u64>,
        req: &SaveRequest,
        view: &ViewRows,
        outcome: SaveOutcome,
        cx: &mut Context<Self>,
    ) {
        let same_result = store.read(cx).result.as_ref().map(|r| r.generation) == generation;
        store.update(cx, |st, cx| {
            st.saving = None;
            cx.notify();
        });
        if !same_result {
            // The tab was closed or its result replaced while saving.
            if let SaveOutcome::Saved(_) = outcome {
                self.status = "Changes saved".into();
            }
            cx.notify();
            return;
        }
        let shown_at = |row: Option<RowRef>| -> String {
            match row {
                Some(r @ RowRef::Data(_)) => view.position(r).map(|p| format!("row {}", p + 1)).unwrap_or_else(|| "a row".into()),
                Some(RowRef::New(_)) => "a new row".into(),
                None => String::new(),
            }
        };
        match outcome {
            SaveOutcome::Saved(report) => {
                store.update(cx, |st, cx| {
                    st.apply_saved(&report, view);
                    cx.notify();
                });
                self.output.update(cx, |o, cx| o.after_save(cx));
                self.set_tab_status(tab_id, saved_summary(&report));
            }
            SaveOutcome::Conflicts(conflicts) => {
                let st = store.read(cx);
                let columns = st.result.as_ref().map(|r| r.columns.clone()).unwrap_or_default();
                let rows = conflicts
                    .into_iter()
                    .map(|c| {
                        let (loaded, yours) = match c.row {
                            RowRef::Data(i) => (
                                req.updates.iter().find(|u| u.row == c.row).map(|u| u.original.clone())
                                    .or_else(|| req.deletes.iter().find(|d| d.row == c.row).map(|d| d.original.clone()))
                                    .or_else(|| st.result.as_ref().and_then(|r| r.rows.get(i).cloned()))
                                    .unwrap_or_default(),
                                req.updates.iter().find(|u| u.row == c.row).map(|u| u.sets.clone()).unwrap_or_default(),
                            ),
                            RowRef::New(_) => (Vec::new(), Vec::new()),
                        };
                        ConflictRow { shown_at: view.position(c.row).map(|p| p + 1), conflict: c, loaded, yours }
                    })
                    .collect();
                self.save_conflicts = Some(ConflictPrompt { tab_id: tab_id.to_string(), columns, key: req.key.clone(), conflicts: rows });
                self.set_tab_status(tab_id, "Save paused: rows changed since they were loaded");
            }
            SaveOutcome::Failed(f) => {
                let at = shown_at(f.row);
                let place = if at.is_empty() { String::new() } else { format!(" ({at})") };
                let message = format!("Error while {}{place}: {}", f.action.label(), f.message);
                store.update(cx, |st, cx| {
                    st.save_error = Some((message.clone(), f.row));
                    cx.notify();
                });
                if let Some(row) = f.row {
                    self.output.update(cx, |o, cx| o.reveal_row(row, cx));
                }
                self.set_tab_status(tab_id, "Save failed — nothing was changed");
                self.show_error_popup("Save failed — nothing was changed", format!("{message}\n\nAll changes in this save were rolled back. Your edits are still here, so you can fix the problem and save again."), cx);
            }
            SaveOutcome::Cancelled => {
                self.set_tab_status(tab_id, "Save stopped — nothing was changed");
            }
            SaveOutcome::CommitUnknown(e) => {
                store.update(cx, |st, cx| {
                    st.save_error = Some((format!("Commit status unknown: {e}"), None));
                    cx.notify();
                });
                self.set_tab_status(tab_id, "Commit status unknown — re-run the query to check");
                self.show_error_popup(
                    "Commit status unknown",
                    format!("The connection failed while committing ({e}). The changes may or may not have been saved. Re-run the query to check before saving again; your edits are still here."),
                    cx,
                );
            }
        }
        self.output_tab = OutputTab::Results;
        cx.notify();
    }

    /// The Stop button.
    pub fn stop_save(&mut self, tab_id: &str, cx: &mut Context<Self>) {
        let Some(store) = self.sql_store(tab_id) else { return };
        let Some(ctl) = store.read(cx).saving.clone() else { return };
        if ctl.committing() {
            return;
        }
        self.set_tab_status(tab_id, "Stopping the save…");
        drop(runtime::spawn(async move { ctl.stop().await }));
        cx.notify();
    }

    /// "Open as SQL": the statements a save would run, in a new tab.
    pub fn open_edits_as_sql(&mut self, cx: &mut Context<Self>) {
        let tab_id = self.active_tab_id.clone();
        let (req, source) = match self.save_request(&tab_id, false, cx) {
            Ok(r) => r,
            Err(e) => {
                self.show_error_popup("Can't build the SQL", e, cx);
                return;
            }
        };
        let sql = req.script();
        if sql.is_empty() {
            return;
        }
        let sql = format!(
            "-- Changes queued in the grid, as SQL. Running this is not the same as\n-- Save: it does not check for rows changed since they were loaded.\n{}\n",
            crate::ui::sql_text::formatted(&sql, &source.driver)
        );
        let id = self.add_tab(source.conn_id, source.database, cx);
        self.set_tab_default_schema(&id, source.default_schema, cx);
        self.set_tab_sql(&id, &sql, cx);
        self.set_active_tab(id, cx);
    }

    // ─── Discarding changes ─────────────────────────────────────────────────

    /// Ask before `action` if it would throw away queued changes; otherwise
    /// (or once confirmed) do it.
    pub fn guard_discard(&mut self, action: DiscardAction, cx: &mut Context<Self>) {
        let affected = self.discard_affects(&action, cx);
        if affected.is_empty() {
            self.do_discard_action(action, cx);
        } else {
            self.discard_confirm = Some(action);
            cx.notify();
        }
    }

    fn discard_affects(&self, action: &DiscardAction, cx: &gpui::App) -> Vec<TabId> {
        let changed = |id: &str| self.tabs_with_changes(None, cx).iter().any(|t| t == id);
        match action {
            DiscardAction::RunQuery(id) | DiscardAction::Clear(id) => {
                if changed(id) { vec![id.clone()] } else { vec![] }
            }
            DiscardAction::CloseTabs(ids) => ids.iter().filter(|id| changed(id)).cloned().collect(),
            DiscardAction::Disconnect(conn) => self.tabs_with_changes(Some(conn), cx),
            DiscardAction::DeleteGroup(group) => {
                let conns: Vec<String> = self.settings.server_groups.iter().find(|g| g.id == *group).map(|g| g.connection_ids.clone()).unwrap_or_default();
                conns.iter().flat_map(|c| self.tabs_with_changes(Some(c), cx)).collect()
            }
            DiscardAction::Quit => self.tabs_with_changes(None, cx),
        }
    }

    /// Carry out `action`, dropping any queued changes it affects.
    pub fn do_discard_action(&mut self, action: DiscardAction, cx: &mut Context<Self>) {
        for id in self.discard_affects(&action, cx) {
            self.stop_save(&id, cx);
            if let Some(store) = self.sql_store(&id) {
                store.update(cx, |st, cx| {
                    st.pending.clear();
                    st.save_error = None;
                    cx.notify();
                });
            }
        }
        match action {
            DiscardAction::RunQuery(id) => self.run_query(&id, cx),
            DiscardAction::CloseTabs(ids) => {
                for id in ids {
                    self.remove_tab(&id, cx);
                }
            }
            DiscardAction::Disconnect(conn) => self.disconnect_conn(&conn, cx),
            DiscardAction::DeleteGroup(group) => self.delete_server_group(&group, cx),
            DiscardAction::Clear(_) => {}
            DiscardAction::Quit => cx.quit(),
        }
        cx.notify();
    }

    /// Quit, or ask first when there are unsaved changes. Returns whether it
    /// is fine to quit straight away.
    pub fn request_quit(&mut self, cx: &mut Context<Self>) -> bool {
        if self.tabs_with_changes(None, cx).is_empty() {
            return true;
        }
        self.discard_confirm = Some(DiscardAction::Quit);
        cx.notify();
        false
    }

    // ─── Dialogs ────────────────────────────────────────────────────────────

    pub fn render_discard_confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let action = self.discard_confirm.clone()?;
        let tabs = self.discard_affects(&action, cx);
        if tabs.is_empty() {
            // Nothing left to lose (saved meanwhile): drop the question rather
            // than act on an answer that was never given.
            self.discard_confirm = None;
            return None;
        }
        let s = Scale(self.scale());
        let titles: Vec<String> = tabs.iter().filter_map(|id| self.tab(id)).map(|t| format!("\u{2022} {}", t.title)).collect();
        let counts: usize = tabs.iter().filter_map(|id| self.sql_store(id)).map(|st| st.read(cx).pending.count()).sum();
        let (title, body, yes) = match &action {
            DiscardAction::Clear(_) => ("Discard Changes?", format!("Discard {counts} unsaved change(s)? This can't be undone."), "Discard"),
            DiscardAction::RunQuery(_) => ("Run Query?", format!("Running the query again replaces the result and discards {counts} unsaved change(s)."), "Discard and Run"),
            DiscardAction::CloseTabs(_) => ("Close Tab?", format!("Closing discards {counts} unsaved change(s) in:"), "Discard and Close"),
            DiscardAction::Disconnect(_) => ("Disconnect?", format!("Disconnecting closes its tabs and discards {counts} unsaved change(s) in:"), "Discard and Disconnect"),
            DiscardAction::DeleteGroup(_) => ("Delete Server Group?", format!("Deleting the group disconnects its connections and discards {counts} unsaved change(s) in:"), "Discard and Delete"),
            DiscardAction::Quit => ("Quit multidb?", format!("Quitting discards {counts} unsaved change(s) in:"), "Discard and Quit"),
        };
        let list = !matches!(action, DiscardAction::Clear(_) | DiscardAction::RunQuery(_));
        let modal = dialogs::modal_box(460.0)
            .id("discard-confirm")
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialogs::modal_header(s, title, cx.listener(|this, _, _, cx| {
                this.discard_confirm = None;
                cx.notify();
            })))
            .child(
                div()
                    .px(px(20.))
                    .py(px(16.))
                    .flex()
                    .flex_col()
                    .gap(px(6.))
                    .t(s, 13.0)
                    .text_color(theme::TEXT_DIM)
                    .child(body)
                    .when(list, |d| d.children(titles.into_iter().map(|t| div().text_color(theme::TEXT).child(t)))),
            )
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(dialogs::button("discard-no", s, "Keep Changes", Btn::Secondary, false, cx.listener(|this, _, _, cx| {
                        this.discard_confirm = None;
                        cx.notify();
                    })))
                    .child(dialogs::button("discard-yes", s, yes, Btn::Stop, false, cx.listener(move |this, _, _, cx| {
                        this.discard_confirm = None;
                        this.do_discard_action(action.clone(), cx);
                    }))),
            );
        Some(
            overlay(0.6)
                .id("discard-overlay")
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.discard_confirm = None;
                    cx.notify();
                }))
                .child(modal)
                .into_any_element(),
        )
    }

    /// Saving to a table that can't roll back (MySQL MyISAM): say so first.
    pub fn render_nontx_confirm(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let tab_id = self.nontx_confirm.clone()?;
        let Some(store) = self.sql_store(&tab_id) else {
            self.nontx_confirm = None;
            return None;
        };
        let engine = store.read(cx).edit_info.as_ref().map(|i| i.meta.engine.clone()).unwrap_or_default();
        let s = Scale(self.scale());
        let modal = dialogs::modal_box(460.0)
            .id("nontx-confirm")
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialogs::modal_header(s, "Changes Can't Be Rolled Back", cx.listener(|this, _, _, cx| {
                this.nontx_confirm = None;
                cx.notify();
            })))
            .child(div().px(px(20.)).py(px(16.)).t(s, 13.0).text_color(theme::TEXT_DIM).child(format!(
                "This table uses the {engine} storage engine, which does not support transactions. If a statement fails part way through, the changes made before it stay saved."
            )))
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(dialogs::button("nontx-no", s, "Cancel", Btn::Secondary, false, cx.listener(|this, _, _, cx| {
                        this.nontx_confirm = None;
                        cx.notify();
                    })))
                    .child(dialogs::button("nontx-yes", s, "Save Anyway", Btn::Primary, false, cx.listener(move |this, _, _, cx| {
                        this.nontx_confirm = None;
                        this.start_save(&tab_id, true, None, cx);
                    }))),
            );
        Some(
            overlay(0.6)
                .id("nontx-overlay")
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.nontx_confirm = None;
                    cx.notify();
                }))
                .child(modal)
                .into_any_element(),
        )
    }

    pub fn render_save_conflicts(&mut self, window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.save_conflicts.clone()?;
        let s = Scale(self.scale());
        let mono = self.mono_family.clone();
        let total = prompt.conflicts.len();
        // Colours: loaded (struck through), theirs (blue), yours (amber).
        let loaded_fg = theme::rgba8(154, 154, 172, 1.0);
        let theirs_fg = theme::rgba8(199, 215, 255, 1.0);
        let theirs_bg = theme::rgba8(91, 140, 255, 0.14);
        let yours_fg = theme::rgba8(255, 216, 117, 1.0);
        let yours_bg = theme::rgba8(255, 200, 50, 0.13);
        let warn_fg = theme::rgba8(255, 180, 168, 1.0);
        let line_border = theme::rgba8(38, 38, 54, 1.0);
        let name_w = px(120. * s.0);
        let cell = |content: AnyElement| div().flex_1().min_w(px(0.)).px(px(12.)).py(px(7.)).child(content);
        let value = |text: String, fg: theme::Rgba| {
            div().font_family(mono.clone()).text_color(fg).overflow_hidden().text_ellipsis().whitespace_nowrap().child(short(text)).into_any_element()
        };
        let note = |text: &str, fg: theme::Rgba| div().text_color(fg).child(text.to_string()).into_any_element();

        let mut list = div().flex().flex_col().gap(px(12.));
        for row in prompt.conflicts.iter().take(CONFLICTS_SHOWN) {
            let place = match (row.conflict.row, row.shown_at) {
                (RowRef::Data(_), Some(n)) => format!("Row {n}"),
                _ => "Row".into(),
            };
            let key = prompt
                .key
                .iter()
                .map(|&k| format!("{} = {}", prompt.columns.get(k).cloned().unwrap_or_default(), row.loaded.get(k).map(value_text).unwrap_or_default()))
                .collect::<Vec<_>>()
                .join(", ");
            let badge = match conflict_kind(row) {
                ConflictKind::Both(cols) => {
                    let names: Vec<String> = cols.iter().filter_map(|c| prompt.columns.get(*c).cloned()).collect();
                    Some(format!("You both changed {}", names.join(", ")))
                }
                ConflictKind::Deleting => Some("Marked for deletion".to_string()),
                ConflictKind::Separate => None,
            };
            let header = div()
                .flex()
                .items_center()
                .gap(px(10.))
                .px(px(12.))
                .py(px(9.))
                .bg(theme::rgba8(35, 35, 58, 1.0))
                .child(div().t(s, 13.0).font_weight(FontWeight::SEMIBOLD).child(place))
                .child(div().t(s, 12.0).font_family(mono.clone()).text_color(theme::TEXT_DIM).overflow_hidden().text_ellipsis().whitespace_nowrap().min_w(px(0.)).child(key))
                .child(div().flex_1())
                .child(match badge {
                    Some(b) => div()
                        .flex_none()
                        .t(s, 11.0)
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(warn_fg)
                        .bg(theme::rgba8(248, 113, 113, 0.16))
                        .border_1()
                        .border_color(theme::rgba8(248, 113, 113, 0.45))
                        .rounded(px(999.))
                        .px(px(8.))
                        .py(px(2.))
                        .child(b)
                        .into_any_element(),
                    None => div().flex_none().t(s, 11.0).text_color(theme::TEXT_DIM).child("Different columns changed").into_any_element(),
                });
            let column_heads = div()
                .flex()
                .t(s, 12.0)
                .font_weight(FontWeight::MEDIUM)
                .text_color(theme::TEXT_MUTED)
                .border_b_1()
                .border_color(theme::BORDER)
                .child(div().w(name_w).flex_none().px(px(12.)).py(px(7.)).child("Column"))
                .child(cell(note("Loaded", theme::TEXT_MUTED)))
                .child(cell(note("Now in database", theme::TEXT_MUTED)))
                .child(cell(note("Your change", theme::TEXT_MUTED)));
            let lines = conflict_lines(row);
            let count = lines.len();
            let mut table = div().flex().flex_col().t(s, 12.0).child(column_heads);
            for (i, line) in lines.into_iter().enumerate() {
                let name = prompt.columns.get(line.col).cloned().unwrap_or_default();
                // Struck through where their value replaced it.
                let loaded = value(line.loaded, loaded_fg);
                let loaded = if line.now.is_some() { div().line_through().child(loaded).into_any_element() } else { loaded };
                let (now_el, now_bg) = match line.now {
                    Some(v) => (value(v, theirs_fg), Some(theirs_bg)),
                    None => (note("unchanged", theme::TEXT_MUTED), None),
                };
                let (yours_el, yours_bg_c) = match line.yours {
                    Yours::Value(v) => (value(v, yours_fg), Some(yours_bg)),
                    Yours::NotEdited => (note("not edited", theme::TEXT_MUTED), None),
                    Yours::Deleted => (note("row deleted", warn_fg), None),
                };
                table = table.child(
                    div()
                        .flex()
                        .when(i + 1 < count, |d| d.border_b_1().border_color(line_border))
                        .child(div().w(name_w).flex_none().px(px(12.)).py(px(7.)).font_weight(FontWeight::MEDIUM).overflow_hidden().text_ellipsis().whitespace_nowrap().child(name))
                        .child(cell(loaded))
                        .child(cell(now_el).when_some(now_bg, |d, bg| d.bg(bg)))
                        .child(cell(yours_el).when_some(yours_bg_c, |d, bg| d.bg(bg))),
                );
            }
            list = list.child(div().border_1().border_color(theme::BORDER).rounded(px(8.)).overflow_hidden().child(header).child(table));
        }
        if total > CONFLICTS_SHOWN {
            list = list.child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).child(format!("…and {} more rows", total - CONFLICTS_SHOWN)));
        }

        // Only the list scrolls: the header and the buttons always stay on screen.
        let scroll = self.conflicts_scroll.clone();
        let body = div()
            .id("save-conflicts-list")
            .flex_1()
            .min_h(px(0.))
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .px(px(22.))
            .py(px(14.))
            .child(list)
            .into_any_element();
        let body = self.scrollable("save-conflicts", &scroll, body, cx).flex_1().min_h(px(0.));
        let max_h = (f32::from(window.viewport_size().height) - 2.0 * dialogs::MODAL_GUTTER).max(240.0);

        let title = if total == 1 { "1 row changed since you loaded it".to_string() } else { format!("{total} rows changed since you loaded them") };
        let swatch = |color: theme::Rgba, label: &'static str| {
            div().flex().items_center().gap(px(6.)).child(div().w(px(10.)).h(px(10.)).rounded(px(2.)).bg(color)).child(label)
        };
        let close = cx.listener(|this, _, _, cx| {
            this.save_conflicts = None;
            cx.notify();
        });
        let header = div()
            .flex_none()
            .flex()
            .items_start()
            .gap(px(14.))
            .px(px(22.))
            .pt(px(20.))
            .pb(px(16.))
            .border_b_1()
            .border_color(theme::BORDER)
            .child(
                div()
                    .flex_none()
                    .w(px(36.))
                    .h(px(36.))
                    .rounded(px(8.))
                    .bg(theme::rgba8(255, 200, 50, 0.14))
                    .flex()
                    .items_center()
                    .justify_center()
                    .t(s, 18.0)
                    .font_weight(FontWeight::BOLD)
                    .text_color(theme::AMBER)
                    .child("!"),
            )
            .child(
                div()
                    .flex_1()
                    .flex()
                    .flex_col()
                    .gap(px(4.))
                    .child(div().t(s, 17.0).font_weight(FontWeight::SEMIBOLD).child(title))
                    .child(div().t(s, 13.0).text_color(theme::TEXT_DIM).child("Someone else saved changes to these rows. Nothing has been written yet.")),
            )
            .child(
                div()
                    .id("conflict-close")
                    .flex_none()
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(6.))
                    .cursor_pointer()
                    .hover(|st| st.bg(theme::BG_HOVER))
                    .on_click(close)
                    .child(crate::ui::icons::icon(crate::ui::icons::Icon::Close, s.fs(14.0), theme::TEXT_MUTED)),
            );
        let legend = div()
            .flex_none()
            .flex()
            .gap(px(16.))
            .px(px(22.))
            .py(px(10.))
            .bg(theme::BG_PANEL)
            .border_b_1()
            .border_color(theme::BORDER)
            .t(s, 12.0)
            .text_color(theme::TEXT_DIM)
            .child(swatch(theme::rgba8(59, 59, 82, 1.0), "Loaded"))
            .child(swatch(theme::rgba8(91, 140, 255, 1.0), "Now in database"))
            .child(swatch(theme::AMBER, "Your change"));
        let tab_id = prompt.tab_id.clone();
        let footer = div()
            .flex_none()
            .flex()
            .items_center()
            .gap(px(10.))
            .px(px(22.))
            .py(px(14.))
            .border_t_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_PANEL)
            .child(div().flex_1().t(s, 12.0).text_color(theme::TEXT_DIM).child("Override writes your changes over theirs. Cancel keeps your changes queued."))
            .child(dialogs::button("conflict-override", s, "Override", Btn::Stop, false, cx.listener(move |this, _, _, cx| {
                this.save_conflicts = None;
                this.start_save(&tab_id, false, None, cx);
            })))
            .child(dialogs::button("conflict-cancel", s, "Cancel", Btn::Primary, false, cx.listener(|this, _, _, cx| {
                this.save_conflicts = None;
                cx.notify();
            })));
        let modal = dialogs::modal_box(720.0)
            .id("save-conflicts")
            .occlude()
            .flex()
            .flex_col()
            .max_h(px(max_h))
            .overflow_hidden()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(header)
            .child(legend)
            .child(body)
            .child(footer);
        Some(
            overlay(0.6)
                .id("save-conflicts-overlay")
                .occlude()
                .on_mouse_down(MouseButton::Left, cx.listener(|this, _, _, cx| {
                    this.save_conflicts = None;
                    cx.notify();
                }))
                .child(modal)
                .into_any_element(),
        )
    }
}

/// "Saved: 2 updated, 1 inserted, 3 deleted (1 was already deleted)".
pub fn saved_summary(r: &SaveReport) -> SharedString {
    let mut parts = Vec::new();
    if !r.updated.is_empty() {
        parts.push(format!("{} updated", r.updated.len()));
    }
    if !r.inserted.is_empty() {
        parts.push(format!("{} inserted", r.inserted.len()));
    }
    if !r.deleted.is_empty() {
        parts.push(format!("{} deleted", r.deleted.len()));
    }
    let mut text = if parts.is_empty() { "Saved".to_string() } else { format!("Saved: {}", parts.join(", ")) };
    if !r.already_deleted.is_empty() {
        text.push_str(&format!(" ({} already deleted by someone else)", r.already_deleted.len()));
    }
    text.into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(changed: Vec<usize>, deleting: bool, yours: Vec<(usize, Cell)>) -> ConflictRow {
        ConflictRow {
            conflict: Conflict {
                row: RowRef::Data(2),
                current: vec![json!(3), json!("Fast as a Shark"), json!("1.29"), json!(263500)],
                changed,
                deleting,
            },
            shown_at: Some(3),
            loaded: vec![json!(3), json!("Fast As a Shark"), json!("0.99"), Value::Null],
            yours,
        }
    }

    #[test]
    fn both_changed_lists_theirs_then_your_other_edits() {
        let r = row(vec![1, 2], false, vec![(1, Cell::Value("Live".into())), (3, Cell::Now)]);
        assert_eq!(conflict_kind(&r), ConflictKind::Both(vec![1]));
        let lines = conflict_lines(&r);
        assert_eq!(lines.len(), 3);
        assert_eq!(lines[0], ConflictLine { col: 1, loaded: "Fast As a Shark".into(), now: Some("Fast as a Shark".into()), yours: Yours::Value("Live".into()) });
        assert_eq!(lines[1].yours, Yours::NotEdited);
        assert_eq!(lines[1].now.as_deref(), Some("1.29"));
        assert_eq!(lines[2], ConflictLine { col: 3, loaded: "NULL".into(), now: None, yours: Yours::Value("NOW()".into()) });
    }

    #[test]
    fn a_row_being_deleted_shows_only_their_changes() {
        let r = row(vec![2], true, vec![(1, Cell::Value("ignored".into()))]);
        assert_eq!(conflict_kind(&r), ConflictKind::Deleting);
        let lines = conflict_lines(&r);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].yours, Yours::Deleted);
    }

    #[test]
    fn different_columns_are_not_a_clash() {
        let r = row(vec![3], false, vec![(1, Cell::Value("x".into()))]);
        assert_eq!(conflict_kind(&r), ConflictKind::Separate);
        assert_eq!(conflict_lines(&r).iter().map(|l| l.col).collect::<Vec<_>>(), vec![3, 1]);
    }
}
