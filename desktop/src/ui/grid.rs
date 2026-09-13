//! Virtualised results grid (port of `ResultsGrid.svelte`): header row built
//! from divs, body painted directly like the old `<canvas>` renderer.

use crate::ui::js;
use crate::ui::model::{pending_get, PendingEdits, QueryResult, SortDirection};
use crate::ui::textfmt::{escape_tsv_cell, format_value_for_clipboard};
use crate::ui::theme::{self, hsla, Rgba};
use gpui::{
    fill, point, px, quad, size, App, Bounds, BorderStyle, ContentMask, Corners, Edges, Element, ElementId, Entity,
    Font, FontStyle, FontWeight, GlobalElementId, IntoElement, LayoutId, PathBuilder, Pixels, Point, SharedString,
    Style, TextRun, Window,
};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub const BASE_ROW_HEIGHT: f32 = 28.0;
pub const BASE_CELL_PAD_X: f32 = 10.0;
const EXPLAIN_DEFAULT_TEXT_LEN: usize = 120;
/// How much of a value a column will widen to show when it is expanded
/// (double-clicking the header's right edge). Anything longer than this keeps
/// its eye button, which opens the whole value in a window.
pub const EXPAND_CHARS: usize = 256;
/// Side of the eye button drawn in cells whose value is longer than that.
pub const EYE_SIZE: f32 = 13.0;
pub const SCROLLBAR: f32 = 12.0;
/// `::-webkit-scrollbar-thumb` sits inside a 3px border of the track colour.
pub const SCROLLBAR_INSET: f32 = 3.0;
/// Shortest the thumb is allowed to get, so that there is always something
/// big enough to see and grab however many rows the query returned.
const MIN_THUMB: f32 = 28.0;

/// Radius of a pill of this size. gpui does *not* clamp corner radii to the
/// quad — a radius larger than the box makes the arcs miss it and the quad
/// disappears — so `border-radius: 999px` has to be worked out for real.
pub fn pill_radius(w: f32, h: f32) -> f32 {
    0.5 * w.min(h).max(0.0)
}

/// Which scrollbar a press or a drag is on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Bar {
    Vertical,
    Horizontal,
}

/// A scrollbar drag: which bar, and where inside the thumb it was grabbed.
#[derive(Clone, Copy, Debug)]
pub struct ScrollDrag {
    pub bar: Bar,
    pub grab: f32,
}

/// The body/scrollbar layout for one paint of the grid: how big the content is,
/// which scrollbars that needs, and therefore how much room the rows get. Paint
/// and hit-testing both go through this so a click lands on the thumb that was
/// drawn.
#[derive(Clone, Copy, Debug, Default)]
pub struct ScrollGeom {
    /// Body size, i.e. the outer size minus whichever scrollbars are shown.
    pub view_w: f32,
    pub view_h: f32,
    pub content_w: f32,
    pub content_h: f32,
    pub has_v: bool,
    pub has_h: bool,
}

impl ScrollGeom {
    pub fn new(outer_w: f32, outer_h: f32, content_w: f32, content_h: f32) -> Self {
        // Each scrollbar eats into the space the other one measures against.
        let mut has_v = content_h > outer_h;
        let mut has_h = content_w > outer_w - if has_v { SCROLLBAR } else { 0.0 };
        if has_h && !has_v {
            has_v = content_h > outer_h - SCROLLBAR;
        }
        if has_v && !has_h {
            has_h = content_w > outer_w - SCROLLBAR;
        }
        ScrollGeom {
            view_w: outer_w - if has_v { SCROLLBAR } else { 0.0 },
            view_h: outer_h - if has_h { SCROLLBAR } else { 0.0 },
            content_w,
            content_h,
            has_v,
            has_h,
        }
    }

    pub fn max_scroll_y(&self) -> f32 {
        (self.content_h - self.view_h).max(0.0)
    }

    pub fn max_scroll_x(&self) -> f32 {
        (self.content_w - self.view_w).max(0.0)
    }

    /// `(offset, length)` of the vertical thumb along the track.
    pub fn v_thumb(&self, scroll_y: f32) -> (f32, f32) {
        thumb(self.view_h, self.content_h, scroll_y)
    }

    pub fn h_thumb(&self, scroll_x: f32) -> (f32, f32) {
        thumb(self.view_w, self.content_w, scroll_x)
    }

    /// The scroll offset that puts the thumb's near edge at `pos` along the
    /// track — the inverse of [`ScrollGeom::v_thumb`].
    pub fn scroll_for_v_thumb(&self, pos: f32) -> f32 {
        let (_, th) = self.v_thumb(0.0);
        scroll_for(pos, self.view_h, th, self.max_scroll_y())
    }

    pub fn scroll_for_h_thumb(&self, pos: f32) -> f32 {
        let (_, tw) = self.h_thumb(0.0);
        scroll_for(pos, self.view_w, tw, self.max_scroll_x())
    }

    /// Which scrollbar, if any, is under a point in body-local coordinates.
    pub fn bar_at(&self, local: Point<Pixels>) -> Option<Bar> {
        let (x, y): (f32, f32) = (local.x.into(), local.y.into());
        if x < 0.0 || y < 0.0 {
            return None;
        }
        if self.has_v && x >= self.view_w && y < self.view_h {
            return Some(Bar::Vertical);
        }
        if self.has_h && y >= self.view_h && x < self.view_w {
            return Some(Bar::Horizontal);
        }
        None
    }

    /// Distance along the track of a point on `bar`.
    pub fn pos_on(&self, bar: Bar, local: Point<Pixels>) -> f32 {
        match bar {
            Bar::Vertical => local.y.into(),
            Bar::Horizontal => local.x.into(),
        }
    }
}

