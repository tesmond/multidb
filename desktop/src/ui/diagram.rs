//! Relationship viewer tab (`RelationshipDiagram.svelte`).
//!
//! Everything derived from the schema — the graph, its lookup index, the
//! schema hash and the default layout — is computed once per schema (see
//! [`DiagramCache`]) and the filtered graph once per filter, rather than on
//! every frame as before, when each mouse move, pan step and caret blink
//! rebuilt and re-hashed the whole schema.
//!
//! Drawing is limited to what is on screen: only cards that intersect the
//! viewport become elements, and of those only the column rows in view; only
//! edges whose bounds cross the viewport are stroked. Zoomed far out the cards
//! are painted as plain boxes (with their titles while those are still
//! legible) instead of being built from elements at all.

use crate::models::SchemaTree;
use crate::ui::relationship::{self as rel, DiagramGraph, GraphIndex, Layout, Point as LPoint};
use crate::ui::theme::{self, hsla, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{shadow, TextExt};
use crate::ui::workspace::{Tab, TabKind, Workspace};
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, App, Bounds, Context, CursorStyle, DispatchPhase, Entity, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, ScrollDelta, ScrollWheelEvent, SharedString,
    Subscription, Window,
};
use std::rc::Rc;
use std::sync::Arc;

/// What the diagram derives from one schema. Rebuilt only when the
/// connection's schema is replaced.
pub struct DiagramCache {
    /// Identity of the schema this was built from (`Arc::as_ptr`).
    schema: usize,
    pub graph: DiagramGraph,
    pub index: GraphIndex,
    pub hash: String,
    pub default_layout: Layout,
    /// Schema hash plus table ids: when it changes the layout is re-merged.
    pub key: String,
}

impl DiagramCache {
    fn build(schema: Option<&Arc<SchemaTree>>) -> Self {
        let graph = schema.map(|s| rel::build_graph(s)).unwrap_or_default();
        let hash = schema.map(|s| rel::schema_hash(s)).unwrap_or_default();
        let key = if hash.is_empty() {
            String::new()
        } else {
            format!("{hash}:{}", graph.tables.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join("|"))
        };
        DiagramCache {
            schema: schema_id(schema),
            index: GraphIndex::new(&graph),
            default_layout: rel::default_layout(&graph),
            graph,
            hash,
            key,
        }
    }
}

/// The graph after the filter, rebuilt only when the filter text changes.
pub struct VisibleGraph {
    schema: usize,
    filter: String,
    pub graph: DiagramGraph,
}

fn schema_id(schema: Option<&Arc<SchemaTree>>) -> usize {
    schema.map(|s| Arc::as_ptr(s) as usize).unwrap_or(0)
}

pub struct DiagramState {
    pub pan: (f32, f32),
    pub zoom: f32,
    pub layout: Layout,
    pub layout_key: String,
    pub hovered_edge: String,
    pub selected_edge: String,
    pub filter: Entity<TextInput>,
    pub drag_table: Option<(String, gpui::Point<Pixels>, LPoint)>,
    pub panning: Option<(gpui::Point<Pixels>, (f32, f32))>,
    pub canvas_bounds: Option<Bounds<Pixels>>,
    cache: Option<Rc<DiagramCache>>,
    visible: Option<Rc<VisibleGraph>>,
    _sub: Subscription,
}

/// rem = 16px (the document root font size in the old UI).
const REM: f32 = 16.0;
/// Zoom limits. The lower one is well below the old 50% so that a large
/// schema can be taken in whole.
const MIN_ZOOM: f32 = 0.1;
const MAX_ZOOM: f32 = 2.5;
/// Below this zoom cards are painted as boxes instead of built from elements:
/// their text is too small to read and there can be hundreds on screen.
const DETAIL_ZOOM: f32 = 0.4;
/// Titles on painted cards are drawn only while at least this many pixels.
const MIN_TITLE_PX: f32 = 5.0;
/// Screen-space margin around the viewport inside which things are still
/// drawn, so a pan of a few pixels never exposes an undrawn edge.
const OVERDRAW_PX: f32 = 120.0;

impl DiagramState {
    pub fn new(window: &mut Window, cx: &mut Context<Workspace>) -> Self {
        let look = InputLook {
            font_size: 13.333,
            line_height: crate::ui::metrics::line_height_normal(13.333),
            height: crate::ui::metrics::line_height_normal(13.333) + 16.0,
            radius: 6.0,
            bg: theme::BG_SURFACE,
            ..InputLook::dialog(1.0)
        };
        let filter = cx.new(|cx| TextInput::new(cx, look).with_placeholder("Filter tables or columns"));
        let sub = cx.subscribe_in(&filter, window, |_ws, _i, ev: &InputEvent, _w, cx| {
            if matches!(ev, InputEvent::Changed) {
                cx.notify();
            }
        });
        DiagramState {
            pan: (40.0, 32.0),
            zoom: 1.0,
            layout: Layout::new(),
            layout_key: String::new(),
            hovered_edge: String::new(),
            selected_edge: String::new(),
            filter,
            drag_table: None,
            panning: None,
            canvas_bounds: None,
            cache: None,
            visible: None,
            _sub: sub,
        }
    }
}

fn clamp_zoom(v: f32) -> f32 {
    if v.is_finite() {
        v.clamp(MIN_ZOOM, MAX_ZOOM)
    } else {
        1.0
    }
}

