//! Storage dashboard: how a database's space divides between its tables.
//!
//! Everything here is derived from the schema tree the navigator already
//! holds — every driver's schema loader records a size per table
//! (`pg_total_relation_size`, `information_schema.TABLES`, `dbstat`) — so
//! opening the dashboard costs no extra queries, and Refresh simply reloads
//! the schema.

use crate::models::SchemaTree;
use crate::ui::model::format_bytes;
use crate::ui::theme::{self, hsla, Rgba};
use crate::ui::widgets::{Scale, TextExt};
use crate::ui::workspace::{Tab, TabKind, Workspace};
use gpui::{
    canvas, div, point, prelude::*, px, uniform_list, AnyElement, Bounds, Context, FontWeight, MouseMoveEvent,
    PathBuilder, Pixels, SharedString, UniformListScrollHandle, Window,
};
use std::cell::Cell;
use std::collections::BTreeSet;
use std::f32::consts::TAU;
use std::rc::Rc;

/// How many tables get a slice of their own; the rest share one.
pub const TOP_TABLES: usize = 10;

/// Slice colours, in fixed order. The first eight are the reference
/// categorical palette's dark steps; nine and ten extend it with a teal and a
/// rust from the same lightness band. Validated against the panel surface
/// (`#1e1e2e`): all inside the band, >= 3:1 contrast, and every neighbouring
/// pair clear of the colour-blind separation floor. Identity never rests on
/// colour alone — the legend names every slice and hovering either one
/// highlights both.
pub const SLICE_COLORS: [Rgba; TOP_TABLES] = [
    theme::hex(0x3987e5),
    theme::hex(0xd95926),
    theme::hex(0x199e70),
    theme::hex(0xc98500),
    theme::hex(0xd55181),
    theme::hex(0x008300),
    theme::hex(0x9085e9),
    theme::hex(0xe66767),
    theme::hex(0x2aa3c0),
    theme::hex(0xc2682e),
];
/// "Other" is not a series, so it takes a neutral rather than a hue.
pub const OTHER_COLOR: Rgba = theme::hex(0x5c6070);

/// Which part of a connection the dashboard covers.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct StorageScope {
    /// A database inside a multi-database tree (PostgreSQL browsing every
    /// database). `None` covers them all.
    pub database: Option<String>,
    /// A single schema (for MySQL, a schema *is* a database).
    pub schema: Option<String>,
    /// What the tab and header call it.
    pub label: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TableSize {
    pub database: String,
    pub schema: String,
    pub name: String,
    /// `None` when the server did not report a size for the table.
    pub bytes: Option<i64>,
}

impl TableSize {
    fn size(&self) -> i64 {
        self.bytes.unwrap_or(0).max(0)
    }

    /// The location to print before the name, when the scope spans more than
    /// one: `db.schema`, `schema` or nothing.
    pub fn prefix(&self, show_database: bool, show_schema: bool) -> String {
        let mut parts = Vec::new();
        if show_database && !self.database.is_empty() {
            parts.push(self.database.as_str());
        }
        if show_schema && !self.schema.is_empty() {
            parts.push(self.schema.as_str());
        }
        parts.join(".")
    }
}

/// Every table in `scope`, largest first (ties by name, so the order is
/// stable between refreshes).
pub fn collect_table_sizes(tree: &SchemaTree, scope: &StorageScope) -> Vec<TableSize> {
    let mut out = Vec::new();
    let schema_ok = |name: &str| scope.schema.as_deref().is_none_or(|s| s == name);
    if scope.database.is_none() {
        if scope.schema.is_none() {
            out.extend(tree.tables.iter().map(|t| TableSize {
                database: String::new(),
                schema: String::new(),
                name: t.name.clone(),
                bytes: t.size_bytes,
            }));
        }
        for sc in tree.schemas.iter().filter(|sc| schema_ok(&sc.name)) {
            out.extend(sc.tables.iter().map(|t| TableSize {
                database: String::new(),
                schema: sc.name.clone(),
                name: t.name.clone(),
                bytes: t.size_bytes,
            }));
        }
    }
    for db in tree.databases.iter().filter(|d| scope.database.as_deref().is_none_or(|n| n == d.name)) {
        for sc in db.schemas.iter().filter(|sc| schema_ok(&sc.name)) {
            out.extend(sc.tables.iter().map(|t| TableSize {
                database: db.name.clone(),
                schema: sc.name.clone(),
                name: t.name.clone(),
                bytes: t.size_bytes,
            }));
        }
    }
    sort_tables(&mut out, true);
    out
}

