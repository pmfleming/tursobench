pub(super) const ROW_HEIGHT: f32 = 29.0;
pub(super) const STATUS_HEIGHT: f32 = ROW_HEIGHT + 2.0;
use super::presentation::{BAR_INSET, BAR_SPACING, CELL_INSET, CONTENT_INSET, GUTTER_WIDTH};
pub(super) const HEADER_HEIGHT: f32 = 42.0;
use super::{
    App, View,
    icons::{self, Icon},
    shell,
};
use crate::appearance::Tokens;
use eframe::egui;
use tursobench_core::{FilterOp, ForeignKey, Inspection, Table, Value};

enum GridAction {
    Cell(usize, usize),
    Edit(usize, usize),
    Record(usize),
    CopyCell(usize, usize),
    CopyRow(usize),
    Delete(usize),
    BulkDelete,
    Spreadsheet(super::spreadsheet::Action),
    /// Follow the cell's `n`th matching foreign key: row, column, `n`.
    Follow(usize, usize, usize),
    /// Filter the browse by the cell's stored value: row, column, comparison.
    Filter(usize, usize, FilterOp),
    Inspect(usize, usize),
    Stage(usize, usize, Value),
}
/// A context-menu entry: shown, enabled, label, shortcut and what it asks for.
type MenuEntry = (bool, bool, &'static str, &'static str, GridAction);
struct CellAppearance<'a> {
    value: &'a Value,
    column: &'a str,
    null_display: &'a str,
    style: super::columns::CellStyle,
    selected: bool,
    focused: bool,
    deleted: bool,
    staged: bool,
    related: bool,
    row: usize,
    /// What a blob or deferred value is; never decoded here.
    summary: Option<std::sync::Arc<super::value::Summary>>,
}
impl CellAppearance<'_> {
    fn text(&self) -> String {
        if let Some(summary) = &self.summary {
            return summary.text();
        }
        match self.value {
            Value::Null => self.null_display.to_owned(),
            Value::Text(s) if s.is_empty() => "(empty text)".into(),
            Value::Real(n) => format!("{n:?}"),
            _ => self.value.preview(),
        }
    }
    fn describe(&self, ui: &egui::Ui, response: &egui::Response, number: usize) {
        let state = if self.deleted {
            "Deletion staged · "
        } else if self.staged {
            "Staged · "
        } else {
            ""
        };
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::Cell,
                ui.is_enabled(),
                self.selected,
                format!("Row {number} · {}: {state}{}", self.column, self.text()),
            )
        });
    }
    fn text_color(&self, ui: &egui::Ui, tokens: Tokens) -> egui::Color32 {
        if self.deleted {
            ui.visuals().error_fg_color
        } else if self.related {
            tokens.relationship
        } else if matches!(self.value, Value::Null) {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        }
    }
    fn paint_text(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        color: egui::Color32,
    ) {
        let mut job = egui::text::LayoutJob::simple_singleline(
            self.text(),
            crate::appearance::monospace(ui.ctx(), 12.0),
            color,
        );
        job.wrap.max_width = (rect.width() - 14.0).max(1.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let position = if matches!(self.value, Value::Integer(_) | Value::Real(_)) {
            rect.right_center() - egui::vec2(CELL_INSET + galley.size().x, galley.size().y * 0.5)
        } else {
            rect.left_center() + egui::vec2(CELL_INSET, -galley.size().y * 0.5)
        };
        painter.galley(position, galley, color);
    }
    fn paint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let fill = if self.selected {
            ui.visuals().selection.bg_fill
        } else {
            cell_fill(ui, self.deleted, self.staged, self.row)
        };
        painter.rect_filled(rect, 0.0, fill);
        if self.focused {
            let stroke = ui.visuals().selection.stroke;
            painter.rect_stroke(rect, 0.0, stroke, egui::StrokeKind::Inside);
        }
        let styled = self.style.paint(ui, rect, self.column, self.value);
        let tokens = Tokens::of(ui.ctx());
        let color = self.text_color(ui, tokens);
        if let Some(summary) = &self.summary {
            let inner = rect.shrink2(egui::vec2(CELL_INSET - 2.0, 0.0));
            summary.paint(ui, inner, color, true);
        } else if !styled {
            self.paint_text(ui, &painter, rect, color);
        }
        let line = egui::Stroke::new(1.0, tokens.line_soft);
        painter.hline(rect.x_range(), rect.bottom() - 0.5, line);
        painter.vline(rect.right() - 0.5, rect.y_range(), line);
        self.paint_marks(ui, &painter, rect, color, tokens);
    }
    /// Dots under a reference, a strike through a staged deletion, and a bar at
    /// the left of a staged cell.
    fn paint_marks(
        &self,
        ui: &egui::Ui,
        painter: &egui::Painter,
        rect: egui::Rect,
        color: egui::Color32,
        tokens: Tokens,
    ) {
        if self.related {
            let end = rect.right() - CELL_INSET;
            let mut x = rect.left() + CELL_INSET;
            while x < end {
                painter.circle_filled(egui::pos2(x, rect.bottom() - 4.0), 0.65, color);
                x += 3.0;
            }
        }
        let error = ui.visuals().error_fg_color;
        if self.deleted {
            let inset = egui::vec2(5.0, 0.0);
            let strike = [rect.left_center() + inset, rect.right_center() - inset];
            painter.line_segment(strike, egui::Stroke::new(1.0, error));
        }
        let bar = if self.deleted {
            Some(error)
        } else {
            self.staged.then_some(tokens.pending)
        };
        if let Some(bar) = bar {
            painter.vline(
                rect.left() + 1.5,
                rect.y_range(),
                egui::Stroke::new(3.0, bar),
            );
        }
    }
}
impl App {
    fn script_results_ui(&mut self, ui: &mut egui::Ui) {
        let document = &self.database.document;
        if document.script_results.is_empty() {
            return;
        }
        let mut selected = None;
        ui.collapsing("Script results and outcomes", |ui| {
            ui.label(&document.script_message);
            for (i, result) in document.script_results.iter().enumerate() {
                let shown = document.selected_script == Some(i);
                let retained = shown.then_some(document.result.rows.len());
                if ui.button(statement_label(result, retained)).clicked() {
                    selected = Some(i);
                }
            }
        });
        if let Some(i) = selected {
            self.database.document.select_script_result(i);
        }
    }
    pub(super) fn grid_contents(&mut self, ui: &mut egui::Ui) {
        self.profile_header(ui);
        if self.explain_panel(ui) {
            return;
        }
        self.update_draft_presentation();
        self.grid_status_bar(ui);
        self.script_results_ui(ui);
        if self.database.document.stale {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                "Data changed since this result was loaded. Refresh to reload.",
            );
        }
        #[cfg(test)]
        {
            self.fk_cell_rect = None;
        }
        ui.spacing_mut().item_spacing = egui::Vec2::ZERO;
        self.grid_keyboard_focus(ui);
        self.database.document.tools.context.hovered = None;
        if !ui.input(|i| i.pointer.primary_down()) {
            self.database.document.tools.dragging_selection = false;
        }
        let rows_height = self.rows_height(ui);
        #[cfg(test)]
        {
            self.database.ui.grid_rect = ui.available_rect_before_wrap();
        }
        self.prepare_columns(ui);
        self.explain_scan_note(ui);
        let (drawn, action) = self.grid_table(ui, rows_height);
        if let Some(action) = action {
            self.grid_action(ui, action);
        }
        if let Some(cell) = self.inspected_deferred().cloned()
            && ui.button("View large value…").clicked()
        {
            self.open_large_value(cell);
        }
        self.hover_context(ui.ctx());
        let ui_state = &self.database.document.ui;
        if ui_state.nav == shell::Nav::Table && ui_state.table_view == shell::TableView::Data {
            self.paging_footer(ui, drawn);
        }
        let document = &self.database.document;
        if !document.tools.inspection_error.is_empty() && document.result.columns.is_empty() {
            ui.colored_label(ui.visuals().warn_fg_color, &document.tools.inspection_error);
        }
        if document.result.rows.is_empty() && document.active.is_none() {
            icons::empty_state(ui, Icon::Data, "No rows to display");
        }
    }
    /// The height left for rows below the header and any large-value, loading
    /// or page-error rows under them.
    fn rows_height(&mut self, ui: &egui::Ui) -> f32 {
        let pages = &self.database.document.tools.pages;
        let auxiliary_rows = usize::from(self.inspected_deferred().is_some())
            + usize::from(pages.pending.is_some())
            + 2 * usize::from(!pages.error.is_empty());
        let remaining =
            (ui.available_height() - HEADER_HEIGHT - auxiliary_rows as f32 * ROW_HEIGHT).max(0.0);
        let tools = &mut self.database.document.tools;
        tools.row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
        tools.grid_height = remaining;
        remaining
    }
    /// The column headers and the rows in view, which scroll together
    /// horizontally; returns the rows drawn and what one of them asked for.
    fn grid_table(
        &mut self,
        ui: &mut egui::Ui,
        rows_height: f32,
    ) -> (std::ops::Range<usize>, Option<GridAction>) {
        let editable = self.can_edit();
        let headers = !self.database.document.result.columns.is_empty();
        let tools = &mut self.database.document.tools;
        let scroll = tools.restore_scroll.take().unwrap_or(tools.grid_scroll);
        if headers {
            header_band(ui);
        }
        let mut drawn = 0..0;
        let mut action = None;
        let horizontal = egui::ScrollArea::horizontal()
            .horizontal_scroll_offset(scroll.x)
            .id_salt("result-columns")
            .show(ui, |ui| {
                if headers {
                    self.grid_header(ui);
                }
                let rows = self.database.document.result.rows.len();
                let vertical = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(scroll.y)
                    .id_salt("result-rows")
                    .max_height(rows_height)
                    .show_rows(ui, ROW_HEIGHT, rows, |ui, range| {
                        drawn = range.clone();
                        #[cfg(test)]
                        {
                            let stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
                            self.database.ui.visible_rows = range
                                .clone()
                                .filter(|r| (*r + 1) as f32 * stride <= rows_height)
                                .count();
                        }
                        for row in range {
                            if let Some(next) = self.grid_row(ui, row, editable) {
                                action = Some(next);
                            }
                        }
                    });
                self.database.document.tools.grid_scroll.y = vertical.state.clamped_offset().y;
            });
        self.database.document.tools.grid_scroll.x = horizontal.state.clamped_offset().x;
        (drawn, action)
    }
    /// The grid takes keyboard focus as a whole, and keeps the arrow keys while it has it.
    fn grid_keyboard_focus(&mut self, ui: &mut egui::Ui) {
        let grid_id = ui.scope_id().with("grid-keyboard");
        self.database.document.tools.grid_focus = Some(grid_id);
        let grid_response = ui.interact(
            ui.available_rect_before_wrap(),
            grid_id,
            egui::Sense::click(),
        );
        grid_response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Grid,
                ui.is_enabled(),
                "Result grid · arrows move, Shift selects, Enter edits or follows",
            )
        });
        if grid_response.has_focus() {
            let arrows = egui::EventFilter {
                horizontal_arrows: true,
                vertical_arrows: true,
                ..Default::default()
            };
            ui.ctx()
                .memory_mut(|m| m.set_focus_lock_filter(grid_id, arrows));
        }
    }
    /// The inspected cell's value when it was too large to load with the page.
    fn inspected_deferred(&self) -> Option<&tursobench_core::DeferredCell> {
        let document = &self.database.document;
        let (r, c) = document.inspect?;
        match document.result.rows.get(r)?.get(c)? {
            Value::Deferred(cell) => Some(cell),
            _ => None,
        }
    }
    fn explain_scan_note(&self, ui: &mut egui::Ui) {
        let result = &self.database.document.result;
        if self.database.document.view != View::Explain {
            return;
        }
        let Some(detail) = result
            .columns
            .iter()
            .position(|c| c.name.eq_ignore_ascii_case("detail"))
        else {
            return;
        };
        let scans = result
            .rows
            .iter()
            .filter(|r| matches!(r.get(detail), Some(Value::Text(s)) if s.to_uppercase().contains("SCAN ")))
            .count();
        ui.label(format!(
            "Planner notes: {scans} scan entries. Scans may be intentional; not a runtime profile."
        ));
    }
    fn grid_action(&mut self, ui: &egui::Ui, action: GridAction) {
        match action {
            GridAction::Cell(r, c) => self.select_cell(
                r,
                c,
                ui.input(|i| i.modifiers.shift) || self.database.document.tools.dragging_selection,
            ),
            GridAction::Edit(r, c) => self.start_inline(r, c),
            GridAction::Inspect(r, c) => {
                self.open_record_window(r, c);
            }
            GridAction::Stage(r, c, v) => {
                self.stage_cell(r, c, v);
                self.database.document.tools.inline = None;
                if let Some(id) = self.database.document.tools.grid_focus {
                    ui.ctx().memory_mut(|m| m.request_focus(id));
                }
            }
            GridAction::Record(r) => self.open_record_window(r, 0),
            GridAction::CopyCell(r, c) => self.copy_cell(ui.ctx(), r, c),
            GridAction::CopyRow(r) => self.copy_row(ui.ctx(), r),
            GridAction::Delete(r) => self.stage_delete(r),
            GridAction::BulkDelete => self.stage_selected_deletions(),
            GridAction::Spreadsheet(action) => self.spreadsheet_action(action),
            GridAction::Follow(r, c, n) => {
                if let Some(key) = self.cell_references(r, c).get(n).map(|&key| key.clone()) {
                    self.follow_key(r, key);
                }
            }
            GridAction::Filter(r, c, op) => {
                let rows = &self.database.document.result.rows;
                if let Some(value) = rows.get(r).and_then(|values| values.get(c)).cloned() {
                    self.add_filter(tursobench_core::Filter {
                        column: c,
                        op,
                        value,
                    });
                }
            }
        }
    }
    /// Table data loads further pages as it scrolls; show that and any failure.
    fn paging_footer(&mut self, ui: &mut egui::Ui, visible: std::ops::Range<usize>) {
        self.maybe_page(
            visible.start,
            visible.end,
            ui.input(|i| i.smooth_scroll_delta.y),
        );
        let pages = &self.database.document.tools.pages;
        if pages.pending.is_some() {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.small("Loading 100 rows…");
            });
        }
        if !pages.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &pages.error);
            if ui.button("Retry page").clicked() {
                self.request_page(pages.retry_previous);
            }
        }
    }
    fn grid_status_bar(&mut self, ui: &mut egui::Ui) {
        let tokens = Tokens::of(ui.ctx());
        let bar = egui::Panel::bottom("grid-status-bar")
            .resizable(false)
            .exact_size(STATUS_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(tokens.raised)
                    .inner_margin(egui::Margin::symmetric(BAR_INSET as i8, 1))
                    .corner_radius(egui::CornerRadius {
                        sw: 6,
                        se: 6,
                        ..Default::default()
                    }),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(BAR_SPACING, 0.0);
                ui.spacing_mut().interact_size.y = ROW_HEIGHT;
                ui.spacing_mut().button_padding = egui::vec2(5.0, 1.0);
                ui.style_mut().override_font_id =
                    Some(crate::appearance::proportional(ui.ctx(), 12.0));
                self.grid_status_contents(ui);
            });
        ui.painter().hline(
            bar.response.rect.x_range(),
            bar.response.rect.top(),
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        bar.response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Status,
                true,
                if self.database.document.browse.is_some() {
                    "Table data status"
                } else {
                    "Query result status"
                },
            )
        });
    }
    /// The retained row range at the left and, for table data, counting and
    /// going to a row at the right.
    fn grid_status_contents(&mut self, ui: &mut egui::Ui) {
        let browsing = self.database.document.browse.is_some();
        let count_label = self.count_label();
        let actions_width = if browsing {
            let text_width = |text: &str| super::presentation::text_width(ui, text, 12.0);
            let jump = format!("Row {}", self.database.document.tools.jump_row);
            text_width(&jump) + 108.0 + count_label.as_deref().map_or(0.0, |s| text_width(s) + 6.0)
        } else {
            0.0
        };
        let leading = CONTENT_INSET - BAR_INSET;
        let width = ui.available_width().max(leading + actions_width + 120.0);
        let (mut count, mut jump) = (false, false);
        egui::ScrollArea::horizontal()
            .id_salt("grid-status-actions")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add_space(leading);
                    ui.allocate_ui_with_layout(
                        egui::vec2(width - leading - actions_width - BAR_SPACING, ROW_HEIGHT),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| self.retained_rows_label(ui),
                    );
                    if browsing {
                        let layout = egui::Layout::right_to_left(egui::Align::Center);
                        (count, jump) = ui
                            .with_layout(layout, |ui| {
                                self.browse_status_actions(ui, count_label.as_deref())
                            })
                            .inner;
                    }
                });
            });
        if count && let Some(browse) = &self.database.document.browse {
            self.count_table(browse.table.clone());
        }
        if jump {
            self.jump_to_row();
        }
    }
    /// The browsed table's filtered or total row count, once counted.
    fn count_label(&self) -> Option<String> {
        let table = &self.database.document.browse.as_ref()?.table;
        let filtered = self.database.document.tools.filtered_count;
        filtered.map(|n| format!("{n} matching")).or_else(|| {
            let total = self.database.tools.counts.get(table);
            total.map(|n| format!("{n} total"))
        })
    }
    fn retained_rows_label(&self, ui: &mut egui::Ui) {
        let browse = self.database.document.browse.as_ref();
        let start = browse.map_or(0, |b| b.offset);
        let count = self.database.document.result.rows.len();
        let text = egui::RichText::new(format!(
            "{}–{} · {count} retained",
            start + usize::from(count > 0),
            start + count
        ))
        .monospace()
        .size(crate::appearance::text_size(ui.ctx(), 11.0))
        .color(ui.visuals().weak_text_color());
        ui.add(egui::Label::new(text).truncate())
            .on_hover_text(if browse.is_some() {
                "Rows held in memory for this table. Scroll or PgDn loads the next 100."
            } else {
                "Rows held in memory for this result. Exports and copies use these rows without rerunning SQL."
            });
    }
    /// Count the table's rows, and go to a typed row number; returns which was asked for.
    fn browse_status_actions(
        &mut self,
        ui: &mut egui::Ui,
        count_label: Option<&str>,
    ) -> (bool, bool) {
        let counting = self.database.ready
            && !self.database.closing
            && self.database.tools.count_request.is_none();
        let count = ui
            .add_enabled_ui(counting, |ui| {
                icons::button(ui, Icon::Count, "Count matching rows", "")
            })
            .inner
            .clicked();
        if let Some(label) = count_label {
            ui.weak(label);
        }
        ui.separator();
        let enabled = self.paging_idle();
        let mut jump = ui
            .add_enabled_ui(enabled, |ui| icons::button(ui, Icon::Next, "Go to row", ""))
            .inner
            .clicked();
        let input = ui.add_enabled(
            enabled,
            egui::DragValue::new(&mut self.database.document.tools.jump_row)
                .range(1..=i64::MAX as usize)
                .prefix("Row "),
        );
        input.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::SpinButton, enabled, "Row number")
        });
        // Typing a number and pressing Enter is the same as the arrow.
        jump |= input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        input.on_hover_text(
            "Go to row: click to type a row number, then press Enter · or drag to change it",
        );
        (count, jump)
    }

    fn grid_header(&mut self, ui: &mut egui::Ui) {
        let mut sort = None;
        ui.horizontal(|ui| {
            ui.add_sized([GUTTER_WIDTH, HEADER_HEIGHT], egui::Label::new("#"));
            for i in self.visible_columns() {
                if self.grid_column_header(ui, i) {
                    sort = Some(i);
                }
            }
        });
        if let Some(i) = sort {
            self.sort_column(i, ui.input(|i| i.modifiers.shift));
        }
    }
    /// A header click sorts by the column, again to reverse, once more to
    /// clear; `add_level` adds or reverses a further sort level instead.
    fn sort_column(&mut self, column: usize, add_level: bool) {
        if self.database.document.view != View::Browse || !self.requests_enabled() {
            return;
        }
        let Some(b) = self.database.document.browse.as_mut() else {
            return;
        };
        if add_level {
            let sorts = &mut b.options.sorts;
            if let Some((_, descending)) = sorts.iter_mut().find(|(col, _)| *col == column) {
                *descending = !*descending;
            } else if sorts.len() < 7 {
                sorts.push((column, false));
            }
        } else {
            (b.sort, b.descending) = match (b.sort == Some(column), b.descending) {
                (false, _) => {
                    b.options.sorts.clear();
                    (Some(column), false)
                }
                (true, false) => (Some(column), true),
                (true, true) => (None, false),
            };
        }
        b.offset = 0;
        b.options.cursor = None;
        b.options.reverse = false;
        self.reload();
    }
    /// Whether the column is the one a followed reference or a search asked to show.
    fn column_highlighted(&self, name: &str) -> bool {
        let document = &self.database.document;
        let shown = document.table.as_ref();
        let highlight = document.tools.highlight_column.as_ref();
        document.view == View::Browse
            && highlight.is_some_and(|(table, column)| {
                column == name && shown.is_some_and(|t| &t.name == table)
            })
    }
    fn grid_column_header(&mut self, ui: &mut egui::Ui, i: usize) -> bool {
        let header = self.database.document.column_header(i);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(
                self.database.document.tools.columns.widths[i],
                HEADER_HEIGHT,
            ),
            egui::Sense::click_and_drag(),
        );
        header.describe(&response, ui.is_enabled());
        let highlighted = self.column_highlighted(&header.column.name);
        if highlighted && self.database.document.tools.scroll_to_column {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
        }
        header.paint(ui, rect, highlighted, response.hovered());
        // Lead with what the header does; the key legend follows.
        let response = response.on_hover_text(format!(
            "{} · {}\n{}Drag to move · drag the right edge to resize, double-click it to fit · right-click for appearance and hiding\nPK = key position; IX = declared leading index; FK = foreign key. Index method/partial predicate may not apply.",
            header.column.name,
            header.declared_type().unwrap_or("runtime type"),
            if header.sortable() {
                "Click to sort, again to reverse, once more to clear · Shift+click adds a sort level\n"
            } else {
                ""
            }
        ));
        let filter_label = format!("Filter {}", header.column.name);
        let tools = &mut self.database.document.tools;
        if highlighted {
            tools.scroll_to_column = false;
        }
        response.context_menu(|ui| tools.columns.header_menu(ui, i));
        if response.drag_started() {
            response.dnd_set_drag_payload(i);
        }
        if let Some(from) = response.dnd_release_payload::<usize>() {
            tools.columns.move_before(*from, i);
        }
        let funnel = self.column_filter(ui, rect, i, &filter_label);
        let columns = &mut self.database.document.tools.columns;
        let resize = columns.resize_handle(ui, rect, i);
        response.clicked() && !resize.contains_pointer() && !funnel.contains_pointer()
    }
    /// The funnel at a header's right that filters by the column's sampled values.
    fn column_filter(
        &mut self,
        ui: &mut egui::Ui,
        header: egui::Rect,
        i: usize,
        label: &str,
    ) -> egui::Response {
        let tokens = Tokens::of(ui.ctx());
        let rect = egui::Rect::from_min_size(
            header.right_top() + egui::vec2(-24.0, 5.0),
            egui::vec2(17.0, 20.0),
        );
        let funnel = ui.interact(
            rect,
            ui.scope_id().with(("column-filter", i)),
            egui::Sense::click(),
        );
        let browse = self.database.document.browse.as_ref();
        let filtered = browse.is_some_and(|b| b.filters.iter().any(|f| f.column == i));
        if funnel.hovered() {
            ui.painter().rect_filled(rect, 4.0, tokens.raised_hover);
        }
        let color = if filtered {
            tokens.primary
        } else if funnel.hovered() {
            ui.visuals().text_color()
        } else {
            tokens.faint
        };
        icons::paint(ui.painter(), rect.shrink(2.0), Icon::Filter, color);
        funnel
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
        let funnel = funnel.on_hover_text(if filtered {
            format!("{label} · a filter is active on this column")
        } else {
            format!("{label} · choose from sampled values")
        });
        egui::Popup::menu(&funnel).show(|ui| self.column_facets(ui, i));
        funnel
    }
    fn grid_row(&mut self, ui: &mut egui::Ui, row: usize, editable: bool) -> Option<GridAction> {
        ui.horizontal(|ui| {
            let document = &self.database.document;
            let overlay = document.tools.draft_ui.rows.get(&row);
            let deleted = overlay.is_some_and(|r| r.deleted);
            let modified = overlay.is_some_and(|r| r.modified());
            let inspected = document.inspect.is_some_and(|(r, _)| r == row);
            let offset = document.browse.as_ref().map_or(0, |b| b.offset);
            row_gutter(ui, offset + row + 1, deleted, modified, inspected);
            let mut action = None;
            for col in self.visible_columns() {
                action = self
                    .grid_cell(ui, row, col, deleted, offset, editable)
                    .or(action);
            }
            action
        })
        .inner
    }
    fn grid_cell(
        &mut self,
        ui: &mut egui::Ui,
        row: usize,
        col: usize,
        deleted: bool,
        offset: usize,
        editable: bool,
    ) -> Option<GridAction> {
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(self.database.document.tools.columns.widths[col], ROW_HEIGHT),
            egui::Sense::click_and_drag(),
        );
        let inline = self.editing_at(row, col) && !self.record_tile_active();
        let dragged = if inline {
            None
        } else {
            self.drag_select(ui, &response, row, col)
        };
        // Field paths keep these borrows apart from the tool state set below.
        let references = matching_keys(
            self.database.document.view,
            self.database.document.table.as_ref(),
            self.database.document.tools.inspection.as_ref(),
            Some(&self.database.document.result.rows[row]),
            col,
        );
        let staged = self.database.document.tools.draft_ui.value(
            self.database.document.drafts.as_ref(),
            row,
            col,
        );
        let value = staged.unwrap_or(&self.database.document.result.rows[row][col]);
        let appearance = CellAppearance {
            value,
            summary: super::value::Summary::of(ui.ctx(), value),
            column: &self.database.document.result.columns[col].name,
            null_display: &self.preferences.null_display,
            style: self.database.document.tools.columns.style(col),
            selected: self.cell_selected(row, col),
            focused: self.database.document.inspect == Some((row, col)) || response.has_focus(),
            deleted,
            staged: staged.is_some(),
            related: !references.is_empty(),
            row,
        };
        appearance.describe(ui, &response, offset + row + 1);
        if inline {
            return self.inline_cell(ui, rect, row, col);
        }
        if self.database.document.tools.scroll_to_cell
            && self.database.document.inspect == Some((row, col))
        {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
            self.database.document.tools.scroll_to_cell = false;
        }
        appearance.paint(ui, rect);
        #[cfg(test)]
        if !references.is_empty() {
            self.fk_cell_rect = Some(rect);
        }
        #[cfg(test)]
        if row == 0 && col == 0 {
            self.first_cell_rect = Some(rect);
        }
        let click = CellClick {
            row,
            col,
            references: &references,
            filterable: self.database.document.browse.is_some(),
            selected: appearance.selected,
        };
        let action = click.action(ui, &response).or(dragged);
        let summary = appearance.summary.clone();
        if response.hovered() && references.len() == 1 {
            self.database.document.tools.context.hovered = Some((row, col));
        }
        let response = match summary {
            Some(summary) if references.is_empty() => {
                response.on_hover_text(summary.description.as_str())
            }
            _ => self.reference_hover(response, row, &references),
        };
        let mut menu = None;
        response.context_menu(|ui| menu = self.cell_menu(ui, row, col, editable, &references));
        menu.or(action)
    }
    /// A primary drag starts a selection at the cell and extends it over the
    /// cells it crosses; pressing the cell gives the grid keyboard focus.
    fn drag_select(
        &mut self,
        ui: &egui::Ui,
        response: &egui::Response,
        row: usize,
        col: usize,
    ) -> Option<GridAction> {
        let tools = &mut self.database.document.tools;
        let started = response.drag_started_by(egui::PointerButton::Primary);
        if started {
            tools.grid_selection = Some(super::selection::Selection {
                ticket: self.database.document.result.current,
                anchor: (row, col),
                cursor: (row, col),
            });
            tools.dragging_selection = true;
        }
        if (started || response.clicked())
            && let Some(id) = tools.grid_focus
        {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        let extending = response.hovered()
            && tools.dragging_selection
            && ui.input(|i| i.pointer.primary_down());
        if extending && let Some(s) = &mut tools.grid_selection {
            s.cursor = (row, col);
        }
        extending.then_some(GridAction::Cell(row, col))
    }
    /// A cell with references points like a link and previews its parent on hover.
    fn reference_hover(
        &self,
        response: egui::Response,
        row: usize,
        references: &[&ForeignKey],
    ) -> egui::Response {
        if references.is_empty() {
            return response;
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_ui(|ui| match references {
                [key] => self.peek_view(ui, row, key),
                _ => {
                    ui.label("Multiple references: right-click to choose");
                }
            })
    }
    /// The cell being edited in place: its editor, staging the value on submit.
    fn inline_cell(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        row: usize,
        col: usize,
    ) -> Option<GridAction> {
        let edit = self.database.document.tools.inline.as_mut()?;
        let response = ui.put(rect, egui::TextEdit::singleline(&mut edit.editor.input));
        response.context_menu(|ui| edit.editor.type_picker(ui, "inline-type", false));
        match edit.submitted(ui, &response)? {
            Ok(value) => return Some(GridAction::Stage(row, col, value)),
            Err(error) => self.database.status = error,
        }
        None
    }
    fn cell_menu(
        &self,
        ui: &mut egui::Ui,
        row: usize,
        col: usize,
        editable: bool,
        references: &[&ForeignKey],
    ) -> Option<GridAction> {
        let browse = self.database.document.view == View::Browse;
        if browse {
            let selected = self.selected_cells().next().is_some();
            let undo = self.can_undo_grid_edit();
            if let Some(action) = super::spreadsheet::menu(ui, editable, selected, undo) {
                return Some(GridAction::Spreadsheet(action));
            }
            ui.separator();
        }
        use FilterOp::{Equal, NotEqual};
        use GridAction::{BulkDelete, CopyCell, CopyRow, Delete, Filter, Inspect, Record};
        let filterable = browse && self.database.document.browse.is_some();
        let leading = menu_entries(
            ui,
            [
                (
                    filterable,
                    true,
                    "Filter = value",
                    "Alt click",
                    Filter(row, col, Equal),
                ),
                (
                    filterable,
                    true,
                    "Exclude value",
                    "",
                    Filter(row, col, NotEqual),
                ),
                (
                    editable,
                    true,
                    "Stage selected row deletions",
                    "",
                    BulkDelete,
                ),
            ],
        );
        let follow = if references.len() == 1 { "Click" } else { "" };
        let mut parent = None;
        for (n, key) in references.iter().enumerate() {
            let label = format!("Parent: {} (FK {})", key.target, key.id);
            if menu_entry(ui, true, &label, follow) {
                parent = Some(GridAction::Follow(row, col, n));
            }
        }
        let trailing = menu_entries(
            ui,
            [
                (true, true, "Inspect / edit cell", "", Inspect(row, col)),
                (true, true, "Copy stored cell", "", CopyCell(row, col)),
                (true, true, "Copy stored row (TSV)", "", CopyRow(row)),
                (true, true, "Record detail", "Space", Record(row)),
                (
                    self.database.writable,
                    editable,
                    "Stage row deletion",
                    "",
                    Delete(row),
                ),
            ],
        );
        trailing.or(parent).or(leading)
    }
    pub(super) fn cell_references(&self, row: usize, col: usize) -> Vec<&ForeignKey> {
        let document = &self.database.document;
        let values = document.result.rows.get(row).map(Vec::as_slice);
        let info = document.tools.inspection.as_ref();
        matching_keys(document.view, document.table.as_ref(), info, values, col)
    }
}
/// The shown entries, each closing the menu when clicked; returns the last one clicked.
fn menu_entries<const N: usize>(ui: &mut egui::Ui, entries: [MenuEntry; N]) -> Option<GridAction> {
    let mut chosen = None;
    for (shown, enabled, label, shortcut, action) in entries {
        if shown && menu_entry(ui, enabled, label, shortcut) {
            chosen = Some(action);
        }
    }
    chosen
}
fn menu_entry(ui: &mut egui::Ui, enabled: bool, label: &str, shortcut: &str) -> bool {
    let button = egui::Button::new(label).shortcut_text(shortcut);
    let clicked = ui.add_enabled(enabled, button).clicked();
    if clicked {
        ui.close();
    }
    clicked
}
fn matching_keys<'a>(
    view: View,
    table: Option<&Table>,
    info: Option<&'a Inspection>,
    values: Option<&[Value]>,
    col: usize,
) -> Vec<&'a ForeignKey> {
    let (Some(table), Some(info), Some(values)) = (table, info, values) else {
        return vec![];
    };
    if view != View::Browse || table.name != info.table.name {
        return vec![];
    }
    let Some(column) = table.columns.get(col) else {
        return vec![];
    };
    info.foreign_keys
        .iter()
        .filter(|key| {
            key.columns
                .iter()
                .any(|(name, _)| name.eq_ignore_ascii_case(&column.name))
                && key.columns.iter().all(|(source, target)| {
                    target.is_some()
                        && table
                            .columns
                            .iter()
                            .position(|c| c.name.eq_ignore_ascii_case(source))
                            .and_then(|i| values.get(i))
                            .is_some_and(|v| *v != Value::Null)
                })
        })
        .collect()
}