/// A rectangle in diagram space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WorldRect {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl WorldRect {
    pub fn intersects(&self, x: f32, y: f32, w: f32, h: f32) -> bool {
        x < self.x1 && x + w > self.x0 && y < self.y1 && y + h > self.y0
    }

    /// The part of the diagram a `view_w`×`view_h` canvas shows at this pan
    /// and zoom, grown by `margin` screen pixels on every side.
    pub fn of_view(view_w: f32, view_h: f32, pan: (f32, f32), zoom: f32, margin: f32) -> Self {
        WorldRect {
            x0: (-margin - pan.0) / zoom,
            y0: (-margin - pan.1) / zoom,
            x1: (view_w + margin - pan.0) / zoom,
            y1: (view_h + margin - pan.1) / zoom,
        }
    }

    fn of_points(pts: &[(f32, f32)]) -> Option<Self> {
        let first = pts.first()?;
        let mut r = WorldRect { x0: first.0, y0: first.1, x1: first.0, y1: first.1 };
        for p in pts {
            r.x0 = r.x0.min(p.0);
            r.y0 = r.y0.min(p.1);
            r.x1 = r.x1.max(p.0);
            r.y1 = r.y1.max(p.1);
        }
        Some(r)
    }
}

/// The column rows of a card at `card_y` with `rows` rows that fall inside
/// `view` (diagram space).
pub fn visible_rows(card_y: f32, rows: usize, view: &WorldRect) -> std::ops::Range<usize> {
    let top = card_y + rel::CARD_BORDER + rel::HEADER_HEIGHT;
    let first = ((view.y0 - top) / rel::ROW_HEIGHT).floor().max(0.0) as usize;
    let last = ((view.y1 - top) / rel::ROW_HEIGHT).ceil().max(0.0) as usize;
    first.min(rows)..last.min(rows)
}

impl Workspace {
    pub fn show_relationships(&mut self, conn_id: &str, window: &mut Window, cx: &mut Context<Self>) {
        if self.connection(conn_id).is_none() {
            self.status = "Connection not found.".into();
            cx.notify();
            return;
        }
        let conn_id = conn_id.to_string();
        let load = self.load_cached_schema(&conn_id, cx);
        cx.spawn_in(window, async move |this, cx| {
            load.await;
            let refresh = this
                .update(cx, |this, cx| {
                    let has = this.connection(&conn_id).is_some_and(|c| c.schema.is_some());
                    (!has).then(|| this.refresh_schema(&conn_id, cx))
                })
                .ok()
                .flatten();
            if let Some(t) = refresh {
                t.await;
            }
            this.update_in(cx, move |this, window, cx| {
                let Some(conn) = this.connection(&conn_id) else { return };
                if conn.schema.is_none() {
                    this.status = "Schema is not available for this connection.".into();
                    cx.notify();
                    return;
                }
                let name = conn.config.name.clone();
                this.selected_conn_id = conn_id.clone();
                if let Some(t) = this.tabs.iter().find(|t| t.conn_id == conn_id && matches!(t.kind, TabKind::Diagram(_))) {
                    let id = t.id.clone();
                    this.set_active_tab(id, cx);
                    return;
                }
                let state = DiagramState::new(window, cx);
                let tab = Tab { id: crate::ui::model::new_id(), title: format!("{name} Relationships"), conn_id: conn_id.clone(), manually_renamed: false, kind: TabKind::Diagram(state) };
                let id = tab.id.clone();
                this.tabs.push(tab);
                this.set_active_tab(id, cx);
            })
            .ok();
        })
        .detach();
    }
}

impl Workspace {
    /// Bring the active diagram tab's cached graph and filtered graph up to
    /// date, re-merging its layout when the schema changed. Cheap when nothing
    /// changed: two pointer comparisons and a string comparison.
    fn diagram_prepare(&mut self, tab_id: &str, cx: &App) -> Option<(Rc<DiagramCache>, Rc<VisibleGraph>)> {
        let conn_id = self.tab(tab_id)?.conn_id.clone();
        let schema = self.connection(&conn_id)?.schema.clone();
        let id = schema_id(schema.as_ref());
        let store_key_for = |hash: &str| rel::LayoutStore::key(&conn_id, hash);

        // Graph, index, hash and default layout: once per schema.
        let stale = match self.tab(tab_id).map(|t| &t.kind) {
            Some(TabKind::Diagram(st)) => st.cache.as_ref().is_none_or(|c| c.schema != id),
            _ => return None,
        };
        if stale {
            let cache = Rc::new(DiagramCache::build(schema.as_ref()));
            if let Some(TabKind::Diagram(st)) = self.tab_mut(tab_id).map(|t| &mut t.kind) {
                st.cache = Some(cache);
                st.visible = None;
            }
        }
        let cache = match self.tab(tab_id).map(|t| &t.kind) {
            Some(TabKind::Diagram(st)) => st.cache.clone()?,
            _ => return None,
        };

        // Layout: merged again only when the set of tables or the schema changed.
        let persisted = self.settings.relationship_layouts.get(&store_key_for(&cache.hash)).cloned();
        let layout_key = format!("{conn_id}:{}", cache.key);
        let mut save_layout: Option<Layout> = None;
        if let Some(TabKind::Diagram(st)) = self.tab_mut(tab_id).map(|t| &mut t.kind) {
            let layout_key = if cache.key.is_empty() { String::new() } else { layout_key };
            if st.layout_key != layout_key {
                let prev = std::mem::take(&mut st.layout);
                let had_prev = !prev.is_empty();
                st.layout_key = layout_key;
                let carry = persisted.or(if had_prev { Some(prev) } else { None });
                st.layout = rel::merge_layout(&cache.graph, &cache.default_layout, carry.as_ref());
                if !cache.hash.is_empty() && had_prev {
                    save_layout = Some(st.layout.clone());
                }
                if !cache.graph.edges.iter().any(|e| e.id == st.selected_edge) {
                    st.selected_edge = cache.graph.edges.first().map(|e| e.id.clone()).unwrap_or_default();
                }
            }
        }
        if let Some(l) = save_layout {
            self.settings.relationship_layouts.insert(store_key_for(&cache.hash), l);
            self.settings.save();
        }

        // Filtered graph: once per filter text.
        let TabKind::Diagram(st) = &mut self.tab_mut(tab_id)?.kind else { return None };
        let filter = st.filter.read(cx).text().to_string();
        if st.visible.as_ref().is_none_or(|v| v.schema != id || v.filter != filter) {
            let graph = rel::filter_graph(&cache.graph, &filter);
            st.visible = Some(Rc::new(VisibleGraph { schema: id, filter, graph }));
        }
        Some((cache, st.visible.clone()?))
    }

