//! A fixed three-column view over retained schema metadata. Rendering never reads rows.
use super::{node_frame, route_with_bend};
use crate::app::{
    icons::{self, Icon},
    schema::presentation::text_at,
};
use eframe::egui;
use tursobench_core::{ForeignKey, Inspection, SchemaDiagram};

pub(in crate::app) struct ForeignMap<'a> {
    pub info: &'a Inspection,
    pub graph: Option<&'a SchemaDiagram>,
    pub incomplete: bool,
}
struct Node<'a> {
    name: &'a str,
    rows: Vec<String>,
    rect: egui::Rect,
}
struct Edge<'a> {
    source: usize,
    target: usize,
    source_row: usize,
    target_row: usize,
    key: &'a ForeignKey,
    part: usize,
    label: String,
}
struct Layout<'a> {
    nodes: Vec<Node<'a>>,
    edges: Vec<Edge<'a>>,
    height: f32,
}
const HEADER: f32 = 21.0;
const ROW: f32 = 16.0;
fn port(rows: &mut Vec<String>, name: &str) -> usize {
    if let Some(i) = rows.iter().position(|r| r.eq_ignore_ascii_case(name)) {
        return i;
    }
    rows.push(name.into());
    rows.len() - 1
}
impl<'a> ForeignMap<'a> {
    pub fn count(&self) -> usize {
        self.info.foreign_keys.len()
            + self
                .graph
                .into_iter()
                .flat_map(|g| &g.tables)
                .flat_map(|t| t.inspection.iter().flat_map(|i| &i.foreign_keys))
                .filter(|k| k.target.eq_ignore_ascii_case(&self.info.table.name))
                .count()
    }
    fn layout(&self, width: f32) -> Layout<'a> {
        let gap = (width * 0.09).clamp(20.0, 34.0);
        let nw = ((width - gap * 2.0) / 3.0).max(1.0);
        let mut nodes = vec![Node {
            name: &self.info.table.name,
            rows: vec![],
            rect: egui::Rect::NOTHING,
        }];
        let mut edges = Vec::new();
        let mut left = Vec::new();
        let mut right = Vec::new();
        for key in &self.info.foreign_keys {
            let index = nodes.len();
            left.push(index);
            let rows = key
                .columns
                .iter()
                .map(|(_, to)| to.clone().unwrap_or_else(|| "unresolved PK".into()))
                .collect();
            nodes.push(Node {
                name: &key.target,
                rows,
                rect: egui::Rect::NOTHING,
            });
            for (part, (from, to)) in key.columns.iter().enumerate() {
                let source_row = port(&mut nodes[0].rows, from);
                edges.push(Edge {
                    source: 0,
                    target: index,
                    source_row,
                    target_row: part,
                    key,
                    part,
                    label: format!(
                        "{}.{} references {}.{} · ON UPDATE {} · ON DELETE {}",
                        self.info.table.name,
                        from,
                        key.target,
                        to.as_deref().unwrap_or("unresolved PK"),
                        key.on_update,
                        key.on_delete
                    ),
                });
            }
        }
        for table in self.graph.into_iter().flat_map(|g| &g.tables) {
            for key in table
                .inspection
                .iter()
                .flat_map(|i| &i.foreign_keys)
                .filter(|k| k.target.eq_ignore_ascii_case(&self.info.table.name))
            {
                let index = nodes.len();
                right.push(index);
                nodes.push(Node {
                    name: &table.name,
                    rows: key.columns.iter().map(|(from, _)| from.clone()).collect(),
                    rect: egui::Rect::NOTHING,
                });
                for (part, (from, to)) in key.columns.iter().enumerate() {
                    let target_row =
                        port(&mut nodes[0].rows, to.as_deref().unwrap_or("unresolved PK"));
                    edges.push(Edge {
                        source: index,
                        target: 0,
                        source_row: part,
                        target_row,
                        key,
                        part,
                        label: format!(
                            "{}.{} references {}.{} · ON UPDATE {} · ON DELETE {}",
                            table.name,
                            from,
                            self.info.table.name,
                            to.as_deref().unwrap_or("unresolved PK"),
                            key.on_update,
                            key.on_delete
                        ),
                    });
                }
            }
        }
        let node_height = |n: &Node<'_>| HEADER + ROW * n.rows.len() as f32 + 2.0;
        let stack_height = |ids: &[usize]| {
            ids.iter()
                .map(|&i| node_height(&nodes[i]) + 6.0)
                .sum::<f32>()
                .max(6.0)
                - 6.0
        };
        let lh = stack_height(&left);
        let rh = stack_height(&right);
        let height = lh.max(rh).max(node_height(&nodes[0]));
        for (ids, x, stack) in [(&left, 0.0, lh), (&right, 2.0 * (nw + gap), rh)] {
            let mut y = (height - stack) / 2.0;
            for &i in ids {
                let h = node_height(&nodes[i]);
                nodes[i].rect = egui::Rect::from_min_size(egui::pos2(x, y), egui::vec2(nw, h));
                y += h + 6.0;
            }
        }
        let center_height = node_height(&nodes[0]);
        nodes[0].rect = egui::Rect::from_min_size(
            egui::pos2(nw + gap, (height - center_height) / 2.0),
            egui::vec2(nw, center_height),
        );
        Layout {
            nodes,
            edges,
            height,
        }
    }
    pub fn show(&self, ui: &mut egui::Ui) -> Option<String> {
        if self.incomplete {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Cached or incomplete schema — references may be missing",
            );
        }
        if self.count() == 0 {
            ui.weak("No declared foreign keys");
            return None;
        }
        let layout = self.layout(ui.available_width());
        let (canvas, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), layout.height + 4.0),
            egui::Sense::hover(),
        );
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let rect = |index: usize| {
            layout.nodes[index]
                .rect
                .translate(canvas.min.to_vec2() + egui::vec2(0.0, 2.0))
        };
        let mut chosen = None;
        for (i, edge) in layout.edges.iter().enumerate() {
            let source = rect(edge.source);
            let target = rect(edge.target);
            let sy = source.top() + HEADER + ROW * (edge.source_row as f32 + 0.5);
            let ty = target.top() + HEADER + ROW * (edge.target_row as f32 + 0.5);
            let (points, end_side) = route_with_bend(source, target, sy, ty, 6.0);
            let start = points[0];
            let end = *points.last().unwrap();
            ui.painter().add(egui::Shape::line(
                points,
                egui::Stroke::new(1.6, tokens.relationship),
            ));
            ui.painter().circle_filled(start, 2.5, tokens.relationship);
            ui.painter().arrow(
                end + egui::vec2(end_side * 6.0, 0.0),
                egui::vec2(-end_side * 6.0, 0.0),
                egui::Stroke::new(1.5, tokens.relationship),
            );
            let middle = (start + end.to_vec2()) / 2.0;
            let hit = egui::Rect::from_center_size(
                middle,
                egui::vec2((start.x - end.x).abs().max(14.0), 14.0),
            );
            let response = ui.interact(
                hit,
                ui.scope_id().with(("foreign-connector", i)),
                egui::Sense::hover(),
            );
            response
                .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Image, true, &edge.label));
            response.on_hover_text(&edge.label);
            if edge.part == 0 {
                actions(ui, middle, edge.key);
            }
        }
        for (i, node) in layout.nodes.iter().enumerate() {
            let r = rect(i);
            let painter = ui.painter().with_clip_rect(r.intersect(ui.clip_rect()));
            node_frame(ui, &painter, r, i == 0, false, HEADER);
            text_at(
                ui,
                egui::Rect::from_min_max(
                    r.min + egui::vec2(5.0, 0.0),
                    egui::pos2(r.right() - 4.0, r.top() + HEADER),
                ),
                node.name,
                ui.visuals().text_color(),
                11.5,
            );
            for (row, name) in node.rows.iter().enumerate() {
                let area = egui::Rect::from_min_size(
                    r.min + egui::vec2(5.0, HEADER + row as f32 * ROW),
                    egui::vec2(r.width() - 9.0, ROW),
                );
                text_at(
                    ui,
                    area,
                    name,
                    if name == "unresolved PK" {
                        tokens.pending
                    } else {
                        tokens.relationship
                    },
                    10.5,
                );
            }
            let response = ui.interact(
                r,
                ui.scope_id().with(("foreign-node", i)),
                egui::Sense::click(),
            );
            let label = format!(
                "{} {} · {}",
                if i == 0 {
                    "Current table"
                } else {
                    "Open table"
                },
                node.name,
                node.rows.join(", ")
            );
            response.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Button, i != 0 && ui.is_enabled(), &label)
            });
            if response.on_hover_text(label).clicked() && i != 0 {
                chosen = Some(node.name.to_owned());
            }
        }
        chosen
    }
}
fn actions(ui: &egui::Ui, center: egui::Pos2, key: &ForeignKey) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let exceptions: Vec<_> = [
        (true, key.on_delete.as_str()),
        (false, key.on_update.as_str()),
    ]
    .into_iter()
    .filter(|(_, action)| !action.eq_ignore_ascii_case("NO ACTION"))
    .collect();
    for (i, (delete, action)) in exceptions.iter().enumerate() {
        let rect = egui::Rect::from_center_size(
            center + egui::vec2(0.0, (i as f32 - (exceptions.len() - 1) as f32 / 2.0) * 15.0),
            egui::vec2(18.0, 13.0),
        );
        ui.painter().rect_filled(rect, 2.0, ui.visuals().panel_fill);
        let icon = match action.to_ascii_uppercase().as_str() {
            "CASCADE" => Some(if *delete { Icon::Trash } else { Icon::Refresh }),
            "RESTRICT" => Some(Icon::Lock),
            _ => None,
        };
        if let Some(icon) = icon {
            icons::paint(
                ui.painter(),
                rect.shrink2(egui::vec2(3.0, 0.0)),
                icon,
                tokens.pending,
            );
        } else {
            text_at(
                ui,
                rect,
                if action.eq_ignore_ascii_case("SET NULL") {
                    "NULL"
                } else if action.eq_ignore_ascii_case("SET DEFAULT") {
                    "DEF"
                } else {
                    action
                },
                tokens.pending,
                8.0,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tursobench_core::{DiagramTable, Table};
    fn inspection(name: &str, foreign_keys: Vec<ForeignKey>) -> Inspection {
        Inspection {
            table: Table {
                name: name.into(),
                columns: vec![],
                is_table: true,
                has_rowid: true,
                binary_collations: true,
            },
            indexes: vec![],
            foreign_keys,
        }
    }
    fn key(target: &str, id: i64) -> ForeignKey {
        ForeignKey {
            id,
            target: target.into(),
            columns: vec![("a".into(), Some("id_a".into())), ("b".into(), None)],
            on_update: "CASCADE".into(),
            on_delete: "RESTRICT".into(),
        }
    }
    #[test]
    fn compact_layout_includes_both_directions_composites_self_keys_and_unresolved_ports() {
        let info = inspection(
            "current",
            vec![key("parent", 0), key("parent", 1), key("current", 2)],
        );
        let graph = SchemaDiagram {
            tables: vec![DiagramTable {
                name: "child".into(),
                inspection: Some(inspection("child", vec![key("current", 0)])),
                error: None,
            }],
            limited: true,
        };
        let map = ForeignMap {
            info: &info,
            graph: Some(&graph),
            incomplete: true,
        };
        assert_eq!(map.count(), 4);
        for width in [280.0, 396.0, 562.0, 1162.0] {
            let layout = map.layout(width);
            assert_eq!(layout.nodes.len(), 5);
            assert_eq!(layout.edges.len(), 8);
            for node in &layout.nodes {
                assert!(node.rect.is_finite());
                assert!(node.rect.left() >= 0.0 && node.rect.right() <= width + 0.1);
                assert!(node.rect.top() >= 0.0 && node.rect.bottom() <= layout.height + 0.1);
            }
            for edge in &layout.edges {
                let source = &layout.nodes[edge.source];
                let target = &layout.nodes[edge.target];
                assert!(edge.source_row < source.rows.len());
                assert!(edge.target_row < target.rows.len());
                if edge.source == 0 {
                    assert!(target.rect.right() < source.rect.left());
                } else {
                    assert!(source.rect.left() > target.rect.right());
                }
            }
            assert!(
                layout
                    .edges
                    .iter()
                    .any(|e| e.label.contains("unresolved PK"))
            );
        }
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut action = None;
        let mut render = |events| {
            crate::app::test_ui::render(&ctx, egui::vec2(396.0, 500.0), events, |ui| {
                action = map.show(ui);
            })
        };
        let output = render(vec![]);
        let update = output.platform_output.accesskit_update.as_ref().unwrap();
        assert!(update.nodes.iter().any(|(_, n)| n.label().is_some_and(
            |s| s.contains("references")
                && s.contains("ON DELETE RESTRICT")
                && s.contains("ON UPDATE CASCADE")
        )));
        let pos = crate::app::test_ui::control(&output, "Open table child · a, b").center();
        render(crate::app::test_ui::pointer(pos, true));
        render(crate::app::test_ui::pointer(pos, false));
        assert_eq!(action.as_deref(), Some("child"));
    }
}
