//! Layout + painting for the SQL editor (gutters, highlighted lines,
//! selection, cursor, lint decorations). Geometry mirrors CodeMirror's
//! One Dark setup from the old UI.

use super::buffer::Buffer;
use super::SqlEditor;
use crate::ui::sql::{Tok, Token};
use crate::ui::theme::{hsla, one_dark};
use gpui::{
    fill, point, px, relative, size, App, Bounds, ContentMask, Element, ElementId, Entity, Font, FontStyle, FontWeight,
    GlobalElementId, Hsla, IntoElement, LayoutId, PathBuilder, Pixels, Point, ShapedLine, SharedString, Style, TextRun,
    Window,
};

pub const CONTENT_PAD_Y: f32 = 12.0;
pub const LINE_PAD_LEFT: f32 = 6.0;
pub const LINE_PAD_RIGHT: f32 = 2.0;

/// Geometry captured during paint for hit-testing and overlays.
#[derive(Clone)]
pub struct EditorLayout {
    pub bounds: Bounds<Pixels>,
    pub gutter_width: Pixels,
    pub line_height: f32,
    pub font_size: f32,
    pub char_width: Pixels,
    pub text_box_height: f32,
    pub max_line_width: Pixels,
    /// (line index, shaped line) for lines painted this frame.
    pub lines: Vec<(usize, ShapedLine)>,
    pub completion_bounds: Option<Bounds<Pixels>>,
    pub completion_rows: Vec<(usize, Bounds<Pixels>)>,
}

impl EditorLayout {
    fn text_left(&self) -> Pixels {
        self.bounds.left() + self.gutter_width + px(LINE_PAD_LEFT)
    }

    fn shaped(&self, line: usize) -> Option<&ShapedLine> {
        self.lines.iter().find(|(l, _)| *l == line).map(|(_, s)| s)
    }

    /// x of `offset` relative to the start of the text (excluding scroll).
    pub fn x_for_offset(&self, buffer: &Buffer, offset: usize) -> Pixels {
        let line = buffer.line_of(offset);
        let col_bytes = offset - buffer.line_start(line);
        match self.shaped(line) {
            Some(s) => s.x_for_index(col_bytes),
            None => self.char_width * buffer.column_chars(offset) as f32,
        }
    }

    pub fn offset_for_point(&self, buffer: &Buffer, p: Point<Pixels>, scroll: Point<Pixels>) -> usize {
        let y = f32::from(p.y - self.bounds.top() + scroll.y) - CONTENT_PAD_Y;
        let line = if y < 0.0 { 0 } else { (y / self.line_height).floor() as usize };
        if line >= buffer.line_count() {
            return buffer.len();
        }
        let x = p.x - self.text_left() + scroll.x;
        let ls = buffer.line_start(line);
        match self.shaped(line) {
            Some(s) => ls + s.closest_index_for_x(x).min(buffer.line_text(line).len()),
            None => {
                let col = (f32::from(x) / f32::from(self.char_width)).round().max(0.0) as usize;
                buffer.offset_at_column(line, col)
            }
        }
    }

    /// Screen rect of the character box at `offset` (like `coordsAtPos`).
    pub fn bounds_for_offset(&self, buffer: &Buffer, offset: usize, scroll: Point<Pixels>) -> Bounds<Pixels> {
        let line = buffer.line_of(offset);
        let top = self.bounds.top() + px(CONTENT_PAD_Y + line as f32 * self.line_height) - scroll.y;
        let pad = (self.line_height - self.text_box_height) / 2.0;
        let x = self.text_left() + self.x_for_offset(buffer, offset) - scroll.x;
        Bounds::new(point(x, top + px(pad)), size(px(0.), px(self.text_box_height)))
    }

    pub fn completion_hit(&self, p: Point<Pixels>) -> Option<usize> {
        if !self.completion_bounds.is_some_and(|b| b.contains(&p)) {
            return None;
        }
        self.completion_rows.iter().find(|(_, b)| b.contains(&p)).map(|(i, _)| *i)
    }
}

