//! Scrollbars.
//!
//! The geometry (which bars are needed, how long the thumb is and where it
//! sits) lives here so that everything that scrolls — the results grid, the SQL
//! editor and the plain `overflow_y_scroll` lists — draws the same bar and
//! hit-tests it the same way. The look is the old stylesheet's:
//!
//! ```text
//! ::-webkit-scrollbar        { width: 12px; height: 12px }
//! ::-webkit-scrollbar-track  { background: rgba(255,255,255,.2) }
//! ::-webkit-scrollbar-thumb  { background: rgba(236,240,248,.35);
//!                              border: 3px solid rgba(255,255,255,.2);
//!                              border-radius: 999px }
//! ::-webkit-scrollbar-thumb:hover { background: rgba(246,249,255,.68) }
//! ```

use crate::ui::theme::{self, hsla, Rgba};
use gpui::{fill, point, px, size, Bounds, Pixels, Point, ScrollHandle, Window};

pub const SCROLLBAR: f32 = 12.0;
/// `::-webkit-scrollbar-thumb` sits inside a 3px border of the track colour.
pub const SCROLLBAR_INSET: f32 = 3.0;
/// Shortest the thumb is allowed to get, so there is always something big
/// enough to see and to grab however long the content is.
pub const MIN_THUMB: f32 = 28.0;

pub fn track_color() -> Rgba {
    theme::rgba8(255, 255, 255, 0.2)
}

pub fn thumb_color(active: bool) -> Rgba {
    if active {
        theme::rgba8(246, 249, 255, 0.68)
    } else {
        theme::rgba8(236, 240, 248, 0.35)
    }
}

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

/// A drag of one of the scrollbars belonging to a `ScrollHandle` list.
#[derive(Clone)]
pub struct HandleDrag {
    /// Which list, so only its own bar lights up.
    pub id: gpui::SharedString,
    pub handle: ScrollHandle,
    pub bar: Bar,
    pub grab: f32,
}

