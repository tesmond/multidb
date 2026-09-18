//! SQL language support for the editor: tokenizing/highlighting, a light
//! statement tree, completion (CodeMirror-compatible ranking) and lint.

pub mod complete;
pub mod lint;
pub mod parse_check;
pub mod schema;
pub mod tokenizer;

pub use tokenizer::{tokenize, Dialect, Tok, Token};

/// A statement spans from its first significant token to its terminating `;`
/// (inclusive) — mirrors lang-sql's `Statement` node.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StatementSpan {
    pub from: usize,
    pub to: usize,
    /// Index range into the token vector.
    pub first_token: usize,
    pub end_token: usize,
}

pub fn statements(tokens: &[Token]) -> Vec<StatementSpan> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, t) in tokens.iter().enumerate() {
        if matches!(t.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment) {
            continue;
        }
        if start.is_none() {
            start = Some(i);
        }
        if t.kind == Tok::Semi {
            let s = start.take().unwrap();
            out.push(StatementSpan { from: tokens[s].from, to: t.to, first_token: s, end_token: i + 1 });
        }
    }
    if let Some(s) = start {
        let last = tokens
            .iter()
            .enumerate()
            .rev()
            .find(|(_, t)| !matches!(t.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment))
            .map(|(i, _)| i)
            .unwrap_or(s);
        out.push(StatementSpan { from: tokens[s].from, to: tokens[last].to, first_token: s, end_token: last + 1 });
    }
    out
}

/// `syntaxTree(state).resolveInner(pos, -1)` restricted to tokens: the token
/// with `from < pos <= to`.
pub fn token_before(tokens: &[Token], pos: usize) -> Option<(usize, &Token)> {
    tokens.iter().enumerate().find(|(_, t)| t.from < pos && pos <= t.to)
}

/// Previous significant (non-whitespace, non-comment) token before index `i`.
pub fn prev_significant(tokens: &[Token], i: usize) -> Option<(usize, &Token)> {
    tokens[..i]
        .iter()
        .enumerate()
        .rev()
        .find(|(_, t)| !matches!(t.kind, Tok::Whitespace | Tok::LineComment | Tok::BlockComment))
}

/// Statement containing `pos` in the resolveInner(-1) sense.
pub fn statement_at(spans: &[StatementSpan], pos: usize) -> Option<StatementSpan> {
    spans.iter().copied().find(|s| s.from < pos && pos <= s.to)
}
