use super::{App, View};
use eframe::egui;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct Columns {
    signature: Vec<(String, Option<String>)>,
    pub widths: Vec<f32>,
    pub order: Vec<usize>,
    pub hidden: BTreeSet<usize>,
    pub manual: BTreeSet<usize>,
    pub refit: bool,
    natural: Vec<f32>,
    measured_rows: usize,
}
impl Columns {
    pub fn move_before(&mut self, from: usize, target: usize) {
        if from == target || !self.order.contains(&from) || !self.order.contains(&target) {
            return;
        }
        self.order.retain(|&c| c != from);
        let pos = self.order.iter().position(|&c| c == target).unwrap();
        self.order.insert(pos, from);
    }
    pub fn shift(&mut self, column: usize, delta: isize) {
        if let Some(pos) = self.order.iter().position(|&c| c == column) {
            let to = pos.saturating_add_signed(delta).min(self.order.len() - 1);
            self.order.swap(pos, to);
        }
    }
}
impl App {
    pub(super) fn visible_columns(&self) -> Vec<usize> {
        self.document
            .tools
            .columns
            .order
            .iter()
            .copied()
            .filter(|c| {
                *c < self.document.result.columns.len()
                    && !self.document.tools.columns.hidden.contains(c)
            })
            .collect()
    }
    pub(super) fn prepare_columns(&mut self, ui: &egui::Ui) {
        if self.document.result.columns.is_empty() {
            return;
        }
        let scope = format!(
            "{}:{}",
            self.tools.database_scope,
            if self.document.view == View::Browse {
                self.document
                    .table
                    .as_ref()
                    .map_or("browse", |t| t.name.as_str())
            } else {
                "query"
            }
        );
        if self.document.tools.column_scope != scope {
            let old = std::mem::take(&mut self.document.tools.columns);
            self.document.tools.column_layouts.insert(
                std::mem::replace(&mut self.document.tools.column_scope, scope.clone()),
                old,
            );
            self.document.tools.columns = self
                .document
                .tools
                .column_layouts
                .remove(&scope)
                .unwrap_or_default();
            while self.document.tools.column_layouts.len() > 32 {
                self.document.tools.column_layouts.pop_first();
            }
        }
        let signature: Vec<_> = self
            .document
            .result
            .columns
            .iter()
            .map(|c| (c.name.clone(), c.declared_type.clone()))
            .collect();
        let c = &mut self.document.tools.columns;
        if c.signature != signature {
            *c = Columns {
                order: (0..signature.len()).collect(),
                signature,
                refit: true,
                ..Default::default()
            };
        }
        if let Some((_, name)) = &self.document.tools.highlight_column
            && self.document.tools.scroll_to_column
            && let Some(i) = self
                .document
                .result
                .columns
                .iter()
                .position(|c| &c.name == name)
        {
            c.hidden.remove(&i);
        }
        let sample = self.document.result.rows.len().min(200);
        if c.refit || sample > c.measured_rows {
            c.natural = self
                .document
                .result
                .columns
                .iter()
                .enumerate()
                .map(|(i, col)| {
                    let header = ui
                        .painter()
                        .layout_no_wrap(
                            col.name.clone(),
                            egui::FontId::proportional(12.0),
                            ui.visuals().text_color(),
                        )
                        .size()
                        .x
                        + 36.0
                        + if self.document.view == View::Browse
                            && self.document.tools.inspection.as_ref().is_some_and(|info| {
                                info.table.columns.get(i).is_some_and(|c| c.primary_key > 0)
                                    || info.foreign_keys.iter().any(|k| {
                                        k.columns.iter().any(|(name, _)| name == &col.name)
                                    })
                            })
                        {
                            15.0
                        } else {
                            0.0
                        };
                    self.document
                        .result
                        .rows
                        .iter()
                        .take(200)
                        .filter_map(|r| r.get(i))
                        .map(|v| {
                            ui.painter()
                                .layout_no_wrap(
                                    v.preview(),
                                    egui::FontId::monospace(12.0),
                                    ui.visuals().text_color(),
                                )
                                .size()
                                .x
                                + 18.0
                        })
                        .fold(header, f32::max)
                        .clamp(56.0, 300.0)
                })
                .collect();
            c.widths.resize(c.natural.len(), 56.0);
            for (i, w) in c.natural.iter().enumerate() {
                if !c.manual.contains(&i) {
                    c.widths[i] = *w;
                }
            }
            c.measured_rows = sample;
            c.refit = false;
        }
        let flex = self
            .document
            .result
            .columns
            .iter()
            .enumerate()
            .filter(|(i, col)| {
                !c.hidden.contains(i)
                    && !c.manual.contains(i)
                    && col
                        .declared_type
                        .as_deref()
                        .is_some_and(|t| t.to_uppercase().contains("TEXT"))
            })
            .max_by(|(a, _), (b, _)| c.natural[*a].total_cmp(&c.natural[*b]))
            .map(|(i, _)| i);
        if let Some(flex) = flex {
            let rest: f32 = c
                .order
                .iter()
                .filter(|&&i| i != flex && !c.hidden.contains(&i))
                .map(|&i| c.widths[i] + ui.spacing().item_spacing.x)
                .sum();
            c.widths[flex] = (ui.available_width() - 52.0 - rest).max(160.0);
        }
    }
}
