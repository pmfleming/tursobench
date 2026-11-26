use super::{
    App, View,
    document::{Document, ViewTools},
    icons::{self, Icon},
};
use eframe::egui;
use std::collections::{BTreeMap, BTreeSet};
use tursobench_core::{ResultSet, Value};

#[derive(Default)]
pub(super) struct Columns {
    signature: Vec<tursobench_core::Column>,
    pub widths: Vec<f32>,
    pub order: Vec<usize>,
    pub hidden: BTreeSet<usize>,
    pub manual: BTreeSet<usize>,
    pub refit: bool,
    pub visuals: BTreeMap<usize, CellStyle>,
    /// The lens chosen for a column's blobs; automatic when absent.
    pub lenses: BTreeMap<usize, super::value::Lens>,
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
    /// One-based sort position, present only while several columns are sorted.
    sort_level: Option<usize>,
    sortable: bool,
}
impl Document {
    pub fn column_header(&self, i: usize) -> Header<'_> {
        let column = &self.result.columns[i];
        let info = self.browse_inspection();
        // Same precedence as the engine request: the primary sort, then added levels.
        let mut order: Vec<(usize, bool)> = Vec::new();
        if let Some(browse) = &self.browse {
            order.extend(browse.sort.map(|column| (column, browse.descending)));
            for &(column, descending) in &browse.options.sorts {
                if !order.iter().any(|(sorted, _)| *sorted == column) {
                    order.push((column, descending));
                }
            }
        }
        let position = order.iter().position(|(column, _)| *column == i);
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
            descending: position.map(|p| order[p].1),
            sort_level: position.filter(|_| order.len() > 1).map(|p| p + 1),
            sortable: self.view == View::Browse && self.browse.is_some(),
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
    pub fn sortable(&self) -> bool {
        self.sortable
    }

