use ropey::Rope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorLine {
    pub number: i32,
    pub text: String,
    pub class_name: String,
    pub active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletionCandidate {
    pub label: String,
    pub detail: String,
    pub replacement: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EditorSnapshot {
    text: String,
    cursor: usize,
    selection_start: usize,
    selection_end: usize,
}

#[derive(Debug, Clone)]
pub struct SqlEditorBuffer {
    text: Rope,
    cursor: usize,
    selection_start: usize,
    selection_end: usize,
    scroll_line: usize,
    dirty: bool,
    undo: Vec<EditorSnapshot>,
    redo: Vec<EditorSnapshot>,
}

impl Default for SqlEditorBuffer {
    fn default() -> Self {
        Self::new("")
    }
}

impl SqlEditorBuffer {
    pub fn new(text: &str) -> Self {
        Self {
            text: Rope::from_str(text),
            cursor: 0,
            selection_start: 0,
            selection_end: 0,
            scroll_line: 0,
            dirty: false,
            undo: Vec::new(),
            redo: Vec::new(),
        }
    }

    pub fn text(&self) -> String {
        self.text.to_string()
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn dirty(&self) -> bool {
        self.dirty
    }

    pub fn set_text(&mut self, text: &str) {
        self.push_undo();
        self.text = Rope::from_str(text);
        self.cursor = self.text.len_chars();
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.dirty = true;
        self.redo.clear();
    }

    pub fn insert_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        self.push_undo();
        self.delete_selection_without_undo();
        self.text.insert(self.cursor, text);
        self.cursor += text.chars().count();
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.dirty = true;
        self.redo.clear();
    }

    pub fn backspace(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection_without_undo();
            self.dirty = true;
            self.redo.clear();
            return;
        }
        if self.cursor == 0 {
            return;
        }
        self.push_undo();
        let from = self.cursor - 1;
        self.text.remove(from..self.cursor);
        self.cursor = from;
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.dirty = true;
        self.redo.clear();
    }

    pub fn delete_forward(&mut self) {
        if self.has_selection() {
            self.push_undo();
            self.delete_selection_without_undo();
            self.dirty = true;
            self.redo.clear();
            return;
        }
        if self.cursor >= self.text.len_chars() {
            return;
        }
        self.push_undo();
        self.text.remove(self.cursor..self.cursor + 1);
        self.dirty = true;
        self.redo.clear();
    }

    pub fn move_left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.ensure_cursor_visible();
    }

    pub fn move_right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.len_chars());
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.ensure_cursor_visible();
    }

    pub fn move_up(&mut self) {
        let (line, col) = self.cursor_line_col();
        if line == 0 {
            return;
        }
        self.cursor = self.line_col_to_char(line - 1, col);
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.ensure_cursor_visible();
    }

    pub fn move_down(&mut self) {
        let (line, col) = self.cursor_line_col();
        if line + 1 >= self.text.len_lines() {
            return;
        }
        self.cursor = self.line_col_to_char(line + 1, col);
        self.selection_start = self.cursor;
        self.selection_end = self.cursor;
        self.ensure_cursor_visible();
    }

    pub fn select_all(&mut self) {
        self.cursor = self.text.len_chars();
        self.selection_start = 0;
        self.selection_end = self.cursor;
    }

    pub fn undo(&mut self) {
        let Some(snapshot) = self.undo.pop() else {
            return;
        };
        self.redo.push(self.snapshot());
        self.restore(snapshot);
    }

    pub fn redo(&mut self) {
        let Some(snapshot) = self.redo.pop() else {
            return;
        };
        self.undo.push(self.snapshot());
        self.restore(snapshot);
    }

    pub fn visible_lines(&self, count: usize) -> Vec<EditorLine> {
        let start = self
            .scroll_line
            .min(self.text.len_lines().saturating_sub(1));
        let end = (start + count.max(1)).min(self.text.len_lines());
        let active_line = self.cursor_line_col().0;

        (start..end)
            .map(|idx| {
                let text = self
                    .text
                    .line(idx)
                    .to_string()
                    .trim_end_matches(['\r', '\n'])
                    .to_string();
                EditorLine {
                    number: idx as i32 + 1,
                    class_name: dominant_class(&text).to_string(),
                    active: idx == active_line,
                    text,
                }
            })
            .collect()
    }

    pub fn completions(&self, schema_words: &[String], dialect: &str) -> Vec<CompletionCandidate> {
        let prefix = self.current_word_prefix().to_ascii_lowercase();
        if prefix.len() < 2 {
            return Vec::new();
        }

        let mut out = Vec::new();
        for keyword in sql_keywords(dialect) {
            if keyword.to_ascii_lowercase().starts_with(&prefix) {
                out.push(CompletionCandidate {
                    label: keyword.to_string(),
                    detail: "keyword".to_string(),
                    replacement: keyword.to_string(),
                });
            }
        }
        for word in schema_words {
            if word.to_ascii_lowercase().starts_with(&prefix) {
                out.push(CompletionCandidate {
                    label: word.clone(),
                    detail: "schema".to_string(),
                    replacement: word.clone(),
                });
            }
        }
        out.truncate(50);
        out
    }

    fn current_word_prefix(&self) -> String {
        let mut chars = Vec::new();
        let mut idx = self.cursor;
        while idx > 0 {
            idx -= 1;
            let ch = self.text.char(idx);
            if ch.is_alphanumeric() || ch == '_' || ch == '.' {
                chars.push(ch);
            } else {
                break;
            }
        }
        chars.into_iter().rev().collect()
    }

    fn has_selection(&self) -> bool {
        self.selection_start != self.selection_end
    }

    fn delete_selection_without_undo(&mut self) {
        if !self.has_selection() {
            return;
        }
        let from = self.selection_start.min(self.selection_end);
        let to = self.selection_start.max(self.selection_end);
        self.text.remove(from..to);
        self.cursor = from;
        self.selection_start = from;
        self.selection_end = from;
    }

    fn cursor_line_col(&self) -> (usize, usize) {
        let line = self
            .text
            .char_to_line(self.cursor.min(self.text.len_chars()));
        let line_start = self.text.line_to_char(line);
        (line, self.cursor.saturating_sub(line_start))
    }

    fn line_col_to_char(&self, line: usize, col: usize) -> usize {
        let line = line.min(self.text.len_lines().saturating_sub(1));
        let line_start = self.text.line_to_char(line);
        let line_len = self.text.line(line).len_chars();
        line_start + col.min(line_len.saturating_sub(1))
    }

    fn ensure_cursor_visible(&mut self) {
        let line = self.cursor_line_col().0;
        if line < self.scroll_line {
            self.scroll_line = line;
        } else if line >= self.scroll_line + 200 {
            self.scroll_line = line.saturating_sub(199);
        }
    }

    fn snapshot(&self) -> EditorSnapshot {
        EditorSnapshot {
            text: self.text(),
            cursor: self.cursor,
            selection_start: self.selection_start,
            selection_end: self.selection_end,
        }
    }

    fn push_undo(&mut self) {
        self.undo.push(self.snapshot());
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
    }

    fn restore(&mut self, snapshot: EditorSnapshot) {
        self.text = Rope::from_str(&snapshot.text);
        self.cursor = snapshot.cursor.min(self.text.len_chars());
        self.selection_start = snapshot.selection_start.min(self.text.len_chars());
        self.selection_end = snapshot.selection_end.min(self.text.len_chars());
        self.dirty = true;
        self.ensure_cursor_visible();
    }
}

