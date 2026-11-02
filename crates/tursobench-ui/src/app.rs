use tursobench_core::{
    Access, Browse, CommitOutcome, ConnectionSettings, DraftBatch, Event, EventKind, ResultSet,
    SchemaObject, Table, Ticket, TransactionState,
};
mod commands;
mod databases;
mod diagram;
mod editing;
mod editor;
mod files;
mod filters;
mod grid;
mod icons;
mod inspector;
#[cfg(test)]
mod interaction_tests;
mod results;
mod schema;
mod shell;
mod sidebar;
mod smoke;
mod tools;
mod usability;
use eframe::egui;

use tursobench_db::{Operation, Request, Session};

#[derive(Debug, Clone, Copy, PartialEq)]
enum View {
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
    databases: databases::Chooser,
    discard_ticket: Option<Ticket>,
    exclusive: bool,
    writable: bool,
    confirm_close: bool,
    pending_access: Option<bool>,
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
    catalog: ResultSet,
    catalog_ticket: Option<Ticket>,
    catalog_error: String,
    table: Option<Table>,
    browse: Option<Browse>,
    sql: String,
    params: String,
    status: String,
    activity: std::collections::VecDeque<String>,
    filter: String,
    filter_column: usize,
    filter_op: tursobench_core::FilterOp,
    filter_kind: results::CellKind,
    inspect: Option<(usize, usize)>,
    transaction: TransactionState,
    drafts: Option<DraftBatch>,
    insert_json: String,
    show_insert: bool,
    show_preview: bool,
    commit_ticket: Option<Ticket>,
    uncertain_edit: bool,
    #[cfg(test)]
    first_cell_rect: Option<egui::Rect>,
    #[cfg(test)]
    fk_cell_rect: Option<egui::Rect>,
}
impl App {
    pub fn new(ctx: &egui::Context) -> Self {
        let wake = ctx.clone();
        let (store, preferences, load_error) =
            crate::storage::Store::start(move || wake.request_repaint());
        crate::appearance::theme(ctx, preferences.dark_mode);
        let args: Vec<_> = if cfg!(test) {
            vec![]
        } else {
            std::env::args().collect()
        };
        let smoke = args.get(1).is_some_and(|a| a == "--smoke-test");
        let mut app = Self {
            tools: tools::Tools::new(),
            ui: shell::UiState::new(ctx),
            smoke: smoke.then(smoke::Smoke::new),
            databases: databases::Chooser::new(&preferences.discovery),
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
            pending_access: None,
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
            catalog: ResultSet::default(),
            catalog_ticket: None,
            catalog_error: String::new(),
            table: None,
            browse: None,
            sql: "SELECT name, type FROM sqlite_schema ORDER BY name".into(),
            params: "[]".into(),
            activity: std::collections::VecDeque::new(),
            status: load_error.unwrap_or_else(|| {
                "Open a trusted local Turso database. Linux preview; exclusive use only.".into()
            }),
            filter: String::new(),
            filter_column: 0,
            filter_op: Default::default(),
            filter_kind: results::CellKind::Text,
            inspect: None,
            transaction: TransactionState::Autocommit,
            drafts: None,
            insert_json: String::new(),
            show_insert: false,
            show_preview: false,
            commit_ticket: None,
            uncertain_edit: false,
            #[cfg(test)]
            first_cell_rect: None,
            #[cfg(test)]
            fk_cell_rect: None,
        };
        // Tests and desktop smoke runs must never inspect the user's filesystem.
        if !cfg!(test) && !smoke && app.preferences.discovery.enabled {
            app.start_discovery(ctx, None);
        }
        app
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
        self.poll_database_inspection(ctx);
        if !self.databases.inspection.ready(&self.path) {
            self.status = "Checking the selected database header…".into();
            return;
        }
        if !self.databases.inspection.allows_open(&self.path) {
            self.status =
                "Likely SQLite. Select Open with Turso anyway to override this classification."
                    .into();
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
                self.catalog = ResultSet::default();
                self.catalog_ticket = None;
                self.catalog_error.clear();
                self.ui.map_list = false;
                self.ui.diagram = Default::default();
                self.ui.table_view = shell::TableView::Data;
                self.ui.unfolded = None;
                self.tools.counts.clear();
                self.tools.count_request = None;
                self.ui.sql_open = false;
                self.tools.ddl = None;
                self.inspect = None;
                self.tools.detail = None;
                self.drafts = None;
                self.tools.inspection = None;
                self.tools.inspection_ticket = None;
                self.tools.related = None;
                self.tools.back.clear();
                self.tools.result_return = None;
                self.tools.restore_navigation = None;
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
        let previous = self.navigation_entry();
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
                if view != View::Browse
                    && let Some(previous) = previous
                {
                    self.tools.result_return = Some(previous);
                }
                self.tools.restore_navigation = None;
                self.tools.grid_scroll = egui::Vec2::ZERO;
                self.tools.restore_scroll = None;
                self.result.start(ticket);
                self.active = Some(ticket);
                self.view = view;
                self.ui.nav = if view == View::Browse {
                    shell::Nav::Table
                } else {
                    shell::Nav::Result
                };
                self.ui.table_view = shell::TableView::Data;
                self.ui.filter_open = false;
                if matches!(view, View::Query | View::Explain) {
                    self.ui.sql_open = true;
                }
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
        let previous = self.navigation_entry();
        let browse = Browse {
            table: name.clone(),
            offset: 0,
            sort: None,
            descending: false,
            filters: vec![],
        };
        if self
            .submit(Operation::Browse(browse.clone()), View::Browse)
            .is_some()
        {
            if let Some(previous) = previous {
                self.tools.back.push(previous);
                if self.tools.back.len() > 32 {
                    self.tools.back.remove(0);
                }
            }
            self.table = None;
            self.tools.ddl = None;
            self.tools.related = None;
            self.browse = Some(browse);
            self.tools.highlight_column = None;
            self.tools.scroll_to_column = false;
            self.ui.sql_open = false;
            self.ui.unfolded = Some(name.clone());
            self.ui.diagram.selected = Some(name);
            self.ui.diagram.freeze_camera();
        }
    }
    fn reload(&mut self) {
        if let Some(destination) = self.tools.related.clone() {
            self.submit(destination.operation(), View::Browse);
        } else if let Some(b) = self.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn refresh_catalog(&mut self) {
        if !self.ready || self.closing || self.catalog_ticket.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Schema,
        }) {
            Ok(()) => {
                self.catalog.start(ticket);
                self.catalog_ticket = Some(ticket);
                self.catalog_error.clear();
            }
            Err(e) => self.status = e.to_string(),
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
        if self.session.is_none() {
            self.poll_database_inspection(ctx);
        }
        self.poll_discovery(ctx);
        let mut events = Vec::new();
        let mut disconnected = false;
        let polled_session = self.session.as_ref().map(|s| s.id);
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
            self.handle_event(ctx, event);
        }
        if disconnected
            && self
                .session
                .as_ref()
                .is_some_and(|s| Some(s.id) == polled_session)
        {
            self.pending_access = None;
            self.tools.count_request = None;
            self.catalog_ticket = None;
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
    fn handle_event(&mut self, ctx: &egui::Context, event: Event) {
        if self
            .session
            .as_ref()
            .is_none_or(|s| s.id != event.ticket.session)
        {
            return;
        }
        if let Some((_, result)) = &mut self.tools.count_request
            && result.current == Some(event.ticket)
        {
            let finished = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
            if let EventKind::Finished(c) = &event.kind {
                self.transaction = c.transaction;
            }
            let failure = match &event.kind {
                EventKind::Failed(error) => Some(error.to_string()),
                _ => None,
            };
            result.apply(event);
            if finished {
                self.finish_count(failure);
            }
            return;
        }
        if self.catalog_ticket == Some(event.ticket) {
            self.catalog_event(event);
            return;
        }
        if self.discard_ticket == Some(event.ticket)
            && matches!(event.kind, EventKind::Rows(_) | EventKind::Columns(_))
        {
            return;
        }
        match event.kind {
            EventKind::Diagram(graph) if self.ui.diagram.ticket == Some(event.ticket) => {
                let first_load = self.ui.diagram.data.is_none();
                self.ui.diagram.loaded(graph);
                if first_load
                    && let Some(layout) = self
                        .preferences
                        .map_layouts
                        .iter()
                        .find(|l| l.database == self.tools.database_scope)
                {
                    self.ui.diagram.restore(layout);
                }
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
                        self.tools.counts.clear();
                        self.ui.diagram.stale = true;
                        self.drafts = None;
                        self.show_preview = false;
                        self.inspect = None;
                        self.reload();
                        self.status =
                            format!("Committed {operations} staged operations; refreshing page");
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
                self.record_database_open();
                self.refresh_catalog();
                self.ui.nav = shell::Nav::Overview;
                self.load_diagram();
            }
            EventKind::Closed { rollback } => {
                let reopen = if rollback.is_ok() && !self.uncertain_edit && !self.quit {
                    self.pending_access.take()
                } else {
                    self.pending_access = None;
                    None
                };
                self.catalog_ticket = None;
                self.tools.count_request = None;
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
                if let Some(writable) = reopen {
                    self.writable = writable;
                    self.open(ctx);
                }
                if self.quit && self.export_job.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            EventKind::Inspection(info) if self.tools.inspection_ticket == Some(event.ticket) => {
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
                if c.changed > 0 {
                    self.tools.counts.clear();
                }
                if c.changed > 0
                    || self
                        .tools
                        .pending_history
                        .contains_key(&event.ticket.request)
                {
                    self.ui.diagram.stale = self.ui.diagram.data.is_some();
                }
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
                    self.restore_browse_selection();
                }
            }
            _ => {
                self.result.apply(event);
            }
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
        if let Some(layout) = self.ui.diagram.snapshot(&self.tools.database_scope) {
            self.preferences
                .map_layouts
                .retain(|l| l.database != layout.database);
            self.preferences.map_layouts.insert(0, layout);
            self.preferences.normalize();
            self.store.save(&self.preferences);
        }
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
    use tursobench_core::Column;
    use tursobench_core::Value;
    #[test]
    fn renders_empty_and_large_cells_without_engine() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.ui.nav = shell::Nav::Table;
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
