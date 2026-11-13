use super::icons::{self, Icon};
use super::presentation::{self, Intent};
use super::{App, View};
use eframe::egui;
use tursobench_core::{Delivery, DraftBatch, Edit, Table, TransactionState, Value};
use tursobench_db::{Operation, Request};

#[derive(Clone, Copy, PartialEq)]
enum InsertMode {
    Default,
    Null,
    Value,
}
pub(super) struct InsertField {
    mode: InsertMode,
    editor: super::results::CellEditor,
}
impl InsertField {
    fn new(column: &tursobench_core::TableColumn, value: Option<&Value>) -> Self {
        let mode = match value {
            None => InsertMode::Default,
            Some(Value::Null) => InsertMode::Null,
            _ => InsertMode::Value,
        };
        let fallback;
        let initial = if let Some(value) = value {
            value
        } else {
            fallback = {
                let ty = column.declared_type.to_uppercase();
                if ty.contains("INT") {
                    Value::Integer(0)
                } else if ty.contains("REAL") {
                    Value::Real(0.0)
                } else if ty.contains("BLOB") {
                    Value::Blob(vec![])
                } else {
                    Value::Text(String::new())
                }
            };
            &fallback
        };
        Self {
            mode,
            editor: super::results::CellEditor::new(initial),
        }
    }
    fn value(&self) -> Result<Option<Value>, String> {
        match self.mode {
            InsertMode::Default => Ok(None),
            InsertMode::Null => Ok(Some(Value::Null)),
            InsertMode::Value => self.editor.parse().map(Some),
        }
    }
    fn show(&mut self, ui: &mut egui::Ui, index: usize, column: &tursobench_core::TableColumn) {
        ui.group(|ui| {
            ui.label(format!("{} · {}", column.name, column.declared_type));
            egui::ComboBox::from_id_salt(("insert-mode", index))
                .selected_text(match self.mode {
                    InsertMode::Default => "Default / omit",
                    InsertMode::Null => "NULL",
                    InsertMode::Value => "Value",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut self.mode, InsertMode::Default, "Default / omit");
                    ui.selectable_value(&mut self.mode, InsertMode::Null, "NULL");
                    ui.selectable_value(&mut self.mode, InsertMode::Value, "Value");
                });
            if self.mode == InsertMode::Value {
                self.editor.type_picker(ui, ("insert-type", index), true);
                ui.text_edit_singleline(&mut self.editor.input);
            }
        });
    }
}
struct PreparedInsert {
    json: String,
    fields: Vec<InsertField>,
}
impl PreparedInsert {
    fn new(table: &Table, values: Vec<Option<&Value>>) -> Result<Self, serde_json::Error> {
        Ok(Self {
            json: serde_json::to_string_pretty(&values)?,
            fields: table
                .columns
                .iter()
                .zip(values)
                .map(|(column, value)| InsertField::new(column, value))
                .collect(),
        })
    }
}
#[derive(Clone, Copy)]
struct CellChange {
    edit: usize,
    change: usize,
}
#[derive(Default)]
pub(super) struct RowPresentation {
    pub deleted: bool,
    updates: std::collections::BTreeMap<usize, CellChange>,
}
#[derive(Default)]
pub(super) struct Presentation {
    signature: Option<(u64, Option<tursobench_core::Ticket>, usize, usize)>,
    pub rows: std::collections::BTreeMap<usize, RowPresentation>,
    review: Option<Result<Vec<tursobench_core::BoundEdit>, tursobench_core::Failure>>,
    selected: usize,
    parameter_index: Option<usize>,
    parameters: String,
}
impl RowPresentation {
    pub(super) fn modified(&self) -> bool {
        !self.updates.is_empty()
    }
}
impl Presentation {
    pub(super) fn value<'a>(
        &self,
        draft: Option<&'a DraftBatch>,
        row: usize,
        column: usize,
    ) -> Option<&'a Value> {
        let location = self.rows.get(&row)?.updates.get(&column)?;
        let Edit::Update { changes, .. } = draft?.edits.get(location.edit)? else {
            return None;
        };
        changes
            .get(location.change)
            .filter(|(col, _)| *col == column)
            .map(|(_, value)| value)
    }

    fn review_ui(
        &mut self,
        ui: &mut egui::Ui,
        draft: &DraftBatch,
        can_remove: bool,
    ) -> Option<usize> {
        let statements = match self.review.get_or_insert_with(|| draft.preview()) {
            Ok(statements) => statements,
            Err(error) => {
                ui.colored_label(ui.visuals().error_fg_color, error.to_string());
                return None;
            }
        };
        let remove = review_operations(ui, statements.len(), &mut self.selected, can_remove);
        self.selected = self.selected.min(statements.len().saturating_sub(1));
        if let Some((statement, edit)) = statements
            .get(self.selected)
            .zip(draft.edits.get(self.selected))
        {
            if self.parameter_index != Some(self.selected) {
                self.parameters = crate::export::bindings_text(&statement.parameters);
                self.parameter_index = Some(self.selected);
            }
            egui::ScrollArea::vertical()
                .id_salt("review-details")
                .max_height((ui.ctx().content_rect().height() - 420.0).clamp(80.0, 300.0))
                .show(ui, |ui| {
                    edit_diff(ui, &draft.table, edit);
                    read_only_code(ui, &statement.sql);
                    ui.small("Exact typed bindings (including original-value predicates):");
                    read_only_code(ui, &self.parameters);
                });
        }
        remove
    }
}

