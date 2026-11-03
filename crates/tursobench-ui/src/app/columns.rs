use super::{App, View};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use tursobench_core::Value;

#[derive(Default)]
pub(super) struct Columns {
    signature: Vec<tursobench_core::Column>,
    pub widths: Vec<f32>,
    pub order: Vec<usize>,
    pub hidden: BTreeSet<usize>,
    pub manual: BTreeSet<usize>,
    pub refit: bool,
    pub visuals: BTreeMap<usize, CellStyle>,
    natural: Vec<f32>,
    measured_rows: usize,
}
impl Columns {
    pub fn move_before(&mut self, from: usize, target: usize) {
        let Some(from) = self.order.iter().position(|&c| c == from) else {
            return;
        };
        let Some(target) = self.order.iter().position(|&c| c == target) else {
            return;
        };
        let column = self.order.remove(from);
        self.order
            .insert(target - usize::from(from < target), column);
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
        let c = &mut self.document.tools.columns;
        if c.signature != self.document.result.columns {
            *c = Columns {
                order: (0..self.document.result.columns.len()).collect(),
                signature: self.document.result.columns.clone(),
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
                            crate::appearance::proportional(ui.ctx(), 12.0),
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
                                    crate::appearance::monospace(ui.ctx(), 12.0),
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
        // Spare space belongs to the workspace, not to an arbitrary TEXT column.
        // Explicit user widths survive tile resizes; Fit columns uses sampled content.
    }
}

/// Presentation never changes the stored value, filtering, sorting or clipboard data.
#[derive(Clone, Copy, Default, PartialEq)]
pub(super) enum CellStyle {
    #[default]
    Auto,
    Plain,
    Status,
    Priority,
    Percent,
}
impl CellStyle {
    pub const ALL: [(Self, &'static str); 5] = [
        (Self::Auto, "Automatic"),
        (Self::Plain, "Plain"),
        (Self::Status, "Status pill"),
        (Self::Priority, "Priority · 0–5"),
        (Self::Percent, "Progress · 0–100"),
    ];
    fn resolved(self, name: &str) -> Self {
        if self != Self::Auto {
            return self;
        }
        match name.to_ascii_lowercase().as_str() {
            "status" | "state" => Self::Status,
            "priority" => Self::Priority,
            "progress" | "percent" | "percentage" => Self::Percent,
            _ => Self::Plain,
        }
    }
    pub fn paint(self, ui: &egui::Ui, rect: egui::Rect, name: &str, value: &Value) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let style = self.resolved(name);
        let accent = ui.visuals().hyperlink_color;
        match (style, value) {
            (Self::Status, Value::Text(text)) if !text.is_empty() && text.len() <= 32 => {
                let color = match text.to_ascii_lowercase().as_str() {
                    "blocked" | "failed" | "error" => ui.visuals().error_fg_color,
                    "pending" | "in progress" | "review" => ui.visuals().warn_fg_color,
                    _ => accent,
                };
                let width = (text.chars().count() as f32 * 7.2 + 14.0).min(rect.width() - 8.0);
                painter.rect_filled(
                    egui::Rect::from_min_size(
                        rect.min + egui::vec2(3.0, 3.0),
                        egui::vec2(width.max(0.0), 18.0),
                    ),
                    7.0,
                    color.gamma_multiply(0.18),
                );
            }
            (Self::Priority | Self::Percent, value) => {
                let number = match value {
                    Value::Integer(n) => *n as f64,
                    Value::Real(n) => *n,
                    _ => return,
                };
                let max = if style == Self::Priority { 5.0 } else { 100.0 };
                if number.is_finite() && (0.0..=max).contains(&number) {
                    let bar = egui::Rect::from_min_size(
                        rect.left_bottom() + egui::vec2(4.0, -4.0),
                        egui::vec2((rect.width() - 8.0).max(0.0) * (number / max) as f32, 2.0),
                    );
                    painter.rect_filled(bar, 1.0, accent);
                }
            }
            _ => {}
        }
    }
}