pub fn sort_tables(tables: &mut [TableSize], descending: bool) {
    tables.sort_by(|a, b| {
        let by_size = if descending { b.size().cmp(&a.size()) } else { a.size().cmp(&b.size()) };
        by_size
            .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
            .then_with(|| a.schema.cmp(&b.schema))
            .then_with(|| a.database.cmp(&b.database))
    });
}

#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    pub label: String,
    pub prefix: String,
    pub bytes: i64,
    pub fraction: f32,
    pub color: Rgba,
    /// How many tables the slice stands for (more than one only for "Other").
    pub tables: usize,
}

/// The pie: the `top` largest tables with a slice each, then one "Other"
/// slice for whatever remains. Empty tables take no slice at all.
pub fn pie_slices(tables: &[TableSize], top: usize, show_database: bool, show_schema: bool) -> Vec<Slice> {
    let mut sized: Vec<&TableSize> = tables.iter().filter(|t| t.size() > 0).collect();
    sized.sort_by_key(|t| std::cmp::Reverse(t.size()));
    let total: i64 = sized.iter().map(|t| t.size()).sum();
    if total == 0 {
        return Vec::new();
    }
    let fraction = |bytes: i64| (bytes as f64 / total as f64) as f32;
    let mut slices: Vec<Slice> = sized
        .iter()
        .take(top)
        .enumerate()
        .map(|(i, t)| Slice {
            label: t.name.clone(),
            prefix: t.prefix(show_database, show_schema),
            bytes: t.size(),
            fraction: fraction(t.size()),
            color: SLICE_COLORS[i % SLICE_COLORS.len()],
            tables: 1,
        })
        .collect();
    let rest = &sized[sized.len().min(top)..];
    if !rest.is_empty() {
        let bytes: i64 = rest.iter().map(|t| t.size()).sum();
        slices.push(Slice {
            label: format!("Other ({} table{})", rest.len(), if rest.len() == 1 { "" } else { "s" }),
            prefix: String::new(),
            bytes,
            fraction: fraction(bytes),
            color: OTHER_COLOR,
            tables: rest.len(),
        });
    }
    slices
}

/// Whether the tables live in more than one database / schema, which decides
/// whether names need qualifying.
pub fn spans(tables: &[TableSize]) -> (bool, bool) {
    let dbs: BTreeSet<&str> = tables.iter().map(|t| t.database.as_str()).collect();
    let schemas: BTreeSet<(&str, &str)> = tables.iter().map(|t| (t.database.as_str(), t.schema.as_str())).collect();
    (dbs.len() > 1, schemas.len() > 1)
}

/// Which slice an angle falls in. Angles run clockwise from twelve o'clock.
pub fn slice_at(slices: &[Slice], angle: f32) -> Option<usize> {
    let mut start = 0.0;
    for (i, s) in slices.iter().enumerate() {
        let end = start + s.fraction * TAU;
        if angle >= start && angle < end {
            return Some(i);
        }
        start = end;
    }
    None
}

/// `12.3%`, or `<0.1%` for slivers that would otherwise print as nothing.
pub fn percent(bytes: i64, total: i64) -> String {
    if total <= 0 || bytes <= 0 {
        return "0%".into();
    }
    let p = bytes as f64 * 100.0 / total as f64;
    if p < 0.1 {
        "<0.1%".into()
    } else if p >= 10.0 {
        format!("{p:.0}%")
    } else {
        format!("{p:.1}%")
    }
}

pub struct StorageState {
    pub scope: StorageScope,
    pub descending: bool,
    /// Slice under the pointer, in the pie or the legend.
    pub hovered: Option<usize>,
    pub list: UniformListScrollHandle,
    pie_bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
}

impl StorageState {
    pub fn new(scope: StorageScope) -> Self {
        StorageState { scope, descending: true, hovered: None, list: UniformListScrollHandle::new(), pie_bounds: Rc::new(Cell::new(None)) }
    }
}

const PIE_SIZE: f32 = 208.0;
const PIE_HOLE: f32 = 0.58;

