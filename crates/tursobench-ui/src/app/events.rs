//! Classify ticket ownership once before dispatching to a focused handler.
use super::{App, View, document::Document, shell};
use eframe::egui;
use tursobench_core::{
    CommitOutcome, Completion, Event, EventKind, Failure, Ticket, TransactionState,
};
use tursobench_db::{Operation, Request};

enum Target {
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

    fn event_target(&self, ticket: Ticket) -> Option<(Option<u64>, Target)> {
        let session = self.session.as_ref()?;
        if ticket.session != session.id || ticket.database != session.id {
            return None;
        }
        if self.commit_ticket == Some(ticket) {
            return Some((self.commit_owner, Target::Commit));
        }
        for (current, target) in [
            (self.catalog_ticket, Target::Catalog),
            (self.ui.diagram.ticket, Target::Diagram),
            (
                self.tools
                    .count_request
                    .as_ref()
                    .and_then(|(_, r)| r.current),
                Target::Count,
            ),
        ] {
            if current == Some(ticket) {
                return Some((None, target));
            }
        }
        Some(
            std::iter::once(&self.document)
                .chain(self.documents.values())
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
        let previous = self.document.id;
        let routed = owner.is_some_and(|id| id != previous && self.switch_document(id));
        match target {
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
            self.transaction = c.transaction;
        }
        let failure = match &event.kind {
            EventKind::Failed(error) => Some(error.to_string()),
            _ => None,
        };
        if let Some((_, result)) = &mut self.tools.count_request {
            result.apply(event);
        }
        if finished {
            self.finish_count(failure);
        }
    }
    fn diagram_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::Diagram(graph) => {
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
            EventKind::Failed(e) => {
                self.ui.diagram.ticket = None;
                self.ui.diagram.error = e.to_string();
                self.ui.diagram.stale = true;
            }
            _ => {}
        }
    }
    fn commit_outcome(&mut self, ticket: Ticket, outcome: CommitOutcome) {
        self.log_outcome(format!(
            "Request {} staged commit: {outcome:?}",
            ticket.request
        ));
        self.commit_ticket = None;
        self.commit_owner = None;
        match outcome {
            CommitOutcome::Committed { operations } => {
                self.invalidate_views(false, ticket.request);
                self.tools.counts.clear();
                self.document.tools.context = Default::default();
                self.ui.diagram.stale = true;
                self.document.drafts = None;
                self.document.show_preview = false;
                self.document.inspect = None;
                self.reload();
                self.status = format!("Committed {operations} staged operations; refreshing page");
            }
            CommitOutcome::Rejected(e) => {
                self.status = format!("No edits committed; drafts retained: {e}")
            }
            CommitOutcome::Unknown(e) => {
                self.uncertain_edit = true;
                self.status = format!("UNKNOWN outcome; drafts retained. Do not retry: {e}");
            }
        }
    }
    fn opened(&mut self) {
        self.initialize_workspace();
        self.ready = true;
        self.record_database_open();
        self.refresh_catalog();
        self.document.ui.nav = shell::Nav::Overview;
        self.load_diagram();
        self.restore_workspace();
    }
    fn closed(&mut self, ctx: &egui::Context, rollback: Result<(), Failure>) {
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
        if let Some(path) = self
            .pending_database
            .take()
            .filter(|_| self.transaction != TransactionState::Unknown && !self.uncertain_edit)
        {
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
    fn inspection_event(&mut self, kind: EventKind) {
        match kind {
            EventKind::Inspection(info) => {
                self.document.tools.inspection = Some(info);
                self.document.tools.inspection_ticket = None;
                self.document.tools.inspection_error.clear();
            }
            EventKind::Failed(e) => {
                self.document.tools.inspection_ticket = None;
                self.document.tools.inspection_error = e.to_string();
            }
            _ => {}
        }
    }
    fn failed(&mut self, ctx: &egui::Context, ticket: Ticket, e: Failure) {
        self.record_history(ticket, None, true);
        self.log_outcome(format!("Request {} ERROR: {e}", ticket.request));
        if ticket.request == 0 {
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
        } else if self.document.active == Some(ticket) {
            self.status = e.to_string();
            self.document.active = None;
        }
    }
    fn session_event(&mut self, ctx: &egui::Context, event: Event) {
        match event.kind {
            EventKind::Invalidated {
                schema,
                source_request,
            } => self.invalidate_views(schema, source_request),
            EventKind::Opened if event.ticket.request == 0 => self.opened(),
            EventKind::Closed { rollback } if event.ticket.request == 0 => {
                self.closed(ctx, rollback)
            }
            EventKind::Failed(e) => self.failed(ctx, event.ticket, e),
            EventKind::Finished(c) => self.record_completion(event.ticket, &c),
            _ => {}
        }
    }
    fn invalidate_views(&mut self, schema: bool, source_request: u64) {
        self.tools.counts.clear();
        self.ui.diagram.stale = true;
        for document in std::iter::once(&mut self.document).chain(self.documents.values_mut()) {
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
        }
    }
    fn record_completion(&mut self, ticket: Ticket, c: &Completion) {
        self.record_history(ticket, Some(c.elapsed_ms), c.error.is_some());
        self.log_outcome(format!(
            "Request {}: {}",
            ticket.request,
            completion_summary(c)
        ));
        self.transaction = c.transaction;
    }
    fn result_event(&mut self, ctx: &egui::Context, event: Event) {
        if self.document.discard_ticket == Some(event.ticket)
            && matches!(event.kind, EventKind::Rows(_) | EventKind::Columns(_))
        {
            return;
        }
        match event.kind {
            EventKind::Table(table) => {
                self.document.tools.export_table = table.name.clone();
                self.request_inspection(table.name.clone());
                self.document.table = Some(table);
            }
            EventKind::Failed(e) => self.failed(ctx, event.ticket, e),
            EventKind::Finished(c) => {
                self.record_completion(event.ticket, &c);
                self.status = format!("{} ms · {}", c.elapsed_ms, completion_summary(&c));
                self.document.active = None;
                self.document.result.completion = Some(c);
                self.browse_completed();
            }
            kind => {
                if matches!(kind, EventKind::Columns(_)) {
                    self.status = "Running…".into();
                }
                self.document.result.apply(Event {
                    ticket: event.ticket,
                    kind,
                });
            }
        }
    }
    fn browse_completed(&mut self) {
        if self.document.view == View::Browse
            && let Some(browse) = &self.document.browse
            && self.document.result.rows.len() < 100
            && self.document.result.completion.as_ref().is_some_and(|c| {
                c.delivery == tursobench_core::Delivery::Complete
                    && c.error.is_none()
                    && c.page_has_more != Some(true)
            })
        {
            self.document.tools.pages.end = Some(browse.offset + self.document.result.rows.len());
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