/// The eye button: a lens outline with a pupil, drawn as paths because gpui
/// can only shape text it has a glyph for and this has to scale with the grid.
fn paint_eye(window: &mut Window, center: Point<Pixels>, size: f32, color: Rgba) {
    let (hw, hh) = (size * 0.5, size * 0.3);
    let at = |dx: f32, dy: f32| point(center.x + px(dx), center.y + px(dy));
    let steps = 10;
    // A lid is a parabola from one corner of the lens to the other.
    let lid = |t: f32, up: bool| {
        let x = -hw + size * t;
        let bulge = hh * (1.0 - (2.0 * t - 1.0).powi(2));
        (x, if up { -bulge } else { bulge })
    };
    let mut pb = PathBuilder::stroke(px((size * 0.09).max(1.0)));
    for i in 0..=steps {
        let (x, y) = lid(i as f32 / steps as f32, true);
        if i == 0 {
            pb.move_to(at(x, y));
        } else {
            pb.line_to(at(x, y));
        }
    }
    for i in (0..=steps).rev() {
        let (x, y) = lid(i as f32 / steps as f32, false);
        pb.line_to(at(x, y));
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, hsla(color));
    }

    let r = size * 0.17;
    let mut pb = PathBuilder::fill();
    for i in 0..=16 {
        let a = std::f32::consts::TAU * i as f32 / 16.0;
        let p = at(r * a.cos(), r * a.sin());
        if i == 0 {
            pb.move_to(p);
        } else {
            pb.line_to(p);
        }
    }
    pb.close();
    if let Ok(path) = pb.build() {
        window.paint_path(path, hsla(color));
    }
}

fn thumb(view: f32, content: f32, scroll: f32) -> (f32, f32) {
    if content <= view || view <= 0.0 {
        return (0.0, view.max(0.0));
    }
    let len = (view * (view / content)).max(MIN_THUMB).min(view);
    let pos = (scroll / (content - view)).clamp(0.0, 1.0) * (view - len);
    (pos, len)
}

fn scroll_for(thumb_pos: f32, view: f32, thumb_len: f32, max_scroll: f32) -> f32 {
    let travel = view - thumb_len;
    if travel <= 0.0 {
        return 0.0;
    }
    (thumb_pos / travel).clamp(0.0, 1.0) * max_scroll
}

/// Where a cmd/ctrl-arrow, Home or End jump lands.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Jump {
    First,
    Last,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellSel {
    pub r0: usize,
    pub c0: usize,
    pub r1: usize,
    pub c1: usize,
}

impl CellSel {
    pub fn norm(&self) -> (usize, usize, usize, usize) {
        (self.r0.min(self.r1), self.r0.max(self.r1), self.c0.min(self.c1), self.c0.max(self.c1))
    }
}

/// A cell value shown in full, opened from its eye button.
pub struct CellPopup {
    pub column: String,
    /// 1-based row number, as the grid shows it.
    pub row: usize,
    pub text: String,
}

pub struct EditOverlay {
    pub row: usize,
    pub col: usize,
    pub row_data_idx: usize,
    pub input: Entity<crate::ui::widgets::text_input::TextInput>,
    pub origin: Point<Pixels>,
    pub width: f32,
}

#[derive(Default)]
pub struct GridState {
    pub tab_id: String,
    pub generation: u64,
    pub col_key: String,
    pub col_widths: Vec<f32>,
    pub max_len: Vec<usize>,
    pub max_base_width: Vec<f32>,
    pub measured_rows: usize,
    pub font_scale: f32,
    pub scroll_x: f32,
    pub scroll_y: f32,
    pub sel: Option<CellSel>,
    pub selecting: bool,
    pub sel_anchor: Option<(usize, usize)>,
    pub last_selected: Option<(usize, usize)>,
    pub selected_rows: BTreeSet<usize>,
    pub row_anchor: Option<usize>,
    pub hovered_row: Option<usize>,
    pub edit: Option<EditOverlay>,
    pub tooltip: Option<(usize, Point<Pixels>)>,
    pub resizing: Option<(usize, Pixels, f32)>,
    pub did_resize: bool,
    pub body_bounds: Option<Bounds<Pixels>>,
    pub header_left: f32,
    /// In-progress scrollbar-thumb drag.
    pub scroll_drag: Option<ScrollDrag>,
    /// Scrollbar the pointer is over, for the thumb's `:hover` colour.
    pub hovered_bar: Option<Bar>,
    sort_cache: Option<(u64, usize, usize, SortDirection, Arc<Vec<usize>>)>,
    advance_cache: HashMap<char, f32>,
    pub focus: Option<gpui::FocusHandle>,
    pub last_drag_pos: Option<Point<Pixels>>,
}

pub fn row_height(scale: f32) -> f32 {
    BASE_ROW_HEIGHT * scale
}

pub fn row_num_width(total_rows: usize, scale: f32) -> f32 {
    (40.0 * scale).max(total_rows.to_string().len() as f32 * 8.0 * scale + 16.0 * scale)
}

fn initial_text_len(col: &str) -> usize {
    if col.to_lowercase() == "explain" {
        js::char_len(col).max(EXPLAIN_DEFAULT_TEXT_LEN)
    } else {
        js::char_len(col)
    }
}

/// `calculateAutoFitColumnWidth` from lib/columnSizing.ts. There is no upper
/// clamp here: the width comes from cell text already cut to [`EXPAND_CHARS`],
/// which is what bounds how far a column can expand.
pub fn auto_fit_width(max_cell_text_width: f32, header_text_width: f32, pad: f32, scale: f32) -> f32 {
    let cell = max_cell_text_width * scale + pad * 2.0 + 12.0 * scale;
    let header = header_text_width * scale + pad * 2.0 + 28.0 * scale;
    cell.max(header).max(50.0 * scale)
}