    pub fn render_diagram(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab_id = self.active_tab_id.clone();
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let Some(conn_name) = self.connection(&conn_id).map(|c| c.config.name.clone()) else {
            return div().text_color(theme::TEXT_MUTED).child("Relationship viewer unavailable.").into_any_element();
        };
        let Some((cache, vis)) = self.diagram_prepare(&tab_id, cx) else { return div().into_any_element() };
        let Some(TabKind::Diagram(st)) = self.tab(&tab_id).map(|t| &t.kind) else { return div().into_any_element() };
        let graph = &cache.graph;
        let visible = &vis.graph;
        let effective = rel::effective_selected_edge_id(&visible.edges, &st.selected_edge);
        let selected = visible.edges.iter().find(|e| e.id == effective).cloned();
        let active_edge = if st.hovered_edge.is_empty() { effective.clone() } else { st.hovered_edge.clone() };
        let highlighted: std::collections::HashSet<String> = visible
            .edges
            .iter()
            .find(|e| e.id == active_edge)
            .map(|e| e.source_column_ids.iter().chain(e.target_column_ids.iter()).cloned().collect())
            .unwrap_or_default();
        let (pan, zoom) = (st.pan, st.zoom);
        let filter_input = st.filter.clone();

        // What is on screen, in diagram space. Before the first layout there
        // are no canvas bounds yet; the window size is a safe over-estimate.
        let (view_w, view_h) = match st.canvas_bounds {
            Some(b) => (f32::from(b.size.width), f32::from(b.size.height)),
            None => (f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)),
        };
        let view = WorldRect::of_view(view_w, view_h, pan, zoom, OVERDRAW_PX);
        let detailed = zoom >= DETAIL_ZOOM;

