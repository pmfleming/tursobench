mod foreign_map;
pub(super) use foreign_map::ForeignMap;

use super::icons::{self, Icon};
use eframe::egui;
use tursobench_core::{DiagramTable, Inspection, SchemaDiagram, Ticket};

use std::collections::{BTreeMap, BTreeSet};
const WIDTH: f32 = 320.0;
const LINE: f32 = 22.0;
const KEY_LABEL_ZOOM: f32 = 0.5;
pub(super) const MIN_ZOOM: f32 = 0.0001;
pub(super) const MAX_ZOOM: f32 = 2.5;
#[derive(Default)]
pub(super) struct DiagramState {
    pub ticket: Option<Ticket>,
    pub data: Option<SchemaDiagram>,
    pub error: String,
    pub stale: bool,
    pub selected: Option<String>,
    pub show_system: bool,
    pub all_columns: bool,
    pub camera: Camera,
    pack_width: f32,
    layout: Option<CachedLayout>,
    #[cfg(test)]
    pub table_rects: Vec<(String, egui::Rect)>,
    positions: BTreeMap<String, egui::Pos2>,
}
struct CachedLayout {
    key: (bool, bool, f32),
    cards: Vec<(egui::Rect, Vec<usize>)>,
}
#[derive(Default)]
pub(super) struct Camera {
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub fit: bool,
    auto_fit: bool,
    last_canvas_size: egui::Vec2,
    requested_zoom: Option<f32>,
}

struct Graph<'a> {
    tables: Vec<&'a DiagramTable>,
    limited: bool,
}
impl<'a> Graph<'a> {
    fn new(data: &'a SchemaDiagram, show_system: bool) -> Self {
        Self {
            tables: data
                .tables
                .iter()
                .filter(|t| show_system || !t.name.starts_with("__turso_internal_"))
                .collect(),
            limited: data.limited,
        }
    }
    /// Each table's position, by its name in lower case.
    fn positions(&self) -> BTreeMap<String, usize> {
        self.tables
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.to_ascii_lowercase(), i))
            .collect()
    }
    /// Counts, and warnings that the map is partial or may be stale.
    fn notices(&self, ui: &mut egui::Ui, stale: bool) {
        let relationships: usize = self
            .tables
            .iter()
            .filter_map(|t| t.inspection.as_ref())
            .map(|i| i.foreign_keys.len())
            .sum();
        let errors = self.tables.iter().filter(|t| t.error.is_some()).count();
        ui.horizontal_wrapped(|ui| {
            if self.limited || errors > 0 {
                ui.small(format!(
                    "{} tables · {relationships} links · {errors} metadata errors",
                    self.tables.len()
                ));
            }
            if self.limited {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "LIMITED — not the entire schema (512 tables / 8 MiB metadata budget)",
                );
            }
            if stale {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Cached — may be stale; refresh after SQL/schema changes",
                );
            }
        });
    }
    /// How wide to pack the cards so the map roughly takes the canvas's shape.
    fn pack_width(&self, all: bool, canvas: egui::Vec2) -> f32 {
        let aspect = (canvas.x / canvas.y.max(1.0)).clamp(0.5, 8.0);
        let area: f32 = self
            .tables
            .iter()
            .map(|node| {
                let rows = node
                    .inspection
                    .as_ref()
                    .map_or(1, |i| key_rows(i, all).len().max(1));
                (WIDTH + 40.0) * (100.0 + rows as f32 * LINE)
            })
            .sum();
        let columns = ((area * aspect).sqrt() / (WIDTH + 40.0)).ceil().max(1.0);
        (columns * (WIDTH + 40.0)).clamp(WIDTH, 3600.0)
    }
}

