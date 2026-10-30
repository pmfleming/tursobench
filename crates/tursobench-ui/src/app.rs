use eframe::egui;
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session};

#[derive(Clone, Copy, PartialEq)]
enum View {
    Schema,
    Query,
    Browse,
}
pub struct App {
    path: String,
    picker: Option<std::sync::mpsc::Receiver<Option<std::path::PathBuf>>>,
    discard_ticket: Option<Ticket>,
    exclusive: bool,
    writable: bool,
    confirm_close: bool,
    session: Option<Session>,
    ready: bool,
    closing: bool,
    quit: bool,
    sequence: u64,
    generation: u64,
    active: Option<Ticket>,
    view: View,
    result: ResultSet,
    schema: Vec<(String, String)>,
    table: Option<Table>,
    browse: Option<Browse>,
    sql: String,
    params: String,
    status: String,
    filter: String,
    filter_column: usize,
    inspect: Option<(usize, usize)>,
    transaction: TransactionState,
}
impl App {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self {
            path: std::env::args().nth(1).unwrap_or_default(),
            picker: None,
            discard_ticket: None,
            exclusive: false,
            writable: false,
            confirm_close: false,
            session: None,
            ready: false,
            closing: false,
            quit: false,
            sequence: 0,
            generation: 0,
            active: None,
            view: View::Query,
            result: ResultSet::default(),
            schema: vec![],
            table: None,
            browse: None,
            sql: "SELECT name, type FROM sqlite_schema ORDER BY name".into(),
            params: "[]".into(),
            status: "Open a trusted local Turso database. Linux preview; exclusive use only."
                .into(),
            filter: "{\"type\":\"Text\",\"value\":\"\"}".into(),
            filter_column: 0,
            inspect: None,
            transaction: TransactionState::Autocommit,
        }
    }
    fn open(&mut self, ctx: &egui::Context) {
        let wake = ctx.clone();
        match Session::open(
            self.path.clone().into(),
            if self.writable {
                Access::ReadWrite
            } else {
                Access::ReadOnly
            },
            self.exclusive,
            move || wake.request_repaint(),
        ) {
            Ok(s) => {
                self.result = ResultSet::default();
                self.schema.clear();
                self.table = None;
                self.browse = None;
                self.transaction = TransactionState::Autocommit;
                self.session = Some(s);
                self.status = "Opening…".into();
                self.closing = false;
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn submit(&mut self, operation: Operation, view: View) {
        let Some(s) = &self.session else {
            return;
        };
        if !self.ready || self.closing {
            return;
        }
        if let Some(t) = self.active {
            s.discard_results(t.request);
        }
        self.sequence += 1;
        self.generation += 1;
        let ticket = s.ticket(self.sequence, self.generation);
        match s.submit(Request { ticket, operation }) {
            Ok(()) => {
                self.result.start(ticket);
                self.active = Some(ticket);
                self.view = view;
                self.inspect = None;
                self.status = "Executing… (discard does not interrupt execution)".into();
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn run_sql(&mut self) {
        match serde_json::from_str::<Vec<Value>>(&self.params) {
            Ok(parameters) => {
                self.table = None;
                self.browse = None;
                self.submit(
                    Operation::Execute {
                        sql: self.sql.clone(),
                        parameters,
                        budget: Budget::default(),
                    },
                    View::Query,
                );
            }
            Err(e) => self.status = format!("Typed parameter JSON: {e}"),
        }
    }
    fn browse_table(&mut self, name: String) {
        self.table = None;
        self.browse = Some(Browse {
            table: name,
            offset: 0,
            sort: None,
            descending: false,
            filter: None,
        });
        self.reload();
    }
    fn reload(&mut self) {
        if let Some(b) = self.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.picker {
            match rx.try_recv() {
                Ok(path) => {
                    if let Some(path) = path {
                        self.path = path.to_string_lossy().into_owned();
                    }
                    self.picker = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => self.picker = None,
                Err(_) => {}
            }
        }
        let mut events = Vec::new();
        let mut disconnected = false;
        if let Some(s) = &self.session {
            loop {
                match s.events.try_recv() {
                    Ok(e) => events.push(e),
                    Err(std::sync::mpsc::TryRecvError::Empty) => break,
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        disconnected = true;
                        break;
                    }
                }
            }
        }
        for event in events {
            if self
                .session
                .as_ref()
                .is_none_or(|s| s.id != event.ticket.session)
            {
                continue;
            }
            if self.discard_ticket == Some(event.ticket)
                && matches!(event.kind, EventKind::Rows(_) | EventKind::Columns(_))
            {
                continue;
            }
            match event.kind {
                EventKind::Opened => {
                    self.ready = true;
                    self.submit(Operation::Schema, View::Schema);
                }
                EventKind::Closed { rollback } => {
                    if rollback.is_err() {
                        self.quit = false;
                        self.transaction = TransactionState::Unknown;
                    }
                    self.status = match rollback {
                        Ok(()) => "Closed; transaction cleanup confirmed".into(),
                        Err(e) => format!("Close outcome uncertain: {e}"),
                    };
                    self.session = None;
                    self.ready = false;
                    self.active = None;
                    self.closing = false;
                    if self.quit {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                EventKind::Failed(e) => {
                    self.status = e.to_string();
                    if event.ticket.request == 0 {
                        self.session = None;
                        self.ready = false;
                    } else if self.active == Some(event.ticket) {
                        self.active = None;
                    }
                }
                EventKind::Table(t) if self.result.current == Some(event.ticket) => {
                    self.table = Some(t);
                }
                EventKind::Finished(ref c) => {
                    self.transaction = c.transaction;
                    if self.result.current == Some(event.ticket) {
                        self.status = format!(
                            "{:?}: {} rows seen, {} changes; transaction {:?}{}",
                            c.delivery,
                            c.rows_seen,
                            c.changed,
                            c.transaction,
                            c.error
                                .as_ref()
                                .map_or(String::new(), |e| format!("; ERROR {e}"))
                        );
                        self.active = None;
                        self.result.apply(event);
                        if self.view == View::Schema {
                            self.schema = self
                                .result
                                .rows
                                .iter()
                                .filter_map(|r| match (&r[0], &r[1]) {
                                    (Value::Text(kind), Value::Text(name))
                                        if kind == "table" || kind == "view" =>
                                    {
                                        Some((kind.clone(), name.clone()))
                                    }
                                    _ => None,
                                })
                                .collect();
                        }
                    }
                }
                _ => {
                    self.result.apply(event);
                }
            }
        }
        if disconnected && self.session.is_some() {
            self.session = None;
            self.ready = false;
            self.closing = false;
            self.quit = false;
            self.active = None;
            self.transaction = TransactionState::Unknown;
            self.status =
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into();
        }
    }
    fn request_close(&mut self) {
        if self.writable && !self.closing {
            self.confirm_close = true;
        } else {
            self.close();
        }
    }
    fn close(&mut self) {
        self.confirm_close = false;
        if let Some(s) = &self.session {
            s.close();
            self.closing = true;
            self.status = "Waiting for execution and explicit transaction cleanup…".into();
        }
    }
    pub fn render(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("connection").show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.heading("TursoBench");
                ui.label(if self.writable {
                    "Turso 0.8.3 · READ / WRITE"
                } else {
                    "Turso 0.8.3 · READ ONLY"
                });
                if ui
                    .add_enabled(
                        self.session.is_none() && self.picker.is_none(),
                        egui::Button::new("Choose file…"),
                    )
                    .clicked()
                {
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    self.picker = Some(rx);
                    let wake = ctx.clone();
                    std::thread::spawn(move || {
                        let path = rfd::FileDialog::new()
                            .set_title("Open existing Turso database")
                            .pick_file();
                        let _ = tx.send(path);
                        wake.request_repaint();
                    });
                }
                ui.add_enabled(
                    self.session.is_none(),
                    egui::TextEdit::singleline(&mut self.path)
                        .hint_text("Existing database path")
                        .desired_width(400.0),
                );
                if ui
                    .add_enabled(
                        self.session.is_none() && self.exclusive && !self.path.is_empty(),
                        egui::Button::new("Open"),
                    )
                    .clicked()
                {
                    self.open(&ctx);
                }
                if ui
                    .add_enabled(
                        self.session.is_some() && !self.closing,
                        egui::Button::new("Close"),
                    )
                    .clicked()
                {
                    self.request_close();
                }
            });
            ui.add_enabled(
                self.session.is_none(),
                egui::Checkbox::new(
                    &mut self.writable,
                    "Enable read/write access (close before changing mode)",
                ),
            );
            ui.checkbox(
                &mut self.exclusive,
                "I confirm this file is not in use by another application (including SQLite tools)",
            );
            if self.active.is_some() && ui.button("Discard results (execution continues)").clicked()
            {
                if let (Some(s), Some(t)) = (&self.session, self.active) {
                    s.discard_results(t.request);
                }
                self.discard_ticket = self.active;
                self.result.rows.clear();
                self.status = "Discard requested; writes are NOT cancelled or rolled back".into();
            }
        });
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.label(&self.status);
        });
        egui::Panel::left("schema").default_size(200.0).show(ui, |ui| {
            ui.heading("Schema");
            if ui.add_enabled(self.ready && !self.closing, egui::Button::new("Refresh schema")).clicked() { self.submit(Operation::Schema, View::Schema); }
            egui::ScrollArea::vertical().show(ui, |ui| {
                for (kind, name) in self.schema.clone() { if ui.button(format!("{kind}: {name}")).clicked() { self.browse_table(name); } }
            });
            ui.small("Browsing uses a separate connection. Uncommitted editor changes are not visible.");
        });
        egui::CentralPanel::default().show(ui, |ui| {
            ui.heading("SQL editor — one statement");
            ui.add(egui::TextEdit::multiline(&mut self.sql).code_editor().desired_rows(4).desired_width(f32::INFINITY));
            ui.horizontal(|ui| {
                ui.label("Typed parameters (JSON array):"); ui.text_edit_singleline(&mut self.params);
                if ui.add_enabled(self.ready && !self.closing, egui::Button::new("Run SQL")).clicked() { self.run_sql(); }
            });
            ui.small("Example: [{\"type\":\"Integer\",\"value\":42}, {\"type\":\"Null\"}]. Bind as ?1, ?2.");
            ui.horizontal(|ui| {
                ui.label(format!("Editor transaction: {:?}", self.transaction));
                for (label, sql) in [("BEGIN", "BEGIN"), ("COMMIT", "COMMIT"), ("ROLLBACK", "ROLLBACK")] {
                    if ui.add_enabled(self.ready && self.active.is_none() && !self.closing, egui::Button::new(label)).clicked() {
                        self.submit(Operation::Execute { sql: sql.into(), parameters: vec![], budget: Budget::default() }, View::Query);
                    }
                }
            });
            ui.separator();
            if self.view == View::Browse { self.browse_controls(ui); }
            self.grid(ui);
        });
        if self.confirm_close {
            egui::Window::new("Close writable database?").collapsible(false).show(&ctx, |ui| {
                ui.label("Active execution will finish. Any remaining editor transaction will be explicitly rolled back. Discard does not undo committed writes.");
                if ui.button("Wait, roll back transaction, and close").clicked() { self.close(); }
                if ui.button("Keep open").clicked() { self.confirm_close = false; self.quit = false; }
            });
        }
        if let Some((row, col)) = self.inspect {
            let mut open = true;
            egui::Window::new("Cell inspector — typed value")
                .open(&mut open)
                .show(&ctx, |ui| {
                    if let Some(v) = self.result.rows.get(row).and_then(|r| r.get(col)) {
                        let mut text = serde_json::to_string_pretty(v).unwrap_or_default();
                        egui::ScrollArea::both().max_height(400.0).show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut text)
                                    .code_editor()
                                    .desired_width(600.0),
                            );
                        });
                        ui.small("Read-only inspector. Blob bytes and NULL retain their types.");
                    }
                });
            if !open {
                self.inspect = None;
            }
        }
    }
    fn browse_controls(&mut self, ui: &mut egui::Ui) {
        let Some(browse) = self.browse.clone() else {
            return;
        };
        ui.label(format!(
            "{} — page {} (100 rows/page; not a snapshot)",
            browse.table,
            browse.offset / 100 + 1
        ));
        ui.horizontal(|ui| {
            if ui
                .add_enabled(browse.offset > 0, egui::Button::new("Previous"))
                .clicked()
            {
                self.browse.as_mut().unwrap().offset =
                    self.browse.as_ref().unwrap().offset.saturating_sub(100);
                self.reload();
            }
            if ui.button("Next").clicked() {
                self.browse.as_mut().unwrap().offset =
                    (self.browse.as_ref().unwrap().offset + 100).min(1_000_000);
                self.reload();
            }
            if ui.button("Refresh page").clicked() {
                self.reload();
            }
        });
        if let Some(table) = self.table.clone() {
            ui.horizontal(|ui| {
                egui::ComboBox::from_id_salt("filter-column")
                    .selected_text(
                        table
                            .columns
                            .get(self.filter_column)
                            .map_or("Column", |c| &c.name),
                    )
                    .show_ui(ui, |ui| {
                        for (i, c) in table.columns.iter().enumerate() {
                            ui.selectable_value(&mut self.filter_column, i, &c.name);
                        }
                    });
                ui.label("IS typed JSON:");
                ui.text_edit_singleline(&mut self.filter);
                if ui.button("Filter").clicked() {
                    match serde_json::from_str::<Value>(&self.filter) {
                        Ok(v) => {
                            let b = self.browse.as_mut().unwrap();
                            b.filter = Some((self.filter_column, v));
                            b.offset = 0;
                            self.reload();
                        }
                        Err(e) => self.status = e.to_string(),
                    }
                }
                if ui.button("Clear filter").clicked() {
                    let b = self.browse.as_mut().unwrap();
                    b.filter = None;
                    b.offset = 0;
                    self.reload();
                }
            });
        }
    }
    fn grid(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::horizontal().show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, c) in self.result.columns.clone().iter().enumerate() {
                    if ui
                        .add_sized([180.0, 24.0], egui::Button::new(&c.name))
                        .clicked()
                        && self.view == View::Browse
                    {
                        if let Some(b) = self.browse.as_mut() {
                            b.descending = b.sort == Some(i) && !b.descending;
                            b.sort = Some(i);
                            b.offset = 0;
                        }
                        self.reload();
                    }
                }
            });
            egui::ScrollArea::vertical().show_rows(
                ui,
                26.0,
                self.result.rows.len(),
                |ui, range| {
                    for row in range {
                        ui.horizontal(|ui| {
                            for (col, value) in self.result.rows[row].iter().enumerate() {
                                if ui
                                    .add_sized(
                                        [180.0, 24.0],
                                        egui::Button::new(value.preview()).truncate(),
                                    )
                                    .clicked()
                                {
                                    self.inspect = Some((row, col));
                                }
                            }
                        });
                    }
                },
            );
        });
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.session.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit = true;
            self.request_close();
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render(ui);
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn renders_empty_and_large_cells_without_engine() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.result.columns = vec![Column {
            name: "data".into(),
            declared_type: None,
        }];
        app.result.rows = vec![
            vec![Value::Text("λ".repeat(30000))],
            vec![Value::Blob(vec![0; 64000])],
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        assert!(app.session.is_none());
        assert!(app.active.is_none());
    }
}
