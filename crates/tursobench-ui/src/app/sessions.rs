//! Database state moves between workspaces; result buffers are never cloned.
use super::{App, DatabaseWorkspace, shell};
use eframe::egui;
use std::path::{Path, PathBuf};
const MAX_DATABASES: usize = 4;
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
fn fuzzy(label: &str, query: &str) -> bool {
    let lower = label.to_lowercase();
    let mut chars = lower.chars();
    query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|c| chars.by_ref().any(|v| v == c))
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
        self.parked.retain(|_, d| {
            d.session.is_some()
                || d.export_job.is_some()
                || d.uncertain_edit
                || d.transaction == tursobench_core::TransactionState::Unknown
                || d.documents
                    .values()
                    .chain(std::iter::once(&d.document))
                    .any(|v| v.has_drafts())
        });
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
            .find(|d| d.session.is_some() && same_file(Path::new(&d.path), path))
            .map(|d| d.id)
    }
    pub(super) fn request_database(&mut self, ctx: &egui::Context, path: PathBuf) {
        if let Some(id) = self.duplicate_workspace(&path) {
            self.select_workspace(id);
            return;
        }
        if self.new_workspace(ctx) {
            self.set_database_path(path.to_string_lossy().into_owned());
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
    pub(super) fn database_tabs(&mut self, ui: &mut egui::Ui) {
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::PageDown)) {
            self.cycle_database(1);
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::PageUp)) {
            self.cycle_database(-1);
        }
        if ui.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::P)) {
            self.object_search_open = true;
        }
        if self.parked.is_empty() {
            return;
        }
        let mut selected = None;
        egui::Panel::top("database-tabs").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                for d in std::iter::once(&self.database).chain(self.parked.values()) {
                    let file = Path::new(&d.path)
                        .file_name()
                        .map_or_else(|| "Open database".into(), |s| s.to_string_lossy());
                    let label = format!(
                        "{}{}{}",
                        file,
                        if d.transaction == tursobench_core::TransactionState::Active {
                            " · transaction"
                        } else {
                            ""
                        },
                        if d.closing { " · closing" } else { "" }
                    );
                    if ui
                        .selectable_label(d.id == self.database.id, label)
                        .on_hover_text(&d.path)
                        .clicked()
                    {
                        selected = Some(d.id);
                    }
                }
                if ui
                    .button("+")
                    .on_hover_text("Open another database")
                    .clicked()
                {
                    selected = Some(0);
                }
                if ui.button("Find object…").clicked() {
                    self.object_search_open = true;
                }
            });
        });
        if let Some(id) = selected {
            if id == 0 {
                self.new_workspace(ui.ctx());
            } else {
                self.select_workspace(id);
            }
        }
    }
    fn cycle_database(&mut self, direction: isize) {
        let mut ids: Vec<_> = self
            .parked
            .keys()
            .copied()
            .chain(std::iter::once(self.database.id))
            .collect();
        ids.sort();
        let index = ids
            .iter()
            .position(|id| *id == self.database.id)
            .unwrap_or(0);
        let next = (index as isize + direction).rem_euclid(ids.len() as isize) as usize;
        self.select_workspace(ids[next]);
    }
    pub(super) fn object_search_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.object_search_open;
        if !open {
            return;
        }
        let mut chosen = None;
        egui::Window::new("Find object across databases")
            .open(&mut open)
            .show(ctx, |ui| {
                ui.text_edit_singleline(&mut self.object_search);
                egui::ScrollArea::vertical()
                    .max_height(400.0)
                    .show(ui, |ui| {
                        let mut shown = 0;
                        for d in std::iter::once(&self.database).chain(self.parked.values()) {
                            for object in &d.schema {
                                let label = format!(
                                    "{} · {} · {}",
                                    Path::new(&d.path)
                                        .file_name()
                                        .unwrap_or_default()
                                        .to_string_lossy(),
                                    object.kind,
                                    object.name
                                );
                                if shown < 100 && fuzzy(&label, &self.object_search) {
                                    shown += 1;
                                    if ui.button(label).clicked() {
                                        chosen = Some((
                                            d.id,
                                            object.kind.clone(),
                                            object.name.clone(),
                                            None,
                                        ));
                                    }
                                }
                            }
                            if let Some(graph) = &d.ui.diagram.data {
                                for table in &graph.tables {
                                    if let Some(info) = &table.inspection {
                                        for column in &info.table.columns {
                                            let label = format!(
                                                "{} · {}.{}",
                                                Path::new(&d.path)
                                                    .file_name()
                                                    .unwrap_or_default()
                                                    .to_string_lossy(),
                                                table.name,
                                                column.name
                                            );
                                            if shown < 100 && fuzzy(&label, &self.object_search) {
                                                shown += 1;
                                                if ui.button(label).clicked() {
                                                    chosen = Some((
                                                        d.id,
                                                        "table".into(),
                                                        table.name.clone(),
                                                        Some(column.name.clone()),
                                                    ));
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                        if shown == 100 {
                            ui.weak("First 100 matches; refine the search.");
                        }
                    });
            });
        self.object_search_open = open;
        if let Some((id, kind, name, column)) = chosen {
            self.select_workspace(id);
            if self.database.id == id {
                if kind == "table" || kind == "view" {
                    self.execute_command(ctx, super::commands::Command::Table(name, column));
                } else {
                    self.submit(
                        tursobench_db::Operation::Definition(name),
                        super::View::Diagnostics,
                    );
                }
                self.object_search_open = false;
            }
        }
    }
}