impl Workspace {
    pub fn show_storage(&mut self, conn_id: &str, scope: StorageScope, cx: &mut Context<Self>) {
        if self.connection(conn_id).is_none() {
            self.status = "Connection not found.".into();
            cx.notify();
            return;
        }
        let conn_id = conn_id.to_string();
        let load = self.load_cached_schema(&conn_id, cx);
        cx.spawn(async move |this, cx| {
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
            this.update(cx, move |this, cx| {
                let Some(conn) = this.connection(&conn_id) else { return };
                if conn.schema.is_none() {
                    this.status = "Schema is not available for this connection.".into();
                    cx.notify();
                    return;
                }
                let name = conn.config.name.clone();
                this.selected_conn_id = conn_id.clone();
                if let Some(t) = this
                    .tabs
                    .iter()
                    .find(|t| t.conn_id == conn_id && matches!(&t.kind, TabKind::Storage(st) if st.scope == scope))
                {
                    let id = t.id.clone();
                    this.set_active_tab(id, cx);
                    return;
                }
                let title = if scope.label.is_empty() || scope.label == name {
                    format!("{name} Storage")
                } else {
                    format!("{} Storage", scope.label)
                };
                let tab = Tab {
                    id: crate::ui::model::new_id(),
                    title,
                    conn_id: conn_id.clone(),
                    manually_renamed: false,
                    kind: TabKind::Storage(StorageState::new(scope)),
                };
                let id = tab.id.clone();
                this.tabs.push(tab);
                this.set_active_tab(id, cx);
            })
            .ok();
        })
        .detach();
    }

    fn storage_state_mut(&mut self, tab_id: &str) -> Option<&mut StorageState> {
        match self.tab_mut(tab_id).map(|t| &mut t.kind) {
            Some(TabKind::Storage(st)) => Some(st),
            _ => None,
        }
    }

    fn set_storage_hover(&mut self, tab_id: &str, hovered: Option<usize>, cx: &mut Context<Self>) {
        if let Some(st) = self.storage_state_mut(tab_id) {
            if st.hovered != hovered {
                st.hovered = hovered;
                cx.notify();
            }
        }
    }

