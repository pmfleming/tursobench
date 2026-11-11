use tursobench_core::{Access, Browse, ConnectionSettings, Ticket, TransactionState};
mod checks;
mod columns;
mod commands;
mod context;
mod database_navigation;
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
mod paging;
mod panels;
mod presentation;
mod recovery;
mod results;
mod ribbon;
mod schema;
mod schema_compare;
mod selection;
mod sessions;
mod settings;
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
use sessions::DatabaseWorkspace;
pub struct App {
    settings: settings::State,
    comparison: schema_compare::State,
    database: DatabaseWorkspace,
    parked: std::collections::BTreeMap<u64, DatabaseWorkspace>,
    next_workspace: u64,
    recovery_store: crate::recovery::Store,
    polling_background: bool,
    sidebar_search: database_navigation::Search,
    smoke: Option<smoke::Smoke>,
    preferences: crate::storage::Preferences,
    memory: crate::resources::MemorySnapshot,
    appearance: crate::appearance::Appearance,
    store: crate::storage::Store,
    picker: Option<std::sync::mpsc::Receiver<files::PickResult>>,
    databases: databases::Chooser,
    quit: bool,
    #[cfg(test)]
    first_cell_rect: Option<egui::Rect>,
    #[cfg(test)]
    fk_cell_rect: Option<egui::Rect>,
}

