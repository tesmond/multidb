//! Editor diagnostics: common syntax slips, schema-aware identifier checks
//! (ports of `findSqlCommonSyntaxDiagnostics` / `findSqlSemanticDiagnostics`)
//! and parser-error detection equivalent to lang-sql's Lezer error nodes.

use super::complete::{is_reserved, normalize_ident};
use super::schema::DbSchema;
use super::tokenizer::{Dialect, Tok, Token};
use regex::Regex;
use std::collections::HashSet;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub from: usize,
    pub to: usize,
    pub message: String,
}

fn diag(from: usize, to: usize, message: impl Into<String>) -> Diagnostic {
    Diagnostic { from, to, message: message.into() }
}

/// Run every check in the same order the old linter did.
pub fn lint(doc: &str, dialect: Dialect, db: Option<&DbSchema>) -> Vec<Diagnostic> {
    let mut out = common_syntax(doc);
    if let Some(db) = db {
        out.extend(semantic(doc, db));
    }
    let tokens = super::tokenize(doc, dialect);
    for (from, to) in parser_errors(doc, &tokens) {
        let to = if to > from { to } else { next_char_boundary(doc, from) };
        if is_ignorable_parser_error(doc, from, to) {
            continue;
        }
        out.push(diag(from, to, "SQL syntax error"));
    }
    out
}

fn next_char_boundary(doc: &str, from: usize) -> usize {
    doc[from..].chars().next().map(|c| from + c.len_utf8()).unwrap_or(doc.len())
}

/// Error spans comparable to Lezer's recovery for the lang-sql grammar:
/// untokenizable characters, dots not followed by an identifier,
/// unmatched closing brackets and unclosed opening brackets.
pub fn parser_errors(doc: &str, tokens: &[Token]) -> Vec<(usize, usize)> {
    let mut errors = Vec::new();
    let sig: Vec<&Token> = super::tokenizer::significant(tokens).collect();
    let mut stack: Vec<Tok> = Vec::new();
    for (k, t) in sig.iter().enumerate() {
        match t.kind {
            Tok::Invalid => errors.push((t.from, t.to)),
            Tok::Dot => {
                let prev_ok = k > 0 && matches!(sig[k - 1].kind, Tok::Identifier | Tok::QuotedIdentifier);
                let next = sig.get(k + 1);
                let next_ok = matches!(next.map(|n| n.kind), Some(Tok::Identifier) | Some(Tok::QuotedIdentifier));
                if prev_ok && !next_ok {
                    let at = next.map(|n| n.from).unwrap_or(doc.len());
                    errors.push((at, at));
                } else if !prev_ok {
                    errors.push((t.from, t.to));
                }
            }
            Tok::ParenL | Tok::BraceL | Tok::BracketL => stack.push(t.kind),
            Tok::ParenR | Tok::BraceR | Tok::BracketR => {
                let open = match t.kind {
                    Tok::ParenR => Tok::ParenL,
                    Tok::BraceR => Tok::BraceL,
                    _ => Tok::BracketL,
                };
                if stack.last() == Some(&open) {
                    stack.pop();
                } else {
                    errors.push((t.from, t.to));
                }
            }
            Tok::Semi => {
                if !stack.is_empty() {
                    errors.push((t.from, t.from));
                    stack.clear();
                }
            }
            _ => {}
        }
    }
    if !stack.is_empty() {
        errors.push((doc.len(), doc.len()));
    }
    errors
}

/// `isIgnorableSqlParserError` from sqlComplete.ts.
pub fn is_ignorable_parser_error(doc: &str, from: usize, to: usize) -> bool {
    let token = &doc[from..to];
    if token == "/" {
        let before = doc[..from].trim_end().chars().last();
        let after = doc[to..].trim_start().chars().next();
        let before_ok = before.is_some_and(|c| c.is_ascii_alphanumeric() || "_$.)]".contains(c));
        let after_ok = after.is_some_and(|c| c.is_ascii_alphanumeric() || "_$([+-".contains(c));
        return before_ok && after_ok;
    }
    if token != "*" {
        return false;
    }
    static RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)[A-Za-z_][\w$]*\s*\.\s*$").unwrap());
    static TAIL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)[A-Za-z_]\s*\.\s*$").unwrap());
    let before = RE.find(&doc[..from]).map(|m| m.as_str()).unwrap_or("");
    TAIL.is_match(before)
}

