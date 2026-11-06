//! Database state moves between workspaces; result buffers are never cloned.
use super::{App, document, recovery, shell, tools, workspace};
use eframe::egui;
use std::path::{Path, PathBuf};
use tursobench_core::{ResultSet, SchemaObject, Ticket, TransactionState};
use tursobench_db::Session;
pub(super) struct DatabaseWorkspace {
    pub(super) id: u64,
    pub(super) recovery: recovery::State,
    pub(super) pending_database: Option<std::path::PathBuf>,
    pub(super) path: String,
    pub(super) tools: tools::Tools,
    pub(super) document: document::Document,
    pub(super) workspace: workspace::Workspace,
    pub(super) documents: std::collections::BTreeMap<u64, document::Document>,
    pub(super) commit_owner: Option<u64>,
    pub(super) ui: shell::UiState,
    pub(super) export_job: Option<std::sync::mpsc::Receiver<Result<String, String>>>,
    pub(super) writable: bool,
    pub(super) coordination: Option<tursobench_core::Coordination>,
    pub(super) confirm_close: bool,
    pub(super) pending_access: Option<bool>,
    pub(super) session: Option<Session>,
    pub(super) ready: bool,
    pub(super) closing: bool,
    pub(super) sequence: u64,
    pub(super) generation: u64,
    pub(super) schema: Vec<SchemaObject>,
    pub(super) catalog: ResultSet,
    pub(super) catalog_ticket: Option<Ticket>,
    pub(super) catalog_error: String,
    pub(super) catalog_refresh_pending: bool,
    pub(super) diagram_refresh_pending: bool,
    pub(super) status: String,
    pub(super) activity: std::collections::VecDeque<String>,
    pub(super) transaction: TransactionState,
    pub(super) commit_ticket: Option<Ticket>,
    pub(super) uncertain_edit: bool,
}
impl DatabaseWorkspace {
    pub(super) fn retained(&self) -> bool {
        self.session.is_some()
            || self.export_job.is_some()
            || self.uncertain_edit
            || self.transaction == TransactionState::Unknown
            || self
                .documents
                .values()
                .chain(std::iter::once(&self.document))
                .any(|d| d.has_drafts())
    }
    pub(super) fn draft_count(&self) -> usize {
        self.documents
            .values()
            .chain(std::iter::once(&self.document))
            .filter_map(|d| d.drafts.as_ref())
            .map(|d| d.edits.len())
            .sum()
    }

