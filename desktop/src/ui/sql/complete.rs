//! Completion engine: ports of lang-sql's keyword/schema completion sources,
//! the app's "smart" source (`makeSmartCompletionSource`) and CodeMirror's
//! fuzzy matcher + ranking so the popup lists identical options in the same
//! order.

use super::schema::{build_namespace, ColInfo, DbSchema, Namespace, NamespaceNode, TableInfo};
use super::tokenizer::{Dialect, Tok, Token};
use super::{prev_significant, statement_at, statements, token_before, StatementSpan};
use crate::ui::js::locale_compare;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

#[derive(Debug, Clone, PartialEq)]
pub struct Completion {
    pub label: String,
    /// CodeMirror completion `type` (keyword, type, variable, property, class,
    /// interface, namespace, function, constant).
    pub kind: &'static str,
    pub detail: Option<String>,
    pub boost: Option<i32>,
    pub apply: Option<String>,
}

impl Completion {
    fn new(label: impl Into<String>, kind: &'static str) -> Self {
        Self { label: label.into(), kind, detail: None, boost: None, apply: None }
    }
    fn boost(mut self, b: i32) -> Self {
        self.boost = Some(b);
        self
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidFor {
    /// `/^\w*$/`
    Word,
    /// `/^[`'"\[]?\w*[`'"\]]?$/`
    QuotedSpan,
    /// keyword list: `^[\w][\w-]*$`
    KeywordSpan,
}

impl ValidFor {
    pub fn accepts(self, text: &str) -> bool {
        let word = |c: char| c.is_ascii_alphanumeric() || c == '_';
        match self {
            ValidFor::Word => text.chars().all(word),
            ValidFor::KeywordSpan => {
                let mut chars = text.chars();
                match chars.next() {
                    None => true,
                    Some(first) => word(first) && chars.all(|c| word(c) || c == '-'),
                }
            }
            ValidFor::QuotedSpan => {
                let chars: Vec<char> = text.chars().collect();
                let mut i = 0;
                if i < chars.len() && "`'\"[".contains(chars[i]) {
                    i += 1;
                }
                while i < chars.len() && word(chars[i]) {
                    i += 1;
                }
                if i < chars.len() && "`'\"]".contains(chars[i]) {
                    i += 1;
                }
                i == chars.len()
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct SourceResult {
    pub from: usize,
    pub to: Option<usize>,
    pub options: Vec<Completion>,
    pub valid_for: ValidFor,
}

/// A ranked option ready for display.
#[derive(Debug, Clone)]
pub struct RankedOption {
    pub completion: Completion,
    /// Matched char ranges in the label (char offsets, [from, to) pairs).
    pub matched: Vec<(usize, usize)>,
    pub score: i32,
    pub source_from: usize,
    pub source_to: Option<usize>,
}

pub struct Context<'a> {
    pub doc: &'a str,
    pub pos: usize,
    pub explicit: bool,
    pub tokens: &'a [Token],
    pub spans: &'a [StatementSpan],
}

/// Everything needed to answer completion requests for one editor state.
pub struct Engine {
    pub dialect: Dialect,
    pub db: Option<DbSchema>,
    namespace: Level,
    keyword_options: Vec<Completion>,
}

impl Engine {
    pub fn new(dialect: Dialect, db: Option<DbSchema>) -> Self {
        let ns = db.as_ref().map(build_namespace).unwrap_or_default();
        let quote = dialect.spec().identifier_quotes.chars().next().unwrap_or('"');
        let quote = if dialect == Dialect::PostgreSql { '"' } else { quote };
        let mut top = Level::new(quote);
        top.add_namespace(&ns);
        let keyword_options = dialect
            .spec()
            .words
            .iter()
            .map(|(w, t)| {
                let kind = match t {
                    Tok::Type => "type",
                    Tok::Keyword => "keyword",
                    _ => "variable",
                };
                Completion::new(w.to_uppercase(), kind).boost(-1)
            })
            .collect();
        Engine { dialect, db, namespace: top, keyword_options }
    }

    /// Run all sources (smart, schema, keyword — in registration order).
    pub fn query(&self, doc: &str, pos: usize, explicit: bool) -> Vec<SourceResult> {
        let tokens = super::tokenize(doc, self.dialect);
        let spans = statements(&tokens);
        let ctx = Context { doc, pos, explicit, tokens: &tokens, spans: &spans };
        let mut out = Vec::new();
        if let Some(db) = &self.db {
            if let Some(r) = smart_source(db, &ctx) {
                out.push(r);
            }
        }
        if let Some(r) = self.schema_source(&ctx) {
            out.push(r);
        }
        if let Some(r) = self.keyword_source(&ctx) {
            out.push(r);
        }
        out
    }

    fn keyword_source(&self, ctx: &Context) -> Option<SourceResult> {
        if let Some((_, t)) = token_before(ctx.tokens, ctx.pos) {
            if matches!(t.kind, Tok::QuotedIdentifier | Tok::String | Tok::LineComment | Tok::BlockComment | Tok::Dot) {
                return None;
            }
        }
        let token = match_before(ctx.doc, ctx.pos, |line| keyword_tail(line));
        if token.is_none() && !ctx.explicit {
            return None;
        }
        Some(SourceResult {
            from: token.map(|t| t.0).unwrap_or(ctx.pos),
            to: None,
            options: self.keyword_options.clone(),
            valid_for: ValidFor::KeywordSpan,
        })
    }

    fn schema_source(&self, ctx: &Context) -> Option<SourceResult> {
        let (from, quoted, mut parents, empty, anchor) = source_context(ctx);
        if empty && !ctx.explicit {
            return None;
        }
        let aliases = anchor.and_then(|idx| get_aliases(ctx, idx));
        if let Some(aliases) = &aliases {
            if parents.len() == 1 {
                if let Some(path) = aliases.iter().find(|(k, _)| *k == parents[0]).map(|(_, v)| v.clone()) {
                    parents = path;
                }
            }
        }
        let mut level = &self.namespace;
        for name in &parents {
            level = level.children.iter().find(|(n, _)| n == name).map(|(_, l)| l)?;
        }
        let mut options = level.list.clone();
        if std::ptr::eq(level, &self.namespace) {
            if let Some(aliases) = &aliases {
                options.extend(aliases.iter().map(|(name, _)| Completion::new(name.clone(), "constant")));
            }
        }
        if let Some(open) = quoted {
            let close = if open == '[' { ']' } else { open };
            let quote_after = ctx.doc[ctx.pos..].starts_with(close);
            let options = options
                .into_iter()
                .map(|mut c| {
                    if !c.label.starts_with(open) {
                        c.label = format!("{open}{}{close}", c.label);
                    }
                    c.apply = None;
                    c
                })
                .collect();
            return Some(SourceResult {
                from,
                to: quote_after.then(|| ctx.pos + close.len_utf8()),
                options,
                valid_for: ValidFor::QuotedSpan,
            });
        }
        Some(SourceResult { from, to: None, options, valid_for: ValidFor::Word })
    }
}

fn is_word_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// CodeMirror `matchBefore`: looks at (at most 250 chars of) the current line
/// before `pos`; `tail` returns the byte length of the match ending there.
fn match_before(doc: &str, pos: usize, tail: impl Fn(&str) -> usize) -> Option<(usize, String)> {
    let line_start = doc[..pos].rfind('\n').map(|i| i + 1).unwrap_or(0);
    let mut start = line_start;
    let chars_before = doc[line_start..pos].chars().count();
    if chars_before > 250 {
        start = doc[line_start..pos]
            .char_indices()
            .nth(chars_before - 250)
            .map(|(i, _)| line_start + i)
            .unwrap_or(line_start);
    }
    let text = &doc[start..pos];
    let len = tail(text);
    if len == 0 {
        return None;
    }
    Some((pos - len, text[text.len() - len..].to_string()))
}

fn word_tail(text: &str) -> usize {
    text.chars().rev().take_while(|c| is_word_char(*c)).map(|c| c.len_utf8()).sum()
}

/// `/[\w][\w-]*$/` – longest run of word chars/dashes that starts with a word char.
fn keyword_tail(text: &str) -> usize {
    let chars: Vec<char> = text.chars().collect();
    let mut i = chars.len();
    while i > 0 && (is_word_char(chars[i - 1]) || chars[i - 1] == '-') {
        i -= 1;
    }
    // Skip leading dashes: the match must start with a word char.
    while i < chars.len() && chars[i] == '-' {
        i += 1;
    }
    chars[i..].iter().map(|c| c.len_utf8()).sum()
}

// ─── lang-sql schema completion ─────────────────────────────────────────────

#[derive(Debug, Clone)]
struct Level {
    id_quote: char,
    list: Vec<Completion>,
    children: Vec<(String, Level)>,
}

impl Level {
    fn new(id_quote: char) -> Self {
        Level { id_quote, list: Vec::new(), children: Vec::new() }
    }

    fn name_completion(&self, label: &str, kind: &'static str) -> Completion {
        static PLAIN: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^[a-z_][a-z_\d]*$").unwrap());
        let mut c = Completion::new(label, kind);
        if !PLAIN.is_match(label) {
            let close = if self.id_quote == '[' { ']' } else { self.id_quote };
            c.apply = Some(format!("{}{label}{close}", self.id_quote));
        }
        c
    }

    fn child(&mut self, name: &str) -> &mut Level {
        if let Some(idx) = self.children.iter().position(|(n, _)| n == name) {
            return &mut self.children[idx].1;
        }
        if !name.is_empty() && !self.list.iter().any(|c| c.label == name) {
            let c = self.name_completion(name, "type");
            self.list.push(c);
        }
        self.children.push((name.to_string(), Level::new(self.id_quote)));
        &mut self.children.last_mut().unwrap().1
    }

    fn add_completion(&mut self, option: Completion) {
        if let Some(idx) = self.list.iter().position(|o| o.label == option.label) {
            self.list[idx] = option;
        } else {
            self.list.push(option);
        }
    }

    fn add_namespace(&mut self, ns: &Namespace) {
        for (name, node) in &ns.entries {
            let scope = self.child(name);
            match node {
                NamespaceNode::Columns(cols) => {
                    for col in cols {
                        let c = scope.name_completion(col, "property");
                        scope.add_completion(c);
                    }
                }
                NamespaceNode::Nested(inner) => scope.add_namespace(inner),
            }
        }
    }
}

fn id_name(doc: &str, t: &Token) -> String {
    let text = &doc[t.from..t.to];
    let bytes = text.as_bytes();
    if bytes.len() >= 2
        && "`'\"[".as_bytes().contains(&bytes[0])
        && "`'\"]".as_bytes().contains(&bytes[bytes.len() - 1])
    {
        return text[1..text.len() - 1].to_string();
    }
    text.to_string()
}

fn plain_id(t: Option<&Token>) -> bool {
    matches!(t.map(|t| t.kind), Some(Tok::Identifier) | Some(Tok::QuotedIdentifier))
}

fn parents_for(ctx: &Context, mut node: Option<usize>) -> Vec<String> {
    let mut path = Vec::new();
    loop {
        let Some(idx) = node else { return path };
        if ctx.tokens[idx].kind != Tok::Dot {
            return path;
        }
        let Some((name_idx, name)) = prev_significant(ctx.tokens, idx) else { return path };
        if !plain_id(Some(name)) {
            return path;
        }
        path.insert(0, id_name(ctx.doc, name));
        node = prev_significant(ctx.tokens, name_idx).map(|(i, _)| i);
    }
}

/// Returns (from, quoted, parents, empty, anchor token index).
fn source_context(ctx: &Context) -> (usize, Option<char>, Vec<String>, bool, Option<usize>) {
    match token_before(ctx.tokens, ctx.pos) {
        Some((idx, t)) if matches!(t.kind, Tok::Identifier | Tok::QuotedIdentifier | Tok::Keyword) => {
            let quoted = (t.kind == Tok::QuotedIdentifier).then(|| ctx.doc[t.from..].chars().next().unwrap_or('"'));
            let before = prev_significant(ctx.tokens, idx).map(|(i, _)| i);
            (t.from, quoted, parents_for(ctx, before), false, Some(idx))
        }
        Some((idx, t)) if t.kind == Tok::Dot => (ctx.pos, None, parents_for(ctx, Some(idx)), false, Some(idx)),
        other => (ctx.pos, None, Vec::new(), true, other.map(|(i, _)| i)),
    }
}

/// Top-level elements of a statement: tokens with bracket groups collapsed and
/// dotted identifiers merged (like lang-sql's CompositeIdentifier).
#[derive(Debug, Clone)]
enum Elem {
    Tok(usize),
    Group,
    Composite(Vec<usize>),
}

fn statement_elements(ctx: &Context, span: &StatementSpan) -> Vec<Elem> {
    let mut out: Vec<Elem> = Vec::new();
    let mut depth = 0i32;
    let sig: Vec<usize> = (span.first_token..span.end_token)
        .filter(|&i| !matches!(ctx.tokens[i].kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment))
        .collect();
    let mut k = 0;
    while k < sig.len() {
        let i = sig[k];
        let kind = ctx.tokens[i].kind;
        match kind {
            Tok::ParenL | Tok::BraceL | Tok::BracketL => {
                if depth == 0 {
                    out.push(Elem::Group);
                }
                depth += 1;
            }
            Tok::ParenR | Tok::BraceR | Tok::BracketR => {
                if depth > 0 {
                    depth -= 1;
                }
            }
            _ if depth > 0 => {}
            Tok::Identifier | Tok::QuotedIdentifier
                if k + 2 < sig.len() && ctx.tokens[sig[k + 1]].kind == Tok::Dot && plain_id(Some(&ctx.tokens[sig[k + 2]])) =>
            {
                let mut parts = vec![i];
                while k + 2 < sig.len() && ctx.tokens[sig[k + 1]].kind == Tok::Dot && plain_id(Some(&ctx.tokens[sig[k + 2]])) {
                    parts.push(sig[k + 2]);
                    k += 2;
                }
                out.push(Elem::Composite(parts));
            }
            _ => out.push(Elem::Tok(i)),
        }
        k += 1;
    }
    out
}

fn get_aliases(ctx: &Context, at: usize) -> Option<Vec<(String, Vec<String>)>> {
    let t = ctx.tokens[at];
    let span = statement_at(ctx.spans, t.to.max(t.from + 1)).or_else(|| statement_at(ctx.spans, ctx.pos))?;
    let elems = statement_elements(ctx, &span);
    let mut aliases: Option<Vec<(String, Vec<String>)>> = None;
    let mut saw_from = false;
    let mut prev_id: Option<&Elem> = None;
    let kw_of = |e: &Elem| -> Option<String> {
        match e {
            Elem::Tok(i) if ctx.tokens[*i].kind == Tok::Keyword => {
                Some(ctx.doc[ctx.tokens[*i].from..ctx.tokens[*i].to].to_lowercase())
            }
            _ => None,
        }
    };
    let is_plain = |e: Option<&Elem>| matches!(e, Some(Elem::Tok(i)) if plain_id(Some(&ctx.tokens[*i])));
    let path_for = |e: &Elem| -> Vec<String> {
        match e {
            Elem::Composite(parts) => parts.iter().map(|i| id_name(ctx.doc, &ctx.tokens[*i])).collect(),
            Elem::Tok(i) => vec![id_name(ctx.doc, &ctx.tokens[*i])],
            Elem::Group => Vec::new(),
        }
    };
    const END_FROM: &[&str] = &["where", "group", "having", "order", "union", "intersect", "except", "all", "distinct", "limit", "offset", "fetch", "for"];
    for (idx, scan) in elems.iter().enumerate() {
        let kw = kw_of(scan);
        let mut alias: Option<String> = None;
        if !saw_from {
            saw_from = kw.as_deref() == Some("from");
        } else if kw.as_deref() == Some("as") && prev_id.is_some() && is_plain(elems.get(idx + 1)) {
            if let Some(Elem::Tok(i)) = elems.get(idx + 1) {
                alias = Some(id_name(ctx.doc, &ctx.tokens[*i]));
            }
        } else if kw.as_deref().is_some_and(|k| END_FROM.contains(&k)) {
            break;
        } else if prev_id.is_some() && is_plain(Some(scan)) {
            if let Elem::Tok(i) = scan {
                alias = Some(id_name(ctx.doc, &ctx.tokens[*i]));
            }
        }
        if let (Some(alias), Some(prev)) = (alias, prev_id) {
            let list = aliases.get_or_insert_with(Vec::new);
            let path = path_for(prev);
            if let Some(entry) = list.iter_mut().find(|(k, _)| *k == alias) {
                entry.1 = path;
            } else {
                list.push((alias, path));
            }
        }
        prev_id = match scan {
            Elem::Composite(_) => Some(scan),
            Elem::Tok(i) if plain_id(Some(&ctx.tokens[*i])) => Some(scan),
            _ => None,
        };
    }
    aliases
}

// ─── Smart source (sqlComplete.ts) ──────────────────────────────────────────

const RESERVED: &[&str] = &[
    "WHERE", "ON", "SET", "GROUP", "ORDER", "HAVING", "LIMIT", "OFFSET", "INNER", "LEFT", "RIGHT", "OUTER", "CROSS",
    "NATURAL", "FULL", "SELECT", "FROM", "JOIN", "INTO", "VALUES", "UPDATE", "DELETE", "INSERT", "CREATE", "DROP",
    "ALTER", "TABLE", "INDEX", "VIEW", "AS", "BY", "AND", "OR", "NOT", "IN", "IS", "NULL", "LIKE", "BETWEEN",
    "EXISTS", "CASE", "WHEN", "THEN", "ELSE", "END", "DISTINCT", "ALL", "ANY", "UNION", "INTERSECT", "EXCEPT", "WITH",
    "RECURSIVE", "RETURNING", "USING", "LATERAL", "TRUE", "FALSE", "PRIMARY", "FOREIGN", "KEY", "UNIQUE",
    "CONSTRAINT", "DEFAULT", "CHECK", "REFERENCES",
];

pub fn is_reserved(word: &str) -> bool {
    RESERVED.contains(&word.to_uppercase().as_str())
}

fn build_table_map(db: &DbSchema) -> Vec<(String, Vec<ColInfo>)> {
    let mut m: Vec<(String, Vec<ColInfo>)> = Vec::new();
    let mut set = |k: String, v: Vec<ColInfo>| {
        if let Some(e) = m.iter_mut().find(|(n, _)| *n == k) {
            e.1 = v;
        } else {
            m.push((k, v));
        }
    };
    match &db.schemas {
        Some(schemas) if !schemas.is_empty() => {
            for s in schemas {
                for t in &s.tables {
                    set(t.name.to_lowercase(), t.columns.clone());
                }
            }
        }
        _ => {
            for t in db.all_flat_tables() {
                set(t.name.to_lowercase(), t.columns.clone());
            }
        }
    }
    m
}

fn strip_quotes(s: &str) -> String {
    s.replace(['"', '`', '\'', '[', ']'], "")
}

fn extract_aliases(sql: &str, db: &DbSchema) -> HashMap<String, Vec<ColInfo>> {
    static ALIAS: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i-u)\b(?:FROM|JOIN)\s+(?:[\w"'`\[\]]+\s*\.\s*)?([\w"'`\[\]]+)\s+(?:AS\s+)?([\w"'`\[\]]+)"#).unwrap()
    });
    static CTE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"(?i-u)\bWITH\s+([\w"'`]+)\s+AS\s*\("#).unwrap());
    let table_map = build_table_map(db);
    let mut result: HashMap<String, Vec<ColInfo>> = HashMap::new();
    for (name, cols) in &table_map {
        result.insert(name.clone(), cols.clone());
    }
    // The JS regex is global and non-overlapping; `captures_iter` matches that.
    for m in ALIAS.captures_iter(sql) {
        let raw_table = strip_quotes(&m[1]);
        let raw_alias = strip_quotes(&m[2]);
        if is_reserved(&raw_alias) {
            continue;
        }
        if let Some((_, cols)) = table_map.iter().find(|(n, _)| *n == raw_table.to_lowercase()) {
            result.insert(raw_alias.clone(), cols.clone());
            result.insert(raw_alias.to_lowercase(), cols.clone());
        }
    }
    for m in CTE.captures_iter(sql) {
        let cte = m[1].replace(['"', '`', '\''], "");
        result.entry(cte.to_lowercase()).or_default();
    }
    result
}

fn col_completion(c: &ColInfo) -> Completion {
    let parts: Vec<&str> = [c.col_type.as_str(), if c.key == "PRI" { "PK" } else { "" }]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect();
    let detail = parts.join(" · ");
    Completion {
        label: c.name.clone(),
        kind: "property",
        detail: (!detail.is_empty()).then_some(detail),
        boost: Some(if c.key == "PRI" { 12 } else { 10 }),
        apply: None,
    }
}

fn table_completion(t: &TableInfo) -> Completion {
    Completion::new(t.name.clone(), if t.is_view { "interface" } else { "class" }).boost(7)
}

const COMMON_SQL_FUNCTIONS: &[&str] = &[
    "AVG", "CAST", "COALESCE", "COUNT", "LOWER", "MAX", "MIN", "NULLIF", "ROUND", "SUBSTRING", "SUM", "TRIM", "UPPER",
];

fn driver_functions(driver: &str) -> &'static [&'static str] {
    match driver {
        "mysql" => &["DATE_FORMAT", "GROUP_CONCAT", "IFNULL", "JSON_EXTRACT", "NOW"],
        "postgres" => &["ARRAY_AGG", "DATE_TRUNC", "JSONB_BUILD_OBJECT", "NOW", "STRING_AGG"],
        "sqlite" => &["DATETIME", "GROUP_CONCAT", "IFNULL", "JULIANDAY", "STRFTIME"],
        _ => &[],
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    Expression,
    Table,
    Neutral,
}

/// Words (upper-cased) and structural punctuation, skipping strings,
/// quoted identifiers and comments.
pub fn context_tokens(sql: &str) -> Vec<String> {
    let chars: Vec<char> = sql.chars().collect();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();
        if ch == '-' && next == Some('-') {
            i += 2;
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if ch == '/' && next == Some('*') {
            i += 2;
            while i < chars.len() && !(chars[i] == '*' && chars.get(i + 1) == Some(&'/')) {
                i += 1;
            }
            i = (i + 2).min(chars.len());
            continue;
        }
        if matches!(ch, '\'' | '"' | '`' | '[') {
            let close = if ch == '[' { ']' } else { ch };
            i += 1;
            while i < chars.len() {
                if chars[i] == close {
                    if chars.get(i + 1) == Some(&close) && close != ']' {
                        i += 2;
                        continue;
                    }
                    i += 1;
                    break;
                }
                i += 1;
            }
            continue;
        }
        if ch.is_ascii_alphabetic() || ch == '_' {
            let start = i;
            i += 1;
            while i < chars.len() && (is_word_char(chars[i]) || chars[i] == '$') {
                i += 1;
            }
            tokens.push(chars[start..i].iter().collect::<String>().to_uppercase());
            continue;
        }
        if "();,".contains(ch) {
            tokens.push(ch.to_string());
        }
        i += 1;
    }
    tokens
}

pub fn completion_mode(sql_before_cursor: &str) -> Mode {
    let mut modes = vec![Mode::Neutral];
    let mut awaiting_by = false;
    for token in context_tokens(sql_before_cursor) {
        let depth = modes.len() - 1;
        match token.as_str() {
            ";" => {
                modes = vec![Mode::Neutral];
                awaiting_by = false;
                continue;
            }
            "(" => {
                modes.push(modes[depth]);
                continue;
            }
            ")" => {
                if modes.len() > 1 {
                    modes.pop();
                }
                continue;
            }
            _ => {}
        }
        let last = modes.len() - 1;
        match token.as_str() {
            "SELECT" | "WHERE" | "ON" | "HAVING" | "RETURNING" | "SET" | "VALUES" => {
                modes[last] = Mode::Expression;
                awaiting_by = false;
            }
            "FROM" | "JOIN" | "UPDATE" | "INTO" => {
                modes[last] = Mode::Table;
                awaiting_by = false;
            }
            "GROUP" | "ORDER" => awaiting_by = true,
            "BY" if awaiting_by => {
                modes[last] = Mode::Expression;
                awaiting_by = false;
            }
            _ => {}
        }
    }
    *modes.last().unwrap()
}

pub fn normalize_ident(id: &str) -> String {
    id.trim_start_matches(['"', '\'', '`', '['])
        .trim_end_matches(['"', '\'', '`', ']'])
        .to_lowercase()
}

pub fn find_table<'a>(db: &'a DbSchema, schema_name: &str, table_name: &str) -> Option<&'a TableInfo> {
    let lower = table_name.to_lowercase();
    match &db.schemas {
        Some(schemas) if !schemas.is_empty() => {
            for s in schemas.iter().filter(|s| schema_name.is_empty() || s.name.to_lowercase() == schema_name.to_lowercase()) {
                if let Some(t) = s.tables.iter().find(|t| t.name.to_lowercase() == lower) {
                    return Some(t);
                }
            }
            None
        }
        _ => db.all_flat_tables().find(|t| t.name.to_lowercase() == lower),
    }
}

fn referenced_column_names(sql: &str, db: &DbSchema) -> std::collections::HashSet<String> {
    static RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(r#"(?i-u)\b(?:FROM|JOIN)\s+(?:([\w"'`\[\]]+)\s*\.\s*)?([\w"'`\[\]]+)"#).unwrap()
    });
    let mut names = std::collections::HashSet::new();
    for m in RE.captures_iter(sql) {
        let schema = m.get(1).map(|g| normalize_ident(g.as_str())).unwrap_or_default();
        let table = normalize_ident(&m[2]);
        if let Some(t) = find_table(db, &schema, &table) {
            for c in &t.columns {
                names.insert(c.name.to_lowercase());
            }
        }
    }
    names
}

fn statement_bounds(ctx: &Context) -> (usize, usize) {
    if let Some(span) = statement_at(ctx.spans, ctx.pos) {
        return (span.from, span.to);
    }
    let search_end = ctx.pos.saturating_sub(1);
    let from = ctx.doc[..search_end.min(ctx.doc.len())]
        .rfind(';')
        .map(|i| i + 1)
        .or_else(|| (ctx.doc.as_bytes().get(search_end) == Some(&b';') && search_end < ctx.pos).then_some(search_end + 1))
        .unwrap_or(0);
    let to = ctx.doc[ctx.pos..].find(';').map(|i| ctx.pos + i).unwrap_or(ctx.doc.len());
    (from, to)
}

fn smart_source(db: &DbSchema, ctx: &Context) -> Option<SourceResult> {
    static THREE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)(\w+)\.(\w+)\.(\w*)$").unwrap());
    static TWO: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?-u)(\w+)\.(\w*)$").unwrap());
    static CLAUSE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"(?i-u)\b(?:SELECT|WHERE|ON|HAVING|RETURNING|SET|BY|FROM|JOIN)\s*$").unwrap());
    static COMMA: LazyLock<Regex> = LazyLock::new(|| Regex::new(r",\s*$").unwrap());
    let before = &ctx.doc[..ctx.pos];

    if let Some(m) = THREE.captures(before) {
        let (schema_name, table_name, partial) = (&m[1], &m[2], &m[3]);
        let from = ctx.pos - partial.len();
        if let Some(schemas) = &db.schemas {
            if let Some(schema) = schemas.iter().find(|s| s.name.to_lowercase() == schema_name.to_lowercase()) {
                if let Some(table) = schema.tables.iter().find(|t| t.name.to_lowercase() == table_name.to_lowercase()) {
                    return Some(SourceResult {
                        from,
                        to: None,
                        options: table
                            .columns
                            .iter()
                            .filter(|c| c.name.to_lowercase().starts_with(&partial.to_lowercase()))
                            .map(col_completion)
                            .collect(),
                        valid_for: ValidFor::Word,
                    });
                }
            }
        }
    }

    if let Some(m) = TWO.captures(before) {
        let (prefix, partial) = (&m[1], &m[2]);
        let from = ctx.pos - partial.len();
        let lower = prefix.to_lowercase();
        let aliases = extract_aliases(ctx.doc, db);
        let alias_cols = aliases.get(prefix).or_else(|| aliases.get(&lower));
        if let Some(cols) = alias_cols.filter(|c| !c.is_empty()) {
            return Some(SourceResult {
                from,
                to: None,
                options: cols
                    .iter()
                    .filter(|c| c.name.to_lowercase().starts_with(&partial.to_lowercase()))
                    .map(col_completion)
                    .collect(),
                valid_for: ValidFor::Word,
            });
        }
        if let Some(schemas) = &db.schemas {
            if let Some(schema) = schemas.iter().find(|s| s.name.to_lowercase() == lower) {
                return Some(SourceResult {
                    from,
                    to: None,
                    options: schema
                        .tables
                        .iter()
                        .filter(|t| t.name.to_lowercase().starts_with(&partial.to_lowercase()))
                        .map(table_completion)
                        .collect(),
                    valid_for: ValidFor::Word,
                });
            }
        }
        return None;
    }

    let word = match_before(ctx.doc, ctx.pos, word_tail);
    let (b_from, b_to) = statement_bounds(ctx);
    let b_from = b_from.min(ctx.pos);
    let statement_before_cursor = &ctx.doc[b_from..ctx.pos];
    let automatic = CLAUSE.is_match(statement_before_cursor) || COMMA.is_match(statement_before_cursor);
    if word.is_none() && !ctx.explicit && !automatic {
        return None;
    }
    let partial = word.as_ref().map(|w| w.1.to_lowercase()).unwrap_or_default();
    let from = word.as_ref().map(|w| w.0).unwrap_or(ctx.pos);
    let statement_sql = &ctx.doc[b_from..b_to.max(b_from).min(ctx.doc.len())];
    let mode = completion_mode(statement_before_cursor);
    let referenced = referenced_column_names(statement_sql, db);

    let mut options: Vec<Completion> = Vec::new();
    let mut index: HashMap<String, usize> = HashMap::new();
    let mut add = |c: Completion, options: &mut Vec<Completion>| {
        let key = c.label.to_lowercase();
        match index.get(&key) {
            None => {
                index.insert(key, options.len());
                options.push(c);
            }
            Some(&i) => {
                if c.boost.unwrap_or(0) > options[i].boost.unwrap_or(0) {
                    options[i] = c;
                }
            }
        }
    };

    if mode == Mode::Expression {
        for name in COMMON_SQL_FUNCTIONS.iter().chain(driver_functions(&db.driver)) {
            if name.to_lowercase().starts_with(&partial) {
                add(
                    Completion { label: (*name).into(), kind: "function", detail: Some("SQL function".into()), boost: Some(78), apply: None },
                    &mut options,
                );
            }
        }
    }

    let add_table = |t: &TableInfo, options: &mut Vec<Completion>, add: &mut dyn FnMut(Completion, &mut Vec<Completion>)| {
        if !t.name.to_lowercase().starts_with(&partial) {
            return;
        }
        let mut c = table_completion(t);
        c.boost = Some(match mode {
            Mode::Table => 92,
            Mode::Expression => 4,
            Mode::Neutral => c.boost.unwrap_or(0),
        });
        add(c, options);
    };
    let add_column = |col: &ColInfo, options: &mut Vec<Completion>, add: &mut dyn FnMut(Completion, &mut Vec<Completion>)| {
        if mode != Mode::Expression || !col.name.to_lowercase().starts_with(&partial) {
            return;
        }
        let in_scope = referenced.contains(&col.name.to_lowercase());
        let mut c = col_completion(col);
        c.boost = Some(if in_scope { 88 } else { 48 } + if col.key == "PRI" { 2 } else { 0 });
        add(c, options);
    };

    match &db.schemas {
        Some(schemas) => {
            for s in schemas {
                if s.name.to_lowercase().starts_with(&partial) {
                    add(
                        Completion::new(s.name.clone(), "namespace").boost(if mode == Mode::Table { 84 } else { 5 }),
                        &mut options,
                    );
                }
                for t in &s.tables {
                    add_table(t, &mut options, &mut add);
                    for c in &t.columns {
                        add_column(c, &mut options, &mut add);
                    }
                }
            }
        }
        None => {
            for t in db.all_flat_tables() {
                add_table(t, &mut options, &mut add);
                for c in &t.columns {
                    add_column(c, &mut options, &mut add);
                }
            }
        }
    }

    if options.is_empty() {
        return None;
    }
    Some(SourceResult { from, to: None, options, valid_for: ValidFor::Word })
}