fn review_operations(
    ui: &mut egui::Ui,
    count: usize,
    selected: &mut usize,
    can_remove: bool,
) -> Option<usize> {
    let mut remove = None;
    egui::ScrollArea::vertical()
        .id_salt("review-operations")
        .max_height(150.0)
        .show_rows(ui, 29.0, count, |ui, range| {
            for i in range {
                ui.push_id(i, |ui| {
                    ui.horizontal(|ui| {
                        if ui
                            .selectable_label(*selected == i, format!("Operation {}", i + 1))
                            .clicked()
                        {
                            *selected = i;
                        }
                        if ui
                            .add_enabled(can_remove, egui::Button::new("Remove operation"))
                            .clicked()
                        {
                            remove = Some(i);
                        }
                    });
                });
            }
        });
    remove
}

fn read_only_code(ui: &mut egui::Ui, mut text: &str) {
    ui.add(super::results::code_editor(&mut text, ui.available_width(), 2).interactive(false));
}

fn row_hash(row: &[Value]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    row.len().hash(&mut h);
    for value in row {
        std::mem::discriminant(value).hash(&mut h);
        match value {
            Value::Null => {}
            Value::Integer(n) => n.hash(&mut h),
            Value::Real(n) => (if *n == 0.0 { 0 } else { n.to_bits() }).hash(&mut h),
            Value::Text(s) => s.hash(&mut h),
            Value::Blob(b) => b.hash(&mut h),
            Value::Deferred(c) => {
                c.handle.hash(&mut h);
                c.bytes.hash(&mut h);
            }
        }
    }
    h.finish()
}
impl App {
    pub(super) fn update_draft_presentation(&mut self) {
        let Some(draft) = &self.database.document.drafts else {
            self.database.document.tools.draft_ui = Presentation::default();
            return;
        };
        let signature = (
            self.database.document.draft_revision,
            self.database.document.result.current,
            self.database.document.result.rows.len(),
            draft.edits.len(),
        );
        if self.database.document.tools.draft_ui.signature == Some(signature) {
            return;
        }
        let mut edits = std::collections::HashMap::<u64, Vec<(usize, &Edit)>>::new();
        for (index, edit) in draft.edits.iter().enumerate() {
            if let Edit::Update { original, .. } | Edit::Delete { original } = edit {
                edits
                    .entry(row_hash(original))
                    .or_default()
                    .push((index, edit));
            }
        }
        let mut model = Presentation {
            signature: Some(signature),
            selected: self.database.document.tools.draft_ui.selected,
            ..Default::default()
        };
        for (row, values) in self.database.document.result.rows.iter().enumerate() {
            if let Some(edits) = edits.get(&row_hash(values)) {
                let mut entry = RowPresentation::default();
                for &(edit_index, edit) in edits {
                    match edit {
                        Edit::Update { original, changes } if original == values => {
                            for (change_index, (col, _)) in changes.iter().enumerate() {
                                entry.updates.entry(*col).or_insert(CellChange {
                                    edit: edit_index,
                                    change: change_index,
                                });
                            }
                        }
                        Edit::Delete { original } if original == values => entry.deleted = true,
                        _ => {}
                    }
                }
                if entry.deleted || !entry.updates.is_empty() {
                    model.rows.insert(row, entry);
                }
            }
        }
        self.database.document.tools.draft_ui = model;
    }

