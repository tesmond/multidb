//! Single-line text field that renders like the old `<input>` elements
//! (flat box, 1px border that turns accent on focus) with full keyboard,
//! mouse, clipboard and IME support.

use crate::ui::theme::{self, hsla};
use gpui::{
    actions, div, fill, point, prelude::*, px, relative, size, AnyElement, App, Bounds, ClipboardItem, Context,
    CursorStyle,
    ElementId, ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable, FontWeight,
    GlobalElementId, KeyBinding, LayoutId, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, PaintQuad,
    Pixels, Point, Rgba, ShapedLine, SharedString, Style, Task, TextRun, UTF16Selection, UnderlineStyle, Window,
};
use std::ops::Range;
use std::time::Duration;
use unicode_segmentation::UnicodeSegmentation;

actions!(
    text_input,
    [
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteToStart,
        Left,
        Right,
        WordLeft,
        WordRight,
        SelectLeft,
        SelectRight,
        SelectWordLeft,
        SelectWordRight,
        SelectAll,
        Home,
        End,
        SelectToHome,
        SelectToEnd,
        Paste,
        Cut,
        Copy,
        Undo,
        Redo,
        Submit,
        Cancel,
        StepUp,
        StepDown,
    ]
);

pub const CONTEXT: &str = "TextInput";

