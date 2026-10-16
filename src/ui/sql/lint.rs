//! Editor diagnostics: common syntax slips, schema-aware identifier checks
//! (ports of `findSqlCommonSyntaxDiagnostics` / `findSqlSemanticDiagnostics`)
//! and parser-error detection equivalent to lang-sql's Lezer error nodes.

use super::complete::normalize_ident;
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
    /// While the caret is inside this range the diagnostic stays hidden.
    ///
    /// Half-written SQL is invalid SQL, and telling someone so while their
    /// hands are still on the word helps nobody. The editor applies this
    /// against the live caret when it paints, so the underline appears as soon
    /// as they move on and disappears again the moment they come back to fix
    /// it, without anything being re-parsed.
    pub quiet_while_caret_in: Option<std::ops::Range<usize>>,
}

impl Diagnostic {
    pub fn new(from: usize, to: usize, message: impl Into<String>) -> Self {
        Diagnostic { from, to, message: message.into(), quiet_while_caret_in: None }
    }

    /// Whether this should be shown with the caret at `caret`.
    pub fn visible_at(&self, caret: usize) -> bool {
        match &self.quiet_while_caret_in {
            Some(range) => caret < range.start || caret > range.end,
            None => true,
        }
    }
}

fn diag(from: usize, to: usize, message: impl Into<String>) -> Diagnostic {
    Diagnostic::new(from, to, message)
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
        let mut d = diag(from, to, "SQL syntax error");
        // These are token-level slips — an unmatched bracket, a stray
        // character — and they are just as annoying to be told about with the
        // caret still on them.
        d.quiet_while_caret_in = Some(from..to);
        out.push(d);
    }
    // The parser has the last word, but never a second word about something
    // already reported: one mistake should not collect two underlines.
    for d in super::parse_check::parse_diagnostics(doc, dialect, &tokens) {
        if !out.iter().any(|o| o.from < d.to && d.from < o.to) {
            out.push(d);
        }
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
            // The first schema with a table supplies the columns of its
            // unqualified name: the default schema's, when there is one.
            let default = db.default_schema.as_deref();
            let ordered = list
                .iter()
                .filter(|s| Some(s.name.as_str()) == default)
                .chain(list.iter().filter(|s| Some(s.name.as_str()) != default));
            for s in ordered {
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

/// Any word that can name a table. Plenty of real table names (`quote`,
/// `user`, `order`) are keywords or builtins to the tokenizer.
fn is_name(t: &Token) -> bool {
    matches!(t.kind, Tok::Identifier | Tok::QuotedIdentifier | Tok::Keyword | Tok::Builtin | Tok::Type)
}

/// Names a `WITH` clause defines: `name [(columns)] AS [NOT] [MATERIALIZED] (`
/// after `WITH`, `RECURSIVE` or a comma. They are tables as far as the rest of
/// the statement is concerned, but they are not in the database schema.
fn cte_names(sql: &str, sig: &[&Token]) -> HashSet<String> {
    let word = |t: &Token| sql[t.from..t.to].to_ascii_lowercase();
    let mut names = HashSet::new();
    for k in 1..sig.len() {
        if !is_name(sig[k]) {
            continue;
        }
        let prev = sig[k - 1];
        let after_with = prev.kind == Tok::Keyword && matches!(word(prev).as_str(), "with" | "recursive");
        let after_comma = prev.kind == Tok::Punctuation && &sql[prev.from..prev.to] == ",";
        if !after_with && !after_comma {
            continue;
        }
        let mut j = k + 1;
        if sig.get(j).is_some_and(|t| t.kind == Tok::ParenL) {
            // A column list: skip to its closing bracket.
            let mut depth = 0i32;
            while let Some(t) = sig.get(j) {
                match t.kind {
                    Tok::ParenL => depth += 1,
                    Tok::ParenR => depth -= 1,
                    _ => {}
                }
                j += 1;
                if depth == 0 {
                    break;
                }
            }
        }
        if !sig.get(j).is_some_and(|t| word(t) == "as") {
            continue;
        }
        j += 1;
        if sig.get(j).is_some_and(|t| word(t) == "not") {
            j += 1;
        }
        if sig.get(j).is_some_and(|t| word(t) == "materialized") {
            j += 1;
        }
        if sig.get(j).is_some_and(|t| t.kind == Tok::ParenL) {
            names.insert(normalize_ident(&sql[sig[k].from..sig[k].to]));
        }
    }
    names
}

/// The tables a statement reads, from the word after each `FROM` and `JOIN`,
/// reporting the ones the schema doesn't have.
///
/// This works on tokens rather than raw text, so words in strings and comments
/// are never taken for tables, and it skips the places where `FROM` or the word
/// after it isn't a table at all: `JOIN LATERAL`/`FROM ONLY`, `IS DISTINCT
/// FROM`, the `FROM` inside `EXTRACT(… FROM …)`, `TRIM`, `SUBSTRING` and the
/// like, CTE names, and set-returning functions such as `generate_series(…)`
/// or `jsonb_array_elements(…)`, whose columns no schema lists.
fn resolve_table_refs(sql: &str, db: &DbSchema, out: &mut Vec<Diagnostic>) -> Vec<TableRef> {
    let (schemas, schema_tables, table_columns) = flatten(db);
    let tokens = super::tokenize(sql, Dialect::for_driver(&db.driver));
    let sig: Vec<&Token> = super::tokenizer::significant(&tokens).collect();
    let ctes = cte_names(sql, &sig);
    let word = |t: &Token| sql[t.from..t.to].to_ascii_lowercase();
    // One entry per open bracket: whether a query started inside it. A `FROM`
    // in a bracket with no query of its own is a function argument.
    let mut brackets: Vec<bool> = Vec::new();
    let mut refs = Vec::new();
    for (k, t) in sig.iter().enumerate() {
        match t.kind {
            Tok::ParenL => {
                brackets.push(false);
                continue;
            }
            Tok::ParenR => {
                brackets.pop();
                continue;
            }
            Tok::Semi => {
                brackets.clear();
                continue;
            }
            Tok::Keyword => {}
            _ => continue,
        }
        let w = word(t);
        if matches!(w.as_str(), "select" | "delete" | "update" | "insert" | "with" | "values") {
            if let Some(query) = brackets.last_mut() {
                *query = true;
            }
            continue;
        }
        if w != "from" && w != "join" {
            continue;
        }
        if w == "from" {
            if brackets.last() == Some(&false) {
                continue;
            }
            if k > 0 && word(sig[k - 1]) == "distinct" {
                continue;
            }
        }
        let mut j = k + 1;
        while sig.get(j).is_some_and(|n| n.kind == Tok::Keyword && matches!(word(n).as_str(), "lateral" | "only")) {
            j += 1;
        }
        // A bracket here is a subquery or a VALUES list; not a table name.
        let Some(first) = sig.get(j).copied().filter(|n| is_name(n)) else { continue };
        let mut second = None;
        let mut end = j + 1;
        if sig.get(end).is_some_and(|n| n.kind == Tok::Dot) {
            if let Some(n) = sig.get(end + 1).copied().filter(|n| is_name(n)) {
                second = Some(n);
                end += 2;
            }
        }
        let last = second.unwrap_or(first);
        if sig.get(end).is_some_and(|n| n.kind == Tok::ParenL) {
            refs.push(TableRef { table: normalize_ident(&sql[last.from..last.to]), columns: None });
            continue;
        }
        let first_text = &sql[first.from..first.to];
        let first_lower = normalize_ident(first_text);
        if second.is_none() && ctes.contains(&first_lower) {
            refs.push(TableRef { table: first_lower, columns: None });
            continue;
        }
        let (from, to) = (first.from, last.to);
        if let Some(second) = second {
            let second_text = &sql[second.from..second.to];
            let second_lower = normalize_ident(second_text);
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
                out.push(diag(from, to, format!("Unknown table {first_text}.{second_text}")));
                continue;
            }
            let cols = table_columns.get(&second_lower).cloned().unwrap_or_default();
            refs.push(TableRef { table: second_lower, columns: Some(cols) });
            continue;
        }
        match table_columns.get(&first_lower) {
            Some(cols) => refs.push(TableRef { table: first_lower, columns: Some(cols.clone()) }),
            None => out.push(diag(from, to, format!("Unknown table {first_text}"))),
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

fn find_column_refs(expr: &str, dialect: Dialect) -> Vec<ColumnRef> {
    // Use the same lexical rules as highlighting: words inside strings and
    // comments, and dialect keywords such as INTERVAL/MONTH, are not columns.
    let tokens = super::tokenize(expr, dialect);
    let tokens: Vec<_> = super::tokenizer::significant(&tokens).collect();
    let mut refs = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let first = tokens[i];
        i += 1;
        if !matches!(first.kind, Tok::Identifier | Tok::QuotedIdentifier) {
            continue;
        }
        let mut column = first;
        let mut table = None;
        if tokens.get(i).is_some_and(|t| t.kind == Tok::Dot) {
            i += 1;
            let Some(second) = tokens.get(i) else { continue };
            // Qualified names can use keyword spellings, including when
            // whitespace around the dot prevents contextual tokenization.
            if matches!(
                second.kind,
                Tok::Identifier | Tok::QuotedIdentifier | Tok::Keyword | Tok::Type | Tok::Builtin
            ) {
                table = Some(normalize_ident(&expr[first.from..first.to]));
                column = second;
                i += 1;
            } else {
                continue;
            }
        }
        if tokens.get(i).is_some_and(|t| t.kind == Tok::ParenL) {
            continue;
        }
        refs.push(ColumnRef {
            table,
            column: normalize_ident(&expr[column.from..column.to]),
            from: column.from,
            to: column.to,
        });
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
        for r in find_column_refs(&alias_removed, Dialect::for_driver(&db.driver)) {
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
            default_schema: None,
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

    #[test]
    fn mysql_promo_expiry_buckets_have_no_diagnostics() {
        // CURDATE() and balanced quotes/parentheses restore the intended SQL
        // from the issue's pasted query.
        let sql = "SELECT p.store_id,
CASE WHEN b.end_date IS NULL THEN 'open-ended'
WHEN b.end_date >= CURDATE() THEN 'live'
WHEN b.end_date >= CURDATE() - INTERVAL 12 MONTH THEN 'expired<12m'
WHEN b.end_date >= CURDATE() - INTERVAL 24 MONTH THEN 'expired 12-24m'
ELSE 'expired> 24m' END AS bucket,
COUNT(*) AS codes
FROM tails_default.promo_code c
JOIN tails_default.promo_code_batch b ON b.promo_code_batch_id = c.promo_code_batch_id
JOIN tails_default.promo p ON p.promo_id = b.promo_id
GROUP BY 1, 2;";
        let schema = DbSchema {
            driver: "mysql".into(),
            schemas: Some(vec![crate::ui::sql::schema::SchemaInfo {
                name: "tails_default".into(),
                tables: [
                    ("promo_code", &["promo_code_batch_id"][..]),
                    ("promo_code_batch", &["promo_code_batch_id", "promo_id", "end_date"][..]),
                    ("promo", &["promo_id", "store_id"][..]),
                ]
                .into_iter()
                .map(|(name, columns)| TableInfo {
                    name: name.into(),
                    columns: columns.iter().map(|name| ColInfo {
                        name: (*name).into(),
                        col_type: String::new(),
                        key: String::new(),
                    }).collect(),
                    is_view: false,
                })
                .collect(),
            }]),
            ..Default::default()
        };
        for db in [None, Some(&schema)] {
            let found = lint(sql, Dialect::MySql, db);
            assert!(found.is_empty(), "{found:?}");
        }
    }

    #[test]
    fn literals_are_not_column_references() {
        let mut schema = db();
        schema.driver = "mysql".into();
        for sql in [
            "SELECT 'open-ended' FROM users;",
            "SELECT \"open-ended\" FROM users;",
            "SELECT 'it''s expired' FROM users;",
            "SELECT 'it\\'s expired' FROM users;",
            "SELECT id /* explanatory words */ FROM users;",
        ] {
            let found = lint(sql, Dialect::MySql, Some(&schema));
            assert!(found.is_empty(), "{sql}: {found:?}");
        }
    }

    #[test]
    fn interval_units_are_not_column_references() {
        let found = lint("SELECT CURDATE() - INTERVAL 12 MONTH AS expiry FROM users;", Dialect::MySql, Some(&db()));
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn case_expressions_still_report_unknown_columns() {
        let sql = "SELECT CASE WHEN id > 0 THEN 'open-ended' ELSE nope END AS bucket FROM users;";
        let found = lint(sql, Dialect::MySql, Some(&db()));
        let from = sql.find("nope").unwrap();
        assert_eq!(found, vec![diag(from, from + 4, "Unknown column nope")]);

        let sql = "SELECT users.nope, 'open-ended' FROM users;";
        let found = lint(sql, Dialect::MySql, Some(&db()));
        let from = sql.find("nope").unwrap();
        assert_eq!(found, vec![diag(from, from + 4, "Unknown column nope")]);
    }

    fn pg() -> DbSchema {
        let t = |name: &str, cols: &[&str]| TableInfo {
            name: name.into(),
            columns: cols.iter().map(|c| ColInfo { name: (*c).into(), col_type: String::new(), key: String::new() }).collect(),
            is_view: false,
        };
        DbSchema {
            driver: "postgres".into(),
            schemas: Some(vec![crate::ui::sql::schema::SchemaInfo {
                name: "public".into(),
                tables: vec![
                    t("quote", &["quote_id", "parameters", "pet_id"]),
                    t("vat_config", &["vat_config_id", "store_slug", "vat_rate_type", "vat_rate", "start_date"]),
                ],
            }]),
            default_schema: Some("public".into()),
            ..Default::default()
        }
    }

    #[test]
    fn lateral_function_with_ordinality_has_no_diagnostics() {
        let sql = "SELECT q.quote_id, q.parameters->>'tier' AS tier, q.parameters->> 'Strategy_id' AS strategy_id, k.ordinality AS annual_point_number, k.value::numeric AS daily_kcal
FROM quote q
CROSS JOIN LATERAL Jsonb_array_elements_text (q.parameters-> 'kcals')
WITH ORDINALITY AS k(value, ordinality)
WHERE q.pet_id = 12345
ORDER BY q.quote_id, k.ordinality;";
        for db in [None, Some(&pg())] {
            let found = lint(sql, Dialect::PostgreSql, db);
            assert!(found.is_empty(), "{found:?}");
        }
    }

    #[test]
    fn cte_names_are_tables() {
        let sql = "WITH effective AS (
SELECT *, ROW_NUMBER() OVER (
PARTITION BY store_slug, vat_rate_type ORDER BY start_date DESC, vat_config_id DESC
) as rn
 FROM vat_config WHERE start_date <= CURRENT_DATE ), preferred AS (
SELECT *, ROW_NUMBER() OVER (
PARTITION BY store_slug
ORDER BY CASE vat_rate_type WHEN 'reduced' THEN 0 ELSE 1 END
) AS priority
FROM effective WHERE rn=1 AND vat_rate_type IN ('reduced', 'standard')
)
SELECT store_slug, vat_rate FROM preferred WHERE priority=1;";
        for db in [None, Some(&pg())] {
            let found = lint(sql, Dialect::PostgreSql, db);
            assert!(found.is_empty(), "{found:?}");
        }
        // Column lists and MATERIALIZED are part of the CTE syntax too.
        let sql = "WITH RECURSIVE t(n) AS MATERIALIZED (SELECT 1) SELECT n FROM t;";
        assert!(semantic(sql, &pg()).is_empty());
    }

    #[test]
    fn from_that_is_not_a_table_is_ignored() {
        for sql in [
            "SELECT EXTRACT(YEAR FROM start_date) FROM vat_config;",
            "SELECT TRIM(BOTH ' ' FROM store_slug) FROM vat_config;",
            "SELECT SUBSTRING(store_slug FROM 2 FOR 3) FROM vat_config;",
            "SELECT 1 FROM vat_config WHERE store_slug IS DISTINCT FROM vat_rate_type;",
            "SELECT 'copied from elsewhere' FROM vat_config; -- join nowhere",
            "SELECT * FROM ONLY vat_config;",
            "SELECT n FROM generate_series(1, 10) AS n;",
            "SELECT x FROM (SELECT 1 AS x) AS s;",
        ] {
            let found = semantic(sql, &pg());
            assert!(found.is_empty(), "{sql}: {found:?}");
        }
    }

    #[test]
    fn unknown_tables_are_still_reported_on_the_name() {
        let sql = "SELECT * FROM vat_config v JOIN missing m ON m.id = v.vat_config_id;";
        let found = semantic(sql, &pg());
        let from = sql.find("missing").unwrap();
        assert_eq!(found, vec![diag(from, from + 7, "Unknown table missing")]);
        let sql = "SELECT * FROM public.nope;";
        let found = semantic(sql, &pg());
        let from = sql.find("public").unwrap();
        assert_eq!(found, vec![diag(from, from + "public.nope".len(), "Unknown table public.nope")]);
        // A subquery's own FROM still counts.
        let sql = "SELECT * FROM vat_config WHERE store_slug IN (SELECT slug FROM missing);";
        assert_eq!(semantic(sql, &pg())[0].message, "Unknown table missing");
    }
}