impl GridState {
    fn measure(&mut self, text: &str, cx: &App) -> f32 {
        // Sum of per-character advances of the UI font at 12px (what the old
        // canvas `measureText` returned for these strings).
        let mut missing: Vec<char> = Vec::new();
        for ch in text.chars() {
            if !self.advance_cache.contains_key(&ch) {
                missing.push(ch);
            }
        }
        if !missing.is_empty() {
            let ts = cx.text_system();
            let id = ts.resolve_font(&gpui::font(theme::UI_FONT));
            for ch in missing {
                let w = ts.advance(id, px(12.), ch).map(|s| f32::from(s.width)).unwrap_or(5.6);
                self.advance_cache.insert(ch, w);
            }
        }
        text.chars().map(|c| self.advance_cache[&c]).sum()
    }

    fn column_width(&mut self, idx: usize, columns: &[String], cx: &App) -> f32 {
        let header = self.measure(columns.get(idx).map(String::as_str).unwrap_or(""), cx);
        let pad = BASE_CELL_PAD_X * self.font_scale;
        auto_fit_width(self.max_base_width.get(idx).copied().unwrap_or(0.0), header, pad, self.font_scale)
    }

    /// Called whenever the grid is pointed at a (possibly) different result.
    pub fn on_result_changed(&mut self, tab_id: &str, generation: u64, columns: &Arc<Vec<String>>, has_result: bool, scale: f32) {
        let changed = self.tab_id != tab_id || self.generation != generation;
        self.tab_id = tab_id.to_string();
        self.generation = generation;
        self.font_scale = scale;
        if !has_result || changed {
            self.scroll_x = 0.0;
            self.scroll_y = 0.0;
            self.sel = None;
            self.selected_rows.clear();
            self.row_anchor = None;
            self.last_selected = None;
            self.edit = None;
            self.hovered_row = None;
            self.sort_cache = None;
        }
        let key = columns.join("\u{0}");
        if key != self.col_key || !has_result {
            self.col_key = key;
            self.max_len = columns.iter().map(|c| initial_text_len(c)).collect();
            self.max_base_width = vec![0.0; columns.len()];
            self.col_widths = Vec::new();
        }
        self.measured_rows = 0;
    }

    pub fn on_rows_appended(&mut self, tab_id: &str, generation: u64) {
        if self.tab_id == tab_id && self.generation == generation {
            self.sort_cache = None;
        }
    }

    pub fn rescale(&mut self, new_scale: f32, old_scale: f32) {
        if !self.col_widths.is_empty() && old_scale > 0.0 && (new_scale - old_scale).abs() > f32::EPSILON {
            let ratio = new_scale / old_scale;
            self.col_widths = self.col_widths.iter().map(|w| (w * ratio).max(50.0 * new_scale)).collect();
        }
        self.font_scale = new_scale;
    }

    /// Incrementally scan new rows for auto-fit widths (and compute initial widths).
    pub fn sync_measurements(&mut self, result: &QueryResult, cx: &App) {
        let columns = result.columns.clone();
        if self.max_len.len() != columns.len() {
            self.max_len = columns.iter().map(|c| initial_text_len(c)).collect();
            self.max_base_width = vec![0.0; columns.len()];
            self.col_widths.clear();
            self.measured_rows = 0;
        }
        if self.col_widths.is_empty() && !columns.is_empty() {
            self.col_widths = (0..columns.len()).map(|i| self.column_width(i, &columns, cx)).collect();
        }
        let n = result.rows.len();
        if n > self.measured_rows {
            for r in self.measured_rows..n {
                let Some(row) = result.rows.get(r) else { break };
                for c in 0..row.len().min(columns.len()) {
                    let text = match &row[c] {
                        serde_json::Value::Null => "NULL".to_string(),
                        v => js::value_to_string(v),
                    };
                    let len = js::char_len(&text);
                    if len > self.max_len[c] {
                        self.max_len[c] = len;
                    }
                    // Expanding stops at EXPAND_CHARS; the rest of a longer
                    // value is reached through its eye button.
                    let w = self.measure(js::slice_chars(&text, EXPAND_CHARS), cx);
                    if w > self.max_base_width[c] {
                        self.max_base_width[c] = w;
                    }
                }
            }
            self.measured_rows = n;
        }
    }

    pub fn auto_fit(&mut self, idx: usize, columns: &[String], cx: &App) {
        if idx < self.col_widths.len() {
            self.col_widths[idx] = self.column_width(idx, columns, cx);
        }
    }

    pub fn sort_index(&mut self, result: &QueryResult, sort: Option<(usize, SortDirection)>) -> Option<Arc<Vec<usize>>> {
        let (col, dir) = sort?;
        let n = result.rows.len();
        if n == 0 {
            return None;
        }
        if let Some((g, len, c, d, idx)) = &self.sort_cache {
            if *g == result.generation && *len == n && *c == col && *d == dir {
                return Some(idx.clone());
            }
        }
        let keys: Vec<Option<String>> = (0..n)
            .map(|i| result.rows.get(i).and_then(|r| r.get(col)).and_then(js::cell_text))
            .collect();
        let mut idx: Vec<usize> = (0..n).collect();
        let sign = if dir == SortDirection::Asc { 1 } else { -1 };
        idx.sort_by(|&a, &b| {
            use std::cmp::Ordering::*;
            let ord = match (&keys[a], &keys[b]) {
                (None, None) => Equal,
                (None, _) => if sign > 0 { Greater } else { Less },
                (_, None) => if sign > 0 { Less } else { Greater },
                (Some(x), Some(y)) => {
                    let o = js::locale_compare_numeric(x, y);
                    if sign > 0 { o } else { o.reverse() }
                }
            };
            ord
        });
        let idx = Arc::new(idx);
        self.sort_cache = Some((result.generation, n, col, dir, idx.clone()));
        Some(idx)
    }

    pub fn total_width(&self, total_rows: usize) -> f32 {
        row_num_width(total_rows, self.font_scale) + self.col_widths.iter().sum::<f32>()
    }

