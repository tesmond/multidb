//! Editing a query result in place: which results can be edited, what the
//! table looks like, and saving queued edits, inserts and deletes in one
//! transaction.
//!
//! Nothing here knows about the UI. The UI hands [`plan::SaveRequest`] to
//! [`exec::save`] and gets back a [`exec::SaveOutcome`].
//!
//! - [`query`]: is this SELECT a plain read of one table, and which result
//!   column comes from which table column.
//! - [`meta`]: primary key, nullability, generated columns and types of a table.
//! - [`plan`]: the statements a save runs, in order, with their parameters.
//! - [`exec`]: runs a plan in a transaction: conflict check, writes, re-read.

pub mod exec;
pub mod meta;
pub mod plan;
pub mod query;

#[cfg(test)]
mod tests;

/// The database engines editing supports.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Driver {
    Postgres,
    MySql,
    Sqlite,
}

impl Driver {
    pub fn from_name(name: &str) -> Option<Driver> {
        match name {
            "postgres" => Some(Driver::Postgres),
            "mysql" => Some(Driver::MySql),
            "sqlite" => Some(Driver::Sqlite),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Driver::Postgres => "postgres",
            Driver::MySql => "mysql",
            Driver::Sqlite => "sqlite",
        }
    }

    /// An identifier, always quoted, with embedded quotes doubled.
    pub fn quote(self, name: &str) -> String {
        match self {
            Driver::MySql => format!("`{}`", name.replace('`', "``")),
            _ => format!("\"{}\"", name.replace('"', "\"\"")),
        }
    }

    /// Whether two column names are the same column. Postgres compares names
    /// exactly (the parser has already folded unquoted names); MySQL and SQLite
    /// column names are case-insensitive.
    pub fn same_name(self, a: &str, b: &str) -> bool {
        match self {
            Driver::Postgres => a == b,
            _ => a.eq_ignore_ascii_case(b),
        }
    }
}

/// The value a cell is to be given.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cell {
    /// The text typed into the cell; the database converts it to the column's type.
    Value(String),
    /// SQL NULL.
    Null,
    /// Left out of an INSERT, so the column default applies. Only meaningful
    /// for inserted rows.
    Default,
    /// The database's current date/time, as of the save (`NOW()` and its
    /// equivalents for the column's type). Only for date and time columns.
    Now,
}

/// What kind of date/time value a column holds, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Temporal {
    Date,
    Time,
    DateTime,
}

/// The date/time kind of a declared column type (see `ColumnMeta::data_type`).
pub fn temporal_kind(driver: Driver, data_type: &str) -> Option<Temporal> {
    let t = data_type.trim().to_ascii_lowercase();
    match driver {
        Driver::Postgres => {
            if t == "date" {
                Some(Temporal::Date)
            } else if t.starts_with("timestamp") {
                Some(Temporal::DateTime)
            } else if t.starts_with("time") {
                Some(Temporal::Time)
            } else {
                None
            }
        }
        Driver::MySql => {
            let base = t.split(['(', ' ']).next().unwrap_or("");
            match base {
                "date" => Some(Temporal::Date),
                "datetime" | "timestamp" => Some(Temporal::DateTime),
                "time" => Some(Temporal::Time),
                _ => None,
            }
        }
        // SQLite has no date types; go by the declared name, as its own
        // affinity rules do for other types.
        Driver::Sqlite => {
            if t.contains("datetime") || t.contains("timestamp") {
                Some(Temporal::DateTime)
            } else if t.contains("date") {
                Some(Temporal::Date)
            } else if t.contains("time") {
                Some(Temporal::Time)
            } else {
                None
            }
        }
    }
}

/// The SQL for "now" that suits a column of `data_type`, so a DATE gets a
/// date and a DATETIME(3) keeps its milliseconds. `None` for columns that
/// don't hold dates or times.
pub fn now_sql(driver: Driver, data_type: &str) -> Option<String> {
    let kind = temporal_kind(driver, data_type)?;
    // MySQL fractional seconds: DATETIME(3) → NOW(3).
    let fsp = || -> String {
        let t = data_type.trim();
        match (t.find('('), t.find(')')) {
            (Some(a), Some(b)) if b > a + 1 && t[a + 1..b].chars().all(|c| c.is_ascii_digit()) => t[a + 1..b].to_string(),
            _ => String::new(),
        }
    };
    Some(match (driver, kind) {
        (Driver::Postgres, Temporal::Date) => "CURRENT_DATE".into(),
        (Driver::Postgres, Temporal::Time) => {
            if data_type.to_ascii_lowercase().contains("with time zone") { "CURRENT_TIME".into() } else { "LOCALTIME".into() }
        }
        (Driver::Postgres, Temporal::DateTime) => "now()".into(),
        (Driver::MySql, Temporal::Date) => "CURDATE()".into(),
        (Driver::MySql, Temporal::Time) => format!("CURTIME({})", fsp()),
        (Driver::MySql, Temporal::DateTime) => format!("NOW({})", fsp()),
        (Driver::Sqlite, Temporal::Date) => "CURRENT_DATE".into(),
        (Driver::Sqlite, Temporal::Time) => "CURRENT_TIME".into(),
        (Driver::Sqlite, Temporal::DateTime) => "CURRENT_TIMESTAMP".into(),
    })
}

/// Which row of the grid something refers to: a row of the query result (by
/// its index in the result's data, not its position on screen), or a row
/// inserted in the grid and not yet saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RowRef {
    Data(usize),
    New(u64),
}

/// The exact text of a loaded value, for use as a parameter. Unlike the
/// grid's display text, integers are never rounded through an f64.
pub fn exact_text(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::Null => None,
        serde_json::Value::Bool(b) => Some(if *b { "true" } else { "false" }.to_string()),
        serde_json::Value::Number(n) => Some(n.to_string()),
        serde_json::Value::String(s) => Some(s.clone()),
        other => Some(other.to_string()),
    }
}

/// Text that means true or false for a boolean column, in the spellings
/// Postgres accepts. `None` when it is neither.
pub fn parse_bool(text: &str) -> Option<bool> {
    match text.trim().to_ascii_lowercase().as_str() {
        "t" | "true" | "y" | "yes" | "on" | "1" => Some(true),
        "f" | "false" | "n" | "no" | "off" | "0" => Some(false),
        _ => None,
    }
}
