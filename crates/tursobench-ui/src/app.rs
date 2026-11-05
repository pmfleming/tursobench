use tursobench_core::{
    Access, Browse, ConnectionSettings, ResultSet, SchemaObject, Ticket, TransactionState,
};
mod columns;
mod commands;
mod context;
mod databases;
mod diagram;
mod document;
mod editing;
mod editor;
mod events;
mod explain;
mod files;
mod filters;
mod grid;
mod icons;
mod inspector;
#[cfg(test)]
mod interaction_tests;
mod large_values;
mod paging;
mod panels;
mod presentation;
mod recovery;
mod results;
mod ribbon;
mod schema;
mod selection;
mod shell;
mod sidebar;
mod smoke;
pub(crate) mod tiling;
mod tools;
mod transfers;
mod usability;
pub(crate) mod workspace;
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
    recovery: recovery::State,
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
    writable: bool,
    coordination: Option<tursobench_core::Coordination>,
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
    catalog_refresh_pending: bool,
    diagram_refresh_pending: bool,
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
    /// Shared admission for requests that must preserve staged edits and commits.
    fn requests_enabled(&self) -> bool {
        self.ready && !self.closing && !self.has_drafts() && self.commit_ticket.is_none()
    }

    pub(super) fn toggle_theme(&mut self, ctx: &egui::Context) {
        let dark = !ctx.global_style().visuals.dark_mode;
        self.preferences.appearance = Some(crate::appearance::Mode::from_dark(dark));
        self.apply_theme(ctx);
    }
    pub(super) fn apply_theme(&mut self, ctx: &egui::Context) {
        self.appearance.update(
            ctx,
            self.preferences.appearance.unwrap_or_default(),
            self.preferences.font_size,
        );
    }
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
            recovery: recovery::State::new(ctx),
            pending_database: None,
            tools: tools::Tools::default(),
            document: document::Document::default(),
            workspace: Default::default(),
            documents: Default::default(),
            commit_owner: None,
            ui: shell::UiState::new(ctx),
            smoke: smoke.then(smoke::Smoke::new),
            databases: databases::Chooser::new(&preferences.discovery),
            appearance: crate::appearance::Appearance::new(
                ctx,
                !cfg!(test) && (!smoke || std::env::var_os("TURSOBENCH_CAPTURE_DIR").is_some()),
            ),
            preferences,
            store,
            export_job: None,
            path: args
                .get(if smoke { 2 } else { 1 })
                .cloned()
                .unwrap_or_default(),
            picker: None,
            writable: false,
            coordination: None,
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
            catalog_refresh_pending: false,
            diagram_refresh_pending: false,
            activity: std::collections::VecDeque::new(),
            status: load_error.unwrap_or_else(|| {
                "Open a trusted local Turso database. Linux preview; coordinated local access."
                    .into()
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
        if let Some(error) = self.databases.inspection.blocked(&self.path) {
            self.status = error.message.clone();
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
            ConnectionSettings {
                busy_timeout_ms: self.preferences.busy_timeout_ms,
                query_timeout_ms: self.preferences.query_timeout_ms,
            },
            move || wake.request_repaint(),
        ) {
            Ok(s) => {
                self.databases.open_failure = None;
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
            Err(e) => {
                self.status = e.to_string();
                self.databases.open_failure = Some(e);
            }
        }
    }
    fn submit(&mut self, operation: Operation, view: View) -> Option<Ticket> {
        let operation = match operation {
            Operation::Browse(mut spec) => {
                spec.options.page_rows = paging::fetch_rows(self.preferences.row_limit);
                spec.options.page_bytes = paging::PAGE_BYTES;
                Operation::Browse(spec)
            }
            other => other,
        };
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
                self.document.tools.filtered_count = None;
                self.document.script_results.clear();
                self.document.script_message.clear();
                self.document.stale = false;
                self.document.external_refresh = false;
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
                self.status = "Queued for execution…".into();
                Some(ticket)
            }
            Err(e) => {
                self.status = e.to_string();
                None
            }
        }
    }
    fn run_sql(&mut self) {
        self.run_current(None);
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
            options: Default::default(),
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
            if !self.workspace.enabled {
                self.ui.diagram.freeze_camera();
            }
        }
    }
    fn reload(&mut self) {
        if let Some(destination) = &self.document.tools.related {
            self.submit(destination.operation(), View::Browse);
        } else if let Some(b) = self.document.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn refresh_catalog(&mut self) {
        self.tools.counts.clear();
        self.document.tools.context = Default::default();
        if self.catalog_ticket.is_some() {
            self.catalog_refresh_pending = true;
            return;
        }
        self.catalog_refresh_pending = false;
        if !self.ready || self.closing {
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
        self.poll_recovery(ctx);
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
        self.refresh_external_views();
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
        if self.session.is_none() {
            for document in self
                .documents
                .values_mut()
                .chain(std::iter::once(&mut self.document))
            {
                document.disconnect();
            }
        }
        self.load_restored_view(ctx);
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
        self.save_recovery();
        self.save_workspace();
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
        self.large_value_ui(ui.ctx());
        self.transfer_ui(ui.ctx());
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
mod test_ui {
    use eframe::egui;

    pub(super) fn app() -> (egui::Context, App) {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let app = App::new(&ctx);
        (ctx, app)
    }

    pub(super) fn render(
        ctx: &egui::Context,
        size: egui::Vec2,
        events: Vec<egui::Event>,
        draw: impl FnMut(&mut egui::Ui),
    ) -> egui::FullOutput {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            draw,
        );
        output.textures_delta.clear();
        output
    }

    pub(super) fn key_event(
        key: egui::Key,
        pressed: bool,
        modifiers: egui::Modifiers,
    ) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        }
    }

    pub(super) fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        }
    }
    pub(super) fn pointer(pos: egui::Pos2, pressed: bool) -> Vec<egui::Event> {
        vec![egui::Event::PointerMoved(pos), pointer_button(pos, pressed)]
    }

    pub(super) fn nodes(
        output: &egui::FullOutput,
    ) -> &[(egui::accesskit::NodeId, egui::accesskit::Node)] {
        &output
            .platform_output
            .accesskit_update
            .as_ref()
            .expect("accessibility tree")
            .nodes
    }

    pub(super) fn control(output: &egui::FullOutput, name: &str) -> egui::Rect {
        control_from(output, name, f32::NEG_INFINITY)
    }
    pub(super) fn control_from(output: &egui::FullOutput, name: &str, left: f32) -> egui::Rect {
        control_matching(output, name, left, |label| label == name)
    }
    pub(super) fn control_prefix(output: &egui::FullOutput, name: &str) -> egui::Rect {
        control_matching(output, name, f32::NEG_INFINITY, |label| {
            label.starts_with(name)
        })
    }
    fn control_matching(
        output: &egui::FullOutput,
        name: &str,
        left: f32,
        matches: impl Fn(&str) -> bool,
    ) -> egui::Rect {
        let bounds = nodes(output)
            .iter()
            .rev()
            .find_map(|(_, node)| {
                node.bounds().filter(|bounds| {
                    node.label().is_some_and(&matches) && bounds.x0 >= f64::from(left)
                })
            })
            .unwrap_or_else(|| panic!("Missing control: {name}"));
        egui::Rect::from_min_max(
            egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
            egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
        )
    }
    use super::{App, shell};
    use std::time::{Duration, Instant};
    pub(super) fn inspect_selected(app: &mut App, ctx: &egui::Context) {
        let start = Instant::now();
        loop {
            app.poll_database_inspection(ctx);
            if app.databases.inspection.ready(&app.path) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    use tursobench_core::Column;
    use tursobench_core::Value;
    #[test]
    fn renders_empty_and_large_cells_without_engine() {
        let (ctx, mut app) = crate::app::test_ui::app();
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
