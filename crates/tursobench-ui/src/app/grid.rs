use super::{App, View, shell};
use eframe::egui;
use tursobench_core::{ForeignKey, Value};

enum GridAction {
    Cell(usize, usize),
    Record(usize),
    Copy(usize),
    Delete(usize),
    Follow(usize, ForeignKey),
}
impl App {
    pub(super) fn grid_contents(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        {
            self.fk_cell_rect = None;
        }
        let mut action = None;
        let editable = self.can_edit();
        let remaining = (ui.clip_rect().bottom() - ui.cursor().top() - 65.0).clamp(140.0, 900.0);
        self.tools.widths.resize(self.result.columns.len(), 180.0);
        if self.view == View::Explain
            && let Some(detail) = self
                .result
                .columns
                .iter()
                .position(|c| c.name.eq_ignore_ascii_case("detail"))
        {
            let scans=self.result.rows.iter().filter(|r|matches!(r.get(detail),Some(Value::Text(s))if s.to_uppercase().contains("SCAN "))).count();
            ui.label(format!("Planner notes: {scans} scan entries. Scans may be intentional; not a runtime profile."));
        }
        egui::ScrollArea::horizontal()
            .id_salt("result-columns")
            .show(ui, |ui| {
                self.grid_header(ui);
                egui::ScrollArea::vertical()
                    .id_salt("result-rows")
                    .max_height(remaining)
                    .show_rows(ui, 24.0, self.result.rows.len(), |ui, range| {
                        for row in range {
                            if let Some(selected) = self.grid_row(ui, row, editable) {
                                action = Some(selected);
                            }
                        }
                    });
            });
        match action {
            Some(GridAction::Cell(r, c)) => self.open_cell(r, c),
            Some(GridAction::Record(r)) => {
                self.tools.detail = Some(r);
                self.ui.inspector = shell::Inspector::Record;
                self.ui.inspector_open = true;
            }
            Some(GridAction::Copy(r)) => self.copy_row(ui.ctx(), r),
            Some(GridAction::Delete(r)) => self.stage_delete(r),
            Some(GridAction::Follow(r, key)) => self.follow_key(r, key),
            None => {}
        }
        if self.result.columns.is_empty() && self.active.is_none() {
            ui.label(
                "No retained row result. Browse a table, run SQL, or choose a read-only check.",
            );
        } else if self.result.rows.is_empty() && self.active.is_none() {
            ui.label("No rows in this page or lookup.");
        }
    }
    fn grid_header(&mut self, ui: &mut egui::Ui) {
        let mut sort = None;
        ui.horizontal(|ui|{
                ui.add_sized([44.0,42.0],egui::Label::new("#"));
                for(i,column)in self.result.columns.iter().enumerate(){
                    let info=self.tools.inspection.as_ref().filter(|info|self.view==View::Browse&&self.table.as_ref().is_some_and(|t|t.name==info.table.name));
                    let pk=info.and_then(|info|info.table.columns.get(i)).map_or(0,|c|c.primary_key);
                    let indexed=info.is_some_and(|info|info.indexes.iter().any(|idx|idx.columns.first().and_then(Option::as_ref)==Some(&column.name)));
                    let foreign=info.is_some_and(|info|info.foreign_keys.iter().any(|fk|fk.columns.iter().any(|(name,_)|name==&column.name)));
                    let tags=format!("{}{}{}",if pk>0{format!(" PK{pk}")}else{String::new()},if indexed{" IX"}else{""},if foreign{" FK"}else{""});
                    let sorted=self.browse.as_ref().filter(|b|b.sort==Some(i)).map_or("",|b|if b.descending{" ↓"}else{" ↑"});
                    let (rect,response)=ui.allocate_exact_size(egui::vec2(self.tools.widths[i],42.0),egui::Sense::click());
                    response.widget_info(||egui::WidgetInfo::labeled(egui::WidgetType::Button,ui.is_enabled(),format!("Column {}{tags}{sorted}",column.name)));
                    let highlighted = self.tools.highlight_column.as_ref().is_some_and(|(table, name)| self.view == View::Browse && self.table.as_ref().is_some_and(|t| &t.name == table) && &column.name == name);
                    let painter=ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));painter.rect_filled(rect,2.0,if highlighted { ui.visuals().selection.bg_fill } else { ui.visuals().faint_bg_color });
                    painter.text(rect.left_top()+egui::vec2(7.0,4.0),egui::Align2::LEFT_TOP,format!("{}{tags}{sorted}",column.name),egui::FontId::monospace(12.0),ui.visuals().text_color());
                    painter.text(rect.left_bottom()+egui::vec2(7.0,-5.0),egui::Align2::LEFT_BOTTOM,column.declared_type.as_deref().unwrap_or("runtime type"),egui::FontId::proportional(11.0),ui.visuals().weak_text_color());
                    let response=response.on_hover_text(format!("{}\nPK = key position; IX = declared leading index; FK = foreign key. Index method/partial predicate may not apply. Drag the right edge or right-click to resize.",column.name));
                    response.context_menu(|ui|{ui.label("Column width");ui.add(egui::Slider::new(&mut self.tools.widths[i],60.0..=640.0));});
                    let handle=egui::Rect::from_min_max(egui::pos2(rect.right()-6.0,rect.top()),rect.right_bottom());
                    let drag=ui.interact(handle,ui.id().with(("resize-column",i)),egui::Sense::drag()).on_hover_cursor(egui::CursorIcon::ResizeHorizontal);
                    if drag.dragged(){self.tools.widths[i]=(self.tools.widths[i]+drag.drag_delta().x).clamp(60.0,640.0);}
                    if response.clicked()&&!drag.dragged(){sort=Some(i);}
                }
            });

        if let Some(i) = sort
            && self.view == View::Browse
            && self.ready
            && !self.closing
            && !self.has_drafts()
            && self.commit_ticket.is_none()
            && let Some(b) = self.browse.as_mut()
        {
            b.descending = b.sort == Some(i) && !b.descending;
            b.sort = Some(i);
            b.offset = 0;
            self.reload();
        }
    }
    fn grid_row(&mut self, ui: &mut egui::Ui, row: usize, editable: bool) -> Option<GridAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            let values = &self.result.rows[row];
            let deleted = self.drafts.as_ref().is_some_and(|d| d.deleted(values));
            let modified = self
                .drafts
                .as_ref()
                .is_some_and(|d| (0..values.len()).any(|c| d.displayed(values, c).is_some()));
            let offset = self.browse.as_ref().map_or(0, |b| b.offset);
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
            for (col, value) in values.iter().enumerate() {
                let references = self.cell_references(row, col);
                let staged = self.drafts.as_ref().and_then(|d| d.displayed(values, col));
                let displayed = staged.unwrap_or(value);
                let text = match displayed {
                    Value::Null => self.preferences.null_display.clone(),
                    Value::Text(s) if s.is_empty() => "(empty text)".into(),
                    Value::Real(n) => format!("{n:?}"),
                    _ => displayed.preview(),
                };
                let fill = cell_fill(ui.visuals(), deleted, staged.is_some(), row);
                let (rect, response) = ui.allocate_exact_size(
                    egui::vec2(self.tools.widths[col], 24.0),
                    egui::Sense::click(),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::WidgetType::SelectableLabel,
                        ui.is_enabled(),
                        self.inspect == Some((row, col)),
                        format!(
                            "Row {} · {}: {}{text}",
                            offset + row + 1,
                            self.result.columns[col].name,
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
                let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
                painter.rect_filled(rect, 0.0, fill);
                if self.inspect == Some((row, col)) || response.has_focus() {
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
                let mut job = egui::text::LayoutJob::simple_singleline(
                    text,
                    egui::FontId::monospace(12.0),
                    color,
                );
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
                if response.clicked() {
                    action = Some(if references.len() == 1 {
                        GridAction::Follow(row, references[0].clone())
                    } else {
                        GridAction::Cell(row, col)
                    });
                }
                if response.double_clicked() {
                    action = Some(GridAction::Record(row));
                }
                let response = if references.is_empty() {
                    response
                } else {
                    response
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .on_hover_text(if references.len() == 1 {
                            format!(
                                "Follow reference to {} · right-click to inspect or edit",
                                references[0].target
                            )
                        } else {
                            "Multiple references: right-click to choose a target".into()
                        })
                };
                response.context_menu(|ui| {
                    for key in &references {
                        if ui
                            .button(format!("Follow → {} (FK {})", key.target, key.id))
                            .clicked()
                        {
                            action = Some(GridAction::Follow(row, key.clone()));
                            ui.close();
                        }
                    }
                    if ui.button("Inspect / edit cell").clicked() {
                        action = Some(GridAction::Cell(row, col));
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
                    if ui
                        .add_enabled(editable, egui::Button::new("Stage row deletion"))
                        .clicked()
                    {
                        action = Some(GridAction::Delete(row));
                        ui.close();
                    }
                });
            }
        });
        action
    }
    pub(super) fn cell_references(&self, row: usize, col: usize) -> Vec<ForeignKey> {
        if self.view != View::Browse {
            return vec![];
        }
        let (Some(table), Some(info), Some(values)) = (
            &self.table,
            &self.tools.inspection,
            self.result.rows.get(row),
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
