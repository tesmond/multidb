//! SQL formatter following Mozilla's SQL style guide
//! (<https://docs.telemetry.mozilla.org/concepts/sql_style>):
//!
//! * reserved keywords in upper case, function names in lower case;
//! * every root keyword (`SELECT`, `FROM`, `WHERE`, `GROUP BY`, a join, …) on a
//!   line of its own, its arguments on the lines below, indented two spaces,
//!   one per line with trailing commas; a single short argument may share the
//!   keyword's line (`FROM sample`, `LIMIT 10`);
//! * `AND` / `OR` at the start of a condition's line;
//! * joins as `LEFT JOIN` / the table indented below / `ON …` level with it;
//! * CTEs as `name AS (` … `)` with the body indented and a blank line before
//!   the final query; subqueries are opened the same way.
//!
//! The formatter only ever changes whitespace and the case of keywords and
//! built-in function names. Identifiers are never re-cased (MySQL table names
//! are case-sensitive on Linux) and nothing is added or removed. As a final
//! guard the result is tokenized again and compared with the input, token for
//! token: if anything differs, the input is returned unchanged.

use super::tokenizer::{tokenize, Dialect, Tok};

/// Lines are kept to this width where the style leaves a choice (a single
/// argument on its keyword's line, a window or `VALUES` list kept inline).
const MAX_LINE: usize = 100;

/// Bracket nesting beyond this is left as it is: the layout recurses per
/// level, and nothing written by hand nests this deep.
const MAX_DEPTH: usize = 200;

#[derive(Clone, Debug)]
struct T {
    kind: Tok,
    /// The text to print (keywords re-cased).
    text: String,
    lower: String,
    /// The source had whitespace before this token.
    space_before: bool,
    /// The source had a line break before this token.
    newline_before: bool,
    /// A word naming a table (after FROM, JOIN, INTO, …): printed as written,
    /// and a `(` after it is a column list, not a call.
    name_position: bool,
    /// A table name followed by its column list (`INSERT INTO t (a, b)`,
    /// `CREATE TABLE t (…)`), as opposed to a table function in a FROM.
    list_paren: bool,
    /// A unary `-` / `+`: no space after it.
    unary: bool,
}

impl T {
    fn is(&self, word: &str) -> bool {
        self.lower == word && is_word(self.kind)
    }
    fn is_comment(&self) -> bool {
        matches!(self.kind, Tok::LineComment | Tok::BlockComment)
    }
    fn is_comma(&self) -> bool {
        self.kind == Tok::Punctuation && self.text == ","
    }
}

fn is_word(kind: Tok) -> bool {
    matches!(
        kind,
        Tok::Identifier | Tok::QuotedIdentifier | Tok::Keyword | Tok::Builtin | Tok::Type | Tok::Bool | Tok::Null
    )
}

/// Keywords printed in upper case wherever they appear (outside name
/// positions). Words that are commonly also column names (`key`, `first`,
/// `range`, `day`, …) are only upper-cased in context; see `upper_in_context`.
const KEYWORDS: &[&str] = &[
    "select", "from", "where", "and", "or", "not", "in", "is", "null", "like", "ilike", "between", "exists", "case",
    "when", "then", "else", "end", "as", "on", "using", "join", "inner", "left", "right", "full", "outer", "cross",
    "natural", "group", "by", "order", "having", "limit", "offset", "union", "all", "distinct", "intersect", "except",
    "with", "recursive", "insert", "into", "values", "update", "set", "delete", "create", "table", "view", "index",
    "alter", "drop", "add", "primary", "foreign", "references", "unique", "default", "check", "constraint", "if",
    "returning", "over", "partition", "window", "asc", "desc", "nulls", "lateral", "only", "materialized", "fetch",
    "filter", "within", "interval", "cast", "true", "false", "collate", "escape", "similar", "any", "some",
    "conflict", "do", "nothing", "temporary", "temp", "replace", "truncate", "rollup", "cube",
    "grouping", "sets", "qualify", "straight_join", "unbounded", "preceding", "following", "exclude", "ties",
    "current_date", "current_time", "current_timestamp", "localtime", "localtimestamp", "glob", "regexp", "rlike",
    "div", "xor", "unsigned", "zerofill", "auto_increment", "cascade", "restrict", "unlogged", "ignore", "explain",
    "analyze", "semi", "anti", "to", "for", "of", "symmetric", "isnull", "notnull",
];

/// Keywords a `(` may follow with a space; any other word directly before a
/// `(` is a function call and gets none.
const PAREN_KEYWORDS: &[&str] = &[
    "in", "exists", "values", "over", "as", "using", "on", "and", "or", "not", "any", "all", "some", "filter",
    "within", "from", "join", "select", "where", "when", "then", "else", "by", "into", "lateral", "union",
    "intersect", "except", "returning", "set", "key", "references", "check", "unique", "table", "conflict",
    "partition", "with", "recursive", "materialized", "is", "like", "ilike", "between", "case", "distinct",
    "rollup", "cube", "sets", "primary", "foreign", "constraint", "nulls", "having", "limit", "do",
    "return", "returns", "then", "end", "only", "group", "order", "window", "qualify", "of", "to", "for",
];

/// Words after which the next word names a table.
const NAME_AFTER: &[&str] = &["from", "join", "into", "update", "table", "view", "references", "straight_join"];

const INTERVAL_UNITS: &[&str] = &[
    "year", "years", "month", "months", "week", "weeks", "day", "days", "hour", "hours", "minute", "minutes",
    "second", "seconds", "microsecond", "millisecond", "quarter", "day_hour", "day_minute", "day_second",
    "hour_minute", "hour_second", "minute_second", "year_month",
];

/// Format `sql` in the Mozilla style. Returns the input unchanged if it cannot
/// be formatted without changing its tokens.
pub fn format_sql(sql: &str, dialect: Dialect) -> String {
    if sql.trim().is_empty() {
        return sql.to_string();
    }
    let toks = prepare(sql, dialect);
    if nesting_depth(&toks) > MAX_DEPTH {
        return sql.to_string();
    }
    let formatted = format_unchecked(&toks, sql.ends_with('\n'));
    if same_tokens(sql, &formatted, dialect) {
        formatted
    } else {
        sql.to_string()
    }
}

/// `range` widened to whole tokens of `sql`, so formatting a selection never
/// starts or ends inside a string, a quoted name or a comment: a fragment cut
/// from the middle of one would be read as SQL and its spacing changed.
pub fn token_aligned_range(sql: &str, range: std::ops::Range<usize>, dialect: Dialect) -> std::ops::Range<usize> {
    let mut start = range.start.min(sql.len());
    let mut end = range.end.min(sql.len()).max(start);
    for t in tokenize(sql, dialect) {
        if t.kind == Tok::Whitespace {
            continue;
        }
        if t.from < start && start < t.to {
            start = t.from;
        }
        if t.from < end && end < t.to {
            end = t.to;
        }
    }
    start..end
}

