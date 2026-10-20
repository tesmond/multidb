//! Running a save: one transaction, rows locked and checked against what was
//! loaded, then deletes, updates and inserts, each checked for how many rows
//! it touched, then the written rows read back and the transaction committed.
//! Any failure rolls the whole thing back.

use super::plan::{Bind, InsertLookup, RowUpdate, SaveRequest, Stmt, BATCH};
use super::{Driver, RowRef};
use anyhow::{anyhow, Result};
use futures::future::BoxFuture;
use serde_json::Value;
use sqlx::pool::PoolConnection;
use sqlx::{MySql, Postgres, Sqlite};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

/// What happened to a save.
#[derive(Debug, Clone, PartialEq)]
pub enum SaveOutcome {
    Saved(SaveReport),
    /// Rows changed since they were loaded; nothing was written.
    Conflicts(Vec<Conflict>),
    Failed(SaveFailure),
    /// Stopped by the user; rolled back.
    Cancelled,
    /// The connection failed while committing: the changes may or may not
    /// have been saved.
    CommitUnknown(String),
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct SaveReport {
    /// Updated rows as they are now, in result-column order.
    pub updated: Vec<(RowRef, Vec<Value>)>,
    /// Inserted rows as stored, in result-column order.
    pub inserted: Vec<(RowRef, Vec<Value>)>,
    /// Rows deleted, including any that were already gone.
    pub deleted: Vec<RowRef>,
    /// Rows queued for deletion that someone else had already deleted.
    pub already_deleted: Vec<RowRef>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Conflict {
    pub row: RowRef,
    /// The row as it is in the database now, in result-column order.
    pub current: Vec<Value>,
    /// Result columns whose value differs from what was loaded.
    pub changed: Vec<usize>,
    /// The row is queued for deletion (rather than edited).
    pub deleting: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Begin,
    Check,
    Delete,
    Update,
    Insert,
    Commit,
}

impl Action {
    pub fn label(self) -> &'static str {
        match self {
            Action::Begin => "starting the transaction",
            Action::Check => "checking for changes",
            Action::Delete => "deleting",
            Action::Update => "updating",
            Action::Insert => "inserting",
            Action::Commit => "committing",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SaveFailure {
    pub action: Action,
    pub row: Option<RowRef>,
    pub message: String,
}

/// Shared between a running save and the UI: progress, and the Stop button.
#[derive(Default)]
pub struct SaveControl {
    cancel: CancellationToken,
    pub done: AtomicUsize,
    pub total: AtomicUsize,
    committing: AtomicBool,
    /// Held while cancelling and while starting to commit, so a Stop either
    /// lands before COMMIT is sent or not at all.
    gate: tokio::sync::Mutex<Option<ServerCancel>>,
}

enum ServerCancel {
    Postgres { pool: sqlx::PgPool, pid: i32 },
    MySql { pool: sqlx::MySqlPool, id: u64 },
}

impl SaveControl {
    pub fn new() -> Arc<SaveControl> {
        Arc::new(SaveControl::default())
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }

    pub fn committing(&self) -> bool {
        self.committing.load(Ordering::SeqCst)
    }

    /// Stop the save: cancel the statement running on the server and roll
    /// back. Returns false once the commit has been sent, when it is too late.
    pub async fn stop(&self) -> bool {
        let gate = self.gate.lock().await;
        if self.committing() {
            return false;
        }
        self.cancel.cancel();
        match &*gate {
            Some(ServerCancel::Postgres { pool, pid }) => {
                let _ = sqlx::query("SELECT pg_cancel_backend($1)").bind(*pid).execute(pool).await;
            }
            Some(ServerCancel::MySql { pool, id }) => {
                let _ = sqlx::query(&format!("KILL QUERY {id}")).execute(pool).await;
            }
            None => {}
        }
        true
    }

    /// Mark the commit as started, unless a Stop got in first.
    async fn begin_commit(&self) -> bool {
        let _gate = self.gate.lock().await;
        if self.cancel.is_cancelled() {
            return false;
        }
        self.committing.store(true, Ordering::SeqCst);
        true
    }

    #[cfg(test)]
    pub async fn begin_commit_for_test(&self) -> bool {
        self.begin_commit().await
    }

    fn step(&self, n: usize) {
        self.done.fetch_add(n, Ordering::Relaxed);
    }
}

/// What a statement did.
pub struct Done {
    pub rows_affected: u64,
    /// MySQL `LAST_INSERT_ID()` / SQLite `last_insert_rowid()`.
    pub last_insert_id: Option<i64>,
}

/// The few things a save needs from a connection.
pub trait Conn: Send {
    fn driver(&self) -> Driver;
    fn execute<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Done>>;
    fn fetch<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Vec<Vec<Value>>>>;
    /// A statement without parameters (BEGIN, COMMIT, SET …).
    fn simple<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<()>>;
    /// Warnings the last statement raised (MySQL); a warning means a value
    /// was silently changed, which a save must not accept.
    fn warnings<'a>(&'a mut self) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async { Ok(None) })
    }
}

macro_rules! bind_all {
    ($q:expr, $binds:expr, $pg:expr) => {{
        let mut q = $q;
        for b in $binds {
            q = match b {
                Bind::Null => q.bind(None::<String>),
                Bind::Text(t) => q.bind(t.clone()),
                Bind::Int(i) if $pg => q.bind(i.to_string()),
                Bind::Int(i) => q.bind(*i),
                Bind::UInt(u) if $pg => q.bind(u.to_string()),
                Bind::UInt(u) => q.bind(u.to_string()),
                Bind::Float(f) if $pg => q.bind(f.to_string()),
                Bind::Float(f) => q.bind(*f),
            };
        }
        q
    }};
}

impl Conn for PoolConnection<Postgres> {
    fn driver(&self) -> Driver {
        Driver::Postgres
    }
    fn execute<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Done>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, true);
            let r = sqlx::Executor::execute(&mut **self, q).await?;
            Ok(Done { rows_affected: r.rows_affected(), last_insert_id: None })
        })
    }
    fn fetch<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Vec<Vec<Value>>>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, true);
            let rows = sqlx::Executor::fetch_all(&mut **self, q).await?;
            rows.iter().map(crate::queries::pg_row_to_json_values).collect()
        })
    }
    fn simple<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            sqlx::Executor::execute(&mut **self, sql).await?;
            Ok(())
        })
    }
}

