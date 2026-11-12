//! Classify ticket ownership once before dispatching to a focused handler.
use super::{App, View, document::Document, shell};
use eframe::egui;
use tursobench_core::{
    CommitOutcome, Completion, Event, EventKind, Failure, Ticket, TransactionState,
};
use tursobench_db::{Operation, Request};

enum Target {
    Snapshot,
    Result,
    Value,
    Context,
    Page,
    Inspection,
    Count,
    Catalog,
    Diagram,
    Commit,
    Session,
    Transfer,
}
impl Document {
    fn event_target(&self, ticket: Ticket) -> Option<Target> {
        [
            (
                self.tools
                    .context
                    .pending
                    .as_ref()
                    .and_then(|p| p.result.current),
                Target::Context,
            ),
            (
                self.tools
                    .pages
                    .pending
                    .as_ref()
                    .and_then(|p| p.result.current),
                Target::Page,
            ),
            (self.tools.inspection_ticket, Target::Inspection),
            (self.tools.large.ticket, Target::Value),
            (self.result.current, Target::Result),
        ]
        .into_iter()
        .find_map(|(current, target)| (current == Some(ticket)).then_some(target))
    }
}
impl App {
    pub(super) fn load_diagram(&mut self) {
        self.database.request_diagram();
    }

    fn event_target(&self, ticket: Ticket) -> Option<(Option<u64>, Target)> {
        let session = self.database.session.as_ref()?;
        if ticket.session != session.id || ticket.database != session.id {
            return None;
        }
        if self.database.commit_ticket == Some(ticket) {
            return Some((self.database.commit_owner, Target::Commit));
        }
        for (current, target) in [
            (self.database.tools.snapshot.ticket, Target::Snapshot),
            (self.database.catalog_ticket, Target::Catalog),
            (self.database.tools.transfers.ticket, Target::Transfer),
            (self.database.ui.diagram.ticket, Target::Diagram),
            (
                self.database
                    .tools
                    .count_request
                    .as_ref()
                    .and_then(|request| request.result.current),
                Target::Count,
            ),
        ] {
            if current == Some(ticket) {
                return Some((None, target));
            }
        }
        Some(
            std::iter::once(&self.database.document)
                .chain(self.database.documents.values())
                .find_map(|document| {
                    document
                        .event_target(ticket)
                        .map(|target| (Some(document.id), target))
                })
                .unwrap_or((None, Target::Session)),
        )
    }
    pub(super) fn handle_event(&mut self, ctx: &egui::Context, event: Event) {
        let Some((owner, target)) = self.event_target(event.ticket) else {
            return;
        };
        let previous = self.database.document.id;
        let routed = owner.is_some_and(|id| id != previous && self.switch_document(id));
        match target {
            Target::Snapshot => self.snapshot_event(event.kind),
            Target::Transfer => self.transfer_event(event.kind),
            Target::Value => self.value_event(event.kind),
            Target::Context => self.context_event(event),
            Target::Page => self.page_event(event),
            Target::Count => self.count_event(event),
            Target::Catalog => self.catalog_event(event),
            Target::Diagram => self.diagram_event(event.kind),
            Target::Inspection => self.inspection_event(event.kind),
            Target::Commit => {
                if let EventKind::EditsCommitted(outcome) = event.kind {
                    self.commit_outcome(event.ticket, outcome);
                }
            }
            Target::Result => self.result_event(ctx, event),
            Target::Session => self.session_event(ctx, event),
        }
        if routed {
            self.switch_document(previous);
        }
    }
    fn count_event(&mut self, event: Event) {
        let finished = matches!(event.kind, EventKind::Finished(_) | EventKind::Failed(_));
        if let EventKind::Finished(c) = &event.kind {
            self.database.transaction = c.transaction;
        }
        let failure = match &event.kind {
            EventKind::Failed(error) => Some(error.to_string()),
            _ => None,
        };
        if let Some(request) = &mut self.database.tools.count_request {
            request.result.apply(event);
        }
        if finished {
            self.finish_count(failure);
        }
    }
    fn diagram_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::Diagram(graph) => {
                let first_load = self.database.ui.diagram.data.is_none();
                self.database.ui.diagram.loaded(graph);
                if first_load
                    && let Some(layout) = self
                        .preferences
                        .map_layouts
                        .iter()
                        .find(|l| l.database == self.database.tools.database_scope)
                {
                    self.database.ui.diagram.restore(layout);
                }
            }
            EventKind::Failed(e) => {
                self.database.ui.diagram.ticket = None;
                self.database.ui.diagram.error = e.to_string();
                self.database.ui.diagram.stale = true;
            }
            _ => {}
        }
        if self.database.diagram_refresh_pending {
            self.database.diagram_refresh_pending = false;
            self.load_diagram();
        }
    }
    fn commit_outcome(&mut self, ticket: Ticket, outcome: CommitOutcome) {
        self.log_outcome(format!(
            "Request {} staged commit: {outcome:?}",
            ticket.request
        ));
        self.database.commit_ticket = None;
        self.database.commit_owner = None;
        match outcome {
            CommitOutcome::Committed { operations } => {
                self.invalidate_views(false, ticket.request);
                self.database.tools.counts.clear();
                self.database.document.tools.context = Default::default();
                self.database.document.drafts = None;
                self.database.document.tools.spreadsheet = Default::default();
                self.database.document.show_preview = false;
                self.database.document.inspect = None;
                self.reload();
                self.database.status =
                    format!("Committed {operations} staged operations; refreshing page");
            }
            CommitOutcome::Rejected(e) => {
                self.database.status = format!("No edits committed; drafts retained: {e}")
            }
            CommitOutcome::Unknown(e) => {
                self.database.uncertain_edit = true;
                self.database.status =
                    format!("UNKNOWN outcome; drafts retained. Do not retry: {e}");
            }
        }
    }
    fn opened(&mut self) {
        self.database.review_origin = None;
        self.initialize_workspace();
        self.database.ready = true;
        self.record_database_open();
        self.refresh_catalog();
        self.database.document.ui.nav = shell::Nav::Overview;
        self.load_diagram();
        self.restore_workspace();
        self.restore_sql();
    }
    fn closed(&mut self, ctx: &egui::Context, rollback: Result<(), Failure>) {
        self.database.tools.transfers.disconnected();
        let reopen = if rollback.is_ok() && !self.database.uncertain_edit && !self.quit {
            self.database.pending_access.take()
        } else {
            self.database.pending_access = None;
            None
        };
        self.database.catalog_ticket = None;
        self.database.tools.snapshot = Default::default();
        self.database.tools.count_request = None;
        self.database.tools.pending_history.clear();
        self.database.document.tools.inspection_ticket = None;
        self.database.ui.diagram.ticket = None;
        self.database.ui.diagram.stale = true;
        if rollback.is_err() {
            self.quit = false;
            self.database.transaction = TransactionState::Unknown;
        }
        self.database.status = match rollback {
            Ok(()) if self.database.uncertain_edit => "Closed; editor cleanup confirmed, but the earlier staged commit outcome remains UNKNOWN".into(),
            Ok(()) => "Closed; transaction cleanup confirmed".into(),
            Err(e) => format!("Close outcome uncertain: {e}"),
        };
        self.database.session = None;
        self.database.document.ui.nav = shell::Nav::Databases;
        self.database.ready = false;
        self.database.document.active = None;
        self.database.closing = false;
        if let Some(path) = self.database.pending_database.take().filter(|_| {
            self.database.transaction != TransactionState::Unknown && !self.database.uncertain_edit
        }) {
            self.set_database_path(path.to_string_lossy().into_owned());
            self.database.document.ui.nav = shell::Nav::Databases;
        }
        if let Some(writable) = reopen {
            self.database.writable = writable;
            self.open_with_slot(ctx, true);
        }
        if self.quit && self.database.export_job.is_none() {
            self.continue_quit(ctx);
        }
    }
    fn inspection_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::Inspection(info) => {
                self.database.document.tools.inspection = Some(info);
                self.database.document.tools.inspection_ticket = None;
                self.database.document.tools.inspection_error.clear();
            }
            EventKind::Failed(e) => {
                self.database.document.tools.inspection_ticket = None;
                self.database.document.tools.inspection_error = e.to_string();
            }
            _ => {}
        }
    }
    fn failed(&mut self, ctx: &egui::Context, ticket: Ticket, e: Failure) {
        if e.kind == tursobench_core::ErrorKind::Unknown {
            self.database.uncertain_edit = true;
            self.database.transaction = TransactionState::Unknown;
        }
        self.record_history(ticket, None, true);
        self.log_outcome(format!("Request {} ERROR: {e}", ticket.request));
        if ticket.request == 0 {
            self.database.status = e.to_string();
            self.databases.open_failure = Some(e);
            self.database.session = None;
            self.database.ready = false;
            self.database.document.active = None;
            self.database.closing = false;
            self.database.confirm_close = false;
            // Failed opens never send Closed: there is no live session left
            // to acknowledge a pending close or application exit.
            if self.quit && self.database.export_job.is_none() {
                self.continue_quit(ctx);
            }
        } else if self.database.document.active == Some(ticket) {
            self.database.status = e.to_string();
            self.database.document.active = None;
        }
    }
    fn session_event(&mut self, ctx: &egui::Context, event: Event) {
        match event.kind {
            EventKind::Invalidated {
                schema,
                source_request,
            } => self.invalidate_views(schema, source_request),
            EventKind::Opened(coordination) if event.ticket.request == 0 => {
                self.database.coordination = Some(coordination);
                self.opened();
                self.database.status = format!("Connected · {}", coordination.label());
            }
            EventKind::Closed { rollback } if event.ticket.request == 0 => {
                self.closed(ctx, rollback)
            }
            EventKind::Failed(e) => self.failed(ctx, event.ticket, e),
            EventKind::Finished(c) => self.record_completion(event.ticket, &c),
            _ => {}
        }
    }
    fn invalidate_views(&mut self, schema: bool, source_request: u64) {
        self.database.tools.counts.clear();
        if let Some(request) = self.database.tools.count_request.take()
            && let Some(ticket) = request.result.current
            && let Some(session) = &self.database.session
        {
            session.discard_results(ticket.request);
        }
        if schema {
            self.database.tools.snapshot.stale = true;
            self.database.ui.diagram.stale = true;
            self.database.diagram_refresh_pending |= self.database.ui.diagram.ticket.is_some();
        }
        for document in
            std::iter::once(&mut self.database.document).chain(self.database.documents.values_mut())
        {
            if source_request == 0 {
                document.external_refresh = document.view == View::Browse;
                if schema && document.kind == super::tiling::Kind::Structure {
                    document.needs_load = true;
                }
            }
            document.tools.filtered_count = None;
            document.tools.context = Default::default();
            document.tools.pages.pending = None;
            if schema {
                document.tools.inspection = None;
                document.tools.inspection_ticket = None;
            }
            if document
                .result
                .current
                .is_some_and(|t| t.request != source_request)
            {
                document.stale = true;
            }
        }
        if schema {
            self.refresh_catalog();
            self.load_diagram();
        }
    }
    pub(super) fn refresh_external_views(&mut self) {
        if !self.requests_enabled() || self.database.transaction != TransactionState::Autocommit {
            return;
        }
        let previous = self.database.document.id;
        let ids: Vec<_> = std::iter::once(&self.database.document)
            .chain(self.database.documents.values())
            .filter(|d| {
                d.view == View::Browse
                    && d.external_refresh
                    && d.active.is_none()
                    && d.tools.pages.pending.is_none()
            })
            .map(|d| d.id)
            .collect();
        for id in ids {
            if !self.switch_document(id) {
                continue;
            }
            // Reload table/related reads only. Arbitrary SQL is never replayed.
            if self.database.document.browse.is_some()
                || self.database.document.tools.related.is_some()
            {
                let navigation = self.navigation_entry();
                self.reload();
                if self.database.document.active.is_some() {
                    self.database.document.external_refresh = false;
                    self.database.document.tools.restore_navigation = navigation;
                }
            } else {
                self.database.document.external_refresh = false;
            }
        }
        self.switch_document(previous);
    }
    fn record_completion(&mut self, ticket: Ticket, c: &Completion) {
        if c.transaction == TransactionState::Unknown {
            self.database.uncertain_edit = true;
        }
        self.record_history(ticket, Some(c.elapsed_ms), c.error.is_some());
        self.log_outcome(format!(
            "Request {}: {}",
            ticket.request,
            completion_summary(c)
        ));
        self.database.transaction = c.transaction;
    }
    fn result_event(&mut self, ctx: &egui::Context, event: Event) {
        if self.database.document.discard_ticket == Some(event.ticket)
            && matches!(event.kind, EventKind::Rows(_) | EventKind::Columns(_))
        {
            return;
        }
        match event.kind {
            EventKind::ScriptResult(result) => self.database.document.push_script_result(result),
            EventKind::ScriptOutcome {
                atomic,
                rolled_back,
            } => self.database.document.script_message = if rolled_back {
                "Atomic script rolled back; displayed results are from that attempt"
            } else if atomic {
                "Atomic script finished; inspect the final completion status"
            } else {
                "Statements ran sequentially; earlier successful writes may already be committed"
            }
            .into(),
            EventKind::Table(table) => {
                self.database.document.tools.export_table = table.name.clone();
                self.request_inspection(table.name.clone());
                self.database.document.table = Some(table);
            }
            EventKind::Failed(e) => self.failed(ctx, event.ticket, e),
            EventKind::Finished(c) => {
                self.record_completion(event.ticket, &c);
                self.database.status = format!("{} ms · {}", c.elapsed_ms, completion_summary(&c));
                self.database.document.active = None;
                self.database.document.result.completion = Some(c);
                self.browse_completed();
            }
            kind => {
                if matches!(kind, EventKind::Columns(_)) {
                    self.database.status = "Running…".into();
                }
                self.database.document.result.apply(Event {
                    ticket: event.ticket,
                    kind,
                });
            }
        }
    }
    fn browse_completed(&mut self) {
        if self.database.document.view == View::Browse
            && let Some(browse) = &self.database.document.browse
            && self.database.document.result.rows.len() < 100
            && self
                .database
                .document
                .result
                .completion
                .as_ref()
                .is_some_and(|c| {
                    c.delivery == tursobench_core::Delivery::Complete
                        && c.error.is_none()
                        && c.page_has_more != Some(true)
                })
        {
            self.database.document.tools.pages.end =
                Some(browse.offset + self.database.document.result.rows.len());
        }
        self.restore_browse_selection();
    }
}
fn completion_summary(c: &Completion) -> String {
    let error = c
        .error
        .as_ref()
        .map_or(String::new(), |e| format!("; ERROR {e}"));
    format!(
        "{:?}: {} rows seen, {} changes; transaction {:?}{error}",
        c.delivery, c.rows_seen, c.changed, c.transaction
    )
}

impl super::sessions::DatabaseWorkspace {
    pub(super) fn request_diagram(&mut self) {
        if !self.ready || self.closing || self.ui.diagram.ticket.is_some() {
            return;
        }
        let Some(session) = &self.session else {
            return;
        };
        self.sequence += 1;
        let ticket = session.ticket(self.sequence, self.generation);
        match session.submit(Request {
            ticket,
            operation: Operation::Diagram,
        }) {
            Ok(()) => {
                self.ui.diagram.ticket = Some(ticket);
                self.ui.diagram.error.clear();
            }
            Err(e) => self.ui.diagram.error = e.to_string(),
        }
    }
}