    pub fn render_storage(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let s = Scale(self.scale());
        let tab_id = self.active_tab_id.clone();
        let Some(tab) = self.active_tab() else { return div().into_any_element() };
        let conn = self.connection(&tab.conn_id).cloned();
        let TabKind::Storage(st) = &tab.kind else { return div().into_any_element() };
        let (scope, descending, hovered, list_handle, pie_bounds) =
            (st.scope.clone(), st.descending, st.hovered, st.list.clone(), st.pie_bounds.clone());
        let conn_id = tab.conn_id.clone();
        let loading = conn.as_ref().is_some_and(|c| c.schema_loading);

        let mut tables = conn.as_ref().and_then(|c| c.schema.as_deref()).map(|t| collect_table_sizes(t, &scope)).unwrap_or_default();
        let (show_db, show_schema) = spans(&tables);
        let slices = pie_slices(&tables, TOP_TABLES, show_db, show_schema);
        let total: i64 = tables.iter().map(|t| t.size()).sum();
        let unsized_count = tables.iter().filter(|t| t.bytes.is_none()).count();
        let largest = tables.first().map(|t| t.size()).unwrap_or(0);
        let colour_of: std::collections::HashMap<(String, String, String), Rgba> = {
            let mut sized: Vec<&TableSize> = tables.iter().filter(|t| t.size() > 0).collect();
            sized.sort_by_key(|t| std::cmp::Reverse(t.size()));
            sized
                .iter()
                .take(TOP_TABLES)
                .enumerate()
                .map(|(i, t)| ((t.database.clone(), t.schema.clone(), t.name.clone()), SLICE_COLORS[i]))
                .collect()
        };
        if !descending {
            sort_tables(&mut tables, false);
        }

        // ── Toolbar ──
        let subtitle = {
            let conn_name = conn.as_ref().map(|c| c.config.name.clone()).unwrap_or_else(|| "Unknown connection".into());
            if scope.label.is_empty() || scope.label == conn_name { conn_name } else { format!("{conn_name} · {}", scope.label) }
        };
        let c1 = conn_id.clone();
        let refresh = div()
            .id("storage-refresh")
            .border_1()
            .border_color(theme::BORDER)
            .rounded(px(4.))
            .bg(theme::BG_INPUT)
            .text_color(theme::TEXT)
            .t(s, 12.0)
            .px(px(10.))
            .py(px(5.))
            .child(if loading { "Refreshing..." } else { "Refresh" })
            .when(loading, |d| d.opacity(0.5))
            .when(!loading, |d| {
                d.cursor_pointer().hover(|st| st.border_color(theme::ACCENT)).on_click(cx.listener(move |this, _, _, cx| {
                    this.status = "Refreshing schema…".into();
                    let t = this.refresh_schema(&c1, cx);
                    cx.spawn(async move |this, cx| {
                        t.await;
                        this.update(cx, |this, cx| {
                            this.status = "Storage refreshed".into();
                            cx.notify();
                        })
                        .ok();
                    })
                    .detach();
                    cx.notify();
                }))
            });
        let toolbar = div()
            .flex()
            .items_center()
            .justify_between()
            .gap(px(12.))
            .px(px(12.))
            .py(px(10.))
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_TOOLBAR)
            .child(
                div()
                    .flex()
                    .items_baseline()
                    .gap(px(10.))
                    .min_w(px(0.))
                    .child(div().t(s, 14.0).font_weight(FontWeight::SEMIBOLD).child("Storage"))
                    .child(div().t(s, 12.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().overflow_hidden().text_ellipsis().child(subtitle)),
            )
            .child(refresh);

        let mut view = div().flex().flex_col().size_full().min_w(px(0.)).bg(theme::BG_EDITOR).text_color(theme::TEXT).child(toolbar);
        if tables.is_empty() {
            let msg = if loading { "Loading schema…" } else { "No tables found." };
            return view.child(div().p(px(14.)).text_color(theme::TEXT_MUTED).t(s, 12.0).child(msg)).into_any_element();
        }

        // ── Summary tiles ──
        let tile = |label: &'static str, value: String, detail: String| {
            div()
                .flex()
                .flex_col()
                .gap(px(2.))
                .flex_1()
                .min_w(px(0.))
                .px(px(14.))
                .py(px(10.))
                .rounded(px(6.))
                .border_1()
                .border_color(theme::BORDER)
                .bg(theme::BG_SURFACE)
                .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).child(label))
                .child(div().t(s, 18.0).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().overflow_hidden().text_ellipsis().child(value))
                .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().overflow_hidden().text_ellipsis().child(detail))
        };
        let top_share: i64 = slices.iter().filter(|sl| sl.tables == 1).map(|sl| sl.bytes).sum();
        let biggest = tables.iter().max_by_key(|t| t.size()).cloned();
        let tiles = div()
            .flex()
            .gap(px(12.))
            .child(tile("Total size", non_empty_bytes(total), format!("across {} table{}", tables.len(), if tables.len() == 1 { "" } else { "s" })))
            .child(tile(
                "Largest table",
                biggest.as_ref().map(|t| t.name.clone()).unwrap_or_default(),
                biggest.as_ref().map(|t| format!("{} · {} of total", non_empty_bytes(t.size()), percent(t.size(), total))).unwrap_or_default(),
            ))
            .child(tile(
                "Top 10 tables",
                percent(top_share, total),
                format!("{} of {}", non_empty_bytes(top_share), non_empty_bytes(total)),
            ));

        // ── Pie + legend ──
        let pie_slices_paint = slices.clone();
        let bounds_cell = pie_bounds.clone();
        let pie_canvas = canvas(
            move |b, _, _| {
                bounds_cell.set(Some(b));
            },
            move |b, _, window, _| paint_donut(window, b, &pie_slices_paint, hovered),
        )
        .size_full();
        let centre = match hovered.and_then(|i| slices.get(i)) {
            Some(sl) => (sl.label.clone(), non_empty_bytes(sl.bytes), percent(sl.bytes, total)),
            None => ("Total".to_string(), non_empty_bytes(total), format!("{} tables", tables.len())),
        };
        let hit_slices = slices.clone();
        let t_move = tab_id.clone();
        let t_leave = tab_id.clone();
        let pie = div()
            .id("storage-pie")
            .relative()
            .size(px(PIE_SIZE))
            .flex_shrink_0()
            .child(pie_canvas)
            .child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .size_full()
                    .flex()
                    .flex_col()
                    .items_center()
                    .justify_center()
                    .px(px(PIE_SIZE * (1.0 - PIE_HOLE) / 2.0 + 10.0))
                    .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).max_w_full().whitespace_nowrap().overflow_hidden().text_ellipsis().child(centre.0))
                    .child(div().t(s, 20.0).font_weight(FontWeight::SEMIBOLD).whitespace_nowrap().child(centre.1))
                    .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).whitespace_nowrap().child(centre.2)),
            )
            .on_mouse_move(cx.listener(move |this, e: &MouseMoveEvent, _, cx| {
                let hit = pie_bounds.get().and_then(|b| hit_test(b, e.position, &hit_slices));
                this.set_storage_hover(&t_move, hit, cx);
            }))
            .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                if !*h {
                    this.set_storage_hover(&t_leave, None, cx);
                }
            }));

        let mut legend = div().flex().flex_col().gap(px(1.)).flex_1().min_w(px(0.));
        for (i, sl) in slices.iter().enumerate() {
            let t_hover = tab_id.clone();
            let active = hovered == Some(i);
            let dim = hovered.is_some() && !active;
            legend = legend.child(
                div()
                    .id(SharedString::from(format!("storage-legend-{i}")))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(8.))
                    .py(px(4.))
                    .rounded(px(4.))
                    .t(s, 12.0)
                    .when(active, |d| d.bg(theme::BG_HOVER))
                    .when(dim, |d| d.opacity(0.55))
                    .on_hover(cx.listener(move |this, h: &bool, _, cx| {
                        this.set_storage_hover(&t_hover, if *h { Some(i) } else { None }, cx);
                    }))
                    .child(div().size(px(10.)).flex_shrink_0().rounded(px(2.)).bg(sl.color))
                    .child(
                        div()
                            .flex()
                            .flex_1()
                            .min_w(px(0.))
                            .overflow_hidden()
                            .whitespace_nowrap()
                            .when(!sl.prefix.is_empty(), |d| d.child(div().flex_shrink_0().text_color(theme::TEXT_MUTED).child(format!("{}.", sl.prefix))))
                            .child(div().min_w(px(0.)).overflow_hidden().text_ellipsis().child(sl.label.clone())),
                    )
                    .child(div().flex_shrink_0().w(px(64.)).flex().justify_end().text_color(theme::TEXT_DIM).child(non_empty_bytes(sl.bytes)))
                    .child(div().flex_shrink_0().w(px(46.)).flex().justify_end().text_color(theme::TEXT_MUTED).child(percent(sl.bytes, total))),
            );
        }
        let chart_card = div()
            .flex()
            .flex_col()
            .gap(px(12.))
            .w(px(560.))
            .flex_shrink_0()
            .p(px(16.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_SURFACE)
            .child(card_title(s, "Share of storage", "Top 10 tables, remaining tables grouped"))
            .child(if slices.is_empty() {
                div().t(s, 12.0).text_color(theme::TEXT_MUTED).child("No size information was reported for these tables.").into_any_element()
            } else {
                div().flex().items_center().gap(px(20.)).child(pie).child(legend).into_any_element()
            })
            .when(unsized_count > 0, |d| {
                d.child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).child(format!(
                    "{unsized_count} table{} did not report a size.",
                    if unsized_count == 1 { "" } else { "s" }
                )))
            });

        // ── Table of every table, by size ──
        let t_sort = tab_id.clone();
        let header_cell = |text: &'static str| div().whitespace_nowrap().t(s, 11.0).text_color(theme::TEXT_MUTED).font_weight(FontWeight::SEMIBOLD).child(text);
        let header = div()
            .flex()
            .items_center()
            .gap(px(12.))
            .px(px(12.))
            .py(px(7.))
            .border_b_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_TOOLBAR)
            .child(div().w(px(36.)).flex_shrink_0().flex().justify_end().child(header_cell("#")))
            .child(div().flex_1().min_w(px(0.)).child(header_cell("Table")))
            .child(
                div()
                    .id("storage-sort")
                    .w(px(84.))
                    .flex_shrink_0()
                    .flex()
                    .justify_end()
                    .cursor_pointer()
                    .hover(|st| st.text_color(theme::TEXT))
                    .on_click(cx.listener(move |this, _, _, cx| {
                        if let Some(st) = this.storage_state_mut(&t_sort) {
                            st.descending = !st.descending;
                            st.list.scroll_to_item(0, gpui::ScrollStrategy::Top);
                        }
                        cx.notify();
                    }))
                    .child(header_cell(if descending { "Size ▼" } else { "Size ▲" })),
            )
            .child(div().w(px(56.)).flex_shrink_0().flex().justify_end().child(header_cell("% total")))
            .child(div().flex_1().min_w(px(48.)).max_w(px(220.)).child(header_cell("Relative size")));

        let rows = Rc::new(tables.clone());
        let count = rows.len();
        let scale = s;
        let list = uniform_list("storage-list", count, move |range, _window, _cx| {
            range
                .map(|i| {
                    let t = &rows[i];
                    let rank = if descending { i + 1 } else { count - i };
                    let colour = colour_of.get(&(t.database.clone(), t.schema.clone(), t.name.clone())).copied().unwrap_or(OTHER_COLOR);
                    let bar = if largest > 0 { t.size() as f32 / largest as f32 } else { 0.0 };
                    let prefix = t.prefix(show_db, show_schema);
                    div()
                        .w_full()
                        .flex()
                        .items_center()
                        .gap(px(12.))
                        .px(px(12.))
                        .h(px(30.))
                        .t(scale, 12.0)
                        .border_b_1()
                        .border_color(theme::BORDER_SUBTLE)
                        .when(i % 2 == 1, |d| d.bg(theme::BG_ROW_ALT))
                        .child(div().w(px(36.)).flex_shrink_0().flex().justify_end().text_color(theme::TEXT_MUTED).child(rank.to_string()))
                        .child(
                            div()
                                .flex()
                                .flex_1()
                                .min_w(px(0.))
                                .overflow_hidden()
                                .whitespace_nowrap()
                                .when(!prefix.is_empty(), |d| d.child(div().flex_shrink_0().text_color(theme::TEXT_MUTED).child(format!("{prefix}."))))
                                .child(div().min_w(px(0.)).overflow_hidden().text_ellipsis().child(t.name.clone())),
                        )
                        .child(
                            div()
                                .w(px(84.))
                                .flex_shrink_0()
                                .flex()
                                .justify_end()
                                .when(t.bytes.is_none(), |d| d.text_color(theme::TEXT_MUTED))
                                .child(if t.bytes.is_none() { "—".to_string() } else { non_empty_bytes(t.size()) }),
                        )
                        .child(div().w(px(56.)).flex_shrink_0().flex().justify_end().text_color(theme::TEXT_DIM).child(percent(t.size(), total)))
                        .child(
                            div()
                                .flex_1()
                                .min_w(px(48.))
                                .max_w(px(220.))
                                .h(px(8.))
                                .rounded(px(4.))
                                .bg(theme::rgba8(255, 255, 255, 0.05))
                                .child(div().h_full().rounded(px(4.)).bg(colour).w(gpui::relative(bar)).when(t.size() > 0, |d| d.min_w(px(2.)))),
                        )
                })
                .collect::<Vec<_>>()
        })
        .track_scroll(list_handle)
        .flex_1()
        .min_h(px(0.));
        let table_card = div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w(px(0.))
            .min_h(px(0.))
            .rounded(px(6.))
            .border_1()
            .border_color(theme::BORDER)
            .bg(theme::BG_SURFACE)
            .overflow_hidden()
            .child(div().px(px(16.)).pt(px(14.)).pb(px(10.)).child(card_title(s, "All tables by size", &format!("{} tables", tables.len()))))
            .child(header)
            .child(list);

        view = view.child(
            div()
                .id("storage-body")
                .flex_1()
                .min_h(px(0.))
                .overflow_y_scroll()
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(px(12.))
                        .p(px(12.))
                        .size_full()
                        .min_h(px(500.))
                        .child(tiles)
                        .child(div().flex().gap(px(12.)).flex_1().min_h(px(0.)).items_start().child(chart_card).child(table_card.h_full())),
                ),
        );
        view.into_any_element()
    }
}