    pub(super) fn has_drafts(&self) -> bool {
        self.database.document.has_drafts()
            || self.database.documents.values().any(|d| d.has_drafts())
    }
    pub(super) fn can_edit(&self) -> bool {
        !self
            .database
            .documents
            .values()
            .any(|d| d.has_drafts() || d.active.is_some())
            && !self
                .database
                .document
                .result
                .rows
                .iter()
                .flatten()
                .any(|v| matches!(v, Value::Deferred(_)))
            && !self.database.document.stale
            && self.database.writable
            && self.database.ready
            && !self.database.closing
            && !self.database.uncertain_edit
            && self.database.transaction == TransactionState::Autocommit
            && self.database.commit_ticket.is_none()
            && self.database.document.active.is_none()
            && self.database.document.tools.pages.pending.is_none()
            && self.database.document.view == View::Browse
            && self
                .database
                .document
                .table
                .as_ref()
                .is_some_and(Table::editable)
            && self.database.document.drafts.as_ref().is_none_or(|d| {
                d.edits.is_empty() || Some(&d.table) == self.database.document.table.as_ref()
            })
            && self
                .database
                .document
                .result
                .completion
                .as_ref()
                .is_some_and(|c| c.error.is_none() && c.delivery == Delivery::Complete)
    }
    pub(super) fn stage(&mut self, f: impl FnOnce(&mut DraftBatch)) -> bool {
        if !self.can_edit() {
            return false;
        }
        let Some(table) = &self.database.document.table else {
            return false;
        };
        // Reverting the final operation must not carry the old table identity
        // into a subsequent edit after navigating to another table.
        let mut draft = self
            .database
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
        let validation = if draft.edits.is_empty() {
            Ok(())
        } else {
            draft.preview().map(|_| ())
        };
        match validation {
            Ok(_) => {
                super::spreadsheet::State::checkpoint(&mut self.database.document);
                self.database.document.draft_revision =
                    self.database.document.draft_revision.wrapping_add(1);
                self.database.document.drafts = Some(draft);
                self.database.status =
                    "Staged locally. Preview and explicitly commit to write.".into();
                true
            }
            Err(e) => {
                self.database.status = e.to_string();
                false
            }
        }
    }
    pub(super) fn stage_cell(&mut self, row: usize, column: usize, value: Value) {
        if let Some(original) = self.database.document.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_cell(original, column, value));
        }
    }
    pub(super) fn revert_grid_draft(&mut self, row: usize, column: Option<usize>) {
        if let Some(original) = self.database.document.result.rows.get(row).cloned() {
            self.stage(|draft| match column {
                Some(c) => draft.revert_cell(&original, c),
                None => draft.revert_row(&original),
            });
        }
    }
    pub(super) fn stage_selected_deletions(&mut self) {
        let rows = self
            .selected_cells()
            .map(|(r, _)| r)
            .collect::<std::collections::BTreeSet<_>>();
        let originals = rows
            .into_iter()
            .filter_map(|r| self.database.document.result.rows.get(r).cloned())
            .collect::<Vec<_>>();
        self.stage(|d| {
            for original in originals {
                d.stage_delete(original);
            }
        });
    }
    pub(super) fn stage_delete(&mut self, row: usize) {
        if let Some(original) = self.database.document.result.rows.get(row).cloned() {
            self.stage(|d| d.stage_delete(original));
        }
    }
    pub(super) fn clone_row(&mut self, row: usize) {
        if !self.can_edit() {
            return;
        }
        if let (Some(table), Some(row)) = (
            &self.database.document.table,
            self.database.document.result.rows.get(row),
        ) {
            let values: Vec<Option<&Value>> = table
                .columns
                .iter()
                .zip(row)
                .map(|(c, v)| (c.primary_key <= 0).then_some(v))
                .collect();
            let prepared = PreparedInsert::new(table, values);
            if prepared.is_ok() {
                self.database.status = "Clone prepared locally; primary keys omitted. Supply any required keys before staging.".into();
            }
            self.prepare_insert(prepared);
        }
    }
    fn prepare_insert(&mut self, prepared: Result<PreparedInsert, serde_json::Error>) {
        match prepared {
            Ok(prepared) => {
                self.database.document.insert_fields = prepared.fields;
                self.database.document.insert_use_json = false;
                self.database.document.insert_json = prepared.json;
                self.database.document.show_insert = true;
            }
            Err(error) => self.database.status = error.to_string(),
        }
    }
    pub(super) fn begin_insert(&mut self) {
        if self.can_edit()
            && let Some(table) = &self.database.document.table
        {
            let prepared = PreparedInsert::new(table, vec![None; table.columns.len()]);
            self.prepare_insert(prepared);
        }
    }
    pub(super) fn edit_windows(&mut self, ctx: &egui::Context) {
        self.spreadsheet_window(ctx);
        if self.database.document.show_insert {
            self.insert_window(ctx);
        }
        if self.database.document.show_preview {
            self.review_window(ctx);
        }
    }

    fn insert_fields_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(t) = &self.database.document.table {
            ui.label(
                t.columns
                    .iter()
                    .map(|c| c.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", "),
            );
        }
        if self.database.document.insert_use_json {
            ui.small("JSON null omits a column; {\"type\":\"Null\"} inserts SQL NULL.");
            ui.add(
                egui::TextEdit::multiline(&mut self.database.document.insert_json)
                    .code_editor()
                    .desired_width(ui.available_width())
                    .desired_rows(8),
            );
        } else if let Some(table) = &self.database.document.table {
            for (i, (field, column)) in self
                .database
                .document
                .insert_fields
                .iter_mut()
                .zip(&table.columns)
                .enumerate()
            {
                ui.add_enabled_ui(column.hidden == 0, |ui| field.show(ui, i, column));
            }
        }
    }

    fn insert_window(&mut self, ctx: &egui::Context) {
        let mut open = true;
        egui::Window::new("Stage insert — typed column values")
            .frame(crate::app::presentation::window_frame(ctx))
            .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 180.0))
            .open(&mut open)
            .default_width(580.0)
            .max_width((ctx.content_rect().width() - 40.0).max(280.0))
            .show(ctx, |ui| {
                crate::app::presentation::checkbox(
                    ui,
                    &mut self.database.document.insert_use_json,
                    "Advanced JSON input",
                );
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 230.0).clamp(120.0, 400.0))
                    .show(ui, |ui| {
                        self.insert_fields_ui(ui);
                    });
                if presentation::footer(ui, |ui| {
                    ui.add_enabled(
                        self.can_edit(),
                        presentation::action(
                            ui,
                            "Stage insert (no database write)",
                            Intent::Pending,
                        ),
                    )
                })
                .inner
                .clicked()
                {
                    let values = if self.database.document.insert_use_json {
                        serde_json::from_str::<Vec<Option<Value>>>(
                            &self.database.document.insert_json,
                        )
                        .map_err(|e| e.to_string())
                    } else {
                        self.database
                            .document
                            .insert_fields
                            .iter()
                            .map(InsertField::value)
                            .collect::<Result<Vec<_>, _>>()
                    };
                    match values {
                        Ok(values) => {
                            self.stage(|d| d.edits.push(Edit::Insert { values }));
                        }
                        Err(e) => self.database.status = e.to_string(),
                    }
                }
                ui.label(&self.database.status);
            });
        self.database.document.show_insert = open;
    }

    fn review_window(&mut self, ctx: &egui::Context) {
        self.update_draft_presentation();
        let mut open = true;
        egui::Window::new("Review staged atomic transaction")
            .frame(presentation::window_frame(ctx))
            .default_pos(ctx.content_rect().center()-egui::vec2(280.0,180.0))
            .open(&mut open).default_width(760.0)
            .max_width((ctx.content_rect().width()-40.0).max(280.0))
            .show(ctx, |ui| {
                presentation::notice(ui,super::icons::Icon::ReviewEdits,Intent::Pending,"Dedicated connection · foreign keys enabled · original values checked · atomic batch. Only a confirmed commit clears these drafts.");
                let remove = self.database.document.drafts.as_ref().and_then(|draft| {
                    ui.strong(format!("{} · {} staged operations", draft.table.name, draft.edits.len()));
                    self.database.document.tools.draft_ui.review_ui(ui, draft, self.database.commit_ticket.is_none())
                });
                if let Some(index) = remove {
                    self.stage(|draft| { draft.edits.remove(index); });
                }
                if self.database.uncertain_edit { ui.colored_label(ui.visuals().error_fg_color,"UNKNOWN outcome — close/reopen and inspect; never automatically retry."); }
                if self.database.commit_ticket.is_some() { ui.label("Committing — awaiting the actual outcome…"); }
                if presentation::footer(ui, |ui| ui.add_enabled(self.can_edit() && self.has_drafts(),presentation::action(ui,"COMMIT staged edits to database",Intent::Pending))).inner.clicked() { self.commit_drafts(); }
                ui.label(&self.database.status);
            });
        self.database.document.show_preview = open;
    }
    pub(super) fn staged_edit_dock(&mut self, ui: &mut egui::Ui) {
        let count = self
            .database
            .document
            .drafts
            .as_ref()
            .map_or(0, |d| d.edits.len());
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        egui::Frame::new()
            .fill(tokens.pending.gamma_multiply(0.08))
            .inner_margin(egui::Margin::symmetric(4, 2))
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
                    if icons::button(
                        ui,
                        Icon::ReviewEdits,
                        "Review staged edits · Diff / SQL",
                        "",
                    )
                    .clicked()
                    {
                        self.database.document.show_preview = true;
                    }
                    if ui
                        .add_enabled_ui(
                            self.database.commit_ticket.is_none() && !self.database.uncertain_edit,
                            |ui| icons::button(ui, Icon::Undo, "Discard staged edits", ""),
                        )
                        .inner
                        .clicked()
                    {
                        self.database.document.drafts = None;
                        self.database.document.tools.spreadsheet = Default::default();
                    }
                    if self.database.commit_ticket.is_some() {
                        ui.small("Committing…");
                    }
                    if self.database.uncertain_edit {
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
        let (Some(s), Some(batch)) = (&self.database.session, &self.database.document.drafts)
        else {
            return;
        };
        self.database.sequence += 1;
        let ticket = s.ticket(self.database.sequence, self.database.generation);
        match s.submit(Request {
            ticket,
            operation: Operation::CommitEdits(batch.clone()),
        }) {
            Ok(()) => {
                self.database.commit_ticket = Some(ticket);
                self.database.commit_owner = Some(self.database.document.id);
                self.database.status =
                    "Committing staged edits… do not retry until outcome is known".into();
            }
            Err(e) => self.database.status = e.to_string(),
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
