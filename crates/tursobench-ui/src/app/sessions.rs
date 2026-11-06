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
    pub(super) identity: Option<crate::discovery::Identity>,
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
    pub(super) review_origin: Option<u64>,
    pub(super) close_origin: Option<u64>,
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
            identity: None,
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
            review_origin: None,
            close_origin: None,
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
#[cfg(test)]
thread_local! { pub(super) static IDENTITY_PROBES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) }; }
fn same_file(a: &Path, b: &Path) -> bool {
    #[cfg(test)]
    IDENTITY_PROBES.with(|count| count.set(count.get() + 1));
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
        if !self.database_admission_available() {
            self.database.status = self.database_limit_message();
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
        self.save_recovery();
        self.save_workspace();
        let id = self.next_workspace;
        self.next_workspace += 1;
        let previous = std::mem::replace(&mut self.database, DatabaseWorkspace::new(ctx, id));
        self.database.review_origin = Some(previous.id);
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
        self.database_admission_available()
    }
    pub(super) fn database_admission_available(&self) -> bool {
        let retained = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| d.retained())
            .count();
        retained < self.memory.limits(&self.preferences).database_limit
    }
    pub(super) fn database_limit_message(&self) -> String {
        let limits = self.memory.limits(&self.preferences);
        format!(
            "Database limit {} reached ({} budget, {} MiB per database). Close a database or adjust Settings → Database resources.",
            limits.database_limit,
            crate::resources::format_mib(limits.budget_mib),
            crate::resources::DATABASE_RESERVATION_MIB
        )
    }
    pub(super) fn request_database(&mut self, ctx: &egui::Context, path: PathBuf) {
        if let Some(id) = self.duplicate_workspace(&path) {
            let abandoned = (!self.database.retained() && self.database.review_origin.is_some())
                .then_some(self.database.id);
            self.select_workspace(id);
            if let Some(abandoned) = abandoned.filter(|id| *id != self.database.id) {
                self.parked.remove(&abandoned);
            }
            return;
        }
        if path.to_str().is_none() {
            self.database.status =
                "Only UTF-8 database paths are supported by the Turso engine".into();
            return;
        }
        if self.new_workspace(ctx) {
            self.databases.editing_path = false;
            self.sidebar_search.query.clear();
            self.sidebar_search.navigating = false;
            self.sidebar_search.available_highlight = None;
            self.set_database_path(path.to_str().unwrap().into());
        }
    }
    pub(super) fn poll(&mut self, ctx: &egui::Context) {
        self.poll_picker();
        self.poll_discovery(ctx);
        if self.database.session.is_none() {
            self.poll_database_inspection(ctx);
        }
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
        self.settle_sidebar_closes();
        if self.quit {
            self.continue_quit(ctx);
        }
    }
    pub(super) fn cancel_database_review(&mut self) {
        if self.database.retained() {
            return;
        }
        let pending = self.database.id;
        let origin = self.database.review_origin.take();
        self.set_database_path(String::new());
        self.database.writable = false;
        self.databases.editing_path = false;
        if let Some(origin) = origin {
            self.select_workspace(origin);
            if self.database.id != pending {
                self.parked.remove(&pending);
            }
        }
    }

    pub(super) fn request_sidebar_close(&mut self, id: u64) {
        let origin = self.database.id;
        self.select_workspace(id);
        if self.database.id != id
            || self.picker.is_some()
            || self.quit
            || self.database.confirm_close
        {
            return;
        }
        self.database.close_origin = Some(origin);
        self.request_close();
        if !self.database.confirm_close && origin != id {
            self.select_workspace(origin);
        }
        self.settle_sidebar_closes();
    }

    pub(super) fn cancel_sidebar_close(&mut self) {
        self.database.confirm_close = false;
        self.quit = false;
        self.database.pending_database = None;
        if let Some(origin) = self.database.close_origin.take() {
            self.select_workspace(origin);
        }
    }

    fn settle_sidebar_closes(&mut self) {
        if self.quit || self.picker.is_some() {
            return;
        }
        let mut completed = Vec::new();
        for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
            if database.close_origin.is_some()
                && database.session.is_none()
                && !database.closing
                && !database.confirm_close
            {
                let origin = database.close_origin.take().unwrap();
                if !database.retained() {
                    completed.push((database.id, origin));
                }
            }
        }
        for (id, origin) in completed {
            if self.database.id != id {
                continue;
            }
            self.set_database_path(String::new());
            self.database.writable = false;
            let next = self
                .parked
                .get(&origin)
                .filter(|d| d.retained())
                .map(|d| d.id)
                .or_else(|| {
                    self.parked
                        .values()
                        .find(|d| d.session.is_some())
                        .map(|d| d.id)
                })
                .or_else(|| self.parked.values().find(|d| d.retained()).map(|d| d.id));
            if let Some(next) = next {
                self.select_workspace(next);
            }
        }
    }

    pub(super) fn continue_quit(&mut self, ctx: &egui::Context) {
        if self.polling_background || !self.quit {
            return;
        }
        self.save_settings();
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
        if self.database.session.is_some() || self.has_drafts() {
            self.request_close();
            return;
        }
        if let Some(id) = self.parked.values().find(|d| d.retained()).map(|d| d.id) {
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
