pub(super) const ROW_HEIGHT: f32 = 29.0;
pub(super) const STATUS_HEIGHT: f32 = ROW_HEIGHT + 2.0;
use super::presentation::{CELL_INSET, CONTENT_INSET, GUTTER_WIDTH};
pub(super) const HEADER_HEIGHT: f32 = 42.0;
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
    Spreadsheet(super::spreadsheet::Action),
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
            rect.right_center() - egui::vec2(CELL_INSET + galley.size().x, galley.size().y * 0.5)
        } else {
            rect.left_center() + egui::vec2(CELL_INSET, -galley.size().y * 0.5)
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
            let end = rect.right() - CELL_INSET;
            let mut x = rect.left() + CELL_INSET;
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
    fn script_results_ui(&mut self, ui: &mut egui::Ui) {
        if !self.database.document.script_results.is_empty() {
            let mut selected = None;
            ui.collapsing("Script results and outcomes", |ui| {
                ui.label(&self.database.document.script_message);
                for (i, result) in self.database.document.script_results.iter().enumerate() {
                    if ui
                        .button(format!(
                            "Statement {} · {} retained rows · {} changes · {}",
                            result.index + 1,
                            if self.database.document.selected_script == Some(i) {
                                self.database.document.result.rows.len()
                            } else {
                                result.rows.len()
                            },
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
                self.database.document.select_script_result(i);
            }
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
        self.database.document.tools.context.hovered = None;
        let mut action = None;
        let editable = self.can_edit();
        self.database.document.tools.row_stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
        if !ui.input(|i| i.pointer.primary_down()) {
            self.database.document.tools.dragging_selection = false;
        }
        let auxiliary_rows = usize::from(
            self.database
                .document
                .inspect
                .and_then(|(r, c)| self.database.document.result.rows.get(r)?.get(c))
                .is_some_and(|v| matches!(v, Value::Deferred(_))),
        ) + usize::from(self.database.document.tools.pages.pending.is_some())
            + 2 * usize::from(!self.database.document.tools.pages.error.is_empty());
        let remaining =
            (ui.available_height() - HEADER_HEIGHT - auxiliary_rows as f32 * ROW_HEIGHT).max(0.0);
        self.database.document.tools.grid_height = remaining;
        #[cfg(test)]
        {
            self.database.ui.grid_rect = ui.available_rect_before_wrap();
        }
        self.prepare_columns(ui);
        if self.database.document.view == View::Explain
            && let Some(detail) = self
                .database
                .document
                .result
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case("detail"))
        {
            let scans=self.database.document.result.rows.iter().filter(|r|matches!(r.get(detail),Some(Value::Text(s))if s.to_uppercase().contains("SCAN "))).count();
            ui.label(format!("Planner notes: {scans} scan entries. Scans may be intentional; not a runtime profile."));
        }
        let mut page_range = 0..0;
        let restore = self.database.document.tools.restore_scroll.take();
        if !self.database.document.result.columns.is_empty() {
            let band = egui::Rect::from_min_size(
                ui.cursor().min,
                egui::vec2(ui.available_width(), HEADER_HEIGHT),
            );
            ui.painter()
                .rect_filled(band, 0.0, ui.visuals().faint_bg_color);
            ui.painter().hline(
                band.x_range(),
                band.bottom() - 0.5,
                egui::Stroke::new(1.0, crate::appearance::Tokens::of(ui.ctx()).line_soft),
            );
        }
        let horizontal = egui::ScrollArea::horizontal()
            .horizontal_scroll_offset(
                restore.map_or(self.database.document.tools.grid_scroll.x, |s| s.x),
            )
            .id_salt("result-columns")
            .show(ui, |ui| {
                if !self.database.document.result.columns.is_empty() {
                    self.grid_header(ui);
                }
                let vertical = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(
                        restore.map_or(self.database.document.tools.grid_scroll.y, |s| s.y),
                    )
                    .id_salt("result-rows")
                    .max_height(remaining)
                    .show_rows(
                        ui,
                        ROW_HEIGHT,
                        self.database.document.result.rows.len(),
                        |ui, range| {
                            page_range = range.clone();
                            #[cfg(test)]
                            {
                                self.database.ui.visible_rows = range
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
                self.database.document.tools.grid_scroll.y = vertical.state.clamped_offset().y;
            });
        self.database.document.tools.grid_scroll.x = horizontal.state.clamped_offset().x;
        match action {
            Some(GridAction::Cell(r, c)) => self.select_cell(
                r,
                c,
                ui.input(|i| i.modifiers.shift) || self.database.document.tools.dragging_selection,
            ),
            Some(GridAction::Edit(r, c)) => self.start_inline(r, c),
            Some(GridAction::Inspect(r, c)) => {
                self.open_record_window(r, c);
            }
            Some(GridAction::Stage(r, c, v)) => {
                self.stage_cell(r, c, v);
                self.database.document.tools.inline = None;
                if let Some(id) = self.database.document.tools.grid_focus {
                    ui.ctx().memory_mut(|m| m.request_focus(id));
                }
            }
            Some(GridAction::Record(r)) => self.open_record_window(r, 0),
            Some(GridAction::Copy(r)) => self.copy_row(ui.ctx(), r),
            Some(GridAction::Delete(r)) => self.stage_delete(r),
            Some(GridAction::BulkDelete) => self.stage_selected_deletions(),
            Some(GridAction::Spreadsheet(action)) => self.spreadsheet_action(action),
            Some(GridAction::Follow(r, key)) => self.follow_key(r, key),
            Some(GridAction::Filter(filter)) => {
                self.add_filter(filter);
            }
            None => {}
        }
        if let Some(cell) = self
            .database
            .document
            .inspect
            .and_then(|(r, c)| self.database.document.result.rows.get(r)?.get(c))
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
        if self.database.document.ui.nav == shell::Nav::Table
            && self.database.document.ui.table_view == shell::TableView::Data
        {
            self.maybe_page(
                page_range.start,
                page_range.end,
                ui.input(|i| i.smooth_scroll_delta.y),
            );
            if self.database.document.tools.pages.pending.is_some() {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.small("Loading 100 rows…");
                });
            }
            if !self.database.document.tools.pages.error.is_empty() {
                ui.colored_label(
                    ui.visuals().error_fg_color,
                    &self.database.document.tools.pages.error,
                );
                if ui.button("Retry page").clicked() {
                    self.request_page(self.database.document.tools.pages.retry_previous);
                }
            }
        }
        if !self.database.document.tools.inspection_error.is_empty()
            && self.database.document.result.columns.is_empty()
        {
            ui.colored_label(
                ui.visuals().warn_fg_color,
                &self.database.document.tools.inspection_error,
            );
        }
        if self.database.document.result.rows.is_empty() && self.database.document.active.is_none()
        {
            icons::empty_state(ui, Icon::Data, "No rows to display");
        }
    }
    fn grid_status_bar(&mut self, ui: &mut egui::Ui) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let bar = egui::Panel::bottom("grid-status-bar")
            .resizable(false)
            .exact_size(STATUS_HEIGHT)
            .frame(
                egui::Frame::new()
                    .fill(tokens.raised)
                    .inner_margin(egui::Margin::symmetric(
                        super::presentation::BAR_INSET as i8,
                        1,
                    ))
                    .corner_radius(egui::CornerRadius {
                        sw: 6,
                        se: 6,
                        ..Default::default()
                    }),
            )
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = egui::vec2(super::presentation::BAR_SPACING, 0.0);
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

    fn grid_status_contents(&mut self, ui: &mut egui::Ui) {
        let enabled = self.database.ready
            && !self.database.closing
            && !self.database.document.stale
            && !self.has_drafts()
            && self.database.commit_ticket.is_none()
            && self.database.document.active.is_none()
            && self.database.document.tools.pages.pending.is_none();
        let spec = self.database.document.browse.as_ref();
        let mut jump = false;
        let mut count_requested = false;
        let count_label = spec.as_ref().and_then(|b| {
            self.database
                .document
                .tools
                .filtered_count
                .map(|n| format!("{n} matching"))
                .or_else(|| {
                    self.database
                        .tools
                        .counts
                        .get(&b.table)
                        .map(|n| format!("{n} total"))
                })
        });
        let font = crate::appearance::proportional(ui.ctx(), 12.0);
        let text_width = |text: String| {
            ui.painter()
                .layout_no_wrap(text, font.clone(), ui.visuals().text_color())
                .size()
                .x
        };
        let actions_width = if spec.is_some() {
            text_width(format!("Row {}", self.database.document.tools.jump_row))
                + 108.0
                + count_label
                    .as_ref()
                    .map_or(0.0, |s| text_width(s.clone()) + 6.0)
        } else {
            0.0
        };
        let leading = CONTENT_INSET - super::presentation::BAR_INSET;
        let width = ui.available_width().max(leading + actions_width + 120.0);
        egui::ScrollArea::horizontal()
            .id_salt("grid-status-actions")
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal(|ui| {
                    ui.add_space(leading);
                    let start = spec.as_ref().map_or(0, |b| b.offset);
                    let count = self.database.document.result.rows.len();
                    ui.allocate_ui_with_layout(
                        egui::vec2(
                            width - leading - actions_width - super::presentation::BAR_SPACING,
                            ROW_HEIGHT,
                        ),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(format!(
                                        "{}–{} · {count} retained",
                                        start + usize::from(count > 0),
                                        start + count
                                    ))
                                    .monospace()
                                    .size(crate::appearance::text_size(ui.ctx(), 11.0))
                                    .color(ui.visuals().weak_text_color()),
                                )
                                .truncate(),
                            )
                            .on_hover_text(if spec.is_some() {
                                "Rows held in memory for this table. Scroll or PgDn loads the next 100."
                            } else {
                                "Rows held in memory for this result. Exports and copies use these rows without rerunning SQL."
                            });
                        },
                    );
                    if spec.is_some() {
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui
                                .add_enabled_ui(
                                    self.database.ready
                                        && !self.database.closing
                                        && self.database.tools.count_request.is_none(),
                                    |ui| icons::button(ui, Icon::Count, "Count matching rows", ""),
                                )
                                .inner
                                .clicked()
                            {
                                count_requested = true;
                            }
                            if let Some(label) = &count_label {
                                ui.weak(label);
                            }
                            ui.separator();
                            jump = ui
                                .add_enabled_ui(enabled, |ui| {
                                    icons::button(ui, Icon::Next, "Go to row", "")
                                })
                                .inner
                                .clicked();
                            let input = ui.add_enabled(
                                enabled,
                                egui::DragValue::new(&mut self.database.document.tools.jump_row)
                                    .range(1..=i64::MAX as usize)
                                    .prefix("Row "),
                            );
                            input.widget_info(|| {
                                egui::WidgetInfo::labeled(
                                    egui::Role::SpinButton,
                                    enabled,
                                    "Row number",
                                )
                            });
                            // Typing a number and pressing Enter is the same as the arrow.
                            jump |= input.lost_focus()
                                && ui.input(|i| i.key_pressed(egui::Key::Enter));
                            input.on_hover_text(
                                "Go to row: click to type a row number, then press Enter · or drag to change it",
                            );
                        });
                    }
                });
            });
        let next = if jump { spec.cloned() } else { None };
        if count_requested && let Some(name) = spec.map(|b| b.table.clone()) {
            self.count_table(name);
        }
        if let Some(mut next) = next {
            next.offset = self.database.document.tools.jump_row.saturating_sub(1);
            next.options.cursor = None;
            next.options.reverse = false;
            next.options.tail = false;
            if self
                .submit(tursobench_db::Operation::Browse(next.clone()), View::Browse)
                .is_some()
            {
                self.database.document.browse = Some(next);
            }
        }
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
        if let Some(i) = sort
            && self.database.document.view == View::Browse
            && self.requests_enabled()
            && let Some(b) = self.database.document.browse.as_mut()
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
        let header = self.database.document.column_header(i);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(
                self.database.document.tools.columns.widths[i],
                HEADER_HEIGHT,
            ),
            egui::Sense::click_and_drag(),
        );
        header.describe(&response, ui.is_enabled());
        let highlighted = self
            .database
            .document
            .tools
            .highlight_column
            .as_ref()
            .is_some_and(|(table, name)| {
                self.database.document.view == View::Browse
                    && self
                        .database
                        .document
                        .table
                        .as_ref()
                        .is_some_and(|t| &t.name == table)
                    && &header.column.name == name
            });
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
        if highlighted {
            self.database.document.tools.scroll_to_column = false;
        }
        response.context_menu(|ui| self.database.document.tools.columns.header_menu(ui, i));
        if response.drag_started() {
            response.dnd_set_drag_payload(i);
        }
        if let Some(from) = response.dnd_release_payload::<usize>() {
            self.database.document.tools.columns.move_before(*from, i);
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
            .database
            .document
            .browse
            .as_ref()
            .is_some_and(|b| b.filters.iter().any(|f| f.column == i));
        if funnel.hovered() {
            ui.painter()
                .rect_filled(funnel_rect, 4.0, tokens.raised_hover);
        }
        icons::paint(
            ui.painter(),
            funnel_rect.shrink(2.0),
            Icon::Filter,
            if filtered {
                tokens.primary
            } else if funnel.hovered() {
                ui.visuals().text_color()
            } else {
                tokens.faint
            },
        );
        funnel.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), &filter_label)
        });
        let funnel = funnel.on_hover_text(if filtered {
            format!("{filter_label} · a filter is active on this column")
        } else {
            format!("{filter_label} · choose from sampled values")
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
            self.database.document.tools.columns.fit(i);
        }
        if drag.dragged() {
            self.database.document.tools.columns.manual.insert(i);
            self.database.document.tools.columns.widths[i] =
                (self.database.document.tools.columns.widths[i] + drag.drag_delta().x)
                    .clamp(60.0, 640.0);
        }
        response.clicked() && !drag.contains_pointer() && !funnel.contains_pointer()
    }
    fn grid_row(&mut self, ui: &mut egui::Ui, row: usize, editable: bool) -> Option<GridAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let overlay = self.database.document.tools.draft_ui.rows.get(&row);
            let deleted = overlay.is_some_and(|r| r.deleted);
            let modified = overlay.is_some_and(|r| r.modified());
            let offset = self
                .database
                .document
                .browse
                .as_ref()
                .map_or(0, |b| b.offset);
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
                [GUTTER_WIDTH, ROW_HEIGHT],
                egui::Label::new(
                    egui::RichText::new(format!("{marker}{}", offset + row + 1))
                        .color(marker_color)
                        .monospace()
                        .small(),
                ),
            );
            if deleted
                || modified
                || self
                    .database
                    .document
                    .inspect
                    .is_some_and(|(r, _)| r == row)
            {
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
        let values = &self.database.document.result.rows[row];
        let value = &values[col];
        let references = matching_keys(
            self.database.document.view,
            self.database.document.table.as_ref(),
            self.database.document.tools.inspection.as_ref(),
            Some(values),
            col,
        );
        let staged = self.database.document.tools.draft_ui.value(
            self.database.document.drafts.as_ref(),
            row,
            col,
        );
        let displayed = staged.unwrap_or(value);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(self.database.document.tools.columns.widths[col], 29.0),
            egui::Sense::click_and_drag(),
        );
        let appearance = CellAppearance {
            value: displayed,
            column: &self.database.document.result.columns[col].name,
            null_display: &self.preferences.null_display,
            style: self
                .database
                .document
                .tools
                .columns
                .visuals
                .get(&col)
                .copied()
                .unwrap_or_default(),
            selected: self.cell_selected(row, col),
            focused: self.database.document.inspect == Some((row, col)) || response.has_focus(),
            deleted,
            staged: staged.is_some(),
            related: !references.is_empty(),
            row,
        };
        appearance.describe(ui, &response, offset + row + 1);
        if let Some(edit) = self.database.document.tools.inline.as_mut().filter(|e| {
            e.row == row
                && e.column == col
                && self.database.workspace.active != super::tiling::Kind::Record
        }) {
            let response = ui.put(rect, egui::TextEdit::singleline(&mut edit.editor.input));
            response.context_menu(|ui| edit.editor.type_picker(ui, "inline-type", false));
            match edit.submitted(ui, &response) {
                Some(Ok(value)) => action = Some(GridAction::Stage(row, col, value)),
                Some(Err(error)) => self.database.status = error,
                None => {}
            }
            return action;
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
        if response.drag_started_by(egui::PointerButton::Primary) {
            self.database.document.tools.grid_selection = Some(super::selection::Selection {
                ticket: self.database.document.result.current,
                anchor: (row, col),
                cursor: (row, col),
            });
            self.database.document.tools.dragging_selection = true;
            if let Some(id) = self.database.document.tools.grid_focus {
                ui.ctx().memory_mut(|m| m.request_focus(id));
            }
        }
        if response.hovered()
            && self.database.document.tools.dragging_selection
            && ui.input(|i| i.pointer.primary_down())
        {
            if let Some(s) = &mut self.database.document.tools.grid_selection {
                s.cursor = (row, col);
            }
            action = Some(GridAction::Cell(row, col));
        }
        if response.clicked()
            && let Some(id) = self.database.document.tools.grid_focus
        {
            ui.ctx().memory_mut(|m| m.request_focus(id));
        }
        if response.clicked()
            && ui.input(|i| i.modifiers.alt)
            && self.database.document.browse.is_some()
        {
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
        if response.secondary_clicked() && !self.cell_selected(row, col) {
            action = Some(GridAction::Cell(row, col));
        }
        if response.hovered() && references.len() == 1 {
            self.database.document.tools.context.hovered = Some((row, (*references[0]).clone()));
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
        let value = &self.database.document.result.rows[row][col];

        if self.database.document.view == View::Browse {
            if let Some(action) = super::spreadsheet::menu(
                ui,
                editable,
                self.selected_cells().next().is_some(),
                self.can_undo_grid_edit(),
            ) {
                return Some(GridAction::Spreadsheet(action));
            }
            ui.separator();
        }

        if self.database.document.view == View::Browse && self.database.document.browse.is_some() {
            for (op, label, gesture) in [
                (
                    tursobench_core::FilterOp::Equal,
                    "Filter = value",
                    "Alt click",
                ),
                (tursobench_core::FilterOp::NotEqual, "Exclude value", ""),
            ] {
                if ui
                    .add(egui::Button::new(label).shortcut_text(gesture))
                    .clicked()
                {
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
                .add(
                    egui::Button::new(format!("Parent: {} (FK {})", key.target, key.id))
                        .shortcut_text(if references.len() == 1 { "Click" } else { "" }),
                )
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
        if ui
            .add(egui::Button::new("Record detail").shortcut_text("Space"))
            .clicked()
        {
            action = Some(GridAction::Record(row));
            ui.close();
        }
        if self.database.writable
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
            self.database.document.view,
            self.database.document.table.as_ref(),
            self.database.document.tools.inspection.as_ref(),
            self.database
                .document
                .result
                .rows
                .get(row)
                .map(Vec::as_slice),
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
