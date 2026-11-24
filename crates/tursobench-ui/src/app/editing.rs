use super::icons::{self, Icon};
use super::presentation::{self, Intent};
use super::{App, View};
use eframe::egui;
use tursobench_core::{Delivery, DraftBatch, Edit, Table, Ticket, TransactionState, Value};
use tursobench_db::Operation;

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
/// The empty value a new row's editor starts from, by the column's declared type.
fn zero_value(declared_type: &str) -> Value {
    let ty = declared_type.to_uppercase();
    if ty.contains("INT") {
        Value::Integer(0)
    } else if ty.contains("REAL") {
        Value::Real(0.0)
    } else if ty.contains("BLOB") {
        Value::Blob(vec![])
    } else {
        Value::Text(String::new())
    }
}
impl InsertField {
    fn new(column: &tursobench_core::TableColumn, value: Option<&Value>) -> Self {
        let zero;
        let (mode, initial) = match value {
            None => {
                zero = zero_value(&column.declared_type);
                (InsertMode::Default, &zero)
            }
            Some(Value::Null) => (InsertMode::Null, &Value::Null),
            Some(value) => (InsertMode::Value, value),
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
    signature: Option<Signature>,
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
    /// Which of `edits` update or delete this row, compared by its original values.
    fn of(values: &[Value], edits: &[(usize, &Edit)]) -> Self {
        let mut row = Self::default();
        for &(edit, staged) in edits {
            match staged {
                Edit::Update { original, changes } if original == values => {
                    for (change, (col, _)) in changes.iter().enumerate() {
                        row.updates
                            .entry(*col)
                            .or_insert(CellChange { edit, change });
                    }
                }
                Edit::Delete { original } if original == values => row.deleted = true,
                _ => {}
            }
        }
        row
    }
}
/// What the presentation was built from: draft revision, result, row and edit counts.
type Signature = (u64, Option<Ticket>, usize, usize);
impl Presentation {
    /// Each retained row's staged updates and deletion; edits are matched to
    /// rows by a hash of their original values, then compared in full.
    fn build(
        draft: &DraftBatch,
        rows: &[Vec<Value>],
        signature: Signature,
        selected: usize,
    ) -> Self {
        let mut by_original = std::collections::HashMap::<u64, Vec<(usize, &Edit)>>::new();
        for (index, edit) in draft.edits.iter().enumerate() {
            if let Edit::Update { original, .. } | Edit::Delete { original } = edit {
                let entry = by_original.entry(row_hash(original)).or_default();
                entry.push((index, edit));
            }
        }
        let rows = rows.iter().enumerate().filter_map(|(row, values)| {
            let presented = RowPresentation::of(values, by_original.get(&row_hash(values))?);
            (presented.deleted || presented.modified()).then_some((row, presented))
        });
        Self {
            signature: Some(signature),
            rows: rows.collect(),
            selected,
            ..Default::default()
        }
    }
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
        let document = &self.database.document;
        let Some(draft) = &document.drafts else {
            self.database.document.tools.draft_ui = Presentation::default();
            return;
        };
        let signature = (
            document.draft_revision,
            document.result.current,
            document.result.rows.len(),
            draft.edits.len(),
        );
        let presented = &document.tools.draft_ui;
        if presented.signature == Some(signature) {
            return;
        }
        let rows = &document.result.rows;
        let model = Presentation::build(draft, rows, signature, presented.selected);
        self.database.document.tools.draft_ui = model;
    }

    pub(super) fn has_drafts(&self) -> bool {
        self.database.document.has_drafts()
            || self.database.documents.values().any(|d| d.has_drafts())
    }
    pub(super) fn can_edit(&self) -> bool {
        let database = &self.database;
        let document = &database.document;
        let rows = &document.result.rows;
        let table = document.table.as_ref();
        !database
            .documents
            .values()
            .any(|d| d.has_drafts() || d.active.is_some())
            && !rows
                .iter()
                .flatten()
                .any(|v| matches!(v, Value::Deferred(_)))
            && !document.stale
            && database.writable
            && database.ready
            && !database.closing
            && !database.uncertain_edit
            && database.transaction == TransactionState::Autocommit
            && database.commit_ticket.is_none()
            && document.active.is_none()
            && document.tools.pages.pending.is_none()
            && document.view == View::Browse
            && table.is_some_and(Table::editable)
            && document
                .drafts
                .as_ref()
                .is_none_or(|d| d.edits.is_empty() || Some(&d.table) == table)
            && document
                .result
                .completion
                .as_ref()
                .is_some_and(|c| c.error.is_none() && c.delivery == Delivery::Complete)
    }
    pub(super) fn stage(&mut self, f: impl FnOnce(&mut DraftBatch)) -> bool {
        if !self.can_edit() {
            return false;
        }
        let document = &mut self.database.document;
        let Some(table) = &document.table else {
            return false;
        };
        // Reverting the final operation must not carry the old table identity
        // into a subsequent edit after navigating to another table.
        let mut draft = document
            .drafts
            .as_ref()
            .filter(|d| !d.edits.is_empty())
            .cloned()
            .unwrap_or_else(|| DraftBatch {
                table: table.clone(),
                edits: vec![],
            });
        f(&mut draft);
        if !draft.edits.is_empty()
            && let Err(e) = draft.preview()
        {
            self.database.status = e.to_string();
            return false;
        }
        document.checkpoint_drafts();
        document.draft_revision = document.draft_revision.wrapping_add(1);
        document.drafts = Some(draft);
        self.database.status = "Staged locally. Preview and explicitly commit to write.".into();
        true
    }
    /// Stage a change to a retained row, given its original values.
    fn stage_row(&mut self, row: usize, f: impl FnOnce(&mut DraftBatch, Vec<Value>)) {
        if let Some(original) = self.database.document.result.rows.get(row).cloned() {
            self.stage(|draft| f(draft, original));
        }
    }
    pub(super) fn stage_cell(&mut self, row: usize, column: usize, value: Value) {
        self.stage_row(row, |d, original| d.stage_cell(original, column, value));
    }
    pub(super) fn revert_grid_draft(&mut self, row: usize, column: Option<usize>) {
        self.stage_row(row, |draft, original| match column {
            Some(c) => draft.revert_cell(&original, c),
            None => draft.revert_row(&original),
        });
    }
    pub(super) fn stage_delete(&mut self, row: usize) {
        self.stage_row(row, |d, original| d.stage_delete(original));
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
        presentation::dialog(ctx, "Stage insert — typed column values", &mut open)
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
                    match self.insert_values() {
                        Ok(values) => {
                            self.stage(|d| d.edits.push(Edit::Insert { values }));
                        }
                        Err(e) => self.database.status = e,
                    }
                }
                ui.label(&self.database.status);
            });
        self.database.document.show_insert = open;
    }
    /// The new row's values, from the JSON text or the typed fields; `None` omits a column.
    fn insert_values(&self) -> Result<Vec<Option<Value>>, String> {
        let document = &self.database.document;
        if document.insert_use_json {
            serde_json::from_str(&document.insert_json).map_err(|e| e.to_string())
        } else {
            document
                .insert_fields
                .iter()
                .map(InsertField::value)
                .collect()
        }
    }

    fn review_window(&mut self, ctx: &egui::Context) {
        self.update_draft_presentation();
        let mut open = true;
        presentation::dialog(ctx, "Review staged atomic transaction", &mut open)
            .default_width(760.0)
            .max_width((ctx.content_rect().width() - 40.0).max(280.0))
            .show(ctx, |ui| self.review_contents(ui));
        self.database.document.show_preview = open;
    }
    /// Each staged operation with its diff, SQL and bindings, then the commit.
    fn review_contents(&mut self, ui: &mut egui::Ui) {
        presentation::notice(
            ui,
            Icon::ReviewEdits,
            Intent::Pending,
            "Dedicated connection · foreign keys enabled · original values checked · atomic batch. Only a confirmed commit clears these drafts.",
        );
        let can_remove = self.database.commit_ticket.is_none();
        let document = &mut self.database.document;
        let remove = document.drafts.as_ref().and_then(|draft| {
            let count = draft.edits.len();
            ui.strong(format!("{} · {count} staged operations", draft.table.name));
            document.tools.draft_ui.review_ui(ui, draft, can_remove)
        });
        if let Some(index) = remove {
            self.stage(|draft| {
                draft.edits.remove(index);
            });
        }
        if self.database.uncertain_edit {
            ui.colored_label(
                ui.visuals().error_fg_color,
                "UNKNOWN outcome — close/reopen and inspect; never automatically retry.",
            );
        }
        if self.database.commit_ticket.is_some() {
            ui.label("Committing — awaiting the actual outcome…");
        }
        let enabled = self.can_edit() && self.has_drafts();
        let commit = presentation::footer(ui, |ui| {
            let commit = "COMMIT staged edits to database";
            ui.add_enabled(enabled, presentation::action(ui, commit, Intent::Pending))
        });
        if commit.inner.clicked() {
            self.commit_drafts();
        }
        ui.label(&self.database.status);
    }
    pub(super) fn staged_edit_dock(&mut self, ui: &mut egui::Ui) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        egui::Frame::new()
            .fill(tokens.pending.gamma_multiply(0.08))
            .inner_margin(egui::Margin::symmetric(4, 2))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| self.staged_edit_actions(ui, tokens));
            });
    }
    /// The staged count, then commit, review and discard, and the commit's state.
    fn staged_edit_actions(&mut self, ui: &mut egui::Ui, tokens: crate::appearance::Tokens) {
        let count = self
            .database
            .document
            .drafts
            .as_ref()
            .map_or(0, |d| d.edits.len());
        ui.colored_label(tokens.pending, format!("{count} staged"));
        let commit = egui::Button::new(egui::RichText::new("Commit").color(tokens.pending))
            .fill(tokens.pending.gamma_multiply(0.13))
            .stroke(egui::Stroke::new(1.0, tokens.pending.gamma_multiply(0.48)));
        if ui
            .add_enabled(self.can_edit() && count > 0, commit)
            .clicked()
        {
            self.commit_drafts();
        }
        let review = "Review staged edits · Diff / SQL";
        if icons::button(ui, Icon::ReviewEdits, review, "").clicked() {
            self.database.document.show_preview = true;
        }
        let settled = self.database.commit_ticket.is_none() && !self.database.uncertain_edit;
        if ui
            .add_enabled_ui(settled, |ui| {
                icons::button(ui, Icon::Undo, "Discard staged edits", "")
            })
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
    }
    pub(super) fn commit_drafts(&mut self) {
        if !self.can_edit() || !self.has_drafts() {
            return;
        }
        let Some(batch) = self.database.document.drafts.clone() else {
            return;
        };
        match self.database.request(Operation::CommitEdits(batch)) {
            Some(Ok(ticket)) => {
                self.database.commit_ticket = Some(ticket);
                self.database.commit_owner = Some(self.database.document.id);
                self.database.status =
                    "Committing staged edits… do not retry until outcome is known".into();
            }
            Some(Err(e)) => self.database.status = e.to_string(),
            None => {}
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
