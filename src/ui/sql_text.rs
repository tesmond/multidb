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

/// An identifier as it can be written in a query: bare when the database
/// would read the bare word back as the same name, quoted otherwise.
pub fn ident(name: &str, driver: &str) -> String {
    let mut chars = name.chars();
    let starts_ok = chars.next().is_some_and(|c| c.is_ascii_alphabetic() || c == '_');
    // Postgres folds bare identifiers to lower case.
    let rest_ok = name
        .chars()
        .all(|c| c.is_ascii_digit() || c == '_' || c.is_ascii_lowercase() || (c.is_ascii_uppercase() && driver != "postgres"));
    if starts_ok && rest_ok && !crate::ui::sql::tokenizer::is_sql_keyword(name) {
        name.to_string()
    } else {
        quote_identifier(name, driver)
    }
}

/// `INSERT INTO t (a, b) VALUES (?, ?);` naming every column.
pub fn insert_template(driver: &str, table: &str, columns: &[String]) -> String {
    let names: Vec<String> = columns.iter().map(|c| ident(c, driver)).collect();
    let marks = vec!["?"; names.len()];
    format!("INSERT INTO {} ({})\nVALUES ({});\n", ident(table, driver), names.join(", "), marks.join(", "))
}

/// `UPDATE t SET a = ?, b = ? WHERE pk = ?;`: the primary key columns pick
/// the row and every other column is set. A table without a primary key sets
/// every column and matches the row on all of them.
pub fn update_template(driver: &str, table: &str, columns: &[String], primary_key: &[String]) -> String {
    let assign = |c: &String| format!("{} = ?", ident(c, driver));
    let (set, keys): (Vec<&String>, Vec<&String>) = if primary_key.is_empty() {
        (columns.iter().collect(), columns.iter().collect())
    } else {
        let set: Vec<&String> = columns.iter().filter(|c| !primary_key.contains(c)).collect();
        // A table that is all key still needs something to set.
        let set = if set.is_empty() { columns.iter().collect() } else { set };
        (set, primary_key.iter().collect())
    };
    format!(
        "UPDATE {}\nSET {}\nWHERE {};\n",
        ident(table, driver),
        set.into_iter().map(assign).collect::<Vec<_>>().join(",\n    "),
        keys.into_iter().map(assign).collect::<Vec<_>>().join("\n  AND "),
    )
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
    #[test]
    fn ident_quotes_only_when_needed() {
        assert_eq!(super::ident("users", "postgres"), "users");
        assert_eq!(super::ident("Users", "postgres"), "\"Users\"");
        assert_eq!(super::ident("Users", "mysql"), "Users");
        assert_eq!(super::ident("order", "mysql"), "`order`");
        assert_eq!(super::ident("user", "postgres"), "\"user\"");
        assert_eq!(super::ident("2fa", "sqlite"), "\"2fa\"");
        assert_eq!(super::ident("first name", "postgres"), "\"first name\"");
    }

    #[test]
    fn insert_and_update_templates() {
        let cols: Vec<String> = ["id", "name", "order"].iter().map(|s| s.to_string()).collect();
        assert_eq!(
            super::insert_template("postgres", "items", &cols),
            "INSERT INTO items (id, name, \"order\")\nVALUES (?, ?, ?);\n"
        );
        assert_eq!(
            super::update_template("mysql", "items", &cols, &["id".to_string()]),
            "UPDATE items\nSET name = ?,\n    `order` = ?\nWHERE id = ?;\n"
        );
        assert_eq!(
            super::update_template("sqlite", "items", &cols[..2], &[]),
            "UPDATE items\nSET id = ?,\n    name = ?\nWHERE id = ?\n  AND name = ?;\n"
        );
    }

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
