//! The search panel and goto-line dialog that `searchKeymap` gave the old
//! editor (`@codemirror/search`), including its panel markup and styling:
//!
//! ```text
//! .cm-panels           { background: #333338; color: white; z-index: 300 }
//! .cm-panel.cm-search  { padding: 2px 6px 4px; position: relative }
//!   input, button, label { margin: .2em .6em .2em 0 }
//!   input[type=checkbox] { margin-right: .2em }
//!   label                { font-size: 80%; white-space: pre }
//!   [name=close]         { position: absolute; top: 0; right: 4px }
//! .cm-button    { font-size: 70%; padding: .2em 1em; border: 1px solid #888;
//!                 border-radius: 1px; background: linear-gradient(#393939,#111) }
//! .cm-textfield { font-size: 70%; padding: .2em .5em; border: 1px solid #555 }
//! .cm-searchMatch          { background: #00ffff8a }
//! .cm-searchMatch-selected { background: #ff00ff8a }
//! ```
//!
//! One Dark also styles `.cm-panels` (#21252b/ivory) and `.cm-searchMatch`
//! (#72a1ff59), but those rules lose to the base theme's `&dark` variants
//! above, which carry an extra class (`.cm-editor.cm-dark .cm-panels` beats
//! `.theme .cm-panels`). The one rule of One Dark's that survives is the
//! outline, which the base theme never sets.

use crate::ui::theme::{self, rgba8, Rgba};
use crate::ui::widgets::text_input::{InputLook, TextInput};
use gpui::{Entity, FontWeight};
use regex::{Regex, RegexBuilder};
use std::ops::Range;

pub const MATCH: Rgba = rgba8(0, 255, 255, 0.54);
pub const MATCH_SELECTED: Rgba = rgba8(255, 0, 255, 0.54);
/// one-dark adds `outline: 1px solid #457dff` on top of the base theme's
/// background (the base theme sets no outline, so this one survives).
pub const MATCH_OUTLINE: Rgba = theme::hex(0x457dff);
/// `.cm-panels` background and the panel text colour.
pub const PANEL_BG: u32 = 0x333338;
pub const BUTTON_BORDER: u32 = 0x888888;
pub const BUTTON_TOP: u32 = 0x393939;
pub const BUTTON_BOTTOM: u32 = 0x111111;
pub const FIELD_BORDER: u32 = 0x555555;

/// `SearchQuery` from @codemirror/search.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Query {
    pub search: String,
    pub replace: String,
    pub case_sensitive: bool,
    pub regexp: bool,
    pub whole_word: bool,
}

impl Query {
    pub fn is_valid(&self) -> bool {
        !self.search.is_empty() && (!self.regexp || Regex::new(&self.search).is_ok())
    }

    fn regex(&self) -> Option<Regex> {
        let pattern = if self.regexp { self.search.clone() } else { regex::escape(&self.search) };
        let pattern = if self.whole_word { format!(r"\b(?:{pattern})\b") } else { pattern };
        RegexBuilder::new(&pattern).case_insensitive(!self.case_sensitive).build().ok()
    }

    /// Every match in the document, in order.
    pub fn matches(&self, doc: &str) -> Vec<Range<usize>> {
        if !self.is_valid() {
            return Vec::new();
        }
        let Some(re) = self.regex() else { return Vec::new() };
        let mut out = Vec::new();
        let mut from = 0;
        while from <= doc.len() {
            let Some(m) = re.find_at(doc, from) else { break };
            if m.end() == m.start() {
                // Zero-length match: step one character to avoid looping.
                from = next_boundary(doc, m.start());
                if from == m.start() {
                    break;
                }
                continue;
            }
            out.push(m.start()..m.end());
            from = m.end();
        }
        out
    }

    /// First match at or after `pos`, wrapping to the start of the document.
    pub fn next(&self, doc: &str, pos: usize) -> Option<Range<usize>> {
        let all = self.matches(doc);
        all.iter().find(|m| m.start >= pos).cloned().or_else(|| all.first().cloned())
    }

    /// Last match before `pos`, wrapping to the end of the document.
    pub fn previous(&self, doc: &str, pos: usize) -> Option<Range<usize>> {
        let all = self.matches(doc);
        all.iter().rev().find(|m| m.end <= pos).cloned().or_else(|| all.last().cloned())
    }

    /// The replacement for `range`, expanding `$1`/`$&` when in regexp mode.
    pub fn replacement(&self, doc: &str, range: &Range<usize>) -> String {
        if !self.regexp {
            return self.replace.clone();
        }
        let Some(re) = self.regex() else { return self.replace.clone() };
        let Some(caps) = re.captures(&doc[range.clone()]) else { return self.replace.clone() };
        let mut out = String::new();
        caps.expand(&self.replace, &mut out);
        out
    }
}

fn next_boundary(doc: &str, pos: usize) -> usize {
    let mut i = pos + 1;
    while i < doc.len() && !doc.is_char_boundary(i) {
        i += 1;
    }
    i.min(doc.len())
}