#[derive(Clone, Copy, PartialEq)]
enum Quote {
    Single,
    Double,
    Backtick,
}

fn update_quote(q: &mut Option<Quote>, ch: char) -> bool {
    if let Some(cur) = *q {
        if (cur == Quote::Single && ch == '\'') || (cur == Quote::Double && ch == '"') || (cur == Quote::Backtick && ch == '`') {
            *q = None;
        }
        return true;
    }
    false
}

/// `findSqlCommonSyntaxDiagnostics`.
pub fn common_syntax(sql: &str) -> Vec<Diagnostic> {
    static SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)(?-u:\b)SELECT(?-u:\b)(.*?)(?-u:\b)FROM(?-u:\b)").unwrap());
    let mut out = Vec::new();
    for m in SELECT.captures_iter(sql) {
        let body = m.get(1).unwrap();
        let body_start = body.start();
        let body_text = body.as_str();
        let mut depth = 0i32;
        let mut quote: Option<Quote> = None;
        for (i, ch) in body_text.char_indices() {
            if update_quote(&mut quote, ch) {
                continue;
            }
            match ch {
                '\'' => quote = Some(Quote::Single),
                '"' => quote = Some(Quote::Double),
                '`' => quote = Some(Quote::Backtick),
                '(' => depth += 1,
                ')' if depth > 0 => depth -= 1,
                ',' if depth == 0 => {
                    let tail = &body_text[i + 1..];
                    if tail.trim().is_empty() {
                        out.push(diag(body_start + i, body_start + i + 1, "Trailing comma in SELECT list"));
                        continue;
                    }
                    if let Some(tok) = tail.split_whitespace().next() {
                        if tok.to_uppercase() == "FROM" {
                            out.push(diag(body_start + i, body_start + i + 1, "Trailing comma in SELECT list"));
                        }
                    }
                }
                _ => {}
            }
        }
    }
    out
}

fn split_select_expressions(text: &str) -> Vec<(String, usize, usize)> {
    let mut parts = Vec::new();
    let mut start = 0;
    let mut depth = 0i32;
    let mut quote: Option<Quote> = None;
    let push = |parts: &mut Vec<(String, usize, usize)>, s: usize, e: usize| {
        let raw = &text[s..e];
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            let leading = raw.len() - raw.trim_start().len();
            parts.push((trimmed.to_string(), s + leading, e));
        }
    };
    for (i, ch) in text.char_indices() {
        if update_quote(&mut quote, ch) {
            continue;
        }
        match ch {
            '\'' => quote = Some(Quote::Single),
            '"' => quote = Some(Quote::Double),
            '`' => quote = Some(Quote::Backtick),
            '(' => depth += 1,
            ')' if depth > 0 => depth -= 1,
            ',' if depth == 0 => {
                push(&mut parts, start, i);
                start = i + 1;
            }
            _ => {}
        }
    }
    push(&mut parts, start, text.len());
    parts
}

struct TableRef {
    table: String,
    columns: Option<HashSet<String>>,
}

fn flatten(db: &DbSchema) -> (HashSet<String>, std::collections::HashMap<String, HashSet<String>>, std::collections::HashMap<String, HashSet<String>>) {
    let mut schemas = HashSet::new();
    let mut schema_tables: std::collections::HashMap<String, HashSet<String>> = Default::default();
    let mut table_columns: std::collections::HashMap<String, HashSet<String>> = Default::default();
    match &db.schemas {
        Some(list) if !list.is_empty() => {
            for s in list {
                let sn = s.name.to_lowercase();
                schemas.insert(sn.clone());
                let set = schema_tables.entry(sn).or_default();
                for t in &s.tables {
                    let tn = t.name.to_lowercase();
                    set.insert(tn.clone());
                    table_columns
                        .entry(tn)
                        .or_insert_with(|| t.columns.iter().map(|c| c.name.to_lowercase()).collect());
                }
            }
        }
        _ => {
            for t in db.all_flat_tables() {
                table_columns.insert(t.name.to_lowercase(), t.columns.iter().map(|c| c.name.to_lowercase()).collect());
            }
        }
    }
    (schemas, schema_tables, table_columns)
}

