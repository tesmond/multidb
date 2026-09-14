//! SQL editor: a GPUI re-implementation of the CodeMirror 6 setup the web UI
//! used (One Dark theme, line numbers, active line, bracket matching,
//! auto-closing brackets, selection-match highlighting, history, schema-aware
//! autocompletion and lint with gutter markers).

pub mod buffer;
mod element;
pub mod popup;
pub mod search;
mod search_panel;

use crate::ui::sql::complete::{rank, Engine, RankedOption, SourceResult};
use crate::ui::widgets::scroll;
use crate::ui::widgets::text_input::{InputEvent, TextInput};
use crate::ui::sql::lint::{lint, Diagnostic};
use crate::ui::sql::schema::DbSchema;
use crate::ui::sql::{tokenize, Dialect, Tok, Token};
use buffer::{Buffer, EditKind, Selection};
use gpui::{
    actions, div, point, prelude::*, px, App, Bounds, ClipboardItem, Context, EntityInputHandler, EventEmitter, FocusHandle,
    Focusable, KeyBinding, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollWheelEvent,
    SharedString, Task, UTF16Selection, Window,
};
use std::ops::Range;
use std::sync::Arc;
use std::time::Duration;

pub use element::EditorLayout;

actions!(
    sql_editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveWordLeft,
        MoveWordRight,
        MoveLineStart,
        MoveLineEnd,
        MoveDocStart,
        MoveDocEnd,
        MovePageUp,
        MovePageDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectWordLeft,
        SelectWordRight,
        SelectLineStart,
        SelectLineEnd,
        SelectDocStart,
        SelectDocEnd,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        SelectLine,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        DeleteLineStart,
        DeleteLine,
        Newline,
        Tab,
        ShiftTab,
        IndentMore,
        IndentLess,
        ToggleComment,
        MoveLineUp,
        MoveLineDown,
        CopyLineUp,
        CopyLineDown,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        RunQuery,
        StartCompletion,
        Escape,
        CursorMatchingBracket,
        SelectNextOccurrence,
        OpenSearch,
        FindNext,
        FindPrevious,
        SelectSelectionMatches,
        GotoLine,
    ]
);

