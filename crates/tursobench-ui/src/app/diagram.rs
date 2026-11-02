use super::{App, shell};
use eframe::egui;
use tursobench_core::{Inspection, SchemaDiagram, Ticket};
use tursobench_db::{Operation, Request};

use std::collections::BTreeMap;
const WIDTH: f32 = 320.0;
const LINE: f32 = 22.0;
const KEY_LABEL_ZOOM: f32 = 0.5;
#[derive(Default)]
pub(super) struct DiagramState {
    pub ticket: Option<Ticket>,
    pub data: Option<SchemaDiagram>,
    pub error: String,
    pub stale: bool,
    pub selected: Option<String>,
    pub search: String,
    pub all_columns: bool,
    pub zoom: f32,
    pub pan: egui::Vec2,
    pub fit: bool,
    focus_selected: bool,
    auto_fit: bool,
    last_canvas_size: egui::Vec2,
    reset_zoom: bool,
    #[cfg(test)]
    pub table_rects: Vec<(String, egui::Rect)>,
    positions: BTreeMap<String, egui::Pos2>,
}
impl DiagramState {
    pub fn freeze_camera(&mut self) {
        self.auto_fit = false;
    }
    pub fn loaded(&mut self, data: SchemaDiagram) {
        self.data = Some(data);
        self.ticket = None;
        self.error.clear();
        self.stale = false;
        self.positions.clear();
        self.fit = true;
        self.focus_selected = false;
        self.auto_fit = true;
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
fn cards(graph: &SchemaDiagram, all: bool) -> Vec<(egui::Rect, Vec<usize>)> {
    let columns = ((graph.tables.len() as f32).sqrt().ceil() as usize).max(1);
    let mut result = Vec::new();
    let mut y = 0.0;
    for group in graph.tables.chunks(columns) {
        let mut height = 0.0f32;
        for (i, node) in group.iter().enumerate() {
            let rows = node
                .inspection
                .as_ref()
                .map_or_else(Vec::new, |info| key_rows(info, all));
            let h = 70.0 + rows.len().max(1) as f32 * LINE;
            height = height.max(h);
            result.push((
                egui::Rect::from_min_size(
                    egui::pos2(i as f32 * (WIDTH + 70.0), y),
                    egui::vec2(WIDTH, h),
                ),
                rows,
            ));
        }
        y += height + 110.0;
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
impl App {
    pub(super) fn load_diagram(&mut self) {
        if !self.ready || self.closing || self.ui.diagram.ticket.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Diagram,
        }) {
            Ok(()) => {
                self.ui.diagram.ticket = Some(ticket);
                self.ui.diagram.error.clear();
            }
            Err(e) => self.ui.diagram.error = e.to_string(),
        }
    }
    pub(super) fn diagram_ui(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        self.ui.diagram.table_rects.clear();
        ui.horizontal_wrapped(|ui| {
            ui.heading("Schema diagram");
            if ui
                .add_enabled(
                    self.ready && !self.closing && self.ui.diagram.ticket.is_none(),
                    egui::Button::new("Refresh all tables"),
                )
                .clicked()
            {
                self.load_diagram();
            }
            if ui.button("Fit all").clicked() {
                self.ui.diagram.fit = true;
                self.ui.diagram.focus_selected = false;
                self.ui.diagram.auto_fit = true;
            }
            if ui.button("100%").clicked() {
                self.ui.diagram.reset_zoom = true;
                self.ui.diagram.auto_fit = false;
            }
            if ui
                .checkbox(&mut self.ui.diagram.all_columns, "All columns")
                .changed()
            {
                self.ui.diagram.positions.clear();
                self.ui.diagram.fit = true;
                self.ui.diagram.auto_fit = true;
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.ui.diagram.search)
                    .hint_text("Find table…")
                    .desired_width(150.0),
            );
            if ui.button("Find").clicked()
                && let Some(graph) = &self.ui.diagram.data
                && let Some(table) = graph.tables.iter().find(|t| {
                    t.name
                        .to_lowercase()
                        .contains(&self.ui.diagram.search.to_lowercase())
                })
            {
                self.ui.diagram.selected = Some(table.name.clone());
                self.tools.ddl = None;
                self.ui.diagram.focus_selected = true;
                self.ui.diagram.auto_fit = true;
                self.ui.diagram.fit = true;
                self.ui.inspector = shell::Inspector::Structure;
                self.ui.inspector_open = true;
            }
        });
        ui.small("Declared relationships only · PK = primary-key position · UQ = unique-index member · FK = foreign-key member. Drag background to pan; wheel to zoom; drag table headers; click a table to open its Data / Structure drawer.");
        if self.ui.diagram.ticket.is_some() {
            ui.label(
                "Loading committed metadata on the worker… Closing still waits for active work.",
            );
        }
        if !self.ui.diagram.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.ui.diagram.error);
        }
        let Some(graph) = &self.ui.diagram.data else {
            ui.label("Open a database and choose Refresh all tables. No row scans or inferred relationships.");
            return;
        };
        let relationships: usize = graph
            .tables
            .iter()
            .filter_map(|t| t.inspection.as_ref())
            .map(|i| i.foreign_keys.len())
            .sum();
        let errors = graph.tables.iter().filter(|t| t.error.is_some()).count();
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "{} tables · {relationships} foreign keys · {errors} metadata errors",
                graph.tables.len()
            ));
            if graph.limited {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "LIMITED — not the entire schema (512 tables / 8 MiB metadata budget)",
                );
            }
            if self.ui.diagram.stale {
                ui.colored_label(
                    ui.visuals().warn_fg_color,
                    "Cached — may be stale; refresh after SQL/schema changes",
                );
            }
        });
        let mut layout = cards(graph, self.ui.diagram.all_columns);
        for (node, (rect, _)) in graph.tables.iter().zip(&mut layout) {
            if let Some(pos) = self.ui.diagram.positions.get(&node.name) {
                *rect = egui::Rect::from_min_size(*pos, rect.size());
            }
        }
        let (canvas, response) = ui.allocate_exact_size(
            ui.available_size().max(egui::vec2(1.0, 1.0)),
            egui::Sense::drag(),
        );
        let painter = ui.painter_at(canvas);
        painter.rect_filled(canvas, 4.0, ui.visuals().extreme_bg_color);
        let bounds = layout
            .iter()
            .fold(egui::Rect::NOTHING, |r, (next, _)| r.union(*next));
        if self.ui.diagram.last_canvas_size != canvas.size() && self.ui.diagram.auto_fit {
            self.ui.diagram.fit = true;
        }
        self.ui.diagram.last_canvas_size = canvas.size();
        if self.ui.diagram.zoom <= 0.0 {
            self.ui.diagram.zoom = 1.0;
        }
        if self.ui.diagram.fit && bounds.is_finite() {
            let focus = if self.ui.diagram.focus_selected {
                self.ui
                    .diagram
                    .selected
                    .as_ref()
                    .and_then(|s| graph.tables.iter().position(|t| &t.name == s))
                    .map(|i| layout[i].0)
            } else {
                None
            };
            let target = focus.unwrap_or(bounds).expand(60.0);
            self.ui.diagram.zoom = ((canvas.width() - 40.0) / target.width())
                .min((canvas.height() - 40.0) / target.height())
                .clamp(0.0001, 1.25);
            self.ui.diagram.pan =
                canvas.size() * 0.5 - target.center().to_vec2() * self.ui.diagram.zoom;
            self.ui.diagram.fit = false;
        }
        if self.ui.diagram.reset_zoom {
            let center = canvas.size() * 0.5;
            self.ui.diagram.pan = center - (center - self.ui.diagram.pan) / self.ui.diagram.zoom;
            self.ui.diagram.zoom = 1.0;
            self.ui.diagram.reset_zoom = false;
        }
        if response.dragged() {
            self.ui.diagram.pan += response.drag_delta();
            self.ui.diagram.auto_fit = false;
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll != 0.0 {
                self.ui.diagram.auto_fit = false;
                let old = self.ui.diagram.zoom;
                let new = (old * (scroll * 0.002).exp()).clamp(0.0001, 2.5);
                let pointer = ui
                    .input(|i| i.pointer.hover_pos())
                    .unwrap_or(canvas.center())
                    - canvas.min;
                self.ui.diagram.pan = pointer - (pointer - self.ui.diagram.pan) * (new / old);
                self.ui.diagram.zoom = new;
            }
        }
        let zoom = self.ui.diagram.zoom;
        let pan = self.ui.diagram.pan;
        let point = |p: egui::Pos2| canvas.min + pan + p.to_vec2() * zoom;
        let rect = |r: egui::Rect| egui::Rect::from_min_max(point(r.min), point(r.max));
        let accent = ui.visuals().selection.stroke.color;
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
                    let row_y = |i: usize, name: Option<&str>| {
                        let card = &layout[i];
                        graph.tables[i]
                            .inspection
                            .as_ref()
                            .and_then(|info| {
                                name.and_then(|name| {
                                    card.1.iter().position(|&c| {
                                        info.table.columns[c].name.eq_ignore_ascii_case(name)
                                    })
                                })
                            })
                            .map_or(card.0.top() + 22.0, |r| {
                                card.0.top() + 48.0 + r as f32 * LINE
                            })
                    };
                    let (path, end_side) = route(
                        layout[source].0,
                        layout[target].0,
                        row_y(source, Some(from)),
                        row_y(target, to.as_deref()),
                        part,
                    );
                    let points: Vec<_> = path.into_iter().map(point).collect();
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
                        .ui
                        .diagram
                        .selected
                        .as_ref()
                        .is_some_and(|n| n == &node.name || n.eq_ignore_ascii_case(&fk.target));
                    let color = if highlighted {
                        accent
                    } else {
                        ui.visuals().weak_text_color()
                    };
                    painter.add(egui::Shape::line(
                        points,
                        egui::Stroke::new(if highlighted { 2.0 } else { 1.0 }, color),
                    ));
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
        let mut selected = None;
        let mut browse = None;
        for (i, node) in graph.tables.iter().enumerate() {
            let (world, columns) = &layout[i];
            let r = rect(*world);
            if !r.intersects(canvas) {
                continue;
            }
            #[cfg(test)]
            self.ui.diagram.table_rects.push((node.name.clone(), r));
            let painter = painter.with_clip_rect(r.intersect(canvas));
            let is_selected = self.ui.diagram.selected.as_ref() == Some(&node.name);
            painter.rect_filled(r, 4.0, ui.visuals().panel_fill);
            painter.rect_stroke(
                r,
                4.0,
                egui::Stroke::new(
                    if is_selected { 2.0 } else { 1.0 },
                    if is_selected {
                        accent
                    } else {
                        ui.visuals().widgets.noninteractive.bg_stroke.color
                    },
                ),
                egui::StrokeKind::Inside,
            );
            let header = egui::Rect::from_min_size(r.min, egui::vec2(r.width(), 34.0 * zoom));
            painter.rect_filled(header, 3.0, ui.visuals().selection.bg_fill);
            painter.text(
                point(world.min + egui::vec2(10.0, 17.0)),
                egui::Align2::LEFT_CENTER,
                short(&node.name, 36),
                egui::FontId::proportional(if zoom >= 0.35 {
                    (15.0 * zoom).max(11.0)
                } else {
                    15.0 * zoom
                }),
                ui.visuals().text_color(),
            );
            let response = ui.interact(
                r.intersect(canvas),
                ui.id().with(("diagram-node", &node.name)),
                egui::Sense::click(),
            );
            if response.clicked() {
                selected = Some(node.name.clone());
            }
            if response.double_clicked() {
                browse = Some(node.name.clone());
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
                ui.id().with(("diagram-drag", &node.name)),
                egui::Sense::click_and_drag(),
            );
            if drag.clicked() {
                selected = Some(node.name.clone());
            }
            if drag.double_clicked() {
                browse = Some(node.name.clone());
            }
            if drag.dragged() {
                self.ui.diagram.auto_fit = false;
                self.ui
                    .diagram
                    .positions
                    .insert(node.name.clone(), world.min + drag.drag_delta() / zoom);
            }
            if let Some(info) = &node.inspection {
                for (row, &c) in columns.iter().enumerate() {
                    if zoom < KEY_LABEL_ZOOM {
                        break;
                    }
                    let y = point(world.min + egui::vec2(10.0, 48.0 + row as f32 * LINE)).y;
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
                        point(world.min + egui::vec2(10.0, 48.0 + row as f32 * LINE)),
                        egui::Align2::LEFT_CENTER,
                        format!(
                            "{pk}{fk}{uq}{}  {}",
                            short(&col.name, 24),
                            short(&col.declared_type, 12)
                        ),
                        egui::FontId::monospace((12.0 * zoom).max(9.0)),
                        ui.visuals().text_color(),
                    );
                }
                painter.text(
                    point(egui::pos2(world.left() + 10.0, world.bottom() - 16.0)),
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
                    egui::FontId::proportional(if zoom >= 0.35 {
                        (12.0 * zoom).max(9.0)
                    } else {
                        12.0 * zoom
                    }),
                    ui.visuals().weak_text_color(),
                );
            } else {
                painter.text(
                    point(world.min + egui::vec2(10.0, 50.0)),
                    egui::Align2::LEFT_CENTER,
                    "Metadata unavailable — inspect error",
                    egui::FontId::proportional(12.0 * zoom),
                    ui.visuals().error_fg_color,
                );
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
            egui::FontId::proportional(12.0),
            ui.visuals().weak_text_color(),
        );
        if unresolved > 0 {
            painter.text(canvas.left_bottom()+egui::vec2(10.0,-10.0),egui::Align2::LEFT_BOTTOM,format!("{unresolved} unresolved relationship endpoints — inspect Structure; no targets invented"),egui::FontId::proportional(12.0),ui.visuals().warn_fg_color);
        }
        if let Some(name) = browse.or(selected) {
            self.browse_table(name);
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use tursobench_core::DiagramTable;
    #[test]
    fn links_use_facing_ports_in_both_directions_and_keep_self_loops_outside() {
        let left = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH, 120.0));
        let right = left.translate(egui::vec2(WIDTH + 70.0, 0.0));
        let (forward, side) = route(left, right, 48.0, 70.0, 0);
        assert_eq!(forward[0].x, left.right());
        assert_eq!(forward.last().unwrap().x, right.left());
        assert_eq!(side, -1.0);
        let (back, side) = route(right, left, 48.0, 70.0, 0);
        assert_eq!(back[0].x, right.left());
        assert_eq!(back.last().unwrap().x, left.right());
        assert_eq!(side, 1.0);
        let (cycle, _) = route(left, left, 48.0, 48.0, 0);
        assert!(cycle[1..cycle.len() - 1].iter().all(|p| p.x > left.right()));
    }
    #[test]
    fn isolated_and_failed_tables_still_have_non_overlapping_cards() {
        let graph = SchemaDiagram {
            tables: (0..12)
                .map(|i| DiagramTable {
                    name: format!("table{i}"),
                    inspection: None,
                    error: Some("unavailable".into()),
                })
                .collect(),
            limited: false,
        };
        let layout = cards(&graph, false);
        assert_eq!(layout.len(), 12);
        for (i, (a, _)) in layout.iter().enumerate() {
            for (b, _) in &layout[i + 1..] {
                assert!(!a.intersects(*b));
            }
        }
    }
}
