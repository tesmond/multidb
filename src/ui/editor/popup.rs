//! The autocompletion tooltip (`.cm-tooltip-autocomplete`) and the lint hover
//! tooltip (`.cm-tooltip-lint`), reproducing the CodeMirror base theme plus the
//! One Dark overrides used by the old UI:
//!
//! ```text
//! .cm-tooltip                 { background: #353a42; border: none; z-index: 500 }
//! .cm-tooltip-autocomplete ul { font-family: monospace; min-width: 250px;
//!                               max-width: min(700px, 95vw); max-height: 10em }
//! ul > li                     { padding: 1px 3px; line-height: 1.2 }
//! li[aria-selected]           { background: #2c313a; color: #abb2bf }
//! .cm-completionIcon          { font-size: 90%; width: .8em; padding-right: .6em;
//!                               opacity: .6; text-align: center }
//! .cm-completionDetail        { margin-left: .5em; font-style: italic }
//! .cm-completionMatchedText   { text-decoration: underline }
//! .cm-diagnostic              { padding: 3px 6px 3px 8px; margin-left: -1px }
//! .cm-diagnostic-error        { border-left: 5px solid #d11 }
//! ```

use super::{SqlEditor, MAX_RENDERED_OPTIONS};
use crate::ui::metrics;
use crate::ui::sql::complete::RankedOption;
use crate::ui::theme::{self, hsla};
use gpui::prelude::ParentElement as _;
use gpui::{
    anchored, deferred, fill, point, px, size, App, Bounds, ContentMask, DispatchPhase, Element, ElementId, Entity,
    Font, FontStyle, FontWeight, GlobalElementId, Hsla, IntoElement, LayoutId, MouseButton, MouseDownEvent, Pixels,
    ScrollWheelEvent, ShapedLine, SharedString, Style, TextRun, UnderlineStyle, Window,
};

/// WebKit's default fixed-pitch family, which is what `font-family: monospace`
/// resolves to in the WKWebView the old UI ran in.
pub const GENERIC_MONO: &str = "Courier";

const BG: u32 = 0x353a42;
const SELECTED_BG: u32 = 0x2c313a;
const IVORY: u32 = 0xabb2bf;
const MIN_WIDTH: f32 = 250.0;
const MAX_WIDTH: f32 = 700.0;
/// `max-height: 10em`.
const MAX_ROWS_EM: f32 = 10.0;

fn icon_glyph(kind: &str) -> Option<&'static str> {
    Some(match kind {
        "function" | "method" => "ƒ",
        "class" => "○",
        "interface" => "◌",
        "variable" => "𝑥",
        "constant" => "𝐶",
        "type" => "𝑡",
        "enum" => "∪",
        "property" => "□",
        "keyword" => "🔑\u{fe0e}",
        "namespace" => "▢",
        "text" => "abc",
        _ => return None,
    })
}

fn mono(family: &SharedString, italic: bool) -> Font {
    Font {
        family: family.clone(),
        features: Default::default(),
        fallbacks: None,
        weight: FontWeight::NORMAL,
        style: if italic { FontStyle::Italic } else { FontStyle::Normal },
    }
}

/// Half-leading + ascent of a `line-height: 1.2` line box, and its depth below
/// the baseline.
fn strut(font_size: f32) -> (f32, f32) {
    let asc = metrics::generic_ascent(font_size);
    let desc = metrics::generic_descent(font_size);
    let half_leading = (1.2 * font_size - (asc + desc)) / 2.0;
    (half_leading + asc, desc + half_leading)
}

/// Height of one `li` (padding 1px + line box + padding 1px).
pub fn row_height(font_scale: f32) -> f32 {
    let fs = 13.0 * font_scale;
    let (above, below) = strut(fs);
    let (i_above, i_below) = strut(0.9 * fs);
    2.0 + (above.max(i_above) + below.max(i_below))
}

pub fn max_list_height(font_scale: f32) -> f32 {
    MAX_ROWS_EM * 13.0 * font_scale
}

/// The window (`rangeAroundSelected`) of options CodeMirror renders.
pub fn rendered_range(total: usize, selected: usize, max: usize) -> (usize, usize) {
    if total <= max {
        return (0, total);
    }
    if selected <= (total >> 1) {
        let off = selected / max;
        (off * max, ((off + 1) * max).min(total))
    } else {
        let off = (total - selected).div_ceil(max);
        (total - off * max, total - (off - 1) * max)
    }
}

struct Row {
    icon: Option<ShapedLine>,
    label: ShapedLine,
    detail: Option<ShapedLine>,
    index: usize,
    selected: bool,
    width: f32,
}

pub struct PopupLayout {
    rows: Vec<Row>,
    width: f32,
    height: f32,
    row_h: f32,
    baseline: f32,
    icon_advance: f32,
    icon_center: f32,
    detail_gap: f32,
    scroll_top: f32,
}

