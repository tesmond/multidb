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
                self.save_conflicts = Some(ConflictPrompt { tab_id: tab_id.to_string(), columns, conflicts: rows });
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
        let sql = format!("-- Changes queued in the grid, as SQL. Running this is not the same as\n-- Save: it does not check for rows changed since they were loaded.\n{sql}\n");
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

    pub fn render_save_conflicts(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> Option<AnyElement> {
        let prompt = self.save_conflicts.clone()?;
        let s = Scale(self.scale());
        let total = prompt.conflicts.len();
        let deleting = prompt.conflicts.iter().filter(|c| c.conflict.deleting).count();
        let mut list = div().flex().flex_col().gap(px(10.));
        for row in prompt.conflicts.iter().take(CONFLICTS_SHOWN) {
            let place = match (row.conflict.row, row.shown_at) {
                (RowRef::Data(_), Some(n)) => format!("Row {n}"),
                _ => "Row".into(),
            };
            let what = if row.conflict.deleting { " (marked for deletion)" } else { "" };
            let mut entry = div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .child(div().t(s, 12.0).font_weight(FontWeight::SEMIBOLD).text_color(theme::TEXT).child(format!("{place}{what}")));
            for &c in &row.conflict.changed {
                let name = prompt.columns.get(c).cloned().unwrap_or_default();
                let loaded = row.loaded.get(c).map(value_text).unwrap_or_default();
                let now = row.conflict.current.get(c).map(value_text).unwrap_or_default();
                let yours = row.yours.iter().find(|(col, _)| *col == c).map(|(_, v)| cell_text(v));
                let mut line = format!("{name}: was {}, now {}", short(loaded), short(now));
                if let Some(y) = yours {
                    line.push_str(&format!(", yours {}", short(y)));
                }
                entry = entry.child(div().t(s, 12.0).text_color(theme::TEXT_DIM).child(line));
            }
            list = list.child(entry);
        }
        if total > CONFLICTS_SHOWN {
            list = list.child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).child(format!("…and {} more", total - CONFLICTS_SHOWN)));
        }
        let scroll = self.conflicts_scroll.clone();
        let body = div()
            .id("save-conflicts-list")
            .overflow_y_scroll()
            .track_scroll(&scroll)
            .max_h(px(360.))
            .px(px(20.))
            .py(px(12.))
            .child(list)
            .into_any_element();
        let body = self.scrollable("save-conflicts", &scroll, body, cx);
        let intro = format!(
            "{total} row(s) changed in the database since they were loaded{}. Nothing has been saved. Override writes your changes over theirs; Cancel keeps your changes queued so you can look again.",
            if deleting > 0 { format!(", {deleting} of them marked for deletion") } else { String::new() }
        );
        let tab_id = prompt.tab_id.clone();
        let modal = dialogs::modal_box(560.0)
            .id("save-conflicts")
            .occlude()
            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
            .child(dialogs::modal_header(s, "Rows Changed Since Loaded", cx.listener(|this, _, _, cx| {
                this.save_conflicts = None;
                cx.notify();
            })))
            .child(div().px(px(20.)).pt(px(14.)).t(s, 13.0).text_color(theme::TEXT_DIM).child(intro))
            .child(body)
            .child(
                div()
                    .flex()
                    .justify_end()
                    .gap(px(8.))
                    .px(px(20.))
                    .py(px(14.))
                    .border_t_1()
                    .border_color(theme::BORDER)
                    .child(dialogs::button("conflict-cancel", s, "Cancel", Btn::Primary, false, cx.listener(|this, _, _, cx| {
                        this.save_conflicts = None;
                        cx.notify();
                    })))
                    .child(dialogs::button("conflict-override", s, "Override", Btn::Stop, false, cx.listener(move |this, _, _, cx| {
                        this.save_conflicts = None;
                        this.start_save(&tab_id, false, None, cx);
                    }))),
            );
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
