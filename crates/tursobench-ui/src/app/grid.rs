use super::{
    App, View,
    icons::{self, Icon},
    shell,
};
use eframe::egui;
use tursobench_core::{ForeignKey, Value};

enum GridAction {
    Cell(usize, usize),
    Edit(usize, usize),
    Record(usize),
    Copy(usize),
    Delete(usize),
    Follow(usize, ForeignKey),
    Filter(tursobench_core::Filter),
    Inspect(usize, usize),
    Stage(usize, usize, Value),
}
impl App {
    pub(super) fn grid_contents(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        {
            self.fk_cell_rect = None;
        }
        ui.spacing_mut().item_spacing.y = 1.0;
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
        if !ui.input(|i| i.pointer.primary_down()) {
            self.document.tools.dragging_selection = false;
        }
        let remaining =
            (ui.available_height() - if self.workspace.enabled { 58.0 } else { 38.0 }).max(0.0);
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
                self.grid_header(ui);
                let vertical = egui::ScrollArea::vertical()
                    .vertical_scroll_offset(
                        restore.map_or(self.document.tools.grid_scroll.y, |s| s.y),
                    )
                    .id_salt("result-rows")
                    .max_height(remaining)
                    .show_rows(ui, 24.0, self.document.result.rows.len(), |ui, range| {
                        page_range = range.clone();
                        #[cfg(test)]
                        {
                            self.ui.visible_rows = range
                                .clone()
                                .filter(|r| {
                                    (*r + 1) as f32 * (24.0 + ui.spacing().item_spacing.y)
                                        <= remaining
                                })
                                .count();
                        }
                        for row in range {
                            if let Some(selected) = self.grid_row(ui, row, editable) {
                                action = Some(selected);
                            }
                        }
                    });
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
            Some(GridAction::Follow(r, key)) => self.follow_key(r, key),
            Some(GridAction::Filter(filter)) => {
                self.add_filter(filter);
            }
            None => {}
        }
        if self.workspace.enabled {
            let start = self.document.browse.as_ref().map_or(0, |b| b.offset);
            let count = self.document.result.rows.len();
            let range = if count == 0 {
                "0 rows".into()
            } else {
                format!("{}–{} · {} retained", start + 1, start + count, count)
            };
            ui.small(format!("{range} · {}", self.export_scope()))
                .on_hover_text(
                    "Only retained rows are exported. Scroll to load another table page.",
                );
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
        if self.document.result.columns.is_empty() && self.document.active.is_none() {
            ui.label(
                "No retained row result. Browse a table, run SQL, or choose a read-only check.",
            );
        } else if self.document.result.rows.is_empty() && self.document.active.is_none() {
            ui.label("No rows in this page or lookup.");
        }
    }
    fn grid_header(&mut self, ui: &mut egui::Ui) {
        let mut sort = None;
        ui.horizontal(|ui|{
                ui.add_sized([44.0,30.0],egui::Label::new("#"));
                for i in self.visible_columns() {
                    let column=&self.document.result.columns[i];
                    let info=self.document.tools.inspection.as_ref().filter(|info|self.document.view==View::Browse&&self.document.table.as_ref().is_some_and(|t|t.name==info.table.name));
                    let pk=info.and_then(|info|info.table.columns.get(i)).map_or(0,|c|c.primary_key);
                    let indexed=info.is_some_and(|info|info.indexes.iter().any(|idx|idx.columns.first().and_then(Option::as_ref)==Some(&column.name)));
                    let foreign=info.is_some_and(|info|info.foreign_keys.iter().any(|fk|fk.columns.iter().any(|(name,_)|name==&column.name)));
                    let tags=format!("{}{}{}",if pk>0{format!(" PK{pk}")}else{String::new()},if indexed{" IX"}else{""},if foreign{" FK"}else{""});
                    let sorted=self.document.browse.as_ref().filter(|b|b.sort==Some(i)).map_or("",|b|if b.descending{" descending"}else{" ascending"});
                    let (rect,response)=ui.allocate_exact_size(egui::vec2(self.document.tools.columns.widths[i],30.0),egui::Sense::click_and_drag());
                    response.widget_info(||egui::WidgetInfo::labeled(egui::Role::Button,ui.is_enabled(),format!("Column {}{tags}{sorted}",column.name)));
                    let highlighted = self.document.tools.highlight_column.as_ref().is_some_and(|(table, name)| self.document.view == View::Browse && self.document.table.as_ref().is_some_and(|t| &t.name == table) && &column.name == name);
                    if highlighted && self.document.tools.scroll_to_column { ui.scroll_to_rect(rect, Some(egui::Align::Center)); self.document.tools.scroll_to_column = false; }
                    let painter=ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));painter.rect_filled(rect,2.0,if highlighted { ui.visuals().selection.bg_fill } else { ui.visuals().faint_bg_color });
                    let mut left=rect.left()+7.0;
                    for (show,icon) in [(pk>0,Icon::Key),(foreign,Icon::Link)] {
                        if show { icons::paint(&painter,egui::Rect::from_min_size(egui::pos2(left,rect.center().y-6.0),egui::vec2(12.0,12.0)),icon,if foreign{ui.visuals().hyperlink_color}else{ui.visuals().warn_fg_color});left+=15.0; }
                    }
                    painter.with_clip_rect(egui::Rect::from_min_max(rect.min,egui::pos2(rect.right()-if sorted.is_empty(){28.0}else{44.0},rect.bottom())).intersect(ui.clip_rect())).text(egui::pos2(left,rect.center().y),egui::Align2::LEFT_CENTER,&column.name,egui::FontId::proportional(12.0),ui.visuals().text_color());
                    if !sorted.is_empty(){icons::paint(&painter,egui::Rect::from_min_size(rect.right_top()+egui::vec2(-47.0,8.0),egui::vec2(12.0,12.0)),if self.document.browse.as_ref().is_some_and(|b|b.descending){Icon::Down}else{Icon::Up},ui.visuals().selection.stroke.color);}
                    let response=response.on_hover_text(format!("{} · {}\nPK = key position; IX = declared leading index; FK = foreign key. Index method/partial predicate may not apply. Drag the right edge or right-click to resize.",column.name,column.declared_type.as_deref().unwrap_or("runtime type")));
                    response.context_menu(|ui|{
                        if ui.button("Fit contents").clicked(){self.document.tools.columns.manual.remove(&i);self.document.tools.columns.refit=true;ui.close();}
                        if ui.button("Move left").clicked(){self.document.tools.columns.shift(i,-1);ui.close();}
                        if ui.button("Move right").clicked(){self.document.tools.columns.shift(i,1);ui.close();}
                        if ui.button("Hide column").clicked(){self.document.tools.columns.hidden.insert(i);ui.close();}
                    });
                    if response.drag_started(){response.dnd_set_drag_payload(i);}
                    if let Some(from)=response.dnd_release_payload::<usize>() {self.document.tools.columns.move_before(*from,i);}

                    let funnel_rect=egui::Rect::from_min_size(rect.right_top()+egui::vec2(-24.0,5.0),egui::vec2(17.0,20.0));
                    let funnel=ui.interact(funnel_rect,ui.scope_id().with(("column-filter",i)),egui::Sense::click());
                    icons::paint(ui.painter(),funnel_rect.shrink(2.0),Icon::Filter,ui.visuals().weak_text_color());
                    funnel.widget_info(||egui::WidgetInfo::labeled(egui::Role::Button,ui.is_enabled(),format!("Filter {}",column.name)));
                    egui::Popup::menu(&funnel).show(|ui|self.column_facets(ui,i));
                    let handle=egui::Rect::from_min_max(egui::pos2(rect.right()-6.0,rect.top()),rect.right_bottom());
                    let drag=ui.interact(handle,ui.scope_id().with(("resize-column",i)),egui::Sense::click_and_drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    if drag.double_clicked(){self.document.tools.columns.manual.remove(&i);self.document.tools.columns.refit=true;}
                    if drag.dragged(){self.document.tools.columns.manual.insert(i);self.document.tools.columns.widths[i]=(self.document.tools.columns.widths[i]+drag.drag_delta().x).clamp(60.0,640.0);}
                    if response.clicked()&&!drag.contains_pointer()&&!funnel.contains_pointer(){sort=Some(i);}
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
            if b.sort != Some(i) {
                b.sort = Some(i);
                b.descending = false;
            } else if !b.descending {
                b.descending = true;
            } else {
                b.sort = None;
                b.descending = false;
            }
            b.offset = 0;
            self.reload();
        }
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
            ui.add_sized(
                [44.0, 24.0],
                egui::Label::new(
                    egui::RichText::new(format!("{marker}{}", offset + row + 1))
                        .monospace()
                        .small(),
                ),
            );
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
        let references = self.cell_references(row, col);
        let staged = self
            .document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(values, col));
        let displayed = staged.unwrap_or(value);
        let text = match displayed {
            Value::Null => self.preferences.null_display.clone(),
            Value::Text(s) if s.is_empty() => "(empty text)".into(),
            Value::Real(n) => format!("{n:?}"),
            _ => displayed.preview(),
        };
        let fill = cell_fill(ui.visuals(), deleted, staged.is_some(), row);
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(self.document.tools.columns.widths[col], 24.0),
            egui::Sense::click_and_drag(),
        );
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::Cell,
                ui.is_enabled(),
                self.cell_selected(row, col),
                format!(
                    "Row {} · {}: {}{text}",
                    offset + row + 1,
                    self.document.result.columns[col].name,
                    if deleted {
                        "Deletion staged · "
                    } else if staged.is_some() {
                        "Staged · "
                    } else {
                        ""
                    }
                ),
            )
        });
        if let Some(edit) = self
            .document
            .tools
            .inline
            .as_mut()
            .filter(|e| e.row == row && e.column == col)
        {
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
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        let selected = self.cell_selected(row, col);
        painter.rect_filled(
            rect,
            0.0,
            if selected {
                ui.visuals().selection.bg_fill
            } else {
                fill
            },
        );
        if self.document.inspect == Some((row, col)) || response.has_focus() {
            painter.rect_stroke(
                rect,
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let numeric = matches!(displayed, Value::Integer(_) | Value::Real(_));
        let color = if !references.is_empty() {
            ui.visuals().hyperlink_color
        } else if matches!(displayed, Value::Null) {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().text_color()
        };
        let mut job =
            egui::text::LayoutJob::simple_singleline(text, egui::FontId::monospace(12.0), color);
        job.wrap.max_width = (rect.width() - 14.0).max(1.0);
        job.wrap.max_rows = 1;
        job.wrap.break_anywhere = true;
        job.wrap.overflow_character = Some('…');
        let galley = ui.fonts_mut(|fonts| fonts.layout_job(job));
        let position = if numeric {
            rect.right_center() - egui::vec2(7.0 + galley.size().x, galley.size().y * 0.5)
        } else {
            rect.left_center() + egui::vec2(7.0, -galley.size().y * 0.5)
        };
        painter.galley(position, galley, color);
        if !references.is_empty() {
            painter.line_segment(
                [
                    rect.left_bottom() + egui::vec2(7.0, -3.0),
                    rect.right_bottom() + egui::vec2(-7.0, -3.0),
                ],
                egui::Stroke::new(0.5, color),
            );
            #[cfg(test)]
            {
                self.fk_cell_rect = Some(rect);
            }
        }
        if deleted {
            painter.line_segment(
                [
                    rect.left_center() + egui::vec2(5.0, 0.0),
                    rect.right_center() - egui::vec2(5.0, 0.0),
                ],
                egui::Stroke::new(1.0, ui.visuals().error_fg_color),
            );
        }
        if staged.is_some() {
            painter.circle_filled(
                rect.left_top() + egui::vec2(3.0, 3.0),
                2.0,
                ui.visuals().warn_fg_color,
            );
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
                    GridAction::Follow(row, references[0].clone())
                } else {
                    GridAction::Cell(row, col)
                },
            );
        }
        if response.double_clicked() {
            action = Some(GridAction::Edit(row, col));
        }
        if response.hovered() && references.len() == 1 {
            self.document.tools.context.hovered = Some((row, references[0].clone()));
        }
        let response = if references.is_empty() {
            response
        } else {
            response
                .on_hover_cursor(egui::CursorIcon::PointingHand)
                .on_hover_ui(|ui| {
                    if references.len() == 1 {
                        self.peek_view(ui, row, &references[0]);
                    } else {
                        ui.label("Multiple references: right-click to choose");
                    }
                })
        };
        response.context_menu(|ui| {
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
            for key in &references {
                if ui
                    .button(format!("Parent: {} (FK {})", key.target, key.id))
                    .clicked()
                {
                    action = Some(GridAction::Follow(row, key.clone()));
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
        });
        action
    }
    pub(super) fn cell_references(&self, row: usize, col: usize) -> Vec<ForeignKey> {
        if self.document.view != View::Browse {
            return vec![];
        }
        let (Some(table), Some(info), Some(values)) = (
            &self.document.table,
            &self.document.tools.inspection,
            self.document.result.rows.get(row),
        ) else {
            return vec![];
        };
        if table.name != info.table.name {
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
            .cloned()
            .collect()
    }
}

fn cell_fill(visuals: &egui::Visuals, deleted: bool, staged: bool, row: usize) -> egui::Color32 {
    match (deleted, staged, visuals.dark_mode) {
        (true, _, true) => egui::Color32::from_rgb(61, 28, 27),
        (true, _, false) => egui::Color32::from_rgb(246, 218, 216),
        (false, true, true) => egui::Color32::from_rgb(58, 42, 18),
        (false, true, false) => egui::Color32::from_rgb(249, 232, 201),
        _ if row.is_multiple_of(2) => visuals.faint_bg_color,
        _ => visuals.extreme_bg_color,
    }
}