// ─── CodeMirror FuzzyMatcher + ranking ──────────────────────────────────────

const NOT_FULL: i32 = -100;
const CASE_FOLD: i32 = -200;
const BY_WORD: i32 = -100;
const NOT_START: i32 = -700;
const GAP: i32 = -1100;

pub struct FuzzyMatcher {
    chars: Vec<char>,
    folded: Vec<char>,
    pattern: String,
    /// CodeMirror reuses its `byWord` buffer between calls, so `byWord.length`
    /// stays non-zero once any word-start match was recorded.
    by_word_used: std::cell::Cell<bool>,
}

fn fold(c: char) -> char {
    let upper: String = c.to_uppercase().collect();
    if upper == c.to_string() {
        c.to_lowercase().next().unwrap_or(c)
    } else {
        upper.chars().next().unwrap_or(c)
    }
}

impl FuzzyMatcher {
    pub fn new(pattern: &str) -> Self {
        let chars: Vec<char> = pattern.chars().collect();
        let folded = chars.iter().map(|c| fold(*c)).collect();
        FuzzyMatcher { chars, folded, pattern: pattern.to_string(), by_word_used: std::cell::Cell::new(false) }
    }

    fn result(score: i32, positions: &[usize], word_len: usize) -> (i32, Vec<(usize, usize)>) {
        let mut out: Vec<(usize, usize)> = Vec::new();
        for &p in positions {
            match out.last_mut() {
                Some(last) if last.1 == p => last.1 = p + 1,
                _ => out.push((p, p + 1)),
            }
        }
        (score - word_len as i32, out)
    }