fn cell_fill(ui: &egui::Ui, deleted: bool, staged: bool, row: usize) -> egui::Color32 {
    let surface = ui.visuals().extreme_bg_color;
    if deleted {
        crate::appearance::mix(surface, ui.visuals().error_fg_color, 0.13)
    } else if staged {
        crate::appearance::mix(surface, Tokens::of(ui.ctx()).pending, 0.13)
    } else if row.is_multiple_of(2) {
        crate::appearance::mix(surface, ui.visuals().text_color(), 0.025)
    } else {
        surface
    }
}

/// The row number, marked D or M while its deletion or a change is staged,
/// with a bar at its left while staged or inspected.
fn row_gutter(ui: &mut egui::Ui, number: usize, deleted: bool, modified: bool, inspected: bool) {
    let tokens = Tokens::of(ui.ctx());
    let (marker, color) = match (deleted, modified) {
        (true, _) => ("D", ui.visuals().error_fg_color),
        (false, true) => ("M", tokens.pending),
        (false, false) => ("", tokens.faint),
    };
    let text = egui::RichText::new(format!("{marker}{number}"))
        .color(color)
        .monospace()
        .small();
    let gutter = ui.add_sized([GUTTER_WIDTH, ROW_HEIGHT], egui::Label::new(text));
    let bar = if deleted || modified {
        Some(color)
    } else {
        inspected.then_some(tokens.primary)
    };
    if let Some(bar) = bar {
        let (x, y) = (gutter.rect.left() + 1.5, gutter.rect.y_range());
        ui.painter().vline(x, y, egui::Stroke::new(3.0, bar));
    }
}

