use super::App;
use crate::discovery::{self, DatabaseKind};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};
type Check = (
    Result<DatabaseKind, String>,
    Result<(), tursobench_core::Failure>,
);

#[derive(Default)]
pub(in crate::app) struct Inspection {
    path: String,
    result: Option<Result<DatabaseKind, String>>,
    worker: Option<Receiver<Check>>,
    access: Option<Result<(), tursobench_core::Failure>>,
    refresh: bool,
    checked: Option<std::time::Instant>,
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

    pub fn allows_open(&self, path: &str) -> bool {
        self.ready(path)
            && self.blocked(path).is_none()
            && (self.kind(path) != DatabaseKind::LikelySqlite || self.override_sqlite)
    }
    pub fn blocked(&self, path: &str) -> Option<&tursobench_core::Failure> {
        (self.path == path)
            .then_some(self.access.as_ref())
            .flatten()
            .and_then(|r| r.as_ref().err())
    }
    pub fn recheck(&mut self) {
        self.refresh = true;
    }
}

impl App {
    pub(in crate::app) fn poll_database_inspection(&mut self, ctx: &egui::Context) {
        let inspection = &mut self.databases.inspection;
        if let Some(rx) = &inspection.worker {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => Some((
                    Err("File check stopped unexpectedly".into()),
                    Err(tursobench_core::Failure::input(
                        "File check stopped unexpectedly. Check again to retry.",
                    )),
                )),
                Err(TryRecvError::Empty) => None,
            };
            if let Some((result, access)) = result {
                inspection.worker = None;
                inspection.result = Some(result);
                inspection.access = Some(access);
                inspection.checked = Some(std::time::Instant::now());
            }
        }
        // Keep at most one worker, even while the user types paths quickly.
        // A result for an old selection never supplies a new path's classification.
        let changed = inspection.path != self.database.path;
        let retry_blocked = inspection.blocked(&self.database.path).is_some()
            && inspection
                .checked
                .is_some_and(|t| t.elapsed() >= std::time::Duration::from_secs(2));
        if inspection.worker.is_none() && (changed || inspection.refresh || retry_blocked) {
            inspection.refresh = false;
            if changed {
                inspection.path.clone_from(&self.database.path);
                inspection.result = None;
                inspection.access = None;
                inspection.override_sqlite = false;
            }
            if !self.database.path.is_empty() {
                let path = PathBuf::from(&self.database.path);
                let wake = ctx.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                match std::thread::Builder::new()
                    .name("database-header".into())
                    .spawn(move || {
                        let access = tursobench_db::check_access(&path);
                        let kind = if access.is_ok() {
                            discovery::inspect_kind(&path).map_err(|e| e.to_string())
                        } else {
                            Ok(DatabaseKind::Unknown)
                        };
                        let result = (kind, access);
                        let _ = tx.send(result);
                        wake.request_repaint();
                    }) {
                    Ok(_) => inspection.worker = Some(rx),
                    Err(error) => {
                        inspection.result = Some(Err(error.to_string()));
                        inspection.access =
                            Some(Err(tursobench_core::Failure::input(error.to_string())));
                    }
                }
            }
        }
        if inspection.worker.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        } else if inspection.blocked(&self.database.path).is_some() {
            ctx.request_repaint_after(std::time::Duration::from_secs(2));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{DatabaseKind, Inspection, mpsc};

    #[test]
    fn late_results_cannot_classify_another_selection_or_transfer_its_override() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.db");
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER)"]);
        let (ctx, mut app) = crate::app::test_ui::app();
        let (tx, rx) = mpsc::sync_channel(1);
        app.databases.inspection = Inspection {
            path: "previous.db".into(),
            result: None,
            worker: Some(rx),
            override_sqlite: true,
            ..Default::default()
        };
        app.set_database_path(path.to_str().unwrap().into());
        app.poll_database_inspection(&ctx);
        assert!(!app.databases.inspection.ready(&app.path));
        assert!(!app.databases.inspection.allows_open(&app.path));
        assert!(!app.databases.inspection.override_sqlite);
        assert_eq!(
            app.databases.inspection.path, "previous.db",
            "do not spawn workers while the previous one is still running"
        );
        tx.send((Ok(DatabaseKind::LikelySqlite), Ok(()))).unwrap();
        let started = std::time::Instant::now();
        loop {
            app.poll_database_inspection(&ctx);
            if app.databases.inspection.ready(&app.path) {
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert_eq!(
            app.databases.inspection.kind(&app.path),
            DatabaseKind::LikelyTurso
        );
        assert!(!app.databases.inspection.override_sqlite);
        assert!(app.session.is_none());
    }
}
