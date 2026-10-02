//! Single-line text element with CSS `letter-spacing` and WebKit's baseline
//! placement (`round(ascent)` from the top of a `line-height: normal` box).

use crate::ui::theme::hsla;
use gpui::{
    point, px, size, App, Bounds, Element, ElementId, Font, GlobalElementId, IntoElement, LayoutId, Pixels, Rgba,
    ShapedLine, SharedString, Style, TextRun, Window,
};

pub struct SpacedText {
    text: SharedString,
    font: Font,
    font_size: f32,
    line_height: f32,
    color: Rgba,
    spacing: f32,
}

pub fn spaced_text(
    text: impl Into<SharedString>,
    font: Font,
    font_size: f32,
    line_height: f32,
    color: Rgba,
    spacing: f32,
) -> SpacedText {
    SpacedText { text: text.into(), font, font_size, line_height, color, spacing }
}

pub struct SpacedLayout {
    line: ShapedLine,
    width: Pixels,
}

impl IntoElement for SpacedText {
    type Element = Self;
    fn into_element(self) -> Self {
        self
    }
}

impl Element for SpacedText {
    type RequestLayoutState = SpacedLayout;
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
    ) -> (LayoutId, SpacedLayout) {
        let run = TextRun {
            len: self.text.len(),
            font: self.font.clone(),
            color: hsla(self.color),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let line = window.text_system().shape_line(self.text.clone(), px(self.font_size), &[run], None);
        let chars = self.text.chars().count() as f32;
        let width = line.width + px(self.spacing * chars);
        let mut style = Style::default();
        style.size.width = width.into();
        style.size.height = px(self.line_height).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), SpacedLayout { line, width })
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut SpacedLayout,
        _: &mut Window,
        _: &mut App,
    ) {
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        bounds: Bounds<Pixels>,
        layout: &mut SpacedLayout,
        _: &mut (),
        window: &mut Window,
        _: &mut App,
    ) {
        let _ = layout.width;
        let line = &layout.line;
        let a = f32::from(line.ascent).round();
        let d = f32::from(line.descent).abs().round();
        let baseline = bounds.top() + px((self.line_height - (a + d)) / 2.0 + a);
        let color = hsla(self.color);
        let mut i = 0usize;
        for run in &line.runs {
            for glyph in &run.glyphs {
                let origin = point(bounds.left() + glyph.position.x + px(self.spacing * i as f32), baseline);
                if glyph.is_emoji {
                    let _ = window.paint_emoji(origin, run.font_id, glyph.id, line.font_size);
                } else {
                    let _ = window.paint_glyph(origin, run.font_id, glyph.id, line.font_size, color);
                }
                i += 1;
            }
        }
        let _ = size(px(0.), px(0.));
    }
}
