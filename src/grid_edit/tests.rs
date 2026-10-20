//! Tests for result editing. Test ids (E01, X03, …) refer to the cases in
//! docs/grid-editing-spec.md.
//!
//! The integration tests run against SQLite always, and against Postgres and
//! MySQL when `MULTIDB_TEST_PG` / `MULTIDB_TEST_MYSQL` hold connection URLs
//! (`make test-db` starts both and sets them).

use super::exec::{self, Action, SaveControl, SaveOutcome, SaveReport, Source};
use super::meta::{self, ColumnMeta, TableMeta};
use super::plan::{self, Bind, RowDelete, RowInsert, RowUpdate, SaveRequest, Target};
use super::query::{self, Item};
use super::{Cell, Driver, RowRef};
use serde_json::{json, Value};
use std::sync::atomic::{AtomicU64, Ordering};

// ─── Editability (unit) ─────────────────────────────────────────────────────

fn col(name: &str) -> ColumnMeta {
    ColumnMeta { name: name.into(), data_type: "integer".into(), nullable: true, ..Default::default() }
}

fn table(cols: &[&str], pk: &[&str]) -> TableMeta {
    TableMeta {
        schema: "public".into(),
        table: "t".into(),
        columns: cols.iter().map(|c| col(c)).collect(),
        primary_key: pk.iter().map(|s| s.to_string()).collect(),
        transactional: true,
        supports_returning: true,
        ..Default::default()
    }
}

fn names(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

fn editable(driver: Driver, sql: &str, result: &[&str], meta: &TableMeta) -> Result<query::Layout, String> {
    let parsed = query::parse(sql, driver)?;
    query::layout(driver, &parsed, &names(result), meta)
}

#[test]
fn e01_select_star_with_a_key_is_editable() {
    let m = table(&["id", "name"], &["id"]);
    let l = editable(Driver::Postgres, "SELECT * FROM t", &["id", "name"], &m).unwrap();
    assert_eq!(l.key, vec![0]);
    assert_eq!(l.sources, vec![Some(0), Some(1)]);
    assert_eq!(l.readonly, vec![None, None]);
}

#[test]
fn e02_where_order_limit_are_fine() {
    let m = table(&["id", "name", "x"], &["id"]);
    for sql in [
        "SELECT id, name FROM t WHERE x = 1 ORDER BY name LIMIT 10",
        "select id, name from t where x in (select 1) order by 2 desc limit 10 offset 5;",
        "SELECT t.id, t.name FROM t",
        "SELECT a.id, a.name FROM t AS a",
        "SELECT a.id, a.name FROM t a",
    ] {
        let l = editable(Driver::Postgres, sql, &["id", "name"], &m).unwrap_or_else(|e| panic!("{sql}: {e}"));
        assert_eq!(l.key, vec![0], "{sql}");
    }
    editable(Driver::MySql, "SELECT id, name FROM t LIMIT 5, 10", &["id", "name"], &m).unwrap();
}

#[test]
fn e03_qualified_and_quoted_names() {
    let p = query::parse("SELECT * FROM \"public\".\"Users\"", Driver::Postgres).unwrap();
    assert_eq!((p.schema.as_str(), p.table.as_str()), ("public", "Users"));
    let p = query::parse("SELECT * FROM public.Users", Driver::Postgres).unwrap();
    assert_eq!(p.table, "users", "Postgres folds unquoted names");
    let p = query::parse("SELECT * FROM `shop`.`order`", Driver::MySql).unwrap();
    assert_eq!((p.schema.as_str(), p.table.as_str()), ("shop", "order"));
    let p = query::parse("SELECT * FROM Orders", Driver::MySql).unwrap();
    assert_eq!(p.table, "Orders");
    let p = query::parse("SELECT * FROM main.\"my table\"", Driver::Sqlite).unwrap();
    assert_eq!((p.schema.as_str(), p.table.as_str()), ("main", "my table"));
    let p = query::parse("SELECT [id] FROM [t]", Driver::Sqlite).unwrap();
    assert_eq!(p.items, vec![Item::Column { name: "id".into(), alias: None }]);
}

#[test]
fn e04_no_primary_key_is_read_only() {
    let m = table(&["id", "name"], &[]);
    let err = editable(Driver::Postgres, "SELECT * FROM t", &["id", "name"], &m).unwrap_err();
    assert!(err.contains("no primary key"), "{err}");
}

#[test]
fn e05_composite_key_needs_every_part() {
    let m = table(&["a", "b", "v"], &["a", "b"]);
    let l = editable(Driver::Postgres, "SELECT b, v, a FROM t", &["b", "v", "a"], &m).unwrap();
    assert_eq!(l.key, vec![2, 0], "key order follows the primary key, not the SELECT list");
    let err = editable(Driver::Postgres, "SELECT a, v FROM t", &["a", "v"], &m).unwrap_err();
    assert!(err.contains("\"b\""), "{err}");
}

#[test]
fn e06_key_not_selected_is_read_only() {
    let m = table(&["id", "name"], &["id"]);
    assert!(editable(Driver::Postgres, "SELECT name FROM t", &["name"], &m).is_err());
}

#[test]
fn e07_alias_that_shadows_the_key_is_read_only() {
    let m = table(&["id", "name"], &["id"]);
    let err = editable(Driver::Postgres, "SELECT name AS id FROM t", &["id"], &m).unwrap_err();
    assert!(err.contains("hides"), "{err}");
    let err = editable(Driver::Postgres, "SELECT id, name AS id FROM t", &["id", "id"], &m).unwrap_err();
    assert!(err.contains("hides"), "{err}");
}

#[test]
fn e08_alias_that_shadows_another_column_is_read_only() {
    let m = table(&["id", "name", "email"], &["id"]);
    assert!(editable(Driver::Postgres, "SELECT id, name AS email FROM t", &["id", "email"], &m).is_err());
    // MySQL and SQLite compare column names case-insensitively.
    assert!(editable(Driver::MySql, "SELECT id, name AS EMAIL FROM t", &["id", "EMAIL"], &m).is_err());
    // An alias naming no column is fine.
    let l = editable(Driver::Postgres, "SELECT id, name AS full_name FROM t", &["id", "full_name"], &m).unwrap();
    assert_eq!(l.sources, vec![Some(0), Some(1)]);
    // Aliasing a column to its own name is fine.
    editable(Driver::Postgres, "SELECT id AS id, name AS name FROM t", &["id", "name"], &m).unwrap();
}

#[test]
fn e09_expression_named_like_a_column_is_read_only_but_the_grid_is_editable() {
    let m = table(&["id", "price"], &["id"]);
    let l = editable(Driver::Postgres, "SELECT id, price * 2 AS price FROM t", &["id", "price"], &m).unwrap();
    assert_eq!(l.sources, vec![Some(0), None]);
    assert!(l.readonly[1].is_some());
    // …and an expression can never stand in for the key.
    assert!(editable(Driver::Postgres, "SELECT id + 0 AS id, price FROM t", &["id", "price"], &m).is_err());
}

#[test]
fn e10_expressions_are_read_only() {
    let m = table(&["id", "name"], &["id"]);
    let l = editable(Driver::Postgres, "SELECT id, upper(name), name FROM t", &["id", "upper", "name"], &m).unwrap();
    assert_eq!(l.readonly.iter().map(Option::is_some).collect::<Vec<_>>(), vec![false, true, false]);
}

#[test]
fn e11_rejected_shapes() {
    let m = table(&["id", "name"], &["id"]);
    for sql in [
        "SELECT * FROM t JOIN u ON u.id = t.id",
        "SELECT * FROM t LEFT JOIN u USING (id)",
        "SELECT * FROM t, u",
        "SELECT * FROM (SELECT * FROM t) x",
        "WITH x AS (SELECT * FROM t) SELECT * FROM x",
        "SELECT DISTINCT id, name FROM t",
        "SELECT id, name FROM t GROUP BY id, name",
        "SELECT id, count(*) FROM t",
        "SELECT id, name FROM t GROUP BY id HAVING count(*) > 1",
        "SELECT id, row_number() OVER (ORDER BY id) FROM t",
        "SELECT id, name FROM t UNION SELECT id, name FROM t",
        "SELECT id, name INTO t2 FROM t",
        "SELECT * FROM t; SELECT * FROM t",
        "SELECT * FROM t FOR UPDATE",
        "UPDATE t SET name = 'x'",
        "SELECT 1",
        "SELECT * FROM generate_series(1, 3)",
        "SELECT * FROM t AS x(a, b)",
        "SELECT u.id FROM t",
        "SELECT * FROM a.b.t",
        "not sql at all",
    ] {
        assert!(editable(Driver::Postgres, sql, &["id", "name"], &m).is_err(), "should be read-only: {sql}");
    }
    assert!(editable(Driver::MySql, "SELECT DISTINCTROW id, name FROM t", &["id", "name"], &m).is_err());
    assert!(editable(Driver::MySql, "SELECT SQL_CALC_FOUND_ROWS id, name FROM t", &["id", "name"], &m).is_err());
}

#[test]
fn e12_keywords_in_strings_and_comments_are_not_clauses() {
    let m = table(&["id", "note"], &["id"]);
    for sql in [
        "SELECT id, note FROM t WHERE note = 'a JOIN b GROUP BY c'",
        "SELECT id, note FROM t -- JOIN u\n",
        "/* UNION */ SELECT id, note FROM t",
        "SELECT id, note FROM t WHERE note LIKE '%count(%'",
    ] {
        editable(Driver::Postgres, sql, &["id", "note"], &m).unwrap_or_else(|e| panic!("{sql}: {e}"));
    }
}

#[test]
fn e14_binary_columns_are_read_only() {
    let mut m = table(&["id", "data"], &["id"]);
    m.columns[1].binary = true;
    let l = editable(Driver::Postgres, "SELECT * FROM t", &["id", "data"], &m).unwrap();
    assert!(l.readonly[1].as_deref().unwrap().contains("binary"));
    assert!(meta::is_binary_type(Driver::Postgres, "bytea"));
    assert!(meta::is_binary_type(Driver::MySql, "longblob"));
    assert!(meta::is_binary_type(Driver::MySql, "geometry"));
    assert!(meta::is_binary_type(Driver::Sqlite, "BLOB"));
    assert!(!meta::is_binary_type(Driver::Postgres, "text"));
    assert!(!meta::is_binary_type(Driver::MySql, "varchar(10)"));
}

#[test]
fn e15_generated_columns_are_read_only() {
    let mut m = table(&["id", "total"], &["id"]);
    m.columns[1].generated = true;
    let l = editable(Driver::Postgres, "SELECT * FROM t", &["id", "total"], &m).unwrap();
    assert_eq!(l.readonly[1].as_deref(), Some("generated column"));
}

#[test]
fn e18_case_rules_per_engine() {
    let m = table(&["id", "Order"], &["id"]);
    // Postgres: unquoted Order is order, which is not "Order".
    assert!(editable(Driver::Postgres, "SELECT id, Order FROM t", &["id", "order"], &m).is_err());
    editable(Driver::Postgres, "SELECT id, \"Order\" FROM t", &["id", "Order"], &m).unwrap();
    // MySQL: column names are case-insensitive.
    editable(Driver::MySql, "SELECT ID, `ORDER` FROM t", &["ID", "ORDER"], &m).unwrap();
}

#[test]
fn hidden_columns_are_not_part_of_star_and_counts_must_match() {
    let mut m = table(&["id", "secret", "name"], &["id"]);
    m.columns[1].hidden = true;
    let l = editable(Driver::MySql, "SELECT * FROM t", &["id", "name"], &m).unwrap();
    assert_eq!(l.sources, vec![Some(0), Some(2)]);
    // The database reported something other than what the SELECT list says.
    assert!(editable(Driver::MySql, "SELECT * FROM t", &["id", "secret", "name"], &m).is_err());
    assert!(editable(Driver::Postgres, "SELECT id, name FROM t", &["id", "other"], &table(&["id", "name"], &["id"])).is_err());
}

#[test]
fn a_column_selected_twice_is_editable_once() {
    let m = table(&["id", "name"], &["id"]);
    let l = editable(Driver::Postgres, "SELECT id, name, *, name AS name FROM t", &["id", "name", "id", "name", "name"], &m).unwrap();
    assert_eq!(l.readonly.iter().map(Option::is_some).collect::<Vec<_>>(), vec![false, false, true, true, true]);
}

// ─── Save plan (unit) ───────────────────────────────────────────────────────

fn pg_request() -> SaveRequest {
    let mut id = col("id");
    id.data_type = "integer".into();
    id.nullable = false;
    let mut name = col("name");
    name.data_type = "text".into();
    let mut active = col("active");
    active.data_type = "boolean".into();
    active.is_bool = true;
    SaveRequest {
        target: Target { driver: Driver::Postgres, schema: "public".into(), table: "people".into() },
        columns: vec![Some(id), Some(name), Some(active)],
        key: vec![0],
        supports_returning: true,
        updates: vec![],
        deletes: vec![],
        inserts: vec![],
        check_conflicts: true,
        lock_timeout_secs: 10,
    }
}

fn with_driver(mut r: SaveRequest, d: Driver) -> SaveRequest {
    r.target.driver = d;
    r.supports_returning = d != Driver::MySql;
    r
}

#[test]
fn s02_update_sets_only_edited_columns_and_matches_the_original_key() {
    let r = pg_request();
    let u = RowUpdate { row: RowRef::Data(3), original: vec![json!(7), json!("Ann"), json!(true)], sets: vec![(0, Cell::Value("8".into())), (1, Cell::Value("Bob".into()))] };
    let s = r.update(&u);
    assert_eq!(
        s.sql,
        "UPDATE \"public\".\"people\" SET \"id\" = CAST($1 AS integer), \"name\" = CAST($2 AS text) WHERE \"id\" = CAST($3 AS integer) RETURNING \"id\", \"name\", \"active\""
    );
    assert_eq!(s.binds, vec![Bind::Text("8".into()), Bind::Text("Bob".into()), Bind::Text("7".into())]);
    assert_eq!(r.key_after_update(&u), vec![(0, Bind::Text("8".into()))]);
}

#[test]
fn s03_composite_keys_match_every_column() {
    let mut r = with_driver(pg_request(), Driver::MySql);
    r.key = vec![0, 1];
    let rows: Vec<Vec<Value>> = vec![vec![json!(1), json!("a"), json!(true)], vec![json!(2), json!("b"), json!(false)]];
    let refs: Vec<&[Value]> = rows.iter().map(|r| r.as_slice()).collect();
    let s = r.delete(&refs);
    assert_eq!(s.sql, "DELETE FROM `public`.`people` WHERE (`id` = ? AND `name` = ?) OR (`id` = ? AND `name` = ?)");
    assert_eq!(s.binds, vec![Bind::Int(1), Bind::Text("a".into()), Bind::Int(2), Bind::Text("b".into())]);
}

#[test]
fn s04_values_are_bound_never_spliced() {
    for d in [Driver::Postgres, Driver::MySql, Driver::Sqlite] {
        let r = with_driver(pg_request(), d);
        for nasty in ["O'Neil", "'; DROP TABLE people; --", "back\\slash", "$1", "?", "emoji 🎉 שלום", &"x".repeat(1_000_000)] {
            let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, Cell::Value(nasty.to_string()))] };
            let s = r.update(&u);
            assert!(!s.sql.contains(nasty) || nasty == "?" || nasty == "$1", "{d:?} spliced {nasty:.20}");
            assert_eq!(s.binds[0], Bind::Text(nasty.to_string()));
        }
    }
    // The display form escapes for reading.
    let r = with_driver(pg_request(), Driver::MySql);
    let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, Cell::Value("it's a \\ test".into()))] };
    assert_eq!(r.update(&u).display, "UPDATE `public`.`people` SET `name` = 'it''s a \\\\ test' WHERE `id` = 1");
}

