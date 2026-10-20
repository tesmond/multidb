//! Whether a query's result can be edited, and how its columns map onto the
//! table it reads.
//!
//! Only a plain read of one table qualifies:
//! `SELECT <*, columns or expressions> FROM <table> [WHERE] [ORDER BY] [LIMIT]`.
//! Anything that could make a result row differ from one table row (joins,
//! grouping, DISTINCT, set operations, CTEs) makes the result read-only, as
//! does anything that would make it ambiguous which table column a result
//! column shows. Getting this wrong would write to the wrong row or column,
//! so every doubt resolves to read-only.

use super::meta::TableMeta;
use super::Driver;
use regex::Regex;
use sqlparser::ast::{
    Distinct, Expr, GroupByExpr, Ident, ObjectName, ObjectNamePart, SelectItem, SelectItemQualifiedWildcardKind, SetExpr,
    Statement, TableFactor,
};
use sqlparser::dialect::{Dialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
use sqlparser::parser::Parser;
use std::sync::LazyLock;

/// One item of the SELECT list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Item {
    /// `*` or `t.*`.
    Wildcard,
    /// A plain column reference, with the alias it was given if any.
    Column { name: String, alias: Option<String> },
    /// Anything else; shown, never edited.
    Expr { alias: Option<String> },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedSelect {
    /// The schema as written, or empty.
    pub schema: String,
    pub table: String,
    pub items: Vec<Item>,
}

fn dialect(driver: Driver) -> Box<dyn Dialect> {
    match driver {
        Driver::Postgres => Box::new(PostgreSqlDialect {}),
        Driver::MySql => Box::new(MySqlDialect {}),
        Driver::Sqlite => Box::new(SQLiteDialect {}),
    }
}

/// An identifier as the database stores it: Postgres folds unquoted names to
/// lower case. MySQL and SQLite compare case-insensitively, so their names
/// are kept as written.
fn ident_name(driver: Driver, ident: &Ident) -> String {
    if driver == Driver::Postgres && ident.quote_style.is_none() {
        ident.value.to_lowercase()
    } else {
        ident.value.clone()
    }
}

fn object_parts(driver: Driver, name: &ObjectName) -> Option<Vec<String>> {
    name.0
        .iter()
        .map(|part| match part {
            ObjectNamePart::Identifier(i) => Some(ident_name(driver, i)),
            _ => None,
        })
        .collect()
}

static AGGREGATE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(count|sum|avg|min|max|array_agg|string_agg|group_concat|json_agg|jsonb_agg|json_object_agg|jsonb_object_agg|json_arrayagg|json_objectagg|json_group_array|json_group_object|bool_and|bool_or|every|bit_and|bit_or|bit_xor|stddev|stddev_pop|stddev_samp|std|variance|var_pop|var_samp|xmlagg|total|percentile_cont|percentile_disc|mode|any_value)\s*\(|\bover\s*\(|\bover\s+[a-z_]",
    )
    .unwrap()
});