fn system_schema(driver: &str, schema: &str) -> bool {
    match driver {
        "mysql" => ["information_schema", "mysql", "performance_schema", "sys"].contains(&schema),
        "postgres" => ["information_schema", "pg_catalog"].contains(&schema),
        _ => false,
    }
}

fn system_table_columns(driver: &str, schema: &str, table: &str) -> Option<HashSet<String>> {
    if schema != "information_schema" || table != "tables" {
        return None;
    }
    let cols: &[&str] = match driver {
        "mysql" => &[
            "table_catalog", "table_schema", "table_name", "table_type", "engine", "version", "row_format", "table_rows",
            "avg_row_length", "data_length", "max_data_length", "index_length", "data_free", "auto_increment",
            "create_time", "update_time", "check_time", "table_collation", "checksum", "create_options", "table_comment",
        ],
        "postgres" => &[
            "table_catalog", "table_schema", "table_name", "table_type", "self_referencing_column_name",
            "reference_generation", "user_defined_type_catalog", "user_defined_type_schema", "user_defined_type_name",
            "is_insertable_into", "is_typed", "commit_action",
        ],
        _ => return None,
    };
    Some(cols.iter().map(|s| s.to_string()).collect())
}

fn resolve_table_refs(sql: &str, db: &DbSchema, out: &mut Vec<Diagnostic>) -> Vec<TableRef> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r"(?i-u)\b(?:FROM|JOIN)\s+([A-Za-z_][\w$]*)(?:\s*\.\s*([A-Za-z_][\w$]*))?").unwrap()
    });
    static IDENT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)[A-Za-z_][\w$]*").unwrap());
    let (schemas, schema_tables, table_columns) = flatten(db);
    let mut refs = Vec::new();
    for m in RE.captures_iter(sql) {
        let whole = m.get(0).unwrap();
        let first = m.get(1).unwrap().as_str();
        let second = m.get(2).map(|g| g.as_str());
        let first_lower = normalize_ident(first);
        let raw = whole.as_str();
        let first_off = IDENT.find(raw).map(|x| x.start()).unwrap_or(0);
        let from = whole.start() + first_off;
        let to = match second {
            Some(s) => {
                let rest = &raw[first_off + first.len()..];
                from + first.len() + IDENT.find(rest).map(|x| x.start()).unwrap_or(0) + s.len()
            }
            None => from + first.len(),
        };
        if let Some(second) = second {
            let second_lower = normalize_ident(second);
            let has_schema = schemas.contains(&first_lower);
            let has_table = schema_tables.get(&first_lower).is_some_and(|t| t.contains(&second_lower));
            if !has_schema || !has_table {
                if system_schema(&db.driver, &first_lower) {
                    refs.push(TableRef {
                        columns: system_table_columns(&db.driver, &first_lower, &second_lower),
                        table: second_lower,
                    });
                    continue;
                }
                out.push(diag(from, to, format!("Unknown table {first}.{second}")));
                continue;
            }
            let cols = table_columns.get(&second_lower).cloned().unwrap_or_default();
            refs.push(TableRef { table: second_lower, columns: Some(cols) });
            continue;
        }
        match table_columns.get(&first_lower) {
            Some(cols) => refs.push(TableRef { table: first_lower, columns: Some(cols.clone()) }),
            None => out.push(diag(from, to, format!("Unknown table {first}"))),
        }
    }
    refs
}

fn remove_select_alias(expr: &str) -> String {
    static AS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i-u)\s+AS\s+[A-Za-z_][\w$]*$").unwrap());
    static IMPLICIT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)\s+[A-Za-z_][\w$]*$").unwrap());
    let without = AS.replace(expr, "");
    if without != expr {
        return without.into_owned();
    }
    let Some(m) = IMPLICIT.find(expr) else { return expr.to_string() };
    let before = expr[..m.start()].trim_end();
    if before.is_empty() {
        expr.to_string()
    } else {
        before.to_string()
    }
}

struct ColumnRef {
    table: Option<String>,
    column: String,
    from: usize,
    to: usize,
}