    /// Scrollbar layout for the body as it was last painted.
    pub fn geom(&self, total_rows: usize) -> ScrollGeom {
        let Some(b) = self.body_bounds else { return ScrollGeom::default() };
        ScrollGeom::new(
            b.size.width.into(),
            b.size.height.into(),
            self.total_width(total_rows),
            total_rows as f32 * row_height(self.font_scale),
        )
    }

    pub fn clamp_scroll(&mut self, total_rows: usize, view_w: f32, view_h: f32) {
        let rh = row_height(self.font_scale);
        let max_y = (total_rows as f32 * rh - view_h).max(0.0);
        let max_x = (self.total_width(total_rows) - view_w).max(0.0);
        self.scroll_y = self.scroll_y.clamp(0.0, max_y);
        self.scroll_x = self.scroll_x.clamp(0.0, max_x);
    }

    /// (row, col) hit-test in body coordinates.
    pub fn cell_at(&self, local: Point<Pixels>, total_rows: usize) -> Option<(usize, usize)> {
        let rh = row_height(self.font_scale);
        let rnw = row_num_width(total_rows, self.font_scale);
        let mx: f32 = local.x.into();
        let my: f32 = local.y.into();
        if mx < rnw {
            return None;
        }
        let row = ((my + self.scroll_y) / rh).floor();
        if row < 0.0 || row as usize >= total_rows {
            return None;
        }
        let x = mx - rnw + self.scroll_x;
        if x < 0.0 {
            return None;
        }
        let mut cum = 0.0;
        for (c, w) in self.col_widths.iter().enumerate() {
            cum += w;
            if x < cum {
                return Some((row as usize, c));
            }
        }
        None
    }

    /// Where the eye button sits in a cell, in body coordinates. Cells whose
    /// value is longer than [`EXPAND_CHARS`] draw one at their right edge.
    pub fn eye_at(&self, row: usize, col: usize, total_rows: usize) -> Bounds<Pixels> {
        let s = self.font_scale;
        let rh = row_height(s);
        let pad = BASE_CELL_PAD_X * s;
        let size = EYE_SIZE * s;
        let left: f32 = self.col_widths[..col.min(self.col_widths.len())].iter().sum();
        let x = row_num_width(total_rows, s) + left - self.scroll_x;
        let cw = self.col_widths.get(col).copied().unwrap_or(0.0);
        let y = row as f32 * rh - self.scroll_y;
        Bounds::new(
            point(px(x + cw - pad - size), px(y + (rh - size) / 2.0)),
            gpui::size(px(size), px(size)),
        )
    }

    pub fn row_at(&self, local: Point<Pixels>, total_rows: usize) -> Option<usize> {
        let rh = row_height(self.font_scale);
        let rnw = row_num_width(total_rows, self.font_scale);
        if f32::from(local.x) >= rnw {
            return None;
        }
        let row = ((f32::from(local.y) + self.scroll_y) / rh).floor();
        (row >= 0.0 && (row as usize) < total_rows).then_some(row as usize)
    }

    pub fn clear_selection(&mut self) {
        self.sel = None;
        self.selected_rows.clear();
        self.row_anchor = None;
        self.last_selected = None;
    }

    pub fn copy_text(&mut self, result: &QueryResult, sort: Option<(usize, SortDirection)>) -> Option<String> {
        let idx = self.sort_index(result, sort);
        let data_idx = |r: usize| idx.as_ref().and_then(|i| i.get(r).copied()).unwrap_or(r);
        let cols = result.columns.len();
        let ty = |c: usize| result.column_types.get(c).map(String::as_str);
        if !self.selected_rows.is_empty() {
            let lines: Vec<String> = self
                .selected_rows
                .iter()
                .filter_map(|&r| result.rows.get(data_idx(r)))
                .map(|row| {
                    (0..cols)
                        .map(|c| escape_tsv_cell(&format_value_for_clipboard(row.get(c).unwrap_or(&serde_json::Value::Null), ty(c), "")))
                        .collect::<Vec<_>>()
                        .join("\t")
                })
                .collect();
            return (!lines.is_empty()).then(|| lines.join("\n"));
        }
        let sel = self.sel?;
        let (r0, r1, c0, c1) = sel.norm();
        let lines: Vec<String> = (r0..=r1)
            .filter_map(|r| result.rows.get(data_idx(r)))
            .map(|row| {
                (c0..=c1)
                    .map(|c| escape_tsv_cell(&format_value_for_clipboard(row.get(c).unwrap_or(&serde_json::Value::Null), ty(c), "")))
                    .collect::<Vec<_>>()
                    .join("\t")
            })
            .collect();
        Some(lines.join("\n"))
    }

    pub fn move_selection(&mut self, dr: isize, dc: isize, extend: bool, total_rows: usize, cols: usize, view: (f32, f32)) {
        if cols == 0 || total_rows == 0 {
            return;
        }
        let max_row = total_rows as isize - 1;
        let max_col = cols as isize - 1;
        let current = self.head();
        let next = (
            (current.0 as isize + dr).clamp(0, max_row) as usize,
            (current.1 as isize + dc).clamp(0, max_col) as usize,
        );
        self.set_head(next, extend, total_rows, view);
    }

    /// Jump the selection to the first/last row or column — what cmd (or ctrl)
    /// with an arrow, and Home/End, do. `extend` keeps the anchor, so
    /// shift-cmd-down selects everything from the cursor to the last row.
    pub fn jump_selection(
        &mut self,
        row: Option<Jump>,
        col: Option<Jump>,
        extend: bool,
        total_rows: usize,
        cols: usize,
        view: (f32, f32),
    ) {
        if cols == 0 || total_rows == 0 {
            return;
        }
        let current = self.head();
        let next = (
            match row {
                Some(Jump::First) => 0,
                Some(Jump::Last) => total_rows - 1,
                None => current.0.min(total_rows - 1),
            },
            match col {
                Some(Jump::First) => 0,
                Some(Jump::Last) => cols - 1,
                None => current.1.min(cols - 1),
            },
        );
        self.set_head(next, extend, total_rows, view);
    }