impl Conn for PoolConnection<MySql> {
    fn driver(&self) -> Driver {
        Driver::MySql
    }
    fn execute<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Done>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, false);
            let r = sqlx::Executor::execute(&mut **self, q).await?;
            Ok(Done { rows_affected: r.rows_affected(), last_insert_id: Some(r.last_insert_id() as i64) })
        })
    }
    fn fetch<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Vec<Vec<Value>>>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, false);
            let rows = sqlx::Executor::fetch_all(&mut **self, q).await?;
            rows.iter().map(crate::queries::mysql_row_to_json_values).collect()
        })
    }
    fn simple<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            sqlx::Executor::execute(&mut **self, sql).await?;
            Ok(())
        })
    }
    fn warnings<'a>(&'a mut self) -> BoxFuture<'a, Result<Option<String>>> {
        Box::pin(async move {
            use sqlx::Row;
            let rows = sqlx::Executor::fetch_all(&mut **self, "SHOW WARNINGS").await?;
            for row in rows {
                let text = |i: usize| -> String {
                    row.try_get::<String, _>(i)
                        .or_else(|_| row.try_get::<Vec<u8>, _>(i).map(|b| String::from_utf8_lossy(&b).into_owned()))
                        .unwrap_or_default()
                };
                let level = text(0);
                if !level.eq_ignore_ascii_case("note") {
                    return Ok(Some(text(2)));
                }
            }
            Ok(None)
        })
    }
}

impl Conn for PoolConnection<Sqlite> {
    fn driver(&self) -> Driver {
        Driver::Sqlite
    }
    fn execute<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Done>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, false);
            let r = sqlx::Executor::execute(&mut **self, q).await?;
            Ok(Done { rows_affected: r.rows_affected(), last_insert_id: Some(r.last_insert_rowid()) })
        })
    }
    fn fetch<'a>(&'a mut self, stmt: &'a Stmt) -> BoxFuture<'a, Result<Vec<Vec<Value>>>> {
        Box::pin(async move {
            let q = bind_all!(sqlx::query(&stmt.sql), &stmt.binds, false);
            let rows = sqlx::Executor::fetch_all(&mut **self, q).await?;
            rows.iter().map(crate::queries::sqlite_row_to_json_values).collect()
        })
    }
    fn simple<'a>(&'a mut self, sql: &'a str) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            sqlx::Executor::execute(&mut **self, sql).await?;
            Ok(())
        })
    }
}