        let tool_btn = |id: &'static str, label: &'static str| {
            div()
                .id(id)
                .border_1()
                .border_color(theme::BORDER)
                .bg(theme::BG_SURFACE)
                .text_color(theme::TEXT)
                .rounded(px(6.))
                .px(px(10.))
                .py(px(6.))
                .t_rem(13.333)
                .cursor_pointer()
                .child(label)
        };
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_PANEL)
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .flex_1()
                    .min_w(px(0.))
                    .child(div().font_weight(FontWeight::SEMIBOLD).child("Relationship Viewer"))
                    .child(div().text_color(theme::TEXT_MUTED).text_size(px(13.0 * 0.92)).child(conn_name.clone()))
                    .child(div().min_w(px(220.)).max_w(px(320.)).w_full().child(filter_input)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(10.))
                    .child(tool_btn("zoom-out", "−").on_click(cx.listener(|this, _, _, cx| this.diagram_zoom_by(0.9, cx))))
                    .child(div().text_color(theme::TEXT_MUTED).text_size(px(13.0 * 0.92)).child(format!("{}%", (zoom * 100.0).round() as i32)))
                    .child(tool_btn("zoom-in", "+").on_click(cx.listener(|this, _, _, cx| this.diagram_zoom_by(1.1, cx))))
                    .child(tool_btn("reset-layout", "Reset Layout").on_click(cx.listener(|this, _, _, cx| this.diagram_reset(cx)))),
            );

        // Edges that cross the viewport, as screen-space polylines relative to
        // the canvas origin.
        let mut edge_lines: Vec<(Vec<(f32, f32)>, bool)> = Vec::new();
        for e in &visible.edges {
            let pts = rel::edge_points(graph, &cache.index, &st.layout, e);
            let Some(bb) = WorldRect::of_points(&pts) else { continue };
            // Grow by the arrowhead so one pointing into view is kept.
            if !view.intersects(bb.x0 - 12.0, bb.y0 - 12.0, bb.x1 - bb.x0 + 24.0, bb.y1 - bb.y0 + 24.0) {
                continue;
            }
            let active = e.id == st.hovered_edge || e.id == st.selected_edge;
            edge_lines.push((pts.iter().map(|p| (pan.0 + p.0 * zoom, pan.1 + p.1 * zoom)).collect(), active));
        }

        // Cards that intersect the viewport.
        let on_screen: Vec<(&rel::DiagramTable, LPoint)> = visible
            .tables
            .iter()
            .filter_map(|t| {
                let p = st.layout.get(&t.id).copied().unwrap_or_default();
                view.intersects(p.x, p.y, t.width, t.height).then_some((t, p))
            })
            .collect();

        // Zoomed out: cards are painted, not built.
        let painted_cards: Vec<(Bounds<Pixels>, SharedString)> = if detailed {
            Vec::new()
        } else {
            on_screen
                .iter()
                .map(|(t, p)| {
                    let b = Bounds::new(
                        point(px(pan.0 + p.x * zoom), px(pan.1 + p.y * zoom)),
                        gpui::size(px(t.width * zoom), px(t.height * zoom)),
                    );
                    (b, SharedString::from(t.title.clone()))
                })
                .collect()
        };

        let edge_canvas = canvas(
            |_, _, _| (),
            move |b, _, window, cx| {
                for (pts, active) in &edge_lines {
                    let color = if *active { theme::ACCENT_HOVER } else { theme::rgba8(129, 140, 248, 0.65) };
                    let width = if *active { 3.5 } else { 2.5 };
                    paint_edge(window, b, pts, zoom, color, width);
                }
                paint_cards(window, cx, b, &painted_cards, zoom);
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut stage = div().absolute().top_0().left_0().size_full().child(edge_canvas);
        if detailed {
            for (t, p) in &on_screen {
                stage = stage.child(self.render_card(t, *p, pan, zoom, &view, &highlighted, cx));
            }
        }

        let notice = if graph.edges.is_empty() {
            Some("No foreign-key relationships found.")
        } else if visible.tables.is_empty() {
            Some("No tables match the current filter.")
        } else {
            None
        };
        let ws = cx.entity().downgrade();
        let ws_drag = ws.clone();
        let canvas_area = div()
            .id("diagram-canvas")
            .relative()
            .flex_1()
            .min_w(px(0.))
            .overflow_hidden()
            .cursor(CursorStyle::OpenHand)
            .child(
                canvas(
                    move |b, _, cx| {
                        ws.update(cx, |ws, _| {
                            if let Some(TabKind::Diagram(st)) = ws.active_tab_mut().map(|t| &mut t.kind) {
                                st.canvas_bounds = Some(b);
                            }
                        })
                        .ok();
                    },
                    move |b, _, window, _| {
                        paint_dots(window, b);
                        // A table drag or a pan follows the pointer wherever it
                        // goes — over the cards, which occlude this canvas, or
                        // out of the pane — so it listens at window level
                        // rather than on the canvas element.
                        let (on_move, on_up) = (ws_drag.clone(), ws_drag.clone());
                        window.on_mouse_event(move |e: &MouseMoveEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble {
                                on_move.update(cx, |this, cx| this.diagram_drag_move(e, cx)).ok();
                            }
                        });
                        window.on_mouse_event(move |e: &MouseUpEvent, phase, _, cx| {
                            if phase == DispatchPhase::Bubble && e.button == MouseButton::Left {
                                on_up.update(cx, |this, cx| this.diagram_mouse_up(cx)).ok();
                            }
                        });
                    },
                )
                .absolute()
                .top_0()
                .left_0()
                .size_full(),
            )
            .child(stage)
            .when_some(notice, |d, n| {
                d.child(
                    div()
                        .absolute()
                        .left(px(16.))
                        .bottom(px(16.))
                        .px(px(12.))
                        .py(px(10.))
                        .border_1()
                        .border_color(theme::BORDER)
                        .rounded(px(8.))
                        .bg(theme::rgba8(26, 26, 36, 0.92))
                        .text_color(theme::TEXT_MUTED)
                        .child(n),
                )
            })
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| this.diagram_mouse_down(e, cx)))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.diagram_hover(e, cx)))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.diagram_scroll(e, cx)));

        let inspector = {
            let mut panel = div()
                .id("inspector")
                .w(px(320.))
                .flex_shrink_0()
                .border_l_1()
                .border_color(theme::BORDER)
                .bg(theme::BG_PANEL)
                .p(px(14.))
                .overflow_y_scroll()
                .child(div().mb(px(12.)).font_weight(FontWeight::BOLD).text_size(px(0.96 * REM)).line_height(px(crate::ui::metrics::line_height_normal(0.96 * REM))).child("Relationship details"));
            match selected {
                Some(e) => {
                    let row = |label: &str, value: String, multi: bool| {
                        div()
                            .flex()
                            .justify_between()
                            .gap(px(12.))
                            .text_size(px(0.9 * REM))
                            .line_height(px(crate::ui::metrics::line_height_normal(0.9 * REM)))
                            .when(multi, |d| d.items_start())
                            .child(div().flex_shrink_0().text_color(theme::TEXT_MUTED).child(label.to_string()))
                            .child(div().child(value))
                    };
                    panel = panel.child(
                        div()
                            .flex()
                            .flex_col()
                            .gap(px(10.))
                            .p(px(12.))
                            .border_1()
                            .border_color(theme::BORDER)
                            .rounded(px(10.))
                            .bg(theme::BG_SURFACE)
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(e.constraint_name.clone()))
                            .child(row("From", e.source_table_id.clone(), false))
                            .child(row("To", e.target_table_id.clone(), false))
                            .child(row("Columns", format!("{} → {}", e.source_column_ids.join(", "), e.target_column_ids.join(", ")), true))
                            .child(row("On update", if e.on_update.is_empty() { "—".into() } else { e.on_update.clone() }, false))
                            .child(row("On delete", if e.on_delete.is_empty() { "—".into() } else { e.on_delete.clone() }, false)),
                    );
                }
                None => panel = panel.child(div().text_color(theme::TEXT_MUTED).child("Select a relationship to inspect it.")),
            }
            panel
        };
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::BG_EDITOR)
            .child(toolbar)
            .child(div().flex().flex_1().min_h(px(0.)).child(canvas_area).child(inspector))
            .into_any_element()
    }

    /// One table card, sized exactly as the layout assumes (see
    /// `rel::card_height`), with only the column rows inside `view` built.
    #[allow(clippy::too_many_arguments)]
    fn render_card(
        &self,
        t: &rel::DiagramTable,
        p: LPoint,
        pan: (f32, f32),
        zoom: f32,
        view: &WorldRect,
        highlighted: &std::collections::HashSet<String>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let x = pan.0 + p.x * zoom;
        let y = pan.1 + p.y * zoom;
        let tid = t.id.clone();
        let (schema_name, table_name) = (t.schema_name.clone(), t.table_name.clone());
        let rows = visible_rows(p.y, t.columns.len(), view);
        let mut columns = div().flex().flex_col();
        if rows.start > 0 {
            columns = columns.child(div().flex_shrink_0().h(px(rows.start as f32 * rel::ROW_HEIGHT * zoom)));
        }
        for c in &t.columns[rows] {
            columns = columns.child(
                div()
                    .flex_shrink_0()
                    .h(px(rel::ROW_HEIGHT * zoom))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8. * zoom))
                    .px(px(12. * zoom))
                    .border_t(px(zoom))
                    .border_color(theme::rgba8(255, 255, 255, 0.04))
                    .when(highlighted.contains(&c.id), |d| d.bg(theme::rgba8(129, 140, 248, 0.15)))
                    .text_size(px(13. * zoom))
                    .line_height(px(crate::ui::metrics::line_height_normal(13. * zoom)))
                    .child(div().flex_1().min_w(px(0.)).overflow_hidden().whitespace_nowrap().text_ellipsis().child(c.name.clone()))
                    .child(
                        div()
                            .flex()
                            .flex_shrink_0()
                            .gap(px(4. * zoom))
                            .when(c.is_primary_key, |d| d.child(badge("PK", zoom, theme::rgba8(52, 211, 153, 0.18), theme::SUCCESS)))
                            .when(c.is_foreign_key, |d| d.child(badge("FK", zoom, theme::rgba8(129, 140, 248, 0.18), theme::ACCENT_HOVER))),
                    ),
            );
        }
        div()
            .id(SharedString::from(format!("card-{}", t.id)))
            .absolute()
            .left(px(x))
            .top(px(y))
            .w(px(t.width * zoom))
            .h(px(t.height * zoom))
            .flex()
            .flex_col()
            .border(px(rel::CARD_BORDER * zoom))
            .border_color(theme::BORDER)
            .rounded(px(10. * zoom))
            .bg(theme::rgba8(26, 26, 36, 0.96))
            .shadow(vec![shadow(0.0, 14.0 * zoom, 32.0 * zoom, 0.0, theme::rgba8(0, 0, 0, 0.28))])
            .overflow_hidden()
            .occlude()
            // A press on the body pans, as it does on the empty canvas (the
            // header stops the press and drags the table instead).
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, _cx| {
                if let Some(TabKind::Diagram(st)) = this.active_tab_mut().map(|t| &mut t.kind) {
                    st.panning = Some((e.position, st.pan));
                }
            }))
            // Cards occlude the canvas behind them, so they forward the wheel
            // themselves; otherwise scrolling over a card would do nothing.
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| this.diagram_scroll(e, cx)))
            .child(
                div()
                    .id(SharedString::from(format!("cardh-{}", t.id)))
                    .flex_shrink_0()
                    .h(px(rel::HEADER_HEIGHT * zoom))
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap(px(8. * zoom))
                    .px(px(12. * zoom))
                    .border_b(px(zoom))
                    .border_color(theme::BORDER)
                    .bg(theme::rgba8(99, 102, 241, 0.13))
                    .cursor(CursorStyle::ResizeUpRightDownLeft)
                    .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| {
                        cx.stop_propagation();
                        if let Some(TabKind::Diagram(st)) = this.active_tab_mut().map(|t| &mut t.kind) {
                            let origin = st.layout.get(&tid).copied().unwrap_or_default();
                            st.drag_table = Some((tid.clone(), e.position, origin));
                        }
                    }))
                    .child(
                        div()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .child(div().font_weight(FontWeight::BOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().text_size(px(0.96 * REM * zoom)).line_height(px(crate::ui::metrics::line_height_normal(0.96 * REM * zoom))).child(t.title.clone()))
                            .child(div().mt(px(2. * zoom)).text_color(theme::TEXT_MUTED).text_size(px(0.78 * REM * zoom)).line_height(px(crate::ui::metrics::line_height_normal(0.78 * REM * zoom))).child(format!("{} columns", t.columns.len()))),
                    )
                    .child(
                        div()
                            .id(SharedString::from(format!("open-{}", t.id)))
                            .flex_shrink_0()
                            .border(px(zoom))
                            .border_color(theme::BORDER)
                            .bg(theme::BG_SURFACE)
                            .text_color(theme::TEXT)
                            .rounded(px(6. * zoom))
                            .px(px(8. * zoom))
                            .py(px(6. * zoom))
                            .text_size(px(13.333 * zoom))
                            .line_height(px(crate::ui::metrics::line_height_normal(13.333 * zoom)))
                            .cursor_pointer()
                            .on_mouse_down(MouseButton::Left, |_, _, cx| cx.stop_propagation())
                            .on_click(cx.listener(move |this, _, _, cx| {
                                let conn = this.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
                                let driver = this.connection(&conn).map(|c| c.config.driver.clone()).unwrap_or_else(|| "postgres".into());
                                let sql = rel::query_sql(&driver, &schema_name, &table_name);
                                this.selected_conn_id = conn.clone();
                                this.open_query_tab_with_sql(&conn, "", &sql, Some(table_name.clone()), cx);
                            }))
                            .child("Open query"),
                    ),
            )
            .child(columns)
            .into_any_element()
    }

    fn diagram_zoom_by(&mut self, factor: f32, cx: &mut Context<Self>) {
        if let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) {
            match st.canvas_bounds {
                Some(b) => {
                    let pivot = (f32::from(b.size.width) / 2.0, f32::from(b.size.height) / 2.0);
                    apply_zoom(st, st.zoom * factor, pivot);
                }
                None => st.zoom = clamp_zoom(st.zoom * factor),
            }
        }
        cx.notify();
    }

    /// Wheel and trackpad scrolling. A trackpad's two-finger scroll (exact
    /// pixel deltas) pans, as it does in any canvas app now that pinch zooms;
    /// a mouse wheel (line deltas) zooms, as it always has; and either one
    /// zooms while ⌘ or Ctrl is held.
    fn diagram_scroll(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) else { return };
        let Some(b) = st.canvas_bounds else { return };
        let pivot = (f32::from(e.position.x - b.left()), f32::from(e.position.y - b.top()));
        let zoom_modifier = e.modifiers.platform || e.modifiers.control;
        match e.delta {
            ScrollDelta::Pixels(d) if !zoom_modifier => {
                st.pan = (st.pan.0 + f32::from(d.x), st.pan.1 + f32::from(d.y));
            }
            ScrollDelta::Pixels(d) => {
                let dy = f32::from(d.y);
                apply_zoom(st, st.zoom * (dy * 0.01).exp(), pivot);
            }
            ScrollDelta::Lines(d) => {
                if d.y == 0.0 {
                    return;
                }
                // 10% per line, however many lines one event carries (a fast
                // wheel spin arrives as a single event of several lines).
                apply_zoom(st, st.zoom * 1.1f32.powf(d.y.clamp(-10.0, 10.0)), pivot);
            }
        }
        cx.notify();
    }

    /// A trackpad pinch (from `ui::pinch`): zoom about the pointer, when the
    /// pointer is over the active diagram.
    pub fn diagram_pinch(&mut self, factor: f32, window: &Window, cx: &mut Context<Self>) {
        let mouse = window.mouse_position();
        let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) else { return };
        let Some(b) = st.canvas_bounds else { return };
        if !b.contains(&mouse) {
            return;
        }
        let pivot = (f32::from(mouse.x - b.left()), f32::from(mouse.y - b.top()));
        apply_zoom(st, st.zoom * factor, pivot);
        cx.notify();
    }

    fn diagram_reset(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let tab_id = self.active_tab_id.clone();
        let Some((cache, _)) = self.diagram_prepare(&tab_id, cx) else { return };
        if cache.hash.is_empty() {
            return;
        }
        self.settings.relationship_layouts.remove(&rel::LayoutStore::key(&conn_id, &cache.hash));
        self.settings.save();
        if let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) {
            st.layout = cache.default_layout.clone();
        }
        cx.notify();
    }

    fn diagram_mouse_down(&mut self, e: &MouseDownEvent, cx: &mut Context<Self>) {
        let hit = self.diagram_edge_hit(e.position);
        // Zoomed out the cards are painted rather than built, so they cannot
        // take the press themselves: find the one under the pointer here.
        let card = self.diagram_card_hit(e.position);
        let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) else { return };
        if let Some(edge) = hit {
            st.selected_edge = edge;
            cx.notify();
            return;
        }
        if st.zoom < DETAIL_ZOOM {
            if let Some(id) = card {
                let origin = st.layout.get(&id).copied().unwrap_or_default();
                st.drag_table = Some((id, e.position, origin));
                return;
            }
        }
        st.panning = Some((e.position, st.pan));
    }

    /// Continue a table drag or a pan (window-level, see `render_diagram`).
    fn diagram_drag_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) else { return };
        if let Some((id, start, origin)) = st.drag_table.clone() {
            let dx = f32::from(e.position.x - start.x) / st.zoom;
            let dy = f32::from(e.position.y - start.y) / st.zoom;
            st.layout.insert(id, LPoint { x: (origin.x + dx).round(), y: (origin.y + dy).round() });
            cx.notify();
            return;
        }
        if let Some((start, origin)) = st.panning {
            st.pan = (origin.0 + f32::from(e.position.x - start.x), origin.1 + f32::from(e.position.y - start.y));
            cx.notify();
        }
    }

    /// Hovering the canvas highlights the edge under the pointer.
    fn diagram_hover(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        if let Some(TabKind::Diagram(st)) = self.active_tab().map(|t| &t.kind) {
            if st.drag_table.is_some() || st.panning.is_some() {
                return;
            }
        }
        let hover = self.diagram_edge_hit(e.position).unwrap_or_default();
        let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) else { return };
        if st.hovered_edge != hover {
            st.hovered_edge = hover;
            cx.notify();
        }
    }

    fn diagram_mouse_up(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let mut save = None;
        if let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) {
            if st.drag_table.is_none() && st.panning.is_none() {
                return;
            }
            let hash = st.cache.as_ref().map(|c| c.hash.clone()).unwrap_or_default();
            if st.drag_table.take().is_some() && !hash.is_empty() {
                save = Some((hash, st.layout.clone()));
            }
            st.panning = None;
        }
        if let Some((hash, l)) = save {
            self.settings.relationship_layouts.insert(rel::LayoutStore::key(&conn_id, &hash), l);
            self.settings.save();
        }
        cx.notify();
    }

    /// Position in diagram space of a window position, with what it needs.
    fn diagram_world(&self, p: gpui::Point<Pixels>) -> Option<(&DiagramState, (f32, f32))> {
        let TabKind::Diagram(st) = &self.active_tab()?.kind else { return None };
        let b = st.canvas_bounds?;
        let x = (f32::from(p.x - b.left()) - st.pan.0) / st.zoom;
        let y = (f32::from(p.y - b.top()) - st.pan.1) / st.zoom;
        Some((st, (x, y)))
    }

    fn diagram_edge_hit(&self, p: gpui::Point<Pixels>) -> Option<String> {
        let (st, (x, y)) = self.diagram_world(p)?;
        let (cache, vis) = (st.cache.as_ref()?, st.visible.as_ref()?);
        // Within 4 screen pixels, whatever the zoom.
        let tol = 4.0 / st.zoom.min(1.0);
        for e in vis.graph.edges.iter().rev() {
            let pts = rel::edge_points(&cache.graph, &cache.index, &st.layout, e);
            let Some(bb) = WorldRect::of_points(&pts) else { continue };
            if x < bb.x0 - tol || x > bb.x1 + tol || y < bb.y0 - tol || y > bb.y1 + tol {
                continue;
            }
            if pts.windows(2).any(|w| dist_to_segment((x, y), w[0], w[1]) <= tol) {
                return Some(e.id.clone());
            }
        }
        None
    }

    fn diagram_card_hit(&self, p: gpui::Point<Pixels>) -> Option<String> {
        let (st, (x, y)) = self.diagram_world(p)?;
        let vis = st.visible.as_ref()?;
        // Last drawn is on top.
        vis.graph.tables.iter().rev().find_map(|t| {
            let at = st.layout.get(&t.id)?;
            (x >= at.x && x <= at.x + t.width && y >= at.y && y <= at.y + t.height).then(|| t.id.clone())
        })
    }
}

