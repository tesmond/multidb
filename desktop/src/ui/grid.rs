//! Virtualised results grid (port of `ResultsGrid.svelte`): header row built
//! from divs, body painted directly like the old `<canvas>` renderer.

use crate::ui::js;
use crate::ui::model::{pending_get, PendingEdits, QueryResult, SortDirection};
use crate::ui::textfmt::{escape_tsv_cell, format_value_for_clipboard};
use crate::ui::theme::{self, hsla, Rgba};
use gpui::{
    fill, point, px, quad, size, App, Bounds, BorderStyle, ContentMask, Corners, Edges, Element, ElementId, Entity,
    Font, FontStyle, FontWeight, GlobalElementId, IntoElement, LayoutId, Pixels, Point, SharedString, Style, TextRun,
    Window,
};
use std::collections::{BTreeSet, HashMap};
use std::sync::Arc;

pub const BASE_ROW_HEIGHT: f32 = 28.0;
pub const BASE_CELL_PAD_X: f32 = 10.0;
const EXPLAIN_DEFAULT_TEXT_LEN: usize = 120;
pub const SCROLLBAR: f32 = 12.0;

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

/// `calculateAutoFitColumnWidth` from lib/columnSizing.ts.
pub fn auto_fit_width(max_cell_text_width: f32, header_text_width: f32, pad: f32, scale: f32) -> f32 {
    let cell = max_cell_text_width * scale + pad * 2.0 + 12.0 * scale;
    let header = header_text_width * scale + pad * 2.0 + 28.0 * scale;
    cell.max(header).max(50.0 * scale).min(800.0 * scale)
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
                    let w = self.measure(&text, cx);
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
        let current = match self.sel {
            Some(s) => (s.r1, s.c1),
            None => self.last_selected.unwrap_or((0, 0)),
        };
        let next = (
            (current.0 as isize + dr).clamp(0, max_row) as usize,
            (current.1 as isize + dc).clamp(0, max_col) as usize,
        );
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
        // ensureRowVisible / ensureColVisible
        let rh = row_height(self.font_scale);
        let (w, h) = view;
        let visible_rows = (h / rh).floor().max(1.0) as usize;
        let start = (self.scroll_y / rh).floor() as usize;
        if next.0 < start {
            self.scroll_y = next.0 as f32 * rh;
        } else if next.0 > start + visible_rows - 1 {
            self.scroll_y = (next.0 + 1 - visible_rows) as f32 * rh;
        }
        let col_left: f32 = self.col_widths[..next.1].iter().sum();
        let col_right = col_left + self.col_widths.get(next.1).copied().unwrap_or(0.0);
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
        let mut has_v = content_h > oh;
        let mut has_h = content_w > ow - if has_v { SCROLLBAR } else { 0.0 };
        if has_h && !has_v {
            has_v = content_h > oh - SCROLLBAR;
        }
        if has_v && !has_h {
            has_h = content_w > ow - SCROLLBAR;
        }
        let w = ow - if has_v { SCROLLBAR } else { 0.0 };
        let h = oh - if has_h { SCROLLBAR } else { 0.0 };
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
        let thumb = theme::rgba8(236, 240, 248, 0.35);
        if has_v {
            let tr = Bounds::new(point(origin.x + px(w), origin.y), size(px(SCROLLBAR), px(h)));
            window.paint_quad(fill(tr, hsla(track)));
            let ratio = h / content_h;
            let th = (h * ratio).max(20.0);
            let ty = if content_h > h { (st / (content_h - h)) * (h - th) } else { 0.0 };
            let tb = Bounds::new(point(origin.x + px(w + 3.0), origin.y + px(ty + 3.0)), size(px(SCROLLBAR - 6.0), px(th - 6.0)));
            window.paint_quad(fill(tb, hsla(thumb)).corner_radii(px(999.)));
        }
        if has_h {
            let tr = Bounds::new(point(origin.x, origin.y + px(h)), size(px(w), px(SCROLLBAR)));
            window.paint_quad(fill(tr, hsla(track)));
            let ratio = w / content_w;
            let tw = (w * ratio).max(20.0);
            let tx = if content_w > w { (sl / (content_w - w)) * (w - tw) } else { 0.0 };
            let tb = Bounds::new(point(origin.x + px(tx + 3.0), origin.y + px(h + 3.0)), size(px(tw - 6.0), px(SCROLLBAR - 6.0)));
            window.paint_quad(fill(tb, hsla(thumb)).corner_radii(px(999.)));
        }
        if has_v && has_h {
            let corner = Bounds::new(point(origin.x + px(w), origin.y + px(h)), size(px(SCROLLBAR), px(SCROLLBAR)));
            window.paint_quad(fill(corner, hsla(theme::WHITE)));
        }
    }
}
