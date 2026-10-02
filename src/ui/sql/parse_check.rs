//! Context-aware syntax checking, on top of the `sqlparser` crate.
//!
//! The tokenizer and the hand-written checks next door only see one token at a
//! time: they can tell that `(` was never closed, but not that `SELECT * FROM t
//! WHER x = 1` has a word in a place no word belongs. That needs a parser that
//! knows the grammar — and one that knows the *right* grammar, because the
//! dialects disagree about enough (backticks, `LIMIT a, b`, `::` casts) that a
//! single grammar would be wrong for somebody.
//!
//! A parser is a blunt instrument for an editor, though. Half-typed SQL is
//! invalid SQL, and nobody wants to be told so while they are still typing, so
//! everything here is built around **not** crying wolf. Four rules keep it
//! quiet, and a diagnostic has to survive all of them:
//!
//! 1. **Both grammars must reject it.** The statement is parsed with the
//!    connection's dialect *and* with the generic one. If either accepts it,
//!    nothing is reported — a statement only one grammar dislikes is far more
//!    likely to be valid syntax the crate does not implement than a mistake.
//! 2. **The error must land on a token.** Errors that run off the end of the
//!    input ("expected an expression, found: EOF") are what every unfinished
//!    statement produces, so they are dropped.
//! 3. **An unfinished statement is not wrong yet.** If the error is on the last
//!    token of a statement that has no `;`, it is being typed. Dropped.
//! 4. **The caret must have left.** Reported diagnostics carry the range that
//!    silences them, and the editor hides them while the caret is inside it, so
//!    the underline appears when the person moves on and vanishes again the
//!    moment they go back to fix it — without re-parsing anything.
//!
//! Rules 1-3 run here, off the UI thread. Rule 4 is applied at paint time,
//! against the live caret, because the caret moves far more often than the text
//! changes.

use super::lint::Diagnostic;
use super::{statements, Dialect, Tok, Token};
use std::ops::Range;

/// Check every statement in `doc`, returning one diagnostic per bad statement.
pub fn parse_diagnostics(doc: &str, dialect: Dialect, tokens: &[Token]) -> Vec<Diagnostic> {
    let mut out = Vec::new();
    for span in statements(tokens) {
        let range = span.from..span.to.min(doc.len());
        if range.is_empty() {
            continue;
        }
        let terminated = tokens[..span.end_token].iter().rev().any(|t| t.kind == Tok::Semi);
        let statement_tokens = &tokens[span.first_token..span.end_token];
        if let Some(d) = check_statement(doc, range, statement_tokens, terminated, dialect) {
            out.push(d);
        }
    }
    out
}

fn check_statement(
    doc: &str,
    range: Range<usize>,
    tokens: &[Token],
    terminated: bool,
    dialect: Dialect,
) -> Option<Diagnostic> {
    let sql = &doc[range.clone()];
    // Rule 0: a statement with a bracket still open is either half-typed or
    // already covered by the bracket check next door, and the parser's opinion
    // of it is noise either way.
    if !brackets_balanced(tokens) {
        return None;
    }

    // Rule 1: a statement either grammar accepts is not reported.
    let error = parse_error(sql, dialect)?;
    parse_error(sql, Dialect::Standard)?;

    // Rule 2: the error has to point at something that is actually there.
    let offset = range.start + error_offset(sql, &error)?;
    let significant: Vec<&Token> =
        tokens.iter().filter(|t| !matches!(t.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment)).collect();
    let at = significant.iter().position(|t| t.from <= offset && offset < t.to)?;

    // Rule 3: the tail of an unterminated statement is still being written.
    let last_word = significant[at + 1..].iter().all(|t| t.kind == Tok::Semi);
    if last_word && !terminated {
        return None;
    }

    let token = blame(doc, &significant, at);
    let mut diagnostic = Diagnostic::new(token.from, token.to, message(&doc[token.from..token.to], &error));
    // Rule 4, for the editor to apply: quiet while the caret is in the word.
    diagnostic.quiet_while_caret_in = Some(token.from..token.to);
    Some(diagnostic)
}

fn brackets_balanced(tokens: &[Token]) -> bool {
    let mut depth = 0i32;
    for t in tokens {
        match t.kind {
            Tok::ParenL | Tok::BracketL | Tok::BraceL => depth += 1,
            Tok::ParenR | Tok::BracketR | Tok::BraceR => depth -= 1,
            _ => continue,
        }
        if depth < 0 {
            return false;
        }
    }
    depth == 0
}