pub const CONTEXT: &str = "SqlEditor";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("ctrl-b", MoveLeft, c),
        KeyBinding::new("ctrl-f", MoveRight, c),
        KeyBinding::new("ctrl-p", MoveUp, c),
        KeyBinding::new("ctrl-n", MoveDown, c),
        KeyBinding::new("alt-left", MoveWordLeft, c),
        KeyBinding::new("alt-right", MoveWordRight, c),
        KeyBinding::new("cmd-left", MoveLineStart, c),
        KeyBinding::new("cmd-right", MoveLineEnd, c),
        KeyBinding::new("ctrl-a", MoveLineStart, c),
        KeyBinding::new("ctrl-e", MoveLineEnd, c),
        KeyBinding::new("home", MoveLineStart, c),
        KeyBinding::new("end", MoveLineEnd, c),
        KeyBinding::new("cmd-up", MoveDocStart, c),
        KeyBinding::new("cmd-down", MoveDocEnd, c),
        KeyBinding::new("cmd-home", MoveDocStart, c),
        KeyBinding::new("cmd-end", MoveDocEnd, c),
        KeyBinding::new("pageup", MovePageUp, c),
        KeyBinding::new("pagedown", MovePageDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-shift-left", SelectLineStart, c),
        KeyBinding::new("cmd-shift-right", SelectLineEnd, c),
        KeyBinding::new("shift-home", SelectLineStart, c),
        KeyBinding::new("shift-end", SelectLineEnd, c),
        KeyBinding::new("cmd-shift-up", SelectDocStart, c),
        KeyBinding::new("cmd-shift-down", SelectDocEnd, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("ctrl-l", SelectLine, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("ctrl-h", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-d", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-alt-h", DeleteWordLeft, c),
        KeyBinding::new("alt-delete", DeleteWordRight, c),
        KeyBinding::new("cmd-backspace", DeleteLineStart, c),
        KeyBinding::new("cmd-shift-k", DeleteLine, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("shift-tab", ShiftTab, c),
        KeyBinding::new("cmd-]", IndentMore, c),
        KeyBinding::new("cmd-[", IndentLess, c),
        KeyBinding::new("cmd-/", ToggleComment, c),
        KeyBinding::new("alt-up", MoveLineUp, c),
        KeyBinding::new("alt-down", MoveLineDown, c),
        KeyBinding::new("alt-shift-up", CopyLineUp, c),
        KeyBinding::new("alt-shift-down", CopyLineDown, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("cmd-y", Redo, c),
        KeyBinding::new("cmd-enter", RunQuery, c),
        KeyBinding::new("ctrl-space", StartCompletion, c),
        KeyBinding::new("alt-`", StartCompletion, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("cmd-shift-\\", CursorMatchingBracket, c),
        KeyBinding::new("cmd-d", SelectNextOccurrence, c),
        // searchKeymap
        KeyBinding::new("cmd-f", OpenSearch, c),
        KeyBinding::new("ctrl-f", OpenSearch, c),
        KeyBinding::new("cmd-g", FindNext, c),
        KeyBinding::new("f3", FindNext, c),
        KeyBinding::new("cmd-shift-g", FindPrevious, c),
        KeyBinding::new("shift-f3", FindPrevious, c),
        KeyBinding::new("cmd-shift-l", SelectSelectionMatches, c),
        KeyBinding::new("cmd-alt-g", GotoLine, c),
    ]);
}

#[derive(Debug, Clone)]
pub enum EditorEvent {
    Changed,
    Run,
}

impl EventEmitter<EditorEvent> for SqlEditor {}

pub struct CompletionState {
    pub results: Vec<SourceResult>,
    pub options: Vec<RankedOption>,
    pub selected: usize,
    /// Leftmost `from` of the active results (tooltip anchor).
    pub anchor: usize,
    pub explicit: bool,
    /// First rendered option (maxRenderedOptions window).
    pub scroll_top: f32,
}

pub const MAX_RENDERED_OPTIONS: usize = 50;

pub struct SqlEditor {
    focus_handle: FocusHandle,
    pub buffer: Buffer,
    pub font_scale: f32,
    pub mono_family: SharedString,
    dialect: Dialect,
    db: Option<Arc<DbSchema>>,
    engine: Arc<Engine>,
    pub placeholder: SharedString,
    pub tokens: Vec<Token>,
    pub diagnostics: Vec<Diagnostic>,
    pub completion: Option<CompletionState>,
    pub scroll: Point<Pixels>,
    pub cursor_visible: bool,
    blink_task: Option<Task<()>>,
    lint_task: Option<Task<()>>,
    completion_task: Option<Task<()>>,
    pub(crate) layout: Option<EditorLayout>,
    selecting: Option<SelectMode>,
    marked_range: Option<Range<usize>>,
    pub search: Option<search::SearchPanel>,
    pub goto: Option<search::GotoDialog>,
    _search_subs: Vec<gpui::Subscription>,
    pub hover_pos: Option<Point<Pixels>>,
    /// Diagnostics under the pointer plus whether the pointer is on the gutter
    /// marker, and whether the 300ms `hoverTime` has elapsed.
    hover_target: Option<(usize, bool)>,
    hover_ready: bool,
    hover_task: Option<Task<()>>,
    was_focused: bool,
    /// Scrollbar thumb being dragged, and the bar under the pointer.
    pub scroll_drag: Option<scroll::ScrollDrag>,
    pub hovered_bar: Option<scroll::Bar>,
}

#[derive(Clone, Copy)]
enum SelectMode {
    Char,
    Word(usize, usize),
    Line(usize),
}

impl Focusable for SqlEditor {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl SqlEditor {
    pub fn new(text: &str, font_scale: f32, mono_family: SharedString, cx: &mut Context<Self>) -> Self {
        let mut editor = SqlEditor {
            focus_handle: cx.focus_handle(),
            buffer: Buffer::new(text),
            font_scale,
            mono_family,
            dialect: Dialect::PostgreSql,
            db: None,
            engine: Arc::new(Engine::new(Dialect::PostgreSql, None)),
            placeholder: "Type SQL here… (Ctrl+Enter to run)".into(),
            tokens: Vec::new(),
            diagnostics: Vec::new(),
            completion: None,
            scroll: Point::default(),
            cursor_visible: true,
            blink_task: None,
            lint_task: None,
            completion_task: None,
            layout: None,
            selecting: None,
            marked_range: None,
            search: None,
            goto: None,
            _search_subs: Vec::new(),
            hover_pos: None,
            hover_target: None,
            hover_ready: false,
            hover_task: None,
            was_focused: false,
            scroll_drag: None,
            hovered_bar: None,
        };
        editor.retokenize();
        editor.schedule_lint(cx);
        editor
    }

    pub fn text(&self) -> &str {
        self.buffer.text()
    }

    /// External document replacement (keeps CodeMirror's "sync from store").
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if text == self.buffer.text() {
            return;
        }
        self.buffer.set_text(text);
        self.after_change(false, cx);
    }

    /// Reconfigure dialect + schema (the old `sqlCompartment.reconfigure`).
    pub fn configure(&mut self, driver: &str, db: Option<DbSchema>, cx: &mut Context<Self>) {
        let dialect = Dialect::for_driver(driver);
        let db = db.map(Arc::new);
        if dialect == self.dialect && db.as_deref() == self.db.as_deref() {
            return;
        }
        self.dialect = dialect;
        self.engine = Arc::new(Engine::new(dialect, db.as_deref().cloned()));
        self.db = db;
        self.retokenize();
        self.schedule_lint(cx);
        cx.notify();
    }

    pub fn set_font_scale(&mut self, scale: f32, cx: &mut Context<Self>) {
        if (scale - self.font_scale).abs() > f32::EPSILON {
            self.font_scale = scale;
            cx.notify();
        }
    }

    pub fn selected_text(&self) -> &str {
        &self.buffer.text()[self.buffer.selection.range()]
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    fn retokenize(&mut self) {
        self.tokens = tokenize(self.buffer.text(), self.dialect);
    }

    fn after_change(&mut self, typed: bool, cx: &mut Context<Self>) {
        self.retokenize();
        self.schedule_lint(cx);
        self.reset_blink(cx);
        self.scroll_cursor_into_view();
        if typed {
            self.update_completion_after_typing(cx);
        } else if self.completion.is_some() {
            self.refresh_open_completion(cx);
        }
        cx.emit(EditorEvent::Changed);
        cx.notify();
    }

    fn after_selection(&mut self, cx: &mut Context<Self>) {
        self.reset_blink(cx);
        self.scroll_cursor_into_view();
        if let Some(c) = &self.completion {
            let pos = self.buffer.selection.head;
            let valid = c.results.iter().all(|r| pos >= r.from && self.buffer.selection.is_empty());
            if !valid {
                self.completion = None;
            } else {
                self.refresh_open_completion(cx);
            }
        }
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        // CodeMirror: `cm-blink` 1.2s steps(1) — visible 600ms, hidden 600ms.
        self.blink_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(600)).await;
            if this
                .update(cx, |this, cx| {
                    this.cursor_visible = !this.cursor_visible;
                    cx.notify();
                })
                .is_err()
            {
                break;
            }
        }));
    }

    fn schedule_lint(&mut self, cx: &mut Context<Self>) {
        let text = self.buffer.text().to_string();
        let dialect = self.dialect;
        let db = self.db.clone();
        self.lint_task = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(Duration::from_millis(100)).await;
            let diagnostics = cx
                .background_executor()
                .spawn(async move { lint(&text, dialect, db.as_deref()) })
                .await;
            this.update(cx, |this, cx| {
                this.diagnostics = diagnostics;
                cx.notify();
            })
            .ok();
        }));
    }

    // ─── Completion ─────────────────────────────────────────────────────────

    fn start_completion(&mut self, explicit: bool, delay: bool, cx: &mut Context<Self>) {
        let doc = self.buffer.text().to_string();
        let pos = self.buffer.selection.head;
        let engine = self.engine.clone();
        self.completion_task = Some(cx.spawn(async move |this, cx| {
            if delay {
                cx.background_executor().timer(Duration::from_millis(100)).await;
            }
            let (results, options) = cx
                .background_executor()
                .spawn(async move {
                    let results = engine.query(&doc, pos, explicit);
                    let options = rank(&doc, pos, &results);
                    (results, options)
                })
                .await;
            this.update(cx, |this, cx| {
                if this.buffer.selection.head != pos || !this.buffer.selection.is_empty() {
                    return;
                }
                if options.is_empty() || results.is_empty() {
                    this.completion = None;
                } else {
                    let anchor = results.iter().map(|r| r.from).min().unwrap_or(pos);
                    this.completion = Some(CompletionState { results, options, selected: 0, anchor, explicit, scroll_top: 0.0 });
                }
                cx.notify();
            })
            .ok();
        }));
    }

    fn update_completion_after_typing(&mut self, cx: &mut Context<Self>) {
        if self.completion.is_some() {
            self.refresh_open_completion(cx);
        } else {
            self.start_completion(false, true, cx);
        }
    }

    /// Re-filter the open list if every result is still valid for the typed
    /// span, otherwise query the sources again.
    fn refresh_open_completion(&mut self, cx: &mut Context<Self>) {
        let pos = self.buffer.selection.head;
        let doc = self.buffer.text();
        let Some(state) = &mut self.completion else { return };
        let still_valid = state
            .results
            .iter()
            .all(|r| r.from <= pos && doc.is_char_boundary(r.from) && r.valid_for.accepts(&doc[r.from..pos]));
        if still_valid {
            let options = rank(doc, pos, &state.results);
            if options.is_empty() {
                self.completion = None;
            } else {
                state.options = options;
                state.selected = 0;
                state.scroll_top = 0.0;
            }
            cx.notify();
        } else {
            let explicit = state.explicit;
            self.completion = None;
            self.start_completion(explicit, false, cx);
        }
    }

    pub fn accept_completion(&mut self, index: usize, cx: &mut Context<Self>) -> bool {
        let Some(state) = self.completion.take() else { return false };
        let Some(opt) = state.options.get(index) else { return false };
        let pos = self.buffer.selection.head;
        let to = opt.source_to.unwrap_or(pos).max(pos);
        let insert = opt.completion.apply.clone().unwrap_or_else(|| opt.completion.label.clone());
        self.buffer.edit(opt.source_from..to, &insert, EditKind::Other);
        self.completion_task = None;
        self.after_change(false, cx);
        true
    }

    pub fn select_completion(&mut self, index: usize, cx: &mut Context<Self>) {
        let scale = self.font_scale;
        if let Some(c) = &mut self.completion {
            c.selected = index.min(c.options.len().saturating_sub(1));
            // scrollIntoView of the selected option.
            let row_h = popup::row_height(scale);
            let (from, _) = popup::rendered_range(c.options.len(), c.selected, MAX_RENDERED_OPTIONS);
            let rows = (c.options.len() - from).min(MAX_RENDERED_OPTIONS) as f32;
            let view_h = (rows * row_h).min(popup::max_list_height(scale));
            let top = (c.selected - from) as f32 * row_h;
            if top < c.scroll_top {
                c.scroll_top = top;
            } else if top + row_h > c.scroll_top + view_h {
                c.scroll_top = top + row_h - view_h;
            }
            cx.notify();
        }
    }

    /// Diagnostics under `position`: `(index of the first diagnostic, on gutter)`.
    fn diagnostic_at(&self, position: Point<Pixels>) -> Option<(usize, bool)> {
        let layout = self.layout.as_ref()?;
        if self.diagnostics.is_empty() || !layout.bounds.contains(&position) {
            return None;
        }
        let numbers_w = layout.gutter_width - px(1.4 * layout.font_size);
        if position.x < layout.bounds.left() + numbers_w {
            return None;
        }
        let y = f32::from(position.y - layout.bounds.top() + self.scroll.y) - element::CONTENT_PAD_Y;
        if y < 0.0 {
            return None;
        }
        let line = (y / layout.line_height).floor() as usize;
        if line >= self.buffer.line_count() {
            return None;
        }
        if position.x < layout.bounds.left() + layout.gutter_width {
            // Lint gutter marker: diagnostics starting on this line.
            return self
                .diagnostics
                .iter()
                .position(|d| self.buffer.line_of(d.from.min(self.buffer.len())) == line)
                .map(|i| (i, true));
        }
        let offset = layout.offset_for_point(&self.buffer, position, self.scroll);
        self.diagnostics
            .iter()
            .position(|d| offset >= d.from && offset <= d.to)
            .map(|i| (i, false))
    }

    fn update_hover(&mut self, position: Point<Pixels>, cx: &mut Context<Self>) {
        let target = self.diagnostic_at(position);
        if target == self.hover_target {
            return;
        }
        self.hover_target = target;
        self.hover_ready = false;
        self.hover_task = target.map(|_| {
            cx.spawn(async move |this, cx| {
                cx.background_executor().timer(Duration::from_millis(popup::HOVER_TIME)).await;
                this.update(cx, |this, cx| {
                    this.hover_ready = true;
                    cx.notify();
                })
                .ok();
            })
        });
        cx.notify();
    }

    /// Messages shown by the hover tooltip, with its anchor rectangle.
    fn hover_tooltip_data(&self) -> Option<(Vec<String>, Bounds<Pixels>, bool)> {
        if !self.hover_ready {
            return None;
        }
        let (index, gutter) = self.hover_target?;
        let layout = self.layout.as_ref()?;
        let d = self.diagnostics.get(index)?;
        let line = self.buffer.line_of(d.from.min(self.buffer.len()));
        if gutter {
            let messages: Vec<String> = self
                .diagnostics
                .iter()
                .filter(|o| self.buffer.line_of(o.from.min(self.buffer.len())) == line)
                .map(|o| o.message.clone())
                .collect();
            // The marker box: .2em padding inside the 1.4em lint gutter.
            let em = layout.font_size;
            let numbers_w = layout.gutter_width - px(1.4 * em);
            let x = layout.bounds.left() + numbers_w + px(0.2 * em);
            let top = layout.bounds.top() + px(element::CONTENT_PAD_Y + line as f32 * layout.line_height + 0.2 * em)
                - self.scroll.y;
            return Some((messages, Bounds::new(gpui::point(x, top), gpui::size(px(em), px(em))), false));
        }
        let messages: Vec<String> = self
            .diagnostics
            .iter()
            .filter(|o| o.from == d.from && o.to == d.to)
            .map(|o| o.message.clone())
            .collect();
        let anchor = layout.bounds_for_offset(&self.buffer, d.from.min(self.buffer.len()), self.scroll);
        Some((messages, anchor, true))
    }

    // ─── Editing primitives ─────────────────────────────────────────────────

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let range = sel.range();
        let typed_single = text.chars().count() == 1;
        let ch = text.chars().next();
        // closeBrackets: ( [ { ' " `
        if typed_single {
            let c = ch.unwrap();
            let next = self.buffer.char_at(range.end);
            let closing = matches!(c, ')' | ']' | '}' | '\'' | '"' | '`');
            if closing && sel.is_empty() && next == Some(c) && self.is_auto_closed_at(range.end) {
                let pos = self.buffer.next_char(range.end);
                self.buffer.selection = Selection::cursor(pos);
                self.after_selection(cx);
                return;
            }
            if let Some(close) = match c {
                '(' => Some(')'),
                '[' => Some(']'),
                '{' => Some('}'),
                '\'' | '"' | '`' => Some(c),
                _ => None,
            } {
                let quote = matches!(c, '\'' | '"' | '`');
                if !sel.is_empty() {
                    // Wrap the selection.
                    let inner = self.buffer.text()[range.clone()].to_string();
                    let new = format!("{c}{inner}{close}");
                    let start = range.start;
                    self.buffer.edit_many(
                        vec![(range.clone(), new)],
                        Selection { anchor: start + c.len_utf8(), head: start + c.len_utf8() + inner.len() },
                        EditKind::Type,
                    );
                    self.after_change(true, cx);
                    return;
                }
                let next_ok = match next {
                    None => true,
                    Some(n) => n.is_whitespace() || ")]}:;>".contains(n),
                };
                let prev = self.buffer.char_before(range.start);
                let quote_ok = !quote || !prev.is_some_and(|p| p.is_alphanumeric() || p == '_' || p == c);
                if next_ok && quote_ok {
                    let pair = format!("{c}{close}");
                    self.buffer.edit(range.clone(), &pair, EditKind::Type);
                    let pos = range.start + c.len_utf8();
                    self.buffer.selection = Selection::cursor(pos);
                    self.after_change(true, cx);
                    return;
                }
            }
        }
        self.buffer.edit(range, text, EditKind::Type);
        self.after_change(true, cx);
    }

    fn is_auto_closed_at(&self, pos: usize) -> bool {
        // Approximation of CodeMirror's tracked bracket state: the closing
        // char directly follows the cursor and pairs with the char before.
        let before = self.buffer.char_before(pos);
        let at = self.buffer.char_at(pos);
        matches!((before, at), (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}')))
            || matches!(at, Some('\'') | Some('"') | Some('`'))
            || at.is_some_and(|c| matches!(c, ')' | ']' | '}'))
    }

    fn delete_backward(&mut self, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if !sel.is_empty() {
            self.buffer.edit(sel.range(), "", EditKind::Delete);
        } else {
            let pos = sel.head;
            if pos == 0 {
                return;
            }
            let before = self.buffer.char_before(pos);
            let after = self.buffer.char_at(pos);
            let pair = matches!(
                (before, after),
                (Some('('), Some(')')) | (Some('['), Some(']')) | (Some('{'), Some('}')) | (Some('\''), Some('\'')) | (Some('"'), Some('"')) | (Some('`'), Some('`'))
            );
            let start = self.buffer.prev_char(pos);
            let end = if pair { self.buffer.next_char(pos) } else { pos };
            // Deleting indentation removes a whole indent unit (2 spaces).
            let line = self.buffer.line_of(pos);
            let ls = self.buffer.line_start(line);
            let before_text = &self.buffer.text()[ls..pos];
            if !pair && !before_text.is_empty() && before_text.chars().all(|c| c == ' ') {
                let col = before_text.len();
                let target = if col % 2 == 0 { col - 2.min(col) } else { col - 1 };
                self.buffer.edit(ls + target..pos, "", EditKind::Delete);
            } else {
                self.buffer.edit(start..end, "", EditKind::Delete);
            }
        }
        self.after_change(false, cx);
        if self.completion.is_some() {
            self.refresh_open_completion(cx);
        }
    }

    fn delete_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        if range.is_empty() {
            return;
        }
        self.buffer.edit(range, "", EditKind::Delete);
        self.after_change(false, cx);
    }

    fn move_to(&mut self, pos: usize, extend: bool, cx: &mut Context<Self>) {
        let pos = self.buffer.clamp(pos);
        if extend {
            self.buffer.selection.head = pos;
        } else {
            self.buffer.selection = Selection::cursor(pos);
        }
        self.after_selection(cx);
    }

    fn vertical(&mut self, lines: isize, extend: bool, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if !extend && !sel.is_empty() && lines.abs() == 1 {
            let r = sel.range();
            let target = if lines < 0 { r.start } else { r.end };
            // CodeMirror collapses then moves from the relevant end.
            self.buffer.selection = Selection::cursor(target);
        }
        let head = self.buffer.selection.head;
        let line = self.buffer.line_of(head) as isize;
        let goal = self.buffer.goal_column.unwrap_or_else(|| self.buffer.column_chars(head));
        let target_line = line + lines;
        let pos = if target_line < 0 {
            0
        } else if target_line as usize >= self.buffer.line_count() {
            self.buffer.len()
        } else {
            self.buffer.offset_at_column(target_line as usize, goal)
        };
        if extend {
            self.buffer.selection.head = pos;
        } else {
            self.buffer.selection = Selection::cursor(pos);
        }
        self.buffer.goal_column = Some(goal);
        self.after_selection(cx);
    }

    fn page_lines(&self) -> isize {
        let lh = self.line_height();
        let h = self.layout.as_ref().map(|l| l.bounds.size.height).unwrap_or(px(400.));
        (f32::from(h) / lh).floor().max(1.0) as isize
    }

    pub fn line_height(&self) -> f32 {
        13.0 * self.font_scale * 1.6
    }

    fn scroll_cursor_into_view(&mut self) {
        let Some(layout) = &self.layout else { return };
        let lh = self.line_height();
        let line = self.buffer.line_of(self.buffer.selection.head) as f32;
        let top = 12.0 + line * lh;
        let bottom = top + lh;
        let h: f32 = layout.bounds.size.height.into();
        let mut y: f32 = self.scroll.y.into();
        if top < y {
            y = top;
        } else if bottom > y + h {
            y = bottom - h;
        }
        // Horizontal: keep caret inside the text area.
        let cx_pos = layout.x_for_offset(&self.buffer, self.buffer.selection.head);
        let text_w: f32 = (layout.bounds.size.width - layout.gutter_width).into();
        let mut x: f32 = self.scroll.x.into();
        let caret: f32 = cx_pos.into();
        if caret - x > text_w - 4.0 {
            x = caret - text_w + 4.0;
        }
        if caret < x {
            x = (caret - 6.0).max(0.0);
        }
        self.scroll = Point { x: px(x.max(0.0)), y: px(y.max(0.0)) };
    }

    // ─── Actions ────────────────────────────────────────────────────────────

    fn a_left(&mut self, _: &MoveLeft, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if !sel.is_empty() {
            self.move_to(sel.range().start, false, cx);
        } else {
            self.move_to(self.buffer.prev_char(sel.head), false, cx);
        }
    }
    fn a_right(&mut self, _: &MoveRight, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if !sel.is_empty() {
            self.move_to(sel.range().end, false, cx);
        } else {
            self.move_to(self.buffer.next_char(sel.head), false, cx);
        }
    }
    fn a_up(&mut self, _: &MoveUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let n = c.options.len();
            let s = if c.selected == 0 { n - 1 } else { c.selected - 1 };
            self.select_completion(s, cx);
            return;
        }
        self.vertical(-1, false, cx);
    }
    fn a_down(&mut self, _: &MoveDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let n = c.options.len();
            let s = if c.selected + 1 >= n { 0 } else { c.selected + 1 };
            self.select_completion(s, cx);
            return;
        }
        self.vertical(1, false, cx);
    }
    fn a_word_left(&mut self, _: &MoveWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.word_left(self.buffer.selection.head), false, cx);
    }
    fn a_word_right(&mut self, _: &MoveWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.word_right(self.buffer.selection.head), false, cx);
    }
    fn line_start_target(&self, head: usize) -> usize {
        // CodeMirror moves to the first non-space char, then to column 0.
        let line = self.buffer.line_of(head);
        let ls = self.buffer.line_start(line);
        let indent = self.buffer.line_text(line).len() - self.buffer.line_text(line).trim_start().len();
        if head == ls + indent || indent == self.buffer.line_text(line).len() {
            ls
        } else {
            ls + indent
        }
    }
    fn a_line_start(&mut self, _: &MoveLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start_target(self.buffer.selection.head), false, cx);
    }
    fn a_line_end(&mut self, _: &MoveLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.line_of(self.buffer.selection.head);
        self.move_to(self.buffer.line_end(line), false, cx);
    }
    fn a_doc_start(&mut self, _: &MoveDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, false, cx);
    }
    fn a_doc_end(&mut self, _: &MoveDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.len(), false, cx);
    }
    fn a_page_up(&mut self, _: &MovePageUp, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let s = c.selected.saturating_sub(6);
            self.select_completion(s, cx);
            return;
        }
        self.vertical(-self.page_lines(), false, cx);
    }
    fn a_page_down(&mut self, _: &MovePageDown, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let s = (c.selected + 6).min(c.options.len() - 1);
            self.select_completion(s, cx);
            return;
        }
        self.vertical(self.page_lines(), false, cx);
    }
    fn a_sel_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.prev_char(self.buffer.selection.head), true, cx);
    }
    fn a_sel_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.next_char(self.buffer.selection.head), true, cx);
    }
    fn a_sel_up(&mut self, _: &SelectUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-1, true, cx);
    }
    fn a_sel_down(&mut self, _: &SelectDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(1, true, cx);
    }
    fn a_sel_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.word_left(self.buffer.selection.head), true, cx);
    }
    fn a_sel_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.word_right(self.buffer.selection.head), true, cx);
    }
    fn a_sel_line_start(&mut self, _: &SelectLineStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.line_start_target(self.buffer.selection.head), true, cx);
    }
    fn a_sel_line_end(&mut self, _: &SelectLineEnd, _: &mut Window, cx: &mut Context<Self>) {
        let line = self.buffer.line_of(self.buffer.selection.head);
        self.move_to(self.buffer.line_end(line), true, cx);
    }
    fn a_sel_doc_start(&mut self, _: &SelectDocStart, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, true, cx);
    }
    fn a_sel_doc_end(&mut self, _: &SelectDocEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.buffer.len(), true, cx);
    }
    fn a_sel_page_up(&mut self, _: &SelectPageUp, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(-self.page_lines(), true, cx);
    }
    fn a_sel_page_down(&mut self, _: &SelectPageDown, _: &mut Window, cx: &mut Context<Self>) {
        self.vertical(self.page_lines(), true, cx);
    }
    fn a_select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.buffer.selection = Selection { anchor: 0, head: self.buffer.len() };
        self.completion = None;
        self.after_selection(cx);
    }
    fn a_select_line(&mut self, _: &SelectLine, _: &mut Window, cx: &mut Context<Self>) {
        let r = self.buffer.selection.range();
        let l0 = self.buffer.line_of(r.start);
        let l1 = self.buffer.line_of(r.end);
        let end = if l1 + 1 < self.buffer.line_count() { self.buffer.line_start(l1 + 1) } else { self.buffer.len() };
        self.buffer.selection = Selection { anchor: self.buffer.line_start(l0), head: end };
        self.after_selection(cx);
    }
    fn a_backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        self.delete_backward(cx);
    }
    fn a_delete(&mut self, _: &Delete, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let range = if sel.is_empty() { sel.head..self.buffer.next_char(sel.head) } else { sel.range() };
        self.delete_range(range, cx);
    }
    fn a_delete_word_left(&mut self, _: &DeleteWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let range = if sel.is_empty() { self.buffer.word_left(sel.head)..sel.head } else { sel.range() };
        self.delete_range(range, cx);
    }
    fn a_delete_word_right(&mut self, _: &DeleteWordRight, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let range = if sel.is_empty() { sel.head..self.buffer.word_right(sel.head) } else { sel.range() };
        self.delete_range(range, cx);
    }
    fn a_delete_line_start(&mut self, _: &DeleteLineStart, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let range = if sel.is_empty() {
            let ls = self.buffer.line_start(self.buffer.line_of(sel.head));
            if ls == sel.head { self.buffer.prev_char(sel.head)..sel.head } else { ls..sel.head }
        } else {
            sel.range()
        };
        self.delete_range(range, cx);
    }
    fn a_delete_line(&mut self, _: &DeleteLine, _: &mut Window, cx: &mut Context<Self>) {
        let r = self.buffer.selection.range();
        let l0 = self.buffer.line_of(r.start);
        let l1 = self.buffer.line_of(r.end);
        let start = self.buffer.line_start(l0);
        let (start, end) = if l1 + 1 < self.buffer.line_count() {
            (start, self.buffer.line_start(l1 + 1))
        } else if l0 > 0 {
            (start - 1, self.buffer.len())
        } else {
            (start, self.buffer.len())
        };
        self.delete_range(start..end, cx);
    }
    fn a_newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let i = c.selected;
            self.accept_completion(i, cx);
            return;
        }
        let range = self.buffer.selection.range();
        let indent = self.indent_for_newline(range.start);
        // Strip trailing whitespace on the line being split (CodeMirror does).
        let line = self.buffer.line_of(range.start);
        let ls = self.buffer.line_start(line);
        let before = &self.buffer.text()[ls..range.start];
        let trimmed_len = before.trim_end().len();
        let start = if before.trim().is_empty() { range.start } else { ls + trimmed_len };
        let mut end = range.end;
        // Also swallow spaces after the cursor.
        while self.buffer.char_at(end) == Some(' ') {
            end += 1;
        }
        let text = format!("\n{}", " ".repeat(indent));
        self.buffer.edit(start..end, &text, EditKind::Other);
        self.after_change(false, cx);
    }

    fn indent_for_newline(&self, pos: usize) -> usize {
        let spans = crate::ui::sql::statements(&self.tokens);
        let inside = spans.iter().find(|s| {
            let terminated = self.tokens.get(s.end_token.saturating_sub(1)).is_some_and(|t| t.kind == Tok::Semi);
            s.from < pos && (pos < s.to || (!terminated && pos >= s.to))
        });
        match inside {
            Some(span) => self.buffer.indentation_of_line(self.buffer.line_of(span.from)) + 2,
            None => 0,
        }
    }

    fn a_tab(&mut self, _: &Tab, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(c) = &self.completion {
            let i = c.selected;
            self.accept_completion(i, cx);
            return;
        }
        self.indent_more(window, cx);
    }
    fn indent_more(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let r = sel.range();
        let l0 = self.buffer.line_of(r.start);
        let l1 = self.buffer.line_of(r.end);
        let edits: Vec<(Range<usize>, String)> =
            (l0..=l1).map(|l| (self.buffer.line_start(l)..self.buffer.line_start(l), "  ".to_string())).collect();
        let n = edits.len();
        let shift = |p: usize, line_of: usize| p + 2 * (line_of - l0 + 1);
        let new_sel = Selection {
            anchor: shift(sel.anchor, self.buffer.line_of(sel.anchor)),
            head: shift(sel.head, self.buffer.line_of(sel.head)),
        };
        let _ = n;
        self.buffer.edit_many(edits, new_sel, EditKind::Other);
        self.after_change(false, cx);
    }
    fn a_shift_tab(&mut self, _: &ShiftTab, window: &mut Window, cx: &mut Context<Self>) {
        self.indent_less(window, cx);
    }
    fn indent_less(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let r = sel.range();
        let l0 = self.buffer.line_of(r.start);
        let l1 = self.buffer.line_of(r.end);
        let mut edits = Vec::new();
        let mut removed_per_line = Vec::new();
        for l in l0..=l1 {
            let ind = self.buffer.line_text(l).chars().take_while(|c| *c == ' ').count();
            let remove = if ind == 0 { 0 } else if ind % 2 == 0 { 2 } else { ind % 2 };
            let ls = self.buffer.line_start(l);
            if remove > 0 {
                edits.push((ls..ls + remove, String::new()));
            }
            removed_per_line.push((ls, remove));
        }
        let adjust = |p: usize| -> usize {
            let mut out = p;
            for (ls, rem) in &removed_per_line {
                if p >= *ls + *rem {
                    out -= rem;
                } else if p > *ls {
                    out -= p - ls;
                }
            }
            out
        };
        let new_sel = Selection { anchor: adjust(sel.anchor), head: adjust(sel.head) };
        if !edits.is_empty() {
            self.buffer.edit_many(edits, new_sel, EditKind::Other);
            self.after_change(false, cx);
        }
    }
    fn a_indent_more(&mut self, _: &IndentMore, window: &mut Window, cx: &mut Context<Self>) {
        self.indent_more(window, cx);
    }
    fn a_indent_less(&mut self, _: &IndentLess, window: &mut Window, cx: &mut Context<Self>) {
        self.indent_less(window, cx);
    }
    fn a_toggle_comment(&mut self, _: &ToggleComment, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let r = sel.range();
        let l0 = self.buffer.line_of(r.start);
        let mut l1 = self.buffer.line_of(r.end);
        if l1 > l0 && r.end == self.buffer.line_start(l1) {
            l1 -= 1;
        }
        let lines: Vec<usize> = (l0..=l1).filter(|l| !self.buffer.line_text(*l).trim().is_empty()).collect();
        if lines.is_empty() {
            return;
        }
        let all_commented = lines.iter().all(|l| self.buffer.line_text(*l).trim_start().starts_with("--"));
        let min_indent = lines.iter().map(|l| self.buffer.line_text(*l).len() - self.buffer.line_text(*l).trim_start().len()).min().unwrap_or(0);
        let mut edits = Vec::new();
        let mut deltas: Vec<(usize, isize)> = Vec::new();
        for l in &lines {
            let ls = self.buffer.line_start(*l);
            let text = self.buffer.line_text(*l);
            if all_commented {
                let ind = text.len() - text.trim_start().len();
                let has_space = text[ind + 2..].starts_with(' ');
                let len = if has_space { 3 } else { 2 };
                edits.push((ls + ind..ls + ind + len, String::new()));
                deltas.push((ls + ind, -(len as isize)));
            } else {
                edits.push((ls + min_indent..ls + min_indent, "-- ".to_string()));
                deltas.push((ls + min_indent, 3));
            }
        }
        let adjust = |p: usize| -> usize {
            let mut out = p as isize;
            for (at, d) in &deltas {
                if p > *at || (p == *at && *d > 0) {
                    out += d;
                }
            }
            out.max(0) as usize
        };
        let new_sel = Selection { anchor: adjust(sel.anchor), head: adjust(sel.head) };
        self.buffer.edit_many(edits, new_sel, EditKind::Other);
        self.after_change(false, cx);
    }
    fn move_lines(&mut self, up: bool, copy: bool, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let r = sel.range();
        let l0 = self.buffer.line_of(r.start);
        let mut l1 = self.buffer.line_of(r.end);
        if l1 > l0 && r.end == self.buffer.line_start(l1) {
            l1 -= 1;
        }
        let block_start = self.buffer.line_start(l0);
        let block_end = self.buffer.line_end(l1);
        let block = self.buffer.text()[block_start..block_end].to_string();
        if copy {
            let (at, text, shift) = if up {
                (block_start, format!("{block}\n"), 0)
            } else {
                (block_end, format!("\n{block}"), block.len() + 1)
            };
            let new_sel = Selection { anchor: sel.anchor + shift, head: sel.head + shift };
            self.buffer.edit_many(vec![(at..at, text)], new_sel, EditKind::Other);
            self.after_change(false, cx);
            return;
        }
        if up && l0 == 0 || !up && l1 + 1 >= self.buffer.line_count() {
            return;
        }
        if up {
            let prev_start = self.buffer.line_start(l0 - 1);
            let prev = self.buffer.text()[prev_start..block_start - 1].to_string();
            let new_text = format!("{block}\n{prev}");
            let shift = prev.len() + 1;
            let new_sel = Selection { anchor: sel.anchor - shift, head: sel.head - shift };
            self.buffer.edit_many(vec![(prev_start..block_end, new_text)], new_sel, EditKind::Other);
        } else {
            let next_end = self.buffer.line_end(l1 + 1);
            let next = self.buffer.text()[block_end + 1..next_end].to_string();
            let new_text = format!("{next}\n{block}");
            let shift = next.len() + 1;
            let new_sel = Selection { anchor: sel.anchor + shift, head: sel.head + shift };
            self.buffer.edit_many(vec![(block_start..next_end, new_text)], new_sel, EditKind::Other);
        }
        self.after_change(false, cx);
    }
    fn a_move_line_up(&mut self, _: &MoveLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(true, false, cx);
    }
    fn a_move_line_down(&mut self, _: &MoveLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(false, false, cx);
    }
    fn a_copy_line_up(&mut self, _: &CopyLineUp, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(true, true, cx);
    }
    fn a_copy_line_down(&mut self, _: &CopyLineDown, _: &mut Window, cx: &mut Context<Self>) {
        self.move_lines(false, true, cx);
    }
    fn a_copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        let text = if sel.is_empty() {
            // CodeMirror copies the whole line when nothing is selected.
            let l = self.buffer.line_of(sel.head);
            format!("{}\n", self.buffer.line_text(l))
        } else {
            self.selected_text().to_string()
        };
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }
    fn a_cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if sel.is_empty() {
            let l = self.buffer.line_of(sel.head);
            cx.write_to_clipboard(ClipboardItem::new_string(format!("{}\n", self.buffer.line_text(l))));
            let start = self.buffer.line_start(l);
            let end = if l + 1 < self.buffer.line_count() { self.buffer.line_start(l + 1) } else { self.buffer.len() };
            self.delete_range(start..end, cx);
        } else {
            cx.write_to_clipboard(ClipboardItem::new_string(self.selected_text().to_string()));
            self.delete_range(sel.range(), cx);
        }
    }
    fn a_paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            let text = text.replace("\r\n", "\n");
            let range = self.buffer.selection.range();
            self.buffer.edit(range, &text, EditKind::Other);
            self.completion = None;
            self.after_change(false, cx);
        }
    }
    fn a_undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.undo() {
            self.completion = None;
            self.after_change(false, cx);
        }
    }
    fn a_redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if self.buffer.redo() {
            self.completion = None;
            self.after_change(false, cx);
        }
    }
    fn a_run(&mut self, _: &RunQuery, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(EditorEvent::Run);
    }
    fn a_start_completion(&mut self, _: &StartCompletion, _: &mut Window, cx: &mut Context<Self>) {
        self.start_completion(true, false, cx);
    }
    fn a_escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        if self.goto.is_some() || self.search.is_some() {
            self.close_search(window, cx);
        } else if self.completion.take().is_some() {
            cx.notify();
        } else if !self.buffer.selection.is_empty() {
            let head = self.buffer.selection.head;
            self.move_to(head, false, cx);
        }
    }
    fn a_matching_bracket(&mut self, _: &CursorMatchingBracket, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((a, b)) = self.matching_brackets() {
            let head = self.buffer.selection.head;
            let target = if head == a.start || head == a.end { b.start } else { a.start };
            self.move_to(target, false, cx);
        }
    }
    fn a_select_next(&mut self, _: &SelectNextOccurrence, _: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection;
        if sel.is_empty() {
            let r = self.buffer.word_range_at(sel.head);
            self.buffer.selection = Selection { anchor: r.start, head: r.end };
        } else {
            let needle = self.selected_text().to_string();
            let start = sel.range().end;
            let found = self.buffer.text()[start..]
                .find(&needle)
                .map(|i| start + i)
                .or_else(|| self.buffer.text().find(&needle));
            if let Some(i) = found {
                self.buffer.selection = Selection { anchor: i, head: i + needle.len() };
            }
        }
        self.after_selection(cx);
    }


    // ─── Search panel (@codemirror/search) ──────────────────────────────────

    fn a_open_search(&mut self, _: &OpenSearch, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(panel) = &self.search {
            panel.search_input.update(cx, |i, cx| {
                i.select_all_text(cx);
                i.focus(window);
            });
            cx.notify();
            return;
        }
        let look = search::textfield_look(13.0 * self.font_scale);
        // The panel opens seeded with the selection, like CodeMirror.
        let seed = {
            let sel = self.buffer.selection.range();
            let text = &self.buffer.text()[sel.clone()];
            if !sel.is_empty() && !text.contains('\n') { text.to_string() } else { String::new() }
        };
        let search_input = cx.new(|cx| {
            let mut t = TextInput::new(cx, look.clone()).with_placeholder("Find".to_string());
            t.set_text(seed.clone(), cx);
            t
        });
        let replace_input = cx.new(|cx| TextInput::new(cx, look).with_placeholder("Replace".to_string()));
        let subs = vec![
            cx.subscribe_in(&search_input, window, |this, _e, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.search_query_changed(cx),
                InputEvent::Submit => this.find_next(cx),
                InputEvent::Cancel => this.close_search(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            }),
            cx.subscribe_in(&replace_input, window, |this, _e, ev: &InputEvent, window, cx| match ev {
                InputEvent::Changed => this.search_query_changed(cx),
                InputEvent::Submit => this.replace_next(cx),
                InputEvent::Cancel => this.close_search(window, cx),
                InputEvent::Focus | InputEvent::Blur => {}
            }),
        ];
        search_input.update(cx, |i, _| i.focus(window));
        search_input.update(cx, |i, cx| i.select_all_text(cx));
        self._search_subs = subs;
        self.search = Some(search::SearchPanel {
            query: search::Query { search: seed, ..Default::default() },
            search_input,
            replace_input,
            matches: Vec::new(),
        });
        self.search_query_changed(cx);
    }

    pub fn close_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search = None;
        self._search_subs.clear();
        self.goto = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    /// Re-read the fields into the query and recompute the match list.
    pub fn search_query_changed(&mut self, cx: &mut Context<Self>) {
        let (search, replace) = match &self.search {
            Some(p) => (p.search_input.read(cx).text().to_string(), p.replace_input.read(cx).text().to_string()),
            None => return,
        };
        let doc = self.buffer.text().to_string();
        if let Some(p) = &mut self.search {
            p.query.search = search;
            p.query.replace = replace;
            p.matches = p.query.matches(&doc);
        }
        cx.notify();
    }

    pub fn toggle_search_option(&mut self, option: &'static str, cx: &mut Context<Self>) {
        if let Some(p) = &mut self.search {
            match option {
                "case" => p.query.case_sensitive = !p.query.case_sensitive,
                "re" => p.query.regexp = !p.query.regexp,
                _ => p.query.whole_word = !p.query.whole_word,
            }
        }
        self.search_query_changed(cx);
    }

    fn a_find_next(&mut self, _: &FindNext, _: &mut Window, cx: &mut Context<Self>) {
        self.find_next(cx);
    }

    fn a_find_previous(&mut self, _: &FindPrevious, _: &mut Window, cx: &mut Context<Self>) {
        self.find_previous(cx);
    }

    pub fn find_next(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_ref().map(|p| p.query.clone()) else { return };
        let from = self.buffer.selection.range().start + 1;
        let doc = self.buffer.text().to_string();
        if let Some(m) = query.next(&doc, from.min(doc.len())) {
            self.select_range(m, cx);
        }
    }

    pub fn find_previous(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_ref().map(|p| p.query.clone()) else { return };
        let to = self.buffer.selection.range().start;
        let doc = self.buffer.text().to_string();
        if let Some(m) = query.previous(&doc, to) {
            self.select_range(m, cx);
        }
    }

    /// `selectMatches`: this editor has a single selection, so the matches are
    /// all highlighted and the last one is selected.
    pub fn select_all_matches(&mut self, cx: &mut Context<Self>) {
        let Some(p) = &self.search else { return };
        let Some(last) = p.matches.last().cloned() else { return };
        self.select_range(last, cx);
    }

    pub fn replace_next(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_ref().map(|p| p.query.clone()) else { return };
        let doc = self.buffer.text().to_string();
        let sel = self.buffer.selection.range();
        // Replace the selection when it is already a match, else find one first.
        let target = query
            .matches(&doc)
            .into_iter()
            .find(|m| *m == sel)
            .or_else(|| query.next(&doc, sel.start));
        let Some(target) = target else { return };
        if target != sel {
            self.select_range(target, cx);
            return;
        }
        let text = query.replacement(&doc, &target);
        self.buffer.edit(target.clone(), &text, EditKind::Other);
        self.after_change(false, cx);
        let after = target.start + text.len();
        let doc = self.buffer.text().to_string();
        if let Some(next) = query.next(&doc, after) {
            self.select_range(next, cx);
        }
        self.search_query_changed(cx);
    }

    pub fn replace_all(&mut self, cx: &mut Context<Self>) {
        let Some(query) = self.search.as_ref().map(|p| p.query.clone()) else { return };
        let doc = self.buffer.text().to_string();
        let matches = query.matches(&doc);
        if matches.is_empty() {
            return;
        }
        // Apply back to front so earlier offsets stay valid.
        for m in matches.into_iter().rev() {
            let text = query.replacement(&doc, &m);
            self.buffer.edit(m, &text, EditKind::Other);
        }
        self.after_change(false, cx);
        self.search_query_changed(cx);
    }

    fn a_select_selection_matches(&mut self, _: &SelectSelectionMatches, window: &mut Window, cx: &mut Context<Self>) {
        let sel = self.buffer.selection.range();
        if sel.is_empty() {
            return;
        }
        let needle = self.buffer.text()[sel].to_string();
        if self.search.is_none() {
            self.a_open_search(&OpenSearch, window, cx);
        }
        if let Some(p) = &self.search {
            p.search_input.update(cx, |i, cx| i.set_text(needle, cx));
        }
        self.search_query_changed(cx);
        self.select_all_matches(cx);
    }

    fn a_goto_line(&mut self, _: &GotoLine, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(dialog) = &self.goto {
            dialog.input.update(cx, |i, cx| {
                i.select_all_text(cx);
                i.focus(window);
            });
            cx.notify();
            return;
        }
        let line = self.buffer.line_of(self.buffer.selection.head) + 1;
        // The field sits inside the dialog's 80% label, so its own 70% is
        // relative to that.
        let look = search::textfield_look(0.8 * 13.0 * self.font_scale);
        let input = cx.new(|cx| {
            let mut t = TextInput::new(cx, look);
            t.set_text(line.to_string(), cx);
            t
        });
        let sub = cx.subscribe_in(&input, window, |this, _e, ev: &InputEvent, window, cx| match ev {
            InputEvent::Submit => this.goto_line_apply(window, cx),
            InputEvent::Cancel => this.close_search(window, cx),
            _ => {}
        });
        input.update(cx, |i, _| i.focus(window));
        input.update(cx, |i, cx| i.select_all_text(cx));
        self._search_subs.push(sub);
        self.goto = Some(search::GotoDialog { input });
        cx.notify();
    }

    pub fn goto_line_apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(dialog) = &self.goto else { return };
        let value = dialog.input.read(cx).text().to_string();
        let lines = self.buffer.line_count();
        let current = self.buffer.line_of(self.buffer.selection.head) + 1;
        if let Some((line, col)) = search::goto_target(&value, lines, current) {
            let start = self.buffer.line_start(line - 1);
            let len = self.buffer.line_text(line - 1).len();
            let pos = start + col.min(len);
            self.move_to(pos, false, cx);
        }
        self.goto = None;
        window.focus(&self.focus_handle);
        cx.notify();
    }

    fn select_range(&mut self, range: Range<usize>, cx: &mut Context<Self>) {
        self.buffer.selection = Selection { anchor: range.start, head: range.end };
        self.after_selection(cx);
    }

    /// Bracket pair touching the cursor (bracketMatching): (at cursor, match).
    pub fn matching_brackets(&self) -> Option<(Range<usize>, Range<usize>)> {
        let sel = self.buffer.selection;
        if !sel.is_empty() {
            return None;
        }
        let pos = sel.head;
        let idx_of = |p: usize| {
            self.tokens.iter().position(|t| {
                t.from == p && matches!(t.kind, Tok::ParenL | Tok::ParenR | Tok::BracketL | Tok::BracketR | Tok::BraceL | Tok::BraceR)
            })
        };
        // CodeMirror checks the char before the cursor first, then after.
        let candidates = [self.buffer.char_before(pos).map(|c| (pos - c.len_utf8(), c)), self.buffer.char_at(pos).map(|c| (pos, c))];
        for (at, _) in candidates.into_iter().flatten() {
            let Some(i) = idx_of(at) else { continue };
            let tok = self.tokens[i];
            let (open, close, forward) = match tok.kind {
                Tok::ParenL => (Tok::ParenL, Tok::ParenR, true),
                Tok::ParenR => (Tok::ParenL, Tok::ParenR, false),
                Tok::BracketL => (Tok::BracketL, Tok::BracketR, true),
                Tok::BracketR => (Tok::BracketL, Tok::BracketR, false),
                Tok::BraceL => (Tok::BraceL, Tok::BraceR, true),
                _ => (Tok::BraceL, Tok::BraceR, false),
            };
            let mut depth = 0i32;
            if forward {
                for t in &self.tokens[i..] {
                    if t.kind == open {
                        depth += 1;
                    } else if t.kind == close {
                        depth -= 1;
                        if depth == 0 {
                            return Some((tok.from..tok.to, t.from..t.to));
                        }
                    }
                }
            } else {
                for t in self.tokens[..=i].iter().rev() {
                    if t.kind == close {
                        depth += 1;
                    } else if t.kind == open {
                        depth -= 1;
                        if depth == 0 {
                            return Some((tok.from..tok.to, t.from..t.to));
                        }
                    }
                }
            }
            return Some((tok.from..tok.to, tok.from..tok.from));
        }
        None
    }

    // ─── Mouse ──────────────────────────────────────────────────────────────

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        if let Some(local) = self.bar_local(event.position) {
            let geom = self.geom();
            if let Some(bar) = geom.bar_at(local) {
                self.on_scrollbar_press(bar, geom.pos_on(bar, local), cx);
                return;
            }
        }
        let Some(layout) = &self.layout else { return };
        if let Some(idx) = layout.completion_hit(event.position) {
            self.accept_completion(idx, cx);
            return;
        }
        self.completion = None;
        let pos = layout.offset_for_point(&self.buffer, event.position, self.scroll);
        match event.click_count {
            1 => {
                if event.modifiers.shift {
                    self.buffer.selection.head = pos;
                } else {
                    self.buffer.selection = Selection::cursor(pos);
                }
                self.selecting = Some(SelectMode::Char);
            }
            2 => {
                let r = self.buffer.word_range_at(pos);
                self.buffer.selection = Selection { anchor: r.start, head: r.end };
                self.selecting = Some(SelectMode::Word(r.start, r.end));
            }
            _ => {
                let l = self.buffer.line_of(pos);
                let end = if l + 1 < self.buffer.line_count() { self.buffer.line_start(l + 1) } else { self.buffer.len() };
                self.buffer.selection = Selection { anchor: self.buffer.line_start(l), head: end };
                self.selecting = Some(SelectMode::Line(l));
            }
        }
        self.buffer.goal_column = None;
        self.reset_blink(cx);
        cx.notify();
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        // Dragging a scrollbar thumb, or just moving over one.
        if let Some(drag) = self.scroll_drag {
            if let Some(local) = self.bar_local(event.position) {
                let geom = self.geom();
                let along = geom.pos_on(drag.bar, local) - drag.grab;
                match drag.bar {
                    scroll::Bar::Vertical => self.scroll.y = px(geom.scroll_for_v_thumb(along)),
                    scroll::Bar::Horizontal => self.scroll.x = px(geom.scroll_for_h_thumb(along)),
                }
                self.clamp_scroll();
                cx.notify();
            }
            return;
        }
        let over_bar = self.bar_local(event.position).and_then(|l| self.geom().bar_at(l));
        if over_bar != self.hovered_bar {
            self.hovered_bar = over_bar;
            cx.notify();
        }
        let hover_changed = self.hover_pos != Some(event.position);
        self.hover_pos = Some(event.position);
        self.update_hover(event.position, cx);
        let Some(mode) = self.selecting else {
            if hover_changed {
                cx.notify();
            }
            return;
        };
        if !event.dragging() {
            self.selecting = None;
            return;
        }
        let Some(layout) = &self.layout else { return };
        // Auto-scroll when dragging past the edges.
        let b = layout.bounds;
        let lh = self.line_height();
        if event.position.y < b.top() {
            self.scroll.y = (self.scroll.y - px(lh)).max(px(0.));
        } else if event.position.y > b.bottom() {
            self.scroll.y += px(lh);
        }
        let pos = layout.offset_for_point(&self.buffer, event.position, self.scroll);
        match mode {
            SelectMode::Char => self.buffer.selection.head = pos,
            SelectMode::Word(s, e) => {
                let r = self.buffer.word_range_at(pos);
                if pos < s {
                    self.buffer.selection = Selection { anchor: e, head: r.start };
                } else {
                    self.buffer.selection = Selection { anchor: s, head: r.end.max(e) };
                }
            }
            SelectMode::Line(l) => {
                let pl = self.buffer.line_of(pos);
                if pl < l {
                    let end = if l + 1 < self.buffer.line_count() { self.buffer.line_start(l + 1) } else { self.buffer.len() };
                    self.buffer.selection = Selection { anchor: end, head: self.buffer.line_start(pl) };
                } else {
                    let end = if pl + 1 < self.buffer.line_count() { self.buffer.line_start(pl + 1) } else { self.buffer.len() };
                    self.buffer.selection = Selection { anchor: self.buffer.line_start(l), head: end };
                }
            }
        }
        self.clamp_scroll();
        cx.notify();
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, cx: &mut Context<Self>) {
        self.selecting = None;
        self.scroll_drag = None;
        cx.notify();
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(layout) = &self.layout {
            if layout.completion_bounds.is_some_and(|b| b.contains(&event.position)) {
                let delta = event.delta.pixel_delta(px(self.line_height()));
                if let Some(c) = &mut self.completion {
                    c.scroll_top = (c.scroll_top - f32::from(delta.y)).max(0.0);
                }
                cx.notify();
                return;
            }
        }
        let delta = event.delta.pixel_delta(px(self.line_height()));
        self.scroll.x -= delta.x;
        self.scroll.y -= delta.y;
        self.clamp_scroll();
        cx.notify();
    }

    /// How big the document is against the viewport — what the scrollbars are
    /// drawn from, and what the scroll offset is clamped to.
    pub fn geom(&self) -> scroll::ScrollGeom {
        let Some(layout) = &self.layout else { return scroll::ScrollGeom::default() };
        let lh = self.line_height();
        let content_h = 24.0 + self.buffer.line_count() as f32 * lh;
        let content_w = f32::from(layout.gutter_width) + f32::from(layout.max_line_width) + 8.0;
        scroll::ScrollGeom::new(layout.bounds.size.width.into(), layout.bounds.size.height.into(), content_w, content_h)
    }

    fn clamp_scroll(&mut self) {
        if self.layout.is_none() {
            return;
        }
        let geom = self.geom();
        self.scroll.y = px(f32::from(self.scroll.y).clamp(0.0, geom.max_scroll_y()));
        self.scroll.x = px(f32::from(self.scroll.x).clamp(0.0, geom.max_scroll_x()));
    }

    /// A press on one of the editor's scrollbars: grab the thumb or page.
    fn on_scrollbar_press(&mut self, bar: scroll::Bar, along: f32, cx: &mut Context<Self>) {
        let geom = self.geom();
        let at = (f32::from(self.scroll.x), f32::from(self.scroll.y));
        match scroll::press(geom, bar, along, at) {
            scroll::Press::Grabbed(grab) => self.scroll_drag = Some(scroll::ScrollDrag { bar, grab }),
            scroll::Press::Paged(to) => match bar {
                scroll::Bar::Vertical => self.scroll.y = px(to),
                scroll::Bar::Horizontal => self.scroll.x = px(to),
            },
        }
        self.clamp_scroll();
        self.hovered_bar = Some(bar);
        cx.notify();
    }

    /// Body-local position of a window point, for scrollbar hit-testing.
    fn bar_local(&self, position: Point<Pixels>) -> Option<Point<Pixels>> {
        let layout = self.layout.as_ref()?;
        Some(point(position.x - layout.bounds.left(), position.y - layout.bounds.top()))
    }

    // UTF-16 helpers for the platform input handler.
    fn to_utf16(&self, offset: usize) -> usize {
        self.buffer.text()[..offset.min(self.buffer.len())].encode_utf16().count()
    }
    fn from_utf16(&self, offset: usize) -> usize {
        let mut count = 0;
        for (i, ch) in self.buffer.text().char_indices() {
            if count >= offset {
                return i;
            }
            count += ch.len_utf16();
        }
        self.buffer.len()
    }
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        self.from_utf16(r.start)..self.from_utf16(r.end)
    }
}

