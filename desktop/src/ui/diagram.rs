//! Relationship viewer tab (`RelationshipDiagram.svelte`).

use crate::ui::relationship::{self as rel, DiagramEdge, DiagramGraph, Layout, Point as LPoint};
use crate::ui::theme::{self, hsla, Rgba};
use crate::ui::widgets::text_input::{InputEvent, InputLook, TextInput};
use crate::ui::widgets::{shadow, TextExt};
use crate::ui::workspace::{Tab, TabKind, Workspace};
use gpui::{
    canvas, div, point, prelude::*, px, AnyElement, Bounds, Context, CursorStyle, Entity, FontWeight, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PathBuilder, Pixels, ScrollWheelEvent, SharedString, Subscription,
    Window,
};

pub struct DiagramState {
    pub pan: (f32, f32),
    pub zoom: f32,
    pub layout: Layout,
    pub layout_key: String,
    pub last_hash: String,
    pub hovered_edge: String,
    pub selected_edge: String,
    pub filter: Entity<TextInput>,
    pub drag_table: Option<(String, gpui::Point<Pixels>, LPoint)>,
    pub panning: Option<(gpui::Point<Pixels>, (f32, f32))>,
    pub canvas_bounds: Option<Bounds<Pixels>>,
    _sub: Subscription,
}

/// rem = 16px (the document root font size in the old UI).
const REM: f32 = 16.0;

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
            last_hash: String::new(),
            hovered_edge: String::new(),
            selected_edge: String::new(),
            filter,
            drag_table: None,
            panning: None,
            canvas_bounds: None,
            _sub: sub,
        }
    }
}