fn apply_zoom(st: &mut DiagramState, next: f32, pivot: (f32, f32)) {
    let clamped = clamp_zoom(next);
    let wx = (pivot.0 - st.pan.0) / st.zoom;
    let wy = (pivot.1 - st.pan.1) / st.zoom;
    st.zoom = clamped;
    st.pan = (pivot.0 - wx * clamped, pivot.1 - wy * clamped);
}

fn dist_to_segment(p: (f32, f32), a: (f32, f32), b: (f32, f32)) -> f32 {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let len2 = dx * dx + dy * dy;
    let t = if len2 == 0.0 { 0.0 } else { (((p.0 - a.0) * dx + (p.1 - a.1) * dy) / len2).clamp(0.0, 1.0) };
    let (cx, cy) = (a.0 + t * dx, a.1 + t * dy);
    ((p.0 - cx).powi(2) + (p.1 - cy).powi(2)).sqrt()
}

/// Stroke one edge given as screen points relative to the canvas origin.
fn paint_edge(window: &mut Window, b: Bounds<Pixels>, pts: &[(f32, f32)], zoom: f32, color: Rgba, stroke: f32) {
    if pts.len() < 2 {
        return;
    }
    // Never thinner than a pixel, or zoomed-out edges vanish.
    let width = (stroke * zoom).max(1.0);
    let map = |p: (f32, f32)| point(b.left() + px(p.0), b.top() + px(p.1));
    let mut pb = PathBuilder::stroke(px(width));
    pb.move_to(map(pts[0]));
    for p in &pts[1..] {
        pb.line_to(map(*p));
    }
    if let Ok(path) = pb.build() {
        window.paint_path(path, hsla(color));
    }
    // SVG marker: viewBox 0 0 10 10, markerWidth 8 (× stroke width), refX 8 refY 5,
    // path M0 0 L10 5 L0 10 z, orient auto.
    let end = pts[pts.len() - 1];
    let prev = pts[pts.len() - 2];
    let ang = (end.1 - prev.1).atan2(end.0 - prev.0);
    let unit = 0.8 * width;
    let tip = map(end);
    let (sin, cos) = ang.sin_cos();
    let rot = |x: f32, y: f32| point(tip.x + px((x * cos - y * sin) * unit), tip.y + px((x * sin + y * cos) * unit));
    let mut ab = PathBuilder::fill();
    ab.move_to(rot(-8.0, -5.0));
    ab.line_to(rot(2.0, 0.0));
    ab.line_to(rot(-8.0, 5.0));
    ab.close();
    if let Ok(path) = ab.build() {
        window.paint_path(path, hsla(color));
    }
}