pub struct EditorElement {
    pub editor: Entity<SqlEditor>,
}

impl IntoElement for EditorElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

pub fn token_color(kind: Tok) -> Hsla {
    use one_dark::*;
    hsla(match kind {
        Tok::Keyword => VIOLET,
        Tok::Type | Tok::Number | Tok::Bits => CHALKY,
        Tok::Builtin | Tok::Bool => WHISKEY,
        Tok::String | Tok::Bytes => SAGE,
        Tok::Identifier | Tok::SpecialVar => CORAL,
        Tok::QuotedIdentifier | Tok::Operator => CYAN,
        Tok::LineComment | Tok::BlockComment => STONE,
        _ => IVORY,
    })
}

pub fn mono_font(family: &SharedString) -> Font {
    Font {
        family: family.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: FontStyle::Normal,
    }
}

fn line_runs(buffer: &Buffer, tokens: &[Token], line: usize, font: &Font) -> Vec<TextRun> {
    let ls = buffer.line_start(line);
    let le = buffer.line_end(line);
    let mut runs: Vec<TextRun> = Vec::new();
    let mut pos = ls;
    let push = |runs: &mut Vec<TextRun>, len: usize, color: Hsla| {
        if len == 0 {
            return;
        }
        if let Some(last) = runs.last_mut() {
            if last.color == color {
                last.len += len;
                return;
            }
        }
        runs.push(TextRun { len, font: font.clone(), color, background_color: None, underline: None, strikethrough: None });
    };
    let start_idx = tokens.partition_point(|t| t.to <= ls);
    for t in &tokens[start_idx..] {
        if t.from >= le {
            break;
        }
        let from = t.from.max(ls);
        let to = t.to.min(le);
        if from > pos {
            push(&mut runs, from - pos, hsla(one_dark::IVORY));
        }
        push(&mut runs, to - from, token_color(t.kind));
        pos = to;
    }
    if le > pos {
        push(&mut runs, le - pos, hsla(one_dark::IVORY));
    }
    runs
}