fn non_empty_bytes(bytes: i64) -> String {
    let s = format_bytes(Some(bytes));
    if s.is_empty() {
        "0 B".into()
    } else {
        s
    }
}

fn card_title(s: Scale, title: &str, detail: &str) -> gpui::Div {
    div()
        .flex()
        .items_baseline()
        .justify_between()
        .gap(px(8.))
        .child(div().t(s, 13.0).font_weight(FontWeight::SEMIBOLD).child(title.to_string()))
        .child(div().t(s, 11.0).text_color(theme::TEXT_MUTED).child(detail.to_string()))
}

/// Slice under `pos`, if it is on the ring.
fn hit_test(b: Bounds<Pixels>, pos: gpui::Point<Pixels>, slices: &[Slice]) -> Option<usize> {
    let r = f32::from(b.size.width.min(b.size.height)) / 2.0;
    let cx = f32::from(b.left()) + f32::from(b.size.width) / 2.0;
    let cy = f32::from(b.top()) + f32::from(b.size.height) / 2.0;
    let (dx, dy) = (f32::from(pos.x) - cx, f32::from(pos.y) - cy);
    let d = (dx * dx + dy * dy).sqrt();
    if d > r || d < r * PIE_HOLE {
        return None;
    }
    // Clockwise from twelve o'clock.
    let angle = (dx.atan2(-dy) + TAU) % TAU;
    slice_at(slices, angle)
}