/// Zoomed-out cards: a box with the header band, and the title while it is
/// still big enough to read. Bounds are relative to the canvas origin.
fn paint_cards(window: &mut Window, cx: &mut App, b: Bounds<Pixels>, cards: &[(Bounds<Pixels>, SharedString)], zoom: f32) {
    if cards.is_empty() {
        return;
    }
    let radius = px((10.0 * zoom).max(1.0));
    let header_h = px(rel::HEADER_HEIGHT * zoom);
    let title_px = 0.96 * REM * zoom;
    let font = gpui::Font { weight: FontWeight::BOLD, ..gpui::font(theme::UI_FONT) };
    for (r, title) in cards {
        let r = Bounds::new(point(b.left() + r.origin.x, b.top() + r.origin.y), r.size);
        window.paint_quad(
            gpui::quad(r, radius, hsla(theme::rgba8(26, 26, 36, 0.96)), px(1.), hsla(theme::BORDER), gpui::BorderStyle::default()),
        );
        let header = Bounds::new(r.origin, gpui::size(r.size.width, header_h.min(r.size.height)));
        window.paint_quad(
            gpui::fill(header, hsla(theme::rgba8(99, 102, 241, 0.13))).corner_radii(gpui::Corners {
                top_left: radius,
                top_right: radius,
                bottom_left: px(0.),
                bottom_right: px(0.),
            }),
        );
        if title_px >= MIN_TITLE_PX {
            let run = gpui::TextRun {
                len: title.len(),
                font: font.clone(),
                color: hsla(theme::TEXT),
                background_color: None,
                underline: None,
                strikethrough: None,
            };
            let line = window.text_system().shape_line(title.clone(), px(title_px), &[run], None);
            let lh = px(crate::ui::metrics::line_height_normal(title_px));
            let origin = point(r.origin.x + px(12.0 * zoom), r.origin.y + (header_h - lh) / 2.0);
            // Clip to the card so a long title does not spill past it.
            window.with_content_mask(Some(gpui::ContentMask { bounds: header }), |window| {
                let _ = line.paint(origin, lh, window, cx);
            });
        }
    }
}