    /// The moving end of the selection.
    fn head(&self) -> (usize, usize) {
        match self.sel {
            Some(s) => (s.r1, s.c1),
            None => self.last_selected.unwrap_or((0, 0)),
        }
    }

    fn set_head(&mut self, next: (usize, usize), extend: bool, total_rows: usize, view: (f32, f32)) {
        let current = self.head();
        if extend {
            let anchor = match self.sel {
                Some(s) => (s.r0, s.c0),
                None => self.last_selected.unwrap_or(current),
            };
            self.sel = Some(CellSel { r0: anchor.0, c0: anchor.1, r1: next.0, c1: next.1 });
        } else {
            self.sel = Some(CellSel { r0: next.0, c0: next.1, r1: next.0, c1: next.1 });
            self.last_selected = Some(next);
        }
        self.ensure_row_visible(next.0, total_rows, view);
        self.ensure_col_visible(next.1, total_rows, view);
    }

    /// `ensureRowVisible`: scroll the row into view by the smallest amount.
    pub fn ensure_row_visible(&mut self, row: usize, total_rows: usize, view: (f32, f32)) {
        let rh = row_height(self.font_scale);
        let (w, h) = view;
        let visible_rows = (h / rh).floor().max(1.0) as usize;
        let start = (self.scroll_y / rh).floor() as usize;
        if row < start {
            self.scroll_y = row as f32 * rh;
        } else if row > start + visible_rows - 1 {
            self.scroll_y = (row + 1 - visible_rows) as f32 * rh;
        }
        self.clamp_scroll(total_rows, w, h);
    }

    /// `ensureColVisible`.
    pub fn ensure_col_visible(&mut self, col: usize, total_rows: usize, view: (f32, f32)) {
        let (w, h) = view;
        let end = col.min(self.col_widths.len());
        let col_left: f32 = self.col_widths[..end].iter().sum();
        let col_right = col_left + self.col_widths.get(col).copied().unwrap_or(0.0);
        let data_w = (w - row_num_width(total_rows, self.font_scale)).max(1.0);
        if col_left < self.scroll_x {
            self.scroll_x = col_left;
        } else if col_right > self.scroll_x + data_w {
            self.scroll_x = col_right - data_w;
        }
        self.clamp_scroll(total_rows, w, h);
    }
}

// ─── Body element ───────────────────────────────────────────────────────────

pub struct GridBody {
    pub result: QueryResult,
    pub sort_index: Option<Arc<Vec<usize>>>,
    pub col_widths: Vec<f32>,
    pub scale: f32,
    pub scroll: (f32, f32),
    pub sel: Option<CellSel>,
    pub selected_rows: BTreeSet<usize>,
    pub hovered_row: Option<usize>,
    /// Scrollbar being hovered or dragged, drawn with the thumb's hover colour.
    pub active_bar: Option<Bar>,
    pub pending: PendingEdits,
    pub editing: Option<(usize, usize)>,
    pub on_bounds: Box<dyn Fn(Bounds<Pixels>, &mut App)>,
}