/// How big the content is, which scrollbars that needs, and therefore how much
/// room is left for the content itself. Painting and hit-testing both go
/// through this so a click lands on the thumb that was drawn.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ScrollGeom {
    /// Viewport size, i.e. the outer size minus whichever scrollbars are shown.
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

    /// Geometry for a `ScrollHandle` list, from what it measured last frame.
    /// Its `max_offset` is content minus viewport, and the bars it shows are
    /// already accounted for in the bounds it reports.
    pub fn of_handle(handle: &ScrollHandle) -> Self {
        let b = handle.bounds();
        let max = handle.max_offset();
        let (w, h): (f32, f32) = (b.size.width.into(), b.size.height.into());
        let (mx, my): (f32, f32) = (max.width.into(), max.height.into());
        ScrollGeom {
            view_w: w,
            view_h: h,
            content_w: w + mx.max(0.0),
            content_h: h + my.max(0.0),
            has_v: my > 0.5,
            has_h: mx > 0.5,
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

    /// The thumb's extent along `bar`, for the scroll offsets given.
    pub fn thumb_on(&self, bar: Bar, scroll: (f32, f32)) -> (f32, f32) {
        match bar {
            Bar::Vertical => self.v_thumb(scroll.1),
            Bar::Horizontal => self.h_thumb(scroll.0),
        }
    }

    /// One page along `bar`, for a click on the track.
    pub fn page_on(&self, bar: Bar) -> f32 {
        match bar {
            Bar::Vertical => self.view_h,
            Bar::Horizontal => self.view_w,
        }
    }

    /// Which scrollbar, if any, is under a point in content-local coordinates.
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

/// Paint the bars for a directly-rendered surface (the grid, the editor):
/// `origin` is the top-left of the whole scrolling area, `scroll` its current
/// `(x, y)` offset, and `active` whichever bar is hovered or being dragged.
pub fn paint_bars(window: &mut Window, origin: Point<Pixels>, geom: ScrollGeom, scroll: (f32, f32), active: Option<Bar>) {
    let (w, h) = (geom.view_w, geom.view_h);
    let inset = SCROLLBAR_INSET;
    let thickness = SCROLLBAR - 2.0 * inset;
    let track = track_color();
    if geom.has_v {
        let tr = Bounds::new(point(origin.x + px(w), origin.y), size(px(SCROLLBAR), px(h)));
        window.paint_quad(fill(tr, hsla(track)));
        let (ty, th) = geom.v_thumb(scroll.1);
        let len = (th - 2.0 * inset).max(1.0);
        let tb = Bounds::new(point(origin.x + px(w + inset), origin.y + px(ty + inset)), size(px(thickness), px(len)));
        let c = thumb_color(active == Some(Bar::Vertical));
        window.paint_quad(fill(tb, hsla(c)).corner_radii(px(pill_radius(thickness, len))));
    }
    if geom.has_h {
        let tr = Bounds::new(point(origin.x, origin.y + px(h)), size(px(w), px(SCROLLBAR)));
        window.paint_quad(fill(tr, hsla(track)));
        let (tx, tw) = geom.h_thumb(scroll.0);
        let len = (tw - 2.0 * inset).max(1.0);
        let tb = Bounds::new(point(origin.x + px(tx + inset), origin.y + px(h + inset)), size(px(len), px(thickness)));
        let c = thumb_color(active == Some(Bar::Horizontal));
        window.paint_quad(fill(tb, hsla(c)).corner_radii(px(pill_radius(len, thickness))));
    }
    if geom.has_v && geom.has_h {
        let corner = Bounds::new(point(origin.x + px(w), origin.y + px(h)), size(px(SCROLLBAR), px(SCROLLBAR)));
        window.paint_quad(fill(corner, hsla(theme::WHITE)));
    }
}

/// Where a press on a bar leaves things: either a drag that grabbed the thumb
/// at `grab` pixels from its near edge, or a new scroll offset one page
/// towards the click.
pub enum Press {
    Grabbed(f32),
    Paged(f32),
}

pub fn press(geom: ScrollGeom, bar: Bar, pos: f32, scroll: (f32, f32)) -> Press {
    let (start, len) = geom.thumb_on(bar, scroll);
    if pos >= start && pos < start + len {
        Press::Grabbed(pos - start)
    } else {
        let current = match bar {
            Bar::Vertical => scroll.1,
            Bar::Horizontal => scroll.0,
        };
        let page = geom.page_on(bar);
        Press::Paged(current + if pos < start { -page } else { page })
    }
}

/// The outer element of a scrolling pane, holding a single content child.
///
/// gpui takes a scroll container's content size from the union of its direct
/// children's layout bounds, so the child has to be *free to be bigger than
/// the pane on both axes* — and it is easy to take that freedom away by
/// accident. Both halves of this have broken a scrollbar once:
///
///   * a flex **column** stretches its children to the pane's width, so the
///     rows never exceed it and the horizontal bar never appears;
///   * the default `align-items: stretch` of this flex **row** stretches the
///     content column to the pane's height, so the content measures exactly
///     the viewport and the vertical bar never appears.
///
/// Hence: a row, with the child aligned to the start rather than stretched.
/// Use [`scroll_content`] for that child. The caller adds `overflow_scroll`
/// and the scroll handle, which need an element id and so are not available
/// here.
pub fn scroll_body<T: gpui::Styled>(el: T) -> T {
    el.flex().flex_row().items_start()
}

/// The single child of a [`scroll_body`]: the rows, taking their own total
/// height and the width of the widest of them, but never narrower than the
/// pane.
///
/// Deliberately a **block**, not a flex column. A flex column hands its
/// children a share of its own height and lets them shrink to fit, so the rows
/// compress to the pane instead of overflowing it and the vertical scrollbar
/// disappears; block layout stacks them at their natural height and lets the
/// total run past the bottom, which is what there is to scroll. `flex_none`
/// then keeps this block shrink-wrapped around the widest row rather than
/// stretched to the pane, and `min_w_full` keeps short rows full width so the
/// hover highlight still spans the pane.
pub fn scroll_content<T: gpui::Styled>(el: T) -> T {
    el.flex_none().min_w_full()
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{relative, AlignItems, Display, FlexDirection, Refineable, Style, StyleRefinement};

    /// Both scrollbars in the navigator have gone missing before, each time
    /// because the rows were stretched to the pane on one axis and so could
    /// not overflow it. Pin the two rules rather than the appearance.
    #[test]
    fn a_scrolling_pane_lets_its_content_outgrow_it_on_both_axes() {
        let mut body = Style::default();
        body.refine(&scroll_body(StyleRefinement::default()));
        // A row, so the content column keeps its natural width…
        assert_eq!(body.flex_direction, FlexDirection::Row);
        // …and not stretched, so it keeps its natural height. `None` means
        // taffy's default, which *is* stretch, so it has to be set.
        assert!(body.align_items.is_some(), "align-items must be set, or it defaults to stretch");
        assert_ne!(body.align_items, Some(AlignItems::Stretch), "a stretched child is exactly as tall as the pane");

        let mut content = Style::default();
        content.refine(&scroll_content(StyleRefinement::default()));
        // Block, not flex: a flex column shares its height out among the rows
        // and shrinks them to fit, which is exactly how the vertical scrollbar
        // went missing. Block layout stacks them at their natural height.
        assert_eq!(content.display, Display::Block);
        // Neither grown nor shrunk to fit the pane: the rows decide the width.
        assert_eq!(content.flex_grow, 0.0);
        assert_eq!(content.flex_shrink, 0.0);
        // But it still fills the pane when the rows are narrower than it.
        assert_eq!(content.min_size.width, relative(1.).into());
    }

    #[test]
    fn a_scrollbar_only_appears_when_the_content_overflows() {
        let fits = ScrollGeom::new(400.0, 300.0, 400.0, 300.0);
        assert!(!fits.has_v && !fits.has_h);
        assert_eq!((fits.view_w, fits.view_h), (400.0, 300.0));

        // Tall content: the vertical bar narrows the viewport, which is enough
        // to push the (only just fitting) width into overflowing too.
        let tall = ScrollGeom::new(400.0, 300.0, 395.0, 900.0);
        assert!(tall.has_v && tall.has_h);
        assert_eq!((tall.view_w, tall.view_h), (388.0, 288.0));
    }

    #[test]
    fn the_thumb_spans_the_visible_fraction_and_tracks_the_scroll() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 900.0);
        let (top, len) = g.v_thumb(0.0);
        assert_eq!((top, len), (0.0, 100.0));
        let (top, len) = g.v_thumb(g.max_scroll_y());
        assert_eq!((top, len), (200.0, 100.0));
        assert_eq!(g.v_thumb(300.0).0, 100.0);
    }

    #[test]
    fn dragging_the_thumb_maps_back_to_the_scroll_offset() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 900.0);
        assert_eq!(g.scroll_for_v_thumb(0.0), 0.0);
        assert_eq!(g.scroll_for_v_thumb(100.0), 300.0);
        assert_eq!(g.scroll_for_v_thumb(200.0), g.max_scroll_y());
        assert_eq!(g.scroll_for_v_thumb(9999.0), g.max_scroll_y());
    }

    #[test]
    fn tiny_thumbs_keep_a_usable_length() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 300_000.0);
        assert_eq!(g.v_thumb(0.0).1, MIN_THUMB);
        assert_eq!(g.scroll_for_v_thumb(300.0 - MIN_THUMB), g.max_scroll_y());
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

    #[test]
    fn a_pill_radius_never_exceeds_the_box() {
        assert_eq!(pill_radius(6.0, 28.0), 3.0);
        assert_eq!(pill_radius(28.0, 6.0), 3.0);
        assert_eq!(pill_radius(6.0, 1.0), 0.5);
    }

    #[test]
    fn pressing_the_track_pages_and_pressing_the_thumb_grabs_it() {
        let g = ScrollGeom::new(400.0, 300.0, 300.0, 900.0);
        // Thumb is the top 100px at rest.
        match press(g, Bar::Vertical, 40.0, (0.0, 0.0)) {
            Press::Grabbed(grab) => assert_eq!(grab, 40.0),
            Press::Paged(_) => panic!("a press on the thumb should grab it"),
        }
        match press(g, Bar::Vertical, 250.0, (0.0, 0.0)) {
            Press::Paged(to) => assert_eq!(to, 300.0),
            Press::Grabbed(_) => panic!("a press below the thumb should page down"),
        }
    }
}