fn nesting_depth(toks: &[T]) -> usize {
    let mut depth = 0usize;
    let mut max = 0;
    for t in toks {
        match t.kind {
            Tok::ParenL | Tok::BracketL | Tok::BraceL => {
                depth += 1;
                max = max.max(depth);
            }
            Tok::ParenR | Tok::BracketR | Tok::BraceR => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    max
}

fn format_unchecked(toks: &[T], trailing_newline: bool) -> String {
    let mut out = String::new();
    let mut i = 0;
    while i < toks.len() {
        // One statement: up to and including its `;`, plus a comment on the
        // same line after it.
        let start = i;
        while i < toks.len() && toks[i].kind != Tok::Semi {
            i += 1;
        }
        let body = &toks[start..i];
        let mut terminated = false;
        if i < toks.len() {
            terminated = true;
            i += 1;
        }
        let mut trailing = Vec::new();
        while i < toks.len() && toks[i].is_comment() && !toks[i].newline_before {
            trailing.push(toks[i].text.clone());
            i += 1;
        }
        if body.is_empty() && trailing.is_empty() {
            // `;;`: an empty statement stays attached to the one before.
            if terminated {
                out.push(';');
            }
            continue;
        }
        if !out.is_empty() {
            out.push_str("\n\n");
        }
        out.push_str(&format_statement(body));
        if terminated {
            // A `;` after a line comment would be part of the comment.
            if body.last().is_some_and(|t| t.kind == Tok::LineComment) {
                out.push('\n');
            }
            out.push(';');
        }
        for c in trailing {
            out.push(' ');
            out.push_str(&c);
        }
    }
    let trimmed = out.trim_end().to_string();
    if trailing_newline { trimmed + "\n" } else { trimmed }
}

/// Tokens without whitespace, re-cased, with the context flags set.
fn prepare(sql: &str, dialect: Dialect) -> Vec<T> {
    let raw = tokenize(sql, dialect);
    let mut toks: Vec<T> = Vec::new();
    let mut space = false;
    let mut newline = false;
    for t in &raw {
        let text = &sql[t.from..t.to];
        if t.kind == Tok::Whitespace {
            space = true;
            newline |= text.contains('\n');
            continue;
        }
        toks.push(T {
            kind: t.kind,
            text: text.to_string(),
            // Only words are ever compared by name.
            lower: if is_word(t.kind) { text.to_ascii_lowercase() } else { String::new() },
            space_before: space,
            newline_before: newline,
            name_position: false,
            list_paren: false,
            unary: false,
        });
        // A line comment ends its line.
        newline = t.kind == Tok::LineComment;
        space = newline;
    }
    // Per statement: is it DDL (types are upper-cased throughout)?
    let mut ddl = false;
    let mut first_in_statement = true;
    let sig: Vec<usize> = (0..toks.len()).filter(|&i| !toks[i].is_comment()).collect();
    for (n, &i) in sig.iter().enumerate() {
        if toks[i].kind == Tok::Semi {
            first_in_statement = true;
            continue;
        }
        if first_in_statement {
            ddl = matches!(toks[i].lower.as_str(), "create" | "alter");
            first_in_statement = false;
        }
        let prev = n.checked_sub(1).map(|p| &toks[sig[p]]);
        let next = sig.get(n + 1).map(|&x| &toks[x]);
        let prev_dot = prev.is_some_and(|p| p.kind == Tok::Dot);
        let next_dot = next.is_some_and(|x| x.kind == Tok::Dot);
        let t = &toks[i];
        // Name positions: the word after FROM/JOIN/INTO/…, and in DDL after
        // `IF NOT EXISTS` / `INDEX … ON`. `ONLY` / `LATERAL` pass it on.
        let name_position = is_word(t.kind)
            && prev.is_some_and(|p| {
                is_word(p.kind)
                    && (NAME_AFTER.contains(&p.lower.as_str())
                        || (ddl && matches!(p.lower.as_str(), "exists" | "on"))
                        || (matches!(p.lower.as_str(), "only" | "lateral") && p.name_position))
            })
            && !matches!(t.lower.as_str(), "only" | "lateral" | "if" | "select" | "with" | "values");
        // `ONLY` / `LATERAL` themselves are keywords but carry the position on.
        let carries = is_word(t.kind)
            && matches!(t.lower.as_str(), "only" | "lateral")
            && prev.is_some_and(|p| NAME_AFTER.contains(&p.lower.as_str()));
        let unary = t.kind == Tok::Operator
            && matches!(t.text.as_str(), "-" | "+" | "~")
            && prev.is_none_or(|p| {
                matches!(p.kind, Tok::ParenL | Tok::Operator | Tok::BracketL)
                    || p.is_comma()
                    || (is_word(p.kind) && KEYWORDS.contains(&p.lower.as_str()) && !matches!(p.lower.as_str(), "null" | "true" | "false" | "end"))
            });
        let call = next.is_some_and(|x| x.kind == Tok::ParenL);
        let list_paren = (name_position
            && call
            && prev.is_some_and(|p| matches!(p.lower.as_str(), "into" | "table" | "view" | "references" | "exists") || (ddl && p.lower == "on")))
            // In DDL a word before `(` is as often a name with a column list
            // (`USING btree (email)`) as a call, so its spacing is kept.
            || (ddl && call && t.kind == Tok::Identifier && next.is_some_and(|x| x.space_before));
        let after_cast = prev.is_some_and(|p| p.text == ":" || p.is("as"));
        let case = if !is_word(t.kind) || t.kind == Tok::QuotedIdentifier || prev_dot || next_dot || name_position {
            Case::Keep
        } else if call && !PAREN_KEYWORDS.contains(&t.lower.as_str()) {
            // A function call: built-ins lower case, user identifiers as
            // written. A type with a size (`VARCHAR(10)`) in DDL or a cast is
            // a type; elsewhere `date(x)` is a function.
            match t.kind {
                Tok::Keyword | Tok::Builtin => Case::Lower,
                Tok::Type if ddl || after_cast => Case::Upper,
                Tok::Type => Case::Lower,
                _ => Case::Keep,
            }
        } else if matches!(t.kind, Tok::Bool | Tok::Null) {
            Case::Upper
        } else if t.kind == Tok::Type {
            if ddl || after_cast { Case::Upper } else { Case::Keep }
        } else if t.kind == Tok::Keyword && (KEYWORDS.contains(&t.lower.as_str()) || upper_in_context(t, prev, next)) {
            Case::Upper
        } else if t.kind == Tok::Identifier && upper_in_context(t, prev, next) {
            Case::Upper
        } else {
            Case::Keep
        };
        let t = &mut toks[i];
        t.name_position = name_position || carries;
        t.list_paren = list_paren;
        t.unary = unary;
        match case {
            Case::Upper => t.text = t.text.to_ascii_uppercase(),
            Case::Lower => t.text = t.text.to_ascii_lowercase(),
            Case::Keep => {}
        }
    }
    toks
}

enum Case {
    Upper,
    Lower,
    Keep,
}

/// Words that are keywords only in some places and column names in others.
fn upper_in_context(t: &T, prev: Option<&T>, next: Option<&T>) -> bool {
    let p = prev.map(|p| p.lower.as_str()).unwrap_or("");
    let n = next.map(|n| n.lower.as_str()).unwrap_or("");
    match t.lower.as_str() {
        "key" => matches!(p, "primary" | "foreign" | "unique"),
        "first" | "last" => p == "nulls",
        "rows" | "range" | "groups" => matches!(n, "between" | "unbounded" | "current"),
        "current" => n == "row",
        "row" => matches!(p, "current" | "each") || n == "only",
        "next" => p == "fetch",
        "ordinality" => p == "with",
        "action" => p == "no",
        "no" => n == "action",
        u if INTERVAL_UNITS.contains(&u) => {
            prev.is_some_and(|p| matches!(p.kind, Tok::Number | Tok::String) || p.lower == "interval")
        }
        _ => false,
    }
}

// ─── Statements ─────────────────────────────────────────────────────────────

fn format_statement(toks: &[T]) -> String {
    // Comments before the statement each get their own line.
    let mut lines = Vec::new();
    let mut i = 0;
    while i < toks.len() && toks[i].is_comment() {
        lines.push(toks[i].text.clone());
        i += 1;
    }
    let toks = &toks[i..];
    if !toks.is_empty() {
        lines.push(format_body(toks, 0));
    }
    lines.join("\n")
}

fn format_body(toks: &[T], n: usize) -> String {
    let first = toks.iter().find(|t| !t.is_comment()).map(|t| t.lower.as_str()).unwrap_or("");
    match first {
        "select" | "with" | "insert" | "update" | "delete" | "values" | "replace" => format_query(toks, n),
        "create" => format_create(toks, n),
        "explain" => {
            // EXPLAIN [ANALYZE] [VERBOSE] <query>
            let q = toks.iter().position(|t| matches!(t.lower.as_str(), "select" | "with" | "insert" | "update" | "delete"));
            match q {
                Some(q) if q > 0 => format!("{}{}\n{}", ind(n), inline(&toks[..q], n), format_query(&toks[q..], n)),
                _ => format!("{}{}", ind(n), inline(toks, n)),
            }
        }
        _ if toks[0].kind == Tok::ParenL => format_query(toks, n),
        _ => format!("{}{}", ind(n), inline(toks, n)),
    }
}

/// `CREATE TABLE t (…)` one column per line; `CREATE VIEW v AS <query>` with
/// the query formatted below; anything else on one line.
fn format_create(toks: &[T], n: usize) -> String {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Tok::ParenL => {
                if depth == 0 && toks[..i].iter().any(|t| t.is("table")) && !toks[..i].iter().any(|t| t.is("as")) {
                    let Some(close) = matching(toks, i) else { break };
                    let items = split_commas(&toks[i + 1..close]);
                    let mut s = format!("{}{} (", ind(n), inline(&toks[..i], n));
                    for (k, item) in items.iter().enumerate() {
                        s.push('\n');
                        s.push_str(&ind(n + 1));
                        push_item(&mut s, item, n + 1, k + 1 < items.len());
                    }
                    s.push('\n');
                    s.push_str(&ind(n));
                    s.push(')');
                    if close + 1 < toks.len() {
                        s.push(' ');
                        s.push_str(&inline(&toks[close + 1..], n));
                    }
                    return s;
                }
                depth += 1;
            }
            Tok::ParenR => depth -= 1,
            _ if depth == 0 && t.is("as") => {
                let rest = &toks[i + 1..];
                if rest.iter().find(|t| !t.is_comment()).is_some_and(|t| matches!(t.lower.as_str(), "select" | "with" | "values") || t.kind == Tok::ParenL) {
                    return format!("{}{}\n{}", ind(n), inline(&toks[..=i], n), format_query(rest, n));
                }
            }
            _ => {}
        }
    }
    format!("{}{}", ind(n), inline(toks, n))
}

