use super::{App, View};
use eframe::egui;
use tursobench_core::{Delivery, DraftBatch, Edit, Table, TransactionState, Value};
use tursobench_db::{Operation, Request};

impl App {
    pub(super) fn has_drafts(&self) -> bool {
        self.drafts.as_ref().is_some_and(|d| !d.edits.is_empty())
    }
    pub(super) fn can_edit(&self) -> bool {
        self.writable
            && self.ready
            && !self.closing
            && !self.uncertain_edit
            && self.transaction == TransactionState::Autocommit
            && self.commit_ticket.is_none()
            && self.active.is_none()
            && self.view == View::Browse
            && self.table.as_ref().is_some_and(Table::editable)
            && self
                .drafts
                .as_ref()
                .is_none_or(|d| d.edits.is_empty() || Some(&d.table) == self.table.as_ref())
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
        let Some(table) = &self.table else {
            return;
        };
        // Reverting the final operation must not carry the old table identity
        // into a subsequent edit after navigating to another table.
        let mut draft = self
            .drafts
            .as_ref()
            .filter(|d| !d.edits.is_empty())
            .cloned()
            .unwrap_or_else(|| DraftBatch {
                table: table.clone(),
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
    pub(super) fn stage_cell(&mut self, row: usize, column: usize, value: Value) {
        if let Some(original) = self.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_cell(original, column, value));
        }
    }
    pub(super) fn stage_delete(&mut self, row: usize) {
        if let Some(original) = self.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_delete(original));
        }
    }
    pub(super) fn clone_row(&mut self, row: usize) {
        if !self.can_edit() {
            return;
        }
        if let (Some(table), Some(row)) = (&self.table, self.result.rows.get(row)) {
            let values: Vec<Option<&Value>> = table
                .columns
                .iter()
                .zip(row)
                .map(|(c, v)| (c.primary_key <= 0).then_some(v))
                .collect();
            let json = serde_json::to_string_pretty(&values);
            if json.is_ok() {
                self.status = "Clone prepared locally; primary keys omitted. Supply any required keys before staging.".into();
            }
            self.prepare_insert(json);
        }
    }
    fn prepare_insert(&mut self, json: Result<String, serde_json::Error>) {
        match json {
            Ok(json) => {
                self.insert_json = json;
                self.show_insert = true;
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    pub(super) fn edit_controls(&mut self, ui: &mut egui::Ui) {
        if !self.writable && !self.has_drafts() {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(self.can_edit(), egui::Button::new("Stage insert…"))
                .clicked()
                && let Some(table) = &self.table
            {
                let json = serde_json::to_string_pretty(&vec![None::<&Value>; table.columns.len()]);
                self.prepare_insert(json);
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
        if self.writable && self.transaction != TransactionState::Autocommit {
            ui.small("Finish the SQL-editor transaction before staging changes. Staged commits use a separate connection.");
        }
        if self.view == View::Browse
            && self.writable
            && !self.table.as_ref().is_some_and(Table::editable)
        {
            ui.small("Read-only table result: editing requires a non-engine-managed base table with a primary key and supported columns.");
        }
    }
    pub(super) fn edit_windows(&mut self, ctx: &egui::Context) {
        if self.show_insert {
            let mut open = true;
            egui::Window::new("Stage insert — typed column values")
                .open(&mut open).default_width(580.0).max_width((ctx.content_rect().width()-40.0).max(280.0))
                .show(ctx, |ui| {
                    ui.label("One entry per column. JSON null = omit/use DEFAULT; {\"type\":\"Null\"} = SQL NULL.");
                    egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-230.0).clamp(120.0,400.0)).show(ui,|ui| {
                        if let Some(t) = &self.table { ui.label(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")); }
                        ui.add(egui::TextEdit::multiline(&mut self.insert_json).code_editor().desired_width(ui.available_width()).desired_rows(8));
                    });
                    if ui.add_enabled(self.can_edit(),egui::Button::new("Stage insert (no database write)")).clicked() {
                        match serde_json::from_str::<Vec<Option<Value>>>(&self.insert_json) {
                            Ok(values) => self.stage(|d| d.edits.push(Edit::Insert { values })),
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    ui.label(&self.status);
                });
            self.show_insert = open;
        }
        if self.show_preview {
            let mut open = true;
            egui::Window::new("Review staged atomic transaction")
                .open(&mut open).default_width(760.0).max_width((ctx.content_rect().width()-40.0).max(280.0))
                .show(ctx, |ui| {
                    ui.label("Dedicated connection · foreign keys enabled · original values checked · atomic batch. Only a confirmed commit clears these drafts.");
                    let mut remove = None;
                    if let Some(draft) = &self.drafts {
                        ui.strong(format!("{} · {} staged operations",draft.table.name,draft.edits.len()));
                        match draft.preview() {
                            Ok(statements) => {
                                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-260.0).clamp(120.0,480.0)).show(ui,|ui| {
                                    for (i, statement) in statements.iter().enumerate() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.strong(format!("Operation {}",i+1));
                                            if ui.add_enabled(self.commit_ticket.is_none(),egui::Button::new("Remove operation")).clicked() { remove = Some(i); }
                                        });
                                        let mut sql = statement.sql.clone();
                                        ui.add(egui::TextEdit::multiline(&mut sql).code_editor().interactive(false).desired_width(ui.available_width()).desired_rows(2));
                                        ui.small("Exact typed bindings (including original-value predicates):");
                                        let mut parameters=serde_json::to_string(&statement.parameters).unwrap_or_default();
                                        ui.add(egui::TextEdit::multiline(&mut parameters).code_editor().interactive(false).desired_width(ui.available_width()).desired_rows(2));
                                        ui.separator();
                                    }
                                });
                            }
                            Err(error) => { ui.colored_label(ui.visuals().error_fg_color,error.to_string()); }
                        }
                    }
                    if let Some(index) = remove && let Some(draft) = &mut self.drafts { draft.edits.remove(index); }
                    if self.uncertain_edit { ui.colored_label(ui.visuals().error_fg_color,"UNKNOWN outcome — close/reopen and inspect; never automatically retry."); }
                    if self.commit_ticket.is_some() { ui.label("Committing — awaiting the actual outcome…"); }
                    if ui.add_enabled(self.can_edit() && self.has_drafts(),egui::Button::new("COMMIT staged edits to database")).clicked() { self.commit_drafts(); }
                    ui.label(&self.status);
                });
            self.show_preview = open;
        }
    }
    pub(super) fn commit_drafts(&mut self) {
        if !self.can_edit() || !self.has_drafts() {
            return;
        }
        let (Some(s), Some(batch)) = (&self.session, &self.drafts) else {
            return;
        };
        self.sequence += 1;
        let ticket = s.ticket(self.sequence, self.generation);
        match s.submit(Request {
            ticket,
            operation: Operation::CommitEdits(batch.clone()),
        }) {
            Ok(()) => {
                self.commit_ticket = Some(ticket);
                self.status = "Committing staged edits… do not retry until outcome is known".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
}
