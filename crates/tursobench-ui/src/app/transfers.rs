use super::App;
use eframe::egui;
use tursobench_core::{
    Bindings, Browse, EventKind, ImportFormat, ImportPreview, ImportSpec, InputKind, Table, Ticket,
    TransactionState,
};
use tursobench_db::Operation;

pub(super) struct State {
    pub ticket: Option<Ticket>,
    open: bool,
    session: Option<u64>,
    output: String,
    pub(super) input: String,
    format: ImportFormat,
    kinds: Vec<InputKind>,
    mapping: Vec<String>,
    preview: Option<ImportPreview>,
    preview_current: bool,
    table: Option<Table>,
    source: Option<Browse>,
    query: Option<(String, Bindings)>,
    null_marker: String,
    atomic: bool,
    message: String,
}
impl Default for State {
    fn default() -> Self {
        Self {
            ticket: None,
            open: false,
            session: None,
            output: String::new(),
            input: String::new(),
            format: ImportFormat::JsonLines,
            kinds: vec![],
            mapping: vec![],
            preview: None,
            preview_current: false,
            table: None,
            source: None,
            query: None,
            null_marker: "\\N".into(),
            atomic: true,
            message: String::new(),
        }
    }
}
/// A button that reports a click only while `enabled`.
fn action(ui: &mut egui::Ui, enabled: bool, label: &str) -> bool {
    ui.add_enabled(enabled, egui::Button::new(label)).clicked()
}
/// A combo box of `options` labelled by their names; returns whether it changed.
fn choice<T: Copy + PartialEq + std::fmt::Debug>(
    ui: &mut egui::Ui,
    id: impl egui::AsIdSalt,
    value: &mut T,
    options: [T; 4],
) -> bool {
    let mut changed = false;
    egui::ComboBox::from_id_salt(id)
        .selected_text(format!("{value:?}"))
        .show_ui(ui, |ui| {
            for option in options {
                changed |= ui
                    .selectable_value(value, option, format!("{option:?}"))
                    .changed();
            }
        });
    changed
}
impl State {
    /// Exports of the source table or statement to a new JSONL file.
    fn export_ui(&mut self, ui: &mut egui::Ui, enabled: bool) -> Option<Operation> {
        ui.strong("Complete source export · typed JSONL");
        ui.horizontal(|ui| {
            let label = ui.label("New output file");
            ui.text_edit_singleline(&mut self.output)
                .labelled_by(label.id);
        });
        let enabled = enabled && !self.output.is_empty();
        let path = || self.output.clone().into();
        let mut operation = None;
        if let Some(spec) = &self.source {
            ui.label(format!(
                "Table: {} · current filters and sort · all matching rows",
                spec.table
            ));
            if action(ui, enabled, "Export complete table to JSONL") {
                operation = Some(Operation::ExportTable {
                    path: path(),
                    spec: spec.clone(),
                });
            }
        }
        if let Some((text, parameters)) = &self.query {
            ui.label(format!(
                "Statement: {}",
                text.chars().take(160).collect::<String>()
            ));
            if action(ui, enabled, "Execute statement once and export JSONL") {
                operation = Some(Operation::ExportQuery {
                    path: path(),
                    sql: text.clone(),
                    parameters: parameters.clone(),
                });
            }
        }
        operation
    }
    /// The import source, its format and NULL marker, and the preview that
    /// maps it onto the selected table.
    fn import_ui(&mut self, ui: &mut egui::Ui, enabled: bool, writable: bool) -> Option<Operation> {
        ui.strong("Import into the selected table");
        ui.label(
            self.table
                .as_ref()
                .map_or("Select a table before opening this panel", |t| {
                    t.name.as_str()
                }),
        );
        let mut changed = ui.text_edit_singleline(&mut self.input).changed();
        changed |= choice(
            ui,
            "import-format",
            &mut self.format,
            [
                ImportFormat::JsonLines,
                ImportFormat::Json,
                ImportFormat::Csv,
                ImportFormat::Tsv,
            ],
        );
        if self.delimited() {
            ui.label("Unquoted NULL marker");
            changed |= ui.text_edit_singleline(&mut self.null_marker).changed();
        }
        if changed {
            self.preview_current = false;
        }
        let preview = action(
            ui,
            enabled && !self.input.is_empty(),
            "Preview first 10 rows",
        )
        .then(|| Operation::PreviewImport {
            path: self.input.clone().into(),
            format: self.format,
            kinds: self.kinds.clone(),
            null_marker: self.null_marker.clone(),
        });
        self.import_preview_ui(ui, enabled, writable).or(preview)
    }
    fn delimited(&self) -> bool {
        matches!(self.format, ImportFormat::Csv | ImportFormat::Tsv)
    }
    fn import_preview_ui(
        &mut self,
        ui: &mut egui::Ui,
        enabled: bool,
        writable: bool,
    ) -> Option<Operation> {
        let preview = self.preview.as_ref()?;
        ui.label(format!("Input scope: {}", preview.scope));
        if let Some(table) = &self.table {
            let delimited = self.delimited();
            for (i, name) in preview.columns.iter().enumerate() {
                ui.horizontal(|ui| {
                    ui.label(name);
                    mapping_ui(ui, ("import-map", i), table, &mut self.mapping[i]);
                    let kinds = [
                        InputKind::Text,
                        InputKind::Integer,
                        InputKind::Real,
                        InputKind::Blob,
                    ];
                    if delimited && choice(ui, ("import-kind", i), &mut self.kinds[i], kinds) {
                        self.preview_current = false;
                    }
                });
            }
        }
        for row in &preview.rows {
            ui.monospace(
                row.iter()
                    .map(tursobench_core::Value::preview)
                    .collect::<Vec<_>>()
                    .join(" | "),
            );
        }
        crate::app::presentation::checkbox(ui, &mut self.atomic, "Import all rows atomically")
            .on_hover_text(
                "When disabled, batches commit every 1000 rows or 8 MiB. A later error keeps earlier commits.",
            );
        let table = self.table.as_ref();
        let ready = enabled && writable && self.preview_current && table.is_some();
        if !action(ui, ready, "Import mapped rows") {
            return None;
        }
        Some(Operation::Import(ImportSpec {
            path: self.input.clone().into(),
            table: table?.name.clone(),
            format: self.format,
            source_columns: preview.columns.clone(),
            mapping: self.mapping.clone(),
            kinds: self.kinds.clone(),
            null_marker: self.null_marker.clone(),
            atomic: self.atomic,
        }))
    }
    fn maintenance_ui(
        &self,
        ui: &mut egui::Ui,
        enabled: bool,
        writable: bool,
    ) -> Option<Operation> {
        ui.strong("Maintenance · write access and autocommit required");
        ui.small("Checkpoints can be busy while another transaction is active. Close other shared connections before backup; a refused capture may leave this session closed.");
        let enabled = enabled && writable;
        let checkpoint = action(ui, enabled, "Checkpoint WAL").then_some(Operation::Checkpoint);
        let backup = action(
            ui,
            enabled && !self.output.is_empty(),
            "Checkpoint, close session and save verified backup",
        )
        .then(|| Operation::BackupAndClose(self.output.clone().into()));
        backup.or(checkpoint)
    }
    /// A preview with new columns maps each onto the visible table column of
    /// the same name, ignoring case; the mapping is kept for the same columns.
    fn preview_loaded(&mut self, preview: ImportPreview) {
        if self
            .preview
            .as_ref()
            .is_none_or(|p| p.columns != preview.columns)
        {
            let columns = self.table.as_ref().map_or(&[][..], |t| &t.columns);
            self.mapping = preview
                .columns
                .iter()
                .map(|name| {
                    columns
                        .iter()
                        .find(|c| c.hidden == 0 && c.name.eq_ignore_ascii_case(name))
                        .map_or_else(String::new, |c| c.name.clone())
                })
                .collect();
        }
        self.kinds.resize(preview.columns.len(), InputKind::Text);
        self.preview = Some(preview);
        self.preview_current = true;
        self.message = "Preview loaded. Remaining records are validated during import.".into();
    }