pub struct SearchPanel {
    pub query: Query,
    pub search_input: Entity<TextInput>,
    pub replace_input: Entity<TextInput>,
    /// Matches for the current query, recomputed on every change.
    pub matches: Vec<Range<usize>>,
}

pub struct GotoDialog {
    pub input: Entity<TextInput>,
}

/// `.cm-textfield` at the editor's font size.
pub fn textfield_look(font_size: f32) -> InputLook {
    let fs = 0.7 * font_size;
    let lh = crate::ui::metrics::line_height_normal(fs);
    InputLook {
        font_size: fs,
        font_family: theme::UI_FONT.into(),
        font_weight: FontWeight::NORMAL,
        text: theme::WHITE,
        placeholder: crate::ui::metrics::PLACEHOLDER,
        bg: theme::hex(PANEL_BG),
        focus_bg: None,
        border: theme::hex(FIELD_BORDER),
        focus_border: theme::hex(FIELD_BORDER),
        border_width: 1.0,
        radius: 0.0,
        pad_x: (0.5 * fs, 0.5 * fs),
        height: lh + 0.4 * fs + 2.0,
        line_height: lh,
        focus_ring: None,
    }
}

/// Width of a bare `<input>` (WebKit sizes one for `size=20`:
/// `avg_char * 20 + max_char - avg_char`, plus padding and border).
pub fn default_input_width(font_size: f32) -> f32 {
    let fs = 0.7 * font_size;
    let (avg, max) = (0.5 * fs, 1.0 * fs);
    avg * 20.0 + max - avg + fs + 2.0
}

/// `gotoLine`'s `^([+-])?(\d+)?(:\d+)?(%)?$`, returning the new cursor offset.
pub fn goto_target(input: &str, doc_lines: usize, current_line: usize) -> Option<(usize, usize)> {
    let re = Regex::new(r"^([+-])?(\d+)?(:\d+)?(%)?$").ok()?;
    let caps = re.captures(input.trim())?;
    let sign = caps.get(1).map(|m| m.as_str().to_string());
    let ln: Option<i64> = caps.get(2).and_then(|m| m.as_str().parse().ok());
    let col: usize = caps.get(3).and_then(|m| m.as_str()[1..].parse().ok()).unwrap_or(0);
    let percent = caps.get(4).is_some();
    let mut line = ln.unwrap_or(current_line as i64);
    if ln.is_some() && percent {
        let mut pc = line as f64 / 100.0;
        if let Some(sign) = &sign {
            pc = pc * if sign == "-" { -1.0 } else { 1.0 } + current_line as f64 / doc_lines as f64;
        }
        line = (doc_lines as f64 * pc).round() as i64;
    } else if ln.is_some() {
        if let Some(sign) = &sign {
            line = line * if sign == "-" { -1 } else { 1 } + current_line as i64;
        }
    }
    let line = line.clamp(1, doc_lines as i64) as usize;
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(search: &str) -> Query {
        Query { search: search.into(), ..Default::default() }
    }

    #[test]
    fn literal_search_is_case_insensitive_by_default() {
        let doc = "select * from Users where user_id = 1";
        assert_eq!(q("user").matches(doc).len(), 2);
        let cs = Query { case_sensitive: true, ..q("user") };
        assert_eq!(cs.matches(doc).len(), 1);
    }

    #[test]
    fn whole_word_only_matches_words() {
        let doc = "user user_id";
        let ww = Query { whole_word: true, ..q("user") };
        assert_eq!(ww.matches(doc), vec![0..4]);
    }

    #[test]
    fn regexp_matches_and_expands_groups() {
        let doc = "a1 b2";
        let mut re = Query { regexp: true, ..q(r"([a-z])(\d)") };
        assert_eq!(re.matches(doc).len(), 2);
        re.replace = "$2$1".into();
        assert_eq!(re.replacement(doc, &(0..2)), "1a");
    }

    #[test]
    fn next_and_previous_wrap() {
        let doc = "x .. x .. x";
        let query = q("x");
        assert_eq!(query.next(doc, 1), Some(5..6));
        assert_eq!(query.next(doc, 11), Some(0..1));
        assert_eq!(query.previous(doc, 5), Some(0..1));
        assert_eq!(query.previous(doc, 0), Some(10..11));
    }

    #[test]
    fn goto_line_forms() {
        // absolute, relative, with column, percentage
        assert_eq!(goto_target("5", 10, 2), Some((5, 0)));
        assert_eq!(goto_target("+3", 10, 2), Some((5, 0)));
        assert_eq!(goto_target("-1", 10, 2), Some((1, 0)));
        assert_eq!(goto_target("4:2", 10, 2), Some((4, 2)));
        assert_eq!(goto_target("50%", 10, 2), Some((5, 0)));
        assert_eq!(goto_target("nope", 10, 2), None);
    }
}