    pub(super) fn new(ctx: &egui::Context, id: u64) -> Self {
        Self {
            id,
            recovery: recovery::State::new(ctx),
            pending_database: Default::default(),
            path: Default::default(),
            tools: Default::default(),
            document: Default::default(),
            workspace: Default::default(),
            documents: Default::default(),
            commit_owner: Default::default(),
            ui: shell::UiState::new(ctx),
            export_job: Default::default(),
            writable: Default::default(),
            coordination: Default::default(),
            confirm_close: Default::default(),
            pending_access: Default::default(),
            session: Default::default(),
            ready: Default::default(),
            closing: Default::default(),
            sequence: Default::default(),
            generation: Default::default(),
            schema: Default::default(),
            catalog: Default::default(),
            catalog_ticket: Default::default(),
            catalog_error: Default::default(),
            catalog_refresh_pending: Default::default(),
            diagram_refresh_pending: Default::default(),
            status: Default::default(),
            activity: Default::default(),
            transaction: TransactionState::Autocommit,
            commit_ticket: Default::default(),
            uncertain_edit: Default::default(),
        }
    }
}
pub(super) const MAX_DATABASES: usize = 4;
fn same_file(a: &Path, b: &Path) -> bool {
    if a == b {
        return true;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let (Ok(a), Ok(b)) = (std::fs::metadata(a), std::fs::metadata(b)) {
            return a.dev() == b.dev() && a.ino() == b.ino();
        }
    }
    std::fs::canonicalize(a)
        .ok()
        .zip(std::fs::canonicalize(b).ok())
        .is_some_and(|(a, b)| a == b)
}
impl App {
    fn swap_workspace(&mut self, id: u64) -> bool {
        if self.database.id == id {
            return true;
        }
        let Some(next) = self.parked.remove(&id) else {
            return false;
        };
        let previous = std::mem::replace(&mut self.database, next);
        self.parked.insert(previous.id, previous);
        true
    }
    pub(super) fn select_workspace(&mut self, id: u64) {
        if self.picker.is_some() || self.database.confirm_close || self.quit {
            return;
        }
        self.save_recovery();
        self.save_workspace();
        self.swap_workspace(id);
    }
    pub(super) fn new_workspace(&mut self, ctx: &egui::Context) -> bool {
        if self.picker.is_some() || self.quit || self.database.confirm_close {
            return false;
        }
        if self.database.session.is_none()
            && !self.has_drafts()
            && self.database.export_job.is_none()
            && !self.database.uncertain_edit
            && self.database.transaction != tursobench_core::TransactionState::Unknown
        {
            self.database.document.ui.nav = shell::Nav::Databases;
            return true;
        }
        // Closed, clean tabs can be released; live sessions and uncertain state cannot.
        self.parked.retain(|_, d| d.retained());
        if self.parked.len() + 1 >= MAX_DATABASES {
            self.database.status =
                "Four database workspaces are already retained; close one before opening another"
                    .into();
            return false;
        }
        self.save_recovery();
        self.save_workspace();
        let id = self.next_workspace;
        self.next_workspace += 1;
        let previous = std::mem::replace(&mut self.database, DatabaseWorkspace::new(ctx, id));
        self.parked.insert(previous.id, previous);
        true
    }
    pub(super) fn duplicate_workspace(&self, path: &Path) -> Option<u64> {
        std::iter::once(&self.database)
            .chain(self.parked.values())
            .find(|d| d.retained() && same_file(Path::new(&d.path), path))
            .map(|d| d.id)
    }
    pub(super) fn can_request_database(&self) -> bool {
        if self.picker.is_some() || self.quit || self.database.confirm_close {
            return false;
        }
        !self.database.retained()
            || self.parked.values().filter(|d| d.retained()).count() + 1 < MAX_DATABASES
    }
    pub(super) fn request_database(&mut self, ctx: &egui::Context, path: PathBuf) {
        if let Some(id) = self.duplicate_workspace(&path) {
            self.select_workspace(id);
            return;
        }
        if path.to_str().is_none() {
            self.database.status =
                "Only UTF-8 database paths are supported by the Turso engine".into();
            return;
        }
        if self.new_workspace(ctx) {
            self.sidebar_search.query.clear();
            self.set_database_path(path.to_str().unwrap().into());
        }
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        let focused = self.database.id;
        let ids: Vec<_> = self.parked.keys().copied().collect();
        self.polling_background = true;
        for id in ids {
            self.swap_workspace(id);
            self.poll_active(ctx);
        }
        self.swap_workspace(focused);
        self.polling_background = false;
        self.poll_active(ctx);
        if self.quit {
            self.continue_quit(ctx);
        }
    }
    pub(super) fn continue_quit(&mut self, ctx: &egui::Context) {
        if self.polling_background || !self.quit {
            return;
        }
        if self.database.transaction == tursobench_core::TransactionState::Unknown
            || self.database.uncertain_edit
        {
            self.quit = false;
            self.database.status =
                "Exit paused: inspect the uncertain outcome before closing".into();
            return;
        }
        if self.database.export_job.is_some()
            || self.database.confirm_close
            || self.database.closing
        {
            return;
        }
        if self.database.session.is_some() {
            self.request_close();
            return;
        }
        if let Some(id) = self
            .parked
            .values()
            .find(|d| {
                d.session.is_some()
                    || d.export_job.is_some()
                    || d.uncertain_edit
                    || d.transaction == tursobench_core::TransactionState::Unknown
            })
            .map(|d| d.id)
        {
            self.swap_workspace(id);
            self.continue_quit(ctx);
        } else {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    pub(super) fn cycle_database(&mut self, direction: isize) {
        let mut ids: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| d.retained())
            .map(|d| d.id)
            .collect();
        ids.sort();
        if ids.is_empty() {
            return;
        }
        let next = if let Some(index) = ids.iter().position(|id| *id == self.database.id) {
            (index as isize + direction).rem_euclid(ids.len() as isize) as usize
        } else if direction > 0 {
            0
        } else {
            ids.len() - 1
        };
        self.select_workspace(ids[next]);
    }
}
