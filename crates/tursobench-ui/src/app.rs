use tursobench_core::{
    Access, Browse, CommitOutcome, ConnectionSettings, Event, EventKind, ResultSet, SchemaObject,
    Ticket, TransactionState,
};
mod columns;
mod commands;
mod context;
mod databases;
mod diagram;
mod document;
mod editing;
mod editor;
mod files;
mod filters;
mod grid;
mod icons;
mod inspector;
#[cfg(test)]
mod interaction_tests;
mod paging;
mod results;
mod ribbon;
mod schema;
mod selection;
mod shell;
mod sidebar;
mod smoke;
mod tiling;
mod tools;
mod usability;
mod workspace;
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
    pending_database: Option<std::path::PathBuf>,
    path: String,
    tools: tools::Tools,
    document: document::Document,
    workspace: workspace::Workspace,
    documents: std::collections::BTreeMap<u64, document::Document>,
    commit_owner: Option<u64>,
    ui: shell::UiState,
    smoke: Option<smoke::Smoke>,
    preferences: crate::storage::Preferences,
    appearance: crate::appearance::Appearance,
    store: crate::storage::Store,
    export_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    picker: Option<std::sync::mpsc::Receiver<files::PickResult>>,
    databases: databases::Chooser,
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
    schema: Vec<SchemaObject>,
    catalog: ResultSet,
    catalog_ticket: Option<Ticket>,
    catalog_error: String,
    status: String,
    activity: std::collections::VecDeque<String>,
    transaction: TransactionState,
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

        let args: Vec<_> = if cfg!(test) {
            vec![]
        } else {
            std::env::args().collect()
        };
        let smoke = args.get(1).is_some_and(|a| a == "--smoke-test");
        let mut app = Self {
            pending_database: None,
            tools: tools::Tools::new(),
            document: document::Document::default(),
            workspace: Default::default(),
            documents: Default::default(),
            commit_owner: None,
            ui: shell::UiState::new(ctx),
            smoke: smoke.then(smoke::Smoke::new),
            databases: databases::Chooser::new(&preferences.discovery),
            appearance: crate::appearance::Appearance::new(ctx, !cfg!(test) && !smoke),
            preferences,
            store,
            export_job: None,
            path: args
                .get(if smoke { 2 } else { 1 })
                .cloned()
                .unwrap_or_default(),
            picker: None,
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
            schema: vec![],
            catalog: ResultSet::default(),
            catalog_ticket: None,
            catalog_error: String::new(),
            activity: std::collections::VecDeque::new(),
            status: load_error.unwrap_or_else(|| {
                "Open a trusted local Turso database. Linux preview; exclusive use only.".into()
            }),
            transaction: TransactionState::Autocommit,
            commit_ticket: None,
            uncertain_edit: false,
            #[cfg(test)]
            first_cell_rect: None,
            #[cfg(test)]
            fk_cell_rect: None,
        };
        app.apply_theme(ctx);
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
                self.documents.clear();
                self.workspace = Default::default();
                self.document = document::Document::default();
                self.commit_owner = None;
                self.document.result = ResultSet::default();
                self.schema.clear();
                self.catalog = ResultSet::default();
                self.catalog_ticket = None;
                self.catalog_error.clear();
                self.ui.diagram = Default::default();
                self.document.ui.table_view = shell::TableView::Data;
                self.ui.unfolded = None;
                self.document.ui.inspector_initialized = false;
                self.tools.counts.clear();
                self.tools.count_request = None;
                self.document.ui.sql_open = false;
                self.document.tools.ddl = None;
                self.document.inspect = None;
                self.document.tools.detail = None;
                self.document.drafts = None;
                self.document.tools.inspection = None;
                self.document.tools.inspection_ticket = None;
                self.document.tools.related = None;
                self.tools.back.clear();
                self.tools.result_return = None;
                self.document.tools.restore_navigation = None;
                let path = std::path::PathBuf::from(&self.path);
                self.tools.database_scope = if path.is_absolute() {
                    self.path.clone()
                } else {
                    std::env::current_dir()
                        .map(|cwd| cwd.join(path).to_string_lossy().into_owned())
                        .unwrap_or_default()
                };
                self.document.table = None;
                self.document.browse = None;
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
        if self.workspace.enabled
            && view != View::Browse
            && self.document.kind != tiling::Kind::Result
        {
            self.create_window(tiling::Kind::Result, None, Some(self.document.id), true)?;
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
                if let Some(old) = self.document.active {
                    s.discard_results(old.request);
                }
                if view != View::Browse
                    && let Some(previous) = previous
                {
                    self.tools.result_return = Some(previous);
                }
                self.document.tools.restore_navigation = None;
                self.document.tools.grid_scroll = egui::Vec2::ZERO;
                self.document.tools.restore_scroll = None;
                self.document.tools.context = Default::default();
                self.document.tools.pages = Default::default();
                self.document.tools.grid_selection = None;
                self.document.tools.inline = None;
                self.document.result.start(ticket);
                self.document.active = Some(ticket);
                self.document.view = view;
                self.document.ui.nav = if view == View::Browse {
                    shell::Nav::Table
                } else {
                    shell::Nav::Result
                };
                self.document.ui.table_view = shell::TableView::Data;
                self.document.ui.filter_open = false;
                if matches!(view, View::Query | View::Explain) {
                    self.document.ui.sql_open = true;
                }
                if view != View::Browse {
                    self.document.tools.export_table = "exported_rows".into();
                }
                self.document.inspect = None;
                self.document.tools.detail = None;
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
        self.run_text(self.document.sql.clone(), None);
    }
    fn browse_table(&mut self, name: String) {
        if self.workspace.enabled
            && let Some(id) = self.find_window(tiling::Kind::Data, Some(&name), None)
        {
            // Focusing retained data is allowed even while that same view owns staged edits.
            if !self.has_drafts() || self.document_ref(id).is_some_and(|d| d.has_drafts()) {
                self.focus_window(id);
                self.workspace.expanded = None;
                return;
            }
        }
        if self.has_drafts() || self.commit_ticket.is_some() {
            self.status = "Commit or discard drafts first".into();
            return;
        }
        if !self.ready || self.closing {
            self.status = "Open a database before browsing; retained values are unchanged".into();
            return;
        }
        let previous = self.navigation_entry();
        if self.workspace.enabled {
            if let Some(id) = self.find_window(tiling::Kind::Data, Some(&name), None) {
                self.focus_window(id);
                self.workspace.expanded = None;
                return;
            }
            if self
                .create_window(tiling::Kind::Data, Some(name.clone()), None, false)
                .is_none()
            {
                return;
            }
        }
        let browse = Browse {
            table: name.clone(),
            offset: 0,
            sort: None,
            descending: false,
            filters: vec![],
        };
        self.document.browse = Some(browse.clone());
        self.document.view = View::Browse;
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
            self.document.table = None;
            self.document.tools.ddl = None;
            self.document.tools.related = None;
            self.document.browse = Some(browse);
            self.document.tools.highlight_column = None;
            self.document.tools.scroll_to_column = false;
            self.document.ui.sql_open = false;
            self.ui.unfolded = Some(name.clone());
            self.ui.diagram.selected = Some(name);
            self.ui.diagram.freeze_camera();
        }
    }
    fn reload(&mut self) {
        if let Some(destination) = self.document.tools.related.clone() {
            self.submit(destination.operation(), View::Browse);
        } else if let Some(b) = self.document.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn refresh_catalog(&mut self) {
        self.tools.counts.clear();
        self.document.tools.context = Default::default();
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
        self.apply_theme(ctx);
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
            self.document.active = None;
            self.transaction = TransactionState::Unknown;
            self.tools.pending_history.clear();
            self.document.tools.inspection_ticket = None;
            self.ui.diagram.ticket = None;
            self.ui.diagram.stale = true;
            self.status =
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into();
        }
    }
    fn handle_event(&mut self, ctx: &egui::Context, event: Event) {
        let previous = self.document.id;
        if let Some(owner) = self.document_owner(event.ticket) {
            self.switch_document(owner);
        }
        self.handle_document_event(ctx, event);
        self.switch_document(previous);
    }
    fn handle_document_event(&mut self, ctx: &egui::Context, event: Event) {
        if self
            .session
            .as_ref()
            .is_none_or(|s| s.id != event.ticket.session)
        {
            return;
        }
        if self
            .document
            .tools
            .context
            .pending
            .as_ref()
            .is_some_and(|p| p.result.current == Some(event.ticket))
        {
            self.context_event(event);
            return;
        }
        if self
            .document
            .tools
            .pages
            .pending
            .as_ref()
            .is_some_and(|p| p.result.current == Some(event.ticket))
        {
            self.page_event(event);
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
        if self.document.discard_ticket == Some(event.ticket)
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
                self.commit_owner = None;
                match outcome {
                    CommitOutcome::Committed { operations } => {
                        self.tools.counts.clear();
                        self.document.tools.context = Default::default();
                        self.ui.diagram.stale = true;
                        self.document.drafts = None;
                        self.document.show_preview = false;
                        self.document.inspect = None;
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
                self.workspace.enabled = true;
                self.workspace.tree = Some(tiling::Node::leaf(self.document.id));
                self.workspace.focused = self.document.id;
                self.document.kind = tiling::Kind::Map;
                self.ready = true;
                self.record_database_open();
                self.refresh_catalog();
                self.document.ui.nav = shell::Nav::Overview;
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
                self.document.tools.inspection_ticket = None;
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
                self.document.ui.nav = shell::Nav::Databases;
                self.ready = false;
                self.document.active = None;
                self.closing = false;
                if let Some(path) = self.pending_database.take().filter(|_| {
                    self.transaction != TransactionState::Unknown && !self.uncertain_edit
                }) {
                    self.set_database_path(path.to_string_lossy().into_owned());
                    self.document.ui.nav = shell::Nav::Databases;
                }
                if let Some(writable) = reopen {
                    self.writable = writable;
                    self.open(ctx);
                }
                if self.quit && self.export_job.is_none() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            }
            EventKind::Inspection(info)
                if self.document.tools.inspection_ticket == Some(event.ticket) =>
            {
                self.document.tools.inspection = Some(info);
                self.document.tools.inspection_ticket = None;
                self.document.tools.inspection_error.clear();
            }
            EventKind::Failed(e) if self.document.tools.inspection_ticket == Some(event.ticket) => {
                self.document.tools.inspection_ticket = None;
                self.document.tools.inspection_error = e.to_string();
            }
            EventKind::Failed(e) => {
                self.record_history(event.ticket, None, true);
                self.log_outcome(format!("Request {} ERROR: {e}", event.ticket.request));
                if event.ticket.request == 0 {
                    self.status = e.to_string();
                    self.session = None;
                    self.ready = false;
                    self.document.active = None;
                    self.closing = false;
                    self.confirm_close = false;
                    // Failed opens never send Closed: there is no live session left
                    // to acknowledge a pending close or application exit.
                    if self.quit && self.export_job.is_none() {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                } else if self.document.active == Some(event.ticket) {
                    self.status = e.to_string();
                    self.document.active = None;
                }
            }
            EventKind::Table(t) if self.document.result.current == Some(event.ticket) => {
                self.document.tools.export_table = t.name.clone();
                self.request_inspection(t.name.clone());
                self.document.table = Some(t);
            }
            EventKind::Finished(ref c) => {
                if c.changed > 0 {
                    self.document.tools.context = Default::default();
                    self.tools.counts.clear();
                }
                if c.changed > 0
                    || self
                        .tools
                        .pending_history
                        .contains_key(&event.ticket.request)
                {
                    self.ui.diagram.stale = self.ui.diagram.data.is_some();
                    self.document.tools.context = Default::default();
                    self.tools.counts.clear();
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
                if self.document.result.current == Some(event.ticket) {
                    self.document.tools.last_elapsed = c.elapsed_ms;
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
                    self.document.active = None;
                    self.document.result.apply(event);
                    if self.document.view == View::Browse
                        && let Some(browse) = &self.document.browse
                        && self.document.result.rows.len() < 100
                        && self.document.result.completion.as_ref().is_some_and(|c| {
                            c.delivery == tursobench_core::Delivery::Complete && c.error.is_none()
                        })
                    {
                        self.document.tools.pages.end =
                            Some(browse.offset + self.document.result.rows.len());
                    }
                    self.restore_browse_selection();
                }
            }
            _ => {
                self.document.result.apply(event);
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
            self.pending_database = None;
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
        for document in self.documents.values_mut() {
            document.drafts = None;
            document.show_preview = false;
            document.show_insert = false;
        }
        self.document.drafts = None;
        self.document.show_preview = false;
        self.document.show_insert = false;
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
        app.document.ui.nav = shell::Nav::Table;
        app.document.result.columns = vec![Column {
            name: "data".into(),
            declared_type: None,
        }];
        app.document.result.rows = vec![
            vec![Value::Text("λ".repeat(30000))],
            vec![Value::Blob(vec![0; 64000])],
        ];
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| app.render(ui));
        output.textures_delta.clear();
        assert!(app.session.is_none());
        assert!(app.document.active.is_none());
        assert!(app.first_cell_rect.is_some());
    }
}