/// The message of an error, without sqlx's "error returned from database:" prefix.
pub fn error_message(err: &anyhow::Error) -> String {
    if let Some(sqlx::Error::Database(db)) = err.downcast_ref::<sqlx::Error>() {
        let mut msg = db.message().to_string();
        if let Some(c) = db.constraint() {
            if !msg.contains(c) {
                msg.push_str(&format!(" (constraint \"{c}\")"));
            }
        }
        return msg;
    }
    format!("{err:#}")
}

/// Whether the error came from the database refusing something (as opposed
/// to the connection failing), so the outcome is known.
fn is_database_error(err: &anyhow::Error) -> bool {
    matches!(err.downcast_ref::<sqlx::Error>(), Some(sqlx::Error::Database(_)))
}

/// A row's key, as a map key.
fn key_string(req: &SaveRequest, row: &[Value]) -> String {
    let parts: Vec<&Value> = req.key.iter().map(|&k| row.get(k).unwrap_or(&Value::Null)).collect();
    serde_json::to_string(&parts).unwrap_or_default()
}

/// A row as read back (table columns, in `read_columns` order) laid out in
/// result-column order; expression columns keep `fallback`'s values.
fn to_result_row(req: &SaveRequest, read: &[Value], fallback: Option<&[Value]>) -> Vec<Value> {
    let mut out: Vec<Value> = match fallback {
        Some(f) => f.to_vec(),
        None => vec![Value::Null; req.columns.len()],
    };
    out.resize(req.columns.len(), Value::Null);
    for ((_, cols), value) in req.read_columns().iter().zip(read) {
        for &c in cols {
            out[c] = value.clone();
        }
    }
    out
}

/// Result columns (that come from the table) whose values differ.
fn changed_columns(req: &SaveRequest, loaded: &[Value], now: &[Value]) -> Vec<usize> {
    (0..req.columns.len())
        .filter(|&c| req.columns[c].is_some() && loaded.get(c) != now.get(c))
        .collect()
}

struct Fail(Action, Option<RowRef>, anyhow::Error);

fn fail(action: Action, row: Option<RowRef>, err: impl Into<anyhow::Error>) -> Fail {
    Fail(action, row, err.into())
}

enum Body {
    Done(SaveReport),
    Conflicts(Vec<Conflict>),
}

