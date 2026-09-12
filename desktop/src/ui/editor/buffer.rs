//! Text buffer with line index, selection and grouped undo history for the
//! SQL editor.

use std::ops::Range;
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Selection {
    pub anchor: usize,
    pub head: usize,
}

impl Selection {
    pub fn cursor(pos: usize) -> Self {
        Selection { anchor: pos, head: pos }
    }
    pub fn range(&self) -> Range<usize> {
        self.anchor.min(self.head)..self.anchor.max(self.head)
    }
    pub fn is_empty(&self) -> bool {
        self.anchor == self.head
    }
}

#[derive(Clone)]
struct HistoryEntry {
    text: String,
    selection: Selection,
    at: Instant,
    kind: EditKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EditKind {
    Type,
    Delete,
    Other,
}

pub struct Buffer {
    text: String,
    line_starts: Vec<usize>,
    pub selection: Selection,
    undo: Vec<HistoryEntry>,
    redo: Vec<(String, Selection)>,
    /// Preferred x (in chars) for vertical movement.
    pub goal_column: Option<usize>,
}

impl Buffer {
    pub fn new(text: &str) -> Self {
        let mut b = Buffer {
            text: text.to_string(),
            line_starts: Vec::new(),
            selection: Selection::cursor(0),
            undo: Vec::new(),
            redo: Vec::new(),
            goal_column: None,
        };
        b.reindex();
        b
    }

