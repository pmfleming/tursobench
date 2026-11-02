use super::{App, View, results::CellKind};
use eframe::egui;
use tursobench_core::{Filter, FilterOp, Value};
use tursobench_db::Operation;

impl App {
    pub(super) fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let (Some(table), Some(mut next)) = (self.table.clone(), self.browse.clone()) else {
            return;
        };
        let mut changed = false;
        let mut remove = None;
        if !next.filters.is_empty() {
            ui.horizontal_wrapped(|ui| {
                for (index, filter) in next.filters.iter().enumerate() {
                    let name = table
                        .columns
                        .get(filter.column)
                        .map_or("?", |c| c.name.as_str());
                    let value = if filter.op.unary() {
                        String::new()
                    } else {
                        filter.value.preview()
                    };
                    if ui
                        .button(format!("{name} {} {value} ×", filter.op.label()))
                        .on_hover_text("Remove filter")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                }
                if !next.filters.is_empty() && ui.button("Clear filters").clicked() {
                    changed = true;
                    next.filters.clear();
                }
            });
        }
        if let Some(index) = remove.filter(|&i| i < next.filters.len()) {
            next.filters.remove(index);
            changed = true;
        }
        if self.ui.filter_open {
            ui.group(|ui| {
            ui.horizontal_wrapped(|ui| {
                self.filter_column = self.filter_column.min(table.columns.len().saturating_sub(1));
                egui::ComboBox::from_id_salt("filter-column").selected_text(table.columns.get(self.filter_column).map_or("Column", |c| c.name.as_str()))
                    .show_ui(ui, |ui| { for (i, c) in table.columns.iter().enumerate() { ui.selectable_value(&mut self.filter_column, i, &c.name); } });
                egui::ComboBox::from_id_salt("filter-operator").selected_text(self.filter_op.label())
                    .show_ui(ui, |ui| { for op in FilterOp::ALL { ui.selectable_value(&mut self.filter_op, op, op.label()); } });
                if !self.filter_op.unary() {
                    if self.filter_op == FilterOp::Contains { self.filter_kind = CellKind::Text; }
                    ui.add_enabled_ui(self.filter_op != FilterOp::Contains, |ui| {
                        egui::ComboBox::from_id_salt("filter-type").selected_text(self.filter_kind.label()).show_ui(ui, |ui| {
                            for kind in [CellKind::Text, CellKind::Integer, CellKind::Real, CellKind::Null, CellKind::Blob] { ui.selectable_value(&mut self.filter_kind, kind, kind.label()); }
                        });
                    });
                    if self.filter_kind != CellKind::Null { ui.add(egui::TextEdit::singleline(&mut self.filter).hint_text("Value").desired_width(160.0)); }
                }
                if ui.add_enabled(next.filters.len() < 32, egui::Button::new("Add filter")).clicked() {
                    let value = if self.filter_op.unary() { Ok(Value::Null) } else { self.filter_kind.parse(&self.filter) };
                    match value {
                        Ok(value) => { next.filters.push(Filter { column: self.filter_column, op: self.filter_op, value }); changed = true; }
                        Err(e) => self.status = e,
                    }
                }
            });
            if self.tools.inspection.as_ref().filter(|i| i.table.name == table.name).is_some_and(|info| {
                table.columns.get(self.filter_column).is_some_and(|c| c.primary_key != 1 && !info.indexes.iter().any(|index| index.columns.first().and_then(Option::as_ref) == Some(&c.name)))
            }) { ui.small("Possible table scan: no declared leading index for this column. Use EXPLAIN to check the actual plan."); }
            ui.small("All filters must match. Choose the stored value type explicitly. Contains treats % and _ literally; equality includes NULL.");
        });
        }
        if changed {
            next.offset = 0;
            if self
                .submit(Operation::Browse(next.clone()), View::Browse)
                .is_some()
            {
                self.browse = Some(next);
            }
        }
    }
}