/// Run `req` on `conn` in one transaction. The connection is left outside a
/// transaction whatever happens, or `Err` is returned to say it must be
/// closed rather than reused.
pub async fn run<C: Conn>(conn: &mut C, req: &SaveRequest, ctl: &SaveControl) -> std::result::Result<SaveOutcome, SaveOutcome> {
    let driver = conn.driver();
    if let Err((row, message)) = req.validate() {
        return Ok(SaveOutcome::Failed(SaveFailure { action: Action::Begin, row, message }));
    }
    let updates = req.effective_updates();
    let total = req.deletes.len() + updates.len() + req.inserts.len() + if req.check_conflicts { updates.len() + req.deletes.len() } else { 0 };
    ctl.total.store(total, Ordering::Relaxed);
    ctl.done.store(0, Ordering::Relaxed);

    // Session settings that must be put back afterwards.
    let restore: Option<String> = match driver {
        Driver::MySql => {
            let stmt = Stmt { sql: "SELECT @@SESSION.innodb_lock_wait_timeout".into(), binds: vec![], display: String::new() };
            let previous = conn.fetch(&stmt).await.ok().and_then(|rows| rows.first().and_then(|r| r.first().cloned()));
            let set = format!("SET SESSION innodb_lock_wait_timeout = {}", req.lock_timeout_secs.max(1));
            if let Err(e) = conn.simple(&set).await {
                return Ok(SaveOutcome::Failed(SaveFailure { action: Action::Begin, row: None, message: error_message(&e) }));
            }
            previous.and_then(|v| super::exact_text(&v)).map(|v| format!("SET SESSION innodb_lock_wait_timeout = {v}"))
        }
        Driver::Sqlite => {
            let stmt = Stmt { sql: "PRAGMA busy_timeout".into(), binds: vec![], display: String::new() };
            let previous = conn.fetch(&stmt).await.ok().and_then(|rows| rows.first().and_then(|r| r.first().cloned()));
            let set = format!("PRAGMA busy_timeout = {}", req.lock_timeout_secs.max(1) as u64 * 1000);
            if let Err(e) = conn.simple(&set).await {
                return Ok(SaveOutcome::Failed(SaveFailure { action: Action::Begin, row: None, message: error_message(&e) }));
            }
            previous.and_then(|v| super::exact_text(&v)).map(|v| format!("PRAGMA busy_timeout = {v}"))
        }
        Driver::Postgres => None,
    };

    let begin = match driver {
        Driver::Sqlite => "BEGIN IMMEDIATE",
        Driver::MySql => "START TRANSACTION",
        Driver::Postgres => "BEGIN",
    };
    let outcome = match conn.simple(begin).await {
        Err(e) => {
            let failure = SaveFailure { action: Action::Begin, row: None, message: error_message(&e) };
            Ok(SaveOutcome::Failed(failure))
        }
        Ok(()) => {
            let body = async {
                if driver == Driver::Postgres {
                    let set = format!("SET LOCAL lock_timeout = '{}s'", req.lock_timeout_secs.max(1));
                    conn.simple(&set).await.map_err(|e| fail(Action::Begin, None, e))?;
                }
                body(conn, req, ctl, &updates).await
            }
            .await;
            match body {
                Ok(Body::Done(report)) => {
                    if !ctl.begin_commit().await {
                        rollback(conn, Ok(SaveOutcome::Cancelled)).await
                    } else {
                        match conn.simple("COMMIT").await {
                            Ok(()) => Ok(SaveOutcome::Saved(report)),
                            Err(e) if is_database_error(&e) => {
                                // Refused at commit (a deferred constraint): rolled back.
                                let failure = SaveFailure { action: Action::Commit, row: None, message: error_message(&e) };
                                rollback(conn, Ok(SaveOutcome::Failed(failure))).await
                            }
                            Err(e) => Err(SaveOutcome::CommitUnknown(error_message(&e))),
                        }
                    }
                }
                Ok(Body::Conflicts(conflicts)) => rollback(conn, Ok(SaveOutcome::Conflicts(conflicts))).await,
                Err(Fail(action, row, err)) => {
                    let outcome = if ctl.is_cancelled() {
                        SaveOutcome::Cancelled
                    } else {
                        SaveOutcome::Failed(SaveFailure { action, row, message: error_message(&err) })
                    };
                    rollback(conn, Ok(outcome)).await
                }
            }
        }
    };
    if let Some(sql) = restore {
        if outcome.is_ok() && conn.simple(&sql).await.is_err() {
            return Err(outcome.unwrap_or_else(|o| o));
        }
    }
    outcome
}

/// Roll back, keeping `outcome`; if even that fails the connection's state is
/// unknown, which the `Err` reports.
async fn rollback<C: Conn>(conn: &mut C, outcome: std::result::Result<SaveOutcome, SaveOutcome>) -> std::result::Result<SaveOutcome, SaveOutcome> {
    match conn.simple("ROLLBACK").await {
        Ok(()) => outcome,
        Err(_) => Err(outcome.unwrap_or_else(|o| o)),
    }
}

async fn checked_execute<C: Conn>(conn: &mut C, stmt: &Stmt, ctl: &SaveControl) -> Result<Done> {
    if ctl.is_cancelled() {
        return Err(anyhow!("save stopped"));
    }
    let done = conn.execute(stmt).await?;
    if let Some(w) = conn.warnings().await? {
        return Err(anyhow!("{w}"));
    }
    Ok(done)
}

async fn checked_fetch<C: Conn>(conn: &mut C, stmt: &Stmt, ctl: &SaveControl) -> Result<Vec<Vec<Value>>> {
    if ctl.is_cancelled() {
        return Err(anyhow!("save stopped"));
    }
    let rows = conn.fetch(stmt).await?;
    if let Some(w) = conn.warnings().await? {
        return Err(anyhow!("{w}"));
    }
    Ok(rows)
}