#[test]
fn s05_postgres_casts_to_the_declared_type() {
    let mut r = pg_request();
    let types = ["numeric(10,2)", "date", "timestamp with time zone", "uuid", "jsonb", "public.mood", "integer[]", "interval", "citext"];
    for t in types {
        r.columns[1].as_mut().unwrap().data_type = t.into();
        let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("x"), json!(true)], sets: vec![(1, Cell::Value("v".into()))] };
        assert!(r.update(&u).sql.contains(&format!("CAST($1 AS {t})")), "{t}");
    }
}

#[test]
fn booleans_become_1_or_0_on_mysql_and_sqlite() {
    for d in [Driver::MySql, Driver::Sqlite] {
        let r = with_driver(pg_request(), d);
        for (text, b) in [("true", 1), ("FALSE", 0), ("t", 1), ("no", 0), ("1", 1)] {
            let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(2, Cell::Value(text.into()))] };
            assert_eq!(r.update(&u).binds[0], Bind::Int(b), "{d:?} {text}");
        }
        // The loaded key value of a boolean is 1/0 too.
        assert_eq!(plan::bind_loaded(d, &json!(true)), Bind::Int(1));
    }
    // Postgres reads the text itself.
    let r = pg_request();
    let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(2, Cell::Value("yes".into()))] };
    assert_eq!(r.update(&u).binds[0], Bind::Text("yes".into()));
}

#[test]
fn big_integers_keep_every_digit() {
    let big = serde_json::from_str::<Value>("9007199254740993").unwrap();
    assert_eq!(plan::bind_loaded(Driver::Postgres, &big), Bind::Text("9007199254740993".into()));
    assert_eq!(plan::bind_loaded(Driver::MySql, &big), Bind::Int(9007199254740993));
    let huge = serde_json::from_str::<Value>("18446744073709551615").unwrap();
    assert_eq!(plan::bind_loaded(Driver::MySql, &huge), Bind::UInt(u64::MAX));
}

#[test]
fn s06_s07_inserts_leave_defaults_out() {
    let r = pg_request();
    let all_default = RowInsert { row: RowRef::New(1), cells: vec![(0, Cell::Default), (1, Cell::Default), (2, Cell::Default)] };
    assert_eq!(r.insert(&all_default).sql, "INSERT INTO \"public\".\"people\" DEFAULT VALUES RETURNING \"id\", \"name\", \"active\"");
    let m = with_driver(pg_request(), Driver::MySql);
    assert_eq!(m.insert(&all_default).sql, "INSERT INTO `public`.`people` () VALUES ()");
    let some = RowInsert { row: RowRef::New(2), cells: vec![(0, Cell::Default), (1, Cell::Null), (2, Cell::Value("t".into()))] };
    let s = r.insert(&some);
    assert_eq!(s.sql, "INSERT INTO \"public\".\"people\" (\"name\", \"active\") VALUES (NULL, CAST($1 AS boolean)) RETURNING \"id\", \"name\", \"active\"");
    assert_eq!(s.binds, vec![Bind::Text("t".into())]);
}

#[test]
fn generated_columns_are_never_written_on_insert() {
    let mut r = pg_request();
    r.columns[2].as_mut().unwrap().generated = true;
    let ins = RowInsert { row: RowRef::New(1), cells: vec![(1, Cell::Value("x".into())), (2, Cell::Value("t".into()))] };
    assert!(!r.insert(&ins).sql.contains("active\")"));
}