impl DiagramState {
    pub fn arrange_nodes(&mut self) {
        self.layout = None;
        self.positions.clear();
        self.pack_width = 0.0;
        self.camera.fit = true;
    }
    pub fn toggle_columns(&mut self) {
        self.all_columns = !self.all_columns;
        self.arrange_nodes();
        self.camera.auto_fit = true;
    }
    pub fn toggle_system(&mut self) {
        self.show_system = !self.show_system;
        self.camera.fit = true;
    }
    pub fn snapshot(&self, database: &str) -> Option<crate::storage::MapLayout> {
        self.data.as_ref()?;
        if self.camera.zoom <= 0.0 {
            return None;
        }
        Some(crate::storage::MapLayout {
            database: database.into(),
            positions: self
                .positions
                .iter()
                .map(|(name, p)| (name.clone(), [p.x, p.y]))
                .collect(),
            pan: [self.camera.pan.x, self.camera.pan.y],
            zoom: self.camera.zoom,
            pack_width: self.pack_width,
            all_columns: self.all_columns,
            selected: self.selected.clone(),
        })
    }
    pub fn restore(&mut self, layout: &crate::storage::MapLayout) {
        let Some(graph) = &self.data else {
            return;
        };
        self.layout = None;
        self.positions = layout
            .positions
            .iter()
            .filter(|(name, _)| graph.tables.iter().any(|t| &t.name == *name))
            .map(|(name, p)| (name.clone(), egui::pos2(p[0], p[1])))
            .collect();
        self.camera.pan = egui::vec2(layout.pan[0], layout.pan[1]);
        self.camera.zoom = layout.zoom;
        self.pack_width = layout.pack_width;
        self.all_columns = layout.all_columns;
        self.selected = layout
            .selected
            .clone()
            .filter(|name| graph.tables.iter().any(|t| &t.name == name));
        self.camera.fit = false;
        self.camera.auto_fit = false;
    }
    pub fn freeze_camera(&mut self) {
        self.camera.auto_fit = false;
    }
    pub fn loaded(&mut self, data: SchemaDiagram) {
        self.layout = None;
        self.data = Some(data);
        self.ticket = None;
        self.error.clear();
        self.stale = false;
        self.positions.retain(|name, _| {
            self.data
                .as_ref()
                .is_some_and(|g| g.tables.iter().any(|t| &t.name == name))
        });
        if self.camera.zoom <= 0.0 {
            self.camera.fit = true;
            self.camera.auto_fit = true;
        }
    }
}
fn key_rows(info: &Inspection, all: bool) -> Vec<usize> {
    let members: std::collections::BTreeSet<_> = info
        .foreign_keys
        .iter()
        .flat_map(|f| f.columns.iter().map(|(s, _)| s.to_ascii_lowercase()))
        .chain(
            info.indexes
                .iter()
                .filter(|i| i.unique)
                .flat_map(|i| i.columns.iter().flatten().map(|n| n.to_ascii_lowercase())),
        )
        .collect();
    info.table
        .columns
        .iter()
        .enumerate()
        .filter(|(_, c)| all || c.primary_key > 0 || members.contains(&c.name.to_ascii_lowercase()))
        .map(|(i, _)| i)
        .collect()
}
// Stable breadth-first layers within each connected component. Parent hubs
// start each component; cycles and self references cannot recurse indefinitely.
#[cfg(test)]
thread_local! { static LAYOUT_BUILDS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn cards(graph: &Graph<'_>, all: bool, pack_width: f32) -> Vec<(egui::Rect, Vec<usize>)> {
    #[cfg(test)]
    LAYOUT_BUILDS.with(|count| count.set(count.get() + 1));
    let count = graph.tables.len();
    let (edges, incoming, outgoing) = neighbours(graph);
    let mut result: Vec<_> = graph
        .tables
        .iter()
        .map(|node| {
            let rows = node
                .inspection
                .as_ref()
                .map_or_else(Vec::new, |i| key_rows(i, all));
            let size = egui::vec2(WIDTH, 70.0 + rows.len().max(1) as f32 * LINE);
            (egui::Rect::from_min_size(egui::Pos2::ZERO, size), rows)
        })
        .collect();
    let heights: Vec<_> = result
        .iter()
        .map(|(rect, _)| rect.height() + 45.0)
        .collect();
    // The height a layout packed to `pack_width` would reach; a long level wraps
    // into side-by-side columns instead of growing past it.
    let area: f32 = heights.iter().map(|h| (WIDTH + 40.0) * h).sum();
    let tallest = heights.iter().copied().fold(0.0, f32::max);
    let column_height = (area / pack_width.max(WIDTH)).max(tallest);
    let mut unseen: BTreeSet<_> = (0..count).collect();
    let mut shelf = egui::Pos2::ZERO;
    let mut shelf_height = 0.0f32;
    let mut roots: Vec<_> = (0..count).collect();
    roots.sort_unstable_by_key(|&i| (outgoing[i] != 0, std::cmp::Reverse(incoming[i]), i));
    for root in roots {
        if !unseen.remove(&root) {
            continue;
        }
        let levels = component_levels(root, &edges, &mut unseen);
        let columns = wrap_levels(&levels, &heights, column_height);
        let component_width = columns.len() as f32 * (WIDTH + 40.0);
        if shelf.x > 0.0 && shelf.x + component_width > pack_width {
            shelf.x = 0.0;
            shelf.y += shelf_height + 30.0;
            shelf_height = 0.0;
        }
        for (depth, column) in columns.iter().enumerate() {
            let mut y = shelf.y;
            for &i in column {
                let x = shelf.x + depth as f32 * (WIDTH + 40.0);
                result[i].0 = egui::Rect::from_min_size(egui::pos2(x, y), result[i].0.size());
                y += heights[i];
            }
            shelf_height = shelf_height.max(y - shelf.y);
        }
        shelf.x += component_width;
    }
    result
}
/// Each table's related tables, and how many foreign keys point into and out
/// of it. Self references are left out.
fn neighbours(graph: &Graph<'_>) -> (Vec<BTreeSet<usize>>, Vec<usize>, Vec<usize>) {
    let count = graph.tables.len();
    let names = graph.positions();
    let mut edges = vec![BTreeSet::new(); count];
    let mut incoming = vec![0usize; count];
    let mut outgoing = vec![0usize; count];
    for (i, node) in graph.tables.iter().enumerate() {
        let keys = node.inspection.iter().flat_map(|info| &info.foreign_keys);
        for fk in keys {
            let Some(&j) = names.get(&fk.target.to_ascii_lowercase()) else {
                continue;
            };
            if i != j {
                edges[i].insert(j);
                edges[j].insert(i);
                incoming[j] += 1;
                outgoing[i] += 1;
            }
        }
    }
    (edges, incoming, outgoing)
}
/// The tables reachable from `root`, by distance, taking each from `unseen`.
fn component_levels(
    root: usize,
    edges: &[BTreeSet<usize>],
    unseen: &mut BTreeSet<usize>,
) -> Vec<Vec<usize>> {
    let mut queue = std::collections::VecDeque::from([(root, 0usize)]);
    let mut levels: Vec<Vec<usize>> = Vec::new();
    while let Some((i, depth)) = queue.pop_front() {
        if levels.len() <= depth {
            levels.push(Vec::new());
        }
        levels[depth].push(i);
        for &j in &edges[i] {
            if unseen.remove(&j) {
                queue.push_back((j, depth + 1));
            }
        }
    }
    levels
}
/// Columns of cards, one per level, wrapping a level taller than `limit`.
fn wrap_levels(levels: &[Vec<usize>], heights: &[f32], limit: f32) -> Vec<Vec<usize>> {
    let mut columns: Vec<Vec<usize>> = Vec::new();
    for level in levels {
        let mut column = Vec::new();
        let mut height = 0.0;
        for &i in level {
            if !column.is_empty() && height + heights[i] > limit {
                columns.push(std::mem::take(&mut column));
                height = 0.0;
            }
            column.push(i);
            height += heights[i];
        }
        columns.push(column);
    }
    columns
}

/// A key column's line on its card, "PK1 FK UQ name  type", and whether it
/// takes part in a relationship.
fn key_row_text(info: &Inspection, col: &tursobench_core::TableColumn) -> (String, bool) {
    let named = |name: &str| name.eq_ignore_ascii_case(&col.name);
    let fk = info
        .foreign_keys
        .iter()
        .any(|f| f.columns.iter().any(|(n, _)| named(n)));
    let uq = info
        .indexes
        .iter()
        .any(|i| i.unique && i.columns.iter().flatten().any(|n| named(n)));
    let pk = if col.primary_key > 0 {
        format!("PK{} ", col.primary_key)
    } else {
        String::new()
    };
    let text = format!(
        "{pk}{}{}{}  {}",
        if fk { "FK " } else { "" },
        if uq { "UQ " } else { "" },
        short(&col.name, 24),
        short(&col.declared_type, 12)
    );
    (text, col.primary_key > 0 || fk)
}
/// Text scaled with the map, kept legible down to 35% zoom; below that it
/// shrinks freely as the cards do.
fn scaled_text(size: f32, floor: f32, zoom: f32) -> f32 {
    if zoom >= 0.35 {
        (size * zoom).max(floor)
    } else {
        size * zoom
    }
}
fn short(text: &str, limit: usize) -> String {
    let mut s: String = text.chars().take(limit).collect();
    if text.chars().count() > limit {
        s.push('…');
    }
    s
}
// Choose the facing ports; reverse relationships must not cross their own cards.
fn route(
    source: egui::Rect,
    target: egui::Rect,
    sy: f32,
    ty: f32,
    part: usize,
) -> (Vec<egui::Pos2>, f32) {
    route_with_bend(source, target, sy, ty, 25.0 + (part % 4) as f32 * 10.0)
}
fn route_with_bend(
    source: egui::Rect,
    target: egui::Rect,
    sy: f32,
    ty: f32,
    bend: f32,
) -> (Vec<egui::Pos2>, f32) {
    if (source.center().x - target.center().x).abs() < 1.0 {
        let start = egui::pos2(source.right(), sy);
        let end = egui::pos2(target.right(), ty);
        let outside = source.right().max(target.right()) + bend;
        let points = if start == end {
            vec![
                start,
                egui::pos2(outside, sy),
                egui::pos2(outside, sy - 25.0),
                egui::pos2(source.right() + 12.0, sy - 25.0),
                egui::pos2(source.right() + 12.0, sy),
                end,
            ]
        } else {
            vec![start, egui::pos2(outside, sy), egui::pos2(outside, ty), end]
        };
        (points, 1.0)
    } else {
        let rightward = target.center().x > source.center().x;
        let side = if rightward { 1.0 } else { -1.0 };
        let start = egui::pos2(
            if rightward {
                source.right()
            } else {
                source.left()
            },
            sy,
        );
        let end = egui::pos2(
            if rightward {
                target.left()
            } else {
                target.right()
            },
            ty,
        );
        (
            vec![
                start,
                start + egui::vec2(side * bend, 0.0),
                end - egui::vec2(side * bend, 0.0),
                end,
            ],
            -side,
        )
    }
}
struct DiagramDrawing<'a> {
    graph: &'a Graph<'a>,
    layout: &'a [(egui::Rect, Vec<usize>)],
    canvas: egui::Rect,
    zoom: f32,
    pan: egui::Vec2,
    selected: Option<&'a str>,
}
/// Shared table-card treatment for the full diagram and the compact Structure map.
fn node_frame(
    ui: &egui::Ui,
    painter: &egui::Painter,
    rect: egui::Rect,
    selected: bool,
    related: bool,
    header_height: f32,
) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    painter.rect_filled(rect, 4.0, ui.visuals().panel_fill);
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(
            if selected || related { 2.0 } else { 1.0 },
            if selected {
                ui.visuals().selection.stroke.color
            } else if related {
                tokens.relationship
            } else {
                tokens.line
            },
        ),
        egui::StrokeKind::Inside,
    );
    let header = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), header_height));
    painter.rect_filled(
        header,
        3.0,
        if selected {
            ui.visuals().selection.bg_fill
        } else {
            tokens.raised
        },
    );
}
fn paint_grid(painter: &egui::Painter, canvas: egui::Rect, pan: egui::Vec2, color: egui::Color32) {
    let grid_origin = canvas.min + egui::vec2(pan.x.rem_euclid(18.0), pan.y.rem_euclid(18.0));
    let mut y = grid_origin.y;
    while y < canvas.bottom() {
        let mut x = grid_origin.x;
        while x < canvas.right() {
            painter.circle_filled(egui::pos2(x, y), 0.75, color);
            x += 18.0;
        }
        y += 18.0;
    }
}
impl Camera {
    pub fn reset_zoom(&mut self) {
        self.request_zoom(1.0);
    }
    pub fn zoom(&self) -> f32 {
        self.requested_zoom
            .unwrap_or(if self.zoom > 0.0 { self.zoom } else { 1.0 })
    }
    pub fn step_zoom(&mut self, factor: f32) {
        self.request_zoom(self.zoom() * factor);
    }
    fn request_zoom(&mut self, zoom: f32) {
        self.requested_zoom = Some(zoom.clamp(MIN_ZOOM, MAX_ZOOM));
        self.fit = false;
        self.auto_fit = false;
    }
    fn zoom_at(&mut self, zoom: f32, anchor: egui::Vec2) {
        let old = if self.zoom > 0.0 { self.zoom } else { 1.0 };
        self.pan = anchor - (anchor - self.pan) * (zoom / old);
        self.zoom = zoom;
    }
    fn apply_zoom(&mut self, canvas_size: egui::Vec2) {
        if let Some(zoom) = self.requested_zoom.take() {
            self.zoom_at(zoom, canvas_size * 0.5);
        }
    }
    fn update(
        &mut self,
        ui: &egui::Ui,
        canvas: egui::Rect,
        response: &egui::Response,
        bounds: egui::Rect,
    ) {
        if self.last_canvas_size != canvas.size() && self.auto_fit {
            self.fit = true;
        }
        self.last_canvas_size = canvas.size();
        if self.zoom <= 0.0 {
            self.zoom = 1.0;
        }
        if self.fit && bounds.is_finite() {
            let target = bounds.expand(16.0);
            self.zoom = ((canvas.width() - 40.0) / target.width())
                .min((canvas.height() - 40.0) / target.height())
                .clamp(MIN_ZOOM, 1.25);
            self.pan = canvas.size() * 0.5 - target.center().to_vec2() * self.zoom;
            self.fit = false;
        }
        self.apply_zoom(canvas.size());
        if response.dragged() {
            self.pan += response.drag_delta();
            self.auto_fit = false;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.auto_fit = false;
                let old = self.zoom;
                let new = (old * (scroll * 0.002).exp()).clamp(MIN_ZOOM, MAX_ZOOM);
                let pointer = ui
                    .input(|i| i.pointer.hover_pos())
                    .unwrap_or(canvas.center())
                    - canvas.min;
                self.zoom_at(new, pointer);
            }
        }
    }
}
impl DiagramDrawing<'_> {
    fn point(&self, p: egui::Pos2) -> egui::Pos2 {
        self.canvas.min + self.pan + p.to_vec2() * self.zoom
    }
    fn row_y(&self, table: usize, name: Option<&str>) -> f32 {
        let (rect, rows) = &self.layout[table];
        let row = self.graph.tables[table]
            .inspection
            .as_ref()
            .zip(name)
            .and_then(|(info, name)| {
                rows.iter()
                    .position(|&c| info.table.columns[c].name.eq_ignore_ascii_case(name))
            });
        row.map_or(rect.top() + 22.0, |row| {
            rect.top() + 48.0 + row as f32 * LINE
        })
    }
    fn node(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        i: usize,
        related: bool,
    ) -> (bool, Option<egui::Pos2>) {
        let node = self.graph.tables[i];
        let (world, _) = &self.layout[i];
        let r = egui::Rect::from_min_max(self.point(world.min), self.point(world.max));
        let canvas = self.canvas;
        let zoom = self.zoom;
        let accent = ui.visuals().selection.stroke.color;
        let is_selected = self.selected == Some(node.name.as_str());
        if is_selected {
            painter.rect_stroke(
                r.expand(3.0),
                8.0,
                egui::Stroke::new(3.0, accent.gamma_multiply(0.13)),
                egui::StrokeKind::Outside,
            );
        }
        let painter = painter.with_clip_rect(r.intersect(canvas));
        node_frame(ui, &painter, r, is_selected, related, 34.0 * zoom);
        let header = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), 34.0 * zoom));
        painter.text(
            self.point(world.min + egui::vec2(10.0, 17.0)),
            egui::Align2::LEFT_CENTER,
            short(&node.name, 36),
            crate::appearance::proportional(ui.ctx(), scaled_text(15.0, 11.0, zoom)),
            ui.visuals().text_color(),
        );
        let response = ui.interact(
            r.intersect(canvas),
            ui.scope_id().with(("diagram-node", &node.name)),
            egui::Sense::click(),
        );
        let mut clicked = response.clicked();
        response.on_hover_text(format!(
            "{}\n{}",
            node.name,
            node.error
                .as_deref()
                .unwrap_or("Click to browse its rows · drag the title to move it")
        ));
        let drag = ui.interact(
            header.intersect(canvas),
            ui.scope_id().with(("diagram-drag", &node.name)),
            egui::Sense::click_and_drag(),
        );
        clicked |= drag.clicked();
        let moved = drag.dragged().then(|| world.min + drag.drag_delta() / zoom);
        self.details(ui, &painter, i);
        (clicked, moved)
    }
    fn details(&self, ui: &egui::Ui, painter: &egui::Painter, index: usize) {
        let (world, columns) = &self.layout[index];
        let zoom = self.zoom;
        let canvas = self.canvas;
        let Some(info) = &self.graph.tables[index].inspection else {
            painter.text(
                self.point(world.min + egui::vec2(10.0, 50.0)),
                egui::Align2::LEFT_CENTER,
                "Metadata unavailable — inspect error",
                crate::appearance::proportional(ui.ctx(), 12.0 * zoom),
                ui.visuals().error_fg_color,
            );
            return;
        };
        if zoom >= KEY_LABEL_ZOOM {
            for (row, &c) in columns.iter().enumerate() {
                let at = self.point(world.min + egui::vec2(10.0, 48.0 + row as f32 * LINE));
                if at.y < canvas.top() - LINE * zoom || at.y > canvas.bottom() + LINE * zoom {
                    continue;
                }
                let (text, related) = key_row_text(info, &info.table.columns[c]);
                let color = if related {
                    crate::appearance::Tokens::of(ui.ctx()).relationship
                } else {
                    ui.visuals().text_color()
                };
                let font = crate::appearance::monospace(ui.ctx(), (12.0 * zoom).max(9.0));
                painter.text(at, egui::Align2::LEFT_CENTER, text, font, color);
            }
        }
        let (columns, keys) = (info.table.columns.len(), info.foreign_keys.len());
        let summary = if zoom < KEY_LABEL_ZOOM {
            format!("{columns} columns · {keys} FKs")
        } else {
            format!(
                "{columns} columns · {} indexes · {keys} FKs",
                info.indexes.len()
            )
        };
        let size = scaled_text(12.0, 9.0, zoom);
        painter.text(
            self.point(egui::pos2(world.left() + 10.0, world.bottom() - 16.0)),
            egui::Align2::LEFT_CENTER,
            summary,
            crate::appearance::proportional(ui.ctx(), size),
            ui.visuals().weak_text_color(),
        );
    }
    fn links(&self, ui: &egui::Ui, painter: &egui::Painter) -> usize {
        let Self {
            graph,
            layout,
            canvas,
            zoom,
            ..
        } = *self;
        let accent = crate::appearance::Tokens::of(ui.ctx()).relationship;
        let index = graph.positions();
        let mut unresolved = 0;
        for (source, node) in graph.tables.iter().enumerate() {
            let Some(info) = &node.inspection else {
                continue;
            };
            for fk in &info.foreign_keys {
                let Some(&target) = index.get(&fk.target.to_ascii_lowercase()) else {
                    unresolved += fk.columns.len();
                    continue;
                };
                for (part, (from, to)) in fk.columns.iter().enumerate() {
                    if !graph.tables[target].inspection.as_ref().is_some_and(|i| {
                        to.as_ref().is_some_and(|name| {
                            i.table
                                .columns
                                .iter()
                                .any(|c| c.name.eq_ignore_ascii_case(name))
                        })
                    }) {
                        unresolved += 1;
                    }
                    let (path, end_side) = route(
                        layout[source].0,
                        layout[target].0,
                        self.row_y(source, Some(from)),
                        self.row_y(target, to.as_deref()),
                        part,
                    );
                    let points: Vec<_> = path.into_iter().map(|p| self.point(p)).collect();
                    let start = points[0];
                    let Some(&end) = points.last() else {
                        continue;
                    };
                    if !egui::Rect::from_two_pos(start, end)
                        .expand(100.0 * zoom)
                        .intersects(canvas)
                    {
                        continue;
                    }
                    let highlighted = self
                        .selected
                        .as_ref()
                        .is_some_and(|n| n == &node.name || n.eq_ignore_ascii_case(&fk.target));
                    let color = if highlighted {
                        accent
                    } else {
                        accent.gamma_multiply(0.55)
                    };
                    painter.add(egui::Shape::line(points, egui::Stroke::new(1.6, color)));
                    painter.circle_filled(start, 2.5, color);
                    painter.circle_filled(end, 2.5, color);
                    if zoom >= 0.25 {
                        painter.arrow(
                            end + egui::vec2(end_side * 10.0, 0.0),
                            egui::vec2(-end_side * 10.0, 0.0),
                            egui::Stroke::new(1.5, color),
                        );
                    }
                }
            }
        }
        unresolved
    }
}
impl DiagramState {
    pub(super) fn show(
        &mut self,
        ui: &mut egui::Ui,
        relatives: &BTreeSet<String>,
    ) -> Option<String> {
        #[cfg(test)]
        self.table_rects.clear();
        if self.ticket.is_some() {
            ui.label(
                "Loading committed metadata on the worker… Closing still waits for active work.",
            );
        }
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
        let Some(data) = self.data.as_ref() else {
            if self.ticket.is_none() && self.error.is_empty() {
                icons::empty_state(ui, Icon::Map, "No relationships to display");
            }
            return None;
        };
        let graph = Graph::new(data, self.show_system);
        graph.notices(ui, self.stale);
        if graph.tables.is_empty() {
            if self.ticket.is_none() && self.error.is_empty() {
                icons::empty_state(ui, Icon::Map, "No relationships to display");
            }
            return None;
        }
        if self.pack_width <= 0.0
            || (self.camera.auto_fit && self.camera.last_canvas_size != ui.available_size())
        {
            self.pack_width = graph.pack_width(self.all_columns, ui.available_size());
        }
        let key = (self.show_system, self.all_columns, self.pack_width);
        if self.layout.as_ref().is_none_or(|layout| layout.key != key) {
            self.layout = Some(CachedLayout {
                key,
                cards: cards(&graph, self.all_columns, self.pack_width),
            });
        }
        let layout = &mut self.layout.as_mut().unwrap().cards;
        for (node, (rect, _)) in graph.tables.iter().zip(layout.iter_mut()) {
            if let Some(pos) = self.positions.get(&node.name) {
                *rect = egui::Rect::from_min_size(*pos, rect.size());
            }
        }
        let (canvas, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(1.0, 1.0)),
            egui::Sense::drag(),
        );
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 4.0, ui.visuals().extreme_bg_color);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        paint_grid(&painter, canvas, self.camera.pan, tokens.line);
        let bounds = layout
            .iter()
            .fold(egui::Rect::NOTHING, |r, (next, _)| r.union(*next));
        self.camera.update(ui, canvas, &response, bounds);
        let zoom = self.camera.zoom;
        let pan = self.camera.pan;
        let point = |p: egui::Pos2| canvas.min + pan + p.to_vec2() * zoom;
        let rect = |r: egui::Rect| egui::Rect::from_min_max(point(r.min), point(r.max));
        let drawing = DiagramDrawing {
            graph: &graph,
            layout,
            canvas,
            zoom,
            pan,
            selected: self.selected.as_deref(),
        };
        let unresolved = drawing.links(ui, &painter);
        let mut selected = None;
        for (i, node) in graph.tables.iter().enumerate() {
            let (world, _) = &layout[i];
            let r = rect(*world);
            if !r.intersects(canvas) {
                continue;
            }
            #[cfg(test)]
            self.table_rects.push((node.name.clone(), r));
            let (clicked, moved) = drawing.node(ui, &painter, i, relatives.contains(&node.name));
            if clicked {
                selected = Some(node.name.clone());
            }
            if let Some(pos) = moved {
                self.camera.auto_fit = false;
                self.positions.insert(node.name.clone(), pos);
            }
        }
        let hint = if zoom < KEY_LABEL_ZOOM {
            " · zoom in for key labels"
        } else {
            ""
        };
        let font = crate::appearance::proportional(ui.ctx(), 12.0);
        painter.text(
            canvas.left_top() + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            format!("{:.0}%{hint}", zoom * 100.0),
            font.clone(),
            ui.visuals().weak_text_color(),
        );
        if unresolved > 0 {
            painter.text(
                canvas.left_bottom() + egui::vec2(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{unresolved} unresolved relationship endpoints — inspect Structure; no targets invented"),
                font,
                ui.visuals().warn_fg_color,
            );
        }
        selected
    }
}
#[cfg(test)]
mod tests {
    use super::{Camera, Graph, Inspection, SchemaDiagram, WIDTH, cards, route};
    use eframe::egui;
    use tursobench_core::DiagramTable;
    #[test]
    fn camera_and_selection_reuse_layout_but_schema_and_layout_changes_rebuild_it() {
        let graph = || SchemaDiagram {
            tables: (0..32)
                .map(|i| DiagramTable {
                    name: format!("table_{i}"),
                    inspection: None,
                    error: None,
                })
                .chain(std::iter::once(DiagramTable {
                    name: "__turso_internal_hidden".into(),
                    inspection: None,
                    error: None,
                }))
                .collect(),
            limited: false,
        };
        let ctx = egui::Context::default();
        let mut state = super::DiagramState::default();
        state.loaded(graph());
        let draw = |state: &mut super::DiagramState, size| {
            crate::app::test_ui::render(&ctx, size, vec![], |ui| {
                state.show(ui, &Default::default());
            });
        };
        let size = egui::vec2(800.0, 500.0);
        draw(&mut state, size);
        draw(&mut state, size);
        let builds = || super::LAYOUT_BUILDS.with(|count| count.get());
        let initial = builds();
        state.camera.pan += egui::vec2(10.0, 20.0);
        state.camera.step_zoom(1.25);
        state.selected = Some("table_0".into());
        state
            .positions
            .insert("table_0".into(), egui::pos2(123.0, 456.0));
        draw(&mut state, size);
        draw(&mut state, size);
        assert_eq!(builds(), initial);
        assert_eq!(
            state.layout.as_ref().unwrap().cards[0].0.min,
            egui::pos2(123.0, 456.0)
        );

        state.camera.auto_fit = true;
        draw(&mut state, egui::vec2(1600.0, 400.0));
        assert!(
            builds() > initial,
            "packing-width changes must rebuild layout"
        );
        let before = builds();
        state.toggle_columns();
        draw(&mut state, size);
        assert!(builds() > before);
        assert!(state.positions.is_empty());
        let before = builds();
        state.toggle_system();
        draw(&mut state, size);
        assert!(builds() > before);
        assert_eq!(state.layout.as_ref().unwrap().cards.len(), 33);
        let before = builds();
        state.loaded(graph());
        draw(&mut state, size);
        assert!(builds() > before);
        let before = builds();
        let saved = state.snapshot("db").unwrap();
        state.restore(&saved);
        draw(&mut state, size);
        assert!(builds() > before);
        let before = builds();
        draw(&mut state, size);
        assert_eq!(builds(), before);
    }
    #[test]
    fn button_zoom_preserves_the_canvas_center_and_respects_limits() {
        let mut camera = Camera {
            zoom: 0.5,
            pan: egui::vec2(30.0, -40.0),
            auto_fit: true,
            fit: true,
            ..Default::default()
        };
        let size = egui::vec2(800.0, 400.0);
        let world_center = (size * 0.5 - camera.pan) / camera.zoom;
        for factor in [1.25, 1.0 / 1.25, 1000.0, 0.0000001] {
            camera.step_zoom(factor);
            camera.apply_zoom(size);
            assert!((super::MIN_ZOOM..=super::MAX_ZOOM).contains(&camera.zoom));
            assert!(((size * 0.5 - camera.pan) / camera.zoom - world_center).length() < 0.1);
            assert!(!camera.auto_fit && !camera.fit);
        }
        camera.reset_zoom();
        camera.apply_zoom(size);
        assert_eq!(camera.zoom, 1.0);
        assert!(((size * 0.5 - camera.pan) / camera.zoom - world_center).length() < 0.1);
    }