fn paint_donut(window: &mut Window, b: Bounds<Pixels>, slices: &[Slice], hovered: Option<usize>) {
    let size = f32::from(b.size.width.min(b.size.height));
    let cx = f32::from(b.left()) + f32::from(b.size.width) / 2.0;
    let cy = f32::from(b.top()) + f32::from(b.size.height) / 2.0;
    let outer = size / 2.0 - 4.0;
    let inner = outer * PIE_HOLE;
    // A 2px gap of panel surface between neighbouring slices.
    let gap = if slices.len() > 1 { 2.0 } else { 0.0 };
    let mut start = 0.0f32;
    for (i, sl) in slices.iter().enumerate() {
        let sweep = sl.fraction * TAU;
        let end = start + sweep;
        let grow = if hovered == Some(i) { 4.0 } else { 0.0 };
        let (ro, ri) = (outer + grow, inner);
        let pad_o = (gap / 2.0) / ro;
        let pad_i = (gap / 2.0) / ri;
        let (a0o, a1o) = (start + pad_o, end - pad_o);
        let (a0i, a1i) = (start + pad_i, end - pad_i);
        if a1o > a0o && a1i > a0i {
            let at = |r: f32, a: f32| point(px(cx + r * a.sin()), px(cy - r * a.cos()));
            let steps = ((sweep / TAU) * 180.0).ceil().max(2.0) as usize;
            let mut pb = PathBuilder::fill();
            pb.move_to(at(ro, a0o));
            for k in 1..=steps {
                pb.line_to(at(ro, a0o + (a1o - a0o) * k as f32 / steps as f32));
            }
            for k in (0..=steps).rev() {
                pb.line_to(at(ri, a0i + (a1i - a0i) * k as f32 / steps as f32));
            }
            pb.close();
            if let Ok(path) = pb.build() {
                let colour = if hovered.is_some() && hovered != Some(i) { theme::with_alpha(sl.color, 0.45) } else { sl.color };
                window.paint_path(path, hsla(colour));
            }
        }
        start = end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::{Database, Schema, Table};

    fn table(name: &str, bytes: Option<i64>) -> Table {
        Table { name: name.into(), table_type: "BASE TABLE".into(), size_bytes: bytes, columns: Vec::new() }
    }

    fn sizes(n: usize) -> Vec<TableSize> {
        (0..n)
            .map(|i| TableSize { database: String::new(), schema: String::new(), name: format!("t{i:02}"), bytes: Some(((i + 1) * 1000) as i64) })
            .collect()
    }

    #[test]
    fn tables_come_largest_first_with_ties_by_name() {
        let tree = SchemaTree {
            tables: vec![table("b", Some(10)), table("a", Some(10)), table("big", Some(500)), table("none", None)],
            ..Default::default()
        };
        let got: Vec<String> = collect_table_sizes(&tree, &StorageScope::default()).into_iter().map(|t| t.name).collect();
        assert_eq!(got, vec!["big", "a", "b", "none"]);
    }

    #[test]
    fn the_top_ten_get_a_slice_each_and_the_rest_share_one() {
        let mut tables = sizes(14);
        sort_tables(&mut tables, true);
        let slices = pie_slices(&tables, TOP_TABLES, false, false);
        assert_eq!(slices.len(), 11);
        assert_eq!(slices[0].label, "t13");
        assert_eq!(slices[0].color, SLICE_COLORS[0]);
        let other = slices.last().unwrap();
        assert_eq!(other.label, "Other (4 tables)");
        assert_eq!(other.bytes, 1000 + 2000 + 3000 + 4000);
        assert_eq!(other.tables, 4);
        assert_eq!(other.color, OTHER_COLOR);
        let sum: f32 = slices.iter().map(|s| s.fraction).sum();
        assert!((sum - 1.0).abs() < 1e-4);
    }

    #[test]
    fn ten_or_fewer_tables_need_no_other_slice_and_empty_tables_take_none() {
        let mut tables = sizes(10);
        tables.push(TableSize { database: String::new(), schema: String::new(), name: "empty".into(), bytes: Some(0) });
        tables.push(TableSize { database: String::new(), schema: String::new(), name: "unknown".into(), bytes: None });
        let slices = pie_slices(&tables, TOP_TABLES, false, false);
        assert_eq!(slices.len(), 10);
        assert!(slices.iter().all(|s| s.tables == 1));
        assert!(pie_slices(&[], TOP_TABLES, false, false).is_empty());
    }

    #[test]
    fn a_scope_narrows_to_one_database_or_schema() {
        let tree = SchemaTree {
            databases: vec![
                Database {
                    name: "app".into(),
                    schemas: vec![
                        Schema { name: "public".into(), tables: vec![table("users", Some(30))], ..Default::default() },
                        Schema { name: "audit".into(), tables: vec![table("events", Some(90))], ..Default::default() },
                    ],
                    ..Default::default()
                },
                Database {
                    name: "billing".into(),
                    schemas: vec![Schema { name: "public".into(), tables: vec![table("invoices", Some(50))], ..Default::default() }],
                    ..Default::default()
                },
            ],
            ..Default::default()
        };
        let all = collect_table_sizes(&tree, &StorageScope::default());
        assert_eq!(all.len(), 3);
        assert_eq!(spans(&all), (true, true));
        assert_eq!(all[0].prefix(true, true), "app.audit");

        let app = collect_table_sizes(&tree, &StorageScope { database: Some("app".into()), ..Default::default() });
        assert_eq!(app.iter().map(|t| t.name.as_str()).collect::<Vec<_>>(), vec!["events", "users"]);
        assert_eq!(spans(&app), (false, true));

        let public = collect_table_sizes(&tree, &StorageScope { database: Some("app".into()), schema: Some("public".into()), label: String::new() });
        assert_eq!(public.len(), 1);
        assert_eq!(spans(&public), (false, false));
    }

    #[test]
    fn mysql_schemas_are_scoped_without_a_database() {
        let tree = SchemaTree {
            schemas: vec![
                Schema { name: "shop".into(), tables: vec![table("orders", Some(5))], ..Default::default() },
                Schema { name: "crm".into(), tables: vec![table("leads", Some(7))], ..Default::default() },
            ],
            ..Default::default()
        };
        let crm = collect_table_sizes(&tree, &StorageScope { schema: Some("crm".into()), ..Default::default() });
        assert_eq!(crm.len(), 1);
        assert_eq!(crm[0].name, "leads");
    }

    #[test]
    fn angles_map_to_slices_clockwise_from_the_top() {
        let tables = vec![
            TableSize { database: String::new(), schema: String::new(), name: "a".into(), bytes: Some(3) },
            TableSize { database: String::new(), schema: String::new(), name: "b".into(), bytes: Some(1) },
        ];
        let slices = pie_slices(&tables, TOP_TABLES, false, false);
        assert_eq!(slice_at(&slices, 0.1), Some(0));
        assert_eq!(slice_at(&slices, TAU * 0.74), Some(0));
        assert_eq!(slice_at(&slices, TAU * 0.76), Some(1));
    }

    #[test]
    fn percentages_keep_slivers_visible() {
        assert_eq!(percent(1, 10_000), "<0.1%");
        assert_eq!(percent(55, 1000), "5.5%");
        assert_eq!(percent(420, 1000), "42%");
        assert_eq!(percent(0, 1000), "0%");
    }

    #[test]
    fn the_slice_palette_has_no_repeats() {
        let set: BTreeSet<String> = SLICE_COLORS.iter().map(|c| format!("{c:?}")).collect();
        assert_eq!(set.len(), SLICE_COLORS.len());
        assert!(!set.contains(&format!("{OTHER_COLOR:?}")));
    }
}
