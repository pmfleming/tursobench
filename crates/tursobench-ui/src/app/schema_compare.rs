//! Snapshot comparison is global; capture ownership stays with each database session.
use super::{App, DatabaseWorkspace};
use eframe::egui;
use std::sync::{Arc, mpsc};
use tursobench_core::{EventKind, SchemaSnapshot, Ticket};
use tursobench_db::{
    Operation, Request,
    schema_compare::{Difference, Status, compare},
};

#[derive(Default)]
pub(super) struct Capture {
    pub ticket: Option<Ticket>,
    pub snapshot: Option<Arc<SchemaSnapshot>>,
    pub stale: bool,
    pub error: String,
}
struct Compared {
    source: u64,
    target: u64,
    desired: Arc<SchemaSnapshot>,
    current: Arc<SchemaSnapshot>,
    differences: Vec<Difference>,
}
#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    pub source: u64,
    pub target: u64,
    job: Option<mpsc::Receiver<Compared>>,
    result: Option<Compared>,
    filter: String,
    identical: bool,
    error: String,
}
impl App {
    pub(super) fn comparison_ready(&self) -> bool {
        self.comparison
            .result
            .as_ref()
            .is_some_and(|r| self.comparison_matches(r))
    }
    fn comparison_workspace(&self, id: u64) -> Option<&DatabaseWorkspace> {
        if self.database.id == id {
            Some(&self.database)
        } else {
            self.parked.get(&id)
        }
    }
    pub(super) fn capture_schema(&mut self, id: u64) {
        let workspace = if self.database.id == id {
            Some(&mut self.database)
        } else {
            self.parked.get_mut(&id)
        };
        let Some(w) =
            workspace.filter(|w| w.ready && !w.closing && w.tools.snapshot.ticket.is_none())
        else {
            return;
        };
        let Some(session) = &w.session else {
            return;
        };
        w.sequence += 1;
        let ticket = session.ticket(w.sequence, w.generation);
        w.tools.snapshot.snapshot = None;
        w.tools.snapshot.error.clear();
        match session.submit(Request {
            ticket,
            operation: Operation::CaptureSchema,
        }) {
            Ok(()) => w.tools.snapshot.ticket = Some(ticket),
            Err(e) => w.tools.snapshot.error = e.to_string(),
        }
    }
    pub(super) fn snapshot_event(&mut self, event: EventKind) {
        let state = &mut self.database.tools.snapshot;
        match event {
            EventKind::SchemaSnapshot(snapshot) => {
                state.snapshot = Some(Arc::new(snapshot));
                state.ticket = None;
                state.stale = false;
            }
            EventKind::Failed(e) => {
                state.error = e.to_string();
                state.ticket = None;
            }
            _ => {}
        }
    }
    fn comparison_matches(&self, result: &Compared) -> bool {
        [
            (result.source, &result.desired),
            (result.target, &result.current),
        ]
        .into_iter()
        .all(|(id, snapshot)| {
            self.comparison_workspace(id).is_some_and(|w| {
                w.ready
                    && !w.closing
                    && !w.tools.snapshot.stale
                    && w.tools
                        .snapshot
                        .snapshot
                        .as_ref()
                        .is_some_and(|s| Arc::ptr_eq(s, snapshot))
            })
        }) && self.comparison.source == result.source
            && self.comparison.target == result.target
    }
    pub(super) fn poll_comparison(&mut self, ctx: &egui::Context) {
        if let Some(job) = &self.comparison.job {
            match job.try_recv() {
                Ok(result) => {
                    self.comparison.job = None;
                    if self.comparison_matches(&result) {
                        self.comparison.result = Some(result);
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.comparison.job = None;
                    self.comparison.error =
                        "Schema comparison worker stopped; refresh to retry".into();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if self
            .comparison
            .result
            .as_ref()
            .is_some_and(|r| !self.comparison_matches(r))
        {
            self.comparison.result = None;
        }
        if !self.comparison.open
            || self.comparison.job.is_some()
            || self.comparison.result.is_some()
            || !self.comparison.error.is_empty()
        {
            return;
        }
        let source = self.comparison.source;
        let target = self.comparison.target;
        if source == target {
            return;
        }
        let snapshot = |id| {
            self.comparison_workspace(id)
                .filter(|w| w.ready && !w.closing && !w.tools.snapshot.stale)
                .and_then(|w| w.tools.snapshot.snapshot.clone())
        };
        let (Some(desired), Some(current)) = (snapshot(source), snapshot(target)) else {
            return;
        };
        let (tx, rx) = mpsc::sync_channel(1);
        let wake = ctx.clone();
        match std::thread::Builder::new()
            .name("schema-compare".into())
            .spawn(move || {
                let differences = compare(&desired, &current);
                let _ = tx.send(Compared {
                    source,
                    target,
                    desired,
                    current,
                    differences,
                });
                wake.request_repaint();
            }) {
            Ok(_) => self.comparison.job = Some(rx),
            Err(e) => self.comparison.error = e.to_string(),
        }
    }
    pub(super) fn open_migration(
        &mut self,
        target: u64,
        sql: String,
        expected: Arc<SchemaSnapshot>,
    ) {
        self.select_workspace(target);
        if self.database.id != target || !self.database.ready {
            return;
        }
        if let Some(id) = self.create_window(super::tiling::Kind::Sql, None, None, true) {
            self.switch_document(id);
            self.database.document.sql = sql;
            self.database.document.params = "[]".into();
            self.database.document.migration = Some(expected);
            self.database.status = "Migration draft opened. Review SQL, then Run all; execution is atomic and checks the target schema again.".into();
        }
    }
    pub(super) fn comparison_ui(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::F7)) {
            self.comparison.open = true;
        }
        self.poll_comparison(ctx);
        if !self.comparison.open {
            return;
        }
        let choices: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|w| w.ready && !w.closing)
            .map(|w| (w.id, w.path.clone()))
            .collect();
        if !choices.iter().any(|(id, _)| *id == self.comparison.source) {
            self.comparison.source = choices.first().map_or(0, |c| c.0);
        }
        if !choices.iter().any(|(id, _)| *id == self.comparison.target) {
            self.comparison.target = choices
                .iter()
                .find(|(id, _)| *id != self.comparison.source)
                .map_or(0, |c| c.0);
        }
        let mut open = true;
        let mut refresh = false;
        let mut draft = None;
        egui::Window::new("Schema comparison").open(&mut open).default_width(760.0).default_height(520.0).show(ctx, |ui| {
            ui.label("Compare committed main schemas. Added/removed refer to changes needed in the target to match the desired schema.");
            if choices.len() < 2 { ui.label("Open a second database workspace to compare schemas."); return; }
            for (label, selected) in [("Desired schema", &mut self.comparison.source), ("Target database", &mut self.comparison.target)] {
                egui::ComboBox::from_id_salt(label).selected_text(format!("{label}: {}", choices.iter().find(|c| c.0 == *selected).map_or("Choose…", |c| c.1.as_str()))).show_ui(ui, |ui| {
                    for (id,path) in &choices { ui.selectable_value(selected,*id,path); }
                });
            }
            let pending = choices.iter().any(|(id,_)| self.comparison_workspace(*id).is_some_and(|w| w.tools.snapshot.ticket.is_some()));
            refresh = ui.add_enabled(self.comparison.source != self.comparison.target && !pending && self.comparison.job.is_none(),egui::Button::new("Capture / refresh both schemas")).clicked();
            for id in [self.comparison.source,self.comparison.target] {
                if let Some(w) = self.comparison_workspace(id) {
                    if w.tools.snapshot.ticket.is_some() { ui.label(format!("Capturing {}…",w.path)); }
                    if w.tools.snapshot.stale { ui.label(format!("{}: snapshot stale; refresh",w.path)); }
                    if !w.tools.snapshot.error.is_empty() { ui.colored_label(ui.visuals().error_fg_color, &w.tools.snapshot.error); }
                }
            }
            if !self.comparison.error.is_empty() { ui.label(&self.comparison.error); }
            if self.comparison.job.is_some() { ui.spinner(); }
            ui.horizontal(|ui| { ui.label("Filter objects"); ui.text_edit_singleline(&mut self.comparison.filter); ui.checkbox(&mut self.comparison.identical,"Show identical"); });
            // Choice widgets can invalidate a previous result within this frame.
            let Some(result) = self.comparison.result.as_ref().filter(|r| self.comparison_matches(r)) else { return; };
            let changed = result.differences.iter().filter(|d|d.status!=Status::Identical).count();
            ui.label(format!("{changed} changed objects / {} total · definitions compared literally",result.differences.len()));
            let query = self.comparison.filter.to_lowercase();
            let rows: Vec<_> = result.differences.iter().filter(|d| (self.comparison.identical || d.status!=Status::Identical) && d.name.to_lowercase().contains(&query)).collect();
            if rows.len() > 200 { ui.label("Showing first 200 matches; narrow the filter."); }
            egui::ScrollArea::vertical().max_height(420.0).show(ui,|ui| {
                for d in rows.into_iter().take(200) {
                    ui.collapsing(format!("{:?} · {} {}",d.status,d.kind,d.name),|ui| {
                        for detail in &d.details { ui.label(detail); }
                        for (label,snapshot) in [("Target DDL",&result.current),("Desired DDL",&result.desired)] {
                            if let Some(ddl) = snapshot.objects.iter().find(|o|o.kind==d.kind && o.name==d.name).and_then(|o|o.ddl.as_ref()) {
                                ui.collapsing(label, |ui| { if ui.button("Copy definition").clicked() { ui.ctx().copy_text(ddl.clone()); } ui.monospace(ddl.chars().take(16_384).collect::<String>()); if ddl.len()>16_384 { ui.label("Display truncated; Copy definition retains the full SQL."); } });
                            }
                        }
                        if let Some(guidance) = &d.guidance { ui.label(guidance); }
                        if let Some(sql) = &d.migration {
                            ui.monospace(sql.chars().take(16_384).collect::<String>());
                            if ui.button("Open guarded migration draft in target editor").clicked() { draft=Some((result.target,sql.clone(),result.current.clone())); }
                        }
                    });
                }
            });
            ui.label("Drafts run only on explicit execution in read/write mode. Target schema is checked inside the transaction. Migration drafts are session-only; refresh comparison after reopening.");
        });
        self.comparison.open = open;
        if refresh {
            self.comparison.result = None;
            self.comparison.error.clear();
            self.capture_schema(self.comparison.source);
            self.capture_schema(self.comparison.target);
        }
        if let Some((target, sql, expected)) = draft {
            self.open_migration(target, sql, expected);
            self.comparison.open = false;
        }
    }
}