fn dominant_class(line: &str) -> &'static str {
    let trimmed = line.trim_start();
    if trimmed.starts_with("--") || trimmed.starts_with("/*") {
        "comment"
    } else if trimmed.starts_with('\'') || trimmed.starts_with('"') {
        "string"
    } else if trimmed
        .split_whitespace()
        .next()
        .is_some_and(|word| sql_keywords("postgres").contains(&word.to_ascii_uppercase().as_str()))
    {
        "keyword"
    } else {
        "plain"
    }
}

pub fn sql_keywords(dialect: &str) -> &'static [&'static str] {
    match dialect {
        "mysql" => &[
            "SELECT", "FROM", "WHERE", "JOIN", "LEFT", "RIGHT", "INNER", "OUTER", "INSERT",
            "UPDATE", "DELETE", "CREATE", "ALTER", "DROP", "LIMIT", "ORDER", "GROUP", "HAVING",
            "DATABASE", "TABLE", "INDEX", "VIEW", "EXPLAIN", "DESCRIBE", "SHOW",
        ],
        "sqlite" => &[
            "SELECT", "FROM", "WHERE", "JOIN", "LEFT", "INSERT", "UPDATE", "DELETE", "CREATE",
            "ALTER", "DROP", "LIMIT", "ORDER", "GROUP", "HAVING", "PRAGMA", "TABLE", "INDEX",
            "VIEW", "WITHOUT", "ROWID",
        ],
        _ => &[
            "SELECT",
            "FROM",
            "WHERE",
            "JOIN",
            "LEFT",
            "RIGHT",
            "FULL",
            "INNER",
            "OUTER",
            "INSERT",
            "UPDATE",
            "DELETE",
            "CREATE",
            "ALTER",
            "DROP",
            "LIMIT",
            "OFFSET",
            "ORDER",
            "GROUP",
            "HAVING",
            "RETURNING",
            "WITH",
            "TABLE",
            "INDEX",
            "VIEW",
            "EXPLAIN",
            "ANALYZE",
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::SqlEditorBuffer;

    #[test]
    fn insert_delete_and_undo() {
        let mut editor = SqlEditorBuffer::new("");
        editor.insert_text("select");
        editor.insert_text(" *");
        assert_eq!(editor.text(), "select *");
        editor.backspace();
        assert_eq!(editor.text(), "select ");
        editor.undo();
        assert_eq!(editor.text(), "select *");
    }

    #[test]
    fn cursor_moves_across_lines() {
        let mut editor = SqlEditorBuffer::new("one\ntwo\nthree");
        editor.move_right();
        editor.move_right();
        editor.move_down();
        assert_eq!(editor.cursor(), 6);
    }

    #[test]
    fn keyword_completion_uses_prefix() {
        let mut editor = SqlEditorBuffer::new("");
        editor.insert_text("sel");
        let completions = editor.completions(&[], "postgres");
        assert!(completions.iter().any(|item| item.label == "SELECT"));
    }
}
