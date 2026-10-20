//! What editing needs to know about a table: its columns (type, nullability,
//! whether they can be written) and its primary key.

use super::Driver;
use anyhow::{anyhow, Result};
use sqlx::{MySqlPool, PgPool, Row, SqlitePool};

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct ColumnMeta {
    pub name: String,
    /// The declared type: Postgres `format_type` (what values are cast to),
    /// MySQL `COLUMN_TYPE`, SQLite's declared type.
    pub data_type: String,
    pub nullable: bool,
    /// Computed by the database (generated columns, Postgres `GENERATED
    /// ALWAYS AS IDENTITY`): never written, left to its default on insert.
    pub generated: bool,
    /// Not part of `SELECT *` (MySQL invisible columns, SQLite hidden columns).
    pub hidden: bool,
    /// Filled in by the database when an insert leaves it out (MySQL
    /// `AUTO_INCREMENT`, SQLite's rowid alias).
    pub auto_increment: bool,
    pub has_default: bool,
    pub is_bool: bool,
    /// Binary or spatial: what the grid shows can't be typed back.
    pub binary: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct TableMeta {
    /// The schema (Postgres), database (MySQL) or attached database (SQLite)
    /// the table was found in; empty only for SQLite's main database.
    pub schema: String,
    pub table: String,
    pub columns: Vec<ColumnMeta>,
    /// Primary key column names, in key order.
    pub primary_key: Vec<String>,
    /// The storage engine, where there is a choice (MySQL).
    pub engine: String,
    /// Whether changes can be rolled back. False for MySQL MyISAM and friends.
    pub transactional: bool,
    /// `INSERT … RETURNING` / `UPDATE … RETURNING` are available.
    pub supports_returning: bool,
}

impl TableMeta {
    pub fn column(&self, driver: Driver, name: &str) -> Option<usize> {
        self.columns.iter().position(|c| driver.same_name(&c.name, name))
    }
}

/// Types whose grid text is not a value you can type back in.
pub fn is_binary_type(driver: Driver, data_type: &str) -> bool {
    let t = data_type.to_ascii_lowercase();
    let base = t.split(['(', ' ']).next().unwrap_or("");
    match driver {
        Driver::Postgres => matches!(base, "bytea" | "geometry" | "geography" | "bit" | "varbit") || t.starts_with("bit varying"),
        Driver::MySql => matches!(
            base,
            "binary"
                | "varbinary"
                | "tinyblob"
                | "blob"
                | "mediumblob"
                | "longblob"
                | "bit"
                | "geometry"
                | "point"
                | "linestring"
                | "polygon"
                | "multipoint"
                | "multilinestring"
                | "multipolygon"
                | "geometrycollection"
                | "geomcollection"
        ),
        Driver::Sqlite => t.contains("blob"),
    }
}

/// MySQL engines that support transactions.
pub fn mysql_engine_is_transactional(engine: &str) -> bool {
    matches!(engine.to_ascii_lowercase().as_str(), "innodb" | "ndb" | "ndbcluster" | "tokudb" | "rocksdb")
}

// ─── Postgres ───────────────────────────────────────────────────────────────

/// Candidates, in order: `schema.table` when a schema is known, then the bare
/// name resolved through the search path.
pub async fn postgres(pool: &PgPool, schema: &str, table: &str) -> Result<TableMeta> {
    let d = Driver::Postgres;
    let mut candidates = Vec::new();
    if !schema.is_empty() {
        candidates.push(format!("{}.{}", d.quote(schema), d.quote(table)));
    }
    candidates.push(d.quote(table));
    for regclass in candidates {
        let found = sqlx::query(
            "SELECT n.nspname::text, c.relname::text, c.relkind::text
             FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
             WHERE c.oid = to_regclass($1)",
        )
        .bind(&regclass)
        .fetch_optional(pool)
        .await?;
        let Some(found) = found else { continue };
        let (nsp, rel, kind): (String, String, String) = (found.try_get(0)?, found.try_get(1)?, found.try_get(2)?);
        if !matches!(kind.as_str(), "r" | "p") {
            return Err(anyhow!("\"{rel}\" is not a table"));
        }
        // attidentity arrived in Postgres 10, attgenerated in 12.
        let version: i32 = sqlx::query_scalar::<_, String>("SELECT current_setting('server_version_num')")
            .fetch_one(pool)
            .await?
            .parse()
            .unwrap_or(0);
        let generated_expr = if version >= 120000 { "a.attgenerated <> ''" } else { "false" };
        let identity_expr = if version >= 100000 { "a.attidentity::text" } else { "''" };
        let rows = sqlx::query(&format!(
            "SELECT a.attname::text,
                    format_type(a.atttypid, a.atttypmod),
                    NOT a.attnotnull,
                    {generated_expr},
                    {identity_expr},
                    a.atthasdef,
                    t.typname::text,
                    COALESCE(bt.typname::text, '')
             FROM pg_attribute a
             JOIN pg_type t ON t.oid = a.atttypid
             LEFT JOIN pg_type bt ON bt.oid = t.typbasetype
             WHERE a.attrelid = to_regclass($1) AND a.attnum > 0 AND NOT a.attisdropped
             ORDER BY a.attnum"
        ))
        .bind(&regclass)
        .fetch_all(pool)
        .await?;
        let mut columns = Vec::with_capacity(rows.len());
        for row in rows {
            let name: String = row.try_get(0)?;
            let data_type: String = row.try_get(1)?;
            let nullable: bool = row.try_get(2)?;
            let generated: bool = row.try_get(3)?;
            let identity: String = row.try_get(4)?;
            let has_default: bool = row.try_get(5)?;
            let typname: String = row.try_get(6)?;
            let base: String = row.try_get(7)?;
            let effective = if base.is_empty() { typname.clone() } else { base };
            columns.push(ColumnMeta {
                is_bool: effective == "bool",
                binary: is_binary_type(d, &effective) || is_binary_type(d, &data_type),
                name,
                data_type,
                nullable,
                // Identity ALWAYS refuses explicit values.
                generated: generated || identity == "a",
                hidden: false,
                auto_increment: !identity.is_empty(),
                has_default: has_default || !identity.is_empty() || generated,
            });
        }
        let primary_key: Vec<String> = sqlx::query_scalar(
            "SELECT a.attname::text
             FROM pg_index i
             JOIN pg_attribute a ON a.attrelid = i.indrelid AND a.attnum = ANY(i.indkey)
             WHERE i.indrelid = to_regclass($1) AND i.indisprimary
             ORDER BY array_position(i.indkey::int2[], a.attnum)",
        )
        .bind(&regclass)
        .fetch_all(pool)
        .await?;
        return Ok(TableMeta {
            schema: nsp,
            table: rel,
            columns,
            primary_key,
            engine: String::new(),
            transactional: true,
            supports_returning: true,
        });
    }
    Err(anyhow!("table \"{table}\" not found"))
}

// ─── MySQL ──────────────────────────────────────────────────────────────────

pub async fn mysql(pool: &MySqlPool, schema: &str, table: &str) -> Result<TableMeta> {
    let d = Driver::MySql;
    let schema = if schema.is_empty() {
        let row = sqlx::query("SELECT DATABASE()").fetch_one(pool).await?;
        let db: Option<String> = row.try_get_unchecked(0).ok().flatten();
        db.ok_or_else(|| anyhow!("no database selected"))?
    } else {
        schema.to_string()
    };
    let text = |row: &sqlx::mysql::MySqlRow, i: usize| -> String {
        if let Ok(Some(v)) = row.try_get::<Option<String>, _>(i) {
            return v;
        }
        if let Ok(Some(v)) = row.try_get::<Option<Vec<u8>>, _>(i) {
            return String::from_utf8_lossy(&v).into_owned();
        }
        row.try_get_unchecked::<Option<String>, _>(i).ok().flatten().unwrap_or_default()
    };
    let rows = sqlx::query(
        "SELECT COLUMN_NAME, COLUMN_TYPE, DATA_TYPE, IS_NULLABLE, EXTRA,
                CASE WHEN COLUMN_DEFAULT IS NULL THEN 'N' ELSE 'Y' END
         FROM information_schema.COLUMNS
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?
         ORDER BY ORDINAL_POSITION",
    )
    .bind(&schema)
    .bind(table)
    .fetch_all(pool)
    .await?;
    if rows.is_empty() {
        return Err(anyhow!("table `{table}` not found"));
    }
    let mut columns = Vec::with_capacity(rows.len());
    for row in &rows {
        let column_type = text(row, 1);
        let data_type = text(row, 2).to_ascii_lowercase();
        let extra = text(row, 4).to_ascii_lowercase();
        let generated = extra.contains("virtual generated") || extra.contains("stored generated");
        let nullable = text(row, 3).eq_ignore_ascii_case("YES");
        columns.push(ColumnMeta {
            name: text(row, 0),
            is_bool: data_type == "tinyint" && column_type.to_ascii_lowercase().starts_with("tinyint(1)"),
            binary: is_binary_type(d, &data_type),
            data_type: column_type,
            nullable,
            generated,
            hidden: extra.contains("invisible"),
            auto_increment: extra.contains("auto_increment"),
            has_default: text(row, 5) == "Y" || nullable || generated || extra.contains("auto_increment"),
        });
    }
    let key_rows = sqlx::query(
        "SELECT COLUMN_NAME FROM information_schema.STATISTICS
         WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ? AND INDEX_NAME = 'PRIMARY'
         ORDER BY SEQ_IN_INDEX",
    )
    .bind(&schema)
    .bind(table)
    .fetch_all(pool)
    .await?;
    let primary_key = key_rows.iter().map(|r| text(r, 0)).collect();
    let engine_row = sqlx::query("SELECT ENGINE, TABLE_TYPE FROM information_schema.TABLES WHERE TABLE_SCHEMA = ? AND TABLE_NAME = ?")
        .bind(&schema)
        .bind(table)
        .fetch_optional(pool)
        .await?;
    let (engine, table_type) = engine_row.map(|r| (text(&r, 0), text(&r, 1))).unwrap_or_default();
    if table_type.eq_ignore_ascii_case("VIEW") {
        return Err(anyhow!("`{table}` is a view"));
    }
    Ok(TableMeta {
        schema,
        table: table.to_string(),
        transactional: mysql_engine_is_transactional(&engine),
        engine,
        columns,
        primary_key,
        supports_returning: false,
    })
}

// ─── SQLite ─────────────────────────────────────────────────────────────────

pub async fn sqlite(pool: &SqlitePool, schema: &str, table: &str) -> Result<TableMeta> {
    let d = Driver::Sqlite;
    let prefix = if schema.is_empty() { String::new() } else { format!("{}.", d.quote(schema)) };
    let quoted = d.quote(table);
    // table_xinfo (3.26+) also reports generated and hidden columns.
    let (rows, xinfo) = match sqlx::query(&format!("PRAGMA {prefix}table_xinfo({quoted})")).fetch_all(pool).await {
        Ok(rows) => (rows, true),
        Err(_) => (sqlx::query(&format!("PRAGMA {prefix}table_info({quoted})")).fetch_all(pool).await?, false),
    };
    if rows.is_empty() {
        return Err(anyhow!("table \"{table}\" not found"));
    }
    let master = if schema.is_empty() { "sqlite_master".to_string() } else { format!("{prefix}sqlite_master") };
    let create_sql: Option<String> = sqlx::query_scalar(&format!("SELECT sql FROM {master} WHERE type = 'table' AND name = ? COLLATE NOCASE"))
        .bind(table)
        .fetch_optional(pool)
        .await?
        .flatten();
    let Some(create_sql) = create_sql else {
        return Err(anyhow!("\"{table}\" is not a table"));
    };
    let without_rowid = {
        let upper = create_sql.to_ascii_uppercase();
        let compact: String = upper.split_whitespace().collect::<Vec<_>>().join(" ");
        compact.contains("WITHOUT ROWID")
    };
    let mut keyed: Vec<(i64, String)> = Vec::new();
    let mut columns = Vec::with_capacity(rows.len());
    for row in &rows {
        let name: String = row.try_get("name")?;
        let data_type: String = row.try_get::<Option<String>, _>("type")?.unwrap_or_default();
        let notnull: i64 = row.try_get("notnull")?;
        let default: Option<String> = row.try_get("dflt_value")?;
        let pk: i64 = row.try_get("pk")?;
        let hidden: i64 = if xinfo { row.try_get("hidden")? } else { 0 };
        if pk > 0 {
            keyed.push((pk, name.clone()));
        }
        let upper = data_type.to_ascii_uppercase();
        columns.push(ColumnMeta {
            name,
            is_bool: upper.contains("BOOL"),
            binary: is_binary_type(d, &data_type),
            nullable: notnull == 0 && pk == 0,
            generated: hidden == 2 || hidden == 3,
            hidden: hidden == 1,
            auto_increment: false,
            has_default: default.is_some() || notnull == 0 || hidden == 2 || hidden == 3,
            data_type,
        });
    }
    keyed.sort_by_key(|(n, _)| *n);
    let primary_key: Vec<String> = keyed.into_iter().map(|(_, n)| n).collect();
    // A single INTEGER PRIMARY KEY is the rowid: the database fills it in.
    if primary_key.len() == 1 && !without_rowid {
        if let Some(col) = columns.iter_mut().find(|c| c.name == primary_key[0]) {
            if col.data_type.eq_ignore_ascii_case("INTEGER") {
                col.auto_increment = true;
                col.has_default = true;
                col.nullable = false;
            }
        }
    }
    let version: String = sqlx::query_scalar("SELECT sqlite_version()").fetch_one(pool).await?;
    Ok(TableMeta {
        schema: schema.to_string(),
        table: table.to_string(),
        columns,
        primary_key,
        engine: String::new(),
        transactional: true,
        supports_returning: sqlite_version_at_least(&version, (3, 35, 0)),
    })
}

pub fn sqlite_version_at_least(version: &str, min: (u32, u32, u32)) -> bool {
    let mut parts = version.trim().split('.').map(|p| p.parse::<u32>().unwrap_or(0));
    let v = (parts.next().unwrap_or(0), parts.next().unwrap_or(0), parts.next().unwrap_or(0));
    v >= min
}
