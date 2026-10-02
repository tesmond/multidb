//! Small SQL text helpers ported from the Svelte stores/components.

use regex::Regex;
use serde_json::Value;
use std::sync::LazyLock;

static WS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\s+").unwrap());

/// `extractFirstTableName` from appStore.ts – first table after FROM/JOIN.
pub fn extract_first_table_name(sql: &str) -> Option<String> {
    if sql.is_empty() {
        return None;
    }
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r#"(?i-u)\b(?:FROM|JOIN)\s+(?:["'`\[]?[\w-]+["'`\]]?\s*\.\s*)?["'`\[]?([\w-]+)["'`\]]?"#,
        )
        .unwrap()
    });
    let normalized = WS.replace_all(sql, " ");
    let normalized = normalized.trim();
    RE.captures(normalized).map(|c| c[1].to_string())
}

/// `isDDL` from SqlEditor.svelte.
pub fn is_ddl(sql: &str) -> bool {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?im-u)^\s*(?:CREATE|DROP|ALTER|RENAME|TRUNCATE|COMMENT\s+ON)\b").unwrap()
    });
    RE.is_match(sql)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimpleSelect {
    pub table_name: String,
    pub schema_name: String,
}

/// `parseSimpleSelect` from OutputPanel.svelte.
pub fn parse_simple_select(sql: &str) -> Option<SimpleSelect> {
    static SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i-u)^SELECT\b").unwrap());
    static JOIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i-u)\bJOIN\b").unwrap());
    static SUBQ: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i-u)\bFROM\s*\(").unwrap());
    static FROM: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i-u)\bFROM\s+(?:([`"\w\[\]]+)\s*\.\s*)?([`"\w\[\]]+)"#).unwrap()
    });
    let s = WS.replace_all(sql, " ");
    let s = s.trim();
    if !SELECT.is_match(s) || JOIN.is_match(s) || SUBQ.is_match(s) {
        return None;
    }
    let m = FROM.captures(s)?;
    let clean = |t: &str| t.replace(['`', '"', '[', ']'], "");
    let first = m.get(1).map(|g| g.as_str()).unwrap_or("");
    let second = m.get(2).map(|g| g.as_str());
    Some(match second {
        Some(table) => SimpleSelect {
            schema_name: clean(first),
            table_name: clean(table),
        },
        None => SimpleSelect {
            schema_name: String::new(),
            table_name: clean(first),
        },
    })
}

/// Identifier quoting used by the navigator (no escaping, as before).
pub fn quote_identifier_plain(name: &str, driver: &str) -> String {
    if driver == "mysql" {
        format!("`{name}`")
    } else {
        format!("\"{name}\"")
    }
}

/// Identifier quoting with escaping (OutputPanel / relationship viewer).
pub fn quote_identifier(name: &str, driver: &str) -> String {
    if driver == "mysql" {
        format!("`{}`", name.replace('`', "``"))
    } else {
        format!("\"{}\"", name.replace('"', "\"\""))
    }
}

pub fn qualify_table(driver: &str, table: &str, schema: Option<&str>) -> String {
    match schema {
        Some(schema) if !schema.is_empty() => format!(
            "{}.{}",
            quote_identifier_plain(schema, driver),
            quote_identifier_plain(table, driver)
        ),
        _ => table.to_string(),
    }
}

/// `quoteValue` from OutputPanel.svelte. Pending edits are always strings;
/// original values may be any JSON value.
pub fn quote_value(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Number(_) => super::js::value_to_string(value),
        other => format!("'{}'", super::js::value_to_string(other).replace('\'', "''")),
    }
}

pub struct EditTarget<'a> {
    pub driver: &'a str,
    pub table_name: &'a str,
    pub schema_name: &'a str,
    pub primary_key_cols: &'a [String],
}

/// `generateUpdateSQL` from OutputPanel.svelte. `edits` is (row index, [(col index, new value)])
/// in insertion order.
pub fn generate_update_sql(
    target: &EditTarget,
    columns: &[String],
    rows: &[Vec<Value>],
    edits: &[(usize, Vec<(usize, String)>)],
) -> String {
    let qi = |n: &str| quote_identifier(n, target.driver);
    let full_table = if target.schema_name.is_empty() {
        qi(target.table_name)
    } else {
        format!("{}.{}", qi(target.schema_name), qi(target.table_name))
    };
    let mut lines = Vec::new();
    for (row_idx, col_edits) in edits {
        let Some(row) = rows.get(*row_idx) else { continue };
        let set_clauses: Vec<String> = col_edits
            .iter()
            .map(|(col_idx, value)| {
                let col = columns.get(*col_idx).map(String::as_str).unwrap_or("undefined");
                format!("{} = {}", qi(col), quote_value(&Value::String(value.clone())))
            })
            .collect();
        let where_clauses: Vec<String> = target
            .primary_key_cols
            .iter()
            .map(|pk| {
                let val = columns
                    .iter()
                    .position(|c| c == pk)
                    .and_then(|idx| row.get(idx))
                    .cloned()
                    .unwrap_or(Value::Null);
                format!("{} = {}", qi(pk), quote_value(&val))
            })
            .collect();
        if !set_clauses.is_empty() && !where_clauses.is_empty() {
            lines.push(format!(
                "UPDATE {full_table}\nSET {}\nWHERE {};",
                set_clauses.join(",\n    "),
                where_clauses.join(" AND ")
            ));
        }
    }
    lines.join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn extracts_first_table() {
        assert_eq!(extract_first_table_name("select * from public.users u"), Some("users".into()));
        assert_eq!(extract_first_table_name("SELECT 1"), None);
        assert_eq!(extract_first_table_name("SELECT * FROM `shop`.`orders`"), Some("orders".into()));
    }

    #[test]
    fn detects_ddl() {
        assert!(is_ddl("  create table x (id int)"));
        assert!(is_ddl("SELECT 1;\nDROP TABLE x"));
        assert!(!is_ddl("SELECT * FROM created"));
    }

    #[test]
    fn parses_simple_select() {
        assert_eq!(
            parse_simple_select("SELECT * FROM \"public\".\"users\" LIMIT 100;"),
            Some(SimpleSelect { table_name: "users".into(), schema_name: "public".into() })
        );
        assert_eq!(
            parse_simple_select("select * from orders"),
            Some(SimpleSelect { table_name: "orders".into(), schema_name: String::new() })
        );
        assert_eq!(parse_simple_select("SELECT * FROM a JOIN b ON 1=1"), None);
    }

    #[test]
    fn generates_update_sql() {
        let sql = generate_update_sql(
            &EditTarget { driver: "postgres", table_name: "users", schema_name: "public", primary_key_cols: &["id".into()] },
            &["id".into(), "name".into()],
            &[vec![json!(7), json!("Ann")]],
            &[(0, vec![(1, "O'Neil".into())])],
        );
        assert_eq!(sql, "UPDATE \"public\".\"users\"\nSET \"name\" = 'O''Neil'\nWHERE \"id\" = 7;");
    }
}