/// Parse `sql` and check it is a plain read of one table. The error is the
/// reason to show for the result being read-only.
pub fn parse(sql: &str, driver: Driver) -> Result<ParsedSelect, String> {
    let d = dialect(driver);
    let statements = Parser::parse_sql(d.as_ref(), sql).map_err(|_| "the query could not be parsed".to_string())?;
    let [statement] = statements.as_slice() else {
        return Err("more than one statement".into());
    };
    let Statement::Query(query) = statement else {
        return Err("not a SELECT".into());
    };
    if query.with.is_some() {
        return Err("the query uses WITH".into());
    }
    if !query.locks.is_empty() || query.for_clause.is_some() {
        return Err("the query locks rows (FOR UPDATE / FOR SHARE)".into());
    }
    if query.settings.is_some() || query.format_clause.is_some() || !query.pipe_operators.is_empty() {
        return Err("the query uses unsupported clauses".into());
    }
    let SetExpr::Select(select) = query.body.as_ref() else {
        return Err("the query is not a single SELECT (UNION, VALUES, …)".into());
    };
    match &select.distinct {
        None | Some(Distinct::All) => {}
        Some(_) => return Err("the query uses DISTINCT".into()),
    }
    if select.top.is_some() || select.select_modifiers.is_some() {
        return Err("the query uses SELECT modifiers".into());
    }
    if select.into.is_some() {
        return Err("the query uses SELECT … INTO".into());
    }
    if select.exclude.is_some() {
        return Err("the query uses EXCLUDE".into());
    }
    match &select.group_by {
        GroupByExpr::Expressions(exprs, modifiers) if exprs.is_empty() && modifiers.is_empty() => {}
        _ => return Err("the query uses GROUP BY".into()),
    }
    if select.having.is_some() || select.qualify.is_some() || !select.named_window.is_empty() {
        return Err("the query uses HAVING, QUALIFY or WINDOW".into());
    }
    if !select.lateral_views.is_empty()
        || select.prewhere.is_some()
        || !select.connect_by.is_empty()
        || !select.cluster_by.is_empty()
        || !select.distribute_by.is_empty()
        || !select.sort_by.is_empty()
        || select.value_table_mode.is_some()
    {
        return Err("the query uses unsupported clauses".into());
    }
    let [from] = select.from.as_slice() else {
        return Err(if select.from.is_empty() { "the query reads no table".into() } else { "the query reads more than one table".into() });
    };
    if !from.joins.is_empty() {
        return Err("the query uses a JOIN".into());
    }
    let TableFactor::Table { name, alias, args, with_hints, version, partitions, json_path, sample, .. } = &from.relation else {
        return Err("the query does not read a plain table".into());
    };
    if args.is_some() || !with_hints.is_empty() || version.is_some() || !partitions.is_empty() || json_path.is_some() || sample.is_some() {
        return Err("the query does not read a plain table".into());
    }
    let parts = object_parts(driver, name).ok_or_else(|| "the table name could not be read".to_string())?;
    let (schema, table) = match parts.as_slice() {
        [t] => (String::new(), t.clone()),
        [s, t] => (s.clone(), t.clone()),
        _ => return Err("three-part table names are not supported".into()),
    };
    let alias_name = match alias {
        Some(a) => {
            if !a.columns.is_empty() {
                return Err("the table alias renames columns".into());
            }
            Some(ident_name(driver, &a.name))
        }
        None => None,
    };
    // A qualifier names this table: its alias if it has one, otherwise its
    // name, optionally schema-qualified.
    let qualifies = |q: &[String]| -> bool {
        match (q, &alias_name) {
            ([a], Some(alias)) => driver.same_name(a, alias),
            ([t], None) => driver.same_name(t, &table),
            ([s, t], None) => driver.same_name(t, &table) && !schema.is_empty() && driver.same_name(s, &schema),
            _ => false,
        }
    };
    let column_ref = |expr: &Expr| -> Result<Option<String>, String> {
        match expr {
            Expr::Identifier(i) => Ok(Some(ident_name(driver, i))),
            Expr::CompoundIdentifier(ids) if ids.len() >= 2 => {
                let names: Vec<String> = ids.iter().map(|i| ident_name(driver, i)).collect();
                let (col, qual) = names.split_last().unwrap();
                if qualifies(qual) {
                    Ok(Some(col.clone()))
                } else {
                    Err(format!("the column {} does not refer to the table", ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(".")))
                }
            }
            _ => Ok(None),
        }
    };
    let mut items = Vec::with_capacity(select.projection.len());
    for item in &select.projection {
        match item {
            SelectItem::Wildcard(_) => {
                if item.to_string().trim() != "*" {
                    return Err("the query uses * with options".into());
                }
                items.push(Item::Wildcard);
            }
            SelectItem::QualifiedWildcard(kind, opts) => {
                let SelectItemQualifiedWildcardKind::ObjectName(obj) = kind else {
                    return Err("the query uses an unsupported wildcard".into());
                };
                let q = object_parts(driver, obj).ok_or_else(|| "the query uses an unsupported wildcard".to_string())?;
                if !qualifies(&q) || !opts.to_string().trim().is_empty() {
                    return Err("the query uses an unsupported wildcard".into());
                }
                items.push(Item::Wildcard);
            }
            SelectItem::UnnamedExpr(expr) => {
                if AGGREGATE.is_match(&expr.to_string()) {
                    return Err("the query uses an aggregate or window function".into());
                }
                items.push(match column_ref(expr)? {
                    Some(name) => Item::Column { name, alias: None },
                    None => Item::Expr { alias: None },
                });
            }
            SelectItem::ExprWithAlias { expr, alias } => {
                if AGGREGATE.is_match(&expr.to_string()) {
                    return Err("the query uses an aggregate or window function".into());
                }
                let alias = Some(ident_name(driver, alias));
                items.push(match column_ref(expr)? {
                    Some(name) => Item::Column { name, alias },
                    None => Item::Expr { alias },
                });
            }
            SelectItem::ExprWithAliases { .. } => return Err("the query uses multiple aliases".into()),
        }
    }
    Ok(ParsedSelect { schema, table, items })
}

