use super::App;
use crate::discovery::{self, DatabaseKind};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
    time::{Duration, Instant},
};
use tursobench_core::Failure;
type Check = (Result<DatabaseKind, String>, Result<(), Failure>);

#[derive(Default)]
pub(in crate::app) struct Inspection {
    path: String,
    result: Option<Result<DatabaseKind, String>>,
    worker: Option<Receiver<Check>>,
    access: Option<Result<(), Failure>>,
    refresh: bool,
    checked: Option<Instant>,
    pub override_sqlite: bool,
}

impl Inspection {
    pub fn ready(&self, path: &str) -> bool {
        self.path == path && self.result.is_some()
    }

    pub fn kind(&self, path: &str) -> DatabaseKind {
        if self.path == path {
            self.result
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .copied()
                .unwrap_or_default()
        } else {
            DatabaseKind::Unknown
        }
    }

    /// Whether the checked file looks like SQLite and needs the user's override.
    pub fn likely_sqlite(&self, path: &str) -> bool {
        self.ready(path) && self.kind(path) == DatabaseKind::LikelySqlite
    }

    pub fn allows_open(&self, path: &str) -> bool {
        self.ready(path)
            && self.blocked(path).is_none()
            && (!self.likely_sqlite(path) || self.override_sqlite)
    }
    pub fn blocked(&self, path: &str) -> Option<&Failure> {
        (self.path == path)
            .then_some(self.access.as_ref())
            .flatten()
            .and_then(|r| r.as_ref().err())
    }
    pub fn recheck(&mut self) {
        self.refresh = true;
    }

    /// Collect a finished check, start one for a new, refreshed or still
    /// blocked selection, and schedule the repaint that notices the next.
    fn poll(&mut self, path: &str, ctx: &egui::Context) {
        self.receive();
        // Keep at most one worker, even while the user types paths quickly.
        // A result for an old selection never supplies a new path's classification.
        let changed = self.path != path;
        let retry_blocked = self.blocked(path).is_some()
            && self
                .checked
                .is_some_and(|t| t.elapsed() >= Duration::from_secs(2));
        if self.worker.is_none() && (changed || self.refresh || retry_blocked) {
            self.refresh = false;
            if changed {
                *self = Self {
                    path: path.into(),
                    ..Self::default()
                };
            }
            if !path.is_empty() {
                self.start(ctx);
            }
        }
        if self.worker.is_some() {
            ctx.request_repaint_after(Duration::from_millis(50));
        } else if self.blocked(path).is_some() {
            ctx.request_repaint_after(Duration::from_secs(2));
        }
    }

    fn receive(&mut self) {
        let Some(rx) = &self.worker else {
            return;
        };
        let (result, access) = match rx.try_recv() {
            Ok(check) => check,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => (
                Err("File check stopped unexpectedly".into()),
                Err(Failure::input(
                    "File check stopped unexpectedly. Check again to retry.",
                )),
            ),
        };
        self.worker = None;
        self.result = Some(result);
        self.access = Some(access);
        self.checked = Some(Instant::now());
    }

    /// Check the selected file's access, then its header, off the UI thread.
    fn start(&mut self, ctx: &egui::Context) {
        let path = PathBuf::from(&self.path);
        let wake = ctx.clone();
        let (tx, rx) = mpsc::sync_channel(1);
        let spawned = std::thread::Builder::new()
            .name("database-header".into())
            .spawn(move || {
                let access = tursobench_db::check_access(&path);
                let kind = if access.is_ok() {
                    discovery::inspect_kind(&path).map_err(|e| e.to_string())
                } else {
                    Ok(DatabaseKind::Unknown)
                };
                let _ = tx.send((kind, access));
                wake.request_repaint();
            });
        match spawned {
            Ok(_) => self.worker = Some(rx),
            Err(error) => {
                self.result = Some(Err(error.to_string()));
                self.access = Some(Err(Failure::input(error.to_string())));
            }
        }
    }
}

impl App {
    pub(in crate::app) fn poll_database_inspection(&mut self, ctx: &egui::Context) {
        self.databases.inspection.poll(&self.database.path, ctx);
    }

    /// Why the file cannot open, or a spinner while it is checked; returns
    /// whether the user asked to check a blocked file again.
    pub(super) fn open_issue_ui(&self, ui: &mut egui::Ui) -> bool {
        let path = &self.database.path;
        let blocked = self.databases.inspection.blocked(path);
        let Some(error) = self.databases.open_failure.as_ref().or(blocked) else {
            if !self.databases.inspection.ready(path) {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.weak("Checking file…");
                });
            }
            return false;
        };
        let title = if self.databases.open_failure.is_some() {
            "Couldn’t open database"
        } else if error.kind == tursobench_core::ErrorKind::Busy {
            "Database in use"
        } else {
            "Cannot open database"
        };
        let color = ui.visuals().error_fg_color;
        let mut recheck = false;
        egui::Frame::new()
            .fill(color.gamma_multiply(0.09))
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
            .corner_radius(6)
            .inner_margin(10.0)
            .show(ui, |ui| {
                ui.colored_label(color, egui::RichText::new(title).strong());
                ui.add(egui::Label::new(&error.message).wrap());
                if blocked.is_some() {
                    recheck = ui
                        .button("Check again")
                        .on_hover_text("Recheck this file’s availability and locks")
                        .clicked();
                }
            });
        recheck
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseKind, Inspection, mpsc};
    use crate::app::test_ui;

    #[test]
    fn late_results_cannot_classify_another_selection_or_transfer_its_override() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.db");
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER)"]);
        let (ctx, mut app) = test_ui::app();
        let (tx, rx) = mpsc::sync_channel(1);
        app.databases.inspection = Inspection {
            path: "previous.db".into(),
            worker: Some(rx),
            override_sqlite: true,
            ..Default::default()
        };
        app.set_database_path(path.to_str().unwrap().into());
        app.poll_database_inspection(&ctx);
        let inspection = &app.databases.inspection;
        assert!(!inspection.ready(&app.database.path));
        assert!(!inspection.allows_open(&app.database.path));
        assert!(!inspection.override_sqlite);
        assert_eq!(
            inspection.path, "previous.db",
            "do not spawn workers while the previous one is still running"
        );
        tx.send((Ok(DatabaseKind::LikelySqlite), Ok(()))).unwrap();
        test_ui::inspect_selected(&mut app, &ctx);
        assert_eq!(
            app.databases.inspection.kind(&app.database.path),
            DatabaseKind::LikelyTurso
        );
        assert!(!app.databases.inspection.override_sqlite);
        assert!(app.database.session.is_none());
    }
}