/// The token to underline for an error the parser reported at `at`.
///
/// A grammar stops at the first token it cannot fit, which is often one past
/// the one a person would point at: in `SELECT * FROM customers WHER id = 1`
/// the parser happily reads `WHER` as a table alias and only objects when it
/// reaches `id`. So if the word before the error is a near-miss of a keyword,
/// that is the mistake, and underlining it is what makes the message useful.
fn blame<'a>(doc: &str, significant: &[&'a Token], at: usize) -> &'a Token {
    let token = significant[at];
    if nearest_keyword(&doc[token.from..token.to]).is_some() {
        return token;
    }
    if at > 0 {
        let previous = significant[at - 1];
        if matches!(previous.kind, Tok::Identifier | Tok::Keyword)
            && nearest_keyword(&doc[previous.from..previous.to]).is_some()
        {
            return previous;
        }
    }
    token
}

/// The parse error for one statement, or `None` when the grammar accepts it.
fn parse_error(sql: &str, dialect: Dialect) -> Option<String> {
    use sqlparser::dialect::{Dialect as SqlDialect, GenericDialect, MySqlDialect, PostgreSqlDialect, SQLiteDialect};
    let grammar: Box<dyn SqlDialect> = match dialect {
        Dialect::MySql => Box::new(MySqlDialect {}),
        Dialect::PostgreSql => Box::new(PostgreSqlDialect {}),
        Dialect::Sqlite => Box::new(SQLiteDialect {}),
        Dialect::Standard => Box::new(GenericDialect {}),
    };
    sqlparser::parser::Parser::parse_sql(grammar.as_ref(), sql).err().map(|err| err.to_string())
}

/// Byte offset inside the statement that an error message points at.
///
/// `sqlparser` reports the position in its message rather than in the error
/// type — `… at Line: 1, Column: 17` — and leaves it off entirely when it ran
/// out of input, which is rule 2's cue to say nothing.
fn error_offset(sql: &str, message: &str) -> Option<usize> {
    let (line, column) = error_location(message)?;
    offset_at(sql, line, column)
}

fn error_location(message: &str) -> Option<(usize, usize)> {
    let tail = message.rsplit_once(" at Line: ")?.1;
    let (line, column) = tail.split_once(", Column: ")?;
    Some((line.trim().parse().ok()?, column.trim().parse().ok()?))
}

/// Byte offset of a 1-based line and column, both counted in characters.
fn offset_at(sql: &str, line: usize, column: usize) -> Option<usize> {
    if line == 0 || column == 0 {
        return None;
    }
    let mut offset = 0;
    for _ in 1..line {
        offset += sql[offset..].find('\n')? + 1;
    }
    let rest = &sql[offset..];
    let end = rest.find('\n').unwrap_or(rest.len());
    let mut chars = rest[..end].char_indices();
    match column - 1 {
        0 => Some(offset),
        skip => chars.nth(skip).map(|(i, _)| offset + i),
    }
}

/// What to say about the offending word.
///
/// A word that is nearly a keyword is nearly always a typo, and saying so is
/// far more use than repeating the grammar's expectations, so that case gets
/// its own wording. Everything else falls back to what the parser said, minus
/// the position, which the underline already shows.
fn message(word: &str, error: &str) -> String {
    if let Some(keyword) = nearest_keyword(word) {
        return format!("Unknown keyword `{word}` — did you mean `{keyword}`?");
    }
    let detail = error
        .strip_prefix("sql parser error: ")
        .unwrap_or(error)
        .rsplit_once(" at Line: ")
        .map(|(head, _)| head)
        .unwrap_or(error);
    format!("SQL syntax error: {detail}")
}

/// The keyword a word was probably meant to be, if any.
///
/// Only near-misses of the same rough length count, and a word that *is* a
/// keyword is never a near-miss of another one, so `SELECT` is never offered as
/// a correction for `SET`.
fn nearest_keyword(word: &str) -> Option<&'static str> {
    // Short words are one edit from half the keyword list — `id` is one from
    // `IN`, `ord` one from `OR` — so they are left alone. A word has to be long
    // enough that being one letter off is evidence of anything.
    if word.len() < 4 || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return None;
    }
    let upper = word.to_uppercase();
    let keywords = super::complete::reserved_words();
    if keywords.contains(&upper.as_str()) {
        return None;
    }
    // One edit for a short word, two once there is enough of it to be sure.
    let budget = if upper.len() >= 6 { 2 } else { 1 };
    keywords
        .iter()
        .filter(|k| k.len().abs_diff(upper.len()) <= budget)
        .map(|k| (edit_distance(&upper, k), *k))
        .filter(|(distance, _)| *distance <= budget)
        // On a tie, prefer the keyword the typed word is the start of: a word
        // cut short is the commonest way to get one wrong, so `WHER` is
        // `WHERE` rather than the equally-close `WHEN`.
        .min_by_key(|(distance, keyword)| (*distance, !keyword.starts_with(&upper), *keyword))
        .map(|(_, keyword)| keyword)
}

