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
    #[cfg(test)]
    pub table_rects: Vec<(String, egui::Rect)>,
    positions: BTreeMap<String, egui::Pos2>,
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
}

impl DiagramState {
    pub fn arrange_nodes(&mut self) {
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
fn cards(graph: &Graph<'_>, all: bool, pack_width: f32) -> Vec<(egui::Rect, Vec<usize>)> {
    use std::collections::{BTreeSet, VecDeque};
    let count = graph.tables.len();
    let names: BTreeMap<_, _> = graph
        .tables
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name.to_ascii_lowercase(), i))
        .collect();
    let mut edges = vec![BTreeSet::new(); count];
    let mut incoming = vec![0usize; count];
    let mut outgoing = vec![0usize; count];
    for (i, node) in graph.tables.iter().enumerate() {
        if let Some(info) = &node.inspection {
            for fk in &info.foreign_keys {
                if let Some(&j) = names.get(&fk.target.to_ascii_lowercase()) {
                    if i == j {
                        continue;
                    }
                    edges[i].insert(j);
                    edges[j].insert(i);
                    incoming[j] += 1;
                    outgoing[i] += 1;
                }
            }
        }
    }
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
    let mut unseen: BTreeSet<_> = (0..count).collect();
    let mut shelf = egui::Pos2::ZERO;
    let mut shelf_height = 0.0f32;
    while let Some(&root) = unseen
        .iter()
        .max_by_key(|&&i| (outgoing[i] == 0, incoming[i], std::cmp::Reverse(i)))
    {
        unseen.remove(&root);
        let mut queue = VecDeque::from([(root, 0usize)]);
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
        let component_width = levels.len() as f32 * (WIDTH + 40.0);
        if shelf.x > 0.0 && shelf.x + component_width > pack_width {
            shelf.x = 0.0;
            shelf.y += shelf_height + 30.0;
            shelf_height = 0.0;
        }
        for (depth, level) in levels.iter().enumerate() {
            let mut y = shelf.y;
            for &i in level {
                result[i].0 = egui::Rect::from_min_size(
                    egui::pos2(shelf.x + depth as f32 * (WIDTH + 40.0), y),
                    result[i].0.size(),
                );
                y += result[i].0.height() + 45.0;
            }
            shelf_height = shelf_height.max(y - shelf.y);
        }
        shelf.x += component_width;
    }
    result
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
    let bend = 25.0 + (part % 4) as f32 * 10.0;
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
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let mut clicked = false;
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
        painter.rect_filled(r, 7.0, ui.visuals().panel_fill);
        painter.rect_stroke(
            r,
            4.0,
            egui::Stroke::new(
                if is_selected || related { 2.0 } else { 1.0 },
                if is_selected {
                    accent
                } else if related {
                    tokens.relationship
                } else {
                    ui.visuals().widgets.noninteractive.bg_stroke.color
                },
            ),
            egui::StrokeKind::Inside,
        );
        let header = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), 34.0 * zoom));
        painter.rect_filled(
            header,
            6.0,
            if is_selected {
                ui.visuals().selection.bg_fill
            } else {
                tokens.raised
            },
        );
        painter.text(
            self.point(world.min + egui::vec2(10.0, 17.0)),
            egui::Align2::LEFT_CENTER,
            short(&node.name, 36),
            crate::appearance::proportional(
                ui.ctx(),
                if zoom >= 0.35 {
                    (15.0 * zoom).max(11.0)
                } else {
                    15.0 * zoom
                },
            ),
            ui.visuals().text_color(),
        );
        let response = ui.interact(
            r.intersect(canvas),
            ui.scope_id().with(("diagram-node", &node.name)),
            egui::Sense::click(),
        );
        if response.clicked() {
            clicked = true;
        }
        response.on_hover_text(format!(
            "{}\n{}",
            node.name,
            node.error
                .as_deref()
                .unwrap_or("Click: open Data / Structure drawer")
        ));
        let drag = ui.interact(
            header.intersect(canvas),
            ui.scope_id().with(("diagram-drag", &node.name)),
            egui::Sense::click_and_drag(),
        );
        if drag.clicked() {
            clicked = true;
        }
        let moved = drag.dragged().then(|| world.min + drag.drag_delta() / zoom);
        self.details(ui, &painter, i);
        (clicked, moved)
    }
    fn details(&self, ui: &egui::Ui, painter: &egui::Painter, index: usize) {
        let (world, columns) = &self.layout[index];
        let node = &self.graph.tables[index];
        let zoom = self.zoom;
        let canvas = self.canvas;
        if let Some(info) = &node.inspection {
            for (row, &c) in columns.iter().enumerate() {
                if zoom < KEY_LABEL_ZOOM {
                    break;
                }
                let y = self
                    .point(world.min + egui::vec2(10.0, 48.0 + row as f32 * LINE))
                    .y;
                if y < canvas.top() - LINE * zoom || y > canvas.bottom() + LINE * zoom {
                    continue;
                }
                let col = &info.table.columns[c];
                let pk = if col.primary_key > 0 {
                    format!("PK{} ", col.primary_key)
                } else {
                    String::new()
                };
                let fk = if info.foreign_keys.iter().any(|f| {
                    f.columns
                        .iter()
                        .any(|(n, _)| n.eq_ignore_ascii_case(&col.name))
                }) {
                    "FK "
                } else {
                    ""
                };
                let uq = if info.indexes.iter().any(|i| {
                    i.unique
                        && i.columns
                            .iter()
                            .flatten()
                            .any(|n| n.eq_ignore_ascii_case(&col.name))
                }) {
                    "UQ "
                } else {
                    ""
                };
                painter.text(
                    self.point(world.min + egui::vec2(10.0, 48.0 + row as f32 * LINE)),
                    egui::Align2::LEFT_CENTER,
                    format!(
                        "{pk}{fk}{uq}{}  {}",
                        short(&col.name, 24),
                        short(&col.declared_type, 12)
                    ),
                    crate::appearance::monospace(ui.ctx(), (12.0 * zoom).max(9.0)),
                    if col.primary_key > 0 || !fk.is_empty() {
                        crate::appearance::Tokens::of(ui.ctx()).relationship
                    } else {
                        ui.visuals().text_color()
                    },
                );
            }
            painter.text(
                self.point(egui::pos2(world.left() + 10.0, world.bottom() - 16.0)),
                egui::Align2::LEFT_CENTER,
                if zoom < KEY_LABEL_ZOOM {
                    format!(
                        "{} columns · {} FKs",
                        info.table.columns.len(),
                        info.foreign_keys.len()
                    )
                } else {
                    format!(
                        "{} columns · {} indexes · {} FKs",
                        info.table.columns.len(),
                        info.indexes.len(),
                        info.foreign_keys.len()
                    )
                },
                crate::appearance::proportional(
                    ui.ctx(),
                    if zoom >= 0.35 {
                        (12.0 * zoom).max(9.0)
                    } else {
                        12.0 * zoom
                    },
                ),
                ui.visuals().weak_text_color(),
            );
        } else {
            painter.text(
                self.point(world.min + egui::vec2(10.0, 50.0)),
                egui::Align2::LEFT_CENTER,
                "Metadata unavailable — inspect error",
                crate::appearance::proportional(ui.ctx(), 12.0 * zoom),
                ui.visuals().error_fg_color,
            );
        }
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
        let index: BTreeMap<_, _> = graph
            .tables
            .iter()
            .enumerate()
            .map(|(i, t)| (t.name.to_ascii_lowercase(), i))
            .collect();
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
        let relationships: usize = graph
            .tables
            .iter()
            .filter_map(|t| t.inspection.as_ref())
            .map(|i| i.foreign_keys.len())
            .sum();
        let errors = graph.tables.iter().filter(|t| t.error.is_some()).count();
        ui.horizontal_wrapped(|ui| {
            if graph.limited || errors > 0 {
                ui.small(format!(
                    "{} tables · {relationships} links · {errors} metadata errors",
                    graph.tables.len()
                ));
            }
            if graph.limited {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "LIMITED — not the entire schema (512 tables / 8 MiB metadata budget)",
                );
            }
            if self.stale {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Cached — may be stale; refresh after SQL/schema changes",
                );
            }
        });
        if graph.tables.is_empty() {
            if self.ticket.is_none() && self.error.is_empty() {
                icons::empty_state(ui, Icon::Map, "No relationships to display");
            }
            return None;
        }
        if self.pack_width <= 0.0
            || (self.camera.auto_fit && self.camera.last_canvas_size != ui.available_size())
        {
            let aspect = (ui.available_width() / ui.available_height().max(1.0)).clamp(0.5, 4.0);
            let area: f32 = graph
                .tables
                .iter()
                .map(|node| {
                    let rows = node
                        .inspection
                        .as_ref()
                        .map_or(1, |i| key_rows(i, self.all_columns).len().max(1));
                    (WIDTH + 40.0) * (100.0 + rows as f32 * LINE)
                })
                .sum();
            let columns = ((area * aspect).sqrt() / (WIDTH + 40.0)).ceil().max(1.0);
            self.pack_width = (columns * (WIDTH + 40.0)).clamp(WIDTH, 3600.0);
        }
        let mut layout = cards(&graph, self.all_columns, self.pack_width);
        for (node, (rect, _)) in graph.tables.iter().zip(&mut layout) {
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
            layout: &layout,
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
        painter.text(
            canvas.left_top() + egui::vec2(10.0, 8.0),
            egui::Align2::LEFT_TOP,
            format!(
                "{:.0}%{}",
                zoom * 100.0,
                if zoom < KEY_LABEL_ZOOM {
                    " · zoom in for key labels"
                } else {
                    ""
                }
            ),
            crate::appearance::proportional(ui.ctx(), 12.0),
            ui.visuals().weak_text_color(),
        );
        if unresolved > 0 {
            painter.text(canvas.left_bottom()+egui::vec2(10.0,-10.0),egui::Align2::LEFT_BOTTOM,format!("{unresolved} unresolved relationship endpoints — inspect Structure; no targets invented"),crate::appearance::proportional(ui.ctx(), 12.0),ui.visuals().warn_fg_color);
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

    #[test]
    fn layouts_keep_relationships_deterministic_and_all_cards_separate() {
        use tursobench_core::{ForeignKey, Table};
        let mut graph = SchemaDiagram {
            tables: [
                ("child", vec!["parent", "child", "missing"]),
                ("cycle_a", vec!["cycle_b"]),
                ("cycle_b", vec!["cycle_a"]),
                ("parent", vec![]),
            ]
            .into_iter()
            .map(|(name, targets)| DiagramTable {
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
                        .into_iter()
                        .map(|target| ForeignKey {
                            id: 0,
                            target: target.into(),
                            columns: vec![],
                            on_update: "NO ACTION".into(),
                            on_delete: "NO ACTION".into(),
                        })
                        .collect(),
                }),
            })
            .collect(),
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
