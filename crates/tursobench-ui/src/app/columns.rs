use super::{
    App, View,
    document::Document,
    icons::{self, Icon},
};
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
pub(super) struct Header<'a> {
    pub column: &'a tursobench_core::Column,
    original_type: Option<&'a str>,
    primary_key: i64,
    indexed: bool,
    foreign: bool,
    descending: Option<bool>,
}
impl Document {
    pub fn column_header(&self, i: usize) -> Header<'_> {
        let column = &self.result.columns[i];
        let info = self.browse_inspection();
        Header {
            column,
            original_type: self
                .table
                .as_ref()
                .filter(|_| self.view == View::Browse)
                .and_then(|t| t.columns.get(i))
                .filter(|c| c.name == column.name)
                .map(|c| c.declared_type.as_str()),
            primary_key: info
                .and_then(|info| info.table.columns.get(i))
                .map_or(0, |c| c.primary_key),
            indexed: info.is_some_and(|info| {
                info.indexes
                    .iter()
                    .any(|idx| idx.columns.first().and_then(Option::as_ref) == Some(&column.name))
            }),
            foreign: info.is_some_and(|info| {
                info.foreign_keys
                    .iter()
                    .any(|fk| fk.columns.iter().any(|(name, _)| name == &column.name))
            }),
            descending: self
                .browse
                .as_ref()
                .filter(|b| b.sort == Some(i))
                .map(|b| b.descending),
        }
    }
}
impl Header<'_> {
    pub fn declared_type(&self) -> Option<&str> {
        self.original_type.or(self.column.declared_type.as_deref())
    }
    pub fn key_icon(&self) -> Option<Icon> {
        if self.primary_key > 0 {
            Some(Icon::Key)
        } else {
            self.foreign.then_some(Icon::Link)
        }
    }
    pub fn is_foreign(&self) -> bool {
        self.foreign
    }

    fn has_key(&self) -> bool {
        self.primary_key > 0 || self.foreign
    }
    pub fn describe(&self, response: &egui::Response, enabled: bool) {
        let tags = format!(
            "{}{}{}",
            if self.primary_key > 0 {
                format!(" PK{}", self.primary_key)
            } else {
                String::new()
            },
            if self.indexed { " IX" } else { "" },
            if self.foreign { " FK" } else { "" }
        );
        let sorted = match self.descending {
            None => "",
            Some(true) => " descending",
            Some(false) => " ascending",
        };
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                enabled,
                format!("Column {}{tags}{sorted}", self.column.name),
            )
        });
    }
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, highlighted: bool) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if highlighted {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        }
        let mut left = rect.left() + super::presentation::CELL_INSET;
        for (show, icon) in [
            (self.primary_key > 0, Icon::Key),
            (self.foreign, Icon::Link),
        ] {
            if show {
                icons::paint(
                    &painter,
                    egui::Rect::from_min_size(
                        egui::pos2(left, rect.top() + 7.0),
                        egui::vec2(12.0, 12.0),
                    ),
                    icon,
                    tokens.relationship,
                );
                left += 15.0;
            }
        }
        let reserve = if self.descending.is_some() {
            44.0
        } else {
            28.0
        };
        for (text, pos, font, color, reserve) in [
            (
                self.column.name.as_str(),
                egui::pos2(left, rect.top() + 14.0),
                crate::appearance::proportional(ui.ctx(), 12.0),
                ui.visuals().text_color(),
                reserve,
            ),
            (
                self.declared_type().unwrap_or("VALUE"),
                rect.min + egui::vec2(super::presentation::CELL_INSET, 31.0),
                crate::appearance::monospace(ui.ctx(), 9.5),
                tokens.faint,
                0.0,
            ),
        ] {
            let clip = egui::Rect::from_min_max(rect.min, rect.max - egui::vec2(reserve, 0.0));
            painter.with_clip_rect(clip.intersect(ui.clip_rect())).text(
                pos,
                egui::Align2::LEFT_CENTER,
                text,
                font,
                color,
            );
        }
        painter.vline(
            rect.right() - 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, tokens.line),
        );
        if self.descending.is_some() {
            painter.hline(
                rect.x_range(),
                rect.bottom() - 1.0,
                egui::Stroke::new(2.0, tokens.primary),
            );
            icons::paint(
                &painter,
                egui::Rect::from_min_size(
                    rect.right_top() + egui::vec2(-47.0, 8.0),
                    egui::vec2(12.0, 12.0),
                ),
                if self.descending == Some(true) {
                    Icon::Down
                } else {
                    Icon::Up
                },
                ui.visuals().selection.stroke.color,
            );
        }
    }
}
impl Columns {
    pub fn visible(&self, count: usize) -> impl Iterator<Item = usize> + '_ {
        self.order
            .iter()
            .copied()
            .filter(move |c| *c < count && !self.hidden.contains(c))
    }

    pub fn fit(&mut self, i: usize) {
        self.manual.remove(&i);
        self.refit = true;
    }
    pub fn header_menu(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.menu_button("Cell appearance", |ui| {
            let style = self.visuals.entry(i).or_default();
            for (value, label) in CellStyle::ALL {
                ui.selectable_value(style, value, label);
            }
        });
        if ui.button("Fit contents").clicked() {
            self.fit(i);
            ui.close();
        }
        for (label, direction) in [("Move left", -1), ("Move right", 1)] {
            if ui.button(label).clicked() {
                self.shift(i, direction);
                ui.close();
            }
        }
        if ui.button("Hide column").clicked() {
            self.hidden.insert(i);
            ui.close();
        }
    }

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
            .visible(self.document.result.columns.len())
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
            let key_columns: Vec<_> = (0..self.document.result.columns.len())
                .map(|i| self.document.column_header(i).has_key())
                .collect();
            let c = &mut self.document.tools.columns;
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
                        + if key_columns[i] { 15.0 } else { 0.0 };
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
    pub fn paint(self, ui: &egui::Ui, rect: egui::Rect, name: &str, value: &Value) -> bool {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let style = self.resolved(name);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let accent = tokens.primary;
        match (style, value) {
            (Self::Status, Value::Text(text)) if !text.is_empty() && text.len() <= 32 => {
                let color = match text.to_ascii_lowercase().as_str() {
                    "blocked" | "failed" | "error" => ui.visuals().error_fg_color,
                    "pending" | "in progress" | "review" => tokens.relationship,
                    _ => tokens.primary_text,
                };
                let galley = painter.layout_no_wrap(
                    text.clone(),
                    crate::appearance::proportional(ui.ctx(), 11.5),
                    color,
                );
                let pill = egui::Rect::from_min_size(
                    rect.left_center() + egui::vec2(5.0, -10.0),
                    egui::vec2(
                        (galley.size().x + 26.0).min((rect.width() - 10.0).max(0.0)),
                        20.0,
                    ),
                );
                painter.rect_filled(pill, 10.0, color.gamma_multiply(0.13));
                let painter = painter.with_clip_rect(pill.intersect(ui.clip_rect()));
                painter.circle_filled(pill.left_center() + egui::vec2(8.0, 0.0), 3.0, color);
                painter.galley(
                    pill.left_center() + egui::vec2(16.0, -galley.size().y / 2.0),
                    galley,
                    color,
                );
                return true;
            }
            (Self::Priority | Self::Percent, value) => {
                let number = match value {
                    Value::Integer(n) => *n as f64,
                    Value::Real(n) => *n,
                    _ => return false,
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
        false
    }
}