    /// Returns (score, matched char ranges) or None.
    pub fn matches(&self, word: &str) -> Option<(i32, Vec<(usize, usize)>)> {
        let w: Vec<char> = word.chars().collect();
        let len = self.chars.len();
        if len == 0 {
            return Some((NOT_FULL, Vec::new()));
        }
        if w.len() < len {
            return None;
        }
        if len == 1 {
            let first = w[0];
            let mut score = if w.len() == 1 { 0 } else { NOT_FULL };
            if first == self.chars[0] {
            } else if first == self.folded[0] {
                score += CASE_FOLD;
            } else {
                return None;
            }
            return Some((score, vec![(0, 1)]));
        }
        let direct = find_chars(&w, &self.chars);
        if direct == Some(0) {
            return Some((if w.len() == len { 0 } else { NOT_FULL }, vec![(0, len)]));
        }
        let mut any = vec![0usize; len];
        let mut any_to = 0;
        let e = w.len().min(200);
        if direct.is_none() {
            let mut i = 0;
            while i < e && any_to < len {
                let next = w[i];
                if next == self.chars[any_to] || next == self.folded[any_to] {
                    any[any_to] = i;
                    any_to += 1;
                }
                i += 1;
            }
            if any_to < len {
                return None;
            }
        }
        let mut precise_to = 0;
        let mut by_word = vec![0usize; len];
        let mut by_word_to = 0;
        let mut by_word_folded = false;
        let (mut adjacent_to, mut adjacent_start, mut adjacent_end) = (0usize, -1isize, -1isize);
        let has_lower = w.iter().any(|c| c.is_ascii_lowercase());
        let mut word_adjacent = true;
        let mut prev_type = 0; // 0 NonWord, 1 Upper, 2 Lower
        let mut i = 0;
        while i < e && by_word_to < len {
            let next = w[i];
            if direct.is_none() {
                if precise_to < len && next == self.chars[precise_to] {
                    precise_to += 1;
                }
                if adjacent_to < len {
                    if next == self.chars[adjacent_to] || next == self.folded[adjacent_to] {
                        if adjacent_to == 0 {
                            adjacent_start = i as isize;
                        }
                        adjacent_end = i as isize + 1;
                        adjacent_to += 1;
                    } else {
                        adjacent_to = 0;
                    }
                }
            }
            let code = next as u32;
            let ty = if code < 0xff {
                if next.is_ascii_digit() || next.is_ascii_lowercase() {
                    2
                } else if next.is_ascii_uppercase() {
                    1
                } else {
                    0
                }
            } else if next.to_lowercase().next() != Some(next) {
                1
            } else if next.to_uppercase().next() != Some(next) {
                2
            } else {
                0
            };
            if i == 0 || (ty == 1 && has_lower) || (prev_type == 0 && ty != 0) {
                if self.chars[by_word_to] == next || (self.folded[by_word_to] == next && {
                    by_word_folded = true;
                    true
                }) {
                    by_word[by_word_to] = i;
                    by_word_to += 1;
                    self.by_word_used.set(true);
                } else if self.by_word_used.get() {
                    word_adjacent = false;
                }
            }
            prev_type = ty;
            i += 1;
        }
        let wl = w.len();
        if by_word_to == len && by_word[0] == 0 && word_adjacent {
            return Some(Self::result(BY_WORD + if by_word_folded { CASE_FOLD } else { 0 }, &by_word, wl));
        }
        if adjacent_to == len && adjacent_start == 0 {
            return Some((
                CASE_FOLD - wl as i32 + if adjacent_end as usize == wl { 0 } else { NOT_FULL },
                vec![(0, adjacent_end as usize)],
            ));
        }
        if let Some(d) = direct {
            return Some((NOT_START - wl as i32, vec![(d, d + len)]));
        }
        if adjacent_to == len {
            return Some((CASE_FOLD + NOT_START - wl as i32, vec![(adjacent_start as usize, adjacent_end as usize)]));
        }
        if by_word_to == len {
            return Some(Self::result(
                BY_WORD + if by_word_folded { CASE_FOLD } else { 0 } + NOT_START + if word_adjacent { 0 } else { GAP },
                &by_word,
                wl,
            ));
        }
        if len == 2 {
            return None;
        }
        Some(Self::result(if any[0] != 0 { NOT_START } else { 0 } + CASE_FOLD + GAP, &any, wl))
    }

