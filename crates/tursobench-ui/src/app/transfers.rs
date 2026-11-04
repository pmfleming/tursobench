use super::App;
use eframe::egui;
use tursobench_core::{
    Bindings, Browse, EventKind, ImportFormat, ImportPreview, ImportSpec, InputKind, Table, Ticket,
    TransactionState,
};
use tursobench_db::{Operation, Request};

pub(super) struct State {
    pub ticket: Option<Ticket>,
    open: bool,
    session: Option<u64>,
    output: String,
    input: String,
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
impl State {
    fn import_preview_ui(
        &mut self,
        ui: &mut egui::Ui,
        enabled: bool,
        writable: bool,
    ) -> Option<Operation> {
        let mut operation = None;
        let state = self;
        if let Some(preview) = &state.preview {
            ui.label(format!("Input scope: {}", preview.scope));
            if let Some(table) = &state.table {
                for (i, name) in preview.columns.iter().enumerate() {
                    ui.horizontal(|ui| {
                        ui.label(name);
                        egui::ComboBox::from_id_salt(("import-map", i))
                            .selected_text(if state.mapping[i].is_empty() {
                                "Skip"
                            } else {
                                &state.mapping[i]
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut state.mapping[i], String::new(), "Skip");
                                for c in table.columns.iter().filter(|c| c.hidden == 0) {
                                    ui.selectable_value(
                                        &mut state.mapping[i],
                                        c.name.clone(),
                                        &c.name,
                                    );
                                }
                            });
                        if matches!(state.format, ImportFormat::Csv | ImportFormat::Tsv) {
                            egui::ComboBox::from_id_salt(("import-kind", i))
                                .selected_text(format!("{:?}", state.kinds[i]))
                                .show_ui(ui, |ui| {
                                    for kind in [
                                        InputKind::Text,
                                        InputKind::Integer,
                                        InputKind::Real,
                                        InputKind::Blob,
                                    ] {
                                        if ui
                                            .selectable_value(
                                                &mut state.kinds[i],
                                                kind,
                                                format!("{kind:?}"),
                                            )
                                            .changed()
                                        {
                                            state.preview_current = false;
                                        }
                                    }
                                });
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
            ui.checkbox(&mut state.atomic,"Import all rows atomically").on_hover_text("When disabled, batches commit every 1000 rows or 8 MiB. A later error keeps earlier commits.");
            if ui
                .add_enabled(
                    enabled && writable && state.preview_current && state.table.is_some(),
                    egui::Button::new("Import mapped rows"),
                )
                .clicked()
                && let Some(table) = &state.table
            {
                operation = Some(Operation::Import(ImportSpec {
                    path: state.input.clone().into(),
                    table: table.name.clone(),
                    format: state.format,
                    source_columns: preview.columns.clone(),
                    mapping: state.mapping.clone(),
                    kinds: state.kinds.clone(),
                    null_marker: state.null_marker.clone(),
                    atomic: state.atomic,
                }));
            }
        }
        operation
    }

    pub(super) fn disconnected(&mut self) {
        if self.ticket.take().is_some() {
            self.message = "Source session closed; pending transfer did not complete".into();
        }
    }
}
impl App {
    pub(super) fn open_transfers(&mut self) {
        if self.tools.transfers.ticket.is_some() {
            self.tools.transfers.open = true;
            return;
        }
        let statement = self.current_text();
        let query = statement.and_then(|text| {
            Bindings::parse_json(&self.document.params).map(|params| (text, params))
        });
        self.tools.transfers = State {
            open: true,
            session: self.session.as_ref().map(|s| s.id),
            table: self.document.table.clone(),
            source: self.document.browse.clone(),
            message: query
                .as_ref()
                .err()
                .map_or_else(String::new, ToString::to_string),
            query: query.ok(),
            ..Default::default()
        };
    }
    fn transfer_request(&mut self, operation: Operation) {
        let Some(session) = &self.session else {
            return;
        };
        if self.tools.transfers.session != Some(session.id) {
            self.tools.transfers.message =
                "Reopen the transfer panel for the current database".into();
            return;
        }
        if !self.requests_enabled() || self.tools.transfers.ticket.is_some() {
            return;
        }
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request { ticket, operation }) {
            Ok(()) => {
                self.tools.transfers.ticket = Some(ticket);
                self.tools.transfers.message = "Queued…".into();
            }
            Err(e) => self.tools.transfers.message = e.to_string(),
        }
    }
    pub(super) fn transfer_event(&mut self, kind: EventKind) {
        let state = &mut self.tools.transfers;
        match kind {
            EventKind::ImportPreview(preview) => {
                if state
                    .preview
                    .as_ref()
                    .is_none_or(|p| p.columns != preview.columns)
                {
                    state.mapping = preview
                        .columns
                        .iter()
                        .map(|name| {
                            state
                                .table
                                .as_ref()
                                .and_then(|t| {
                                    t.columns.iter().find(|c| {
                                        c.hidden == 0 && c.name.eq_ignore_ascii_case(name)
                                    })
                                })
                                .map_or_else(String::new, |c| c.name.clone())
                        })
                        .collect();
                }
                state.kinds.resize(preview.columns.len(), InputKind::Text);
                state.preview = Some(preview);
                state.preview_current = true;
                state.message =
                    "Preview loaded. Remaining records are validated during import.".into();
            }
            EventKind::TransferFinished(outcome) => {
                self.transaction = outcome.transaction;
                if outcome.transaction == TransactionState::Unknown {
                    self.uncertain_edit = true;
                }
                state.message = outcome.message;
                self.status = state.message.clone();
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
        if !self.tools.transfers.open {
            return;
        }
        let writable = !self.uncertain_edit
            && self.writable
            && self.transaction == TransactionState::Autocommit
            && self.document.active.is_none()
            && self.documents.values().all(|d| d.active.is_none());
        let enabled = self.requests_enabled()
            && self.tools.transfers.ticket.is_none()
            && self.tools.transfers.session == self.session.as_ref().map(|s| s.id);
        let mut open = true;
        let mut operation = None;
        let mut cancel = false;
        let state = &mut self.tools.transfers;
        egui::Window::new("Data transfer and maintenance").id(egui::Id::unique("data-transfers")).open(&mut open).default_width(680.0).max_height(ctx.content_rect().height()*0.85).vscroll(true).show(ctx,|ui|{
            ui.strong("Complete source export · typed JSONL");
            ui.label("New output file");ui.text_edit_singleline(&mut state.output);
            if let Some(spec)=&state.source {
                ui.label(format!("Table: {} · current filters and sort · all matching rows",spec.table));
                if ui.add_enabled(enabled&&!state.output.is_empty(),egui::Button::new("Export complete table to JSONL")).clicked(){operation=Some(Operation::ExportTable{path:state.output.clone().into(),spec:spec.clone()});}
            }
            if let Some((text,parameters))=&state.query {
                ui.label(format!("Statement: {}",text.chars().take(160).collect::<String>()));
                if ui.add_enabled(enabled&&!state.output.is_empty(),egui::Button::new("Execute statement once and export JSONL")).clicked(){operation=Some(Operation::ExportQuery{path:state.output.clone().into(),sql:text.clone(),parameters:parameters.clone()});}
            }
            ui.separator();ui.strong("Import into the selected table");
            ui.label(state.table.as_ref().map_or("Select a table before opening this panel",|t|t.name.as_str()));
            if ui.text_edit_singleline(&mut state.input).changed(){state.preview_current=false;}
            egui::ComboBox::from_id_salt("import-format").selected_text(format!("{:?}",state.format)).show_ui(ui,|ui|{
                for format in [ImportFormat::JsonLines,ImportFormat::Json,ImportFormat::Csv,ImportFormat::Tsv]{if ui.selectable_value(&mut state.format,format,format!("{format:?}")).changed(){state.preview_current=false;}}
            });
            if matches!(state.format,ImportFormat::Csv|ImportFormat::Tsv){ui.label("Unquoted NULL marker");if ui.text_edit_singleline(&mut state.null_marker).changed(){state.preview_current=false;}}
            if ui.add_enabled(enabled&&!state.input.is_empty(),egui::Button::new("Preview first 10 rows")).clicked(){operation=Some(Operation::PreviewImport{path:state.input.clone().into(),format:state.format,kinds:state.kinds.clone(),null_marker:state.null_marker.clone()});}
            if let Some(next)=state.import_preview_ui(ui,enabled,writable){operation=Some(next);}
            ui.separator();ui.strong("Maintenance · write access and autocommit required");
            ui.small("Checkpoints can be busy while another transaction is active. Close other shared connections before backup; a refused capture may leave this session closed.");
            if ui.add_enabled(enabled&&writable,egui::Button::new("Checkpoint WAL")).clicked(){operation=Some(Operation::Checkpoint);}
            if ui.add_enabled(enabled&&writable&&!state.output.is_empty(),egui::Button::new("Checkpoint, close session and save verified backup")).clicked(){operation=Some(Operation::BackupAndClose(state.output.clone().into()));}
            if state.ticket.is_some(){ui.spinner();cancel=ui.button("Cancel transfer after the current write").clicked();}
            ui.label(&state.message);
        });
        self.tools.transfers.open = open;
        if cancel
            && let Some(ticket) = self.tools.transfers.ticket
            && let Some(session) = &self.session
        {
            session.discard_results(ticket.request);
        }
        if let Some(operation) = operation {
            self.transfer_request(operation);
        }
    }
}