    fn has_key(&self) -> bool {
        self.primary_key > 0 || self.foreign
    }
    pub fn describe(&self, response: &egui::Response, enabled: bool) {
        let key = (self.primary_key > 0).then(|| format!(" PK{}", self.primary_key));
        let index = if self.indexed { " IX" } else { "" };
        let foreign = if self.foreign { " FK" } else { "" };
        let direction = match self.descending {
            Some(true) => " descending",
            Some(false) => " ascending",
            None => "",
        };
        // A sort level is shown only while several columns are sorted.
        let level = self.sort_level.map(|level| format!(", sort level {level}"));
        let (key, level) = (key.unwrap_or_default(), level.unwrap_or_default());
        let name = &self.column.name;
        let label = format!("Column {name}{key}{index}{foreign}{direction}{level}");
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &label));
    }
    pub fn paint(&self, ui: &egui::Ui, rect: egui::Rect, highlighted: bool, hovered: bool) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        // A sortable header answers the pointer, so it reads as a control.
        let hint = hovered && self.sortable;
        if highlighted {
            painter.rect_filled(rect, 2.0, ui.visuals().selection.bg_fill);
        } else if hint {
            painter.rect_filled(rect, 2.0, tokens.raised_hover);
        }
        let mut left = rect.left() + super::presentation::CELL_INSET;
        for (show, icon) in [
            (self.primary_key > 0, Icon::Key),
            (self.foreign, Icon::Link),
        ] {
            if show {
                let at = egui::Rect::from_min_size(
                    egui::pos2(left, rect.top() + 7.0),
                    egui::vec2(12.0, 12.0),
                );
                icons::paint(&painter, at, icon, tokens.relationship);
                left += 15.0;
            }
        }
        let reserve = if self.descending.is_some() || hint {
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
        self.paint_sort(ui, &painter, rect, hint);
    }
    /// The sort direction and level of a sorted column, underlined; on a
    /// hovered unsorted one, a faint arrow where the next click's sort will show.
    fn paint_sort(&self, ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, hint: bool) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let arrow = egui::Rect::from_min_size(
            rect.right_top() + egui::vec2(-47.0, 8.0),
            egui::vec2(12.0, 12.0),
        );
        let Some(descending) = self.descending else {
            if hint {
                icons::paint(painter, arrow, Icon::Up, tokens.faint);
            }
            return;
        };
        let color = ui.visuals().selection.stroke.color;
        let underline = egui::Stroke::new(2.0, tokens.primary);
        painter.hline(rect.x_range(), rect.bottom() - 1.0, underline);
        let icon = if descending { Icon::Down } else { Icon::Up };
        icons::paint(painter, arrow, icon, color);
        if let Some(level) = self.sort_level {
            painter.text(
                rect.right_top() + egui::vec2(-31.0, 14.0),
                egui::Align2::CENTER_CENTER,
                level,
                crate::appearance::monospace(ui.ctx(), 9.0),
                color,
            );
        }
    }
}
impl Columns {
    pub fn visible(&self, count: usize) -> impl Iterator<Item = usize> + '_ {
        self.order
            .iter()
            .copied()
            .filter(move |&c| self.shown(c, count))
    }
    /// Whether `column`, of `count`, is shown rather than hidden.
    pub fn shown(&self, column: usize, count: usize) -> bool {
        column < count && !self.hidden.contains(&column)
    }
    /// The presentation chosen for a column; automatic unless the user picked one.
    pub fn style(&self, column: usize) -> CellStyle {
        self.visuals.get(&column).copied().unwrap_or_default()
    }
    /// The lens chosen for a column's blobs, if the user picked one.
    pub fn lens(&self, column: usize) -> Option<super::value::Lens> {
        self.lenses.get(&column).copied()
    }

    pub fn fit(&mut self, i: usize) {
        self.manual.remove(&i);
        self.refit = true;
    }
    /// The handle on a header's right edge: dragging resizes the column and a
    /// double-click fits it to its contents.
    pub fn resize_handle(&mut self, ui: &egui::Ui, header: egui::Rect, i: usize) -> egui::Response {
        let handle = egui::Rect::from_min_max(
            egui::pos2(header.right() - 6.0, header.top()),
            header.right_bottom(),
        );
        let id = ui.scope_id().with(("resize-column", i));
        let drag = ui
            .interact(handle, id, egui::Sense::click_and_drag())
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if drag.double_clicked() {
            self.fit(i);
        }
        if drag.dragged() {
            self.manual.insert(i);
            self.widths[i] = (self.widths[i] + drag.drag_delta().x).clamp(60.0, 640.0);
        }
        drag
    }
    pub fn header_menu(&mut self, ui: &mut egui::Ui, i: usize) {
        ui.menu_button("Cell appearance", |ui| {
            let style = self.visuals.entry(i).or_default();
            for (value, label) in CellStyle::ALL {
                ui.selectable_value(style, value, label);
            }
            // How the column's blobs read in the record tile.
            ui.separator();
            let chosen = self.lenses.get(&i).copied();
            if ui
                .selectable_label(chosen.is_none(), "Automatic lens")
                .clicked()
            {
                self.lenses.remove(&i);
            }
            for lens in super::value::Lens::ALL {
                if ui
                    .selectable_label(chosen == Some(lens), lens.name())
                    .clicked()
                {
                    self.lenses.insert(i, lens);
                }
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
    /// A fresh layout in schema order when the result's columns changed.
    fn reset_for(&mut self, columns: &[tursobench_core::Column]) {
        if self.signature != columns {
            *self = Columns {
                order: (0..columns.len()).collect(),
                signature: columns.to_vec(),
                refit: true,
                ..Default::default()
            };
        }
    }
    /// Natural widths from each header and up to 200 sampled values. Spare
    /// space belongs to the workspace, not to an arbitrary TEXT column, and
    /// widths the user set survive until the column is fitted again.
    fn measure(&mut self, ui: &egui::Ui, result: &ResultSet, keyed: &[bool]) {
        let font = crate::appearance::monospace(ui.ctx(), 12.0);
        let value_width = |v: &Value| {
            // A blob's summary follows its icon.
            let (text, icon) = match super::value::Summary::of(ui.ctx(), v) {
                Some(summary) => (summary.text(), 14.0),
                None => (v.preview(), 0.0),
            };
            let galley = ui
                .painter()
                .layout_no_wrap(text, font.clone(), ui.visuals().text_color());
            galley.size().x + 18.0 + icon
        };
        self.natural = result
            .columns
            .iter()
            .zip(keyed)
            .enumerate()
            .map(|(i, (col, &keyed))| {
                let header = super::presentation::text_width(ui, col.name.as_str(), 12.0)
                    + 36.0
                    + if keyed { 15.0 } else { 0.0 };
                result
                    .rows
                    .iter()
                    .take(200)
                    .filter_map(|r| r.get(i))
                    .map(value_width)
                    .fold(header, f32::max)
                    .clamp(56.0, 300.0)
            })
            .collect();
        self.widths.resize(self.natural.len(), 56.0);
        for (i, w) in self.natural.iter().enumerate() {
            if !self.manual.contains(&i) {
                self.widths[i] = *w;
            }
        }
        self.measured_rows = result.rows.len().min(200);
        self.refit = false;
    }
}
impl ViewTools {
    /// Keep the column layout under its scope and bring back the one last used for `scope`.
    fn switch_column_scope(&mut self, scope: String) {
        if self.column_scope == scope {
            return;
        }
        let previous = std::mem::replace(&mut self.column_scope, scope);
        let layout = std::mem::take(&mut self.columns);
        self.column_layouts.insert(previous, layout);
        self.columns = self
            .column_layouts
            .remove(&self.column_scope)
            .unwrap_or_default();
        while self.column_layouts.len() > 32 {
            self.column_layouts.pop_first();
        }
    }
}
impl App {
    pub(super) fn visible_columns(&self) -> Vec<usize> {
        let document = &self.database.document;
        let count = document.result.columns.len();
        document.tools.columns.visible(count).collect()
    }
    pub(super) fn prepare_columns(&mut self, ui: &egui::Ui) {
        let document = &mut self.database.document;
        if document.result.columns.is_empty() {
            return;
        }
        let table = match (document.view, &document.table) {
            (View::Browse, Some(table)) => table.name.as_str(),
            (View::Browse, None) => "browse",
            _ => "query",
        };
        let scope = format!("{}:{table}", self.database.tools.database_scope);
        document.tools.switch_column_scope(scope);
        document.tools.columns.reset_for(&document.result.columns);
        // A followed reference or search shows its column even when hidden.
        if let Some((_, name)) = &document.tools.highlight_column
            && document.tools.scroll_to_column
            && let Some(i) = document.result.columns.iter().position(|c| &c.name == name)
        {
            document.tools.columns.hidden.remove(&i);
        }
        let columns = &document.tools.columns;
        if columns.refit || document.result.rows.len().min(200) > columns.measured_rows {
            let keyed: Vec<_> = (0..document.result.columns.len())
                .map(|i| document.column_header(i).has_key())
                .collect();
            document.tools.columns.measure(ui, &document.result, &keyed);
        }
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
    /// Paint the value's presentation; returns whether it replaced the text.
    pub fn paint(self, ui: &egui::Ui, rect: egui::Rect, name: &str, value: &Value) -> bool {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let style = self.resolved(name);
        let number = match value {
            Value::Integer(n) => *n as f64,
            Value::Real(n) => *n,
            Value::Text(text) if style == Self::Status && !text.is_empty() && text.len() <= 32 => {
                status_pill(ui, &painter, rect, text);
                return true;
            }
            _ => return false,
        };
        let max = match style {
            Self::Priority => 5.0,
            Self::Percent => 100.0,
            _ => return false,
        };
        // A bar under the number shows its share of the scale; other numbers get none.
        if number.is_finite() && (0.0..=max).contains(&number) {
            let width = (rect.width() - 8.0).max(0.0) * (number / max) as f32;
            let at = rect.left_bottom() + egui::vec2(4.0, -4.0);
            let bar = egui::Rect::from_min_size(at, egui::vec2(width, 2.0));
            painter.rect_filled(bar, 1.0, crate::appearance::Tokens::of(ui.ctx()).primary);
        }
        false
    }
}
/// A status as a tinted pill, coloured for failures and for work under way.
fn status_pill(ui: &egui::Ui, painter: &egui::Painter, rect: egui::Rect, text: &str) {
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let color = match text.to_ascii_lowercase().as_str() {
        "blocked" | "failed" | "error" => ui.visuals().error_fg_color,
        "pending" | "in progress" | "review" => tokens.relationship,
        _ => tokens.primary_text,
    };
    let font = crate::appearance::proportional(ui.ctx(), 11.5);
    let galley = painter.layout_no_wrap(text.to_owned(), font, color);
    let width = (galley.size().x + 26.0).min((rect.width() - 10.0).max(0.0));
    let pill = egui::Rect::from_min_size(
        rect.left_center() + egui::vec2(5.0, -10.0),
        egui::vec2(width, 20.0),
    );
    painter.rect_filled(pill, 10.0, color.gamma_multiply(0.13));
    let painter = painter.with_clip_rect(pill.intersect(ui.clip_rect()));
    painter.circle_filled(pill.left_center() + egui::vec2(8.0, 0.0), 3.0, color);
    let at = pill.left_center() + egui::vec2(16.0, -galley.size().y / 2.0);
    painter.galley(at, galley, color);
}