#[test]
fn s08_deletes_are_batched_by_500() {
    let mut r = pg_request();
    r.deletes = (0..1201).map(|i| RowDelete { row: RowRef::Data(i), original: vec![json!(i), json!("n"), json!(true)] }).collect();
    let script = r.script();
    assert_eq!(script.matches("DELETE FROM").count(), 3);
    let originals: Vec<&[Value]> = r.deletes.iter().map(|d| d.original.as_slice()).collect();
    let sizes: Vec<usize> = originals.chunks(plan::BATCH).map(|c| r.delete(c).binds.len()).collect();
    assert_eq!(sizes, vec![500, 500, 201]);
}

#[test]
fn s09_a_deleted_row_is_not_also_updated() {
    let mut r = pg_request();
    r.updates = vec![RowUpdate { row: RowRef::Data(1), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, Cell::Value("b".into()))] }];
    r.deletes = vec![RowDelete { row: RowRef::Data(1), original: vec![json!(1), json!("a"), json!(true)] }];
    assert!(r.effective_updates().is_empty());
    assert!(!r.script().contains("UPDATE"));
}

#[test]
fn s01_script_order_is_delete_update_insert() {
    let mut r = pg_request();
    r.inserts = vec![RowInsert { row: RowRef::New(1), cells: vec![(1, Cell::Value("z".into()))] }];
    r.updates = vec![RowUpdate { row: RowRef::Data(2), original: vec![json!(2), json!("b"), json!(true)], sets: vec![(1, Cell::Value("c".into()))] }];
    r.deletes = vec![RowDelete { row: RowRef::Data(1), original: vec![json!(1), json!("a"), json!(true)] }];
    let s = r.script();
    let (d, u, i) = (s.find("DELETE").unwrap(), s.find("UPDATE").unwrap(), s.find("INSERT").unwrap());
    assert!(d < u && u < i, "{s}");
}

#[test]
fn s11_mysql_insert_must_be_findable_afterwards() {
    let mut r = with_driver(pg_request(), Driver::MySql);
    r.inserts = vec![RowInsert { row: RowRef::New(1), cells: vec![(1, Cell::Value("x".into()))] }];
    let err = r.validate().unwrap_err();
    assert_eq!(err.0, Some(RowRef::New(1)));
    // With the key auto-incremented, it is.
    r.columns[0].as_mut().unwrap().auto_increment = true;
    r.validate().unwrap();
    // Or with the key entered.
    r.columns[0].as_mut().unwrap().auto_increment = false;
    r.inserts[0].cells.push((0, Cell::Value("5".into())));
    r.validate().unwrap();
}

#[test]
fn s12_identifiers_are_quoted_and_escaped() {
    assert_eq!(Driver::Postgres.quote("we\"ird"), "\"we\"\"ird\"");
    assert_eq!(Driver::MySql.quote("we`ird"), "`we``ird`");
    assert_eq!(Driver::Sqlite.quote("order"), "\"order\"");
}

#[test]
fn default_is_only_for_new_rows() {
    let mut r = pg_request();
    r.updates = vec![RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, Cell::Default)] }];
    assert!(r.validate().is_err());
}

/// A tiny deterministic generator, so the property tests need no crate.
struct Lcg(u64);
impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: u64) -> u64 {
        self.next() % n.max(1)
    }
}

#[test]
fn s13_s14_plans_touch_exactly_the_queued_rows_and_are_deterministic() {
    let mut rng = Lcg(42);
    for _ in 0..300 {
        let mut r = with_driver(pg_request(), [Driver::Postgres, Driver::MySql, Driver::Sqlite][rng.below(3) as usize]);
        let rows: Vec<Vec<Value>> = (0..20).map(|i| vec![json!(i * 10), json!(format!("n{i}")), json!(i % 2 == 0)]).collect();
        let mut deleted = std::collections::BTreeSet::new();
        for _ in 0..rng.below(5) {
            let i = rng.below(20) as usize;
            if deleted.insert(i) {
                r.deletes.push(RowDelete { row: RowRef::Data(i), original: rows[i].clone() });
            }
        }
        let mut updated = std::collections::BTreeSet::new();
        for _ in 0..rng.below(6) {
            let i = rng.below(20) as usize;
            if updated.insert(i) {
                let sets = vec![(1 + rng.below(2) as usize, Cell::Value(format!("v{}", rng.next())))];
                r.updates.push(RowUpdate { row: RowRef::Data(i), original: rows[i].clone(), sets });
            }
        }
        let script = r.script();
        assert_eq!(script, r.script(), "deterministic");
        let expected_updates = updated.difference(&deleted).count();
        assert_eq!(script.matches("UPDATE ").count(), expected_updates);
        for u in r.effective_updates() {
            let s = r.update(u);
            // The WHERE binds exactly this row's key.
            assert_eq!(s.binds.last(), Some(&plan::bind_loaded(r.driver(), &u.original[0])));
            assert_eq!(s.binds.len(), u.sets.len() + 1);
        }
        if !deleted.is_empty() {
            let originals: Vec<&[Value]> = r.deletes.iter().map(|d| d.original.as_slice()).collect();
            let s = r.delete(&originals);
            let mut keys: Vec<Bind> = deleted.iter().map(|&i| plan::bind_loaded(r.driver(), &rows[i][0])).collect();
            let mut got = s.binds.clone();
            let ord = |b: &Bind| format!("{b:?}");
            keys.sort_by_key(ord);
            got.sort_by_key(ord);
            assert_eq!(got, keys);
        }
    }
}

// ─── Integration ────────────────────────────────────────────────────────────

/// A database to test against, and a second connection's worth of access to
/// play "someone else".
#[derive(Clone)]
enum Db {
    Pg(sqlx::PgPool),
    My(sqlx::MySqlPool),
    Lite(sqlx::SqlitePool),
}

static SEQ: AtomicU64 = AtomicU64::new(0);

fn unique(prefix: &str) -> String {
    format!("{prefix}_{}_{}", std::process::id(), SEQ.fetch_add(1, Ordering::Relaxed))
}

impl Db {
    fn driver(&self) -> Driver {
        match self {
            Db::Pg(_) => Driver::Postgres,
            Db::My(_) => Driver::MySql,
            Db::Lite(..) => Driver::Sqlite,
        }
    }

    async fn exec(&self, sql: &str) {
        let r = match self {
            Db::Pg(p) => sqlx::raw_sql(sql).execute(p).await.map(|_| ()),
            Db::My(p) => sqlx::raw_sql(sql).execute(p).await.map(|_| ()),
            Db::Lite(p) => sqlx::raw_sql(sql).execute(p).await.map(|_| ()),
        };
        r.unwrap_or_else(|e| panic!("{sql}: {e}"));
    }

    /// Rows as the grid would load them.
    async fn load(&self, sql: &str) -> (Vec<String>, Vec<Vec<Value>>) {
        use sqlx::{Column, Row};
        macro_rules! go {
            ($pool:expr, $decode:path) => {{
                let rows = sqlx::query(sql).fetch_all($pool).await.unwrap_or_else(|e| panic!("{sql}: {e}"));
                let cols = rows.first().map(|r| r.columns().iter().map(|c| c.name().to_string()).collect()).unwrap_or_default();
                (cols, rows.iter().map(|r| $decode(r).unwrap()).collect())
            }};
        }
        match self {
            Db::Pg(p) => go!(p, crate::queries::pg_row_to_json_values),
            Db::My(p) => go!(p, crate::queries::mysql_row_to_json_values),
            Db::Lite(p) => go!(p, crate::queries::sqlite_row_to_json_values),
        }
    }

    async fn meta(&self, table: &str) -> TableMeta {
        match self {
            Db::Pg(p) => meta::postgres(p, "", table).await,
            Db::My(p) => meta::mysql(p, "", table).await,
            Db::Lite(p) => meta::sqlite(p, "", table).await,
        }
        .unwrap()
    }

    fn source(&self) -> Source {
        match self {
            Db::Pg(p) => Source::Postgres(p.clone()),
            Db::My(p) => Source::MySql(p.clone()),
            Db::Lite(p) => Source::Sqlite(p.clone()),
        }
    }

    fn q(&self, name: &str) -> String {
        self.driver().quote(name)
    }

    /// Column type spellings that differ per engine.
    fn serial(&self) -> &'static str {
        match self {
            Db::Pg(_) => "SERIAL PRIMARY KEY",
            Db::My(_) => "INT AUTO_INCREMENT PRIMARY KEY",
            Db::Lite(..) => "INTEGER PRIMARY KEY",
        }
    }

    fn text_type(&self) -> &'static str {
        match self {
            Db::My(_) => "VARCHAR(255)",
            _ => "TEXT",
        }
    }

    fn now(&self) -> &'static str {
        match self {
            Db::Pg(_) => "TIMESTAMP DEFAULT CURRENT_TIMESTAMP",
            Db::My(_) => "DATETIME DEFAULT CURRENT_TIMESTAMP",
            Db::Lite(..) => "TEXT DEFAULT CURRENT_TIMESTAMP",
        }
    }

    /// Hold a write lock on `table` from another connection until the
    /// returned sender is dropped or sent to.
    async fn hold_lock(&self, table: &str, where_clause: &str) -> tokio::sync::oneshot::Sender<()> {
        let (tx, rx) = tokio::sync::oneshot::channel::<()>();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel::<()>();
        let t = self.q(table);
        match self.clone() {
            Db::Pg(p) => {
                let sql = format!("SELECT * FROM {t} WHERE {where_clause} FOR UPDATE");
                tokio::spawn(async move {
                    let mut tx_ = p.begin().await.unwrap();
                    sqlx::query(&sql).fetch_all(&mut *tx_).await.unwrap();
                    let _ = ready_tx.send(());
                    let _ = rx.await;
                    tx_.rollback().await.unwrap();
                });
            }
            Db::My(p) => {
                let sql = format!("SELECT * FROM {t} WHERE {where_clause} FOR UPDATE");
                tokio::spawn(async move {
                    let mut tx_ = p.begin().await.unwrap();
                    sqlx::query(&sql).fetch_all(&mut *tx_).await.unwrap();
                    let _ = ready_tx.send(());
                    let _ = rx.await;
                    tx_.rollback().await.unwrap();
                });
            }
            Db::Lite(p) => {
                tokio::spawn(async move {
                    let mut c = p.acquire().await.unwrap();
                    sqlx::Executor::execute(&mut *c, "BEGIN IMMEDIATE").await.unwrap();
                    let _ = ready_tx.send(());
                    let _ = rx.await;
                    sqlx::Executor::execute(&mut *c, "ROLLBACK").await.unwrap();
                });
            }
        }
        ready_rx.await.unwrap();
        tx
    }

    /// Row locks held on `table` by any transaction.
    async fn locks_on(&self, table: &str) -> i64 {
        match self {
            Db::Pg(p) => sqlx::query_scalar("SELECT count(*) FROM pg_locks l JOIN pg_class c ON c.oid = l.relation WHERE c.relname = $1")
                .bind(table)
                .fetch_one(p)
                .await
                .unwrap(),
            Db::My(p) => sqlx::query_scalar::<_, i64>("SELECT count(*) FROM performance_schema.data_locks WHERE OBJECT_NAME = ?")
                .bind(table)
                .fetch_one(p)
                .await
                .unwrap_or(0),
            Db::Lite(..) => 0,
        }
    }
}