fn paint_dots(window: &mut Window, b: Bounds<Pixels>) {
    // background: radial-gradient(circle at 1px 1px, rgba(255,255,255,.06) 1px, transparent 0) 24px grid
    //           + linear-gradient(180deg, rgba(255,255,255,.02), transparent 45%)
    let h: f32 = b.size.height.into();
    let w: f32 = b.size.width.into();
    let steps = 24;
    for i in 0..steps {
        let t0 = i as f32 / steps as f32 * 0.45;
        let a = 0.02 * (1.0 - t0 / 0.45);
        let y0 = t0 * h;
        let y1 = (i + 1) as f32 / steps as f32 * 0.45 * h;
        window.paint_quad(gpui::fill(
            Bounds::new(point(b.left(), b.top() + px(y0)), gpui::size(px(w), px(y1 - y0))),
            hsla(theme::rgba8(255, 255, 255, a)),
        ));
    }
    let dot = hsla(theme::rgba8(255, 255, 255, 0.06));
    let mut y = 0.0;
    while y < h {
        let mut x = 0.0;
        while x < w {
            window.paint_quad(gpui::fill(Bounds::new(point(b.left() + px(x), b.top() + px(y)), gpui::size(px(2.), px(2.))), dot).corner_radii(px(1.)));
            x += 24.0;
        }
        y += 24.0;
    }
}