async fn body<C: Conn>(conn: &mut C, req: &SaveRequest, ctl: &SaveControl, updates: &[&RowUpdate]) -> std::result::Result<Body, Fail> {
    let driver = req.driver();
    let mut report = SaveReport::default();
    let mut gone: HashSet<RowRef> = HashSet::new();

    // ── Conflict check: lock the rows and compare them with what was loaded.
    if req.check_conflicts {
        let mut wanted: Vec<(RowRef, &[Value], bool)> = Vec::new();
        for d in &req.deletes {
            wanted.push((d.row, &d.original, true));
        }
        for u in updates {
            wanted.push((u.row, &u.original, false));
        }
        let mut conflicts = Vec::new();
        for chunk in wanted.chunks(BATCH) {
            let originals: Vec<&[Value]> = chunk.iter().map(|(_, o, _)| *o).collect();
            let stmt = req.lock_select(&originals);
            let rows = checked_fetch(conn, &stmt, ctl).await.map_err(|e| fail(Action::Check, None, e))?;
            let mut found: HashMap<String, Vec<Value>> = HashMap::new();
            for read in rows {
                let row = to_result_row(req, &read, None);
                found.insert(key_string(req, &row), row);
            }
            for (row_ref, original, deleting) in chunk {
                match found.get(&key_string(req, original)) {
                    None if *deleting => {
                        gone.insert(*row_ref);
                    }
                    None => {
                        return Err(fail(
                            Action::Check,
                            Some(*row_ref),
                            anyhow!("this row no longer exists in the database; it was deleted or its key was changed"),
                        ));
                    }
                    Some(now) => {
                        let changed = changed_columns(req, original, now);
                        if !changed.is_empty() {
                            conflicts.push(Conflict { row: *row_ref, current: now.clone(), changed, deleting: *deleting });
                        }
                    }
                }
            }
            ctl.step(chunk.len());
        }
        if !conflicts.is_empty() {
            return Ok(Body::Conflicts(conflicts));
        }
    }

    // ── Deletes, in batches.
    let to_delete: Vec<&super::plan::RowDelete> = req.deletes.iter().filter(|d| !gone.contains(&d.row)).collect();
    for chunk in to_delete.chunks(BATCH) {
        let originals: Vec<&[Value]> = chunk.iter().map(|d| d.original.as_slice()).collect();
        let first = chunk.first().map(|d| d.row);
        let stmt = req.delete(&originals);
        let done = checked_execute(conn, &stmt, ctl).await.map_err(|e| fail(Action::Delete, first, e))?;
        let expected = chunk.len() as u64;
        if done.rows_affected > expected {
            return Err(fail(Action::Delete, first, anyhow!("deleting {expected} row(s) would have deleted {} rows", done.rows_affected)));
        }
        if done.rows_affected < expected {
            // Only rows someone else already deleted may be missing.
            let left = checked_fetch(conn, &req.select_by_loaded_key(&originals), ctl).await.map_err(|e| fail(Action::Delete, first, e))?;
            if !left.is_empty() {
                let row = to_result_row(req, &left[0], None);
                let row_ref = chunk.iter().find(|d| key_string(req, &d.original) == key_string(req, &row)).map(|d| d.row).or(first);
                return Err(fail(Action::Delete, row_ref, anyhow!("a row could not be deleted")));
            }
            // Which ones were already gone isn't known without the check; count them all as deleted.
        }
        for d in chunk {
            report.deleted.push(d.row);
        }
        ctl.step(chunk.len());
    }
    for d in &req.deletes {
        if gone.contains(&d.row) {
            report.already_deleted.push(d.row);
            report.deleted.push(d.row);
            ctl.step(1);
        }
    }

    // ── Updates, one row each.
    // (row, its key after the update, its values as loaded)
    type Reread<'a> = (RowRef, Vec<(usize, Bind)>, &'a [Value]);
    let mut reread: Vec<Reread> = Vec::new();
    for u in updates {
        let stmt = req.update(u);
        if req.returning() {
            let rows = checked_fetch(conn, &stmt, ctl).await.map_err(|e| fail(Action::Update, Some(u.row), e))?;
            match rows.len() {
                1 => report.updated.push((u.row, to_result_row(req, &rows[0], Some(&u.original)))),
                0 => return Err(fail(Action::Update, Some(u.row), anyhow!("this row no longer exists in the database; it was deleted or its key was changed"))),
                n => return Err(fail(Action::Update, Some(u.row), anyhow!("the update would have changed {n} rows"))),
            }
        } else {
            let done = checked_execute(conn, &stmt, ctl).await.map_err(|e| fail(Action::Update, Some(u.row), e))?;
            match done.rows_affected {
                1 => {}
                0 => {
                    // MySQL counts rows changed, not rows matched: an update
                    // that leaves the values as they were reports 0. The row
                    // must still exist.
                    let originals = [u.original.as_slice()];
                    let left = checked_fetch(conn, &req.select_by_loaded_key(&originals), ctl)
                        .await
                        .map_err(|e| fail(Action::Update, Some(u.row), e))?;
                    if left.len() != 1 {
                        return Err(fail(Action::Update, Some(u.row), anyhow!("this row no longer exists in the database; it was deleted or its key was changed")));
                    }
                }
                n => return Err(fail(Action::Update, Some(u.row), anyhow!("the update would have changed {n} rows"))),
            }
            reread.push((u.row, req.key_after_update(u), &u.original));
        }
        ctl.step(1);
    }

    // ── Inserts.
    for ins in &req.inserts {
        let stmt = req.insert(ins);
        if req.returning() {
            let rows = checked_fetch(conn, &stmt, ctl).await.map_err(|e| fail(Action::Insert, Some(ins.row), e))?;
            if rows.len() != 1 {
                return Err(fail(Action::Insert, Some(ins.row), anyhow!("the insert returned {} rows", rows.len())));
            }
            report.inserted.push((ins.row, to_result_row(req, &rows[0], None)));
        } else {
            let lookup = req.insert_lookup(ins).map_err(|m| fail(Action::Insert, Some(ins.row), anyhow!(m)))?;
            let done = checked_execute(conn, &stmt, ctl).await.map_err(|e| fail(Action::Insert, Some(ins.row), e))?;
            if done.rows_affected != 1 {
                return Err(fail(Action::Insert, Some(ins.row), anyhow!("the insert wrote {} rows", done.rows_affected)));
            }
            let select = match lookup {
                InsertLookup::Rowid => req.select_rowid(done.last_insert_id.unwrap_or(0)),
                InsertLookup::Key { mut entered, auto } => {
                    if let Some(col) = auto {
                        let id = done.last_insert_id.unwrap_or(0);
                        entered.push((col, if driver == Driver::MySql { Bind::UInt(id as u64) } else { Bind::Int(id) }));
                    }
                    req.select_written(&entered)
                }
            };
            let rows = checked_fetch(conn, &select, ctl).await.map_err(|e| fail(Action::Insert, Some(ins.row), e))?;
            if rows.len() != 1 {
                return Err(fail(Action::Insert, Some(ins.row), anyhow!("the new row could not be read back ({} rows matched its key)", rows.len())));
            }
            report.inserted.push((ins.row, to_result_row(req, &rows[0], None)));
        }
        ctl.step(1);
    }

    // ── Read back updated rows where the update couldn't return them.
    for (row_ref, key, original) in reread {
        let rows = checked_fetch(conn, &req.select_written(&key), ctl).await.map_err(|e| fail(Action::Update, Some(row_ref), e))?;
        if rows.len() != 1 {
            return Err(fail(Action::Update, Some(row_ref), anyhow!("the updated row could not be read back ({} rows matched its key)", rows.len())));
        }
        report.updated.push((row_ref, to_result_row(req, &rows[0], Some(original))));
    }
    Ok(Body::Done(report))
}