/// Remove test databases left by earlier runs (an hour or more old).
fn sweep_old_sqlite_files() {
    let Ok(entries) = std::fs::read_dir(std::env::temp_dir()) else { return };
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        let old = e.metadata().and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age.as_secs() > 3600);
        if name.starts_with("multidb_edit_test_") && old {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

async fn databases() -> Vec<Db> {
    let mut out = Vec::new();
    sweep_old_sqlite_files();
    let path = std::env::temp_dir().join(format!("{}.sqlite", unique("multidb_edit_test")));
    let opts = sqlx::sqlite::SqliteConnectOptions::new().filename(&path).create_if_missing(true).foreign_keys(true);
    let pool = sqlx::sqlite::SqlitePoolOptions::new().max_connections(4).connect_with(opts).await.unwrap();
    out.push(Db::Lite(pool));
    if let Ok(url) = std::env::var("MULTIDB_TEST_PG") {
        let pool = sqlx::postgres::PgPoolOptions::new().max_connections(6).connect(&url).await.expect("MULTIDB_TEST_PG");
        out.push(Db::Pg(pool));
    }
    if let Ok(url) = std::env::var("MULTIDB_TEST_MYSQL") {
        let pool = sqlx::mysql::MySqlPoolOptions::new().max_connections(6).connect(&url).await.expect("MULTIDB_TEST_MYSQL");
        out.push(Db::My(pool));
    }
    out
}

/// A table with rows, loaded the way the grid loads it.
struct Fixture {
    db: Db,
    table: String,
    columns: Vec<String>,
    rows: Vec<Vec<Value>>,
    req: SaveRequest,
}

impl Fixture {
    /// Create `table` with `ddl` columns, insert `rows`, and load `select`
    /// (with `{t}` standing for the table).
    async fn new(db: &Db, ddl: &str, inserts: &[&str], select: &str) -> Fixture {
        let table = unique("ge");
        let t = db.q(&table);
        let engine = if matches!(db, Db::My(_)) { " ENGINE=InnoDB" } else { "" };
        db.exec(&format!("CREATE TABLE {t} ({ddl}){engine}")).await;
        for row in inserts {
            db.exec(&format!("INSERT INTO {t} {row}")).await;
        }
        if matches!(db, Db::Pg(_)) && ddl.contains("SERIAL") {
            // Rows inserted with explicit ids leave the sequence behind.
            db.exec(&format!("SELECT setval(pg_get_serial_sequence('{t}', 'id'), COALESCE(max(id), 0) + 1, false) FROM {t}")).await;
        }
        let mut f = Fixture { db: db.clone(), table, columns: vec![], rows: vec![], req: pg_request() };
        f.reload(select).await;
        f
    }

    async fn reload(&mut self, select: &str) {
        let sql = select.replace("{t}", &self.db.q(&self.table));
        let (columns, rows) = self.db.load(&sql).await;
        let meta = self.db.meta(&self.table).await;
        let driver = self.db.driver();
        let parsed = query::parse(&sql, driver).unwrap();
        let columns = if columns.is_empty() {
            // No rows: take the column names from the table, as the SELECT list says.
            meta.columns.iter().filter(|c| !c.hidden).map(|c| c.name.clone()).collect()
        } else {
            columns
        };
        let layout = query::layout(driver, &parsed, &columns, &meta).unwrap();
        self.req = SaveRequest {
            target: Target { driver, schema: if driver == Driver::Sqlite { String::new() } else { meta.schema.clone() }, table: meta.table.clone() },
            columns: layout.sources.iter().map(|s| s.map(|i| meta.columns[i].clone())).collect(),
            key: layout.key,
            supports_returning: meta.supports_returning,
            updates: vec![],
            deletes: vec![],
            inserts: vec![],
            check_conflicts: true,
            lock_timeout_secs: 10,
        };
        self.columns = columns;
        self.rows = rows;
    }

    fn col(&self, name: &str) -> usize {
        self.columns.iter().position(|c| c.eq_ignore_ascii_case(name)).unwrap_or_else(|| panic!("no column {name}"))
    }

    /// Index of the loaded row whose `id` column is `id`.
    fn row(&self, id: i64) -> usize {
        let c = self.col("id");
        self.rows.iter().position(|r| r[c].as_i64() == Some(id)).unwrap_or_else(|| panic!("no row {id}"))
    }

    fn edit(&mut self, id: i64, column: &str, cell: Cell) {
        let (r, c) = (self.row(id), self.col(column));
        match self.req.updates.iter_mut().find(|u| u.row == RowRef::Data(r)) {
            Some(u) => u.sets.push((c, cell)),
            None => self.req.updates.push(RowUpdate { row: RowRef::Data(r), original: self.rows[r].clone(), sets: vec![(c, cell)] }),
        }
    }

    fn delete(&mut self, id: i64) {
        let r = self.row(id);
        self.req.deletes.push(RowDelete { row: RowRef::Data(r), original: self.rows[r].clone() });
    }

    fn insert(&mut self, n: u64, cells: &[(&str, Cell)]) {
        let cells = cells.iter().map(|(c, v)| (self.col(c), v.clone())).collect();
        self.req.inserts.push(RowInsert { row: RowRef::New(n), cells });
    }

    async fn save(&self) -> SaveOutcome {
        exec::save(self.db.source(), self.req.clone(), SaveControl::new()).await
    }

    async fn saved(&self) -> SaveReport {
        match self.save().await {
            SaveOutcome::Saved(r) => r,
            other => panic!("{:?}: expected a save, got {other:?}", self.db.driver()),
        }
    }

    /// The table's rows now, ordered by id, as (id, `column`) text.
    async fn values(&self, column: &str) -> Vec<(i64, Option<String>)> {
        let t = self.db.q(&self.table);
        let (cols, rows) = self.db.load(&format!("SELECT * FROM {t} ORDER BY id")).await;
        let (ci, cc) = (cols.iter().position(|c| c == "id").unwrap_or(0), cols.iter().position(|c| c.eq_ignore_ascii_case(column)).unwrap_or(0));
        rows.iter().map(|r| (r[ci].as_i64().unwrap(), super::exact_text(&r[cc]))).collect()
    }

    async fn count(&self) -> usize {
        let t = self.db.q(&self.table);
        self.db.load(&format!("SELECT * FROM {t}")).await.1.len()
    }

    async fn sql(&self, sql: &str) {
        self.db.exec(&sql.replace("{t}", &self.db.q(&self.table))).await;
    }
}

fn people(db: &Db) -> String {
    format!("id {}, name {} NOT NULL, email {} UNIQUE, score INT, created {}", db.serial(), db.text_type(), db.text_type(), db.now())
}

const PEOPLE: [&str; 3] = [
    "(id, name, email, score) VALUES (1, 'Ann', 'ann@x', 10)",
    "(id, name, email, score) VALUES (2, 'Bob', 'bob@x', 20)",
    "(id, name, email, score) VALUES (3, 'Cid', NULL, NULL)",
];

fn v(s: &str) -> Cell {
    Cell::Value(s.into())
}

fn some(s: &str) -> Option<String> {
    Some(s.to_string())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x01_x02_edits_are_committed_and_returned() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.edit(1, "name", v("Anna"));
        f.edit(1, "score", v("11"));
        f.edit(2, "name", v("Bobby"));
        let report = f.saved().await;
        assert_eq!(f.values("name").await, vec![(1, some("Anna")), (2, some("Bobby")), (3, some("Cid"))], "{:?}", db.driver());
        assert_eq!(f.values("score").await[0], (1, some("11")));
        // The report carries the rows as stored, ready to patch into the grid.
        let (row, values) = report.updated.iter().find(|(r, _)| *r == RowRef::Data(f.row(1))).unwrap();
        assert_eq!(*row, RowRef::Data(0));
        assert_eq!(values[f.col("name")], json!("Anna"));
        assert_eq!(values[f.col("score")], json!(11), "{:?}: stored as a number", db.driver());
        assert_eq!(values[f.col("email")], f.rows[0][f.col("email")], "untouched columns as loaded");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x03_x04_x05_null_text_null_and_empty_string() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.edit(1, "email", Cell::Null);
        f.edit(2, "email", v("NULL"));
        f.edit(3, "email", v(""));
        f.saved().await;
        assert_eq!(f.values("email").await, vec![(1, None), (2, some("NULL")), (3, some(""))], "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x06_x07_x08_inserts_use_defaults_and_read_back_generated_values() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.insert(1, &[("name", v("Dee")), ("score", Cell::Null)]);
        f.insert(2, &[("name", v("Eve")), ("email", v("eve@x")), ("score", v("5"))]);
        let report = f.saved().await;
        assert_eq!(report.inserted.len(), 2);
        let (_, dee) = &report.inserted[0];
        assert_eq!(dee[f.col("id")], json!(4), "{:?}: generated key read back", db.driver());
        assert_eq!(dee[f.col("score")], Value::Null);
        assert_ne!(dee[f.col("created")], Value::Null, "{:?}: column default read back", db.driver());
        assert_eq!(report.inserted[1].1[f.col("id")], json!(5));
        assert_eq!(f.count().await, 5);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x08_a_row_of_nothing_but_defaults() {
    for db in databases().await {
        let ddl = format!("id {}, note {} DEFAULT 'hello', n INT", db.serial(), db.text_type());
        let mut f = Fixture::new(&db, &ddl, &[], "SELECT * FROM {t}").await;
        f.insert(1, &[]);
        let report = f.saved().await;
        let (_, row) = &report.inserted[0];
        assert_eq!(row[f.col("note")], json!("hello"), "{:?}", db.driver());
        assert_eq!(row[f.col("n")], Value::Null);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x09_deletes_one_and_many() {
    for db in databases().await {
        let ddl = format!("id INT PRIMARY KEY, v {}", db.text_type());
        let values: Vec<String> = (1..=1201).map(|i| format!("({i}, 'v{i}')")).collect();
        let insert = format!("(id, v) VALUES {}", values.join(", "));
        let mut f = Fixture::new(&db, &ddl, &[&insert], "SELECT * FROM {t} ORDER BY id").await;
        f.delete(7);
        f.saved().await;
        assert_eq!(f.count().await, 1200);
        f.reload("SELECT * FROM {t} ORDER BY id").await;
        for id in (1..=1201).filter(|&i| i != 7) {
            f.delete(id);
        }
        let report = f.saved().await;
        assert_eq!(report.deleted.len(), 1200);
        assert_eq!(f.count().await, 0, "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x10_x11_mixed_changes_in_one_transaction_in_a_safe_order() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        // Deleting ann@x and inserting a new ann@x only works if the delete runs first.
        f.delete(1);
        f.insert(1, &[("id", v("10")), ("name", v("New Ann")), ("email", v("ann@x"))]);
        f.edit(2, "name", v("Robert"));
        f.saved().await;
        assert_eq!(f.values("email").await, vec![(2, some("bob@x")), (3, None), (10, some("ann@x"))], "{:?}", db.driver());
        assert_eq!(f.values("name").await[0], (2, some("Robert")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x12_editing_the_key_finds_the_row_by_its_old_key() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.edit(2, "id", v("20"));
        f.edit(2, "name", v("Bob 20"));
        let report = f.saved().await;
        assert_eq!(f.values("name").await, vec![(1, some("Ann")), (3, some("Cid")), (20, some("Bob 20"))], "{:?}", db.driver());
        assert_eq!(report.updated[0].1[f.col("id")], json!(20));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x13_x14_the_grid_gets_what_the_database_stored() {
    for db in databases().await {
        let ddl = match db {
            Db::Pg(_) => "id INT PRIMARY KEY, amount NUMERIC(10,2), flag BOOLEAN, day DATE",
            Db::My(_) => "id INT PRIMARY KEY, amount DECIMAL(10,2), flag BOOLEAN, day DATE",
            Db::Lite(..) => "id INTEGER PRIMARY KEY, amount REAL, flag BOOLEAN, day DATE",
        };
        let mut f = Fixture::new(&db, ddl, &["(id, amount, flag, day) VALUES (1, 1.5, true, '2024-01-02')"], "SELECT * FROM {t}").await;
        f.edit(1, "amount", v("2.5"));
        f.edit(1, "flag", v("false"));
        let report = f.saved().await;
        let row = &report.updated[0].1;
        // Exactly what loading the row again gives.
        let (_, now) = db.load(&format!("SELECT * FROM {}", db.q(&f.table))).await;
        assert_eq!(*row, now[0], "{:?}", db.driver());
        assert_eq!(row[f.col("flag")], json!(false), "{:?}", db.driver());
        // Saving again from the returned row is not a conflict.
        let mut f2 = Fixture { rows: vec![row.clone()], ..f };
        f2.req.updates.clear();
        f2.edit(1, "amount", v("3"));
        f2.saved().await;
    }
    if let Some(db) = databases().await.into_iter().find(|d| matches!(d, Db::Pg(_))) {
        // A trigger's change shows in the returned row.
        let mut f = Fixture::new(&db, "id INT PRIMARY KEY, name TEXT", &["(id, name) VALUES (1, 'a')"], "SELECT * FROM {t}").await;
        let func = format!("{}_up", f.table);
        f.sql(&format!(
            "CREATE FUNCTION {func}() RETURNS trigger AS $$ BEGIN NEW.name := upper(NEW.name); RETURN NEW; END $$ LANGUAGE plpgsql;
             CREATE TRIGGER up BEFORE UPDATE ON {{t}} FOR EACH ROW EXECUTE FUNCTION {func}();"
        ))
        .await;
        f.edit(1, "name", v("shout"));
        let report = f.saved().await;
        assert_eq!(report.updated[0].1[f.col("name")], json!("SHOUT"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x19_long_and_unicode_text_round_trips() {
    for db in databases().await {
        let ddl = match db {
            Db::My(_) => "id INT PRIMARY KEY, body LONGTEXT CHARACTER SET utf8mb4",
            _ => "id INT PRIMARY KEY, body TEXT",
        };
        let mut f = Fixture::new(&db, ddl, &["(id, body) VALUES (1, 'x')"], "SELECT * FROM {t}").await;
        let long = format!("{}🎉 שלום 'quotes' \\ back\\slash \"dq\" $1 ?", "é".repeat(500_000));
        f.edit(1, "body", Cell::Value(long.clone()));
        f.saved().await;
        assert_eq!(f.values("body").await, vec![(1, Some(long))], "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn x21_sqlite_without_returning_reads_rows_back_another_way() {
    let db = databases().await.remove(0);
    let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
    f.req.supports_returning = false;
    f.edit(1, "name", v("A2"));
    f.insert(1, &[("name", v("Dee"))]);
    let report = f.saved().await;
    assert_eq!(report.updated[0].1[f.col("name")], json!("A2"));
    assert_eq!(report.inserted[0].1[f.col("id")], json!(4));
}

// ── Conflicts

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c01_c04_c05_a_changed_row_is_reported_and_can_be_overridden() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET name = 'Theirs' WHERE id = 1").await;
        f.edit(1, "name", v("Mine"));
        f.edit(2, "name", v("Bob 2"));
        match f.save().await {
            SaveOutcome::Conflicts(c) => {
                assert_eq!(c.len(), 1, "{:?}", db.driver());
                assert_eq!(c[0].row, RowRef::Data(f.row(1)));
                assert_eq!(c[0].changed, vec![f.col("name")]);
                assert_eq!(c[0].current[f.col("name")], json!("Theirs"));
                assert!(!c[0].deleting);
            }
            other => panic!("{other:?}"),
        }
        // Nothing was written, not even the row without a conflict.
        assert_eq!(f.values("name").await[..2], [(1, some("Theirs")), (2, some("Bob"))]);
        // They change it again; Override writes anyway.
        f.sql("UPDATE {t} SET name = 'Theirs again' WHERE id = 1").await;
        f.req.check_conflicts = false;
        f.saved().await;
        assert_eq!(f.values("name").await[..2], [(1, some("Mine")), (2, some("Bob 2"))], "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c02_c03_any_shown_column_counts_columns_not_shown_do_not() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT id, name, email FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET score = 99 WHERE id = 1").await;
        f.edit(1, "name", v("Mine"));
        f.saved().await;
        f.reload("SELECT id, name, email FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET email = 'changed@x' WHERE id = 1").await;
        f.edit(1, "name", v("Mine again"));
        assert!(matches!(f.save().await, SaveOutcome::Conflicts(c) if c[0].changed == vec![2]), "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c06_c14_a_change_that_was_put_back_is_not_a_conflict() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET name = 'Tmp' WHERE id = 1").await;
        f.sql("UPDATE {t} SET name = 'Ann' WHERE id = 1").await;
        f.edit(1, "score", v("1"));
        f.saved().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c07_an_edited_row_that_was_deleted_cannot_be_saved() {
    for db in databases().await {
        for check in [true, false] {
            let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
            f.sql("DELETE FROM {t} WHERE id = 2").await;
            f.edit(1, "name", v("Ann 2"));
            f.edit(2, "name", v("Bob 2"));
            f.req.check_conflicts = check;
            match f.save().await {
                SaveOutcome::Failed(e) => {
                    assert_eq!(e.row, Some(RowRef::Data(f.row(2))), "{:?}", db.driver());
                    assert!(e.message.contains("no longer exists"), "{}", e.message);
                }
                other => panic!("{:?} check={check}: {other:?}", db.driver()),
            }
            assert_eq!(f.values("name").await[0], (1, some("Ann")), "rolled back");
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c08_c09_deletes_of_rows_already_gone_count_as_done() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("DELETE FROM {t} WHERE id = 1").await;
        f.delete(1);
        let report = f.saved().await;
        assert_eq!(report.already_deleted, vec![RowRef::Data(0)]);

        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("DELETE FROM {t} WHERE id = 1").await;
        f.delete(1);
        f.delete(2);
        f.edit(3, "name", v("Cid 2"));
        let report = f.saved().await;
        assert_eq!(report.already_deleted.len(), 1);
        assert_eq!(report.deleted.len(), 2);
        assert_eq!(f.values("name").await, vec![(3, some("Cid 2"))], "{:?}", db.driver());

        // Without the check (Override), a gone row is still not an error.
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("DELETE FROM {t} WHERE id = 1").await;
        f.delete(1);
        f.delete(2);
        f.req.check_conflicts = false;
        f.saved().await;
        assert_eq!(f.count().await, 1);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c10_a_row_to_delete_that_changed_is_a_conflict() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET score = 50 WHERE id = 2").await;
        f.delete(2);
        match f.save().await {
            SaveOutcome::Conflicts(c) => assert!(c[0].deleting && c[0].changed == vec![f.col("score")], "{c:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.count().await, 3);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c11_many_conflicts_are_all_reported() {
    for db in databases().await {
        let ddl = format!("id INT PRIMARY KEY, v {}", db.text_type());
        let values: Vec<String> = (1..=60).map(|i| format!("({i}, 'v')")).collect();
        let insert = format!("(id, v) VALUES {}", values.join(", "));
        let mut f = Fixture::new(&db, &ddl, &[&insert], "SELECT * FROM {t} ORDER BY id").await;
        f.sql("UPDATE {t} SET v = 'theirs'").await;
        for id in 1..=60 {
            f.edit(id, "v", v("mine"));
        }
        assert!(matches!(f.save().await, SaveOutcome::Conflicts(c) if c.len() == 60));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c13_values_that_decode_lossily_are_not_false_conflicts() {
    for db in databases().await {
        let ddl = match db {
            Db::Pg(_) => "id INT PRIMARY KEY, f DOUBLE PRECISION, r REAL, n NUMERIC, ts TIMESTAMPTZ, t TIMESTAMP(6), j JSONB, u UUID, big BIGINT",
            Db::My(_) => "id INT PRIMARY KEY, f DOUBLE, r FLOAT, n DECIMAL(30,10), ts TIMESTAMP(6) NULL, t DATETIME(6), j JSON, u CHAR(36), big BIGINT UNSIGNED",
            Db::Lite(..) => "id INTEGER PRIMARY KEY, f REAL, r REAL, n NUMERIC, ts TEXT, t DATETIME, j TEXT, u TEXT, big INTEGER",
        };
        let big = if matches!(db, Db::My(_)) { "18446744073709551615" } else { "9223372036854775807" };
        let row = format!(
            "(id, f, r, n, ts, t, j, u, big) VALUES (1, 0.1, 0.3, 12345678901234567890.0123456789, '2024-03-04 05:06:07.123456+00', '2024-03-04 05:06:07.123456', '{{\"b\": 1, \"a\": [1, 2.50]}}', 'a0eebc99-9c0b-4ef8-bb6d-6bb9bd380a11', {big})"
        );
        let row = if matches!(db, Db::My(_)) { row.replace("+00'", "'") } else { row };
        let mut f = Fixture::new(&db, ddl, &[&row], "SELECT * FROM {t}").await;
        f.edit(1, "big", Cell::Value(big.into()));
        f.saved().await;
        // The key itself is a big integer that a float would round.
        let ddl = match db {
            Db::My(_) => "id BIGINT UNSIGNED PRIMARY KEY, v VARCHAR(10)".to_string(),
            _ => "id BIGINT PRIMARY KEY, v TEXT".to_string(),
        };
        let mut f = Fixture::new(&db, &ddl, &["(id, v) VALUES (9007199254740993, 'a')", "(id, v) VALUES (9007199254740992, 'b')"], "SELECT * FROM {t} ORDER BY id").await;
        let r = f.rows.iter().position(|r| r[0] == serde_json::from_str::<Value>("9007199254740993").unwrap()).unwrap();
        f.req.updates.push(RowUpdate { row: RowRef::Data(r), original: f.rows[r].clone(), sets: vec![(1, v("changed"))] });
        f.saved().await;
        let (_, rows) = db.load(&format!("SELECT v FROM {} ORDER BY id", db.q(&f.table))).await;
        assert_eq!(rows, vec![vec![json!("b")], vec![json!("changed")]], "{:?}: the neighbouring key was not touched", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn c16_inserting_a_key_someone_else_just_used_is_an_error() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.sql("INSERT INTO {t} (id, name) VALUES (7, 'theirs')").await;
        f.insert(1, &[("id", v("7")), ("name", v("mine"))]);
        f.edit(1, "name", v("Ann 2"));
        assert!(matches!(f.save().await, SaveOutcome::Failed(e) if e.row == Some(RowRef::New(1)) && e.action == Action::Insert));
        assert_eq!(f.values("name").await[0], (1, some("Ann")), "{:?}", db.driver());
    }
}

// ── Failures roll everything back

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f01_a_foreign_key_stops_every_delete() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        let child = unique("child");
        let (c, t) = (db.q(&child), db.q(&f.table));
        let engine = if matches!(db, Db::My(_)) { " ENGINE=InnoDB" } else { "" };
        db.exec(&format!("CREATE TABLE {c} (id INT PRIMARY KEY, person_id INT, CONSTRAINT {child}_fk FOREIGN KEY (person_id) REFERENCES {t}(id)){engine}")).await;
        db.exec(&format!("INSERT INTO {c} VALUES (1, 2)")).await;
        f.delete(1);
        f.delete(2);
        f.delete(3);
        match f.save().await {
            SaveOutcome::Failed(e) => assert_eq!(e.action, Action::Delete, "{e:?}"),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.count().await, 3, "{:?}: every delete rolled back", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f02_f05_a_late_failure_undoes_the_early_work() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.delete(3);
        f.edit(1, "name", v("Changed"));
        f.insert(1, &[("name", v("Dup")), ("email", v("bob@x"))]);
        match f.save().await {
            SaveOutcome::Failed(e) => {
                assert_eq!((e.action, e.row), (Action::Insert, Some(RowRef::New(1))), "{:?}", db.driver());
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(f.values("name").await, vec![(1, some("Ann")), (2, some("Bob")), (3, some("Cid"))]);
        // Fixing the problem and saving again applies everything once.
        f.req.inserts[0].cells = vec![(f.col("name"), v("Dup")), (f.col("email"), v("dup@x"))];
        f.saved().await;
        assert_eq!(f.count().await, 3, "{:?}", db.driver());
        assert_eq!(f.values("name").await[0], (1, some("Changed")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f03_f04_f06_bad_values_are_errors_with_the_row() {
    for db in databases().await {
        if matches!(db, Db::Lite(..)) {
            // SQLite stores 'abc' in an INT column as text; only STRICT tables refuse it.
            continue;
        }
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.edit(2, "score", v("abc"));
        assert!(matches!(f.save().await, SaveOutcome::Failed(e) if e.row == Some(RowRef::Data(1))), "{:?}", db.driver());
    }
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.insert(1, &[("email", v("x@x"))]); // name is NOT NULL with no default
        assert!(matches!(f.save().await, SaveOutcome::Failed(e) if e.row == Some(RowRef::New(1))), "{:?}", db.driver());
        f.req.inserts.clear();
        f.edit(1, "name", Cell::Null);
        assert!(matches!(f.save().await, SaveOutcome::Failed(e) if e.row == Some(RowRef::Data(0))), "{:?}", db.driver());
    }
    for db in databases().await {
        if matches!(db, Db::My(_)) {
            continue; // CHECK is enforced from MySQL 8.0.16 only
        }
        let mut f = Fixture::new(&db, "id INT PRIMARY KEY, n INT CHECK (n > 0)", &["(id, n) VALUES (1, 1)"], "SELECT * FROM {t}").await;
        f.edit(1, "n", v("-1"));
        assert!(matches!(f.save().await, SaveOutcome::Failed(_)), "{:?}", db.driver());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f05_unique_violation_on_update() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.edit(1, "name", v("ok"));
        f.edit(2, "email", v("ann@x"));
        match f.save().await {
            SaveOutcome::Failed(e) => assert_eq!((e.action, e.row), (Action::Update, Some(RowRef::Data(1)))),
            other => panic!("{other:?}"),
        }
        assert_eq!(f.values("name").await[0], (1, some("Ann")));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn f11_a_lock_held_too_long_times_out_and_rolls_back() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        let release = db.hold_lock(&f.table, "id = 2").await;
        f.req.lock_timeout_secs = 1;
        f.edit(2, "name", v("Mine"));
        let started = std::time::Instant::now();
        let outcome = f.save().await;
        assert!(matches!(outcome, SaveOutcome::Failed(_)), "{:?}: {outcome:?}", db.driver());
        assert!(started.elapsed() < std::time::Duration::from_secs(8), "{:?} took {:?}", db.driver(), started.elapsed());
        drop(release);
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        f.req.lock_timeout_secs = 10;
        f.saved().await;
    }
}

// ── Stop

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t01_t06_stop_while_blocked_cancels_on_the_server_and_rolls_back() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        let release = db.hold_lock(&f.table, "id = 2").await;
        f.edit(1, "name", v("Mine 1"));
        f.edit(2, "name", v("Mine 2"));
        f.req.lock_timeout_secs = 30;
        let ctl = SaveControl::new();
        let task = tokio::spawn(exec::save(db.source(), f.req.clone(), ctl.clone()));
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        let started = std::time::Instant::now();
        assert!(ctl.stop().await);
        if matches!(db, Db::Lite(..)) {
            // SQLite can't interrupt a statement waiting for a lock; let it go.
            drop(release);
            let outcome = task.await.unwrap();
            assert!(matches!(outcome, SaveOutcome::Cancelled | SaveOutcome::Failed(_)), "{outcome:?}");
        } else {
            let outcome = tokio::time::timeout(std::time::Duration::from_secs(5), task).await.expect("stop took effect").unwrap();
            assert_eq!(outcome, SaveOutcome::Cancelled, "{:?}", db.driver());
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            drop(release);
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        assert_eq!(f.values("name").await[..2], [(1, some("Ann")), (2, some("Bob"))], "{:?}", db.driver());
        assert_eq!(db.locks_on(&f.table).await, 0, "{:?}: no transaction left holding the rows", db.driver());
        // The pool still works for the next save.
        f.saved().await;
        assert_eq!(f.values("name").await[..2], [(1, some("Mine 1")), (2, some("Mine 2"))]);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t04_stop_is_refused_once_committing() {
    let ctl = SaveControl::new();
    assert!(ctl.begin_commit_for_test().await);
    assert!(!ctl.stop().await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t02_stop_before_writing_writes_nothing() {
    for db in databases().await {
        let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
        f.delete(1);
        let ctl = SaveControl::new();
        ctl.stop().await;
        let outcome = exec::save(db.source(), f.req.clone(), ctl).await;
        assert_eq!(outcome, SaveOutcome::Cancelled);
        assert_eq!(f.count().await, 3);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn meta_reads_keys_nullability_and_generated_columns() {
    for db in databases().await {
        let ddl = match db {
            Db::Pg(_) => "a INT, b TEXT, n INT NOT NULL, g INT GENERATED ALWAYS AS (n * 2) STORED, flag BOOLEAN, data BYTEA, PRIMARY KEY (b, a)",
            Db::My(_) => "a INT, b VARCHAR(10), n INT NOT NULL, g INT GENERATED ALWAYS AS (n * 2) STORED, flag BOOLEAN, data BLOB, PRIMARY KEY (b, a)",
            Db::Lite(..) => "a INT, b TEXT, n INT NOT NULL, g INT GENERATED ALWAYS AS (n * 2) STORED, flag BOOLEAN, data BLOB, PRIMARY KEY (b, a)",
        };
        let table = unique("meta");
        let engine = if matches!(db, Db::My(_)) { " ENGINE=InnoDB" } else { "" };
        db.exec(&format!("CREATE TABLE {} ({ddl}){engine}", db.q(&table))).await;
        let m = db.meta(&table).await;
        assert_eq!(m.primary_key, vec!["b", "a"], "{:?}", db.driver());
        let c = |n: &str| m.columns.iter().find(|c| c.name == n).unwrap().clone();
        assert!(!c("n").nullable && !c("a").nullable);
        assert!(c("g").generated, "{:?}", db.driver());
        assert!(c("flag").is_bool, "{:?}", db.driver());
        assert!(c("data").binary, "{:?}", db.driver());
        assert!(m.transactional);
    }
    if let Some(db) = databases().await.into_iter().find(|d| matches!(d, Db::My(_))) {
        let table = unique("myisam");
        db.exec(&format!("CREATE TABLE {} (id INT PRIMARY KEY) ENGINE=MyISAM", db.q(&table))).await;
        assert!(!db.meta(&table).await.transactional);
    }
    if let Some(db) = databases().await.into_iter().find(|d| matches!(d, Db::Pg(_))) {
        let table = unique("ident");
        db.exec(&format!("CREATE TABLE {} (id INT GENERATED ALWAYS AS IDENTITY PRIMARY KEY, v TEXT)", db.q(&table))).await;
        let m = db.meta(&table).await;
        assert!(m.columns[0].generated && m.columns[0].auto_increment);
        // A view is not editable.
        let view = unique("view");
        db.exec(&format!("CREATE VIEW {} AS SELECT * FROM {}", db.q(&view), db.q(&table))).await;
        assert!(meta::postgres(match &db { Db::Pg(p) => p, _ => unreachable!() }, "", &view).await.is_err());
    }
}

#[test]
fn sqlite_version_compare() {
    assert!(meta::sqlite_version_at_least("3.35.0", (3, 35, 0)));
    assert!(meta::sqlite_version_at_least("3.46.1", (3, 35, 0)));
    assert!(!meta::sqlite_version_at_least("3.34.9", (3, 35, 0)));
    assert!(meta::sqlite_version_at_least("4.0", (3, 35, 0)));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_values_silently_changed_are_refused() {
    let Some(db) = databases().await.into_iter().find(|d| matches!(d, Db::My(_))) else { return };
    let mut f = Fixture::new(&db, "id INT PRIMARY KEY, code VARCHAR(3)", &["(id, code) VALUES (1, 'abc')"], "SELECT * FROM {t}").await;
    f.edit(1, "code", v("abcdef"));
    // Strict mode refuses it outright; without strict mode MySQL would
    // truncate it with a warning, which the save treats as an error too.
    assert!(matches!(f.save().await, SaveOutcome::Failed(_)));
    assert_eq!(f.values("code").await, vec![(1, some("abc"))]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mysql_update_to_the_same_value_is_not_an_error() {
    let Some(db) = databases().await.into_iter().find(|d| matches!(d, Db::My(_))) else { return };
    let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
    // An UPDATE that leaves the row as it was still matched exactly one row.
    f.edit(1, "score", v("10"));
    f.saved().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn script_is_what_open_as_sql_shows() {
    let db = databases().await.remove(0);
    let mut f = Fixture::new(&db, &people(&db), &PEOPLE, "SELECT * FROM {t} ORDER BY id").await;
    f.delete(3);
    f.edit(1, "name", v("O'Neil"));
    f.insert(1, &[("name", v("New"))]);
    let s = f.req.script();
    assert!(s.contains("DELETE FROM"), "{s}");
    assert!(s.contains("'O''Neil'"), "{s}");
    assert!(s.trim_end().ends_with(';'));
    // Running the script by hand does what Save does.
    db.exec(&s.replace(" RETURNING \"id\", \"name\", \"email\", \"score\", \"created\"", "")).await;
    let names: Vec<Option<String>> = f.values("name").await.into_iter().map(|(_, n)| n).collect();
    assert_eq!(names, vec![some("O'Neil"), some("Bob"), some("New")]);
}

// ─── Failure paths, with a scripted connection ──────────────────────────────

/// A connection whose answers are scripted: every statement is recorded, and
/// a statement containing a trigger text fails the chosen way.
struct Scripted {
    driver: Driver,
    log: Vec<String>,
    /// (text the SQL contains, how it fails)
    fail: Vec<(&'static str, Failure)>,
    /// Rows a SELECT / RETURNING gives back.
    rows: Vec<Vec<Value>>,
    affected: u64,
    warning: Option<String>,
}

#[derive(Clone, Copy)]
enum Failure {
    /// The database refuses the statement.
    Refused,
    /// The connection is lost.
    Lost,
}

#[derive(Debug)]
struct Refusal(String);
impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for Refusal {}
impl sqlx::error::DatabaseError for Refusal {
    fn message(&self) -> &str {
        &self.0
    }
    fn as_error(&self) -> &(dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn as_error_mut(&mut self) -> &mut (dyn std::error::Error + Send + Sync + 'static) {
        self
    }
    fn into_error(self: Box<Self>) -> Box<dyn std::error::Error + Send + Sync + 'static> {
        self
    }
    fn kind(&self) -> sqlx::error::ErrorKind {
        sqlx::error::ErrorKind::Other
    }
}

impl Scripted {
    fn new(driver: Driver) -> Scripted {
        Scripted { driver, log: vec![], fail: vec![], rows: vec![], affected: 1, warning: None }
    }

    fn check(&mut self, sql: &str) -> anyhow::Result<()> {
        self.log.push(sql.to_string());
        for (text, how) in &self.fail {
            if sql.contains(text) {
                return Err(match how {
                    Failure::Refused => sqlx::Error::Database(Box::new(Refusal(format!("refused: {sql}")))).into(),
                    Failure::Lost => sqlx::Error::Io(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "connection reset")).into(),
                });
            }
        }
        Ok(())
    }
}

impl exec::Conn for Scripted {
    fn driver(&self) -> Driver {
        self.driver
    }
    fn execute<'a>(&'a mut self, stmt: &'a plan::Stmt) -> futures::future::BoxFuture<'a, anyhow::Result<exec::Done>> {
        Box::pin(async move {
            self.check(&stmt.sql)?;
            Ok(exec::Done { rows_affected: self.affected, last_insert_id: Some(1) })
        })
    }
    fn fetch<'a>(&'a mut self, stmt: &'a plan::Stmt) -> futures::future::BoxFuture<'a, anyhow::Result<Vec<Vec<Value>>>> {
        Box::pin(async move {
            self.check(&stmt.sql)?;
            Ok(self.rows.clone())
        })
    }
    fn simple<'a>(&'a mut self, sql: &'a str) -> futures::future::BoxFuture<'a, anyhow::Result<()>> {
        Box::pin(async move { self.check(sql) })
    }
    fn warnings<'a>(&'a mut self) -> futures::future::BoxFuture<'a, anyhow::Result<Option<String>>> {
        Box::pin(async move { Ok(self.warning.clone()) })
    }
}

/// One edited row of `pg_request()`'s table, read back as stored.
fn scripted_request(driver: Driver) -> (SaveRequest, Scripted) {
    let mut r = with_driver(pg_request(), driver);
    r.check_conflicts = false;
    r.updates = vec![RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, v("b"))] }];
    let mut c = Scripted::new(driver);
    c.rows = vec![vec![json!(1), json!("b"), json!(true)]];
    (r, c)
}

#[tokio::test]
async fn f14_connection_lost_at_commit_is_reported_as_unknown() {
    let (r, mut c) = scripted_request(Driver::Postgres);
    c.fail.push(("COMMIT", Failure::Lost));
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    // Err: the connection must not go back to the pool.
    assert!(matches!(outcome, Err(SaveOutcome::CommitUnknown(_))), "{outcome:?}");
}

#[tokio::test]
async fn a_commit_the_database_refuses_is_a_failure_and_rolled_back() {
    let (r, mut c) = scripted_request(Driver::Postgres);
    c.fail.push(("COMMIT", Failure::Refused));
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    assert!(matches!(&outcome, Ok(SaveOutcome::Failed(f)) if f.action == Action::Commit), "{outcome:?}");
    assert_eq!(c.log.last().map(String::as_str), Some("ROLLBACK"));
}

#[tokio::test]
async fn f13_connection_lost_mid_save_rolls_back_or_closes_the_connection() {
    let (r, mut c) = scripted_request(Driver::Postgres);
    c.fail.push(("UPDATE", Failure::Lost));
    c.fail.push(("ROLLBACK", Failure::Lost));
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    // Rollback failed too: report the failure, and close the connection.
    assert!(matches!(&outcome, Err(SaveOutcome::Failed(f)) if f.action == Action::Update && f.row == Some(RowRef::Data(0))), "{outcome:?}");
    assert!(!c.log.iter().any(|s| s == "COMMIT"));
}

#[tokio::test]
async fn every_failure_ends_in_rollback_never_commit() {
    for step in ["BEGIN", "SET LOCAL", "UPDATE", "COMMIT"] {
        let (r, mut c) = scripted_request(Driver::Postgres);
        c.fail.push((step, Failure::Refused));
        let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
        assert!(matches!(outcome, Ok(SaveOutcome::Failed(_))), "{step}: {outcome:?}");
        let committed = c.log.iter().any(|s| s == "COMMIT") && step != "COMMIT";
        assert!(!committed, "{step}: {:?}", c.log);
        if step != "BEGIN" {
            assert_eq!(c.log.last().map(String::as_str), Some("ROLLBACK"), "{step}");
        }
    }
}

#[tokio::test]
async fn mysql_warnings_fail_the_save() {
    let (r, mut c) = scripted_request(Driver::MySql);
    c.warning = Some("Data truncated for column 'name' at row 1".into());
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    assert!(matches!(&outcome, Ok(SaveOutcome::Failed(f)) if f.message.contains("truncated")), "{outcome:?}");
    assert!(c.log.iter().any(|s| s == "ROLLBACK"));
    // The session setting changed for the save is put back.
    assert!(c.log.last().unwrap().starts_with("SET SESSION innodb_lock_wait_timeout") || c.log.iter().any(|s| s.contains("innodb_lock_wait_timeout = 10")));
}

#[tokio::test]
async fn t04_a_stop_before_commit_rolls_back_instead() {
    let (r, mut c) = scripted_request(Driver::Postgres);
    let ctl = SaveControl::default();
    ctl.stop().await;
    let outcome = exec::run(&mut c, &r, &ctl).await;
    assert_eq!(outcome, Ok(SaveOutcome::Cancelled));
    assert!(!c.log.iter().any(|s| s == "COMMIT"));
}

#[tokio::test]
async fn an_update_matching_two_rows_is_refused() {
    let (r, mut c) = scripted_request(Driver::Postgres);
    c.rows = vec![vec![json!(1), json!("b"), json!(true)], vec![json!(1), json!("b"), json!(true)]];
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    assert!(matches!(&outcome, Ok(SaveOutcome::Failed(f)) if f.message.contains("2 rows")), "{outcome:?}");
    let (r, mut c) = scripted_request(Driver::MySql);
    c.affected = 2;
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    assert!(matches!(&outcome, Ok(SaveOutcome::Failed(f)) if f.message.contains("2 rows")), "{outcome:?}");
}

#[tokio::test]
async fn a_delete_touching_more_rows_than_asked_is_refused() {
    let mut r = with_driver(pg_request(), Driver::Postgres);
    r.check_conflicts = false;
    r.deletes = vec![RowDelete { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)] }];
    let mut c = Scripted::new(Driver::Postgres);
    c.affected = 3;
    let outcome = exec::run(&mut c, &r, &SaveControl::default()).await;
    assert!(matches!(&outcome, Ok(SaveOutcome::Failed(f)) if f.action == Action::Delete), "{outcome:?}");
}

#[tokio::test]
async fn progress_counts_up_to_the_total() {
    let (mut r, mut c) = scripted_request(Driver::Postgres);
    r.check_conflicts = true;
    c.rows = vec![vec![json!(1), json!("a"), json!(true)]];
    let ctl = SaveControl::default();
    // The lock SELECT finds the row unchanged; the UPDATE returns it.
    let outcome = exec::run(&mut c, &r, &ctl).await;
    assert!(matches!(outcome, Ok(SaveOutcome::Saved(_))), "{outcome:?}");
    assert_eq!(ctl.done.load(Ordering::Relaxed), ctl.total.load(Ordering::Relaxed));
    assert_eq!(ctl.total.load(Ordering::Relaxed), 2);
}

// ─── NOW() for date and time columns ────────────────────────────────────────

#[test]
fn now_matches_the_column_type() {
    use super::{now_sql, temporal_kind, Temporal};
    let pg = |t: &str| now_sql(Driver::Postgres, t);
    assert_eq!(pg("date").as_deref(), Some("CURRENT_DATE"));
    assert_eq!(pg("timestamp without time zone").as_deref(), Some("now()"));
    assert_eq!(pg("timestamp(3) with time zone").as_deref(), Some("now()"));
    assert_eq!(pg("time without time zone").as_deref(), Some("LOCALTIME"));
    assert_eq!(pg("time(0) with time zone").as_deref(), Some("CURRENT_TIME"));
    assert_eq!(pg("interval"), None);
    assert_eq!(pg("text"), None);
    assert_eq!(pg("public.timestamp_ish"), None);
    let my = |t: &str| now_sql(Driver::MySql, t);
    assert_eq!(my("date").as_deref(), Some("CURDATE()"));
    assert_eq!(my("datetime").as_deref(), Some("NOW()"));
    assert_eq!(my("datetime(3)").as_deref(), Some("NOW(3)"));
    assert_eq!(my("timestamp(6)").as_deref(), Some("NOW(6)"));
    assert_eq!(my("time(2)").as_deref(), Some("CURTIME(2)"));
    assert_eq!(my("year"), None);
    assert_eq!(my("varchar(20)"), None);
    let lite = |t: &str| now_sql(Driver::Sqlite, t);
    assert_eq!(lite("DATETIME").as_deref(), Some("CURRENT_TIMESTAMP"));
    assert_eq!(lite("timestamp").as_deref(), Some("CURRENT_TIMESTAMP"));
    assert_eq!(lite("DATE").as_deref(), Some("CURRENT_DATE"));
    assert_eq!(lite("TIME").as_deref(), Some("CURRENT_TIME"));
    assert_eq!(lite("TEXT"), None);
    assert_eq!(temporal_kind(Driver::MySql, "DATETIME(3)"), Some(Temporal::DateTime));
}

#[test]
fn now_is_written_as_sql_not_a_parameter() {
    let mut r = pg_request();
    r.columns.push(Some(ColumnMeta { name: "seen".into(), data_type: "timestamp with time zone".into(), nullable: true, ..Default::default() }));
    let u = RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true), Value::Null], sets: vec![(3, Cell::Now)] };
    let s = r.update(&u);
    assert!(s.sql.starts_with("UPDATE \"public\".\"people\" SET \"seen\" = now() WHERE"), "{}", s.sql);
    assert_eq!(s.binds.len(), 1, "only the key is bound");
    let ins = RowInsert { row: RowRef::New(1), cells: vec![(1, v("x")), (3, Cell::Now)] };
    assert!(r.insert(&ins).sql.contains("VALUES (CAST($1 AS text), now())"), "{}", r.insert(&ins).sql);
    r.updates = vec![u];
    r.validate().unwrap();
}

#[test]
fn now_is_refused_for_non_date_and_key_columns() {
    let mut r = pg_request();
    r.updates = vec![RowUpdate { row: RowRef::Data(0), original: vec![json!(1), json!("a"), json!(true)], sets: vec![(1, Cell::Now)] }];
    assert!(r.validate().unwrap_err().1.contains("not a date or time column"));
    let mut r = pg_request();
    r.columns[0].as_mut().unwrap().data_type = "date".into();
    r.inserts = vec![RowInsert { row: RowRef::New(1), cells: vec![(0, Cell::Now)] }];
    assert!(r.validate().unwrap_err().1.contains("key column"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn now_sets_the_databases_current_date_and_time() {
    for db in databases().await {
        let ddl = match db {
            Db::Pg(_) => "id INT PRIMARY KEY, d DATE, ts TIMESTAMP, tz TIMESTAMPTZ, t TIME, note TEXT",
            Db::My(_) => "id INT PRIMARY KEY, d DATE, ts DATETIME(3), tz TIMESTAMP NULL, t TIME, note VARCHAR(10)",
            Db::Lite(..) => "id INTEGER PRIMARY KEY, d DATE, ts DATETIME, tz TIMESTAMP, t TIME, note TEXT",
        };
        let mut f = Fixture::new(&db, ddl, &["(id, note) VALUES (1, 'a')"], "SELECT * FROM {t}").await;
        let before = chrono::Utc::now();
        for c in ["d", "ts", "tz", "t"] {
            f.edit(1, c, Cell::Now);
        }
        f.insert(1, &[("id", v("2")), ("d", Cell::Now), ("ts", Cell::Now)]);
        let report = f.saved().await;
        let row = &report.updated[0].1;
        for c in ["d", "ts", "tz", "t"] {
            assert_ne!(row[f.col(c)], Value::Null, "{:?}: {c}", db.driver());
        }
        // The date is today's (allowing for midnight in either the database's
        // zone or UTC).
        let d = row[f.col("d")].as_str().unwrap().to_string();
        let today = |off: i64| (before + chrono::Duration::days(off)).format("%Y-%m-%d").to_string();
        assert!([today(-1), today(0), today(1)].contains(&d), "{:?}: {d}", db.driver());
        // The timestamp is within a day of now, whatever the zone.
        let ts = row[f.col("ts")].as_str().unwrap();
        assert!(ts.starts_with(&today(0)[..4]), "{:?}: {ts}", db.driver());
        let (_, ins) = &report.inserted[0];
        assert_ne!(ins[f.col("d")], Value::Null);
        assert_ne!(ins[f.col("ts")], Value::Null);
        assert_eq!(ins[f.col("t")], Value::Null);
    }
}
