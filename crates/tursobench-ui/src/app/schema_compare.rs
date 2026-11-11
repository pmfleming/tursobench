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
            self.database.document.set_sql(sql);
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
            .map(|w| Choice {
                id: w.id,
                path: &w.path,
                capture: &w.tools.snapshot,
            })
            .collect();
        match self.comparison.show(ctx, &choices) {
            Some(Action::Capture) => {
                self.comparison.result = None;
                self.comparison.error.clear();
                self.capture_schema(self.comparison.source);
                self.capture_schema(self.comparison.target);
            }
            Some(Action::Draft {
                target,
                sql,
                expected,
            }) => {
                self.open_migration(target, sql, expected);
                self.comparison.open = false;
            }
            None => {}
        }
    }
}
struct Choice<'a> {
    id: u64,
    path: &'a str,
    capture: &'a Capture,
}
enum Action {
    Capture,
    Draft {
        target: u64,
        sql: String,
        expected: Arc<SchemaSnapshot>,
    },
}
impl Capture {
    fn show(&self, ui: &mut egui::Ui, path: &str) {
        if self.ticket.is_some() {
            ui.label(format!("Capturing {path}…"));
        }
        if self.stale {
            ui.label(format!("{path}: snapshot stale; refresh"));
        }
        if !self.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &self.error);
        }
    }
}
fn choose(ui: &mut egui::Ui, label: &str, selected: &mut u64, choices: &[Choice<'_>]) {
    let path = choices
        .iter()
        .find(|c| c.id == *selected)
        .map_or("Choose…", |c| c.path);
    egui::ComboBox::from_id_salt(label)
        .selected_text(format!("{label}: {path}"))
        .show_ui(ui, |ui| {
            for c in choices {
                ui.selectable_value(selected, c.id, c.path);
            }
        });
}
impl State {
    fn show(&mut self, ctx: &egui::Context, choices: &[Choice<'_>]) -> Option<Action> {
        if !choices.iter().any(|c| c.id == self.source) {
            self.source = choices.first().map_or(0, |c| c.id);
        }
        if !choices.iter().any(|c| c.id == self.target) {
            self.target = choices
                .iter()
                .find(|c| c.id != self.source)
                .map_or(0, |c| c.id);
        }
        let mut open = self.open;
        let mut action = None;
        egui::Window::new("Schema comparison")
            .open(&mut open)
            .default_width(760.0)
            .default_height(520.0)
            .show(ctx, |ui| action = self.controls(ui, choices));
        self.open = open;
        action
    }
    fn controls(&mut self, ui: &mut egui::Ui, choices: &[Choice<'_>]) -> Option<Action> {
        ui.label("Compare committed main schemas. Added/removed refer to changes needed in the target to match the desired schema.");
        if choices.len() < 2 {
            ui.label("Open a second database workspace to compare schemas.");
            return None;
        }
        choose(ui, "Desired schema", &mut self.source, choices);
        choose(ui, "Target database", &mut self.target, choices);
        let pending = choices.iter().any(|c| c.capture.ticket.is_some());
        if ui
            .add_enabled(
                self.source != self.target && !pending && self.job.is_none(),
                egui::Button::new("Capture / refresh both schemas"),
            )
            .clicked()
        {
            return Some(Action::Capture);
        }
        for c in choices
            .iter()
            .filter(|c| c.id == self.source || c.id == self.target)
        {
            c.capture.show(ui, c.path);
        }
        if !self.error.is_empty() {
            ui.label(&self.error);
        }
        if self.job.is_some() {
            ui.spinner();
        }
        ui.horizontal(|ui| {
            ui.label("Filter objects");
            ui.text_edit_singleline(&mut self.filter);
            ui.checkbox(&mut self.identical, "Show identical");
        });
        let result = self
            .result
            .as_ref()
            .filter(|r| r.source == self.source && r.target == self.target)?;
        let action = result.show(ui, &self.filter, self.identical);
        ui.label("Drafts run only on explicit execution in read/write mode. Target schema is checked inside the transaction. Migration drafts are session-only; refresh comparison after reopening.");
        action
    }
}
impl Compared {
    fn show(&self, ui: &mut egui::Ui, filter: &str, identical: bool) -> Option<Action> {
        let changed = self
            .differences
            .iter()
            .filter(|d| d.status != Status::Identical)
            .count();
        ui.label(format!(
            "{changed} changed objects / {} total · definitions compared literally",
            self.differences.len()
        ));
        let query = filter.to_lowercase();
        let mut matches = self.differences.iter().filter(|d| {
            (identical || d.status != Status::Identical) && d.name.to_lowercase().contains(&query)
        });
        let mut action = None;
        egui::ScrollArea::vertical()
            .max_height(420.0)
            .show(ui, |ui| {
                for d in matches.by_ref().take(200) {
                    ui.collapsing(format!("{:?} · {} {}", d.status, d.kind, d.name), |ui| {
                        if let Some(sql) = self.difference(ui, d) {
                            action = Some(Action::Draft {
                                target: self.target,
                                sql,
                                expected: self.current.clone(),
                            });
                        }
                    });
                }
                if matches.next().is_some() {
                    ui.label("Showing first 200 matches; narrow the filter.");
                }
            });
        action
    }
    fn difference(&self, ui: &mut egui::Ui, d: &Difference) -> Option<String> {
        for detail in &d.details {
            ui.label(detail);
        }
        for (label, snapshot) in [
            ("Target DDL", &self.current),
            ("Desired DDL", &self.desired),
        ] {
            if let Some(ddl) = snapshot
                .objects
                .iter()
                .find(|o| o.kind == d.kind && o.name == d.name)
                .and_then(|o| o.ddl.as_deref())
            {
                ui.collapsing(label, |ui| {
                    if ui.button("Copy definition").clicked() {
                        ui.ctx().copy_text(ddl.to_owned());
                    }
                    sql_preview(ui, ddl);
                });
            }
        }
        if let Some(guidance) = &d.guidance {
            ui.label(guidance);
        }
        let sql = d.migration.as_ref()?;
        sql_preview(ui, sql);
        ui.button("Open guarded migration draft in target editor")
            .clicked()
            .then(|| sql.clone())
    }
}
fn sql_preview(ui: &mut egui::Ui, sql: &str) {
    ui.monospace(sql.chars().take(16_384).collect::<String>());
    if sql.chars().count() > 16_384 {
        ui.label("Display truncated; Copy definition retains the full SQL.");
    }
}