fn find_column_refs(expr: &str) -> Vec<ColumnRef> {
    static TOKEN: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?-u)([A-Za-z_][\w$]*)(?:\s*\.\s*([A-Za-z_][\w$]*))?").unwrap());
    static CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\(").unwrap());
    static WILD: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\s*\.\s*\*").unwrap());
    let mut refs = Vec::new();
    for m in TOKEN.captures_iter(expr) {
        let whole = m.get(0).unwrap();
        let first = m.get(1).unwrap().as_str();
        let second = m.get(2).map(|g| g.as_str());
        let after = &expr[whole.end()..];
        let is_fn = second.is_none() && CALL.is_match(after);
        let is_wild = second.is_none() && WILD.is_match(after);
        if is_fn || is_wild || is_reserved(first) {
            continue;
        }
        if let Some(second) = second {
            let off = whole.as_str().rfind(second).unwrap_or(0);
            refs.push(ColumnRef {
                table: Some(normalize_ident(first)),
                column: normalize_ident(second),
                from: whole.start() + off,
                to: whole.start() + off + second.len(),
            });
            continue;
        }
        refs.push(ColumnRef { table: None, column: normalize_ident(first), from: whole.start(), to: whole.start() + first.len() });
    }
    refs
}

/// `findSqlSemanticDiagnostics`.
pub fn semantic(sql: &str, db: &DbSchema) -> Vec<Diagnostic> {
    static SELECT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?is)(?-u:\b)SELECT(?-u:\b)(.*?)(?-u:\b)FROM(?-u:\b)").unwrap());
    static DISTINCT: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i-u)\bDISTINCT\b\s*\*?").unwrap());
    let mut out = Vec::new();
    let refs = resolve_table_refs(sql, db, &mut out);
    let Some(m) = SELECT.captures(sql) else { return out };
    let body = m.get(1).unwrap();
    if body.as_str().is_empty() {
        return out;
    }
    let select_start = body.start();
    for (text, efrom, _) in split_select_expressions(body.as_str()) {
        let trimmed = text.trim();
        if trimmed.is_empty() || trimmed == "*" || DISTINCT.is_match(trimmed) {
            continue;
        }
        let alias_removed = remove_select_alias(trimmed);
        for r in find_column_refs(&alias_removed) {
            let label = &alias_removed[r.from..r.to];
            if let Some(table) = &r.table {
                if let Some(tr) = refs.iter().rev().find(|x| &x.table == table) {
                    if let Some(cols) = &tr.columns {
                        if !cols.contains(&r.column) {
                            out.push(diag(select_start + efrom + r.from, select_start + efrom + r.to, format!("Unknown column {label}")));
                        }
                    }
                }
                continue;
            }
            let found = refs.iter().any(|x| x.columns.as_ref().is_none_or(|c| c.contains(&r.column)));
            if !found && !refs.is_empty() {
                out.push(diag(select_start + efrom + r.from, select_start + efrom + r.to, format!("Unknown column {label}")));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sql::schema::{ColInfo, TableInfo};

    fn db() -> DbSchema {
        DbSchema {
            driver: "sqlite".into(),
            schemas: None,
            tables: vec![TableInfo {
                name: "users".into(),
                columns: vec![ColInfo { name: "id".into(), col_type: "int".into(), key: "PRI".into() }],
                is_view: false,
            }],
            views: vec![],
        }
    }

    #[test]
    fn reports_unknown_identifiers() {
        let d = semantic("SELECT id, nope FROM users", &db());
        assert_eq!(d, vec![diag(11, 15, "Unknown column nope")]);
        let d = semantic("SELECT * FROM missing", &db());
        assert_eq!(d[0].message, "Unknown table missing");
    }

    #[test]
    fn trailing_comma() {
        assert_eq!(common_syntax("SELECT a, FROM t")[0].message, "Trailing comma in SELECT list");
    }

    #[test]
    fn parser_errors_are_filtered() {
        assert!(lint("SELECT users.* FROM users", Dialect::Sqlite, None).is_empty());
        assert_eq!(lint("SELECT (1 FROM t", Dialect::Sqlite, None).len(), 1);
    }
}