    pub fn pattern(&self) -> &str {
        &self.pattern
    }
}

fn find_chars(hay: &[char], needle: &[char]) -> Option<usize> {
    if needle.len() > hay.len() {
        return None;
    }
    (0..=hay.len() - needle.len()).find(|&i| hay[i..i + needle.len()] == *needle)
}

fn dedupe_score(c: &Completion) -> i32 {
    c.boost.unwrap_or(0) * 100 + if c.apply.is_some() { 10 } else { 0 } + 1
}

/// `sortOptions` from @codemirror/autocomplete.
pub fn rank(doc: &str, pos: usize, results: &[SourceResult]) -> Vec<RankedOption> {
    let mut options: Vec<RankedOption> = Vec::new();
    for r in results {
        let to = pos;
        let pattern = if r.from <= to { &doc[r.from..to] } else { "" };
        let matcher = FuzzyMatcher::new(pattern);
        for c in &r.options {
            if let Some((score, matched)) = matcher.matches(&c.label) {
                options.push(RankedOption {
                    score: score + c.boost.unwrap_or(0),
                    completion: c.clone(),
                    matched,
                    source_from: r.from,
                    source_to: r.to,
                });
            }
        }
    }
    options.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| locale_compare(&a.completion.label, &b.completion.label)));
    let mut result: Vec<RankedOption> = Vec::new();
    for opt in options {
        let dup = result.last().is_some_and(|prev| {
            let p = &prev.completion;
            let c = &opt.completion;
            p.label == c.label && p.detail == c.detail && p.kind == c.kind && p.apply == c.apply && p.boost == c.boost
        });
        if !dup {
            result.push(opt);
        } else if dedupe_score(&opt.completion) > dedupe_score(&result.last().unwrap().completion) {
            *result.last_mut().unwrap() = opt;
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sql::schema::SchemaInfo;

    fn db() -> DbSchema {
        let col = |n: &str, k: &str| ColInfo { name: n.into(), col_type: "integer".into(), key: k.into() };
        DbSchema {
            driver: "postgres".into(),
            schemas: Some(vec![SchemaInfo {
                name: "public".into(),
                tables: vec![
                    TableInfo { name: "users".into(), columns: vec![col("id", "PRI"), col("name", "")], is_view: false },
                    TableInfo { name: "orders".into(), columns: vec![col("id", "PRI"), col("user_id", "")], is_view: false },
                ],
            }]),
            ..Default::default()
        }
    }

    fn labels(doc: &str, explicit: bool) -> Vec<String> {
        let engine = Engine::new(Dialect::PostgreSql, Some(db()));
        let pos = doc.len();
        let results = engine.query(doc, pos, explicit);
        rank(doc, pos, &results).into_iter().map(|o| o.completion.label).collect()
    }

    #[test]
    fn completes_alias_columns() {
        let l = labels("SELECT * FROM users u WHERE u.", false);
        assert_eq!(l, vec!["id", "name"]);
    }

    #[test]
    fn table_mode_prefers_tables() {
        let l = labels("SELECT * FROM o", false);
        assert_eq!(l[0], "orders");
    }

    #[test]
    fn fuzzy_matcher_scores() {
        let m = FuzzyMatcher::new("sel");
        assert_eq!(m.matches("select").unwrap().0, NOT_FULL);
        assert!(m.matches("SELECT").is_some());
        assert!(FuzzyMatcher::new("xz").matches("select").is_none());
    }

    #[test]
    fn mode_detection() {
        assert_eq!(completion_mode("SELECT a, "), Mode::Expression);
        assert_eq!(completion_mode("SELECT a FROM "), Mode::Table);
        assert_eq!(completion_mode("SELECT a FROM t ORDER BY "), Mode::Expression);
    }
}