impl IntoElement for GridBody {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

fn ui_font(style: FontStyle) -> Font {
    Font { family: theme::UI_FONT.into(), features: Default::default(), fallbacks: None, weight: FontWeight::NORMAL, style }
}

impl Element for GridBody {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }
    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, window: &mut Window, cx: &mut App) -> (LayoutId, ()) {
        let mut style = Style::default();
        style.size.width = gpui::relative(1.).into();
        style.size.height = gpui::relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(&mut self, _: Option<&GlobalElementId>, _: Option<&gpui::InspectorElementId>, _: Bounds<Pixels>, _: &mut (), _: &mut Window, _: &mut App) {}

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        outer: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        (self.on_bounds)(outer, cx);
        let s = self.scale;
        let rh = row_height(s);
        let pad = BASE_CELL_PAD_X * s;
        let total = self.result.rows.len();
        let rnw = row_num_width(total, s);
        let content_w = rnw + self.col_widths.iter().sum::<f32>();
        let content_h = total as f32 * rh;
        let ow: f32 = outer.size.width.into();
        let oh: f32 = outer.size.height.into();
        // Scrollbars take 12px when content overflows (custom WebKit scrollbar styling).
        let geom = ScrollGeom::new(ow, oh, content_w, content_h);
        let (has_v, has_h) = (geom.has_v, geom.has_h);
        let (w, h) = (geom.view_w, geom.view_h);
        let origin = outer.origin;
        let bounds = Bounds::new(origin, size(px(w), px(h)));
        let (sl, st) = self.scroll;
        let start_row = (st / rh).floor() as usize;
        let y_off = -(st % rh);
        let visible = (h / rh).ceil() as usize + 2;

        let mut col_x = Vec::with_capacity(self.col_widths.len());
        let mut cxp = rnw;
        for w in &self.col_widths {
            col_x.push(cxp);
            cxp += w;
        }
        let n = self.col_widths.len();
        let mut col_start = 0;
        let mut col_end = n;
        for c in 0..n {
            if col_x[c] + self.col_widths[c] > sl {
                col_start = c;
                break;
            }
        }
        for c in (0..n).rev() {
            if col_x[c] < sl + w {
                col_end = c + 1;
                break;
            }
        }
        col_start = col_start.saturating_sub(1);
        col_end = (col_end + 1).min(n);

        let (sr0, sr1, sc0, sc1) = self.sel.map(|x| x.norm()).unwrap_or((usize::MAX, 0, usize::MAX, 0));
        let at = |x: f32, y: f32| point(origin.x + px(x), origin.y + px(y));
        let rect = |x: f32, y: f32, ww: f32, hh: f32| Bounds::new(at(x, y), size(px(ww), px(hh)));
        let paint = |window: &mut Window, r: Bounds<Pixels>, c: Rgba| window.paint_quad(fill(r, hsla(c)));

        let ts = window.text_system().clone();
        let font = ui_font(FontStyle::Normal);
        let font_italic = ui_font(FontStyle::Italic);
        let fid = ts.resolve_font(&font);
        let fs12 = 12.0 * s;
        let fs11 = 11.0 * s;
        // Canvas `textBaseline = 'middle'`: baseline at centre + (ascent - descent)/2.
        let mid_shift = |fs: f32| {
            let a: f32 = ts.ascent(fid, px(fs)).into();
            let d: f32 = f32::from(ts.descent(fid, px(fs))).abs();
            (a - d) / 2.0
        };
        let shift12 = mid_shift(fs12);
        let shift11 = mid_shift(fs11);

        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            paint(window, bounds, theme::BG);
            paint(window, rect(0.0, 0.0, rnw, h), theme::BG_PANEL);
            paint(window, rect(rnw - 1.0, 0.0, 1.0, h), theme::BORDER);

            for i in 0..visible {
                let abs = start_row + i;
                if abs >= total {
                    break;
                }
                let data_idx = self.sort_index.as_ref().and_then(|x| x.get(abs).copied()).unwrap_or(abs);
                let Some(row) = self.result.rows.get(data_idx) else { continue };
                let y = y_off + i as f32 * rh;
                if y + rh < 0.0 || y > h {
                    continue;
                }
                let in_cell_sel = self.sel.is_some() && abs >= sr0 && abs <= sr1;
                let in_row_sel = self.selected_rows.contains(&abs);
                let row_bg = if Some(abs) == self.hovered_row {
                    Some(theme::BG_HOVER)
                } else if abs % 2 == 1 {
                    Some(theme::BG_ROW_ALT)
                } else {
                    None
                };
                if let Some(c) = row_bg {
                    paint(window, rect(rnw, y, w - rnw, rh), c);
                }
                if in_row_sel {
                    paint(window, rect(rnw, y, w - rnw, rh - 1.0), theme::GRID_ROW_SEL);
                }
                paint(window, rect(0.0, y + rh - 1.0, w, 1.0), theme::BORDER_SUBTLE);

                for c in col_start..col_end {
                    let cw = self.col_widths[c];
                    let x = col_x[c] - sl;
                    paint(window, rect(x + cw - 1.0, y, 1.0, rh), theme::BORDER_SUBTLE);
                    if in_cell_sel && c >= sc0 && c <= sc1 {
                        paint(window, rect(x, y, cw - 1.0, rh - 1.0), theme::GRID_SEL);
                    } else if in_cell_sel {
                        let c2 = if Some(abs) == self.hovered_row {
                            theme::BG_HOVER
                        } else if abs % 2 == 1 {
                            theme::BG_ROW_ALT
                        } else {
                            theme::BG
                        };
                        paint(window, rect(x, y, cw - 1.0, rh - 1.0), c2);
                    }
                    let text_max = cw - pad * 2.0;
                    if text_max <= 0.0 {
                        continue;
                    }
                    if self.editing == Some((data_idx, c)) {
                        paint(window, rect(x + 1.0, y, cw - 2.0, rh - 1.0), theme::BG_PANEL);
                        continue;
                    }
                    let dirty = pending_get(&self.pending, data_idx, c);
                    if dirty.is_some() {
                        paint(window, rect(x, y, cw - 1.0, rh - 1.0), theme::GRID_DIRTY);
                        paint(window, rect(x, y, 2.0, rh - 1.0), theme::GRID_BORDER_DIRTY);
                    }
                    let clip = rect(x + 1.0, y, cw - 2.0, rh);
                    let value = row.get(c).cloned().unwrap_or(serde_json::Value::Null);
                    let (text, color, italic) = match dirty {
                        Some(d) if d.is_empty() => (None, theme::TEXT, false),
                        Some(d) => (Some(d.clone()), theme::TEXT, false),
                        None => match &value {
                            serde_json::Value::Null => (Some("NULL".to_string()), theme::with_alpha(theme::TEXT_MUTED, 0.6), true),
                            v => {
                                let t = js::value_to_string(v);
                                if t.is_empty() { (None, theme::TEXT, false) } else { (Some(t), theme::TEXT, false) }
                            }
                        },
                    };
                    if let Some(text) = text {
                        // Values too long to ever fit get an eye button that
                        // opens the whole thing; the text stops short of it.
                        let long = js::char_len(&text) > EXPAND_CHARS;
                        let eye = EYE_SIZE * s;
                        let clip = if long { rect(x + 1.0, y, (cw - 2.0 - eye - pad * 0.5).max(0.0), rh) } else { clip };
                        if long {
                            paint_eye(
                                window,
                                at(x + cw - pad - eye / 2.0, y + rh / 2.0),
                                eye,
                                theme::TEXT_MUTED,
                            );
                        }
                        // Canvas fillText renders the string on one line.
                        let text: String = text.replace(['\n', '\r'], " ");
                        let text = if text.len() > 4000 { js::slice_chars(&text, 1000).to_string() } else { text };
                        let run = TextRun {
                            len: text.len(),
                            font: if italic { font_italic.clone() } else { font.clone() },
                            color: hsla(color),
                            background_color: None,
                            underline: None,
                            strikethrough: None,
                        };
                        let line = ts.shape_line(SharedString::from(text), px(fs12), &[run], None);
                        let baseline = y + rh / 2.0 + shift12;
                        let ascent: f32 = line.ascent.into();
                        let descent: f32 = f32::from(line.descent).abs();
                        let lh = ascent + descent;
                        let top = baseline - ascent;
                        window.with_content_mask(Some(ContentMask { bounds: clip }), |window| {
                            line.paint(at(x + pad, top), px(lh), window, cx).ok();
                        });
                    }
                }

                // Sticky row-number column.
                paint(window, rect(0.0, y, rnw, rh), if in_row_sel { theme::GRID_ROWNUM_SEL } else { theme::BG_PANEL });
                paint(window, rect(rnw - 1.0, y, 1.0, rh), theme::BORDER);
                let num = (abs + 1).to_string();
                let run = TextRun { len: num.len(), font: font.clone(), color: hsla(theme::TEXT_MUTED), background_color: None, underline: None, strikethrough: None };
                let line = ts.shape_line(SharedString::from(num), px(fs11), &[run], None);
                let ascent: f32 = line.ascent.into();
                let descent: f32 = f32::from(line.descent).abs();
                let baseline = y + rh / 2.0 + shift11;
                let lw: f32 = line.width.into();
                line.paint(at(rnw - 8.0 * s - lw, baseline - ascent), px(ascent + descent), window, cx).ok();
            }

            // Selection border (1.5px stroke inside the rectangle).
            if let Some(sel) = self.sel {
                if sc0 < n && sc1 < n {
                    let top_vis = sr0.max(start_row);
                    let bot_vis = sr1.min(start_row + visible - 1);
                    let _ = sel;
                    if top_vis <= bot_vis {
                        let yt = y_off + (top_vis - start_row) as f32 * rh;
                        let yb = y_off + (bot_vis - start_row + 1) as f32 * rh - 1.0;
                        let xl = col_x[sc0] - sl;
                        let xr = col_x[sc1] - sl + self.col_widths[sc1] - 1.0;
                        let data_area = rect(rnw, 0.0, w - rnw, h);
                        window.with_content_mask(Some(ContentMask { bounds: data_area }), |window| {
                            window.paint_quad(quad(
                                rect(xl, yt, xr - xl, yb - yt),
                                Corners::default(),
                                gpui::transparent_black(),
                                Edges::all(px(1.5)),
                                hsla(theme::GRID_BORDER_SEL),
                                BorderStyle::Solid,
                            ));
                        });
                    }
                }
            }
        });

        // Custom scrollbars: track rgba(255,255,255,.2), thumb rgba(236,240,248,.35)
        // inset by a 3px border, fully rounded.
        let track = theme::rgba8(255, 255, 255, 0.2);
        let thumb_color = |bar: Bar| {
            if self.active_bar == Some(bar) {
                // `::-webkit-scrollbar-thumb:hover`
                theme::rgba8(246, 249, 255, 0.68)
            } else {
                theme::rgba8(236, 240, 248, 0.35)
            }
        };
        let inset = SCROLLBAR_INSET;
        let thickness = SCROLLBAR - 2.0 * inset;
        if has_v {
            let tr = Bounds::new(point(origin.x + px(w), origin.y), size(px(SCROLLBAR), px(h)));
            window.paint_quad(fill(tr, hsla(track)));
            let (ty, th) = geom.v_thumb(st);
            let len = (th - 2.0 * inset).max(1.0);
            let tb = Bounds::new(point(origin.x + px(w + inset), origin.y + px(ty + inset)), size(px(thickness), px(len)));
            window.paint_quad(fill(tb, hsla(thumb_color(Bar::Vertical))).corner_radii(px(pill_radius(thickness, len))));
        }
        if has_h {
            let tr = Bounds::new(point(origin.x, origin.y + px(h)), size(px(w), px(SCROLLBAR)));
            window.paint_quad(fill(tr, hsla(track)));
            let (tx, tw) = geom.h_thumb(sl);
            let len = (tw - 2.0 * inset).max(1.0);
            let tb = Bounds::new(point(origin.x + px(tx + inset), origin.y + px(h + inset)), size(px(len), px(thickness)));
            window.paint_quad(fill(tb, hsla(thumb_color(Bar::Horizontal))).corner_radii(px(pill_radius(len, thickness))));
        }
        if has_v && has_h {
            let corner = Bounds::new(point(origin.x + px(w), origin.y + px(h)), size(px(SCROLLBAR), px(SCROLLBAR)));
            window.paint_quad(fill(corner, hsla(theme::WHITE)));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `columnSizing.test.ts`.
    #[test]
    fn auto_fit_uses_the_wider_of_header_and_cells() {
        assert_eq!(auto_fit_width(12.0, 140.0, 10.0, 1.0), 188.0);
        assert_eq!(auto_fit_width(120.0, 10.0, 10.0, 1.0), 152.0);
    }

    #[test]
    fn auto_fit_has_a_floor_but_no_ceiling() {
        assert_eq!(auto_fit_width(0.0, 0.0, 10.0, 1.0), 50.0);
        // What bounds an expanded column is the measured text, which is cut to
        // EXPAND_CHARS before it is measured — not a fixed pixel clamp.
        assert_eq!(auto_fit_width(4000.0, 0.0, 10.0, 1.0), 4032.0);
    }

    #[test]
    fn expanding_measures_at_most_256_characters() {
        // A 5,000-character value measures the same as a 256-character one, so
        // one enormous cell cannot stretch the column past the limit.
        let long = "x".repeat(5000);
        assert_eq!(js::char_len(js::slice_chars(&long, EXPAND_CHARS)), EXPAND_CHARS);
        assert_eq!(js::slice_chars(&long, EXPAND_CHARS), &long[..EXPAND_CHARS]);
    }

    #[test]
    fn the_eye_button_sits_at_the_right_of_its_cell() {
        let mut g = GridState { font_scale: 1.0, col_widths: vec![100.0, 200.0], ..Default::default() };
        let rnw = row_num_width(10, 1.0);
        let eye = g.eye_at(0, 1, 10);
        // Second column: right edge less the cell padding and the button.
        assert_eq!(f32::from(eye.right()), rnw + 300.0 - BASE_CELL_PAD_X);
        assert_eq!(f32::from(eye.size.width), EYE_SIZE);
        // It travels with the scroll.
        g.scroll_x = 40.0;
        g.scroll_y = 15.0;
        let moved = g.eye_at(2, 1, 10);
        assert_eq!(f32::from(moved.left()), f32::from(eye.left()) - 40.0);
        assert_eq!(f32::from(moved.top()), f32::from(eye.top()) + 2.0 * BASE_ROW_HEIGHT - 15.0);
    }

    #[test]
    fn a_scrollbar_only_appears_when_the_content_overflows() {
        let fits = ScrollGeom::new(400.0, 300.0, 400.0, 300.0);
        assert!(!fits.has_v && !fits.has_h);
        assert_eq!((fits.view_w, fits.view_h), (400.0, 300.0));

        // Tall content: the vertical bar narrows the body, which is enough to
        // push the (only just fitting) width into overflowing too.
        let tall = ScrollGeom::new(400.0, 300.0, 395.0, 900.0);
        assert!(tall.has_v && tall.has_h);
        assert_eq!((tall.view_w, tall.view_h), (388.0, 288.0));
    }

    #[test]
    fn the_thumb_spans_the_visible_fraction_and_tracks_the_scroll() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 900.0);
        // No horizontal bar, so the body keeps its full height.
        let (top, len) = g.v_thumb(0.0);
        assert_eq!((top, len), (0.0, 100.0));
        // Scrolled to the end the thumb sits at the end of its travel.
        let (top, len) = g.v_thumb(g.max_scroll_y());
        assert_eq!((top, len), (200.0, 100.0));
        // Halfway down.
        assert_eq!(g.v_thumb(300.0).0, 100.0);
    }

    #[test]
    fn dragging_the_thumb_maps_back_to_the_scroll_offset() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 900.0);
        assert_eq!(g.scroll_for_v_thumb(0.0), 0.0);
        assert_eq!(g.scroll_for_v_thumb(100.0), 300.0);
        assert_eq!(g.scroll_for_v_thumb(200.0), g.max_scroll_y());
        // Past the end of the track the scroll saturates.
        assert_eq!(g.scroll_for_v_thumb(9999.0), g.max_scroll_y());
    }

    #[test]
    fn tiny_thumbs_keep_a_usable_length() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 300_000.0);
        assert_eq!(g.v_thumb(0.0).1, MIN_THUMB);
        // And the ends of the track still map to the ends of the content.
        assert_eq!(g.scroll_for_v_thumb(300.0 - MIN_THUMB), g.max_scroll_y());
    }

    /// A grid with `rows` rows of three 100px columns, showing ten rows.
    fn grid(rows: usize) -> (GridState, (f32, f32)) {
        let mut g = GridState { font_scale: 1.0, col_widths: vec![100.0; 3], ..Default::default() };
        g.sel = Some(CellSel { r0: 0, c0: 0, r1: 0, c1: 0 });
        g.last_selected = Some((0, 0));
        assert!(rows > 0);
        (g, (200.0, 10.0 * BASE_ROW_HEIGHT))
    }

    #[test]
    fn cmd_down_and_end_go_to_the_last_row() {
        let (mut g, view) = grid(100);
        g.jump_selection(Some(Jump::Last), None, false, 100, 3, view);
        let sel = g.sel.unwrap();
        assert_eq!((sel.r0, sel.r1), (99, 99), "a plain jump moves the whole selection");
        assert_eq!(sel.c1, 0, "the column is left alone");
        // The last row is scrolled into view, at the bottom of the body.
        assert_eq!(g.scroll_y, 90.0 * BASE_ROW_HEIGHT);
    }

    #[test]
    fn shift_cmd_down_selects_everything_below_and_lands_on_the_last_row() {
        let (mut g, view) = grid(100);
        g.jump_selection(None, None, false, 100, 3, view);
        g.move_selection(3, 0, false, 100, 3, view); // start on row 3
        g.jump_selection(Some(Jump::Last), None, true, 100, 3, view);
        let sel = g.sel.unwrap();
        assert_eq!(sel.norm(), (3, 99, 0, 0), "row 3 through the last row stays selected");
        assert_eq!(g.scroll_y, 90.0 * BASE_ROW_HEIGHT, "and the view follows the moving end");
    }

    #[test]
    fn home_returns_to_the_first_row() {
        let (mut g, view) = grid(100);
        g.jump_selection(Some(Jump::Last), None, false, 100, 3, view);
        g.jump_selection(Some(Jump::First), None, false, 100, 3, view);
        assert_eq!(g.sel.unwrap().norm(), (0, 0, 0, 0));
        assert_eq!(g.scroll_y, 0.0);
    }

    #[test]
    fn jumping_sideways_scrolls_the_column_into_view() {
        let (mut g, view) = grid(100);
        g.jump_selection(None, Some(Jump::Last), false, 100, 3, view);
        assert_eq!(g.sel.unwrap().c1, 2);
        assert!(g.scroll_x > 0.0, "the last column is brought into a 200px-wide body");
        g.jump_selection(None, Some(Jump::First), false, 100, 3, view);
        assert_eq!(g.sel.unwrap().c1, 0);
        assert_eq!(g.scroll_x, 0.0);
    }

    #[test]
    fn a_pill_radius_never_exceeds_the_box() {
        // gpui draws nothing when the radius overshoots the quad, so the
        // 6px-wide thumb has to ask for 3px, not `border-radius: 999px`.
        assert_eq!(pill_radius(6.0, 28.0), 3.0);
        assert_eq!(pill_radius(28.0, 6.0), 3.0);
        assert_eq!(pill_radius(6.0, 1.0), 0.5);
    }

    #[test]
    fn points_hit_the_bar_they_are_over() {
        let g = ScrollGeom::new(400.0, 300.0, 900.0, 900.0);
        assert_eq!(g.bar_at(point(px(200.), px(150.))), None);
        assert_eq!(g.bar_at(point(px(394.), px(150.))), Some(Bar::Vertical));
        assert_eq!(g.bar_at(point(px(200.), px(294.))), Some(Bar::Horizontal));
        // The corner square belongs to neither.
        assert_eq!(g.bar_at(point(px(394.), px(294.))), None);
    }
}