    fn reindex(&mut self) {
        self.line_starts.clear();
        self.line_starts.push(0);
        for (i, b) in self.text.bytes().enumerate() {
            if b == b'\n' {
                self.line_starts.push(i + 1);
            }
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn line_count(&self) -> usize {
        self.line_starts.len()
    }

    pub fn line_start(&self, line: usize) -> usize {
        self.line_starts[line.min(self.line_starts.len() - 1)]
    }

    pub fn line_end(&self, line: usize) -> usize {
        if line + 1 < self.line_starts.len() {
            self.line_starts[line + 1] - 1
        } else {
            self.text.len()
        }
    }

    pub fn line_text(&self, line: usize) -> &str {
        &self.text[self.line_start(line)..self.line_end(line)]
    }

    pub fn line_of(&self, offset: usize) -> usize {
        match self.line_starts.binary_search(&offset) {
            Ok(i) => i,
            Err(i) => i - 1,
        }
    }

    pub fn clamp(&self, offset: usize) -> usize {
        let mut o = offset.min(self.text.len());
        while !self.text.is_char_boundary(o) {
            o -= 1;
        }
        o
    }

    /// Replace the whole document (external change, e.g. navigator sets SQL).
    pub fn set_text(&mut self, text: &str) {
        self.record(EditKind::Other, true);
        self.text = text.to_string();
        self.reindex();
        let end = self.selection.head.min(self.text.len());
        let end = self.clamp(end);
        self.selection = Selection::cursor(end);
    }

    fn record(&mut self, kind: EditKind, force_new: bool) {
        let now = Instant::now();
        let merge = !force_new
            && kind != EditKind::Other
            && self
                .undo
                .last()
                .is_some_and(|e| e.kind == kind && now.duration_since(e.at) < Duration::from_millis(500));
        if merge {
            if let Some(last) = self.undo.last_mut() {
                last.at = now;
            }
        } else {
            self.undo.push(HistoryEntry { text: self.text.clone(), selection: self.selection, at: now, kind });
            if self.undo.len() > 500 {
                self.undo.remove(0);
            }
        }
        self.redo.clear();
    }

    /// Replace `range` with `new_text`, putting the cursor after the insert.
    pub fn edit(&mut self, range: Range<usize>, new_text: &str, kind: EditKind) {
        self.record(kind, false);
        let range = self.clamp(range.start)..self.clamp(range.end);
        self.text.replace_range(range.clone(), new_text);
        self.reindex();
        let pos = range.start + new_text.len();
        self.selection = Selection::cursor(pos);
        self.goal_column = None;
    }

    /// Apply several non-overlapping edits (sorted by start), then set selection.
    pub fn edit_many(&mut self, mut edits: Vec<(Range<usize>, String)>, selection: Selection, kind: EditKind) {
        self.record(kind, true);
        edits.sort_by_key(|(r, _)| std::cmp::Reverse(r.start));
        for (range, text) in edits {
            self.text.replace_range(range, &text);
        }
        self.reindex();
        self.selection = Selection { anchor: self.clamp(selection.anchor), head: self.clamp(selection.head) };
        self.goal_column = None;
    }

    pub fn undo(&mut self) -> bool {
        if let Some(entry) = self.undo.pop() {
            self.redo.push((std::mem::replace(&mut self.text, entry.text), self.selection));
            self.selection = entry.selection;
            self.reindex();
            true
        } else {
            false
        }
    }

    pub fn redo(&mut self) -> bool {
        if let Some((text, sel)) = self.redo.pop() {
            self.undo.push(HistoryEntry { text: std::mem::replace(&mut self.text, text), selection: self.selection, at: Instant::now(), kind: EditKind::Other });
            self.selection = sel;
            self.reindex();
            true
        } else {
            false
        }
    }

    pub fn prev_char(&self, offset: usize) -> usize {
        self.text[..offset].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    pub fn next_char(&self, offset: usize) -> usize {
        self.text[offset..].chars().next().map(|c| offset + c.len_utf8()).unwrap_or(self.text.len())
    }

    pub fn char_at(&self, offset: usize) -> Option<char> {
        self.text[offset..].chars().next()
    }

    pub fn char_before(&self, offset: usize) -> Option<char> {
        self.text[..offset].chars().next_back()
    }

    fn class(c: char) -> u8 {
        if c.is_alphanumeric() || c == '_' {
            2
        } else if c.is_whitespace() {
            0
        } else {
            1
        }
    }

    /// CodeMirror group movement: skip whitespace, then a run of one class.
    pub fn word_left(&self, offset: usize) -> usize {
        let line_start = self.line_start(self.line_of(offset));
        if offset == line_start {
            return self.prev_char(offset);
        }
        let mut pos = offset;
        while pos > line_start && self.char_before(pos).is_some_and(|c| c.is_whitespace()) {
            pos = self.prev_char(pos);
        }
        if pos == line_start {
            return pos;
        }
        let cls = Self::class(self.char_before(pos).unwrap());
        while pos > line_start && self.char_before(pos).is_some_and(|c| Self::class(c) == cls) {
            pos = self.prev_char(pos);
        }
        pos
    }

    pub fn word_right(&self, offset: usize) -> usize {
        let line_end = self.line_end(self.line_of(offset));
        if offset == line_end {
            return self.next_char(offset);
        }
        let mut pos = offset;
        while pos < line_end && self.char_at(pos).is_some_and(|c| c.is_whitespace()) {
            pos = self.next_char(pos);
        }
        if pos == line_end {
            return pos;
        }
        let cls = Self::class(self.char_at(pos).unwrap());
        while pos < line_end && self.char_at(pos).is_some_and(|c| Self::class(c) == cls) {
            pos = self.next_char(pos);
        }
        pos
    }

    pub fn word_range_at(&self, offset: usize) -> Range<usize> {
        let line = self.line_of(offset);
        let (ls, le) = (self.line_start(line), self.line_end(line));
        let at = self.char_at(offset).filter(|_| offset < le);
        let before = self.char_before(offset).filter(|_| offset > ls);
        let cls = match (at, before) {
            (Some(c), _) if Self::class(c) == 2 => 2,
            (_, Some(c)) if Self::class(c) == 2 => 2,
            (Some(c), _) => Self::class(c),
            (_, Some(c)) => Self::class(c),
            _ => return offset..offset,
        };
        let mut start = offset;
        while start > ls && self.char_before(start).is_some_and(|c| Self::class(c) == cls) {
            start = self.prev_char(start);
        }
        let mut end = offset;
        while end < le && self.char_at(end).is_some_and(|c| Self::class(c) == cls) {
            end = self.next_char(end);
        }
        start..end
    }

    pub fn column_chars(&self, offset: usize) -> usize {
        let ls = self.line_start(self.line_of(offset));
        self.text[ls..offset].chars().count()
    }

    pub fn offset_at_column(&self, line: usize, col: usize) -> usize {
        let ls = self.line_start(line);
        let text = self.line_text(line);
        text.char_indices().nth(col).map(|(i, _)| ls + i).unwrap_or(ls + text.len())
    }

    pub fn indentation_of_line(&self, line: usize) -> usize {
        self.line_text(line).chars().take_while(|c| *c == ' ' || *c == '\t').map(|c| if c == '\t' { 4 } else { 1 }).sum()
    }
}
