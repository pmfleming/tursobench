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
        if self.path != path {
            self.path = path;
            self.exclusive = false;
            self.writable = false;
            self.databases.inspection.override_sqlite = false;
        }
    }

    pub(super) fn choose_file(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() || self.session.is_some() {
            return;
        }
        self.ui.nav = shell::Nav::Databases;
        self.databases.focus_filter = true;
        ctx.request_repaint();
    }

    pub(super) fn browse_file(&mut self, ctx: &egui::Context) {
        if self.picker.is_some() || self.session.is_some() {
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
                self.status = "Waiting for file selection…".into();
            }
            Err(error) => {
                self.status = format!(
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
                    self.databases.message =
                        Some("No search folder selected. You can enter a folder path below.".into())
                }
                Err(error) => {
                    self.databases.message =
                        Some(format!("Could not choose a search folder: {error}"))
                }
            }
            return;
        }
        self.databases.focus_filter = true;
        match result {
            Ok(Some(path)) if self.session.is_none() => {
                if let Some(path) = path.to_str() {
                    self.set_database_path(path.to_owned());
                    self.ui.nav = shell::Nav::Databases;
                    self.status =
                        "File selected. Review access and exclusive use before opening.".into();
                } else {
                    self.status =
                        "Only UTF-8 database paths are supported; no lossy path substitution"
                            .into();
                }
            }
            Ok(Some(_)) => {
                self.status = "File selection ignored because a database is already open".into();
            }
            Ok(None) => {
                // rfd reports both cancellation and some backend failures as None.
                self.status = "No file selected. If the dialog did not appear, enter the database path manually.".into();
            }
            Err(error) => {
                self.status = format!(
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
            app.path = "previous.db".into();
            app.exclusive = true;
            app.writable = true;
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
            assert!(app.status.contains(message), "{}", app.status);
            assert_eq!(app.path, "previous.db");
            assert!(app.exclusive && app.writable);
            app.ui.nav = shell::Nav::Result;
            let (tx, rx) = mpsc::sync_channel(1);
            app.picker = Some(rx);
            tx.send(Ok(Some("retry.db".into()))).unwrap();
            app.poll_picker();
            assert_eq!(app.path, "retry.db");
            assert!(!app.exclusive && !app.writable);
            assert_eq!(app.ui.nav, shell::Nav::Databases);
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.path = "previous.db".into();
        app.exclusive = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        app.open(&ctx);
        assert!(app.session.is_none());
        assert!(app.status.contains("Finish choosing a file"));
        // Ctrl+O or another button click must not replace an in-flight picker.
        app.choose_file(&ctx);
        tx.send(Ok(Some("selected.db".into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.path, "selected.db");
    }
}