    pub(super) fn disconnected(&mut self) {
        if self.ticket.take().is_some() {
            self.message = "Source session closed; pending transfer did not complete".into();
        }
    }
}
/// A source column's destination: a visible table column, or Skip (empty).
fn mapping_ui(ui: &mut egui::Ui, id: impl egui::AsIdSalt, table: &Table, mapping: &mut String) {
    egui::ComboBox::from_id_salt(id)
        .selected_text(if mapping.is_empty() {
            "Skip"
        } else {
            mapping.as_str()
        })
        .show_ui(ui, |ui| {
            ui.selectable_value(mapping, String::new(), "Skip");
            for c in table.columns.iter().filter(|c| c.hidden == 0) {
                ui.selectable_value(mapping, c.name.clone(), &c.name);
            }
        });
}
impl App {
    pub(super) fn open_transfers(&mut self) {
        if self.database.tools.transfers.ticket.is_some() {
            self.database.tools.transfers.open = true;
            return;
        }
        let statement = self.current_text();
        let query = statement.and_then(|text| {
            Bindings::parse_json(&self.database.document.params).map(|params| (text, params))
        });
        self.database.tools.transfers = State {
            open: true,
            session: self.database.session.as_ref().map(|s| s.id),
            table: self.database.document.table.clone(),
            source: self.database.document.browse.clone(),
            message: query
                .as_ref()
                .err()
                .map_or_else(String::new, ToString::to_string),
            query: query.ok(),
            ..Default::default()
        };
    }
    fn transfer_request(&mut self, operation: Operation) {
        let Some(session) = self.database.session.as_ref().map(|s| s.id) else {
            return;
        };
        let state = &mut self.database.tools.transfers;
        if state.session != Some(session) {
            state.message = "Reopen the transfer panel for the current database".into();
            return;
        }
        if !self.requests_enabled() || self.database.tools.transfers.ticket.is_some() {
            return;
        }
        let submitted = self.database.request(operation);
        let state = &mut self.database.tools.transfers;
        match submitted {
            Some(Ok(ticket)) => {
                state.ticket = Some(ticket);
                state.message = "Queued…".into();
            }
            Some(Err(e)) => state.message = e.to_string(),
            None => {}
        }
    }
    pub(super) fn transfer_event(&mut self, kind: EventKind) {
        let state = &mut self.database.tools.transfers;
        match kind {
            EventKind::ImportPreview(preview) => state.preview_loaded(preview),
            EventKind::TransferFinished(outcome) => {
                self.database.transaction = outcome.transaction;
                if outcome.transaction == TransactionState::Unknown {
                    self.database.uncertain_edit = true;
                }
                state.message = outcome.message;
                self.database.status = state.message.clone();
            }
            EventKind::Failed(e) => {
                state.message = e.to_string();
                state.preview_current = false;
            }
            _ => return,
        }
        state.ticket = None;
    }
    pub(super) fn transfer_ui(&mut self, ctx: &egui::Context) {
        if !self.database.tools.transfers.open {
            return;
        }
        let writable = !self.database.uncertain_edit
            && self.database.writable
            && self.database.transaction == TransactionState::Autocommit
            && self.database.document.active.is_none()
            && self.database.documents.values().all(|d| d.active.is_none());
        let enabled = self.requests_enabled()
            && self.database.tools.transfers.ticket.is_none()
            && self.database.tools.transfers.session
                == self.database.session.as_ref().map(|s| s.id);
        let mut open = true;
        let mut operation = None;
        let mut cancel = false;
        let state = &mut self.database.tools.transfers;
        egui::Window::new("Data transfer and maintenance")
            .id(egui::Id::unique("data-transfers"))
            .open(&mut open)
            .default_width(680.0)
            .max_height(ctx.content_rect().height() * 0.85)
            .vscroll(true)
            .show(ctx, |ui| {
                let export = state.export_ui(ui, enabled);
                ui.separator();
                let import = state.import_ui(ui, enabled, writable);
                ui.separator();
                let maintenance = state.maintenance_ui(ui, enabled, writable);
                // Later sections win, as only one button is clicked per frame.
                operation = maintenance.or(import).or(export);
                if state.ticket.is_some() {
                    ui.spinner();
                    cancel = ui
                        .button("Cancel transfer after the current write")
                        .clicked();
                }
                ui.label(&state.message);
            });
        self.database.tools.transfers.open = open;
        if cancel
            && let Some(ticket) = self.database.tools.transfers.ticket
            && let Some(session) = &self.database.session
        {
            session.discard_results(ticket.request);
        }
        if let Some(operation) = operation {
            self.transfer_request(operation);
        }
    }
}