    /// A table with no columns whose foreign keys point at `targets`.
    fn table(name: &str, targets: &[&str]) -> DiagramTable {
        use tursobench_core::{ForeignKey, Table};
        DiagramTable {
            name: name.into(),
            error: None,
            inspection: Some(Inspection {
                table: Table {
                    name: name.into(),
                    columns: vec![],
                    is_table: true,
                    has_rowid: true,
                    binary_collations: true,
                },
                indexes: vec![],
                foreign_keys: targets
                    .iter()
                    .map(|target| ForeignKey {
                        id: 0,
                        target: (*target).into(),
                        columns: vec![],
                        on_update: "NO ACTION".into(),
                        on_delete: "NO ACTION".into(),
                    })
                    .collect(),
            }),
        }
    }
    #[test]
    fn a_hub_with_many_children_spreads_across_a_wide_tile() {
        let data = SchemaDiagram {
            tables: std::iter::once(table("hub", &[]))
                .chain((0..12).map(|i| table(&format!("child{i}"), &["hub"])))
                .collect(),
            limited: false,
        };
        let layout = cards(&Graph::new(&data, false), false, 2000.0);
        let bounds = layout
            .iter()
            .fold(egui::Rect::NOTHING, |r, (next, _)| r.union(*next));
        // One long level wraps into several columns, so the map is wider than tall.
        assert!(bounds.width() > bounds.height(), "{bounds:?}");
        let columns: std::collections::BTreeSet<_> =
            layout[1..].iter().map(|(r, _)| r.left() as i32).collect();
        assert!(columns.len() > 1);
        // The hub still sits left of every child, and no cards overlap.
        assert!(
            layout[1..]
                .iter()
                .all(|(r, _)| r.left() > layout[0].0.right())
        );
        for (i, (a, _)) in layout.iter().enumerate() {
            assert!(layout[i + 1..].iter().all(|(b, _)| !a.intersects(*b)));
        }
    }