/// How the result's columns map onto the table, for an editable result.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Layout {
    /// Per result column, the table column (index into `TableMeta::columns`)
    /// it shows, or `None` for an expression.
    pub sources: Vec<Option<usize>>,
    /// Per result column, why it can't be edited; `None` if it can.
    pub readonly: Vec<Option<String>>,
    /// The result column holding each primary key column, in key order.
    pub key: Vec<usize>,
}

/// Match the result's columns to the table. `result_columns` are the names
/// the database reported for the result, which must line up exactly with
/// what the SELECT list says — if they don't, something is not understood
/// and the result stays read-only.
pub fn layout(driver: Driver, parsed: &ParsedSelect, result_columns: &[String], meta: &TableMeta) -> Result<Layout, String> {
    if meta.primary_key.is_empty() {
        return Err("the table has no primary key".into());
    }
    // (source column, the name the result should report for it)
    let mut expanded: Vec<(Option<usize>, Option<String>)> = Vec::new();
    for item in &parsed.items {
        match item {
            Item::Wildcard => {
                for (i, c) in meta.columns.iter().enumerate() {
                    if !c.hidden {
                        expanded.push((Some(i), Some(c.name.clone())));
                    }
                }
            }
            Item::Column { name, alias } => {
                let Some(src) = meta.column(driver, name) else {
                    return Err(format!("column \"{name}\" is not a column of the table"));
                };
                if let Some(alias) = alias {
                    if !driver.same_name(alias, &meta.columns[src].name) && meta.column(driver, alias).is_some() {
                        return Err(format!("the alias \"{alias}\" hides the table's own \"{alias}\" column"));
                    }
                }
                expanded.push((Some(src), Some(alias.clone().unwrap_or_else(|| meta.columns[src].name.clone()))));
            }
            Item::Expr { alias } => expanded.push((None, alias.clone())),
        }
    }
    if expanded.len() != result_columns.len() {
        return Err("the result's columns don't match the table's".into());
    }
    for ((src, expected), actual) in expanded.iter().zip(result_columns) {
        if src.is_some() {
            if let Some(expected) = expected {
                if !driver.same_name(expected, actual) {
                    return Err("the result's columns don't match the table's".into());
                }
            }
        }
    }
    let sources: Vec<Option<usize>> = expanded.iter().map(|(s, _)| *s).collect();
    let mut key = Vec::with_capacity(meta.primary_key.len());
    for pk in &meta.primary_key {
        let Some(src) = meta.column(driver, pk) else {
            return Err(format!("primary key column \"{pk}\" is missing from the table"));
        };
        let Some(pos) = sources.iter().position(|s| *s == Some(src)) else {
            return Err(format!("the primary key column \"{pk}\" is not in the result"));
        };
        key.push(pos);
    }
    let readonly = sources
        .iter()
        .enumerate()
        .map(|(i, src)| {
            let Some(src) = src else { return Some("this column is an expression, not a table column".to_string()) };
            if sources[..i].contains(&Some(*src)) {
                return Some("this column appears more than once; edit its first occurrence".to_string());
            }
            let col = &meta.columns[*src];
            if col.generated {
                return Some("generated column".to_string());
            }
            if col.binary {
                return Some("binary and spatial values can't be edited here".to_string());
            }
            None
        })
        .collect();
    Ok(Layout { sources, readonly, key })
}