// ─── Entry point ────────────────────────────────────────────────────────────

/// Where a save gets its connection.
pub enum Source {
    Postgres(sqlx::PgPool),
    MySql(sqlx::MySqlPool),
    Sqlite(sqlx::SqlitePool),
}

/// Save `req` using a connection from `source`.
pub async fn save(source: Source, req: SaveRequest, ctl: Arc<SaveControl>) -> SaveOutcome {
    macro_rules! go {
        ($pool:expr, |$conn:ident| $cancel:expr) => {{
            let mut $conn = match $pool.acquire().await {
                Ok(c) => c,
                Err(e) => {
                    return SaveOutcome::Failed(SaveFailure { action: Action::Begin, row: None, message: error_message(&e.into()) });
                }
            };
            let cancel: Option<ServerCancel> = $cancel;
            *ctl.gate.lock().await = cancel;
            let result = run(&mut $conn, &req, &ctl).await;
            *ctl.gate.lock().await = None;
            match result {
                Ok(outcome) => outcome,
                Err(outcome) => {
                    // The connection's state is unknown: close it rather
                    // than give it back to the pool.
                    $conn.close_on_drop();
                    outcome
                }
            }
        }};
    }
    match source {
        Source::Postgres(pool) => go!(pool, |conn| {
            let pid: Option<i32> = sqlx::query_scalar("SELECT pg_backend_pid()").fetch_one(&mut *conn).await.ok();
            pid.map(|pid| ServerCancel::Postgres { pool: pool.clone(), pid })
        }),
        Source::MySql(pool) => go!(pool, |conn| {
            let id: Option<u64> = sqlx::query_scalar("SELECT CONNECTION_ID()").fetch_one(&mut *conn).await.ok();
            id.map(|id| ServerCancel::MySql { pool: pool.clone(), id })
        }),
        Source::Sqlite(pool) => go!(pool, |conn| {
            let _ = &conn;
            None
        }),
    }
}