fn shape(
    window: &mut Window,
    text: &str,
    font: &Font,
    font_size: f32,
    color: Hsla,
    underline: &[(usize, usize)],
) -> ShapedLine {
    let mut runs: Vec<TextRun> = Vec::new();
    let mut push = |len: usize, underlined: bool| {
        if len == 0 {
            return;
        }
        runs.push(TextRun {
            len,
            font: font.clone(),
            color,
            background_color: None,
            underline: underlined
                .then(|| UnderlineStyle { thickness: px(1.), color: Some(color), wavy: false }),
            strikethrough: None,
        });
    };
    if underline.is_empty() {
        push(text.len(), false);
    } else {
        // Matched ranges are char offsets into the label.
        let idx: Vec<usize> = text.char_indices().map(|(i, _)| i).chain([text.len()]).collect();
        let at = |c: usize| idx.get(c).copied().unwrap_or(text.len());
        let mut pos = 0usize;
        for (from, to) in underline {
            let (from, to) = (at(*from), at(*to));
            if from > pos {
                push(from - pos, false);
            }
            if to > from {
                push(to - from, true);
            }
            pos = to.max(pos);
        }
        if pos < text.len() {
            push(text.len() - pos, false);
        }
    }
    window.text_system().shape_line(text.to_string().into(), px(font_size), &runs, None)
}

/// Measures the tooltip and returns it positioned like CodeMirror's tooltip
/// plugin (below the completion's `from`, flipped above when it does not fit).
pub fn build(editor: &SqlEditor, entity: Entity<SqlEditor>, window: &mut Window, _cx: &mut App) -> Option<gpui::AnyElement> {
    let state = editor.completion.as_ref()?;
    let layout = editor.layout.as_ref()?;
    if state.options.is_empty() {
        return None;
    }
    let scale = editor.font_scale;
    let fs = 13.0 * scale;
    let icon_fs = 0.9 * fs;
    let family: SharedString = GENERIC_MONO.into();
    let font = mono(&family, false);
    let italic = mono(&family, true);

    let (above, below) = strut(fs);
    let (i_above, i_below) = strut(icon_fs);
    let content_h = above.max(i_above) + below.max(i_below);
    let row_h = content_h + 2.0;
    let baseline = 1.0 + above.max(i_above);
    let icon_advance = 1.4 * icon_fs; // width .8em + padding-right .6em
    let icon_center = 0.4 * icon_fs;
    let detail_gap = 0.5 * fs;

    let (from, to) = rendered_range(state.options.len(), state.selected, MAX_RENDERED_OPTIONS);
    let mut rows = Vec::with_capacity(to - from);
    let mut max_width: f32 = 0.0;
    for i in from..to {
        let opt: &RankedOption = &state.options[i];
        let selected = i == state.selected;
        let color = hsla(theme::hex(if selected { IVORY } else { 0xffffff }));
        let icon = icon_glyph(opt.completion.kind).map(|g| {
            let c = hsla(theme::with_alpha(theme::hex(if selected { IVORY } else { 0xffffff }), 0.6));
            shape(window, g, &font, icon_fs, c, &[])
        });
        let label = shape(window, &opt.completion.label, &font, fs, color, &opt.matched);
        let detail = opt
            .completion
            .detail
            .as_ref()
            .map(|d| shape(window, d, &italic, fs, color, &[]));
        let width = 3.0
            + icon.as_ref().map(|_| icon_advance).unwrap_or(0.0)
            + f32::from(label.width)
            + detail.as_ref().map(|d| detail_gap + f32::from(d.width)).unwrap_or(0.0)
            + 3.0;
        max_width = max_width.max(width);
        rows.push(Row { icon, label, detail, index: i, selected, width });
    }

    let viewport = window.viewport_size();
    let vw = f32::from(viewport.width);
    let vh = f32::from(viewport.height);
    let width = max_width.clamp(MIN_WIDTH, MAX_WIDTH.min(0.95 * vw));
    let content_height = rows.len() as f32 * row_h;
    let mut height = content_height.min(MAX_ROWS_EM * fs);
    let scroll_top = state.scroll_top.clamp(0.0, (content_height - height).max(0.0));

    // Anchor: coordsAtPos(min from of the active sources).
    let anchor = layout.bounds_for_offset(&editor.buffer, state.anchor.min(editor.buffer.len()), editor.scroll);
    let left = f32::from(anchor.left()).min(vw - width).max(0.0);
    let (pos_top, pos_bottom) = (f32::from(anchor.top()), f32::from(anchor.bottom()));
    let mut place_above = false;
    if pos_bottom + height > vh && vh - pos_bottom < pos_top {
        place_above = true;
    }
    let space_vert = if place_above { pos_top } else { vh - pos_bottom };
    if space_vert < height {
        if space_vert < row_h {
            return None;
        }
        height = space_vert;
    }
    let top = if place_above { pos_top - height } else { pos_bottom };

    let layout = PopupLayout {
        rows,
        width,
        height,
        row_h,
        baseline,
        icon_advance,
        icon_center,
        detail_gap,
        scroll_top,
    };
    Some(
        deferred(
            anchored()
                .position(point(px(left), px(top)))
                .child(PopupElement { editor: entity, layout }),
        )
        .with_priority(1)
        .into_any_element(),
    )
}

