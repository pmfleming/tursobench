use super::presentation::{self, Intent};
use super::{App, View};
use eframe::egui;
use tursobench_core::{Delivery, DraftBatch, Edit, Table, TransactionState, Value};
use tursobench_db::{Operation, Request};

impl App {
    pub(super) fn has_drafts(&self) -> bool {
        self.document.has_drafts() || self.documents.values().any(|d| d.has_drafts())
    }
    pub(super) fn can_edit(&self) -> bool {
        !self
            .documents
            .values()
            .any(|d| d.has_drafts() || d.active.is_some())
            && self.writable
            && self.ready
            && !self.closing
            && !self.uncertain_edit
            && self.transaction == TransactionState::Autocommit
            && self.commit_ticket.is_none()
            && self.document.active.is_none()
            && self.document.tools.pages.pending.is_none()
            && self.document.view == View::Browse
            && self.document.table.as_ref().is_some_and(Table::editable)
            && self.document.drafts.as_ref().is_none_or(|d| {
                d.edits.is_empty() || Some(&d.table) == self.document.table.as_ref()
            })
            && self
                .document
                .result
                .completion
                .as_ref()
                .is_some_and(|c| c.error.is_none() && c.delivery == Delivery::Complete)
    }
    fn stage(&mut self, f: impl FnOnce(&mut DraftBatch)) {
        if !self.can_edit() {
            return;
        }
        let Some(table) = &self.document.table else {
            return;
        };
        // Reverting the final operation must not carry the old table identity
        // into a subsequent edit after navigating to another table.
        let mut draft = self
            .document
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
                self.document.drafts = Some(draft);
                self.status = "Staged locally. Preview and explicitly commit to write.".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    pub(super) fn stage_cell(&mut self, row: usize, column: usize, value: Value) {
        if let Some(original) = self.document.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_cell(original, column, value));
        }
    }
    pub(super) fn stage_delete(&mut self, row: usize) {
        if let Some(original) = self.document.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_delete(original));
        }
    }
    pub(super) fn clone_row(&mut self, row: usize) {
        if !self.can_edit() {
            return;
        }
        if let (Some(table), Some(row)) = (&self.document.table, self.document.result.rows.get(row))
        {
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
                self.document.insert_json = json;
                self.document.show_insert = true;
            }
            Err(error) => self.status = error.to_string(),
        }
    }
    pub(super) fn begin_insert(&mut self) {
        if self.can_edit()
            && let Some(table) = &self.document.table
        {
            let json = serde_json::to_string_pretty(&vec![None::<&Value>; table.columns.len()]);
            self.prepare_insert(json);
        }
    }
    pub(super) fn edit_windows(&mut self, ctx: &egui::Context) {
        if self.document.show_insert {
            let mut open = true;
            egui::Window::new("Stage insert — typed column values").frame(crate::app::presentation::window_frame(&ctx))
                .open(&mut open).default_width(580.0).max_width((ctx.content_rect().width()-40.0).max(280.0))
                .show(ctx, |ui| {
                    presentation::notice(ui,super::icons::Icon::Plus,Intent::Pending,"One entry per column. JSON null = omit/use DEFAULT; {\"type\":\"Null\"} = SQL NULL.");
                    egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-230.0).clamp(120.0,400.0)).show(ui,|ui| {
                        if let Some(t) = &self.document.table { ui.label(t.columns.iter().map(|c| c.name.as_str()).collect::<Vec<_>>().join(", ")); }
                        ui.add(egui::TextEdit::multiline(&mut self.document.insert_json).code_editor().desired_width(ui.available_width()).desired_rows(8));
                    });
                    if presentation::footer(ui, |ui| ui.add_enabled(self.can_edit(),presentation::action(ui,"Stage insert (no database write)",Intent::Pending))).inner.clicked() {
                        match serde_json::from_str::<Vec<Option<Value>>>(&self.document.insert_json) {
                            Ok(values) => self.stage(|d| d.edits.push(Edit::Insert { values })),
                            Err(e) => self.status = e.to_string(),
                        }
                    }
                    ui.label(&self.status);
                });
            self.document.show_insert = open;
        }
        if self.document.show_preview {
            let mut open = true;
            egui::Window::new("Review staged atomic transaction").frame(crate::app::presentation::window_frame(&ctx))
                .open(&mut open).default_width(760.0).max_width((ctx.content_rect().width()-40.0).max(280.0))
                .show(ctx, |ui| {
                    presentation::notice(ui,super::icons::Icon::ReviewEdits,Intent::Pending,"Dedicated connection · foreign keys enabled · original values checked · atomic batch. Only a confirmed commit clears these drafts.");
                    let mut remove = None;
                    if let Some(draft) = &self.document.drafts {
                        ui.strong(format!("{} · {} staged operations",draft.table.name,draft.edits.len()));
                        match draft.preview() {
                            Ok(statements) => {
                                egui::ScrollArea::vertical().max_height((ctx.content_rect().height()-260.0).clamp(120.0,480.0)).show(ui,|ui| {
                                    for (i, statement) in statements.iter().enumerate() {
                                        ui.horizontal_wrapped(|ui| {
                                            ui.strong(format!("Operation {}",i+1));
                                            if ui.add_enabled(self.commit_ticket.is_none(),egui::Button::new("Remove operation")).clicked() { remove = Some(i); }
                                        });
                                        edit_diff(ui,&draft.table,&draft.edits[i]);
                                        let mut sql = statement.sql.as_str();
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
                    if let Some(index) = remove && let Some(draft) = &mut self.document.drafts { draft.edits.remove(index); }
                    if self.uncertain_edit { ui.colored_label(ui.visuals().error_fg_color,"UNKNOWN outcome — close/reopen and inspect; never automatically retry."); }
                    if self.commit_ticket.is_some() { ui.label("Committing — awaiting the actual outcome…"); }
                    if presentation::footer(ui, |ui| ui.add_enabled(self.can_edit() && self.has_drafts(),presentation::action(ui,"COMMIT staged edits to database",Intent::Pending))).inner.clicked() { self.commit_drafts(); }
                    ui.label(&self.status);
                });
            self.document.show_preview = open;
        }
    }
    pub(super) fn staged_edit_dock(&mut self, ui: &mut egui::Ui) {
        let count = self.document.drafts.as_ref().map_or(0, |d| d.edits.len());
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        egui::Frame::new()
            .fill(tokens.pending.gamma_multiply(0.08))
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.colored_label(tokens.pending, format!("{count} staged"));
                    if ui
                        .add_enabled(
                            self.can_edit() && count > 0,
                            egui::Button::new(egui::RichText::new("Commit").color(tokens.pending))
                                .fill(tokens.pending.gamma_multiply(0.13))
                                .stroke(egui::Stroke::new(
                                    1.0,
                                    tokens.pending.gamma_multiply(0.48),
                                )),
                        )
                        .clicked()
                    {
                        self.commit_drafts();
                    }
                    if ui.button("Diff / SQL").clicked() {
                        self.document.show_preview = true;
                    }
                    if ui
                        .add_enabled(
                            self.commit_ticket.is_none() && !self.uncertain_edit,
                            egui::Button::new("Discard"),
                        )
                        .clicked()
                    {
                        self.document.drafts = None;
                    }
                    if self.commit_ticket.is_some() {
                        ui.small("Committing…");
                    }
                    if self.uncertain_edit {
                        ui.colored_label(
                            ui.visuals().error_fg_color,
                            "Outcome unknown — inspect before retrying",
                        );
                    }
                });
            });
    }
    pub(super) fn commit_drafts(&mut self) {
        if !self.can_edit() || !self.has_drafts() {
            return;
        }
        let (Some(s), Some(batch)) = (&self.session, &self.document.drafts) else {
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
                self.commit_owner = Some(self.document.id);
                self.status = "Committing staged edits… do not retry until outcome is known".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
}

fn edit_diff(ui: &mut egui::Ui, table: &tursobench_core::Table, edit: &Edit) {
    let mut field = |column: usize, before: Option<&Value>, after: Option<&Value>| {
        let Some(column) = table.columns.get(column) else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(&column.name);
            if let Some(value) = before {
                ui.label(
                    egui::RichText::new(value.preview())
                        .strikethrough()
                        .color(ui.visuals().error_fg_color),
                );
            }
            ui.label("->");
            ui.colored_label(
                crate::appearance::Tokens::of(ui.ctx()).pending,
                after.map_or_else(|| "DEFAULT / removed".into(), Value::preview),
            );
        });
    };
    match edit {
        Edit::Update { original, changes } => {
            for (column, value) in changes {
                field(*column, original.get(*column), Some(value));
            }
        }
        Edit::Insert { values } => {
            for (column, value) in values.iter().enumerate() {
                field(column, None, value.as_ref());
            }
        }
        Edit::Delete { original } => {
            for (column, value) in original.iter().enumerate() {
                field(column, Some(value), None);
            }
        }
    }
}