pub struct Prepaint {
    lines: Vec<(usize, ShapedLine)>,
    numbers: Vec<(usize, ShapedLine)>,
    placeholder: Option<ShapedLine>,
    layout: EditorLayout,
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = Prepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> Prepaint {
        let editor = self.editor.read(cx);
        let scale = editor.font_scale;
        let font_size = 13.0 * scale;
        let lh = font_size * 1.6;
        let font = mono_font(&editor.mono_family);
        let ts = window.text_system().clone();
        let font_id = ts.resolve_font(&font);
        let char_width = ts.advance(font_id, px(font_size), '0').map(|s| s.width).unwrap_or(px(font_size * 0.6));
        let ascent: f32 = ts.ascent(font_id, px(font_size)).into();
        let descent: f32 = f32::from(ts.descent(font_id, px(font_size))).abs();
        let text_box_height = ascent.round() + descent.round();

        let buffer = &editor.buffer;
        let line_count = buffer.line_count();
        // Gutter: line numbers (padding 0 3px 0 5px, min-width 20px) + lint (1.4em).
        let digits = line_count.to_string();
        let digits_w = char_width * digits.chars().count() as f32;
        let numbers_width = (digits_w + px(8.)).max(px(20.));
        let lint_width = px(1.4 * font_size);
        let gutter_width = numbers_width + lint_width;

        let scroll = editor.scroll;
        let first = ((f32::from(scroll.y) - CONTENT_PAD_Y) / lh).floor().max(0.0) as usize;
        let visible = (f32::from(bounds.size.height) / lh).ceil() as usize + 2;
        let last = (first + visible).min(line_count);

        let mut lines = Vec::new();
        let mut numbers = Vec::new();
        let mut max_line_width = editor.layout.as_ref().map(|l| l.max_line_width).unwrap_or(px(0.));
        for line in first..last {
            let text = buffer.line_text(line).to_string();
            let runs = line_runs(buffer, &editor.tokens, line, &font);
            let shaped = ts.shape_line(text.into(), px(font_size), &runs, None);
            max_line_width = max_line_width.max(shaped.width);
            lines.push((line, shaped));
            let n = (line + 1).to_string();
            let run = TextRun {
                len: n.len(),
                font: font.clone(),
                color: hsla(one_dark::STONE),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            numbers.push((line, ts.shape_line(n.into(), px(font_size), &[run], None)));
        }
        let placeholder = buffer.text().is_empty().then(|| {
            let text: SharedString = editor.placeholder.clone();
            let run = TextRun {
                len: text.len(),
                font: font.clone(),
                color: hsla(one_dark::PLACEHOLDER),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            ts.shape_line(text, px(font_size), &[run], None)
        });
        let layout = EditorLayout {
            bounds,
            gutter_width,
            line_height: lh,
            font_size,
            char_width,
            text_box_height,
            max_line_width,
            lines: lines.clone(),
            completion_bounds: editor.layout.as_ref().and_then(|l| l.completion_bounds),
            completion_rows: editor.layout.as_ref().map(|l| l.completion_rows.clone()).unwrap_or_default(),
        };
        Prepaint { lines, numbers, placeholder, layout }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        pp: &mut Prepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.editor.read(cx).focus_handle.clone();
        let focused = focus.is_focused(window);
        window.handle_input(&focus, gpui::ElementInputHandler::new(bounds, self.editor.clone()), cx);

        let editor = self.editor.read(cx);
        let buffer = &editor.buffer;
        let layout = &pp.layout;
        let lh = layout.line_height;
        let scroll = editor.scroll;
        let gutter_w = layout.gutter_width;
        let text_left = bounds.left() + gutter_w + px(LINE_PAD_LEFT) - scroll.x;
        let content_right = bounds.right();
        let line_top = |line: usize| bounds.top() + px(CONTENT_PAD_Y + line as f32 * lh) - scroll.y;
        let sel = buffer.selection;
        let sel_range = sel.range();
        let head_line = buffer.line_of(sel.head);
        let box_pad = (lh - layout.text_box_height) / 2.0;

        window.paint_quad(fill(bounds, hsla(one_dark::BACKGROUND)));

        let text_area = Bounds::from_corners(point(bounds.left() + gutter_w, bounds.top()), bounds.bottom_right());
        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            // Active line.
            window.paint_quad(fill(
                Bounds::from_corners(point(bounds.left() + gutter_w, line_top(head_line)), point(content_right, line_top(head_line) + px(lh))),
                hsla(one_dark::ACTIVE_LINE),
            ));

            // Selection-match highlights (highlightSelectionMatches).
            if !sel.is_empty() && sel_range.len() <= 200 {
                let needle = &buffer.text()[sel_range.clone()];
                if !needle.trim().is_empty() && !needle.contains('\n') {
                    for (line, shaped) in &pp.lines {
                        let ls = buffer.line_start(*line);
                        let text = buffer.line_text(*line);
                        let mut from = 0;
                        while let Some(i) = text[from..].find(needle) {
                            let s = ls + from + i;
                            if s != sel_range.start {
                                let x0 = shaped.x_for_index(from + i);
                                let x1 = shaped.x_for_index(from + i + needle.len());
                                let top = line_top(*line) + px(box_pad);
                                window.paint_quad(fill(
                                    Bounds::from_corners(point(text_left + x0, top), point(text_left + x1, top + px(layout.text_box_height))),
                                    hsla(one_dark::SELECTION_MATCH),
                                ));
                            }
                            from += i + needle.len().max(1);
                        }
                    }
                }
            }

            // Selection (full line-height rectangles, like CodeMirror's layer).
            if !sel.is_empty() {
                let l0 = buffer.line_of(sel_range.start);
                let l1 = buffer.line_of(sel_range.end);
                let left_side = bounds.left() + gutter_w + px(LINE_PAD_LEFT) - scroll.x;
                let right_side = content_right.max(text_left + layout.max_line_width + px(LINE_PAD_RIGHT)) - px(LINE_PAD_RIGHT);
                for (line, shaped) in &pp.lines {
                    let line = *line;
                    if line < l0 || line > l1 {
                        continue;
                    }
                    let ls = buffer.line_start(line);
                    let x0 = if line == l0 { text_left + shaped.x_for_index(sel_range.start - ls) } else { left_side };
                    let x1 = if line == l1 { text_left + shaped.x_for_index(sel_range.end - ls) } else { right_side };
                    if x1 > x0 {
                        window.paint_quad(fill(
                            Bounds::from_corners(point(x0, line_top(line)), point(x1, line_top(line) + px(lh))),
                            hsla(one_dark::SELECTION),
                        ));
                    }
                }
            }

            // Matching brackets.
            if focused {
                if let Some((a, b)) = editor.matching_brackets() {
                    for r in [a, b] {
                        if r.is_empty() {
                            continue;
                        }
                        let line = buffer.line_of(r.start);
                        if let Some((_, shaped)) = pp.lines.iter().find(|(l, _)| *l == line) {
                            let ls = buffer.line_start(line);
                            let x0 = shaped.x_for_index(r.start - ls);
                            let x1 = shaped.x_for_index(r.end - ls);
                            let top = line_top(line) + px(box_pad);
                            window.paint_quad(fill(
                                Bounds::from_corners(point(text_left + x0, top), point(text_left + x1, top + px(layout.text_box_height))),
                                hsla(one_dark::MATCHING_BRACKET),
                            ));
                        }
                    }
                }
            }

            // Lint ranges: wavy underline or point marker.
            for d in &editor.diagnostics {
                let (from, to) = (d.from.min(buffer.len()), d.to.min(buffer.len()));
                let l0 = buffer.line_of(from);
                let l1 = buffer.line_of(to);
                for (line, shaped) in &pp.lines {
                    let line = *line;
                    if line < l0 || line > l1 {
                        continue;
                    }
                    let ls = buffer.line_start(line);
                    let le = buffer.line_end(line);
                    let s = from.max(ls) - ls;
                    let e = to.min(le) - ls;
                    let baseline_bottom = line_top(line) + px(box_pad + layout.text_box_height + 0.7);
                    if from == to {
                        // cm-lintPoint: 6×4 triangle under the position.
                        let x = text_left + shaped.x_for_index(s);
                        let mut pb = PathBuilder::fill();
                        pb.move_to(point(x - px(2.), baseline_bottom));
                        pb.line_to(point(x + px(4.), baseline_bottom));
                        pb.line_to(point(x + px(1.), baseline_bottom - px(4.)));
                        pb.close();
                        if let Ok(path) = pb.build() {
                            window.paint_path(path, hsla(one_dark::LINT_BORDER));
                        }
                        continue;
                    }
                    let x0 = text_left + shaped.x_for_index(s);
                    let x1 = text_left + shaped.x_for_index(e);
                    if x1 <= x0 {
                        continue;
                    }
                    // SVG pattern: m0 2.5 l2 -1.5 l1 0 l2 1.5 l1 0 (6×3 tile).
                    let top = baseline_bottom - px(3.);
                    let mut pb = PathBuilder::stroke(px(0.7));
                    let mut x = x0;
                    pb.move_to(point(x, top + px(2.5)));
                    while x < x1 {
                        let seg = [(2.0, -1.5), (1.0, 0.0), (2.0, 1.5), (1.0, 0.0)];
                        let mut y = 2.5;
                        for (dx, dy) in seg {
                            x += px(dx);
                            y += dy;
                            pb.line_to(point(x.min(x1), top + px(y)));
                            if x >= x1 {
                                break;
                            }
                        }
                    }
                    if let Ok(path) = pb.build() {
                        window.paint_path(path, hsla(one_dark::LINT_ERROR));
                    }
                }
            }

        });

        // Text and the cursor are painted in their own passes: `ShapedLine::paint`
        // needs `&mut App`, which cannot be borrowed while the editor is.
        let cursor = (focused && editor.cursor_visible && sel.is_empty())
            .then(|| {
                pp.lines.iter().find(|(l, _)| *l == head_line).map(|(_, shaped)| {
                    let x = text_left + shaped.x_for_index(sel.head - buffer.line_start(head_line)) - px(0.6);
                    Bounds::new(point(x, line_top(head_line) + px(box_pad)), size(px(1.2), px(layout.text_box_height)))
                })
            })
            .flatten();
        let text_lines: Vec<(Pixels, Pixels)> =
            pp.lines.iter().map(|(line, _)| (text_left, line_top(*line))).collect();
        let placeholder_origin = point(text_left, line_top(0));
        let _ = editor;
        window.with_content_mask(Some(ContentMask { bounds: text_area }), |window| {
            for ((_, shaped), (x, y)) in pp.lines.iter().zip(text_lines) {
                shaped.paint(point(x, y), px(lh), window, cx).ok();
            }
            if let Some(ph) = &pp.placeholder {
                ph.paint(placeholder_origin, px(lh), window, cx).ok();
            }
            if let Some(cursor) = cursor {
                window.paint_quad(fill(cursor, hsla(one_dark::CURSOR)));
            }
        });
        let editor = self.editor.read(cx);
        let buffer = &editor.buffer;

        // Gutters (sticky; drawn over horizontally scrolled text).
        let gutter = Bounds::new(bounds.origin, size(gutter_w, bounds.size.height));
        window.paint_quad(fill(gutter, hsla(one_dark::BACKGROUND)));
        let mut marked: Vec<usize> = editor.diagnostics.iter().map(|d| buffer.line_of(d.from.min(buffer.len()))).collect();
        let _ = editor;
        window.with_content_mask(Some(ContentMask { bounds: gutter }), |window| {
            window.paint_quad(fill(
                Bounds::new(point(bounds.left(), line_top(head_line)), size(gutter_w, px(lh))),
                hsla(one_dark::HIGHLIGHT_BACKGROUND),
            ));
            let numbers_w = gutter_w - px(1.4 * layout.font_size);
            for (line, shaped) in &pp.numbers {
                let x = bounds.left() + numbers_w - px(3.) - shaped.width;
                shaped.paint(point(x, line_top(*line)), px(lh), window, cx).ok();
            }
            // Lint gutter markers: red circle (fill #f87, stroke #f43).
            marked.sort_unstable();
            marked.dedup();
            let em = layout.font_size;
            for line in marked {
                let cx0 = bounds.left() + numbers_w + px(0.2 * em + em / 2.0);
                let cy = line_top(line) + px(0.2 * em + em / 2.0);
                let r = em * 15.0 / 40.0;
                let stroke = em * 6.0 / 40.0;
                let mut outer = PathBuilder::fill();
                outer.move_to(point(cx0 + px(r + stroke / 2.0), cy));
                outer.arc_to(point(px(r + stroke / 2.0), px(r + stroke / 2.0)), px(0.), false, true, point(cx0 - px(r + stroke / 2.0), cy));
                outer.arc_to(point(px(r + stroke / 2.0), px(r + stroke / 2.0)), px(0.), false, true, point(cx0 + px(r + stroke / 2.0), cy));
                outer.close();
                if let Ok(p) = outer.build() {
                    window.paint_path(p, hsla(crate::ui::theme::hex(0xff4433)));
                }
                let mut inner = PathBuilder::fill();
                let ri = r - stroke / 2.0;
                inner.move_to(point(cx0 + px(ri), cy));
                inner.arc_to(point(px(ri), px(ri)), px(0.), false, true, point(cx0 - px(ri), cy));
                inner.arc_to(point(px(ri), px(ri)), px(0.), false, true, point(cx0 + px(ri), cy));
                inner.close();
                if let Ok(p) = inner.build() {
                    window.paint_path(p, hsla(crate::ui::theme::hex(0xff8877)));
                }
            }
        });

        let layout = pp.layout.clone();
        self.editor.update(cx, |editor, _| {
            editor.layout = Some(EditorLayout { lines: std::mem::take(&mut pp.lines), ..layout });
        });
    }
}