impl App {
    /// Shared admission for requests that must preserve staged edits and commits.
    fn requests_enabled(&self) -> bool {
        self.database.ready
            && !self.database.closing
            && !self.has_drafts()
            && self.database.commit_ticket.is_none()
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
        let recovery_wake = ctx.clone();
        let mut app = Self {
            settings: Default::default(),
            comparison: Default::default(),
            database: DatabaseWorkspace::new(ctx, 1),
            parked: Default::default(),
            next_workspace: 2,
            recovery_store: crate::recovery::Store::start(move || recovery_wake.request_repaint()),
            polling_background: false,
            sidebar_search: database_navigation::Search {
                focus: true,
                ..Default::default()
            },
            smoke: smoke.then(smoke::Smoke::new),
            databases: databases::Chooser::new(&preferences.discovery),
            appearance: crate::appearance::Appearance::new(
                ctx,
                !cfg!(test) && (!smoke || std::env::var_os("TURSOBENCH_CAPTURE_DIR").is_some()),
            ),
            preferences,
            memory: if cfg!(test) {
                crate::resources::MemorySnapshot {
                    total_mib: Some(32768),
                    available_mib: Some(16384),
                    process_limited: false,
                }
            } else {
                crate::resources::MemorySnapshot::detect()
            },
            store,
            picker: None,
            quit: false,
            #[cfg(test)]
            first_cell_rect: None,
            #[cfg(test)]
            fk_cell_rect: None,
        };
        app.database.path = args
            .get(if smoke { 2 } else { 1 })
            .cloned()
            .unwrap_or_default();
        app.database.status = load_error.unwrap_or_else(|| {
            "Open a trusted local Turso database. Linux preview; coordinated local access.".into()
        });
        app.apply_theme(ctx);
        // Tests and desktop smoke runs must never inspect the user's filesystem.
        if !cfg!(test) && !smoke && app.preferences.discovery.enabled {
            app.start_discovery(ctx, None);
        }
        app
    }
    fn open(&mut self, ctx: &egui::Context) {
        self.open_with_slot(ctx, false);
    }
    // An acknowledged access-mode reopen reuses the existing workspace's slot.
    fn open_with_slot(&mut self, ctx: &egui::Context, retained_slot: bool) {
        if self.database.session.is_none()
            && let Some(id) = self.duplicate_workspace(std::path::Path::new(&self.database.path))
        {
            self.select_workspace(id);
            return;
        }
        if self.picker.is_some() {
            self.database.status = "Finish choosing a file before opening a database".into();
            return;
        }
        if self.database.session.is_some()
            || self.database.commit_ticket.is_some()
            || self.database.closing
        {
            self.database.status =
                "Wait for acknowledged close before opening another database".into();
            return;
        }
        if self.has_drafts() {
            self.database.status = "Drafts retained after disconnect; inspect the database and discard drafts before reopening".into();
            return;
        }
        if !retained_slot && !self.database_admission_available() {
            self.database.status = self.database_limit_message();
            return;
        }
        self.poll_database_inspection(ctx);
        if !self.databases.inspection.ready(&self.database.path) {
            self.database.status = "Checking the selected database header…".into();
            return;
        }
        if let Some(error) = self.databases.inspection.blocked(&self.database.path) {
            self.database.status = error.message.clone();
            return;
        }
        if !self.databases.inspection.allows_open(&self.database.path) {
            self.database.status =
                "Likely SQLite. Select Open with Turso anyway to override this classification."
                    .into();
            return;
        }
        let wake = ctx.clone();
        match Session::open_with_settings(
            self.database.path.clone().into(),
            if self.database.writable {
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
                self.database.start_session(s);
            }
            Err(e) => {
                self.database.status = e.to_string();
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
        if self.has_drafts() || self.database.commit_ticket.is_some() {
            self.database.status =
                "Commit or discard staged edits before navigating or executing SQL".into();
            return None;
        }
        let previous = self.navigation_entry();
        if self.database.workspace.enabled
            && view != View::Browse
            && self.database.document.kind != tiling::Kind::Result
        {
            self.create_window(
                tiling::Kind::Result,
                None,
                Some(self.database.document.id),
                true,
            )?;
        }
        let s = self.database.session.as_ref()?;
        if !self.database.ready || self.database.closing {
            return None;
        }
        self.database.sequence += 1;
        self.database.generation += 1;
        let ticket = s.ticket(self.database.sequence, self.database.generation);
        match s.submit(Request { ticket, operation }) {
            Ok(()) => {
                if let Some(old) = self.database.document.active {
                    s.discard_results(old.request);
                }
                if view != View::Browse
                    && let Some(previous) = previous
                {
                    self.database.tools.result_return = Some(previous);
                }
                self.database.document.start_result(ticket, view);
                self.database.status = "Queued for execution…".into();
                Some(ticket)
            }
            Err(e) => {
                self.database.status = e.to_string();
                None
            }
        }
    }
    fn run_sql(&mut self) {
        self.run_current(None);
    }
    fn browse_table(&mut self, name: String) {
        if self.database.workspace.enabled
            && let Some(id) = self.find_window(tiling::Kind::Data, Some(&name), None)
        {
            // Focusing retained data is allowed even while that same view owns staged edits.
            if !self.has_drafts() || self.document_ref(id).is_some_and(|d| d.has_drafts()) {
                self.focus_window(id);
                self.database.workspace.expanded = None;
                return;
            }
        }
        if self.has_drafts() || self.database.commit_ticket.is_some() {
            self.database.status = "Commit or discard drafts first".into();
            return;
        }
        if !self.database.ready || self.database.closing {
            self.database.status =
                "Open a database before browsing; retained values are unchanged".into();
            return;
        }
        let previous = self.navigation_entry();
        if self.database.workspace.enabled {
            if let Some(id) = self.find_window(tiling::Kind::Data, Some(&name), None) {
                self.focus_window(id);
                self.database.workspace.expanded = None;
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
        self.database.document.browse = Some(browse.clone());
        self.database.document.view = View::Browse;
        if self
            .submit(Operation::Browse(browse.clone()), View::Browse)
            .is_some()
        {
            if let Some(previous) = previous {
                self.database.tools.back.push(previous);
                if self.database.tools.back.len() > 32 {
                    self.database.tools.back.remove(0);
                }
            }
            self.database.document.table = None;
            self.database.document.tools.ddl = None;
            self.database.document.tools.related = None;
            self.database.document.browse = Some(browse);
            self.database.document.tools.highlight_column = None;
            self.database.document.tools.scroll_to_column = false;
            self.database.document.ui.sql_open = false;
            self.database.ui.unfolded = Some(name.clone());
            self.database.ui.diagram.selected = Some(name);
            if !self.database.workspace.enabled {
                self.database.ui.diagram.freeze_camera();
            }
        }
    }
    fn reload(&mut self) {
        if let Some(destination) = &self.database.document.tools.related {
            self.submit(destination.operation(), View::Browse);
        } else if let Some(b) = self.database.document.browse.clone() {
            self.submit(Operation::Browse(b), View::Browse);
        }
    }
    fn refresh_catalog(&mut self) {
        self.database.tools.counts.clear();
        self.database.document.tools.context = Default::default();
        if self.database.catalog_ticket.is_some() {
            self.database.catalog_refresh_pending = true;
            return;
        }
        self.database.catalog_refresh_pending = false;
        if !self.database.ready || self.database.closing {
            return;
        }
        let Some(session) = &self.database.session else {
            return;
        };
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Schema,
        }) {
            Ok(()) => {
                self.database.catalog.start(ticket);
                self.database.catalog_ticket = Some(ticket);
                self.database.catalog_error.clear();
            }
            Err(e) => self.database.status = e.to_string(),
        }
    }
    fn poll_active(&mut self, ctx: &egui::Context) {
        self.poll_storage(ctx);
        self.poll_recovery(ctx);
        self.apply_theme(ctx);
        if let Some(rx) = &self.database.ui.font_rx {
            match rx.try_recv() {
                Ok((fonts, label)) => {
                    ctx.set_fonts(fonts);
                    self.database.ui.font_label = label;
                    self.database.ui.font_rx = None;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.database.ui.font_rx = None;
                }
                Err(_) => {}
            }
        }
        let mut events = Vec::new();
        let mut disconnected = false;
        let polled_session = self.database.session.as_ref().map(|s| s.id);
        if let Some(s) = &self.database.session {
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
                .database
                .session
                .as_ref()
                .is_some_and(|s| Some(s.id) == polled_session)
        {
            self.database.pending_access = None;
            self.database.tools.count_request = None;
            self.database.catalog_ticket = None;
            self.database.uncertain_edit |= self.database.commit_ticket.take().is_some();
            self.log_outcome(
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into(),
            );
            self.database.session = None;
            self.database.ready = false;
            self.database.closing = false;
            self.quit = false;
            self.database.document.active = None;
            self.database.transaction = TransactionState::Unknown;
            self.database.tools.pending_history.clear();
            self.database.document.tools.inspection_ticket = None;
            self.database.ui.diagram.ticket = None;
            self.database.ui.diagram.stale = true;
            self.database.status =
                "Worker disconnected unexpectedly; execution and rollback outcome UNKNOWN".into();
        }
        if self.database.session.is_none() {
            for document in self
                .database
                .documents
                .values_mut()
                .chain(std::iter::once(&mut self.database.document))
            {
                document.disconnect();
            }
        }
        self.load_restored_view(ctx);
    }
    fn log_outcome(&mut self, message: String) {
        self.database.activity.push_back(message);
        while self.database.activity.len() > 100 {
            self.database.activity.pop_front();
        }
    }
    fn request_close(&mut self) {
        if self.database.session.is_none() && !self.has_drafts() {
            return;
        }
        self.sidebar_search.query.clear();
        if self.database.commit_ticket.is_some() {
            self.database.pending_database = None;
            self.database.status = "Wait for the staged commit outcome before closing".into();
            return;
        }
        if !self.database.closing
            && (self.has_drafts()
                || self.database.writable
                    && self.database.transaction != TransactionState::Autocommit)
        {
            self.database.confirm_close = true;
        } else {
            self.close();
        }
    }
    fn close(&mut self) {
        self.save_recovery();
        self.save_workspace();
        if let Some(layout) = self
            .database
            .ui
            .diagram
            .snapshot(&self.database.tools.database_scope)
        {
            self.preferences
                .map_layouts
                .retain(|l| l.database != layout.database);
            self.preferences.map_layouts.insert(0, layout);
            self.preferences.normalize();
            self.store.save(&self.preferences);
        }
        self.database.confirm_close = false;
        for document in self.database.documents.values_mut() {
            document.drafts = None;
            document.show_preview = false;
            document.show_insert = false;
        }
        self.database.document.drafts = None;
        self.database.document.show_preview = false;
        self.database.document.show_insert = false;
        if let Some(s) = &self.database.session {
            s.close();
            self.database.closing = true;
            self.database.status = "Waiting for execution and explicit transaction cleanup…".into();
        }
    }
    pub fn render(&mut self, ui: &mut egui::Ui) {
        ui.push_id(self.database.id, |ui| self.shell(ui));
        self.large_value_ui(ui.ctx());
        self.transfer_ui(ui.ctx());
        self.profile_ui(ui.ctx());
        self.admin_ui(ui.ctx());
        self.comparison_ui(ui.ctx());
        // Observe edits made during this frame so only pending saves schedule a wakeup.
        self.poll_recovery(ui.ctx());
    }
}
impl eframe::App for App {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll(ctx);
        self.smoke_tick(ctx);
        let closing = ctx.input(|i| i.viewport().close_requested());
        if closing {
            self.save_settings();
        }
        if closing
            && (self.database.session.is_some()
                || self.database.export_job.is_some()
                || self
                    .parked
                    .values()
                    .any(|d| d.session.is_some() || d.export_job.is_some()))
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
            self.quit = true;
            self.continue_quit(ctx);
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
    use super::App;
    use std::time::{Duration, Instant};
    pub(super) fn inspect_selected(app: &mut App, ctx: &egui::Context) {
        let start = Instant::now();
        loop {
            app.poll_database_inspection(ctx);
            if app.databases.inspection.ready(&app.database.path) {
                break;
            }
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
    }
}
