//! The statements a save runs, built from the queued changes.
//!
//! Every value travels as a bound parameter, never spliced into SQL text.
//! Postgres parameters are sent as text and cast to the column's declared
//! type, which is how a value typed into the grid becomes an integer, a date,
//! an enum or an array. MySQL and SQLite convert text themselves; their
//! boolean columns get 1/0.
//!
//! Each [`Stmt`] also carries a display form with the values written out as
//! literals, which is what "Open as SQL" shows.

use super::meta::ColumnMeta;
use super::{exact_text, parse_bool, Cell, Driver, RowRef};
use serde_json::Value;

/// Rows per DELETE and per locking SELECT.
pub const BATCH: usize = 500;

#[derive(Clone, Debug, PartialEq)]
pub enum Bind {
    Null,
    Text(String),
    Int(i64),
    UInt(u64),
    Float(f64),
}

#[derive(Clone, Debug, PartialEq)]
pub struct Stmt {
    pub sql: String,
    pub binds: Vec<Bind>,
    /// The statement with its values written in, for people to read.
    pub display: String,
}

/// The table being saved to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Target {
    pub driver: Driver,
    /// Empty for none (SQLite's main database).
    pub schema: String,
    pub table: String,
}

impl Target {
    pub fn qualified(&self) -> String {
        if self.schema.is_empty() {
            self.driver.quote(&self.table)
        } else {
            format!("{}.{}", self.driver.quote(&self.schema), self.driver.quote(&self.table))
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowUpdate {
    pub row: RowRef,
    /// The row as loaded, in result-column order.
    pub original: Vec<Value>,
    /// (result column, new value)
    pub sets: Vec<(usize, Cell)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowDelete {
    pub row: RowRef,
    pub original: Vec<Value>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RowInsert {
    pub row: RowRef,
    /// (result column, value); columns left at `Default` may be omitted.
    pub cells: Vec<(usize, Cell)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SaveRequest {
    pub target: Target,
    /// Per result column, the table column it shows (`None`: an expression).
    pub columns: Vec<Option<ColumnMeta>>,
    /// The result column of each primary key column, in key order.
    pub key: Vec<usize>,
    pub supports_returning: bool,
    pub updates: Vec<RowUpdate>,
    pub deletes: Vec<RowDelete>,
    pub inserts: Vec<RowInsert>,
    /// Compare the rows against what was loaded before writing.
    pub check_conflicts: bool,
    pub lock_timeout_secs: u32,
}

/// Builds SQL text and its parameters side by side with the readable form.
struct Buf {
    driver: Driver,
    sql: String,
    display: String,
    binds: Vec<Bind>,
}

impl Buf {
    fn new(driver: Driver) -> Self {
        Buf { driver, sql: String::new(), display: String::new(), binds: Vec::new() }
    }

    fn push(&mut self, s: &str) {
        self.sql.push_str(s);
        self.display.push_str(s);
    }

    /// A parameter, cast to `cast` on Postgres.
    fn param(&mut self, bind: Bind, cast: &str) {
        let literal = literal(self.driver, &bind);
        self.binds.push(bind);
        match self.driver {
            Driver::Postgres => {
                let n = self.binds.len();
                if cast.is_empty() {
                    self.sql.push_str(&format!("${n}"));
                    self.display.push_str(&literal);
                } else {
                    self.sql.push_str(&format!("CAST(${n} AS {cast})"));
                    self.display.push_str(&format!("CAST({literal} AS {cast})"));
                }
            }
            _ => {
                self.sql.push('?');
                self.display.push_str(&literal);
            }
        }
    }

    fn finish(self) -> Stmt {
        Stmt { sql: self.sql, binds: self.binds, display: self.display }
    }
}

/// How a bound value reads in SQL text.
pub fn literal(driver: Driver, bind: &Bind) -> String {
    match bind {
        Bind::Null => "NULL".into(),
        Bind::Int(i) => i.to_string(),
        Bind::UInt(u) => u.to_string(),
        Bind::Float(f) => {
            let s = f.to_string();
            if s.contains(['.', 'e', 'E']) || !f.is_finite() { s } else { format!("{s}.0") }
        }
        Bind::Text(t) => {
            let mut escaped = t.replace('\'', "''");
            if driver == Driver::MySql {
                // MySQL reads backslashes in string literals as escapes.
                escaped = escaped.replace('\\', "\\\\");
            }
            format!("'{escaped}'")
        }
    }
}

/// The parameter for a value typed into a cell of `col`.
pub fn bind_text(driver: Driver, col: &ColumnMeta, text: &str) -> Bind {
    if driver != Driver::Postgres && col.is_bool {
        if let Some(b) = parse_bool(text) {
            return Bind::Int(b as i64);
        }
    }
    Bind::Text(text.to_string())
}

pub fn bind_cell(driver: Driver, col: &ColumnMeta, cell: &Cell) -> Bind {
    match cell {
        Cell::Value(text) => bind_text(driver, col, text),
        Cell::Null | Cell::Default | Cell::Now => Bind::Null,
    }
}

/// The parameter for a value as it was loaded (to find its row again).
pub fn bind_loaded(driver: Driver, value: &Value) -> Bind {
    match (driver, value) {
        (_, Value::Null) => Bind::Null,
        (Driver::Postgres, v) => Bind::Text(exact_text(v).unwrap_or_default()),
        (_, Value::Bool(b)) => Bind::Int(*b as i64),
        (_, Value::Number(n)) => {
            if let Some(i) = n.as_i64() {
                Bind::Int(i)
            } else if let Some(u) = n.as_u64() {
                Bind::UInt(u)
            } else {
                Bind::Float(n.as_f64().unwrap_or(0.0))
            }
        }
        (_, v) => Bind::Text(exact_text(v).unwrap_or_default()),
    }
}

impl SaveRequest {
    pub fn driver(&self) -> Driver {
        self.target.driver
    }

    fn col(&self, result_col: usize) -> &ColumnMeta {
        self.columns[result_col].as_ref().expect("an edited or key column has a table column")
    }

    /// "Now" for a result column (validated to be a date/time column).
    fn now(&self, result_col: usize) -> String {
        super::now_sql(self.driver(), &self.col(result_col).data_type).unwrap_or_else(|| "CURRENT_TIMESTAMP".into())
    }

    fn cast(&self, result_col: usize) -> String {
        if self.driver() == Driver::Postgres {
            self.col(result_col).data_type.clone()
        } else {
            String::new()
        }
    }

    /// The table columns read back after a write, in result order, one per
    /// table column, with the result columns each one fills.
    pub fn read_columns(&self) -> Vec<(String, Vec<usize>)> {
        let mut out: Vec<(String, Vec<usize>)> = Vec::new();
        for (i, c) in self.columns.iter().enumerate() {
            let Some(c) = c else { continue };
            match out.iter_mut().find(|(name, _)| *name == c.name) {
                Some((_, cols)) => cols.push(i),
                None => out.push((c.name.clone(), vec![i])),
            }
        }
        out
    }

    fn select_list(&self) -> String {
        let d = self.driver();
        self.read_columns().iter().map(|(name, _)| d.quote(name)).collect::<Vec<_>>().join(", ")
    }

    /// `WHERE` for rows identified by their loaded key values.
    fn key_predicate(&self, buf: &mut Buf, rows: &[&[Value]]) {
        let d = self.driver();
        if self.key.len() == 1 {
            let k = self.key[0];
            buf.push(&d.quote(&self.col(k).name));
            buf.push(" IN (");
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    buf.push(", ");
                }
                buf.param(bind_loaded(d, &row[k]), &self.cast(k));
            }
            buf.push(")");
        } else {
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    buf.push(" OR ");
                }
                buf.push("(");
                self.key_equals(buf, |k| bind_loaded(d, &row[k]));
                buf.push(")");
            }
        }
    }

    /// `k1 = ? AND k2 = ?` with each key column's parameter from `value`.
    fn key_equals(&self, buf: &mut Buf, value: impl Fn(usize) -> Bind) {
        let d = self.driver();
        for (j, &k) in self.key.iter().enumerate() {
            if j > 0 {
                buf.push(" AND ");
            }
            buf.push(&d.quote(&self.col(k).name));
            buf.push(" = ");
            buf.param(value(k), &self.cast(k));
        }
    }

    /// Read (and on Postgres and MySQL lock) the rows, as they are now.
    pub fn lock_select(&self, rows: &[&[Value]]) -> Stmt {
        let mut buf = Buf::new(self.driver());
        buf.push(&format!("SELECT {} FROM {} WHERE ", self.select_list(), self.target.qualified()));
        self.key_predicate(&mut buf, rows);
        if self.driver() != Driver::Sqlite {
            buf.push(" FOR UPDATE");
        }
        buf.finish()
    }

    /// The rows by loaded key, without locking (to check what still exists).
    pub fn select_by_loaded_key(&self, rows: &[&[Value]]) -> Stmt {
        let mut buf = Buf::new(self.driver());
        buf.push(&format!("SELECT {} FROM {} WHERE ", self.select_list(), self.target.qualified()));
        self.key_predicate(&mut buf, rows);
        buf.finish()
    }

    pub fn delete(&self, rows: &[&[Value]]) -> Stmt {
        let mut buf = Buf::new(self.driver());
        buf.push(&format!("DELETE FROM {} WHERE ", self.target.qualified()));
        self.key_predicate(&mut buf, rows);
        buf.finish()
    }

    pub fn returning(&self) -> bool {
        self.supports_returning && self.driver() != Driver::MySql
    }

    pub fn update(&self, u: &RowUpdate) -> Stmt {
        let d = self.driver();
        let mut buf = Buf::new(d);
        buf.push(&format!("UPDATE {} SET ", self.target.qualified()));
        for (i, (col, cell)) in u.sets.iter().enumerate() {
            if i > 0 {
                buf.push(", ");
            }
            let meta = self.col(*col);
            buf.push(&d.quote(&meta.name));
            buf.push(" = ");
            match cell {
                Cell::Default => buf.push("DEFAULT"),
                Cell::Null => buf.push("NULL"),
                Cell::Now => buf.push(&self.now(*col)),
                Cell::Value(_) => buf.param(bind_cell(d, meta, cell), &self.cast(*col)),
            }
        }
        buf.push(" WHERE ");
        self.key_equals(&mut buf, |k| bind_loaded(d, &u.original[k]));
        if self.returning() {
            buf.push(&format!(" RETURNING {}", self.select_list()));
        }
        buf.finish()
    }

    /// The cells an insert writes: everything not left at its default, one
    /// per table column.
    pub fn insert_cells<'a>(&'a self, ins: &'a RowInsert) -> Vec<(usize, &'a Cell)> {
        let mut out: Vec<(usize, &Cell)> = Vec::new();
        for (col, cell) in &ins.cells {
            if matches!(cell, Cell::Default) {
                continue;
            }
            let Some(meta) = self.columns.get(*col).and_then(|c| c.as_ref()) else { continue };
            if meta.generated || out.iter().any(|(c, _)| self.col(*c).name == meta.name) {
                continue;
            }
            out.push((*col, cell));
        }
        out
    }

    pub fn insert(&self, ins: &RowInsert) -> Stmt {
        let d = self.driver();
        let cells = self.insert_cells(ins);
        let mut buf = Buf::new(d);
        buf.push(&format!("INSERT INTO {} ", self.target.qualified()));
        if cells.is_empty() {
            buf.push(if d == Driver::MySql { "() VALUES ()" } else { "DEFAULT VALUES" });
        } else {
            let names: Vec<String> = cells.iter().map(|(c, _)| d.quote(&self.col(*c).name)).collect();
            buf.push(&format!("({}) VALUES (", names.join(", ")));
            for (i, (col, cell)) in cells.iter().enumerate() {
                if i > 0 {
                    buf.push(", ");
                }
                match cell {
                    Cell::Null => buf.push("NULL"),
                    Cell::Now => buf.push(&self.now(*col)),
                    _ => buf.param(bind_cell(d, self.col(*col), cell), &self.cast(*col)),
                }
            }
            buf.push(")");
        }
        if self.returning() {
            buf.push(&format!(" RETURNING {}", self.select_list()));
        }
        buf.finish()
    }

    /// A row found by the key values it was written with (MySQL, and SQLite
    /// without RETURNING). `auto` supplies the generated key value.
    pub fn select_written(&self, key_values: &[(usize, Bind)]) -> Stmt {
        let mut buf = Buf::new(self.driver());
        buf.push(&format!("SELECT {} FROM {} WHERE ", self.select_list(), self.target.qualified()));
        self.key_equals(&mut buf, |k| key_values.iter().find(|(c, _)| *c == k).map(|(_, b)| b.clone()).unwrap_or(Bind::Null));
        buf.finish()
    }

    /// SQLite: the row just inserted, by rowid.
    pub fn select_rowid(&self, rowid: i64) -> Stmt {
        let mut buf = Buf::new(self.driver());
        buf.push(&format!("SELECT {} FROM {} WHERE rowid = ", self.select_list(), self.target.qualified()));
        buf.param(Bind::Int(rowid), "");
        buf.finish()
    }

    /// The key an updated row has after its update: edited key cells take
    /// their new text, the rest keep their loaded values.
    pub fn key_after_update(&self, u: &RowUpdate) -> Vec<(usize, Bind)> {
        let d = self.driver();
        self.key
            .iter()
            .map(|&k| match u.sets.iter().find(|(c, _)| *c == k) {
                Some((_, cell)) => (k, bind_cell(d, self.col(k), cell)),
                None => (k, bind_loaded(d, &u.original[k])),
            })
            .collect()
    }

    /// Updates for rows that are also being deleted are dropped: the delete wins.
    pub fn effective_updates(&self) -> Vec<&RowUpdate> {
        self.updates
            .iter()
            .filter(|u| !u.sets.is_empty() && !self.deletes.iter().any(|d| d.row == u.row))
            .collect()
    }

    /// Problems that would stop a save before it starts.
    pub fn validate(&self) -> Result<(), (Option<RowRef>, String)> {
        if self.key.is_empty() {
            return Err((None, "the table has no primary key".into()));
        }
        for u in &self.updates {
            for (col, cell) in &u.sets {
                let Some(meta) = self.columns.get(*col).and_then(|c| c.as_ref()) else {
                    return Err((Some(u.row), "an edited column is not a table column".into()));
                };
                if meta.generated {
                    return Err((Some(u.row), format!("\"{}\" is a generated column", meta.name)));
                }
                if matches!(cell, Cell::Default) {
                    return Err((Some(u.row), "DEFAULT can only be used in a new row".into()));
                }
                self.check_now(*col, cell).map_err(|e| (Some(u.row), e))?;
            }
            if u.original.len() != self.columns.len() {
                return Err((Some(u.row), "the row does not match the result's columns".into()));
            }
        }
        for del in &self.deletes {
            if del.original.len() != self.columns.len() {
                return Err((Some(del.row), "the row does not match the result's columns".into()));
            }
        }
        for ins in &self.inserts {
            for (col, cell) in &ins.cells {
                self.check_now(*col, cell).map_err(|e| (Some(ins.row), e))?;
            }
        }
        if !self.returning() {
            for ins in &self.inserts {
                self.insert_lookup(ins).map_err(|e| (Some(ins.row), e))?;
            }
        }
        Ok(())
    }

    /// NOW() only goes in date/time columns, and never in a key column: the
    /// row is found again by its key, which must be known before the save.
    fn check_now(&self, col: usize, cell: &Cell) -> Result<(), String> {
        if !matches!(cell, Cell::Now) {
            return Ok(());
        }
        let Some(meta) = self.columns.get(col).and_then(|c| c.as_ref()) else {
            return Err("NOW() needs a table column".into());
        };
        if super::now_sql(self.driver(), &meta.data_type).is_none() {
            return Err(format!("\"{}\" is not a date or time column", meta.name));
        }
        if self.key.contains(&col) {
            return Err(format!("NOW() can't be used for the key column \"{}\"", meta.name));
        }
        Ok(())
    }

    /// How a row will be found again after it is inserted, where RETURNING
    /// isn't available.
    pub fn insert_lookup(&self, ins: &RowInsert) -> Result<InsertLookup, String> {
        let d = self.driver();
        let cells = self.insert_cells(ins);
        let mut entered = Vec::new();
        let mut auto = None;
        for &k in &self.key {
            let meta = self.col(k);
            match cells.iter().find(|(c, _)| self.col(*c).name == meta.name) {
                Some((_, Cell::Value(text))) => entered.push((k, bind_text(d, meta, text))),
                Some(_) => return Err(format!("the key column \"{}\" can't be NULL", meta.name)),
                None if meta.auto_increment && auto.is_none() => auto = Some(k),
                None => {
                    if d == Driver::Sqlite && self.key.len() == 1 {
                        // A rowid table finds its new row by rowid.
                        return Ok(InsertLookup::Rowid);
                    }
                    return Err(format!(
                        "enter a value for the key column \"{}\" so the new row can be found after it is saved",
                        meta.name
                    ));
                }
            }
        }
        Ok(match auto {
            Some(col) if d == Driver::Sqlite => {
                let _ = col;
                InsertLookup::Rowid
            }
            Some(col) => InsertLookup::Key { entered, auto: Some(col) },
            None => InsertLookup::Key { entered, auto: None },
        })
    }

    /// Everything the save would run, as readable SQL (for "Open as SQL").
    pub fn script(&self) -> String {
        let mut out = Vec::new();
        let deletes: Vec<&[Value]> = self.deletes.iter().map(|d| d.original.as_slice()).collect();
        for chunk in deletes.chunks(BATCH) {
            out.push(self.delete(chunk).display);
        }
        for u in self.effective_updates() {
            out.push(self.update(u).display);
        }
        for ins in &self.inserts {
            out.push(self.insert(ins).display);
        }
        out.into_iter().map(|s| format!("{s};")).collect::<Vec<_>>().join("\n\n")
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum InsertLookup {
    /// By key: the values entered, plus the auto-increment column whose value
    /// the database reports.
    Key { entered: Vec<(usize, Bind)>, auto: Option<usize> },
    /// SQLite: by the rowid of the insert.
    Rowid,
}