pub struct PopupElement {
    editor: Entity<SqlEditor>,
    layout: PopupLayout,
}

impl IntoElement for PopupElement {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for PopupElement {
    type RequestLayoutState = ();
    type PrepaintState = ();

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
        style.size.width = px(self.layout.width).into();
        style.size.height = px(self.layout.height).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _bounds: Bounds<Pixels>,
        _: &mut (),
        _window: &mut Window,
        _cx: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let l = &self.layout;
        window.paint_quad(fill(bounds, hsla(theme::hex(BG))));
        let mut rows: Vec<(usize, Bounds<Pixels>)> = Vec::with_capacity(l.rows.len());
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for (n, row) in l.rows.iter().enumerate() {
                let top = bounds.top() + px(n as f32 * l.row_h - l.scroll_top);
                let row_bounds = Bounds::new(point(bounds.left(), top), size(bounds.size.width, px(l.row_h)));
                rows.push((row.index, row_bounds));
                if top + px(l.row_h) < bounds.top() || top > bounds.bottom() {
                    continue;
                }
                if row.selected {
                    window.paint_quad(fill(row_bounds, hsla(theme::hex(SELECTED_BG))));
                }
                let mut x = bounds.left() + px(3.);
                if let Some(icon) = &row.icon {
                    let ix = x + px(l.icon_center) - icon.width / 2.;
                    icon.paint(point(ix, top + px(l.baseline) - icon.ascent), px(l.row_h), window, cx).ok();
                    x += px(l.icon_advance);
                }
                row.label
                    .paint(point(x, top + px(l.baseline) - row.label.ascent), px(l.row_h), window, cx)
                    .ok();
                if let Some(detail) = &row.detail {
                    let dx = x + row.label.width + px(l.detail_gap);
                    detail.paint(point(dx, top + px(l.baseline) - detail.ascent), px(l.row_h), window, cx).ok();
                }
                let _ = row.width;
            }
        });

        // Clicking an option applies it; the wheel scrolls the list. Both are
        // handled here so the tooltip keeps working where it overlaps panes
        // outside the editor.
        let editor = self.editor.clone();
        let hit = rows.clone();
        window.on_mouse_event(move |event: &MouseDownEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || event.button != MouseButton::Left || !bounds.contains(&event.position) {
                return;
            }
            if let Some((index, _)) = hit.iter().find(|(_, b)| b.contains(&event.position)) {
                let index = *index;
                editor.update(cx, |editor, cx| {
                    editor.accept_completion(index, cx);
                });
                window.refresh();
            }
            cx.stop_propagation();
        });
        let editor = self.editor.clone();
        let line_height = px(l.row_h);
        let max_scroll = (l.rows.len() as f32 * l.row_h - f32::from(bounds.size.height)).max(0.0);
        window.on_mouse_event(move |event: &ScrollWheelEvent, phase, window, cx| {
            if phase != DispatchPhase::Capture || !bounds.contains(&event.position) {
                return;
            }
            let delta = event.delta.pixel_delta(line_height);
            editor.update(cx, |editor, cx| {
                if let Some(state) = &mut editor.completion {
                    state.scroll_top = (state.scroll_top - f32::from(delta.y)).clamp(0.0, max_scroll);
                    cx.notify();
                }
            });
            window.refresh();
            cx.stop_propagation();
        });
    }
}

// ─── Lint hover tooltip ─────────────────────────────────────────────────────

/// `hoverTooltip` delay before the lint tooltip appears.
pub const HOVER_TIME: u64 = 300;

/// Builds the diagnostic tooltip shown above the hovered lint range (or below a
/// hovered gutter marker).
pub fn lint_tooltip(
    messages: &[String],
    anchor: Bounds<Pixels>,
    above: bool,
    font_scale: f32,
    window: &mut Window,
    _cx: &mut App,
) -> gpui::AnyElement {
    use gpui::{div, prelude::*};
    let fs = 13.0 * font_scale;
    let lh = metrics::line_height_normal(fs);
    let viewport = window.viewport_size();
    let body = div()
        .bg(hsla(theme::hex(BG)))
        .max_w(px(f32::from(viewport.width) * 0.95))
        .children(messages.iter().map(|m| {
            let text: SharedString = m.clone().into();
            div()
                .border_l(px(5.))
                .border_color(hsla(theme::hex(0xdd1111)))
                .ml(px(-1.))
                .pt(px(3.))
                .pb(px(3.))
                .pl(px(8.))
                .pr(px(6.))
                .font_family(theme::UI_FONT)
                .text_size(px(fs))
                .line_height(px(lh))
                .text_color(hsla(theme::WHITE))
                .child(text)
        }));
    let left = f32::from(anchor.left()).min(f32::from(viewport.width) - 200.0).max(0.0);
    let y = if above { anchor.top() } else { anchor.bottom() };
    deferred(
        anchored()
            .position(point(px(left), y))
            .anchor(if above { gpui::Corner::BottomLeft } else { gpui::Corner::TopLeft })
            .child(body),
    )
    .with_priority(1)
    .into_any_element()
}
