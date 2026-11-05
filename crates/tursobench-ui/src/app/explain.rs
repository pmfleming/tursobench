use super::{App, View};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use tursobench_core::{ResultSet, Value};

#[derive(Default)]
pub(super) struct State {
    pub sql: String,
    pub raw: bool,
    pub bytecode: bool,
}
#[derive(Debug)]
struct Node {
    id: i64,
    parent: i64,
    detail: String,
}
fn nodes(result: &ResultSet) -> Option<Vec<Node>> {
    let column = |name: &str| {
        result
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case(name))
    };
    let (id, parent, detail) = (column("id")?, column("parent")?, column("detail")?);
    let mut seen = BTreeSet::new();
    result
        .rows
        .iter()
        .map(|r| match (r.get(id)?, r.get(parent)?, r.get(detail)?) {
            (Value::Integer(id), Value::Integer(parent), Value::Text(detail))
                if seen.insert(*id) =>
            {
                Some(Node {
                    id: *id,
                    parent: *parent,
                    detail: detail.clone(),
                })
            }
            _ => None,
        })
        .collect()
}
fn draw(
    ui: &mut egui::Ui,
    node: &Node,
    all: &BTreeMap<i64, &Node>,
    seen: &mut BTreeSet<i64>,
    depth: usize,
) {
    if depth >= 64 || !seen.insert(node.id) {
        ui.weak("Unresolved/cyclic plan edge; inspect raw rows.");
        return;
    }
    let upper = node.detail.to_uppercase();
    let color = if upper.contains("SCAN ") || upper.contains("TEMP B-TREE") {
        ui.visuals().warn_fg_color
    } else {
        ui.visuals().text_color()
    };
    egui::CollapsingHeader::new(egui::RichText::new(&node.detail).color(color))
        .id_salt(("plan-node", node.id))
        .default_open(true)
        .show(ui, |ui| {
            ui.weak(if upper.contains("TEMP B-TREE") {
                "Temporary sorting/grouping work; compare ordering and available indexes."
            } else if upper.contains("SCAN ") {
                "Scan: can be appropriate for small tables or broad selections."
            } else if upper.contains("INDEX") || upper.contains("SEARCH") {
                "Indexed lookup; inspect the predicate and selectivity."
            } else {
                "Engine plan operation."
            });
            for child in all
                .values()
                .filter(|n| n.parent == node.id && n.id != node.id)
            {
                draw(ui, child, all, seen, depth + 1);
            }
        });
}
impl App {
    /// Returns true when the tree consumes the result pane; raw/bytecode uses the grid.
    pub(super) fn explain_panel(&mut self, ui: &mut egui::Ui) -> bool {
        if self.database.document.view != View::Explain {
            return false;
        }
        let mut rerun = None;
        ui.horizontal_wrapped(|ui| {
            ui.selectable_value(&mut self.database.document.explain.raw, false, "Plan tree");
            ui.selectable_value(&mut self.database.document.explain.raw, true, "Raw rows");
            if ui
                .button(if self.database.document.explain.bytecode {
                    "Load query plan"
                } else {
                    "Load bytecode"
                })
                .clicked()
            {
                rerun = Some(!self.database.document.explain.bytecode);
            }
        });
        if let Some(bytecode) = rerun {
            self.run_text(self.database.document.explain.sql.clone(), Some(bytecode));
            return true;
        }
        if self.database.document.explain.raw || self.database.document.explain.bytecode {
            return false;
        }
        let Some(nodes) = nodes(&self.database.document.result) else {
            ui.weak("Plan shape unavailable; showing raw engine rows.");
            return false;
        };
        if nodes.is_empty() {
            return false;
        }
        let all: BTreeMap<_, _> = nodes.iter().map(|n| (n.id, n)).collect();
        let mut seen = BTreeSet::new();
        let mut draft = None;
        egui::ScrollArea::both().id_salt("explain-tree").show(ui, |ui| {
            ui.small("Planner diagnostics, not execution measurements. Suggestions require review.");
            for node in &nodes {
                if node.id == node.parent || !all.contains_key(&node.parent) { draw(ui,node,&all,&mut seen,0); }
            }
            if seen.len() < nodes.len() { ui.weak("Some plan edges are cyclic or disconnected; use Raw rows for the complete retained plan."); }
            if nodes.iter().any(|n| n.detail.to_uppercase().contains("SCAN ")) && let Some(graph) = &self.database.ui.diagram.data {
                for table in &graph.tables {
                    if let Some(info) = &table.inspection && let Some(sql) = tursobench_db::advice::index_draft(&self.database.document.explain.sql,info) {
                        ui.separator(); ui.monospace(&sql);
                        ui.small("Existing ordinary indexes were checked. This does not establish that a new index will improve runtime.");
                        if ui.button("Open index draft in SQL editor").clicked() { draft = Some(sql); }
                    }
                }
            }
        });
        if let Some(sql) = draft {
            self.load_sql_window(sql);
        }
        true
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn plan_shapes_are_validated_instead_of_guessing() {
        let mut result = ResultSet::default();
        assert!(nodes(&result).is_none());
        result.columns = ["id", "parent", "notused", "detail"]
            .map(|s| tursobench_core::Column {
                name: s.into(),
                declared_type: None,
            })
            .into();
        result.rows = vec![vec![
            Value::Integer(2),
            Value::Integer(0),
            Value::Integer(0),
            Value::Text("SCAN users".into()),
        ]];
        assert_eq!(nodes(&result).unwrap()[0].id, 2);
        result.rows.push(result.rows[0].clone());
        assert!(nodes(&result).is_none());
    }
}
