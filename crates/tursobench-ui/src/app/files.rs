use super::{App, shell};
use eframe::egui;

use std::{
    path::PathBuf,
    sync::mpsc::{self, Receiver, TryRecvError},
};

pub(super) type PickResult = Result<Option<PathBuf>, String>;

fn start_picker(
    wake: egui::Context,
    pick: impl FnOnce() -> Option<PathBuf> + Send + std::panic::UnwindSafe + 'static,
) -> std::io::Result<Receiver<PickResult>> {
    let (tx, rx) = mpsc::sync_channel(1);
    std::thread::Builder::new()
        .name("database-file-picker".into())
        .spawn(move || {
            let result = std::panic::catch_unwind(pick)
                .map_err(|_| "The file chooser stopped unexpectedly".into());
            let _ = tx.send(result);
            // Wake the UI on failure as well as selection/cancellation.
            wake.request_repaint();
        })?;
    Ok(rx)
}

impl App {
    pub(super) fn set_database_path(&mut self, path: String) {
        if self.database.path != path {
            self.database.path = path;
            self.databases.open_failure = None;

            self.database.writable = false;
            self.databases.inspection.override_sqlite = false;
        }
    }

    pub(super) fn browse_file(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() || self.database.session.is_some() {
            return;
        }
        self.databases.picking_folder = false;
        match start_picker(ctx.clone(), || {
            rfd::FileDialog::new()
                .set_title("Open existing Turso database")
                .pick_file()
        }) {
            Ok(rx) => {
                self.picker = Some(rx);
                self.database.status = "Waiting for file selection…".into();
            }
            Err(error) => {
                self.database.status = format!(
                    "Could not start the file chooser: {error}. Enter the database path manually."
                );
            }
        }
    }

    pub(super) fn browse_search_folder(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() || self.databases.job.is_some() {
            return;
        }
        match start_picker(ctx.clone(), || {
            rfd::FileDialog::new()
                .set_title("Choose a database search folder")
                .pick_folder()
        }) {
            Ok(rx) => {
                self.databases.picking_folder = true;
                self.picker = Some(rx);
                self.databases.message = Some("Waiting for search folder selection…".into());
            }
            Err(error) => {
                self.databases.message = Some(format!(
                    "Could not start the folder chooser: {error}. Enter a folder path below."
                ))
            }
        }
    }

    pub(super) fn poll_picker(&mut self) {
        let Some(rx) = &self.picker else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Empty) => return,
            Err(TryRecvError::Disconnected) => {
                Err("The file chooser worker disconnected unexpectedly".into())
            }
        };
        self.picker = None;
        if std::mem::take(&mut self.databases.picking_folder) {
            match result {
                Ok(Some(path)) => self.add_search_folder(path),
                Ok(None) => {
                    self.databases.message = None;
                }
                Err(error) => {
                    self.databases.message =
                        Some(format!("Could not choose a search folder: {error}"))
                }
            }
            return;
        }
        self.sidebar_search.focus = true;
        match result {
            Ok(Some(path)) if self.database.session.is_none() => {
                if let Some(path) = path.to_str() {
                    if let Some(id) = self.duplicate_workspace(std::path::Path::new(path)) {
                        let abandoned = self.database.id;
                        self.select_workspace(id);
                        if abandoned != id
                            && self.parked.get(&abandoned).is_some_and(|d| !d.retained())
                        {
                            self.parked.remove(&abandoned);
                        }
                        return;
                    }
                    self.set_database_path(path.into());
                    self.database.document.ui.nav = shell::Nav::Databases;
                    self.database.status = "File selected. Review access before opening.".into();
                } else {
                    self.database.status =
                        "Only UTF-8 database paths are supported; no lossy path substitution"
                            .into();
                }
            }
            Ok(Some(_)) => {
                self.database.status =
                    "File selection ignored because a database is already open".into();
            }
            Ok(None) => {
                if self.database.review_origin.is_some() {
                    self.cancel_database_review();
                }
                // rfd reports both cancellation and some backend failures as None.
                self.database.status = "No file selected. If the dialog did not appear, enter the database path manually.".into();
            }
            Err(error) => {
                if self.database.review_origin.is_some() {
                    self.cancel_database_review();
                }
                self.database.status = format!(
                    "Could not choose a file: {error}. Enter the database path manually or try again."
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, PathBuf, mpsc, shell, start_picker};
    use eframe::egui;
    use std::time::Duration;

    #[test]
    fn picker_outcomes_preserve_consent_until_a_new_selection_and_allow_retry() {
        let ctx = egui::Context::default();
        let mut cases = vec![
            (Some(Ok(None)), "No file selected"),
            (Some(Ok(Some("previous.db".into()))), "File selected"),
            (Some(Err("test failure".into())), "test failure"),
            (None, "disconnected unexpectedly"),
        ];
        #[cfg(unix)]
        {
            use std::os::unix::ffi::OsStringExt;
            cases.push((
                Some(Ok(Some(
                    std::ffi::OsString::from_vec(b"invalid-\xff.db".to_vec()).into(),
                ))),
                "Only UTF-8 database paths",
            ));
        }
        for (result, message) in cases {
            let mut app = App::new(&ctx);
            app.database.path = "previous.db".into();

            app.database.writable = true;
            let (tx, rx) = mpsc::sync_channel(1);
            app.picker = Some(rx);
            app.poll_picker();
            assert!(app.picker.is_some(), "pending picker must remain pending");
            if let Some(result) = result {
                tx.send(result).unwrap();
            }
            drop(tx);
            app.poll_picker();
            assert!(app.picker.is_none());
            assert!(
                app.database.status.contains(message),
                "{}",
                app.database.status
            );
            assert_eq!(app.database.path, "previous.db");
            assert!(app.database.writable);
            app.database.document.ui.nav = shell::Nav::Result;
            let (tx, rx) = mpsc::sync_channel(1);
            app.picker = Some(rx);
            tx.send(Ok(Some("retry.db".into()))).unwrap();
            app.poll_picker();
            assert_eq!(app.database.path, "retry.db");
            assert!(!app.database.writable);
            assert_eq!(app.database.document.ui.nav, shell::Nav::Databases);
        }
    }

    #[test]
    fn picker_worker_delivers_selection_cancellation_and_panics() {
        let ctx = egui::Context::default();
        for selection in [Some(PathBuf::from("selected.db")), None] {
            let expected = selection.clone();
            let rx = start_picker(ctx.clone(), move || selection).unwrap();
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(5)).unwrap(),
                Ok(expected)
            );
        }
        let rx = start_picker(ctx, || panic!("simulated native dialog failure")).unwrap();
        assert!(
            rx.recv_timeout(Duration::from_secs(5))
                .unwrap()
                .unwrap_err()
                .contains("stopped unexpectedly")
        );
    }

    #[test]
    fn pending_picker_blocks_opening_the_previous_path() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.path = "previous.db".into();

        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        app.open(&ctx);
        assert!(app.database.session.is_none());
        assert!(app.database.status.contains("Finish choosing a file"));
        // Ctrl+O or another button click must not replace an in-flight picker.
        app.begin_database_browse(&ctx);
        tx.send(Ok(Some("selected.db".into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.database.path, "selected.db");
    }
}