// ─── Queries ────────────────────────────────────────────────────────────────

#[derive(Clone, Copy, PartialEq, Debug)]
enum Clause {
    Preamble,
    Select,
    From,
    Where,
    GroupBy,
    Having,
    OrderBy,
    Limit,
    Window,
    Qualify,
    Returning,
    Values,
    Set,
    Update,
    Insert,
    Delete,
    SetOp,
    Join,
    On,
    Using,
}

/// A query at indent level `n`: CTEs, then one root keyword per clause.
fn format_query(toks: &[T], n: usize) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut toks = toks;
    // Leading comments.
    while let Some(t) = toks.first().filter(|t| t.is_comment()) {
        lines.push(format!("{}{}", ind(n), t.text));
        toks = &toks[1..];
    }
    // A whole query in brackets: `(SELECT …) UNION (SELECT …)` is handled as
    // items; a single bracketed query is kept as such.
    if toks.first().is_some_and(|t| t.kind == Tok::ParenL) && matching(toks, 0) == Some(toks.len() - 1) {
        let inner = &toks[1..toks.len() - 1];
        lines.push(format!("{}(\n{}\n{})", ind(n), format_query(inner, n + 1), ind(n)));
        return lines.join("\n");
    }
    if toks.first().is_some_and(|t| t.is("with")) {
        let (cte_text, rest) = format_ctes(toks, n);
        lines.push(cte_text);
        toks = rest;
        if toks.is_empty() {
            return lines.join("\n");
        }
        if n == 0 {
            lines.push(String::new());
        }
    }
    let clauses = split_clauses(toks);
    let has_join = clauses.iter().any(|(c, _, _)| *c == Clause::Join);
    for (clause, kw, body) in &clauses {
        let kw_text = inline(kw, n);
        match clause {
            Clause::Preamble => lines.push(format!("{}{}", ind(n), inline(body, n))),
            Clause::Select | Clause::GroupBy | Clause::OrderBy | Clause::Returning | Clause::Window | Clause::Set => {
                lines.push(list_clause(&kw_text, body, n, false))
            }
            Clause::From => lines.push(list_clause(&kw_text, body, n, has_join)),
            Clause::Where | Clause::Having | Clause::Qualify => lines.push(condition_clause(&kw_text, body, n)),
            Clause::Limit | Clause::Update | Clause::Delete => {
                lines.push(join_inline(&format!("{}{}", ind(n), kw_text), &inline(body, n)))
            }
            Clause::Insert => lines.push(insert_clause(&kw_text, body, n)),
            Clause::Values => lines.push(values_clause(&kw_text, body, n)),
            Clause::SetOp => {
                lines.push(format!("{}{}", ind(n), kw_text));
                if !body.is_empty() {
                    lines.push(format!("{}{}", ind(n), item(body, n)));
                }
            }
            Clause::Join => {
                lines.push(format!("{}{}", ind(n), kw_text));
                if !body.is_empty() {
                    lines.push(format!("{}{}", ind(n + 1), item(body, n + 1)));
                }
            }
            Clause::On => lines.push(on_clause(&kw_text, body, n + 1)),
            Clause::Using => lines.push(join_inline(&format!("{}{}", ind(n + 1), kw_text), &inline(body, n + 1))),
        }
    }
    lines.join("\n")
}

fn join_inline(head: &str, rest: &str) -> String {
    if rest.is_empty() {
        head.to_string()
    } else {
        format!("{head} {rest}")
    }
}

/// `WITH a AS (…), b AS (…)`: returns the formatted CTEs and the main query.
fn format_ctes(toks: &[T], n: usize) -> (String, &[T]) {
    let mut i = 1;
    let mut head = vec![toks[0].text.clone()];
    if toks.get(1).is_some_and(|t| t.is("recursive")) {
        head.push(toks[1].text.clone());
        i = 2;
    }
    let mut out = String::new();
    let mut first = true;
    loop {
        // name [(columns)] AS [NOT] [MATERIALIZED] (
        let start = i;
        let Some(open) = (i..toks.len()).find(|&j| toks[j].kind == Tok::ParenL && j > start && toks[j - 1].is_word_like_as()) else {
            break;
        };
        let Some(close) = matching(toks, open) else { break };
        let name = inline(&toks[start..open], n);
        if first {
            out.push_str(&format!("{}{} {} (", ind(n), head.join(" "), name));
            first = false;
        } else {
            out.push_str(&format!("\n{}{} (", ind(n), name));
        }
        out.push('\n');
        out.push_str(&format_query(&toks[open + 1..close], n + 1));
        out.push('\n');
        out.push_str(&ind(n));
        out.push(')');
        i = close + 1;
        if toks.get(i).is_some_and(|t| t.is_comma()) {
            out.push(',');
            i += 1;
            continue;
        }
        break;
    }
    if first {
        // Not a CTE list we understand; leave it to the generic path.
        return (format!("{}{}", ind(n), inline(toks, n)), &[]);
    }
    (out, &toks[i..])
}

impl T {
    /// The `AS` / `MATERIALIZED` that comes right before a CTE's `(`.
    fn is_word_like_as(&self) -> bool {
        self.is("as") || self.is("materialized")
    }
}

/// Root keywords at bracket depth 0, each with its body.
fn split_clauses(toks: &[T]) -> Vec<(Clause, &[T], &[T])> {
    // (clause, keyword start, body start)
    let mut out: Vec<(Clause, usize, usize)> = Vec::new();
    let mut depth = 0i32;
    let mut i = 0;
    let mut current = Clause::Preamble;
    while i < toks.len() {
        let t = &toks[i];
        match t.kind {
            Tok::ParenL | Tok::BracketL | Tok::BraceL => depth += 1,
            Tok::ParenR | Tok::BracketR | Tok::BraceR => depth -= 1,
            _ => {}
        }
        if depth != 0 || t.kind == Tok::ParenL {
            i += 1;
            continue;
        }
        if let Some((clause, len)) = clause_at(toks, i, current, out.is_empty()) {
            out.push((clause, i, i + len));
            current = clause;
            i += len;
            continue;
        }
        i += 1;
    }
    let mut result = Vec::new();
    if out.first().is_none_or(|c| c.1 > 0) {
        let end = out.first().map_or(toks.len(), |c| c.1);
        result.push((Clause::Preamble, &toks[0..0], &toks[..end]));
    }
    for (k, (clause, kw, body)) in out.iter().enumerate() {
        let end = out.get(k + 1).map_or(toks.len(), |c| c.1);
        result.push((*clause, &toks[*kw..*body], &toks[*body..end]));
    }
    result
}