/// What a press on a list's scrollbar reports: which bar, and how far along it.
pub type BarPress = (Bar, f32);
type PressFn = std::rc::Rc<dyn Fn(&BarPress, &mut Window, &mut gpui::App)>;
type MoveFn = std::rc::Rc<dyn Fn(&gpui::MouseMoveEvent, &mut Window, &mut gpui::App)>;
type EndFn = std::rc::Rc<dyn Fn(&gpui::MouseUpEvent, &mut Window, &mut gpui::App)>;

/// The overlay drawn on top of a `ScrollHandle` list: a track down the right
/// edge (and along the bottom when it scrolls sideways) with a draggable thumb.
/// `overflow_*_scroll` gives gpui lists wheel scrolling but no visible bar, so
/// this supplies one. The offsets it reads are last frame's, which is exactly
/// what the list is showing.
///
/// `on_move` and `on_end` continue and finish a thumb drag. They are wired to
/// the track as well as to the window root, because the track is `occlude`d:
/// gpui only delivers a move to an element whose hitbox is hovered, and an
/// occluding hitbox takes every element behind it — the root included — out of
/// the hover stack. A drag along a 12px track keeps the pointer *on* the track,
/// so without these the root sees nothing and the thumb stalls; between the two
/// the drag is followed whether the pointer is on the track or off it.
pub fn overlay_bars(
    handle: &ScrollHandle,
    active: Option<Bar>,
    on_press: impl Fn(&BarPress, &mut Window, &mut gpui::App) + 'static,
    on_move: impl Fn(&gpui::MouseMoveEvent, &mut Window, &mut gpui::App) + 'static,
    on_end: impl Fn(&gpui::MouseUpEvent, &mut Window, &mut gpui::App) + 'static,
) -> Option<gpui::AnyElement> {
    use gpui::{div, prelude::*, MouseButton, MouseDownEvent};

    let geom = ScrollGeom::of_handle(handle);
    if !geom.has_v && !geom.has_h {
        return None;
    }
    let bounds = handle.bounds();
    let offset = handle.offset();
    let scroll = (f32::from(offset.x).abs(), f32::from(offset.y).abs());
    let inset = SCROLLBAR_INSET;
    let thickness = SCROLLBAR - 2.0 * inset;
    let on_press = std::rc::Rc::new(on_press);
    let on_move: MoveFn = std::rc::Rc::new(on_move);
    let on_end: EndFn = std::rc::Rc::new(on_end);

    let make = |bar: Bar, on_press: PressFn, on_move: MoveFn, on_end: EndFn| {
        let vertical = bar == Bar::Vertical;
        let (pos, len) = geom.thumb_on(bar, scroll);
        let len = (len - 2.0 * inset).max(1.0);
        let thumb = div().absolute().bg(thumb_color(active == Some(bar))).rounded(px(pill_radius(thickness, len)));
        let thumb = if vertical {
            thumb.top(px(pos + inset)).left(px(inset)).w(px(thickness)).h(px(len))
        } else {
            thumb.left(px(pos + inset)).top(px(inset)).h(px(thickness)).w(px(len))
        };
        let track = div()
            .id(if vertical { "scrollbar-v" } else { "scrollbar-h" })
            .occlude()
            .absolute()
            .bg(track_color())
            .child(thumb);
        let track = if vertical {
            track.top_0().right_0().w(px(SCROLLBAR)).h(px(geom.view_h))
        } else {
            track.left_0().bottom_0().h(px(SCROLLBAR)).w(px(geom.view_w))
        };
        track
            .on_mouse_down(MouseButton::Left, move |e: &MouseDownEvent, window, cx| {
                // Distance along the track, which starts at the list's edge.
                let along = if vertical {
                    f32::from(e.position.y - bounds.top())
                } else {
                    f32::from(e.position.x - bounds.left())
                };
                on_press(&(bar, along), window, cx);
                cx.stop_propagation();
            })
            .on_mouse_move(move |e, window, cx| on_move(e, window, cx))
            .on_mouse_up(MouseButton::Left, move |e, window, cx| on_end(e, window, cx))
            .into_any_element()
    };

    let mut wrap = div().absolute().top_0().left_0().size_full();
    if geom.has_v {
        wrap = wrap.child(make(Bar::Vertical, on_press.clone(), on_move.clone(), on_end.clone()));
    }
    if geom.has_h {
        wrap = wrap.child(make(Bar::Horizontal, on_press.clone(), on_move.clone(), on_end.clone()));
    }
    Some(wrap.into_any_element())
}
