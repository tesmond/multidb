//! The tab spinner (`.tab-spinner { animation: spin 1s linear infinite }`).
//!
//! The old UI rotated a `⟳` glyph with a CSS transform. GPUI can only
//! transform SVG sprites, not shaped text, so the glyph is redrawn as a path
//! (a stroked arc plus an arrow head) which can be rotated per frame.

use crate::ui::theme::{hsla, Rgba};
use gpui::{
    point, px, App, Bounds, Element, ElementId, GlobalElementId, IntoElement, LayoutId, PathBuilder, Pixels, Point,
    Style, Window,
};

pub struct Spinner {
    font_size: f32,
    line_height: f32,
    color: Rgba,
    /// Rotation in turns (0..1).
    angle: f32,
}

pub fn spinner(font_size: f32, line_height: f32, color: Rgba) -> Spinner {
    Spinner { font_size, line_height, color, angle: 0.0 }
}

impl Spinner {
    pub fn angle(mut self, turns: f32) -> Self {
        self.angle = turns;
        self
    }
}

impl IntoElement for Spinner {
    type Element = Self;
    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Spinner {
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
        style.size.width = px(self.font_size).into();
        style.size.height = px(self.line_height).into();
        style.flex_shrink = 0.;
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&gpui::InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut Window,
        _: &mut App,
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
        _: &mut App,
    ) {
        let fs = self.font_size;
        let center = bounds.center();
        let r = 0.34 * fs;
        let stroke = (0.08 * fs).max(1.0);
        let turn = self.angle * std::f32::consts::TAU;
        let at = |a: f32, radius: f32| -> Point<Pixels> {
            let a = a + turn;
            point(center.x + px(radius * a.cos()), center.y + px(radius * a.sin()))
        };

        // Arc from 300° round to 210° (the gap sits where the arrow head goes).
        let start = -std::f32::consts::FRAC_PI_3;
        let sweep = 1.75 * std::f32::consts::PI;
        let steps = 36;
        let mut pb = PathBuilder::stroke(px(stroke));
        pb.move_to(at(start, r));
        for i in 1..=steps {
            pb.line_to(at(start + sweep * i as f32 / steps as f32, r));
        }
        if let Ok(path) = pb.build() {
            window.paint_path(path, hsla(self.color));
        }

        // Arrow head at the end of the arc, pointing along the tangent.
        let end = start + sweep;
        let head = 0.34 * fs;
        let mut pb = PathBuilder::fill();
        pb.move_to(at(end, r + head / 2.0));
        pb.line_to(at(end, r - head / 2.0));
        pb.line_to(at(end + head / (2.0 * r), r));
        pb.close();
        if let Ok(path) = pb.build() {
            window.paint_path(path, hsla(self.color));
        }
    }
}