fn clamp_zoom(v: f32) -> f32 {
    ((v * 100.0).round() / 100.0).clamp(0.5, 2.5)
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

    pub fn render_diagram(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let tab_id = self.active_tab_id.clone();
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let Some(conn) = self.connection(&conn_id).cloned() else {
            return div().text_color(theme::TEXT_MUTED).child("Relationship viewer unavailable.").into_any_element();
        };
        let graph = conn.schema.as_deref().map(rel::build_graph).unwrap_or_default();
        let hash = conn.schema.as_deref().map(rel::schema_hash).unwrap_or_default();
        let default_layout = rel::default_layout(&graph);
        let key = if hash.is_empty() { String::new() } else { format!("{conn_id}:{hash}:{}", graph.tables.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join("|")) };
        let store_key = crate::ui::relationship::LayoutStore::key(&conn_id, &hash);
        let persisted = self.settings.relationship_layouts.get(&store_key).cloned();
        let mut save_layout: Option<Layout> = None;
        {
            let Some(TabKind::Diagram(st)) = self.tab_mut(&tab_id).map(|t| &mut t.kind) else { return div().into_any_element() };
            if st.layout_key != key {
                let prev = std::mem::take(&mut st.layout);
                let had_prev = !prev.is_empty();
                st.layout_key = key.clone();
                let carry = persisted.clone().or(if had_prev { Some(prev) } else { None });
                st.layout = rel::merge_layout(&graph, &default_layout, carry.as_ref());
                if !hash.is_empty() && had_prev {
                    save_layout = Some(st.layout.clone());
                }
                st.last_hash = hash.clone();
                if !graph.edges.iter().any(|e| e.id == st.selected_edge) {
                    st.selected_edge = graph.edges.first().map(|e| e.id.clone()).unwrap_or_default();
                }
            }
        }
        if let Some(l) = save_layout {
            self.settings.relationship_layouts.insert(store_key.clone(), l);
            self.settings.save();
        }
        let Some(TabKind::Diagram(st)) = self.tab(&tab_id).map(|t| &t.kind) else { return div().into_any_element() };
        let filter_text = st.filter.read(cx).text().to_string();
        let visible = rel::filter_graph(&graph, &filter_text);
        let effective = rel::effective_selected_edge_id(&visible.edges, &st.selected_edge);
        let selected = visible.edges.iter().find(|e| e.id == effective).cloned();
        let active_edge = if st.hovered_edge.is_empty() { effective.clone() } else { st.hovered_edge.clone() };
        let highlighted: std::collections::HashSet<String> = visible
            .edges
            .iter()
            .find(|e| e.id == active_edge)
            .map(|e| e.source_column_ids.iter().chain(e.target_column_ids.iter()).cloned().collect())
            .unwrap_or_default();
        let layout: Layout = visible.tables.iter().filter_map(|t| st.layout.get(&t.id).map(|p| (t.id.clone(), *p))).collect();
        let (pan, zoom) = (st.pan, st.zoom);
        let filter_input = st.filter.clone();
        let hovered_edge = st.hovered_edge.clone();
        let sel_edge_raw = st.selected_edge.clone();

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
                    .child(div().text_color(theme::TEXT_MUTED).text_size(px(13.0 * 0.92)).child(conn.config.name.clone()))
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

        // Stage content: edges canvas + table cards.
        let bounds_w = visible.tables.iter().map(|t| layout.get(&t.id).map(|p| p.x).unwrap_or(0.0) + t.width).fold(0.0, f32::max);
        let bounds_h = visible.tables.iter().map(|t| layout.get(&t.id).map(|p| p.y).unwrap_or(0.0) + t.height).fold(0.0, f32::max);
        let content_w = (bounds_w + 160.0).max(900.0);
        let content_h = (bounds_h + 160.0).max(640.0);
        let edges = visible.edges.clone();
        let g2 = graph.clone();
        let l2 = layout.clone();
        let hov = hovered_edge.clone();
        let sel = sel_edge_raw.clone();
        let edge_canvas = canvas(
            |_, _, _| (),
            move |b, _, window, _cx| {
                for e in &edges {
                    let active = e.id == hov || e.id == sel;
                    let color = if active { theme::ACCENT_HOVER } else { theme::rgba8(129, 140, 248, 0.65) };
                    let width = if active { 3.5 } else { 2.5 };
                    paint_edge(window, b, pan, &g2, &l2, e, zoom, color, width);
                }
            },
        )
        .absolute()
        .top_0()
        .left_0()
        .size_full();

        let mut stage = div().absolute().top_0().left_0().size_full().child(edge_canvas);
        for t in &visible.tables {
            let p = layout.get(&t.id).copied().unwrap_or_default();
            let x = pan.0 + p.x * zoom;
            let y = pan.1 + p.y * zoom;
            let tid = t.id.clone();
            let t_open = t.clone();
            let mut columns = div().flex().flex_col();
            for c in &t.columns {
                columns = columns.child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap(px(8. * zoom))
                        .min_h(px(24. * zoom))
                        .px(px(12. * zoom))
                        .border_t(px(zoom))
                        .border_color(theme::rgba8(255, 255, 255, 0.04))
                        .when(highlighted.contains(&c.id), |d| d.bg(theme::rgba8(129, 140, 248, 0.15)))
                        .text_size(px(13. * zoom))
                        .line_height(px(crate::ui::metrics::line_height_normal(13. * zoom)))
                        .child(div().overflow_hidden().whitespace_nowrap().text_ellipsis().child(c.name.clone()))
                        .child(
                            div()
                                .flex()
                                .gap(px(4. * zoom))
                                .when(c.is_primary_key, |d| d.child(badge("PK", zoom, theme::rgba8(52, 211, 153, 0.18), theme::SUCCESS)))
                                .when(c.is_foreign_key, |d| d.child(badge("FK", zoom, theme::rgba8(129, 140, 248, 0.18), theme::ACCENT_HOVER))),
                        ),
                );
            }
            stage = stage.child(
                div()
                    .id(SharedString::from(format!("card-{}", t.id)))
                    .absolute()
                    .left(px(x))
                    .top(px(y))
                    .w(px(t.width * zoom))
                    .border(px(zoom))
                    .border_color(theme::BORDER)
                    .rounded(px(10. * zoom))
                    .bg(theme::rgba8(26, 26, 36, 0.96))
                    .shadow(vec![shadow(0.0, 14.0 * zoom, 32.0 * zoom, 0.0, theme::rgba8(0, 0, 0, 0.28))])
                    .overflow_hidden()
                    .occlude()
                    .child(
                        div()
                            .id(SharedString::from(format!("cardh-{}", t.id)))
                            .flex()
                            .items_center()
                            .justify_between()
                            .gap(px(8. * zoom))
                            .px(px(12. * zoom))
                            .py(px(10. * zoom))
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
                                    .child(div().font_weight(FontWeight::BOLD).text_size(px(0.96 * REM * zoom)).line_height(px(crate::ui::metrics::line_height_normal(0.96 * REM * zoom))).child(t.title.clone()))
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
                                        let driver = this.connection(&this.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default()).map(|c| c.config.driver.clone()).unwrap_or_else(|| "postgres".into());
                                        let conn = this.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
                                        let sql = rel::query_sql(&driver, &t_open.schema_name, &t_open.table_name);
                                        this.selected_conn_id = conn.clone();
                                        this.open_query_tab_with_sql(&conn, "", &sql, Some(t_open.table_name.clone()), cx);
                                    }))
                                    .child("Open query"),
                            ),
                    )
                    .child(columns),
            );
        }

        let notice = if graph.edges.is_empty() {
            Some("No foreign-key relationships found.")
        } else if visible.tables.is_empty() {
            Some("No tables match the current filter.")
        } else {
            None
        };
        let _ = (content_w, content_h);
        let ws = cx.entity().downgrade();
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
                    |b, _, window, _| paint_dots(window, b),
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
            .on_mouse_down(MouseButton::Left, cx.listener(|this, e: &MouseDownEvent, _, cx| {
                let hit = this.diagram_edge_hit(e.position, cx);
                if let Some(TabKind::Diagram(st)) = this.active_tab_mut().map(|t| &mut t.kind) {
                    if let Some(edge) = hit {
                        st.selected_edge = edge;
                        cx.notify();
                        return;
                    }
                    st.panning = Some((e.position, st.pan));
                }
            }))
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, cx| this.diagram_mouse_move(e, cx)))
            .on_mouse_up(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.diagram_mouse_up(cx)))
            .on_mouse_up_out(MouseButton::Left, cx.listener(|this, _: &MouseUpEvent, _, cx| this.diagram_mouse_up(cx)))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, cx| {
                let d = e.delta.pixel_delta(px(20.));
                let factor = if d.y > px(0.) { 1.1 } else { 0.9 };
                if let Some(TabKind::Diagram(st)) = this.active_tab_mut().map(|t| &mut t.kind) {
                    if let Some(b) = st.canvas_bounds {
                        let pivot = ((e.position.x - b.left()).into(), (e.position.y - b.top()).into());
                        apply_zoom(st, st.zoom * factor, pivot);
                    }
                }
                cx.notify();
            }));

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
        let _ = window;
        div()
            .flex()
            .flex_col()
            .size_full()
            .bg(theme::BG_EDITOR)
            .child(toolbar)
            .child(div().flex().flex_1().min_h(px(0.)).child(canvas_area).child(inspector))
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

    fn diagram_reset(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let Some(schema) = self.connection(&conn_id).and_then(|c| c.schema.clone()) else { return };
        let hash = rel::schema_hash(&schema);
        let graph = rel::build_graph(&schema);
        self.settings.relationship_layouts.remove(&rel::LayoutStore::key(&conn_id, &hash));
        self.settings.save();
        if let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) {
            st.layout = rel::default_layout(&graph);
        }
        cx.notify();
    }

    fn diagram_mouse_move(&mut self, e: &MouseMoveEvent, cx: &mut Context<Self>) {
        let hover = self.diagram_edge_hit(e.position, cx).unwrap_or_default();
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
            return;
        }
        if st.hovered_edge != hover {
            st.hovered_edge = hover;
            cx.notify();
        }
    }

    fn diagram_mouse_up(&mut self, cx: &mut Context<Self>) {
        let conn_id = self.active_tab().map(|t| t.conn_id.clone()).unwrap_or_default();
        let hash = self.connection(&conn_id).and_then(|c| c.schema.as_deref().map(rel::schema_hash)).unwrap_or_default();
        let mut save = None;
        if let Some(TabKind::Diagram(st)) = self.active_tab_mut().map(|t| &mut t.kind) {
            if st.drag_table.take().is_some() && !hash.is_empty() {
                save = Some(st.layout.clone());
            }
            st.panning = None;
        }
        if let Some(l) = save {
            self.settings.relationship_layouts.insert(rel::LayoutStore::key(&conn_id, &hash), l);
            self.settings.save();
        }
        cx.notify();
    }

    fn diagram_edge_hit(&self, p: gpui::Point<Pixels>, cx: &gpui::App) -> Option<String> {
        let tab = self.active_tab()?;
        let TabKind::Diagram(st) = &tab.kind else { return None };
        let b = st.canvas_bounds?;
        let schema = self.connection(&tab.conn_id)?.schema.clone()?;
        let graph = rel::build_graph(&schema);
        let filter = st.filter.read(cx).text().to_string();
        let visible = rel::filter_graph(&graph, &filter);
        let (x, y) = ((f32::from(p.x - b.left()) - st.pan.0) / st.zoom, (f32::from(p.y - b.top()) - st.pan.1) / st.zoom);
        for e in visible.edges.iter().rev() {
            let pts = edge_points(&graph, &st.layout, e);
            if pts.windows(2).any(|w| dist_to_segment((x, y), w[0], w[1]) <= 4.0) {
                return Some(e.id.clone());
            }
        }
        None
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

fn anchor(graph: &DiagramGraph, layout: &Layout, table_id: &str, column_id: Option<&String>, source: bool) -> Option<(f32, f32)> {
    let t = graph.tables.iter().find(|t| t.id == table_id)?;
    let p = layout.get(table_id)?;
    let idx = column_id.and_then(|c| t.columns.iter().position(|x| &x.id == c)).unwrap_or(0);
    let x = if source { p.x + t.width } else { p.x };
    let y = p.y + rel::HEADER_HEIGHT + idx as f32 * rel::ROW_HEIGHT + rel::ROW_HEIGHT / 2.0;
    Some((x, y))
}

fn edge_points(graph: &DiagramGraph, layout: &Layout, e: &DiagramEdge) -> Vec<(f32, f32)> {
    let (Some(s), Some(t)) = (
        anchor(graph, layout, &e.source_table_id, e.source_column_ids.first(), true),
        anchor(graph, layout, &e.target_table_id, e.target_column_ids.first(), false),
    ) else {
        return Vec::new();
    };
    if e.is_self_referential {
        let loop_x = s.0 + 90.0;
        // Sample the cubic for hit-testing.
        return (0..=16)
            .map(|i| {
                let t_ = i as f32 / 16.0;
                let mt = 1.0 - t_;
                let x = mt.powi(3) * s.0 + 3.0 * mt * mt * t_ * loop_x + 3.0 * mt * t_ * t_ * loop_x + t_.powi(3) * t.0;
                let y = mt.powi(3) * s.1 + 3.0 * mt * mt * t_ * s.1 + 3.0 * mt * t_ * t_ * t.1 + t_.powi(3) * t.1;
                (x, y)
            })
            .collect();
    }
    let mid = ((s.0 + t.0) / 2.0).round();
    vec![s, (mid, s.1), (mid, t.1), t]
}

#[allow(clippy::too_many_arguments)]
fn paint_edge(window: &mut Window, b: Bounds<Pixels>, pan: (f32, f32), graph: &DiagramGraph, layout: &Layout, e: &DiagramEdge, zoom: f32, color: Rgba, stroke: f32) {
    let pts = edge_points(graph, layout, e);
    if pts.len() < 2 {
        return;
    }
    let map = |p: (f32, f32)| point(b.left() + px(pan.0 + p.0 * zoom), b.top() + px(pan.1 + p.1 * zoom));
    let mut pb = PathBuilder::stroke(px(stroke * zoom));
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
    let unit = 0.8 * stroke * zoom;
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
