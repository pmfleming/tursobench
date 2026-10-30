use super::*;
impl App {
    pub(super) fn has_drafts(&self) -> bool {
        self.drafts.as_ref().is_some_and(|d| !d.edits.is_empty())
    }
    pub(super) fn can_edit(&self) -> bool {
        self.writable
            && self.ready
            && !self.closing
            && !self.uncertain_edit
            && self.commit_ticket.is_none()
            && self.active.is_none()
            && self.view == View::Browse
            && self.table.as_ref().is_some_and(Table::editable)
            && self
                .result
                .completion
                .as_ref()
                .is_some_and(|c| c.error.is_none() && c.delivery == Delivery::Complete)
    }
    fn stage(&mut self, f: impl FnOnce(&mut DraftBatch)) {
        if !self.can_edit() {
            return;
        }
        let mut draft = self.drafts.clone().unwrap_or_else(|| DraftBatch {
            table: self.table.clone().unwrap(),
            edits: vec![],
        });
        f(&mut draft);
        match draft.preview() {
            Ok(_) => {
                self.drafts = Some(draft);
                self.status = "Staged locally. Preview and explicitly commit to write.".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub(super) fn stage_cell(&mut self, row: usize, column: usize) {
        match serde_json::from_str::<Value>(&self.cell_json) {
            Ok(value) => {
                if let Some(original) = self.result.rows.get(row).cloned() {
                    self.stage(|d| d.stage_cell(original, column, value));
                }
            }
            Err(e) => self.status = format!("Invalid typed cell JSON: {e}"),
        }
    }
    pub(super) fn stage_delete(&mut self, row: usize) {
        if let Some(original) = self.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_delete(original));
        }
    }
    pub(super) fn edit_controls(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.can_edit(), egui::Button::new("Stage insert…"))
                .clicked()
            {
                self.insert_json =
                    serde_json::to_string_pretty(&vec![
                        None::<Value>;
                        self.table.as_ref().unwrap().columns.len()
                    ])
                    .unwrap();
                self.show_insert = true;
            }
            ui.label(format!(
                "{} staged operations",
                self.drafts.as_ref().map_or(0, |d| d.edits.len())
            ));
            if ui
                .add_enabled(
                    self.has_drafts() && self.commit_ticket.is_none(),
                    egui::Button::new("Preview edits…"),
                )
                .clicked()
            {
                self.show_preview = true;
            }
            if ui
                .add_enabled(
                    self.has_drafts() && self.commit_ticket.is_none(),
                    egui::Button::new("Discard drafts"),
                )
                .clicked()
            {
                self.drafts = None;
                self.show_preview = false;
                self.status = "Local drafts discarded; no database write".into();
            }
        });
        if self.view == View::Browse
            && self.writable
            && !self.table.as_ref().is_some_and(Table::editable)
        {
            ui.small("Read-only table result: editing requires a base table with a primary key and supported columns.");
        }
    }
    pub(super) fn edit_windows(&mut self, ctx: &egui::Context) {
        if self.show_insert {
            let mut open = true;
            egui::Window::new("Stage insert — typed column values").open(&mut open).show(ctx, |ui| {
                if let Some(t) = &self.table { ui.label(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")); }
                ui.label("One entry per column. JSON null = omit/use DEFAULT; {\"type\":\"Null\"} = SQL NULL.");
                ui.text_edit_multiline(&mut self.insert_json);
                if ui.button("Stage insert (no write)").clicked() {
                    match serde_json::from_str::<Vec<Option<Value>>>(&self.insert_json) {
                        Ok(values) => self.stage(|d| d.edits.push(Edit::Insert { values })),
                        Err(e) => self.status = e.to_string(),
                    }
                }
            });
            self.show_insert = open;
        }
        if self.show_preview {
            let mut open = true;
            egui::Window::new("Preview staged atomic transaction").open(&mut open).default_size([700.0, 400.0]).show(ctx, |ui| {
                ui.label("Operations run in the listed order on a dedicated connection, with foreign keys enabled. Original values are checked; any conflict rolls back the batch.");
                if let Some(draft) = &self.drafts {
                    match draft.preview() {
                        Ok(statements) => { egui::ScrollArea::both().max_height(300.0).show(ui, |ui| {
                            for (i, statement) in statements.iter().enumerate() {
                                ui.label(format!("{}. {}", i + 1, statement.sql));
                                let mut parameters = serde_json::to_string(&statement.parameters).unwrap_or_default();
                                ui.add(egui::TextEdit::multiline(&mut parameters).interactive(false).desired_rows(2));
                            }
                        }); }
                        Err(e) => { ui.label(e.to_string()); }
                    }
                }
                if ui.add_enabled(self.can_edit() && self.has_drafts(), egui::Button::new("COMMIT staged edits to database")).clicked() { self.commit_drafts(); }
            });
            self.show_preview = open;
        }
    }
    pub(super) fn commit_drafts(&mut self) {
        if !self.can_edit() || !self.has_drafts() {
            return;
        }
        let Some(s) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = s.ticket(self.sequence, self.generation);
        match s.submit(Request {
            ticket,
            operation: Operation::CommitEdits(self.drafts.clone().unwrap()),
        }) {
            Ok(()) => {
                self.commit_ticket = Some(ticket);
                self.status = "Committing staged edits… do not retry until outcome is known".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
}