impl EntityInputHandler for SqlEditor {
    fn text_for_range(
        &mut self,
        range: Range<usize>,
        adjusted: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let r = self.range_from_utf16(&range);
        adjusted.replace(self.to_utf16(r.start)..self.to_utf16(r.end));
        Some(self.buffer.text()[r].to_string())
    }

    fn selected_text_range(&mut self, _: bool, _: &mut Window, _: &mut Context<Self>) -> Option<UTF16Selection> {
        let sel = self.buffer.selection;
        let r = sel.range();
        Some(UTF16Selection { range: self.to_utf16(r.start)..self.to_utf16(r.end), reversed: sel.head < sel.anchor })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|r| self.to_utf16(r.start)..self.to_utf16(r.end))
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(&mut self, range: Option<Range<usize>>, text: &str, _: &mut Window, cx: &mut Context<Self>) {
        let range = range.map(|r| self.range_from_utf16(&r)).or(self.marked_range.clone());
        self.marked_range = None;
        if let Some(r) = range {
            if r != self.buffer.selection.range() {
                self.buffer.selection = Selection { anchor: r.start, head: r.end };
            }
        }
        self.insert_text(text, cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range: Option<Range<usize>>,
        text: &str,
        new_selected: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range
            .map(|r| self.range_from_utf16(&r))
            .or(self.marked_range.clone())
            .unwrap_or(self.buffer.selection.range());
        self.buffer.edit(range.clone(), text, EditKind::Type);
        self.marked_range = if text.is_empty() { None } else { Some(range.start..range.start + text.len()) };
        if let Some(sel) = new_selected {
            let s = range.start + sel.start;
            let e = range.start + sel.end;
            self.buffer.selection = Selection { anchor: s.min(self.buffer.len()), head: e.min(self.buffer.len()) };
        }
        self.after_change(false, cx);
    }

    fn bounds_for_range(
        &mut self,
        range: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.layout.as_ref()?;
        let r = self.range_from_utf16(&range);
        Some(layout.bounds_for_offset(&self.buffer, r.start, self.scroll))
    }

    fn character_index_for_point(&mut self, point: Point<Pixels>, _: &mut Window, _: &mut Context<Self>) -> Option<usize> {
        let layout = self.layout.as_ref()?;
        let off = layout.offset_for_point(&self.buffer, point, self.scroll);
        Some(self.to_utf16(off))
    }
}

impl Render for SqlEditor {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused != self.was_focused {
            self.was_focused = focused;
            if focused {
                self.reset_blink(cx);
            } else {
                self.blink_task = None;
                // closeOnBlur
                self.completion = None;
            }
        }
        div()
            .id("sql-editor")
            .key_context(CONTEXT)
            .track_focus(&self.focus_handle)
            .size_full()
            .cursor(gpui::CursorStyle::IBeam)
            .on_action(cx.listener(Self::a_left))
            .on_action(cx.listener(Self::a_right))
            .on_action(cx.listener(Self::a_up))
            .on_action(cx.listener(Self::a_down))
            .on_action(cx.listener(Self::a_word_left))
            .on_action(cx.listener(Self::a_word_right))
            .on_action(cx.listener(Self::a_line_start))
            .on_action(cx.listener(Self::a_line_end))
            .on_action(cx.listener(Self::a_doc_start))
            .on_action(cx.listener(Self::a_doc_end))
            .on_action(cx.listener(Self::a_page_up))
            .on_action(cx.listener(Self::a_page_down))
            .on_action(cx.listener(Self::a_sel_left))
            .on_action(cx.listener(Self::a_sel_right))
            .on_action(cx.listener(Self::a_sel_up))
            .on_action(cx.listener(Self::a_sel_down))
            .on_action(cx.listener(Self::a_sel_word_left))
            .on_action(cx.listener(Self::a_sel_word_right))
            .on_action(cx.listener(Self::a_sel_line_start))
            .on_action(cx.listener(Self::a_sel_line_end))
            .on_action(cx.listener(Self::a_sel_doc_start))
            .on_action(cx.listener(Self::a_sel_doc_end))
            .on_action(cx.listener(Self::a_sel_page_up))
            .on_action(cx.listener(Self::a_sel_page_down))
            .on_action(cx.listener(Self::a_select_all))
            .on_action(cx.listener(Self::a_select_line))
            .on_action(cx.listener(Self::a_backspace))
            .on_action(cx.listener(Self::a_delete))
            .on_action(cx.listener(Self::a_delete_word_left))
            .on_action(cx.listener(Self::a_delete_word_right))
            .on_action(cx.listener(Self::a_delete_line_start))
            .on_action(cx.listener(Self::a_delete_line))
            .on_action(cx.listener(Self::a_newline))
            .on_action(cx.listener(Self::a_tab))
            .on_action(cx.listener(Self::a_shift_tab))
            .on_action(cx.listener(Self::a_indent_more))
            .on_action(cx.listener(Self::a_indent_less))
            .on_action(cx.listener(Self::a_toggle_comment))
            .on_action(cx.listener(Self::a_move_line_up))
            .on_action(cx.listener(Self::a_move_line_down))
            .on_action(cx.listener(Self::a_copy_line_up))
            .on_action(cx.listener(Self::a_copy_line_down))
            .on_action(cx.listener(Self::a_copy))
            .on_action(cx.listener(Self::a_cut))
            .on_action(cx.listener(Self::a_paste))
            .on_action(cx.listener(Self::a_undo))
            .on_action(cx.listener(Self::a_redo))
            .on_action(cx.listener(Self::a_run))
            .on_action(cx.listener(Self::a_start_completion))
            .on_action(cx.listener(Self::a_escape))
            .on_action(cx.listener(Self::a_matching_bracket))
            .on_action(cx.listener(Self::a_select_next))
            .on_action(cx.listener(Self::a_open_search))
            .on_action(cx.listener(Self::a_find_next))
            .on_action(cx.listener(Self::a_find_previous))
            .on_action(cx.listener(Self::a_select_selection_matches))
            .on_action(cx.listener(Self::a_goto_line))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_scroll_wheel(cx.listener(Self::on_scroll))
            .flex()
            .flex_col()
            .child(
                div()
                    .relative()
                    .flex_1()
                    .min_h(px(0.))
                    .w_full()
                    .child(element::EditorElement { editor: cx.entity() }),
            )
            .children(self.render_search_panels(window, cx))
            .children(popup::build(self, cx.entity(), window, cx))
            .children(self.hover_tooltip_data().map(|(messages, anchor, above)| {
                popup::lint_tooltip(&messages, anchor, above, self.font_scale, window, cx)
            }))
    }
}