fn badge(label: &'static str, zoom: f32, bg: Rgba, fg: Rgba) -> gpui::Div {
    div()
        .px(px(6. * zoom))
        .py(px(2. * zoom))
        .rounded(px(999.))
        .bg(bg)
        .text_color(fg)
        .text_size(px(0.68 * REM * zoom))
        .line_height(px(crate::ui::metrics::line_height_normal(0.68 * REM * zoom)))
        .font_weight(FontWeight::SEMIBOLD)
        .child(label)
}

pub fn input_look_for_filter() -> InputLook {
    InputLook::dialog(1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_view_rect_is_the_canvas_in_diagram_space() {
        // 800×600 canvas, panned 100 right and 50 down, at 50%.
        let v = WorldRect::of_view(800.0, 600.0, (100.0, 50.0), 0.5, 0.0);
        assert_eq!(v, WorldRect { x0: -200.0, y0: -100.0, x1: 1400.0, y1: 1100.0 });
        assert!(v.intersects(1390.0, 0.0, 50.0, 50.0));
        assert!(!v.intersects(1400.0, 0.0, 50.0, 50.0));
        assert!(!v.intersects(-260.0, 0.0, 50.0, 50.0));
        // The margin is in screen pixels, so it grows as the zoom shrinks.
        let m = WorldRect::of_view(800.0, 600.0, (0.0, 0.0), 0.5, 10.0);
        assert_eq!((m.x0, m.x1), (-20.0, 1620.0));
    }

    #[test]
    fn only_the_rows_in_view_are_built() {
        let rows_top = rel::CARD_BORDER + rel::HEADER_HEIGHT;
        let view = |y0: f32, y1: f32| WorldRect { x0: 0.0, y0, x1: 100.0, y1 };
        // Whole card in view.
        assert_eq!(visible_rows(0.0, 10, &view(-50.0, 1000.0)), 0..10);
        // Scrolled so rows 100..120 of a 500-row table are visible.
        let r = visible_rows(0.0, 500, &view(rows_top + 100.0 * rel::ROW_HEIGHT, rows_top + 120.0 * rel::ROW_HEIGHT));
        assert_eq!(r, 100..120);
        // A partly visible row is included at both ends.
        let r = visible_rows(0.0, 500, &view(rows_top + 10.5 * rel::ROW_HEIGHT, rows_top + 12.5 * rel::ROW_HEIGHT));
        assert_eq!(r, 10..13);
        // View entirely above or below the rows.
        assert_eq!(visible_rows(1000.0, 10, &view(0.0, 500.0)).len(), 0);
        assert_eq!(visible_rows(0.0, 10, &view(5000.0, 6000.0)).len(), 0);
    }

    #[test]
    fn zoom_is_clamped_but_not_stepped() {
        assert_eq!(clamp_zoom(0.01), MIN_ZOOM);
        assert_eq!(clamp_zoom(9.0), MAX_ZOOM);
        assert_eq!(clamp_zoom(f32::NAN), 1.0);
        // Pinching moves in small steps; rounding them would make it judder.
        assert_eq!(clamp_zoom(0.123), 0.123);
    }
}