/// A script statement's number, retained rows, changes and outcome; the
/// statement shown in the grid retains the grid's rows.
fn statement_label(result: &tursobench_core::ScriptResult, retained: Option<usize>) -> String {
    let rows = retained.unwrap_or(result.rows.len());
    let outcome = if result.completion.error.is_some() {
        "ERROR"
    } else {
        "finished"
    };
    let (index, changed) = (result.index + 1, result.completion.changed);
    format!("Statement {index} · {rows} retained rows · {changed} changes · {outcome}")
}
/// The band behind the column headers, across the whole visible width.
fn header_band(ui: &egui::Ui) {
    let band = egui::Rect::from_min_size(
        ui.cursor().min,
        egui::vec2(ui.available_width(), HEADER_HEIGHT),
    );
    ui.painter()
        .rect_filled(band, 0.0, ui.visuals().faint_bg_color);
    ui.painter().hline(
        band.x_range(),
        band.bottom() - 0.5,
        egui::Stroke::new(1.0, Tokens::of(ui.ctx()).line_soft),
    );
}
/// What a click on a grid cell asks for.
struct CellClick<'a> {
    row: usize,
    col: usize,
    references: &'a [&'a ForeignKey],
    /// Browsing a table, where Alt-click filters by the value.
    filterable: bool,
    selected: bool,
}
impl CellClick<'_> {
    /// Alt-click filters, a click on a single reference follows it (Shift
    /// selects instead), a double-click edits, and a right-click selects an
    /// unselected cell for its menu.
    fn action(&self, ui: &egui::Ui, response: &egui::Response) -> Option<GridAction> {
        let (row, col) = (self.row, self.col);
        let (alt, shift) = ui.input(|i| (i.modifiers.alt, i.modifiers.shift));
        if response.secondary_clicked() && !self.selected {
            return Some(GridAction::Cell(row, col));
        }
        if response.double_clicked() {
            return Some(GridAction::Edit(row, col));
        }
        if !response.clicked() {
            return None;
        }
        Some(if alt && self.filterable {
            GridAction::Filter(row, col, FilterOp::Equal)
        } else if self.references.len() == 1 && !shift {
            GridAction::Follow(row, col, 0)
        } else {
            GridAction::Cell(row, col)
        })
    }
}
#[cfg(test)]
mod tests {
    use super::{egui, shell};
    use tursobench_core::{Column, Value};
    #[test]
    fn renders_empty_and_large_cells_without_engine() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.document.ui.nav = shell::Nav::Table;
        app.database.document.result.columns = vec![Column {
            name: "data".into(),
            declared_type: None,
        }];
        app.database.document.result.rows = vec![
            vec![Value::Text("λ".repeat(30000))],
            vec![Value::Blob(vec![0; 64000])],
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.grid_contents(ui));
        output.textures_delta.clear();
        assert!(app.database.session.is_none());
        assert!(app.database.document.active.is_none());
        assert!(app.first_cell_rect.is_some());
    }
}