/// The root keyword starting at `i`, and how many tokens it spans.
fn clause_at(toks: &[T], i: usize, current: Clause, first: bool) -> Option<(Clause, usize)> {
    let w = |k: usize| toks.get(i + k).filter(|t| is_word(t.kind)).map(|t| t.lower.as_str()).unwrap_or("");
    if !is_word(toks[i].kind) || toks[i].name_position {
        return None;
    }
    // A keyword directly before `(` is part of an expression (`LEFT(x, 3)`).
    let fun = |k: usize| toks.get(i + k).is_some_and(|t| t.kind == Tok::ParenL);
    Some(match w(0) {
        "select" => {
            let mut len = 1;
            if matches!(w(1), "distinct" | "all") {
                len = 2;
                if w(1) == "distinct" && w(2) == "on" && fun(3) {
                    len = matching(toks, i + 3)? - i + 1;
                }
            }
            (Clause::Select, len)
        }
        "from" if current != Clause::Delete || !first => (Clause::From, 1),
        "where" => (Clause::Where, 1),
        "having" => (Clause::Having, 1),
        "qualify" => (Clause::Qualify, 1),
        "window" => (Clause::Window, 1),
        "returning" => (Clause::Returning, 1),
        "group" if w(1) == "by" => (Clause::GroupBy, 2),
        "order" if w(1) == "by" => (Clause::OrderBy, 2),
        "limit" | "offset" | "fetch" => (Clause::Limit, 1),
        "values" => (Clause::Values, 1),
        "set" if matches!(current, Clause::Update) || (current == Clause::Preamble && !first) => (Clause::Set, 1),
        "update" if first => (Clause::Update, 1),
        "delete" if first => (Clause::Delete, if w(1) == "from" { 2 } else { 1 }),
        "insert" | "replace" if first => {
            let mut len = 1;
            while !matches!(w(len), "into" | "") && len < 4 {
                len += 1;
            }
            if w(len) == "into" {
                len += 1;
            } else {
                len = 1;
            }
            (Clause::Insert, len)
        }
        "union" | "intersect" | "except" | "minus" => (Clause::SetOp, if matches!(w(1), "all" | "distinct") { 2 } else { 1 }),
        "on" if matches!(current, Clause::Join) => (Clause::On, 1),
        "using" if matches!(current, Clause::Join) => (Clause::Using, 1),
        "join" | "straight_join" => (Clause::Join, 1),
        "natural" | "left" | "right" | "full" | "inner" | "cross" | "outer" | "semi" | "anti" => {
            let mut len = 0;
            while len < 4 && matches!(w(len), "natural" | "left" | "right" | "full" | "inner" | "cross" | "outer" | "semi" | "anti") {
                len += 1;
            }
            if w(len) != "join" || fun(len) {
                return None;
            }
            (Clause::Join, len + 1)
        }
        _ => return None,
    })
}

/// A clause whose arguments are a comma list: one per line, or a single short
/// one on the keyword's line.
fn list_clause(kw: &str, body: &[T], n: usize, force_multiline: bool) -> String {
    let items = split_commas(body);
    if items.is_empty() {
        return format!("{}{}", ind(n), kw);
    }
    if items.len() == 1 && !force_multiline {
        let one = item(&items[0], n);
        let line = format!("{}{} {}", ind(n), kw, one);
        if fits_on_keyword_line(&line, &one, n) {
            return line;
        }
        // A single multi-line argument (a CASE, a subquery) starts on the
        // keyword's line only if it opens with a bracket; otherwise below.
    }
    let mut s = format!("{}{}", ind(n), kw);
    for (k, it) in items.iter().enumerate() {
        s.push('\n');
        s.push_str(&ind(n + 1));
        push_item(&mut s, it, n + 1, k + 1 < items.len());
    }
    s
}

/// Whether a clause's single argument can stay on the keyword's line: a short
/// one-liner, or one that opens a bracket at the end of the line and closes
/// it at the keyword's indentation (`WHERE id IN (` … `)`, `FROM (` … `) AS s`).
fn fits_on_keyword_line(line: &str, one: &str, n: usize) -> bool {
    let first = line.lines().next().unwrap_or("");
    if first.len() > MAX_LINE {
        return false;
    }
    if !one.contains('\n') {
        return true;
    }
    let last = one.lines().last().unwrap_or("");
    first.ends_with('(') && last.starts_with(&format!("{})", ind(n)))
}

/// Print an item, its comma, and any comment that trailed it in the source
/// (the comma goes before a line comment, not after it).
fn push_item(s: &mut String, it: &[T], n: usize, comma: bool) {
    let mut end = it.len();
    while end > 0 && it[end - 1].is_comment() && !it[end - 1].newline_before {
        end -= 1;
    }
    let (body, trailing) = if end == 0 { (it, &it[it.len()..]) } else { (&it[..end], &it[end..]) };
    s.push_str(&item(body, n));
    if comma {
        s.push(',');
    }
    for c in trailing {
        s.push(' ');
        s.push_str(&c.text);
    }
}

/// `WHERE` / `HAVING`: one condition per line, `AND` / `OR` leading.
fn condition_clause(kw: &str, body: &[T], n: usize) -> String {
    let parts = split_conditions(body);
    if parts.len() == 1 {
        let one = item(parts[0].1, n);
        let line = format!("{}{} {}", ind(n), kw, one);
        if fits_on_keyword_line(&line, &one, n) {
            return line;
        }
    }
    let mut s = format!("{}{}", ind(n), kw);
    for (op, part) in parts {
        s.push('\n');
        s.push_str(&ind(n + 1));
        if let Some(op) = op {
            s.push_str(&op.text);
            s.push(' ');
        }
        s.push_str(&item(part, n + 1));
    }
    s
}

/// A join's `ON`: the first condition on the `ON` line, further ones below
/// it, each starting with its `AND` / `OR`.
fn on_clause(kw: &str, body: &[T], n: usize) -> String {
    let parts = split_conditions(body);
    let mut s = String::new();
    for (k, (op, part)) in parts.iter().enumerate() {
        if k > 0 {
            s.push('\n');
        }
        s.push_str(&ind(n));
        match op {
            Some(op) => {
                s.push_str(&op.text);
                s.push(' ');
            }
            None => {
                s.push_str(kw);
                s.push(' ');
            }
        }
        s.push_str(&item(part, n));
    }
    if parts.is_empty() {
        s = format!("{}{}", ind(n), kw);
    }
    s
}

/// `INSERT INTO t (a, b)`: the column list inline if it fits, else one per line.
fn insert_clause(kw: &str, body: &[T], n: usize) -> String {
    if let Some(open) = body.iter().position(|t| t.kind == Tok::ParenL) {
        if let Some(close) = matching(body, open) {
            let head = join_inline(&format!("{}{}", ind(n), kw), &inline(&body[..open], n));
            let one = format!("{} {}", head, inline(&body[open..], n));
            if one.len() <= MAX_LINE && !one.contains('\n') {
                return one;
            }
            let mut s = format!("{head} (");
            let cols = split_commas(&body[open + 1..close]);
            for (k, c) in cols.iter().enumerate() {
                s.push('\n');
                s.push_str(&ind(n + 1));
                push_item(&mut s, c, n + 1, k + 1 < cols.len());
            }
            s.push('\n');
            s.push_str(&ind(n));
            s.push(')');
            if close + 1 < body.len() {
                s.push(' ');
                s.push_str(&inline(&body[close + 1..], n));
            }
            return s;
        }
    }
    join_inline(&format!("{}{}", ind(n), kw), &inline(body, n))
}

/// `VALUES (…)`: a single row inline if it fits, else its values one per line;
/// several rows one per line.
fn values_clause(kw: &str, body: &[T], n: usize) -> String {
    let rows = split_commas(body);
    if rows.len() == 1 {
        let row = &rows[0];
        let line = format!("{}{} {}", ind(n), kw, item(row, n));
        if line.len() <= MAX_LINE && !line.contains('\n') {
            return line;
        }
        if row.first().is_some_and(|t| t.kind == Tok::ParenL) && matching(row, 0) == Some(row.len() - 1) {
            let vals = split_commas(&row[1..row.len() - 1]);
            let mut s = format!("{}{} (", ind(n), kw);
            for (k, v) in vals.iter().enumerate() {
                s.push('\n');
                s.push_str(&ind(n + 1));
                push_item(&mut s, v, n + 1, k + 1 < vals.len());
            }
            s.push('\n');
            s.push_str(&ind(n));
            s.push(')');
            return s;
        }
    }
    list_clause(kw, body, n, true)
}

// ─── Expressions ────────────────────────────────────────────────────────────

/// One argument of a clause, printed from column `n * 2`. Usually one line;
/// subqueries, long CASE expressions and long windows span several, each
/// continuation line carrying its own indentation.
fn item(toks: &[T], n: usize) -> String {
    let mut p = Printer::new(n, n * 2);
    print_tokens(&mut p, toks, n);
    p.out
}

/// Single-line rendering (used for keyword phrases and plain runs).
fn inline(toks: &[T], n: usize) -> String {
    item(toks, n)
}

struct Printer<'a> {
    out: String,
    prev: Option<&'a T>,
    /// Indent level of the line being written.
    n: usize,
    /// Column the next character lands in, tracked as text is written so
    /// width checks stay O(1) however long the line gets.
    col: usize,
}

impl<'a> Printer<'a> {
    fn new(n: usize, col: usize) -> Self {
        Printer { out: String::new(), prev: None, n, col }
    }

    fn push(&mut self, text: &str) {
        match text.rfind('\n') {
            Some(nl) => self.col = text[nl + 1..].chars().count(),
            None => self.col += text.chars().count(),
        }
        self.out.push_str(text);
    }

    fn newline(&mut self, n: usize) {
        self.push("\n");
        self.push(&ind(n));
    }

    fn gap(&mut self, prev: Option<&T>, next: &T) {
        if let Some(prev) = prev {
            if prev.kind == Tok::LineComment {
                self.newline(self.n);
            } else if needs_space(prev, next) {
                self.push(" ");
            }
        }
    }