    #[test]
    fn layouts_keep_relationships_deterministic_and_all_cards_separate() {
        let mut graph = SchemaDiagram {
            tables: vec![
                table("child", &["parent", "child", "missing"]),
                table("cycle_a", &["cycle_b"]),
                table("cycle_b", &["cycle_a"]),
                table("parent", &[]),
            ],
            limited: false,
        };
        graph.tables.extend((0..508).map(|i| DiagramTable {
            name: format!("isolated{i}"),
            inspection: None,
            error: Some("unavailable".into()),
        }));
        assert!(cards(&Graph::new(&SchemaDiagram::default(), false), false, 1200.0).is_empty());
        let layout = cards(&Graph::new(&graph, false), false, 1200.0);
        assert_eq!(layout.len(), 512);
        graph.tables[4].name = "__turso_internal_hidden".into();
        let visible = Graph::new(&graph, false);
        assert_eq!(visible.tables.len(), 511);
        assert!(std::ptr::eq(visible.tables[0], &graph.tables[0]));
        assert_eq!(Graph::new(&graph, true).tables.len(), 512);
        graph.tables[4].name = "isolated0".into();
        assert_eq!(layout, cards(&Graph::new(&graph, false), false, 1200.0));
        assert!(layout[3].0.left() < layout[0].0.left());
        assert_eq!(layout[3].0.top(), layout[0].0.top());
        for (i, (a, _)) in layout.iter().enumerate() {
            for (b, _) in &layout[i + 1..] {
                assert!(!a.intersects(*b));
            }
        }
    }
    #[test]
    fn links_use_facing_ports_in_both_directions_and_keep_self_loops_outside() {
        let left = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH, 120.0));
        let right = left.translate(egui::vec2(WIDTH + 70.0, 0.0));
        let (forward, side) = route(left, right, 48.0, 70.0, 0);
        assert_eq!(forward[0].x, left.right());
        assert_eq!(forward.last().map(|p| p.x), Some(right.left()));
        assert_eq!(side, -1.0);
        let (back, side) = route(right, left, 48.0, 70.0, 0);
        assert_eq!(back[0].x, right.left());
        assert_eq!(back.last().map(|p| p.x), Some(left.right()));
        assert_eq!(side, 1.0);
        let (cycle, _) = route(left, left, 48.0, 48.0, 0);
        assert!(cycle[1..cycle.len() - 1].iter().all(|p| p.x > left.right()));
    }
}
