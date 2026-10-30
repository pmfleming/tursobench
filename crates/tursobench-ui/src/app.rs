mod diagram;
mod editing;
mod editor;
mod files;
mod grid;
mod inspector;
#[cfg(test)]
mod interaction_tests;
mod results;
mod schema;
mod shell;
mod smoke;
mod tools;
mod usability;
use eframe::egui;
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session};

#[derive(Debug, Clone, Copy, PartialEq)]
enum View {
    Schema,
    Query,
    Browse,
    Explain,
    Diagnostics,
}
pub struct App {
    path: String,
    tools: tools::Tools,
    ui: shell::UiState,
    smoke: Option<smoke::Smoke>,
    preferences: crate::storage::Preferences,
    store: crate::storage::Store,
    export_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    picker: Option<std::sync::mpsc::Receiver<files::PickResult>>,
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
    schema: Vec<SchemaObject>,
    table: Option<Table>,
    browse: Option<Browse>,
    sql: String,
    params: String,
    status: String,
    activity: std::collections::VecDeque<String>,
    filter: String,
    filter_column: usize,
    inspect: Option<(usize, usize)>,
    transaction: TransactionState,
    drafts: Option<DraftBatch>,
    cell_json: String,
    insert_json: String,
    show_insert: bool,
    show_preview: bool,
    commit_ticket: Option<Ticket>,
    uncertain_edit: bool,
    #[cfg(test)]
    first_cell_rect: Option<egui::Rect>,
}
impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let wake = ctx.clone();
        let (store, preferences, load_error) =
            crate::storage::Store::start(move || wake.request_repaint());
        crate::appearance::theme(ctx, preferences.dark_mode);
        let args: Vec<_> = std::env::args().collect();
        let smoke = args.get(1).is_some_and(|a| a == "--smoke-test");
        Self {
            tools: tools::Tools::default(),
            ui: shell::UiState::new(ctx),
            smoke: smoke.then(smoke::Smoke::new),
            preferences,
            store,
            export_job: None,
            path: args
                .get(if smoke { 2 } else { 1 })
                .cloned()
                .unwrap_or_default(),
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
            activity: std::collections::VecDeque::new(),
            status: load_error.unwrap_or_else(|| {
                "Open a trusted local Turso database. Linux preview; exclusive use only.".into()
            }),
            filter: "{\"type\":\"Text\",\"value\":\"\"}".into(),
            filter_column: 0,
            inspect: None,
            transaction: TransactionState::Autocommit,
            drafts: None,
            cell_json: String::new(),
            insert_json: String::new(),
            show_insert: false,
            show_preview: false,
            commit_ticket: None,
            uncertain_edit: false,
            #[cfg(test)]
            first_cell_rect: None,
        }
    }
    fn open(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() {
            self.status = "Finish choosing a file before opening a database".into();
            return;
        }
        if self.session.is_some() || self.commit_ticket.is_some() || self.closing {
            self.status = "Wait for acknowledged close before opening another database".into();
            return;
        }
        if self.has_drafts() {
            self.status = "Drafts retained after disconnect; inspect the database and discard drafts before reopening".into();
            return;
        }
        let wake = ctx.clone();
        match Session::open_with_settings(
            self.path.clone().into(),
            if self.writable {
                Access::ReadWrite
            } else {
                Access::ReadOnly
            },
            self.exclusive,
            ConnectionSettings {
                busy_timeout_ms: self.preferences.busy_timeout_ms,
            },
            move || wake.request_repaint(),
        ) {
            Ok(s) => {
                self.result = ResultSet::default();
                self.schema.clear();
                self.ui.diagram = Default::default();
                self.tools.ddl = None;
                self.inspect = None;
                self.tools.detail = None;
                self.drafts = None;
                self.tools.inspection = None;
                self.tools.inspection_ticket = None;
                self.tools.related = None;
                self.tools.back.clear();
                let path = std::path::PathBuf::from(&self.path);
                self.tools.database_scope = if path.is_absolute() {
                    self.path.clone()
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                self.table = None;
                self.browse = None;
                self.transaction = TransactionState::Autocommit;
                self.uncertain_edit = false;
                self.session = Some(s);
                self.status = "Opening…".into();
                self.closing = false;
            }
            Err(e) => self.status = e.to_string(),
        }
    }
    fn submit(&mut self, operation: Operation, view: View) -> Option<Ticket> {
        if self.has_drafts() || self.commit_ticket.is_some() {
            self.status =
                "Commit or discard staged edits before navigating or executing SQL".into();
            return None;
        }
        let s = self.session.as_ref()?;
        if !self.ready || self.closing {
            return None;
        }
        self.sequence += 1;
        self.generation += 1;
        let ticket = s.ticket(self.sequence, self.generation);
        match s.submit(Request { ticket, operation }) {
            Ok(()) => {
                if let Some(old) = self.active {
                    s.discard_results(old.request);
                }
                self.result.start(ticket);
                self.active = Some(ticket);
                self.view = view;
                self.ui.workspace = if matches!(view, View::Query | View::Explain) {
                    shell::Workspace::Query
                } else {
                    shell::Workspace::Data
                };
                if view != View::Browse {
                    self.tools.export_table = "exported_rows".into();
                }
                self.inspect = None;
                self.tools.detail = None;
                self.status = "Executing… (discard does not interrupt execution)".into();
                Some(ticket)
            }
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }
    fn run_sql(&mut self) {
        self.run_text(self.sql.clone(), None);
    }
    fn browse_table(&mut self, name: String) {
        if self.has_drafts() || self.commit_ticket.is_some() {
            self.status = "Commit or discard drafts first".into();
            return;
        }
        if !self.ready || self.closing {
            self.status = "Open a database before browsing; retained values are unchanged".into();
            return;
        }
        self.table = None;
        self.tools.ddl = None;
        self.tools.related = None;
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
        if let Some((table, keys)) = self.tools.related.clone() {
            self.submit(Operation::Lookup { table, keys }, View::Browse);
        } else if let Some(b) = self.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn poll(&mut self, ctx: &egui::Context) {
        self.poll_storage(ctx);
        if let Some(rx) = &self.ui.font_rx {
            match rx.try_recv() {
                Ok((fonts, label)) => {
                    ctx.set_fonts(fonts);
                    self.ui.font_label = label;
                    self.ui.font_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.ui.font_rx = None;
                }
                Err(_) => {}
            }
        }
        self.poll_picker();
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
                EventKind::Diagram(graph) if self.ui.diagram.ticket == Some(event.ticket) => {
                    self.ui.diagram.loaded(graph);
                }
                EventKind::Failed(e) if self.ui.diagram.ticket == Some(event.ticket) => {
                    self.ui.diagram.ticket = None;
                    self.ui.diagram.error = e.to_string();
                    self.ui.diagram.stale = true;
                }
                EventKind::EditsCommitted(outcome) if self.commit_ticket == Some(event.ticket) => {
                    self.log_outcome(format!(
                        "Request {} staged commit: {outcome:?}",
                        event.ticket.request
                    ));
                    self.commit_ticket = None;
                    match outcome {
                        CommitOutcome::Committed { operations } => {
                            self.drafts = None;
                            self.show_preview = false;
                            self.inspect = None;
                            self.reload();
                            self.status = format!(
                                "Committed {operations} staged operations; refreshing page"
                            );
                        }
                        CommitOutcome::Rejected(e) => {
                            self.status = format!("No edits committed; drafts retained: {e}")
                        }
                        CommitOutcome::Unknown(e) => {
                            self.uncertain_edit = true;
                            self.status =
                                format!("UNKNOWN outcome; drafts retained. Do not retry: {e}");
                        }
                    }
                }
                EventKind::Opened => {
                    self.ready = true;
                    self.submit(Operation::Schema, View::Schema);
                }
                EventKind::Closed { rollback } => {
                    self.tools.pending_history.clear();
                    self.tools.inspection_ticket = None;
                    self.ui.diagram.ticket = None;
                    self.ui.diagram.stale = true;
                    if rollback.is_err() {
                        self.quit = false;
                        self.transaction = TransactionState::Unknown;
                    }
                    self.status = match rollback {
                        Ok(()) if self.uncertain_edit => "Closed; editor cleanup confirmed, but the earlier staged commit outcome remains UNKNOWN".into(),
                        Ok(()) => "Closed; transaction cleanup confirmed".into(),
                        Err(e) => format!("Close outcome uncertain: {e}"),
                    };
                    self.session = None;
                    self.ready = false;
                    self.active = None;
                    self.closing = false;
                    if self.quit && self.export_job.is_none() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
                EventKind::Inspection(info)
                    if self.tools.inspection_ticket == Some(event.ticket) =>
                {
                    self.tools.inspection = Some(info);
                    self.tools.inspection_ticket = None;
                    self.tools.inspection_error.clear();
                }
                EventKind::Failed(e) if self.tools.inspection_ticket == Some(event.ticket) => {
                    self.tools.inspection_ticket = None;
                    self.tools.inspection_error = e.to_string();
                }
                EventKind::Failed(e) => {
                    self.record_history(event.ticket, None, true);
                    self.log_outcome(format!("Request {} ERROR: {e}", event.ticket.request));
                    if event.ticket.request == 0 {
                        self.status = e.to_string();
                        self.session = None;
                        self.ready = false;
                        self.active = None;
                        self.closing = false;
                        self.confirm_close = false;
                        // Failed opens never send Closed: there is no live session left
                        // to acknowledge a pending close or application exit.
                        if self.quit && self.export_job.is_none() {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    } else if self.active == Some(event.ticket) {
                        self.status = e.to_string();
                        self.active = None;
                    }
                }
                EventKind::Table(t) if self.result.current == Some(event.ticket) => {
                    self.tools.export_table = t.name.clone();
                    self.request_inspection(t.name.clone());
                    self.table = Some(t);
                }
                EventKind::Finished(ref c) => {
                    self.ui.diagram.stale = self.ui.diagram.data.is_some();
                    self.record_history(event.ticket, Some(c.elapsed_ms), c.error.is_some());
                    self.log_outcome(format!(
                        "Request {}: {:?}, {} rows seen, {} changes, editor {:?}{}",
                        event.ticket.request,
                        c.delivery,
                        c.rows_seen,
                        c.changed,
                        c.transaction,
                        c.error
                            .as_ref()
                            .map_or(String::new(), |e| format!("; ERROR {e}"))
                    ));
                    self.transaction = c.transaction;
                    if self.result.current == Some(event.ticket) {
                        self.tools.last_elapsed = c.elapsed_ms;
                        self.status = format!(
                            "{} ms · {:?}: {} rows seen, {} changes; transaction {:?}{}",
                            c.elapsed_ms,
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
                                .filter_map(|r| match (&r[0], &r[1], &r[2], &r[3]) {
                                    (
                                        Value::Text(kind),
                                        Value::Text(name),
                                        Value::Text(table),
                                        ddl,
                                    ) => Some(SchemaObject {
                                        kind: kind.clone(),
                                        name: name.clone(),
                                        table: table.clone(),
                                        ddl: match ddl {
                                            Value::Text(s) => Some(s.clone()),
                                            _ => None,
                                        },
                                    }),
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
            self.uncertain_edit |= self.commit_ticket.take().is_some();
            self.log_outcome(
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into(),
            );
            self.session = None;
            self.ready = false;
            self.closing = false;
            self.quit = false;
            self.active = None;
            self.transaction = TransactionState::Unknown;
            self.tools.pending_history.clear();
            self.tools.inspection_ticket = None;
            self.ui.diagram.ticket = None;
            self.ui.diagram.stale = true;
            self.status =
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into();
        }
    }
    fn log_outcome(&mut self, message: String) {
        self.activity.push_back(message);
        while self.activity.len() > 100 {
            self.activity.pop_front();
        }
    }
    fn request_close(&mut self) {
        if self.commit_ticket.is_some() {
            self.status = "Wait for the staged commit outcome before closing".into();
            return;
        }
        if self.writable && !self.closing {
            self.confirm_close = true;
        } else {
            self.close();
        }
    }
    fn close(&mut self) {
        self.confirm_close = false;
        self.drafts = None;
        self.show_preview = false;
        self.show_insert = false;
        if let Some(s) = &self.session {
            s.close();
            self.closing = true;
            self.status = "Waiting for execution and explicit transaction cleanup…".into();
        }
    }
    pub fn render(&mut self, ui: &mut egui::Ui) {
        self.shell(ui);
    }
    fn browse_controls(&mut self, ui: &mut egui::Ui) {
        if self.has_drafts() || self.commit_ticket.is_some() {
            ui.label("Commit or discard drafts before changing pages, sort, or filters.");
            return;
        }
        if !self.tools.back.is_empty() {
            let trail = self
                .tools
                .back
                .iter()
                .map(|d| match d {
                    tools::Destination::Page(b) => b.table.as_str(),
                    tools::Destination::Related(t, _) => t.as_str(),
                })
                .collect::<Vec<_>>()
                .join(" → ");
            ui.small(format!("Reference trail: {trail}"));
        }
        if !self.tools.back.is_empty() || self.tools.related.is_some() {
            ui.horizontal_wrapped(|ui| {
                if ui
                    .add_enabled(
                        !self.tools.back.is_empty(),
                        egui::Button::new("Back · Alt+Left"),
                    )
                    .clicked()
                {
                    self.navigate_back();
                }
                if let Some((table, _)) = &self.tools.related {
                    ui.label(format!(
                        "Related rows in {table} — bounded lookup, up to 100 rows"
                    ));
                }
                if self.tools.related.is_some() && ui.button("Refresh lookup").clicked() {
                    self.reload();
                }
            });
        }
        let Some(browse) = self.browse.clone() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(format!(
                "Page {} · 100 rows/page · not a snapshot",
                browse.offset / 100 + 1
            ));
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
            ui.horizontal_wrapped(|ui| {
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
                if self.tools.inspection.as_ref().is_some_and(|info| table.columns.get(self.filter_column).is_some_and(|c| c.primary_key != 1 && !info.indexes.iter().any(|index| index.columns.first().and_then(Option::as_ref) == Some(&c.name)))) {
                    ui.label("⚠ Possible scan").on_hover_text("No declared leading index key found for this column. Use EXPLAIN to check the actual plan.");
                }
                ui.label("IS typed JSON:");
                ui.add(egui::TextEdit::singleline(&mut self.filter).desired_width(190.0));
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
        self.result_grid(ui);
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.smoke_tick(ctx);
        if ctx.input(|i| i.viewport().close_requested()) && self.export_job.is_some() {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit = true;
            self.status = "Waiting for the export job before exiting…".into();
            return;
        }
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
        app.ui.workspace = shell::Workspace::Data;
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
        assert!(app.first_cell_rect.is_some());
    }
}