    /// Write one token with the space it needs before it.
    fn token(&mut self, t: &'a T) {
        self.gap(self.prev, t);
        self.push(&t.text);
        self.prev = Some(t);
    }

    /// Write already-rendered text that starts with token `first` and ends with `last`.
    fn text(&mut self, first: &'a T, last: &'a T, text: &str) {
        self.gap(self.prev, first);
        self.push(text);
        self.prev = Some(last);
    }
}

fn print_tokens<'a>(p: &mut Printer<'a>, toks: &'a [T], n: usize) {
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == Tok::ParenL {
            if let Some(close) = matching(toks, i) {
                print_group(p, toks, i, close, n);
                i = close + 1;
                continue;
            }
        }
        if t.is("case") {
            if let Some(end) = matching_end(toks, i) {
                print_case(p, &toks[i..=end], n);
                i = end + 1;
                continue;
            }
        }
        p.token(t);
        i += 1;
    }
}

/// A bracketed group: a subquery opened onto its own lines, an over-long
/// window spelled out clause by clause, anything else inline.
fn print_group<'a>(p: &mut Printer<'a>, toks: &'a [T], open: usize, close: usize, n: usize) {
    let inner = &toks[open + 1..close];
    let first_word = inner.iter().find(|t| !t.is_comment());
    let is_query = first_word.is_some_and(|t| t.is("select") || t.is("with") || (t.is("values") && open > 0 && !toks[open - 1].is("in")));
    if is_query {
        let text = format!("(\n{}\n{})", format_query(inner, n + 1), ind(n));
        p.text(&toks[open], &toks[close], &text);
        return;
    }
    // Inline first; a window (`OVER (…)`) that doesn't fit is spelled out.
    let mut q = Printer::new(n, p.col + 1);
    q.token(&toks[open]);
    print_tokens(&mut q, inner, n);
    q.token(&toks[close]);
    let over = open > 0 && toks[open - 1].is("over");
    let fits = !q.out.contains('\n') && q.col <= MAX_LINE;
    if over && (!fits || q.out.contains('\n')) {
        let clauses = window_clauses(inner);
        let mut text = String::from("(");
        for (kw, body) in clauses {
            text.push('\n');
            if body.is_empty() {
                text.push_str(&format!("{}{}", ind(n + 1), inline(kw, n + 1)));
            } else if kw.is_empty() {
                text.push_str(&format!("{}{}", ind(n + 1), item(body, n + 1)));
            } else {
                text.push_str(&list_clause(&inline(kw, n + 1), body, n + 1, false));
            }
        }
        text.push('\n');
        text.push_str(&ind(n));
        text.push(')');
        p.text(&toks[open], &toks[close], &text);
        return;
    }
    let text = q.out;
    p.text(&toks[open], &toks[close], &text);
}

/// `PARTITION BY …`, `ORDER BY …` and the frame of a window, split apart.
fn window_clauses(inner: &[T]) -> Vec<(&[T], &[T])> {
    let mut starts = Vec::new();
    let mut depth = 0i32;
    for (i, t) in inner.iter().enumerate() {
        match t.kind {
            Tok::ParenL => depth += 1,
            Tok::ParenR => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        let next_by = inner.get(i + 1).is_some_and(|x| x.is("by"));
        if (t.is("partition") || t.is("order")) && next_by {
            starts.push((i, 2));
        } else if matches!(t.lower.as_str(), "rows" | "range" | "groups") && is_word(t.kind) {
            starts.push((i, 0));
        }
    }
    let mut out = Vec::new();
    if starts.first().is_none_or(|s| s.0 > 0) {
        let end = starts.first().map_or(inner.len(), |s| s.0);
        out.push((&inner[0..0], &inner[..end]));
    }
    for (k, &(s, len)) in starts.iter().enumerate() {
        let end = starts.get(k + 1).map_or(inner.len(), |x| x.0);
        out.push((&inner[s..s + len], &inner[s + len..end]));
    }
    out
}

/// `CASE … END`: inline when short with a single `WHEN`, otherwise one `WHEN`
/// / `ELSE` per line between `CASE` and `END`.
fn print_case<'a>(p: &mut Printer<'a>, toks: &'a [T], n: usize) {
    let mut q = Printer::new(n, p.col + 1);
    for t in toks {
        q.token(t);
    }
    let whens = case_parts(toks).iter().filter(|(k, _)| toks[*k].is("when")).count();
    let fits = q.col <= MAX_LINE;
    let has_comment = toks.iter().any(|t| t.kind == Tok::LineComment);
    let nested = toks[1..toks.len() - 1].iter().any(|t| t.is("case"));
    if whens <= 1 && fits && !has_comment && !nested {
        // Re-render through the full printer so nested groups are handled.
        let mut r = Printer::new(n, p.col + 1);
        print_tokens_flat(&mut r, toks, n);
        let text = r.out;
        p.text(&toks[0], &toks[toks.len() - 1], &text);
        return;
    }
    let parts = case_parts(toks);
    // CASE [operand]
    let head_end = parts.first().map_or(toks.len() - 1, |(k, _)| *k);
    let mut text = item(&toks[..head_end], n);
    for (start, end) in &parts {
        text.push('\n');
        text.push_str(&ind(n + 1));
        text.push_str(&item(&toks[*start..*end], n + 1));
    }
    text.push('\n');
    text.push_str(&ind(n));
    text.push_str(&toks[toks.len() - 1].text);
    p.text(&toks[0], &toks[toks.len() - 1], &text);
}

/// Inline rendering of a CASE, with nested brackets handled as usual.
fn print_tokens_flat<'a>(p: &mut Printer<'a>, toks: &'a [T], n: usize) {
    let mut i = 0;
    while i < toks.len() {
        if toks[i].kind == Tok::ParenL {
            if let Some(close) = matching(toks, i) {
                print_group(p, toks, i, close, n);
                i = close + 1;
                continue;
            }
        }
        p.token(&toks[i]);
        i += 1;
    }
}

/// The `WHEN …` / `ELSE …` parts of a CASE (start, end) at its own level.
fn case_parts(toks: &[T]) -> Vec<(usize, usize)> {
    let mut starts = Vec::new();
    let mut depth = 0i32;
    let mut cases = 0i32;
    for (i, t) in toks.iter().enumerate().take(toks.len() - 1).skip(1) {
        match t.kind {
            Tok::ParenL => depth += 1,
            Tok::ParenR => depth -= 1,
            _ => {}
        }
        if t.is("case") {
            cases += 1;
        } else if t.is("end") && cases > 0 {
            cases -= 1;
        } else if depth == 0 && cases == 0 && (t.is("when") || t.is("else")) {
            starts.push(i);
        }
    }
    let last = toks.len() - 1;
    starts.iter().enumerate().map(|(k, &s)| (s, starts.get(k + 1).copied().unwrap_or(last))).collect()
}

/// Whether a space goes between two tokens.
/// Tokens whose meaning can depend on what touches them: `'O''Neil'` is
/// two string tokens to the tokenizer but one literal to the database, and
/// `'a' 'b'` is an error in Postgres where `'a''b'` is fine.
fn is_atom(kind: Tok) -> bool {
    is_word(kind) || matches!(kind, Tok::String | Tok::Number | Tok::SpecialVar | Tok::Bits | Tok::Bytes)
}

fn needs_space(prev: &T, cur: &T) -> bool {
    if !cur.space_before && is_atom(prev.kind) && is_atom(cur.kind) {
        return false;
    }
    // Never write `--` or `/*` (or `*/`) by joining two tokens: that would
    // start (or end) a comment.
    let (p, c) = (prev.text.as_bytes(), cur.text.as_bytes());
    if let (Some(&a), Some(&b)) = (p.last(), c.first()) {
        if (a == b'-' && b == b'-') || (a == b'/' && b == b'*') || (a == b'*' && b == b'/') {
            return true;
        }
    }
    // Punctuation the tokenizer splits finely (`::`, `:=`), unknown characters
    // (`$1`) and variables keep exactly the spacing they had.
    let glued = |t: &T| matches!(t.kind, Tok::Invalid) || (t.kind == Tok::Punctuation && t.text != ",");
    if glued(prev) || glued(cur) {
        return cur.space_before;
    }
    if cur.is_comma() || matches!(cur.kind, Tok::ParenR | Tok::BracketR | Tok::Semi) {
        return false;
    }
    if matches!(prev.kind, Tok::ParenL | Tok::BracketL) {
        return false;
    }
    if prev.kind == Tok::Dot || cur.kind == Tok::Dot {
        return false;
    }
    if prev.unary {
        return false;
    }
    if cur.kind == Tok::ParenL {
        // A call: a word right before `(` that isn't a keyword taking a
        // bracketed argument, and isn't a table name before its column list.
        if is_word(prev.kind) && !prev.list_paren {
            let keyword = prev.kind != Tok::QuotedIdentifier && PAREN_KEYWORDS.contains(&prev.lower.as_str());
            return keyword;
        }
        if prev.kind == Tok::ParenR || prev.kind == Tok::BracketR {
            return cur.space_before;
        }
        return true;
    }
    if cur.kind == Tok::BracketL {
        // Array subscripts (`a[1]`) and SQLite bracket identifiers.
        return cur.space_before;
    }
    if prev.kind == Tok::SpecialVar || cur.kind == Tok::SpecialVar {
        return cur.space_before || prev.is_comma() || is_word(prev.kind) || prev.kind == Tok::Operator;
    }
    true
}

