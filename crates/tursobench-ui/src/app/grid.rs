pub(super) const ROW_HEIGHT: f32 = 29.0;
use super::{
    App, View,
    icons::{self, Icon},
    shell,
};
use eframe::egui;
use tursobench_core::{ForeignKey, Inspection, Table, Value};

enum GridAction {
    Cell(usize, usize),
    Edit(usize, usize),
    Record(usize),
    Copy(usize),
    Delete(usize),
    BulkDelete,
    Follow(usize, ForeignKey),
    Filter(tursobench_core::Filter),
    Inspect(usize, usize),
    Stage(usize, usize, Value),
}
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
}
impl CellAppearance<'_> {
    fn text(&self) -> String {
        match self.value {
            Value::Null => self.null_display.to_owned(),
            Value::Text(s) if s.is_empty() => "(empty text)".into(),
            Value::Real(n) => format!("{n:?}"),
            _ => self.value.preview(),
        }
    }
    fn describe(&self, ui: &egui::Ui, response: &egui::Response, number: usize) {
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::Cell,
                ui.is_enabled(),
                self.selected,
                format!(
                    "Row {} · {}: {}{}",
                    number,
                    self.column,
                    if self.deleted {
                        "Deletion staged · "
                    } else if self.staged {
                        "Staged · "
                    } else {
                        ""
                    },
                    self.text()
                ),
            )
        });
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
            rect.right_center() - egui::vec2(7.0 + galley.size().x, galley.size().y * 0.5)
        } else {
            rect.left_center() + egui::vec2(7.0, -galley.size().y * 0.5)
        };
        painter.galley(position, galley, color);
    }
    fn paint(&self, ui: &egui::Ui, rect: egui::Rect) {
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        painter.rect_filled(
            rect,
            0.0,
            if self.selected {
                ui.visuals().selection.bg_fill
            } else {
                cell_fill(ui, self.deleted, self.staged, self.row)
            },
        );
        if self.focused {
            painter.rect_stroke(
                rect,
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let styled = self.style.paint(ui, rect, self.column, self.value);
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let color = if self.deleted {
            ui.visuals().error_fg_color
        } else if self.related {
            tokens.relationship
        } else if matches!(self.value, Value::Null) {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        };
        if !styled {
            self.paint_text(ui, &painter, rect, color);
        }
        painter.hline(
            rect.x_range(),
            rect.bottom() - 0.5,
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        painter.vline(
            rect.right() - 0.5,
            rect.y_range(),
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        if self.related {
            let end = rect.right() - 7.0;
            let mut x = rect.left() + 7.0;
            while x < end {
                painter.circle_filled(egui::pos2(x, rect.bottom() - 4.0), 0.65, color);
                x += 3.0;
            }
        }
        if self.deleted {
            painter.line_segment(
                [
                    rect.left_center() + egui::vec2(5.0, 0.0),
                    rect.right_center() - egui::vec2(5.0, 0.0),
                ],
                egui::Stroke::new(1.0, ui.visuals().error_fg_color),
            );
        }
        if self.staged || self.deleted {
            painter.vline(
                rect.left() + 1.5,
                rect.y_range(),
                egui::Stroke::new(
                    3.0,
                    if self.deleted {
                        ui.visuals().error_fg_color
                    } else {
                        tokens.pending
                    },
                ),
            );
        }
    }
}
impl App {
    pub(super) fn grid_contents(&mut self, ui: &mut egui::Ui) {
        if !self.document.script_results.is_empty() {
            let mut selected = None;
            ui.collapsing("Script results and outcomes", |ui| {
                ui.label(&self.document.script_message);
                for (i, result) in self.document.script_results.iter().enumerate() {
                    if ui
                        .button(format!(
                            "Statement {} · {} retained rows · {} changes · {}",
                            result.index + 1,
                            result.rows.len(),
                            result.completion.changed,
                            if result.completion.error.is_some() {
                                "ERROR"
                            } else {
                                "finished"
                            }
                        ))
                        .clicked()
                    {
                        selected = Some(i);
                    }
                }
            });
            if let Some(i) = selected {
                let result = &self.document.script_results[i];
                self.document.result.columns = result.columns.clone();
                self.document.result.rows = result.rows.clone();
                self.document.result.completion = Some(result.completion.clone());
            }
        }
        if self.document.stale {
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
        let grid_id = ui.scope_id().with("grid-keyboard");
        self.document.tools.grid_focus = Some(grid_id);
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
            ui.ctx().memory_mut(|m| {
                m.set_focus_lock_filter(
                    grid_id,
                    egui::EventFilter {
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        ..Default::default()
                    },
                )
            });
        }
        self.document.tools.context.hovered = None;
        let mut action = None;
        let editable = self.can_edit();
        self.document.tools.row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
        if !ui.input(|i| i.pointer.primary_down()) {
            self.document.tools.dragging_selection = false;
        }
        let remaining =
            (ui.available_height() - if self.workspace.enabled { 72.0 } else { 52.0 }).max(0.0);
        self.document.tools.grid_height = remaining;
        #[cfg(test)]
        {
            self.ui.grid_rect = ui.available_rect_before_wrap();
        }
        self.prepare_columns(ui);
        if self.document.view == View::Explain
            && let Some(detail) = self
                .document
                .result
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case("detail"))
        {
            let scans=self.document.result.rows.iter().filter(|r|matches!(r.get(detail),Some(Value::Text(s))if s.to_uppercase().contains("SCAN "))).count();
            ui.label(format!("Planner notes: {scans} scan entries. Scans may be intentional; not a runtime profile."));
        }
        let mut page_range = 0..0;
        let restore = self.document.tools.restore_scroll.take();
        let horizontal = egui::ScrollArea::horizontal()
            .horizontal_scroll_offset(restore.map_or(self.document.tools.grid_scroll.x, |s| s.x))
            .id_salt("result-columns")
            .show(ui, |ui| {
                if !self.document.result.columns.is_empty() {
                    self.grid_header(ui);
                }
                let vertical = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(
                        restore.map_or(self.document.tools.grid_scroll.y, |s| s.y),
                    )
                    .id_salt("result-rows")
                    .max_height(remaining)
                    .show_rows(
                        ui,
                        ROW_HEIGHT,
                        self.document.result.rows.len(),
                        |ui, range| {
                            page_range = range.clone();
                            #[cfg(test)]
                            {
                                self.ui.visible_rows = range
                                    .clone()
                                    .filter(|r| {
                                        (*r + 1) as f32 * (29.0 + ui.spacing().item_spacing.y)
                                            <= remaining
                                    })
                                    .count();
                            }
                            for row in range {
                                if let Some(selected) = self.grid_row(ui, row, editable) {
                                    action = Some(selected);
                                }
                            }
                        },
                    );
                self.document.tools.grid_scroll.y = vertical.state.clamped_offset().y;
            });
        self.document.tools.grid_scroll.x = horizontal.state.clamped_offset().x;
        match action {
            Some(GridAction::Cell(r, c)) => self.select_cell(
                r,
                c,
                ui.input(|i| i.modifiers.shift) || self.document.tools.dragging_selection,
            ),
            Some(GridAction::Edit(r, c)) => self.start_inline(r, c),
            Some(GridAction::Inspect(r, c)) => {
                if self.workspace.enabled {
                    self.open_record_window(r, c);
                } else {
                    self.open_cell(r, c);
                }
            }
            Some(GridAction::Stage(r, c, v)) => {
                self.stage_cell(r, c, v);
                self.document.tools.inline = None;
                if let Some(id) = self.document.tools.grid_focus {
                    ui.ctx().memory_mut(|m| m.request_focus(id));
                }
            }
            Some(GridAction::Record(r)) if self.workspace.enabled => self.open_record_window(r, 0),
            Some(GridAction::Record(r)) => {
                self.document.tools.detail = Some(r);
                self.document.ui.inspector = shell::DetailMode::Record;
                self.document.ui.inspector_open = true;
            }
            Some(GridAction::Copy(r)) => self.copy_row(ui.ctx(), r),
            Some(GridAction::Delete(r)) => self.stage_delete(r),
            Some(GridAction::BulkDelete) => self.stage_selected_deletions(),
            Some(GridAction::Follow(r, key)) => self.follow_key(r, key),
            Some(GridAction::Filter(filter)) => {
                self.add_filter(filter);
            }
            None => {}
        }
        if self.workspace.enabled && !self.document.result.rows.is_empty() {
            let start = self.document.browse.as_ref().map_or(0, |b| b.offset);
            let count = self.document.result.rows.len();
            let range = format!("{}–{} · {} retained", start + 1, start + count, count);
            ui.small(format!("{range} · {}", self.export_scope()))
                .on_hover_text(
                    "Only retained rows are exported. Scroll to load another table page.",
                );
        }
        if let Some(spec) = self.document.browse.clone() {
            let mut jump = false;
            ui.horizontal(|ui| {
                ui.label("Go to row");
                ui.add(
                    egui::DragValue::new(&mut self.document.tools.jump_row)
                        .range(1..=i64::MAX as usize),
                );
                jump = ui.button("Go").clicked();
                if ui.button("Count matching rows").clicked() {
                    self.count_table(spec.table.clone());
                }
                if let Some(count) = self.tools.counts.get(&spec.table) {
                    ui.small(format!("{count} matching rows (last count)"));
                }
            });
            if jump && !self.has_drafts() {
                let mut next = spec;
                next.offset = self.document.tools.jump_row.saturating_sub(1);
                next.options.cursor = None;
                next.options.reverse = false;
                if self
                    .submit(tursobench_db::Operation::Browse(next.clone()), View::Browse)
                    .is_some()
                {
                    self.document.browse = Some(next);
                }
            }
        }
        if let Some(cell) = self
            .document
            .inspect
            .and_then(|(r, c)| self.document.result.rows.get(r)?.get(c))
            .and_then(|v| {
                if let Value::Deferred(v) = v {
                    Some((**v).clone())
                } else {
                    None
                }
            })
            && ui.button("View large value…").clicked()
        {
            self.open_large_value(cell);
        }
        self.hover_context(ui.ctx());
        if self.document.ui.nav == shell::Nav::Table
            && self.document.ui.table_view == shell::TableView::Data
        {
            self.maybe_page(
                page_range.start,
                page_range.end,
                ui.input(|i| i.smooth_scroll_delta.y),
            );
            if self.document.tools.pages.pending.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Loading 100 rows…");
                });
            }
            if !self.document.tools.pages.error.is_empty() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    &self.document.tools.pages.error,
                );
                if ui.button("Retry page").clicked() {
                    self.request_page(self.document.tools.pages.retry_previous);
                }
            }
        }
        if !self.workspace.enabled
            && self.writable
            && self.document.view == View::Browse
            && self
                .document
                .table
                .as_ref()
                .is_some_and(tursobench_core::Table::editable)
            && ui
                .add_enabled_ui(self.can_edit(), |ui| {
                    icons::control(ui, Icon::Plus, "New row", "", false, true)
                })
                .inner
                .clicked()
        {
            self.begin_insert();
        }
        if !self.document.tools.inspection_error.is_empty()
            && self.document.result.columns.is_empty()
        {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                &self.document.tools.inspection_error,
            );
        }
        if self.document.result.rows.is_empty() && self.document.active.is_none() {
            icons::empty_state(ui, Icon::Data, "No rows to display");
        }
    }
    fn grid_header(&mut self, ui: &mut egui::Ui) {
        let mut sort = None;
        ui.horizontal(|ui| {
            ui.add_sized([44.0, 42.0], egui::Label::new("#"));
            for i in self.visible_columns() {
                if self.grid_column_header(ui, i) {
                    sort = Some(i);
                }
            }
        });
        if let Some(i) = sort
            && self.document.view == View::Browse
            && self.ready
            && !self.closing
            && !self.has_drafts()
            && self.commit_ticket.is_none()
            && let Some(b) = self.document.browse.as_mut()
        {
            if ui.input(|i| i.modifiers.shift) {
                if let Some((_, desc)) = b.options.sorts.iter_mut().find(|(col, _)| *col == i) {
                    *desc = !*desc;
                } else if b.options.sorts.len() < 7 {
                    b.options.sorts.push((i, false));
                }
            } else if b.sort != Some(i) {
                b.options.sorts.clear();
                b.sort = Some(i);
                b.descending = false;
            } else if !b.descending {
                b.descending = true;
            } else {
                b.sort = None;
                b.descending = false;
            }
            b.offset = 0;
            b.options.cursor = None;
            b.options.reverse = false;
            self.reload();
        }
    }
    fn grid_column_header(&mut self, ui: &mut egui::Ui, i: usize) -> bool {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let header = self.document.column_header(i);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(self.document.tools.columns.widths[i], 42.0),
            egui::Sense::click_and_drag(),
        );
        header.describe(&response, ui.is_enabled());
        let highlighted =
            self.document
                .tools
                .highlight_column
                .as_ref()
                .is_some_and(|(table, name)| {
                    self.document.view == View::Browse
                        && self
                            .document
                            .table
                            .as_ref()
                            .is_some_and(|t| &t.name == table)
                        && &header.column.name == name
                });
        if highlighted && self.document.tools.scroll_to_column {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
        }
        header.paint(ui, rect, highlighted);
        let response=response.on_hover_text(format!("{} · {}\nPK = key position; IX = declared leading index; FK = foreign key. Index method/partial predicate may not apply. Drag the right edge or right-click to resize.",header.column.name,header.declared_type().unwrap_or("runtime type")));
        let filter_label = format!("Filter {}", header.column.name);
        if highlighted {
            self.document.tools.scroll_to_column = false;
        }
        response.context_menu(|ui| self.document.tools.columns.header_menu(ui, i));
        if response.drag_started() {
            response.dnd_set_drag_payload(i);
        }
        if let Some(from) = response.dnd_release_payload::<usize>() {
            self.document.tools.columns.move_before(*from, i);
        }

        let funnel_rect = egui::Rect::from_min_size(
            rect.right_top() + egui::vec2(-24.0, 5.0),
            egui::vec2(17.0, 20.0),
        );
        let funnel = ui.interact(
            funnel_rect,
            ui.scope_id().with(("column-filter", i)),
            egui::Sense::click(),
        );
        let filtered = self
            .document
            .browse
            .as_ref()
            .is_some_and(|b| b.filters.iter().any(|f| f.column == i));
        icons::paint(
            ui.painter(),
            funnel_rect.shrink(2.0),
            Icon::Filter,
            if filtered {
                tokens.primary
            } else {
                tokens.faint
            },
        );
        funnel.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &filter_label)
        });
        egui::Popup::menu(&funnel).show(|ui| self.column_facets(ui, i));
        let handle = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 6.0, rect.top()),
            rect.right_bottom(),
        );
        let drag = ui
            .interact(
                handle,
                ui.scope_id().with(("resize-column", i)),
                egui::Sense::click_and_drag(),
            )
            .on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
        if drag.double_clicked() {
            self.document.tools.columns.fit(i);
        }
        if drag.dragged() {
            self.document.tools.columns.manual.insert(i);
            self.document.tools.columns.widths[i] =
                (self.document.tools.columns.widths[i] + drag.drag_delta().x).clamp(60.0, 640.0);
        }
        response.clicked() && !drag.contains_pointer() && !funnel.contains_pointer()
    }
    fn grid_row(&mut self, ui: &mut egui::Ui, row: usize, editable: bool) -> Option<GridAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let values = &self.document.result.rows[row];
            let deleted = self
                .document
                .drafts
                .as_ref()
                .is_some_and(|d| d.deleted(values));
            let modified = self
                .document
                .drafts
                .as_ref()
                .is_some_and(|d| (0..values.len()).any(|c| d.displayed(values, c).is_some()));
            let offset = self.document.browse.as_ref().map_or(0, |b| b.offset);
            let marker = if deleted {
                "D"
            } else if modified {
                "M"
            } else {
                ""
            };
            let tokens = crate::appearance::Tokens::of(ui.ctx());
            let marker_color = if deleted {
                ui.visuals().error_fg_color
            } else if modified {
                tokens.pending
            } else {
                tokens.faint
            };
            let gutter = ui.add_sized(
                [44.0, 29.0],
                egui::Label::new(
                    egui::RichText::new(format!("{marker}{}", offset + row + 1))
                        .color(marker_color)
                        .monospace()
                        .small(),
                ),
            );
            if deleted || modified || self.document.inspect.is_some_and(|(r, _)| r == row) {
                ui.painter().vline(
                    gutter.rect.left() + 1.5,
                    gutter.rect.y_range(),
                    egui::Stroke::new(
                        3.0,
                        if deleted || modified {
                            marker_color
                        } else {
                            tokens.primary
                        },
                    ),
                );
            }
            for col in self.visible_columns() {
                if let Some(next) = self.grid_cell(ui, row, col, deleted, offset, editable) {
                    action = Some(next);
                }
            }
        });
        action
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
        let mut action = None;
        let values = &self.document.result.rows[row];
        let value = &values[col];
        let references = matching_keys(
            self.document.view,
            self.document.table.as_ref(),
            self.document.tools.inspection.as_ref(),
            Some(values),
            col,
        );
        let staged = self
            .document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col));
        let displayed = staged.unwrap_or(value);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(self.document.tools.columns.widths[col], 29.0),
            egui::Sense::click_and_drag(),
        );
        let appearance = CellAppearance {
            value: displayed,
            column: &self.document.result.columns[col].name,
            null_display: &self.preferences.null_display,
            style: self
                .document
                .tools
                .columns
                .visuals
                .get(&col)
                .copied()
                .unwrap_or_default(),
            selected: self.cell_selected(row, col),
            focused: self.document.inspect == Some((row, col)) || response.has_focus(),
            deleted,
            staged: staged.is_some(),
            related: !references.is_empty(),
            row,
        };
        appearance.describe(ui, &response, offset + row + 1);
        if let Some(edit) = self.document.tools.inline.as_mut().filter(|e| {
            e.row == row
                && e.column == col
                && (!self.workspace.enabled || self.workspace.active != super::tiling::Kind::Record)
        }) {
            let response = ui.put(rect, egui::TextEdit::singleline(&mut edit.editor.input));
            response.context_menu(|ui| edit.editor.type_picker(ui, "inline-type", false));
            match edit.submitted(ui, &response) {
                Some(Ok(value)) => action = Some(GridAction::Stage(row, col, value)),
                Some(Err(error)) => self.status = error,
                None => {}
            }
            return action;
        }
        if self.document.tools.scroll_to_cell && self.document.inspect == Some((row, col)) {
            ui.scroll_to_rect(rect, Some(egui::Align::Center));
            self.document.tools.scroll_to_cell = false;
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
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.document.tools.grid_selection = Some(super::selection::Selection {
                ticket: self.document.result.current,
                anchor: (row, col),
                cursor: (row, col),
            });
            self.document.tools.dragging_selection = true;
            if let Some(id) = self.document.tools.grid_focus {
                ui.ctx().memory_mut(|m| m.request_focus(id));
            }
        }
        if response.hovered()
            && self.document.tools.dragging_selection
            && ui.input(|i| i.pointer.primary_down())
        {
            if let Some(s) = &mut self.document.tools.grid_selection {
                s.cursor = (row, col);
            }
            action = Some(GridAction::Cell(row, col));
        }
        if response.clicked()
            && let Some(id) = self.document.tools.grid_focus
        {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        if response.clicked() && ui.input(|i| i.modifiers.alt) && self.document.browse.is_some() {
            action = Some(GridAction::Filter(tursobench_core::Filter {
                column: col,
                op: tursobench_core::FilterOp::Equal,
                value: value.clone(),
            }));
        } else if response.clicked() {
            action = Some(
                if references.len() == 1 && !ui.input(|i| i.modifiers.shift) {
                    GridAction::Follow(row, (*references[0]).clone())
                } else {
                    GridAction::Cell(row, col)
                },
            );
        }
        if response.double_clicked() {
            action = Some(GridAction::Edit(row, col));
        }
        if response.hovered() && references.len() == 1 {
            self.document.tools.context.hovered = Some((row, (*references[0]).clone()));
        }
        let response = if references.is_empty() {
            response
        } else {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_ui(|ui| {
                    if references.len() == 1 {
                        self.peek_view(ui, row, references[0]);
                    } else {
                        ui.label("Multiple references: right-click to choose");
                    }
                })
        };
        response.context_menu(|ui| {
            if let Some(next) = self.cell_menu(ui, row, col, editable, &references) {
                action = Some(next);
            }
        });
        action
    }
    fn cell_menu(
        &self,
        ui: &mut egui::Ui,
        row: usize,
        col: usize,
        editable: bool,
        references: &[&ForeignKey],
    ) -> Option<GridAction> {
        let mut action = None;
        let value = &self.document.result.rows[row][col];

        if self.document.view == View::Browse && self.document.browse.is_some() {
            for (op, label) in [
                (tursobench_core::FilterOp::Equal, "Filter = value"),
                (tursobench_core::FilterOp::NotEqual, "Exclude value"),
            ] {
                if ui.button(label).clicked() {
                    action = Some(GridAction::Filter(tursobench_core::Filter {
                        column: col,
                        op,
                        value: value.clone(),
                    }));
                    ui.close();
                }
            }
        }
        if editable && ui.button("Stage selected row deletions").clicked() {
            return Some(GridAction::BulkDelete);
        }
        for key in references {
            if ui
                .button(format!("Parent: {} (FK {})", key.target, key.id))
                .clicked()
            {
                action = Some(GridAction::Follow(row, (*key).clone()));
                ui.close();
            }
        }
        if ui.button("Inspect / edit cell").clicked() {
            action = Some(GridAction::Inspect(row, col));
            ui.close();
        }
        if ui.button("Copy stored cell").clicked() {
            ui.ctx().copy_text(crate::export::cell_text(value));
            ui.close();
        }
        if ui.button("Copy stored row (TSV)").clicked() {
            action = Some(GridAction::Copy(row));
            ui.close();
        }
        if ui.button("Record detail").clicked() {
            action = Some(GridAction::Record(row));
            ui.close();
        }
        if self.writable
            && ui
                .add_enabled(editable, egui::Button::new("Stage row deletion"))
                .clicked()
        {
            action = Some(GridAction::Delete(row));
            ui.close();
        }
        action
    }
    pub(super) fn cell_references(&self, row: usize, col: usize) -> Vec<&ForeignKey> {
        matching_keys(
            self.document.view,
            self.document.table.as_ref(),
            self.document.tools.inspection.as_ref(),
            self.document.result.rows.get(row).map(Vec::as_slice),
            col,
        )
    }
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
        crate::appearance::mix(
            surface,
            crate::appearance::Tokens::of(ui.ctx()).pending,
            0.13,
        )
    } else if row.is_multiple_of(2) {
        crate::appearance::mix(surface, ui.visuals().text_color(), 0.025)
    } else {
        surface
    }
}
