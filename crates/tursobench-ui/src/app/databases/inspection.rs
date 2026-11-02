use super::App;
use crate::discovery::{self, DatabaseKind};
use eframe::egui;
use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

#[derive(Default)]
pub(in crate::app) struct Inspection {
    path: String,
    result: Option<Result<DatabaseKind, String>>,
    worker: Option<Receiver<Result<DatabaseKind, String>>>,
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

    pub fn error(&self) -> Option<&str> {
        self.result
            .as_ref()
            .and_then(|r| r.as_ref().err())
            .map(String::as_str)
    }

    pub fn allows_open(&self, path: &str) -> bool {
        self.ready(path) && (self.kind(path) != DatabaseKind::LikelySqlite || self.override_sqlite)
    }
}

impl App {
    pub(in crate::app) fn poll_database_inspection(&mut self, ctx: &egui::Context) {
        let inspection = &mut self.databases.inspection;
        if let Some(rx) = &inspection.worker {
            let result = match rx.try_recv() {
                Ok(result) => Some(result),
                Err(TryRecvError::Disconnected) => {
                    Some(Err("Header inspection stopped unexpectedly".into()))
                }
                Err(TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                inspection.worker = None;
                inspection.result = Some(result);
            }
        }
        // Keep at most one worker, even while the user types paths quickly.
        // A result for an old selection never supplies a new path's classification.
        if inspection.worker.is_none() && inspection.path != self.path {
            inspection.path.clone_from(&self.path);
            inspection.result = None;
            inspection.override_sqlite = false;
            if !self.path.is_empty() {
                let path = PathBuf::from(&self.path);
                let wake = ctx.clone();
                let (tx, rx) = mpsc::sync_channel(1);
                match std::thread::Builder::new()
                    .name("database-header".into())
                    .spawn(move || {
                        let result = discovery::inspect_kind(&path).map_err(|e| e.to_string());
                        let _ = tx.send(result);
                        wake.request_repaint();
                    }) {
                    Ok(_) => inspection.worker = Some(rx),
                    Err(error) => inspection.result = Some(Err(error.to_string())),
                }
            }
        }
        if inspection.worker.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn late_results_cannot_classify_another_selection_or_transfer_its_override() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("current.db");
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER)"]);
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        let (tx, rx) = mpsc::sync_channel(1);
        app.databases.inspection = Inspection {
            path: "previous.db".into(),
            result: None,
            worker: Some(rx),
            override_sqlite: true,
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
        tx.send(Ok(DatabaseKind::LikelySqlite)).unwrap();
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