/// Edit distance counting a swap of two neighbours as one mistake, because on a
/// keyboard it is one — `FORM` is a single slip for `FROM`, not two.
fn edit_distance(a: &str, b: &str) -> usize {
    let a: Vec<char> = a.chars().collect();
    let b: Vec<char> = b.chars().collect();
    let mut d = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for (i, row) in d.iter_mut().enumerate() {
        row[0] = i;
    }
    for (j, cell) in d[0].iter_mut().enumerate() {
        *cell = j;
    }
    for i in 1..=a.len() {
        for j in 1..=b.len() {
            let cost = usize::from(a[i - 1] != b[j - 1]);
            d[i][j] = (d[i - 1][j] + 1).min(d[i][j - 1] + 1).min(d[i - 1][j - 1] + cost);
            if i > 1 && j > 1 && a[i - 1] == b[j - 2] && a[i - 2] == b[j - 1] {
                d[i][j] = d[i][j].min(d[i - 2][j - 2] + 1);
            }
        }
    }
    d[a.len()][b.len()]
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::sql::tokenize;

    fn check(doc: &str, dialect: Dialect) -> Vec<Diagnostic> {
        parse_diagnostics(doc, dialect, &tokenize(doc, dialect))
    }

    fn messages(doc: &str) -> Vec<String> {
        check(doc, Dialect::PostgreSql).into_iter().map(|d| d.message).collect()
    }

    #[test]
    fn valid_sql_is_never_flagged() {
        for sql in [
            "SELECT 1",
            "SELECT * FROM customers WHERE id = 1;",
            "select c.id, c.name from customers c join orders o on o.customer_id = c.id where o.total > 100 order by c.name;",
            "WITH recent AS (SELECT * FROM orders WHERE ordered_at > '2020-01-01') SELECT count(*) FROM recent;",
            "INSERT INTO t (a, b) VALUES (1, 2);",
            "UPDATE t SET a = 1 WHERE b = 2;",
            "DELETE FROM t WHERE id IN (SELECT id FROM u);",
            "-- just a comment",
            "",
        ] {
            assert!(messages(sql).is_empty(), "{sql:?} should be accepted");
        }
    }

    /// The check earns its keep only if it stays silent on real SQL, so this
    /// is the important test: a spread of things people actually type, each in
    /// the dialect it belongs to, none of which may be underlined.
    #[test]
    fn real_sql_in_its_own_dialect_is_never_flagged() {
        let cases: &[(Dialect, &str)] = &[
            (Dialect::MySql, "SELECT `id`, `name` FROM `customers` WHERE `name` LIKE '%a%' LIMIT 10, 20;"),
            (Dialect::MySql, "SELECT id FROM t ORDER BY id DESC LIMIT 5;"),
            (Dialect::MySql, "INSERT INTO t (a) VALUES (1) ON DUPLICATE KEY UPDATE a = a + 1;"),
            (Dialect::PostgreSql, "SELECT id::text, data->>'name' FROM events WHERE data ? 'name';"),
            (Dialect::PostgreSql, "SELECT row_number() OVER (PARTITION BY a ORDER BY b) FROM t;"),
            (Dialect::PostgreSql, "INSERT INTO t (a) VALUES (1) ON CONFLICT (a) DO NOTHING RETURNING id;"),
            (Dialect::PostgreSql, "SELECT * FROM generate_series(1, 10) AS g(n);"),
            (Dialect::Sqlite, "SELECT * FROM t WHERE json_extract(payload, '$.id') = 1;"),
            (Dialect::Sqlite, "CREATE TABLE IF NOT EXISTS t (id INTEGER PRIMARY KEY, name TEXT NOT NULL);"),
            (Dialect::Standard, "SELECT CASE WHEN a > 1 THEN 'big' ELSE 'small' END FROM t;"),
            (Dialect::Standard, "SELECT a, count(*) FROM t GROUP BY a HAVING count(*) > 1 ORDER BY 2 DESC;"),
            (Dialect::Standard, "SELECT * FROM a LEFT JOIN b ON a.id = b.a_id WHERE b.id IS NULL;"),
            (Dialect::Standard, "/* a comment */ SELECT 1; -- and another\nSELECT 2;"),
        ];
        for (dialect, sql) in cases {
            let found = check(sql, *dialect);
            assert!(found.is_empty(), "{sql:?} was flagged: {found:?}");
        }
    }

    #[test]
    fn a_misspelled_keyword_is_named_and_corrected() {
        let found = check("SELECT * FROM customers WHER id = 1;", Dialect::PostgreSql);
        assert_eq!(found.len(), 1);
        assert_eq!(&"SELECT * FROM customers WHER id = 1;"[found[0].from..found[0].to], "WHER");
        assert_eq!(found[0].message, "Unknown keyword `WHER` — did you mean `WHERE`?");
        // And it is silent while the caret is still inside the word.
        assert_eq!(found[0].quiet_while_caret_in, Some(found[0].from..found[0].to));
    }

    #[test]
    fn a_half_typed_statement_is_not_an_error_yet() {
        // Every prefix of a statement someone is typing has to stay quiet,
        // because each one is a moment they would have seen an underline.
        let full = "SELECT * FROM customers WHERE id = 1";
        for end in 1..=full.len() {
            let typed = &full[..end];
            assert!(messages(typed).is_empty(), "{typed:?} should still be quiet");
        }
    }

    #[test]
    fn a_finished_statement_before_the_one_being_typed_still_reports() {
        // The first statement is terminated, so its mistake is fair game even
        // though the second is mid-flight.
        let found = messages("SELECT * FROM t WHER a = 1; SELECT * FROM");
        assert_eq!(found, vec!["Unknown keyword `WHER` — did you mean `WHERE`?"]);
    }

    #[test]
    fn syntax_only_one_dialect_knows_is_left_alone() {
        // Backtick quoting is MySQL's; the generic grammar rejects it. Neither
        // spelling should be underlined, because in both cases one of the two
        // parsers was happy.
        assert!(check("SELECT `id` FROM `orders`;", Dialect::MySql).is_empty());
        assert!(check("SELECT \"id\" FROM \"orders\";", Dialect::PostgreSql).is_empty());
    }

    #[test]
    fn a_word_that_is_not_nearly_a_keyword_falls_back_to_the_parser_wording() {
        let found = messages("SELECT * FROM t zzzzzzzz qqqqqqqq;");
        assert_eq!(found.len(), 1);
        assert!(found[0].starts_with("SQL syntax error: "), "{found:?}");
        // The position belongs on the underline, not in the sentence.
        assert!(!found[0].contains("at Line:"), "{found:?}");
    }

    #[test]
    fn the_nearest_keyword_is_only_offered_for_a_real_near_miss() {
        assert_eq!(nearest_keyword("wher"), Some("WHERE"));
        assert_eq!(nearest_keyword("slect"), Some("SELECT"));
        assert_eq!(nearest_keyword("form"), Some("FROM"));
        // Already a keyword.
        assert_eq!(nearest_keyword("select"), None);
        // An ordinary identifier is not a typo of anything.
        assert_eq!(nearest_keyword("customers"), None);
        // Short identifiers are one edit from all sorts of keywords, and
        // suggesting `IN` for a column called `id` would be worse than silence.
        assert_eq!(nearest_keyword("id"), None);
        assert_eq!(nearest_keyword("ord"), None);
        assert_eq!(nearest_keyword("x"), None);
    }

    #[test]
    fn an_open_bracket_is_left_to_the_bracket_check() {
        // The bracket checker already reports this, and while the person is
        // still inside the parentheses the parser has nothing useful to add.
        assert!(messages("SELECT (1 FROM t").is_empty());
        assert!(messages("SELECT count(* FROM t;").is_empty());
    }

    #[test]
    fn a_position_in_the_message_maps_to_the_right_byte() {
        assert_eq!(offset_at("SELECT 1", 1, 1), Some(0));
        assert_eq!(offset_at("SELECT 1", 1, 8), Some(7));
        assert_eq!(offset_at("SELECT\n  1", 2, 3), Some(9));
        // Past the end of the input is exactly the case rule 2 drops.
        assert_eq!(offset_at("SELECT 1", 1, 99), None);
        assert_eq!(offset_at("SELECT 1", 3, 1), None);
        assert_eq!(error_location("nothing here"), None);
        assert_eq!(error_location("Expected: x, found: y at Line: 2, Column: 7"), Some((2, 7)));
    }

    #[test]
    fn multibyte_text_does_not_shift_the_underline() {
        let doc = "SELECT 'café', 'naïve' FROM t WHER a = 1;";
        let found = check(doc, Dialect::PostgreSql);
        assert_eq!(found.len(), 1);
        assert_eq!(&doc[found[0].from..found[0].to], "WHER");
    }
}