// ─── Splitting helpers ──────────────────────────────────────────────────────

/// The index of the bracket closing the one at `open`.
fn matching(toks: &[T], open: usize) -> Option<usize> {
    let mut depth = 0i32;
    for (i, t) in toks.iter().enumerate().skip(open) {
        match t.kind {
            Tok::ParenL | Tok::BracketL | Tok::BraceL => depth += 1,
            Tok::ParenR | Tok::BracketR | Tok::BraceR => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
    }
    None
}

/// The `END` closing the `CASE` at `start`.
fn matching_end(toks: &[T], start: usize) -> Option<usize> {
    let mut depth = 0i32;
    let mut cases = 0i32;
    for (i, t) in toks.iter().enumerate().skip(start) {
        match t.kind {
            Tok::ParenL => depth += 1,
            Tok::ParenR => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return None;
        }
        if t.is("case") {
            cases += 1;
        } else if t.is("end") {
            cases -= 1;
            if cases == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// Split at commas outside brackets. A comment right after a comma on the same
/// source line belongs to the item before it.
fn split_commas(toks: &[T]) -> Vec<Vec<T>> {
    let mut items: Vec<Vec<T>> = Vec::new();
    let mut cur: Vec<T> = Vec::new();
    let mut depth = 0i32;
    let mut cases = 0i32;
    let mut after_comma = false;
    for t in toks {
        match t.kind {
            Tok::ParenL | Tok::BracketL | Tok::BraceL => depth += 1,
            Tok::ParenR | Tok::BracketR | Tok::BraceR => depth -= 1,
            _ => {}
        }
        if t.is("case") {
            cases += 1;
        } else if t.is("end") && cases > 0 {
            cases -= 1;
        }
        if depth == 0 && cases == 0 && t.is_comma() {
            items.push(std::mem::take(&mut cur));
            after_comma = true;
            continue;
        }
        if after_comma && t.is_comment() && !t.newline_before {
            if let Some(last) = items.last_mut() {
                last.push(t.clone());
                continue;
            }
        }
        after_comma = false;
        cur.push(t.clone());
    }
    if !cur.is_empty() || !items.is_empty() {
        items.push(cur);
    }
    items.retain(|i| !i.is_empty());
    items
}

/// Split a condition at `AND` / `OR` outside brackets, CASE expressions and
/// `BETWEEN … AND …`.
fn split_conditions(toks: &[T]) -> Vec<(Option<&T>, &[T])> {
    let mut parts = Vec::new();
    let mut depth = 0i32;
    let mut cases = 0i32;
    let mut between = false;
    let mut start = 0;
    let mut op: Option<&T> = None;
    for (i, t) in toks.iter().enumerate() {
        match t.kind {
            Tok::ParenL | Tok::BracketL => depth += 1,
            Tok::ParenR | Tok::BracketR => depth -= 1,
            _ => {}
        }
        if depth != 0 {
            continue;
        }
        if t.is("case") {
            cases += 1;
        } else if t.is("end") && cases > 0 {
            cases -= 1;
        }
        if cases > 0 {
            continue;
        }
        if t.is("between") {
            between = true;
        } else if t.is("and") && between {
            between = false;
        } else if (t.is("and") || t.is("or")) && i > start {
            parts.push((op, &toks[start..i]));
            op = Some(t);
            start = i + 1;
        }
    }
    if start < toks.len() {
        parts.push((op, &toks[start..]));
    }
    parts
}

fn ind(n: usize) -> String {
    "  ".repeat(n)
}

/// Whether two texts have the same tokens, comments included (words compared
/// without case), and every pair of atoms that touched in one still touches
/// in the other (see `is_atom`).
fn same_tokens(a: &str, b: &str, dialect: Dialect) -> bool {
    // (kind, text, touches the atom before it), compared without allocating.
    let keys = |sql: &str| {
        let mut prev: Option<(Tok, usize)> = None;
        tokenize(sql, dialect)
            .into_iter()
            .filter(|t| t.kind != Tok::Whitespace)
            .map(|t| {
                let touching = prev.is_some_and(|(k, end)| end == t.from && is_atom(k) && is_atom(t.kind));
                prev = Some((t.kind, t.to));
                (t.kind, t.from, t.to, touching)
            })
            .collect::<Vec<_>>()
    };
    let (ka, kb) = (keys(a), keys(b));
    ka.len() == kb.len()
        && ka.iter().zip(&kb).all(|(x, y)| {
            let (tx, ty) = (a[x.1..x.2].trim_end(), b[y.1..y.2].trim_end());
            let text_eq = if is_word(x.0) && x.0 != Tok::QuotedIdentifier { tx.eq_ignore_ascii_case(ty) } else { tx == ty };
            x.0 == y.0 && x.3 == y.3 && text_eq
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pg(sql: &str) -> String {
        format_sql(sql, Dialect::PostgreSql)
    }

    #[test]
    fn mozilla_example() {
        let sql = "select client_id, submission_date from main_summary where sample_id = '42' and submission_date > '2018-01-01' limit 10";
        assert_eq!(
            pg(sql),
            "SELECT\n  client_id,\n  submission_date\nFROM main_summary\nWHERE\n  sample_id = '42'\n  AND submission_date > '2018-01-01'\nLIMIT 10"
        );
    }

    #[test]
    fn single_arguments_share_the_line() {
        assert_eq!(pg("select * from t limit 100;"), "SELECT *\nFROM t\nLIMIT 100;");
    }

    #[test]
    fn functions_lower_keywords_upper_identifiers_kept() {
        assert_eq!(
            pg("SELECT DATE(Submission_Timestamp) as day, COUNT(*) as n FROM Telemetry.Main GROUP BY 1"),
            "SELECT\n  date(Submission_Timestamp) AS day,\n  count(*) AS n\nFROM Telemetry.Main\nGROUP BY 1"
        );
    }

    #[test]
    fn joins() {
        let sql = "select a.x, b.y from a left join b on a.id = b.id and b.ok cross join c";
        assert_eq!(
            pg(sql),
            "SELECT\n  a.x,\n  b.y\nFROM\n  a\nLEFT JOIN\n  b\n  ON a.id = b.id\n  AND b.ok\nCROSS JOIN\n  c"
        );
    }

    #[test]
    fn ctes() {
        let sql = "with sample as (select client_id from main_summary where sample_id = '42') select * from sample limit 10";
        assert_eq!(
            pg(sql),
            "WITH sample AS (\n  SELECT client_id\n  FROM main_summary\n  WHERE sample_id = '42'\n)\n\nSELECT *\nFROM sample\nLIMIT 10"
        );
    }

    #[test]
    fn subqueries_and_case() {
        let sql = "select case when a = 1 then 'one' when a = 2 then 'two' else 'many' end as n from t where id in (select id from u)";
        assert_eq!(
            pg(sql),
            "SELECT\n  CASE\n    WHEN a = 1 THEN 'one'\n    WHEN a = 2 THEN 'two'\n    ELSE 'many'\n  END AS n\nFROM t\nWHERE id IN (\n  SELECT id\n  FROM u\n)"
        );
    }

    #[test]
    fn insert_update_templates() {
        assert_eq!(
            format_sql("INSERT INTO users (id, name)\nVALUES (?, ?);\n", Dialect::MySql),
            "INSERT INTO users (id, name)\nVALUES (?, ?);\n"
        );
        assert_eq!(
            format_sql("UPDATE users\nSET name = ?,\n    email = ?\nWHERE id = ?\n  AND org = ?;\n", Dialect::MySql),
            "UPDATE users\nSET\n  name = ?,\n  email = ?\nWHERE\n  id = ?\n  AND org = ?;\n"
        );
    }

    #[test]
    fn create_table() {
        let sql = "CREATE TABLE `t` (\n`id` int NOT NULL AUTO_INCREMENT, `name` varchar(255) DEFAULT NULL, PRIMARY KEY (`id`)) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4";
        assert_eq!(
            format_sql(sql, Dialect::MySql),
            "CREATE TABLE `t` (\n  `id` INT NOT NULL AUTO_INCREMENT,\n  `name` VARCHAR(255) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4"
        );
    }

    #[test]
    fn postgres_operators_survive() {
        let sql = "select q.parameters->>'tier' as tier, k.value::numeric, $1 from quote q cross join lateral jsonb_array_elements_text(q.parameters->'kcals') with ordinality as k(value, ordinality) where q.pet_id = -1";
        assert_eq!(
            pg(sql),
            "SELECT\n  q.parameters ->> 'tier' AS tier,\n  k.value::NUMERIC,\n  $1\nFROM\n  quote q\nCROSS JOIN\n  LATERAL jsonb_array_elements_text(q.parameters -> 'kcals') WITH ORDINALITY AS k(value, ordinality)\nWHERE q.pet_id = -1"
        );
    }

    #[test]
    fn comments_are_kept() {
        let sql = "-- top\nselect a, -- first\n b /* inline */ from t; -- after";
        let out = pg(sql);
        assert!(same_tokens(sql, &out, Dialect::PostgreSql), "{out}");
        assert_eq!(out, "-- top\nSELECT\n  a, -- first\n  b /* inline */\nFROM t; -- after");
    }

    #[test]
    fn idempotent_and_token_preserving() {
        for sql in [
            "select a, b from t where x between 1 and 2 and y or z order by a desc nulls last",
            "SELECT store_slug, vat_rate FROM preferred WHERE priority=1;",
            "WITH effective AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY store_slug, vat_rate_type ORDER BY start_date DESC, vat_config_id DESC) as rn FROM vat_config WHERE start_date <= CURRENT_DATE), preferred AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY store_slug ORDER BY CASE vat_rate_type WHEN 'reduced' THEN 0 ELSE 1 END) AS priority FROM effective WHERE rn=1 AND vat_rate_type IN ('reduced', 'standard')) SELECT store_slug, vat_rate FROM preferred WHERE priority=1;",
            "select 1; select 2;",
            "delete from t where id = 3",
            "select x from (select 1 as x) s union all select 2",
            "select extract(year from d), trim(both ' ' from s), substring(s from 2 for 3) from t",
            "insert into t (a, b) select a, b from u on conflict (a) do update set b = excluded.b returning *",
            "select * from t where a is distinct from b and c not in (1, 2, 3)",
            "SELECT p.store_id, CASE WHEN b.end_date IS NULL THEN 'open-ended' WHEN b.end_date >= CURDATE() - INTERVAL 12 MONTH THEN 'expired<12m' ELSE 'old' END AS bucket, COUNT(*) AS codes FROM promo_code c JOIN promo p ON p.id = c.promo_id GROUP BY 1, 2",
            "select a from t\n-- trailing comment",
            "explain analyze select * from t",
            "create view v as select a from t",
            "alter table t add column c int",
            "select count(distinct a) filter (where b > 0) from t",
            "select @v := 1, b <=> c",
        ] {
            for d in [Dialect::PostgreSql, Dialect::MySql, Dialect::Sqlite] {
                let once = format_sql(sql, d);
                assert!(same_tokens(sql, &once, d), "tokens changed for {sql:?} ({d:?}):\n{once}");
                assert_eq!(format_sql(&once, d), once, "not idempotent for {sql:?} ({d:?})");
            }
        }
    }

    /// Every kind of statement the app formats, against the exact text it
    /// should produce. These are the reference ("good") layouts.
    #[test]
    fn golden_examples() {
        let cases: &[(&str, Dialect, &str, &str)] = &[
            (
                "style guide: root keywords and arguments",
                Dialect::PostgreSql,
                "select client_id, submission_date from main_summary where sample_id = '42' limit 10",
                "SELECT\n  client_id,\n  submission_date\nFROM main_summary\nWHERE sample_id = '42'\nLIMIT 10",
            ),
            (
                "style guide: select list, aliases, lower-case functions",
                Dialect::PostgreSql,
                "SELECT DATE(submission_timestamp) AS day, os_name, COUNT(*) AS n FROM telemetry.main GROUP BY day, os_name",
                "SELECT\n  date(submission_timestamp) AS day,\n  os_name,\n  count(*) AS n\nFROM telemetry.main\nGROUP BY\n  day,\n  os_name",
            ),
            (
                "style guide: joins",
                Dialect::PostgreSql,
                "select * from telemetry_stable.main_v4 left join static.normalized_os_name on main_v4.environment.system.os.name = normalized_os_name.os_name",
                "SELECT *\nFROM\n  telemetry_stable.main_v4\nLEFT JOIN\n  static.normalized_os_name\n  ON main_v4.environment.system.os.name = normalized_os_name.os_name",
            ),
            (
                "style guide: AND/OR lead",
                Dialect::PostgreSql,
                "select 1 from t where submission_date > '2018-01-01' and sample_id = '42' or x",
                "SELECT 1\nFROM t\nWHERE\n  submission_date > '2018-01-01'\n  AND sample_id = '42'\n  OR x",
            ),
            (
                "style guide: CTE",
                Dialect::PostgreSql,
                "with sample as (select client_id, submission_date from main_summary where sample_id = '42') select * from sample limit 10",
                "WITH sample AS (\n  SELECT\n    client_id,\n    submission_date\n  FROM main_summary\n  WHERE sample_id = '42'\n)\n\nSELECT *\nFROM sample\nLIMIT 10",
            ),
            (
                "issue query 1: LATERAL function WITH ORDINALITY",
                Dialect::PostgreSql,
                "SELECT q.quote_id, q.parameters->>'tier' AS tier, q.parameters->> 'Strategy_id' AS strategy_id, k.ordinality AS annual_point_number, k.value::numeric AS daily_kcal\nFROM quote q\nCROSS JOIN LATERAL Jsonb_array_elements_text (q.parameters-> 'kcals')\nWITH ORDINALITY AS k(value, ordinality)\nWHERE q.pet_id = 12345\nORDER BY q.quote_id, k.ordinality;",
                "SELECT\n  q.quote_id,\n  q.parameters ->> 'tier' AS tier,\n  q.parameters ->> 'Strategy_id' AS strategy_id,\n  k.ordinality AS annual_point_number,\n  k.value::NUMERIC AS daily_kcal\nFROM\n  quote q\nCROSS JOIN\n  LATERAL Jsonb_array_elements_text(q.parameters -> 'kcals') WITH ORDINALITY AS k(value, ordinality)\nWHERE q.pet_id = 12345\nORDER BY\n  q.quote_id,\n  k.ordinality;",
            ),
            (
                "issue query 2: CTEs, windows, CASE",
                Dialect::PostgreSql,
                "WITH effective AS (\nSELECT *, ROW_NUMBER() OVER (\nPARTITION BY store_slug, vat_rate_type ORDER BY start_date DESC, vat_config_id DESC\n) as rn\n FROM vat_config WHERE start_date <= CURRENT_DATE ), preferred AS (\nSELECT *, ROW_NUMBER() OVER (\nPARTITION BY store_slug\nORDER BY CASE vat_rate_type WHEN 'reduced' THEN 0 ELSE 1 END\n) AS priority\nFROM effective WHERE rn=1 AND vat_rate_type IN ('reduced', 'standard')\n)\nSELECT store_slug, vat_rate FROM preferred WHERE priority=1;",
                "WITH effective AS (\n  SELECT\n    *,\n    row_number() OVER (\n      PARTITION BY\n        store_slug,\n        vat_rate_type\n      ORDER BY\n        start_date DESC,\n        vat_config_id DESC\n    ) AS rn\n  FROM vat_config\n  WHERE start_date <= CURRENT_DATE\n),\npreferred AS (\n  SELECT\n    *,\n    row_number() OVER (\n      PARTITION BY store_slug\n      ORDER BY CASE vat_rate_type WHEN 'reduced' THEN 0 ELSE 1 END\n    ) AS priority\n  FROM effective\n  WHERE\n    rn = 1\n    AND vat_rate_type IN ('reduced', 'standard')\n)\n\nSELECT\n  store_slug,\n  vat_rate\nFROM preferred\nWHERE priority = 1;",
            ),
            (
                "View Data template",
                Dialect::Sqlite,
                "SELECT * FROM albums LIMIT 100;",
                "SELECT *\nFROM albums\nLIMIT 100;",
            ),
            (
                "INSERT template",
                Dialect::Sqlite,
                "INSERT INTO albums (AlbumId, Title, ArtistId)\nVALUES (?, ?, ?);\n",
                "INSERT INTO albums (AlbumId, Title, ArtistId)\nVALUES (?, ?, ?);\n",
            ),
            (
                "UPDATE template",
                Dialect::Sqlite,
                "UPDATE albums\nSET Title = ?,\n    ArtistId = ?\nWHERE AlbumId = ?;\n",
                "UPDATE albums\nSET\n  Title = ?,\n  ArtistId = ?\nWHERE AlbumId = ?;\n",
            ),
            (
                "saved grid edits",
                Dialect::PostgreSql,
                "UPDATE \"public\".\"users\"\nSET \"name\" = 'O''Neil'\nWHERE \"id\" = 7;\n\nUPDATE \"public\".\"users\"\nSET \"name\" = 'Ann',\n    \"email\" = 'a@b.c'\nWHERE \"id\" = 8;",
                "UPDATE \"public\".\"users\"\nSET \"name\" = 'O''Neil'\nWHERE \"id\" = 7;\n\nUPDATE \"public\".\"users\"\nSET\n  \"name\" = 'Ann',\n  \"email\" = 'a@b.c'\nWHERE \"id\" = 8;",
            ),
            (
                "Postgres CREATE TABLE template",
                Dialect::PostgreSql,
                "CREATE TABLE users (\n    id integer NOT NULL,\n    email text,\n    CONSTRAINT users_pkey PRIMARY KEY (id)\n);\n\nCREATE INDEX users_email_idx ON public.users USING btree (email);\n",
                "CREATE TABLE users (\n  id INTEGER NOT NULL,\n  email TEXT,\n  CONSTRAINT users_pkey PRIMARY KEY (id)\n);\n\nCREATE INDEX users_email_idx ON public.users USING btree (email);\n",
            ),
            (
                "MySQL CREATE TABLE template",
                Dialect::MySql,
                "CREATE TABLE `t` (\n  `id` int NOT NULL AUTO_INCREMENT,\n  `name` varchar(255) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE=InnoDB DEFAULT CHARSET=utf8mb4;\n",
                "CREATE TABLE `t` (\n  `id` INT NOT NULL AUTO_INCREMENT,\n  `name` VARCHAR(255) DEFAULT NULL,\n  PRIMARY KEY (`id`)\n) ENGINE = InnoDB DEFAULT CHARSET = utf8mb4;\n",
            ),
            (
                "subqueries open onto their own lines",
                Dialect::PostgreSql,
                "select a, (select max(b) from u where u.a = t.a) as mb from t where exists (select 1 from v where v.id = t.id) and t.x > 0",
                "SELECT\n  a,\n  (\n    SELECT max(b)\n    FROM u\n    WHERE u.a = t.a\n  ) AS mb\nFROM t\nWHERE\n  EXISTS (\n    SELECT 1\n    FROM v\n    WHERE v.id = t.id\n  )\n  AND t.x > 0",
            ),
        ];
        for (name, dialect, input, expected) in cases {
            let out = format_sql(input, *dialect);
            assert_eq!(&out, expected, "{name}\n--- got ---\n{out}\n--- want ---\n{expected}");
            // The good version is a fixed point.
            assert_eq!(&format_sql(expected, *dialect), expected, "{name}: not idempotent");
        }
    }

    #[test]
    fn review_regressions() {
        // Two minus signs are never joined into a comment.
        assert_eq!(pg("select - -1"), "SELECT - -1");
        // A `;` after a line comment goes on its own line.
        assert_eq!(pg("select 1 -- note\n;"), "SELECT 1 -- note\n;");
        // Empty statements don't open blank lines.
        assert_eq!(pg("select 1;;"), "SELECT 1;;");
        // MySQL built-ins stay attached to their bracket.
        assert_eq!(format_sql("select if(a, 1, 2), count(*) from t", Dialect::MySql), "SELECT\n  if(a, 1, 2),\n  count(*)\nFROM t");
        // Tokens that touch keep touching: `'O''Neil'` is one literal.
        assert_eq!(pg("select 'O''Neil', e'x', x'0f'"), "SELECT\n  'O''Neil',\n  e'x',\n  x'0f'");
        // Absurd nesting is left alone rather than overflowing the stack.
        let deep = format!("select {}1{}", "(".repeat(20_000), ")".repeat(20_000));
        assert_eq!(pg(&deep), deep);
    }

    #[test]
    fn selections_are_widened_to_whole_tokens() {
        let sql = "SELECT 'a   b' AS x, /* c   d */ y FROM t";
        let inside_string = sql.find("  b").unwrap();
        let r = token_aligned_range(sql, inside_string..sql.len(), Dialect::PostgreSql);
        assert_eq!(&sql[r.clone()], &sql[sql.find('\'').unwrap()..]);
        let in_comment = sql.find("c ").unwrap();
        let r = token_aligned_range(sql, 0..in_comment, Dialect::PostgreSql);
        assert_eq!(&sql[r], &sql[..sql.find("*/").unwrap() + 2]);
        // Already on boundaries: unchanged.
        assert_eq!(token_aligned_range(sql, 0..6, Dialect::PostgreSql), 0..6);
    }

    #[test]
    fn huge_in_list_is_linear() {
        let ids: Vec<String> = (1..=100_000).map(|i| i.to_string()).collect();
        let sql = format!("select * from t where id in ({}) and deleted = false;", ids.join(","));
        let start = std::time::Instant::now();
        let out = pg(&sql);
        let elapsed = start.elapsed();
        assert!(out.starts_with("SELECT *\nFROM t\nWHERE\n  id IN (1, 2, 3, "), "{}", &out[..80]);
        assert!(out.ends_with(", 99999, 100000)\n  AND deleted = FALSE;"));
        // Generous for unoptimised test builds; see the release timing.
        assert!(elapsed.as_secs_f64() < 3.0, "took {elapsed:?}");
        eprintln!("100k-id IN list: {elapsed:?}");
        // Many small groups on one long line (the case that used to be quadratic).
        let pairs: Vec<String> = (1..=20_000).map(|i| format!("({i},{i})")).collect();
        let sql = format!("select * from t where (a, b) in ({})", pairs.join(","));
        let start = std::time::Instant::now();
        let _ = pg(&sql);
        let elapsed = start.elapsed();
        assert!(elapsed.as_secs_f64() < 3.0, "took {elapsed:?}");
        eprintln!("20k tuple IN list: {elapsed:?}");
    }

    #[test]
    fn half_typed_sql_never_panics() {
        let samples = [
            "WITH e AS (SELECT *, ROW_NUMBER() OVER (PARTITION BY a ORDER BY CASE b WHEN 'x' THEN 0 ELSE 1 END) AS rn FROM t), p AS (SELECT * FROM e WHERE rn = 1) SELECT a, (SELECT max(c) FROM u WHERE u.a = p.a) FROM p LEFT JOIN q ON q.id = p.id AND q.x BETWEEN 1 AND 2 WHERE EXISTS (SELECT 1) GROUP BY a ORDER BY a LIMIT 5; -- done",
            "INSERT INTO t (a, b) VALUES (?, ?), (?, ?) ON CONFLICT (a) DO UPDATE SET b = excluded.b RETURNING *;",
            "CREATE TABLE IF NOT EXISTS t (id INT PRIMARY KEY, name VARCHAR(10) DEFAULT 'x', CHECK (id > 0));",
            "select q.p->>'t', k.v::numeric, $1, @v := 2, [col], e'x' /* c */ from quote q cross join lateral f(q.p) with ordinality as k(v, o)",
        ];
        for sql in samples {
            for (i, _) in sql.char_indices() {
                for d in [Dialect::PostgreSql, Dialect::MySql, Dialect::Sqlite] {
                    let out = format_sql(&sql[..i], d);
                    assert!(same_tokens(&sql[..i], &out, d));
                }
            }
        }
    }

    #[test]
    fn long_windows_are_spelled_out() {
        let sql = "SELECT *, ROW_NUMBER() OVER (PARTITION BY store_slug, vat_rate_type ORDER BY start_date DESC, vat_config_id DESC) AS rn FROM vat_config";
        assert_eq!(
            pg(sql),
            "SELECT\n  *,\n  row_number() OVER (\n    PARTITION BY\n      store_slug,\n      vat_rate_type\n    ORDER BY\n      start_date DESC,\n      vat_config_id DESC\n  ) AS rn\nFROM vat_config"
        );
    }
}