pub fn bind_keys(cx: &mut App) {
    let c = Some(CONTEXT);
    cx.bind_keys([
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("alt-backspace", DeleteWordLeft, c),
        KeyBinding::new("cmd-backspace", DeleteToStart, c),
        KeyBinding::new("left", Left, c),
        KeyBinding::new("right", Right, c),
        KeyBinding::new("alt-left", WordLeft, c),
        KeyBinding::new("alt-right", WordRight, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("alt-shift-left", SelectWordLeft, c),
        KeyBinding::new("alt-shift-right", SelectWordRight, c),
        KeyBinding::new("cmd-a", SelectAll, c),
        KeyBinding::new("home", Home, c),
        KeyBinding::new("end", End, c),
        KeyBinding::new("cmd-left", Home, c),
        KeyBinding::new("cmd-right", End, c),
        KeyBinding::new("cmd-shift-left", SelectToHome, c),
        KeyBinding::new("cmd-shift-right", SelectToEnd, c),
        KeyBinding::new("shift-home", SelectToHome, c),
        KeyBinding::new("shift-end", SelectToEnd, c),
        KeyBinding::new("cmd-v", Paste, c),
        KeyBinding::new("cmd-c", Copy, c),
        KeyBinding::new("cmd-x", Cut, c),
        KeyBinding::new("cmd-z", Undo, c),
        KeyBinding::new("cmd-shift-z", Redo, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("escape", Cancel, c),
        KeyBinding::new("up", StepUp, c),
        KeyBinding::new("down", StepDown, c),
    ]);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InputEvent {
    Changed,
    Submit,
    Cancel,
    Focus,
    Blur,
}

impl EventEmitter<InputEvent> for TextInput {}

/// Box appearance for the field.
#[derive(Clone)]
pub struct InputLook {
    pub font_size: f32,
    pub font_family: SharedString,
    pub font_weight: FontWeight,
    pub text: Rgba,
    pub placeholder: Rgba,
    pub bg: Rgba,
    pub focus_bg: Option<Rgba>,
    pub border: Rgba,
    pub focus_border: Rgba,
    pub border_width: f32,
    pub radius: f32,
    pub pad_x: (f32, f32),
    pub height: f32,
    pub line_height: f32,
    /// Extra focus ring (`box-shadow: 0 0 0 Npx color`).
    pub focus_ring: Option<(f32, Rgba)>,
}

impl InputLook {
    /// `input { padding: 7px 10px; font-size: 13px; border: 1px solid; border-radius: 4px }`
    pub fn dialog(scale: f32) -> Self {
        let fs = 13.0 * scale;
        let lh = crate::ui::metrics::line_height_normal(fs);
        InputLook {
            font_size: fs,
            font_family: theme::UI_FONT.into(),
            font_weight: FontWeight::NORMAL,
            text: theme::TEXT,
            placeholder: crate::ui::metrics::PLACEHOLDER,
            bg: theme::BG_INPUT,
            focus_bg: None,
            border: theme::BORDER,
            focus_border: theme::ACCENT,
            border_width: 1.0,
            radius: 4.0,
            pad_x: (10.0, 10.0),
            height: crate::ui::metrics::input_inner_height(fs) + 14.0 + 2.0,
            line_height: lh,
            focus_ring: None,
        }
    }
}

pub struct TextInput {
    focus_handle: FocusHandle,
    content: String,
    placeholder: SharedString,
    selected_range: Range<usize>,
    selection_reversed: bool,
    marked_range: Option<Range<usize>>,
    last_layout: Option<ShapedLine>,
    last_bounds: Option<Bounds<Pixels>>,
    is_selecting: bool,
    scroll_x: Pixels,
    pub look: InputLook,
    pub masked: bool,
    pub readonly: bool,
    /// `type="number"`: Up/Down step the value.
    pub number: Option<(f64, f64, f64)>,
    /// Position in the Tab order, for dialogs. `None` keeps it out of it.
    pub tab_index: Option<isize>,
    undo: Vec<(String, Range<usize>)>,
    redo: Vec<(String, Range<usize>)>,
    cursor_visible: bool,
    blink_task: Option<Task<()>>,
    was_focused: bool,
}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl TextInput {
    pub fn new(cx: &mut Context<Self>, look: InputLook) -> Self {
        let focus_handle = cx.focus_handle();
        TextInput {
            focus_handle,
            content: String::new(),
            placeholder: SharedString::default(),
            selected_range: 0..0,
            selection_reversed: false,
            marked_range: None,
            last_layout: None,
            last_bounds: None,
            is_selecting: false,
            scroll_x: px(0.),
            look,
            masked: false,
            readonly: false,
            number: None,
            tab_index: None,
            undo: Vec::new(),
            redo: Vec::new(),
            cursor_visible: true,
            blink_task: None,
            was_focused: false,
        }
    }

    /// Make this input a tab stop at `index`, so Tab walks a dialog's fields
    /// in order. Inputs without one are skipped by Tab entirely.
    pub fn with_tab_index(mut self, index: isize) -> Self {
        self.tab_index = Some(index);
        self
    }

    /// The focus handle carrying this input's place in the tab order. gpui
    /// takes `tab_stop` from the handle, not from the element, and ignores an
    /// element's `tab_index` entirely when it tracks a handle of its own.
    fn tab_focus_handle(&self, cx: &App) -> FocusHandle {
        let handle = self.focus_handle(cx);
        match self.tab_index {
            Some(i) => handle.tab_stop(true).tab_index(i),
            None => handle,
        }
    }

    pub fn with_placeholder(mut self, p: impl Into<SharedString>) -> Self {
        self.placeholder = p.into();
        self
    }

    pub fn set_placeholder(&mut self, p: impl Into<SharedString>) {
        self.placeholder = p.into();
    }

    pub fn text(&self) -> &str {
        &self.content
    }

    pub fn set_text(&mut self, text: impl Into<String>, cx: &mut Context<Self>) {
        let text = text.into();
        if text != self.content {
            self.content = text;
            let end = self.content.len();
            self.selected_range = end..end;
            self.selection_reversed = false;
            self.marked_range = None;
            self.undo.clear();
            self.redo.clear();
            cx.notify();
        }
    }

    pub fn select_all_text(&mut self, cx: &mut Context<Self>) {
        self.selected_range = 0..self.content.len();
        self.selection_reversed = false;
        cx.notify();
    }

    pub fn is_focused(&self, window: &Window) -> bool {
        self.focus_handle.is_focused(window)
    }

    pub fn focus(&self, window: &mut Window) {
        window.focus(&self.focus_handle);
    }

    fn display_text(&self) -> String {
        if self.masked {
            "•".repeat(self.content.chars().count())
        } else {
            self.content.clone()
        }
    }

    fn display_offset(&self, offset: usize) -> usize {
        if self.masked {
            self.content[..offset].chars().count() * "•".len()
        } else {
            offset
        }
    }

    fn content_offset(&self, display: usize) -> usize {
        if self.masked {
            let n = display / "•".len();
            self.content.char_indices().nth(n).map(|(i, _)| i).unwrap_or(self.content.len())
        } else {
            display
        }
    }

    fn push_undo(&mut self) {
        self.undo.push((self.content.clone(), self.selected_range.clone()));
        if self.undo.len() > 200 {
            self.undo.remove(0);
        }
        self.redo.clear();
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.reset_blink(cx);
        cx.emit(InputEvent::Changed);
        cx.notify();
    }

    fn reset_blink(&mut self, cx: &mut Context<Self>) {
        self.cursor_visible = true;
        self.blink_task = Some(cx.spawn(async move |this, cx| loop {
            cx.background_executor().timer(Duration::from_millis(530)).await;
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

    fn left(&mut self, _: &Left, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.previous_boundary(self.cursor_offset()), cx);
        } else {
            self.move_to(self.selected_range.start, cx)
        }
    }

    fn right(&mut self, _: &Right, _: &mut Window, cx: &mut Context<Self>) {
        if self.selected_range.is_empty() {
            self.move_to(self.next_boundary(self.selected_range.end), cx);
        } else {
            self.move_to(self.selected_range.end, cx)
        }
    }

    fn word_left(&mut self, _: &WordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn word_right(&mut self, _: &WordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.next_word(self.cursor_offset()), cx);
    }

    fn select_left(&mut self, _: &SelectLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_boundary(self.cursor_offset()), cx);
    }

    fn select_right(&mut self, _: &SelectRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_boundary(self.cursor_offset()), cx);
    }

    fn select_word_left(&mut self, _: &SelectWordLeft, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.previous_word(self.cursor_offset()), cx);
    }

    fn select_word_right(&mut self, _: &SelectWordRight, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.next_word(self.cursor_offset()), cx);
    }

    fn select_all(&mut self, _: &SelectAll, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
        self.select_to(self.content.len(), cx)
    }

    fn home(&mut self, _: &Home, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(0, cx);
    }

    fn end(&mut self, _: &End, _: &mut Window, cx: &mut Context<Self>) {
        self.move_to(self.content.len(), cx);
    }

    fn select_to_home(&mut self, _: &SelectToHome, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(0, cx);
    }

    fn select_to_end(&mut self, _: &SelectToEnd, _: &mut Window, cx: &mut Context<Self>) {
        self.select_to(self.content.len(), cx);
    }

    fn backspace(&mut self, _: &Backspace, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete(&mut self, _: &Delete, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.next_boundary(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_word_left(&mut self, _: &DeleteWordLeft, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(self.previous_word(self.cursor_offset()), cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn delete_to_start(&mut self, _: &DeleteToStart, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        if self.selected_range.is_empty() {
            self.select_to(0, cx)
        }
        self.replace_text_in_range(None, "", window, cx)
    }

    fn paste(&mut self, _: &Paste, window: &mut Window, cx: &mut Context<Self>) {
        if self.readonly {
            return;
        }
        if let Some(text) = cx.read_from_clipboard().and_then(|item| item.text()) {
            self.replace_text_in_range(None, &text.replace(['\n', '\r'], " "), window, cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
        }
    }

    fn cut(&mut self, _: &Cut, window: &mut Window, cx: &mut Context<Self>) {
        if !self.selected_range.is_empty() && !self.masked && !self.readonly {
            cx.write_to_clipboard(ClipboardItem::new_string(self.content[self.selected_range.clone()].to_string()));
            self.replace_text_in_range(None, "", window, cx)
        }
    }

    fn undo(&mut self, _: &Undo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, sel)) = self.undo.pop() {
            self.redo.push((self.content.clone(), self.selected_range.clone()));
            self.content = text;
            self.selected_range = sel;
            self.changed(cx);
        }
    }

    fn redo(&mut self, _: &Redo, _: &mut Window, cx: &mut Context<Self>) {
        if let Some((text, sel)) = self.redo.pop() {
            self.undo.push((self.content.clone(), self.selected_range.clone()));
            self.content = text;
            self.selected_range = sel;
            self.changed(cx);
        }
    }

    fn submit(&mut self, _: &Submit, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Submit);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(InputEvent::Cancel);
    }

    fn step(&mut self, delta: f64, cx: &mut Context<Self>) {
        let Some((min, max, step)) = self.number else { return };
        let current: f64 = self.content.trim().parse().unwrap_or(0.0);
        let next = ((current + delta * step) / step).round() * step;
        let next = next.clamp(min, max);
        self.push_undo();
        self.content = crate::ui::js::number_to_string(next);
        let end = self.content.len();
        self.selected_range = end..end;
        self.changed(cx);
    }

    fn step_up(&mut self, _: &StepUp, _: &mut Window, cx: &mut Context<Self>) {
        self.step(1.0, cx);
    }

    fn step_down(&mut self, _: &StepDown, _: &mut Window, cx: &mut Context<Self>) {
        self.step(-1.0, cx);
    }

    fn on_mouse_down(&mut self, event: &MouseDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        window.focus(&self.focus_handle);
        self.is_selecting = true;
        let idx = self.index_for_mouse_position(event.position);
        if event.click_count >= 3 {
            self.selected_range = 0..self.content.len();
            self.selection_reversed = false;
            cx.notify();
        } else if event.click_count == 2 {
            let start = self.previous_word(idx.min(self.content.len()).max(0));
            let start = if self.is_word_start(idx) { idx } else { start };
            let end = self.next_word(start);
            self.selected_range = start..end;
            self.selection_reversed = false;
            cx.notify();
        } else if event.modifiers.shift {
            self.select_to(idx, cx);
        } else {
            self.move_to(idx, cx)
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _window: &mut Window, _: &mut Context<Self>) {
        self.is_selecting = false;
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if self.is_selecting {
            self.select_to(self.index_for_mouse_position(event.position), cx);
        }
    }

    fn move_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.selected_range = offset..offset;
        self.selection_reversed = false;
        self.reset_blink(cx);
        cx.notify()
    }

    fn cursor_offset(&self) -> usize {
        if self.selection_reversed {
            self.selected_range.start
        } else {
            self.selected_range.end
        }
    }

    fn index_for_mouse_position(&self, position: Point<Pixels>) -> usize {
        if self.content.is_empty() {
            return 0;
        }
        let (Some(bounds), Some(line)) = (self.last_bounds.as_ref(), self.last_layout.as_ref()) else {
            return 0;
        };
        let x = position.x - bounds.left() + self.scroll_x;
        self.content_offset(line.closest_index_for_x(x))
    }

    fn select_to(&mut self, offset: usize, cx: &mut Context<Self>) {
        if self.selection_reversed {
            self.selected_range.start = offset
        } else {
            self.selected_range.end = offset
        };
        if self.selected_range.end < self.selected_range.start {
            self.selection_reversed = !self.selection_reversed;
            self.selected_range = self.selected_range.end..self.selected_range.start;
        }
        self.reset_blink(cx);
        cx.notify()
    }

    fn offset_from_utf16(&self, offset: usize) -> usize {
        let mut utf8_offset = 0;
        let mut utf16_count = 0;
        for ch in self.content.chars() {
            if utf16_count >= offset {
                break;
            }
            utf16_count += ch.len_utf16();
            utf8_offset += ch.len_utf8();
        }
        utf8_offset
    }

    fn offset_to_utf16(&self, offset: usize) -> usize {
        let mut utf16_offset = 0;
        let mut utf8_count = 0;
        for ch in self.content.chars() {
            if utf8_count >= offset {
                break;
            }
            utf8_count += ch.len_utf8();
            utf16_offset += ch.len_utf16();
        }
        utf16_offset
    }

    fn range_to_utf16(&self, range: &Range<usize>) -> Range<usize> {
        self.offset_to_utf16(range.start)..self.offset_to_utf16(range.end)
    }

    fn range_from_utf16(&self, range_utf16: &Range<usize>) -> Range<usize> {
        self.offset_from_utf16(range_utf16.start)..self.offset_from_utf16(range_utf16.end)
    }

    fn previous_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .rev()
            .find_map(|(idx, _)| (idx < offset).then_some(idx))
            .unwrap_or(0)
    }

    fn next_boundary(&self, offset: usize) -> usize {
        self.content
            .grapheme_indices(true)
            .find_map(|(idx, _)| (idx > offset).then_some(idx))
            .unwrap_or(self.content.len())
    }

    fn is_word_start(&self, offset: usize) -> bool {
        self.content.split_word_bound_indices().any(|(i, w)| i == offset && w.chars().any(|c| c.is_alphanumeric()))
    }

    fn previous_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .rev()
            .find(|(i, w)| *i < offset && w.chars().any(|c| c.is_alphanumeric() || c == '_'))
            .map(|(i, _)| i)
            .unwrap_or(0)
    }

    fn next_word(&self, offset: usize) -> usize {
        self.content
            .split_word_bound_indices()
            .find(|(i, w)| i + w.len() > offset && w.chars().any(|c| c.is_alphanumeric() || c == '_'))
            .map(|(i, w)| i + w.len())
            .unwrap_or(self.content.len())
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.content[range].to_string())
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        Some(UTF16Selection { range: self.range_to_utf16(&self.selected_range), reversed: self.selection_reversed })
    }

    fn marked_text_range(&self, _window: &mut Window, _cx: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked_range.as_ref().map(|range| self.range_to_utf16(range))
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        self.marked_range = None;
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.readonly {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        let new_text = new_text.replace(['\n', '\r'], "");
        self.push_undo();
        self.content = self.content[0..range.start].to_owned() + &new_text + &self.content[range.end..];
        self.selected_range = range.start + new_text.len()..range.start + new_text.len();
        self.selection_reversed = false;
        self.marked_range.take();
        self.changed(cx);
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.readonly {
            return;
        }
        let range = range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .or(self.marked_range.clone())
            .unwrap_or(self.selected_range.clone());
        self.content = self.content[0..range.start].to_owned() + new_text + &self.content[range.end..];
        self.marked_range = if new_text.is_empty() { None } else { Some(range.start..range.start + new_text.len()) };
        self.selected_range = new_selected_range_utf16
            .as_ref()
            .map(|r| self.range_from_utf16(r))
            .map(|r| r.start + range.start..r.end + range.start)
            .unwrap_or_else(|| range.start + new_text.len()..range.start + new_text.len());
        self.changed(cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let layout = self.last_layout.as_ref()?;
        let range = self.range_from_utf16(&range_utf16);
        let start = self.display_offset(range.start);
        let end = self.display_offset(range.end);
        Some(Bounds::from_corners(
            point(bounds.left() + layout.x_for_index(start) - self.scroll_x, bounds.top()),
            point(bounds.left() + layout.x_for_index(end) - self.scroll_x, bounds.bottom()),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        let bounds = self.last_bounds?;
        let layout = self.last_layout.as_ref()?;
        let idx = layout.closest_index_for_x(point.x - bounds.left() + self.scroll_x);
        Some(self.offset_to_utf16(self.content_offset(idx)))
    }
}

struct TextElement {
    input: Entity<TextInput>,
}

struct PrepaintState {
    line: Option<ShapedLine>,
    cursor: Option<PaintQuad>,
    selection: Option<PaintQuad>,
    scroll_x: Pixels,
}

impl IntoElement for TextElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for TextElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, Self::RequestLayoutState) {
        let look = &self.input.read(cx).look;
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = px(look.line_height).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        window: &mut Window,
        cx: &mut App,
    ) -> Self::PrepaintState {
        let input = self.input.read(cx);
        let look = &input.look;
        let display = input.display_text();
        let focused = input.focus_handle.is_focused(window);
        let (text, color) = if display.is_empty() {
            (input.placeholder.to_string(), look.placeholder)
        } else {
            (display, look.text)
        };
        let font = gpui::Font {
            family: look.font_family.clone(),
            weight: look.font_weight,
            ..gpui::font(look.font_family.clone())
        };
        let run = TextRun { len: text.len(), font, color: hsla(color), background_color: None, underline: None, strikethrough: None };
        let runs = match (&input.marked_range, input.content.is_empty()) {
            (Some(marked), false) if !input.masked => vec![
                TextRun { len: marked.start, ..run.clone() },
                TextRun {
                    len: marked.end - marked.start,
                    underline: Some(UnderlineStyle { color: Some(run.color), thickness: px(1.0), wavy: false }),
                    ..run.clone()
                },
                TextRun { len: text.len() - marked.end, ..run.clone() },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            _ => vec![run.clone()],
        };
        // WebKit repaints selected text in the system's highlight-text colour,
        // which is what keeps it readable against the pale selection; without
        // that the light input text all but disappears.
        let selected_display = {
            let r = input.selected_range.clone();
            (input.display_offset(r.start), input.display_offset(r.end))
        };
        let runs = if !input.content.is_empty() && selected_display.0 < selected_display.1 && text.len() >= selected_display.1 {
            let (a, b) = selected_display;
            vec![
                TextRun { len: a, ..run.clone() },
                TextRun { len: b - a, color: hsla(crate::ui::metrics::SELECTION_TEXT), ..run.clone() },
                TextRun { len: text.len() - b, ..run },
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect()
        } else {
            runs
        };
        let line = window.text_system().shape_line(text.into(), px(look.font_size), &runs, None);

        let cursor_display = input.display_offset(input.cursor_offset());
        let cursor_x = if input.content.is_empty() { px(0.) } else { line.x_for_index(cursor_display) };
        // Keep the caret visible by scrolling horizontally.
        let mut scroll_x = input.scroll_x;
        let width = bounds.size.width;
        if cursor_x - scroll_x > width - px(1.) {
            scroll_x = cursor_x - width + px(1.);
        }
        if cursor_x < scroll_x {
            scroll_x = cursor_x;
        }
        if line.width <= width {
            scroll_x = px(0.);
        }
        let selected = input.selected_range.clone();
        let (selection, cursor) = if selected.is_empty() || input.content.is_empty() {
            let cursor = (focused && input.cursor_visible && !input.readonly).then(|| {
                fill(
                    Bounds::new(point(bounds.left() + cursor_x - scroll_x, bounds.top()), size(px(1.), bounds.size.height)),
                    hsla(look.text),
                )
            });
            (None, cursor)
        } else {
            let start = line.x_for_index(input.display_offset(selected.start));
            let end = line.x_for_index(input.display_offset(selected.end));
            let color = if focused { crate::ui::metrics::SELECTION_FOCUSED } else { crate::ui::metrics::SELECTION_UNFOCUSED };
            (
                Some(fill(
                    Bounds::from_corners(
                        point(bounds.left() + start - scroll_x, bounds.top()),
                        point(bounds.left() + end - scroll_x, bounds.bottom()),
                    ),
                    hsla(color),
                )),
                None,
            )
        };
        PrepaintState { line: Some(line), cursor, selection, scroll_x }
    }

    fn paint(
        &mut self,
        _id: Option<&GlobalElementId>,
        _inspector_id: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _request_layout: &mut Self::RequestLayoutState,
        prepaint: &mut Self::PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus_handle = self.input.read(cx).focus_handle.clone();
        let line_height = px(self.input.read(cx).look.line_height);
        window.handle_input(&focus_handle, ElementInputHandler::new(bounds, self.input.clone()), cx);
        window.with_content_mask(Some(gpui::ContentMask { bounds }), |window| {
            if let Some(selection) = prepaint.selection.take() {
                window.paint_quad(selection)
            }
            let line = prepaint.line.take().unwrap();
            let origin = point(bounds.origin.x - prepaint.scroll_x, bounds.origin.y);
            line.paint(origin, line_height, window, cx).ok();
            if let Some(cursor) = prepaint.cursor.take() {
                window.paint_quad(cursor);
            }
            let scroll_x = prepaint.scroll_x;
            self.input.update(cx, |input, _cx| {
                input.last_layout = Some(line);
                input.last_bounds = Some(bounds);
                input.scroll_x = scroll_x;
            });
        });
    }
}

impl Render for TextInput {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let focused = self.focus_handle.is_focused(window);
        if focused != self.was_focused {
            self.was_focused = focused;
            if focused {
                self.reset_blink(cx);
                cx.emit(InputEvent::Focus);
            } else {
                self.blink_task = None;
                self.is_selecting = false;
                cx.emit(InputEvent::Blur);
            }
        }
        let look = self.look.clone();
        let mut outer = div()
            .flex()
            .items_center()
            .w_full()
            .h(px(look.height))
            .min_w(px(0.))
            .px(px(0.))
            .pl(px(look.pad_x.0))
            .pr(px(look.pad_x.1))
            .rounded(px(look.radius))
            .bg(if focused { look.focus_bg.unwrap_or(look.bg) } else { look.bg })
            .border(px(look.border_width))
            .border_color(if focused { look.focus_border } else { look.border })
            .overflow_hidden()
            .key_context(CONTEXT)
            .track_focus(&self.tab_focus_handle(cx))
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::delete))
            .on_action(cx.listener(Self::delete_word_left))
            .on_action(cx.listener(Self::delete_to_start))
            .on_action(cx.listener(Self::left))
            .on_action(cx.listener(Self::right))
            .on_action(cx.listener(Self::word_left))
            .on_action(cx.listener(Self::word_right))
            .on_action(cx.listener(Self::select_left))
            .on_action(cx.listener(Self::select_right))
            .on_action(cx.listener(Self::select_word_left))
            .on_action(cx.listener(Self::select_word_right))
            .on_action(cx.listener(Self::select_all))
            .on_action(cx.listener(Self::home))
            .on_action(cx.listener(Self::end))
            .on_action(cx.listener(Self::select_to_home))
            .on_action(cx.listener(Self::select_to_end))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::undo))
            .on_action(cx.listener(Self::redo))
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::step_up))
            .on_action(cx.listener(Self::step_down))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_move(cx.listener(Self::on_mouse_move));
        if let (true, Some((w, color))) = (focused, look.focus_ring) {
            outer = outer.shadow(vec![gpui::BoxShadow {
                color: hsla(color),
                offset: point(px(0.), px(0.)),
                blur_radius: px(0.),
                spread_radius: px(w),
            }]);
        }
        let stepper = self.number.is_some().then(|| spin_button(self.look.font_size));
        outer.child(TextElement { input: cx.entity() }).children(stepper)
    }
}

/// WebKit's `::-webkit-inner-spin-button`: a narrow control at the inner right
/// edge of a number input with a stacked pair of arrows.
fn spin_button(font_size: f32) -> AnyElement {
    let arrow = |up: bool| {
        div()
            .w(px(0.62 * font_size))
            .h(px(0.42 * font_size))
            .flex()
            .items_center()
            .justify_center()
            .text_size(px(0.5 * font_size))
            .line_height(px(0.42 * font_size))
            .text_color(theme::hex(0x6b6b6b))
            .child(if up { "▲" } else { "▼" })
    };
    div()
        .flex_shrink_0()
        .ml(px(0.3 * font_size))
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(1.))
        .child(arrow(true))
        .child(arrow(false))
        .into_any_element()
}
