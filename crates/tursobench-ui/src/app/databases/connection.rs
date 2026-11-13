use super::App;
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn connect_ui(&mut self, ui: &mut egui::Ui) {
        ui.centered_and_justified(|ui| {
            ui.label("Choose a database in the sidebar, or press Ctrl+O to browse files.");
        });
    }

    /// The path link and field; the access review is placed by the list itself.
    /// `keep_link` leaves the link in place while a listed row is reviewed, so
    /// selecting a row does not shift the rows above it.
    pub(in crate::app) fn pending_database_ui(&mut self, ui: &mut egui::Ui, keep_link: bool) {
        if self.database.retained() {
            return;
        }
        #[cfg(test)]
        {
            self.databases.edit_path_rect = None;
        }
        let mut focus_path = false;
        if self.database.path.is_empty() || keep_link {
            let edit = ui.link("Enter a path…");
            focus_path = edit.clicked();
            #[cfg(test)]
            {
                self.databases.edit_path_rect = Some(edit.rect);
                if self.database.path.is_empty() {
                    self.databases.open_rect = None;
                    self.databases.open_enabled = false;
                }
            }
            if edit.clicked() {
                self.databases.editing_path = !self.databases.editing_path;
            }
        }
        if self.databases.editing_path {
            self.database_path_ui(ui, focus_path);
        }
    }

    pub(in crate::app) fn retained_database_ui(&mut self, ui: &mut egui::Ui) {
        ui.label("Disconnected · retained data");
        ui.add(egui::Label::new(&self.database.path).truncate())
            .on_hover_text(&self.database.path);
        ui.colored_label(ui.visuals().warn_fg_color, &self.database.status);
        if self.has_drafts() {
            ui.label(
                "Local drafts are retained. Review the outcome before discarding or retrying.",
            );
            if ui.button("Review retained drafts…").clicked() {
                if let Some(id) = self
                    .database
                    .documents
                    .values()
                    .chain(std::iter::once(&self.database.document))
                    .find(|d| d.has_drafts())
                    .map(|d| d.id)
                {
                    self.switch_document(id);
                }
                self.database.document.show_preview = true;
            }
            if ui.button("Discard retained local drafts").clicked() {
                self.database.document.drafts = None;
                for document in self.database.documents.values_mut() {
                    document.drafts = None;
                }
            }
        }
    }

    pub(in crate::app) fn database_access_ui(&mut self, ui: &mut egui::Ui) {
        self.poll_database_inspection(ui.ctx());
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(6.0)
            .inner_margin(8.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                let path = Path::new(&self.database.path);
                let name = path.file_name().unwrap_or(path.as_os_str()).to_string_lossy().into_owned();
                let writable = self.database.writable;
                ui.horizontal(|ui| {
                    let name_width = (ui.available_width() - 56.0 - 2.0 * ui.spacing().item_spacing.x).max(1.0);
                    ui.allocate_ui_with_layout(egui::vec2(name_width, 28.0), egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.set_min_size(egui::vec2(name_width, 28.0));
                        ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
                    }).inner
                        .on_hover_text(&self.database.path);
                    for mode in [false, true] {
                        if crate::app::icons::access_button(ui, mode, Some(self.database.writable == mode)).clicked() {
                            self.database.writable = mode;
                        }
                    }
                });
                if self.database.writable != writable { self.databases.open_failure = None; }
                ui.add(egui::Label::new(&self.database.path).truncate())
                    .on_hover_text(&self.database.path);
                let inspection = &mut self.databases.inspection;
                #[cfg(test)]
                { self.databases.override_rect = None; }
                if inspection.ready(&self.database.path) && inspection.kind(&self.database.path) == crate::discovery::DatabaseKind::LikelySqlite {
                    ui.horizontal_wrapped(|ui| {
                            ui.weak("Likely SQLite");
                            let response = crate::app::presentation::checkbox(ui, &mut inspection.override_sqlite, "Open with Turso anyway");
                            #[cfg(test)]
                            { self.databases.override_rect = Some(response.rect); }
                            response.on_hover_text("Override the engine guess for this selected file. Turso still validates compatibility; this does not convert the file.");
                    });
                }
                ui.add_space(8.0);
                if self.database.writable {
                    ui.small(egui::RichText::new("Opening for writes may change the file. Back up first.").color(ui.visuals().warn_fg_color));
                }
                let enabled = !self.database.path.is_empty()
                    && self.picker.is_none()
                    && self.database_admission_available()
                    && self.databases.inspection.allows_open(&self.database.path)
                    && !self.has_drafts();
                let response = ui.horizontal_wrapped(|ui| {
                    let response = ui.add_enabled(enabled, crate::app::presentation::action(ui,
                        if self.databases.open_failure.is_some() { "Retry" } else { "Open" },
                        crate::app::presentation::Intent::Primary).min_size(egui::vec2(70.0,30.0)));
                    if ui.button("Cancel").clicked() {
                        self.cancel_database_review();
                    }
                    response
                }).inner;
                #[cfg(test)]
                {
                    self.databases.open_rect = Some(response.rect);
                    self.databases.open_enabled = response.enabled();
                }
                if response.clicked() {
                    self.open(ui.ctx());
                }
                let blocked = self.databases.inspection.blocked(&self.database.path).is_some();
                let mut recheck = false;
                let issue = self.databases.open_failure.as_ref()
                    .or_else(|| self.databases.inspection.blocked(&self.database.path));
                if let Some(error) = issue {
                    let title = if self.databases.open_failure.is_some() { "Couldn’t open database" }
                        else if error.kind == tursobench_core::ErrorKind::Busy { "Database in use" }
                        else { "Cannot open database" };
                    let color = ui.visuals().error_fg_color;
                    egui::Frame::new().fill(color.gamma_multiply(0.09))
                        .stroke(egui::Stroke::new(1.0,color.gamma_multiply(0.4)))
                        .corner_radius(6).inner_margin(10.0).show(ui, |ui| {
                            ui.colored_label(color, egui::RichText::new(title).strong());
                            ui.add(egui::Label::new(&error.message).wrap());
                            if blocked {
                                recheck = ui.button("Check again")
                                    .on_hover_text("Recheck this file’s availability and locks")
                                    .clicked();
                            }
                        });
                } else if !self.databases.inspection.ready(&self.database.path) {
                    ui.horizontal(|ui| { ui.spinner(); ui.weak("Checking file…"); });
                }
                if recheck {
                    self.databases.open_failure = None;
                    self.databases.inspection.recheck();
                }
                ui.add_space(8.0);

            });
    }

    fn database_path_ui(&mut self, ui: &mut egui::Ui, focus: bool) {
        let mut path = self.database.path.clone();
        let response = ui.add(
            egui::TextEdit::singleline(&mut path)
                .id(egui::Id::unique("database-path"))
                .desired_width(ui.available_width())
                .hint_text("/path/to/existing.db"),
        );
        if focus {
            response.request_focus();
        }
        if response.changed() {
            self.set_database_path(path);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, egui};
    use crate::app::databases::{Chooser, Entry, State};
    use crate::app::test_ui::inspect_selected as inspect;
    use crate::discovery::Availability;

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> Vec<String> {
        let output = crate::app::test_ui::render(ctx, size, events, |ui| app.render(ui));
        output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) => Some(text.galley.job.text.clone()),
                _ => None,
            })
            .collect()
    }

    fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                crate::app::test_ui::pointer(pos, pressed),
                egui::vec2(1280.0, 720.0),
            );
        }
    }

    #[test]
    fn sqlite_guess_can_be_overridden_and_opens_with_turso() {
        use crate::discovery::DatabaseKind;
        use std::io::{Seek, SeekFrom, Write};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("shared-format.db");
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);
        // A valid shared-format file whose writer metadata resembles SQLite.
        // This exercises the user correcting a false negative for Turso.
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(92)).unwrap();
        file.write_all(&1u32.to_be_bytes()).unwrap();
        file.write_all(&3_047_000u32.to_be_bytes()).unwrap();
        drop(file);
        let before = crate::test_support::snapshot(dir.path());
        let (ctx, mut app) = crate::app::test_ui::app();
        app.set_database_path(path.to_str().unwrap().into());

        inspect(&mut app, &ctx);
        assert_eq!(
            app.databases.inspection.kind(&app.database.path),
            DatabaseKind::LikelySqlite
        );
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(texts.iter().any(|t| t.contains("Likely SQLite")));
        assert!(!app.databases.open_enabled);
        app.open(&ctx);
        assert!(app.database.session.is_none());
        assert!(app.database.status.contains("override"));
        let checkbox = app.databases.override_rect.unwrap().center();
        click(&mut app, &ctx, checkbox);
        assert!(app.databases.inspection.override_sqlite);
        assert!(app.databases.open_enabled);
        assert_eq!(
            crate::test_support::snapshot(dir.path()),
            before,
            "classification and override must not write database files"
        );
        app.open(&ctx);
        let started = std::time::Instant::now();
        while !app.database.ready
            || app.database.document.active.is_some()
            || app.database.catalog_ticket.is_some()
            || app.database.ui.diagram.ticket.is_some()
        {
            app.poll(&ctx);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "{}",
                app.database.status
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.database.schema.iter().any(|t| t.name == "example"));
        app.close();
        while app.database.session.is_some() {
            app.poll(&ctx);
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Consent belongs to a selection, not a lasting preference.
        app.set_database_path(dir.path().join("other.db").to_str().unwrap().into());
        assert!(!app.databases.inspection.override_sqlite);
        app.set_database_path(path.to_str().unwrap().into());
        inspect(&mut app, &ctx);
        assert!(!app.databases.inspection.allows_open(&app.database.path));
    }

    #[test]
    fn access_panel_uses_automatic_checks_and_picker_selection_resets_write_access() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("example.db");
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER)"]);
        let (ctx, mut app) = crate::app::test_ui::app();
        app.set_database_path(path.to_string_lossy().into_owned());
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(!app.databases.open_enabled);
        let open = app.databases.open_rect.unwrap().center();
        click(&mut app, &ctx, open);
        assert!(app.database.session.is_none());

        inspect(&mut app, &ctx);
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(app.databases.open_enabled);
        assert!(app.databases.edit_path_rect.is_none());
        assert!(
            !texts
                .iter()
                .any(|t| t == "Check again" || t == "Edit path…")
        );
        assert!(!app.database.writable);
        for (name, writable) in [("Read/write", true), ("Read only", false)] {
            let output =
                crate::app::test_ui::render(&ctx, egui::vec2(1280.0, 720.0), vec![], |ui| {
                    app.render(ui)
                });
            let icon = crate::app::test_ui::control(&output, name);
            click(&mut app, &ctx, icon.center());
            assert_eq!(app.database.writable, writable);
            let output =
                crate::app::test_ui::render(&ctx, egui::vec2(1280.0, 720.0), vec![], |ui| {
                    app.render(ui)
                });
            assert!(
                crate::app::test_ui::nodes(&output)
                    .iter()
                    .any(|(_, node)| node.label() == Some(name)
                        && node.toggled() == Some(egui::accesskit::Toggled::True))
            );
            assert!(
                app.database.session.is_none(),
                "mode choice must not open the file"
            );
        }
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        app.picker = Some(rx);
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(!app.databases.open_enabled);
        app.picker = None;

        let other = dir.path().join("other.db");
        crate::test_support::create(&other, &["CREATE TABLE other(id INTEGER)"]);
        app.database.writable = true;
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        app.picker = Some(rx);
        tx.send(Ok(Some(other.clone()))).unwrap();
        app.poll_picker();
        assert_eq!(app.database.path, other.to_str().unwrap());
        assert!(!app.database.writable);
        assert!(!app.databases.inspection.ready(&app.database.path));
        assert!(app.database.session.is_none());
    }

    #[test]
    fn search_settings_are_separate_from_access_review() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.set_database_path("/work/example.db".into());

        app.database.writable = true;
        app.databases.message = Some("Folder excluded.".into());
        app.databases.summary = Some(crate::discovery::Summary {
            limited: true,
            error: Some("Search worker disconnected".into()),
            ..Default::default()
        });
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        for hidden in [
            "Search folders",
            "Search settings…",
            "Search a folder…",
            "Folder excluded.",
            "Limit reached",
            "Search worker disconnected",
            "Searched part of the configured locations.",
        ] {
            assert!(!texts.iter().any(|text| text == hidden), "{hidden}");
        }
        assert!(app.databases.refresh_rect.unwrap().width() <= 30.0);
        assert!(!app.settings.open);
        app.execute_command(&ctx, crate::app::commands::Command::Settings);
        app.settings.page = crate::app::settings::Page::Discovery;
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(app.settings.open);
        assert!(
            texts
                .iter()
                .any(|text| text.starts_with("Search folders ·"))
        );
        // Results stay pinned below the folder list when the window is scrolled.
        let settings_rect = ctx
            .memory(|memory| memory.area_rect(egui::Id::unique("application-settings")))
            .unwrap();
        frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::PointerMoved(egui::pos2(
                    settings_rect.center().x,
                    settings_rect.bottom() - 20.0,
                )),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(0.0, -1000.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            egui::vec2(1280.0, 720.0),
        );
        for _ in 0..10 {
            frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        }
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        for visible in [
            "Folder excluded.",
            "Limit reached",
            "Search worker disconnected",
        ] {
            assert!(
                texts.iter().any(|text| text == visible),
                "{visible}: {texts:?}"
            );
        }
        assert_eq!(app.database.path, "/work/example.db");
        assert!(app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn known_failures_are_visible_and_disable_open_then_recheck_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing.db");
        for dark in [false, true] {
            let ctx = egui::Context::default();
            crate::appearance::theme(&ctx, dark);
            let mut app = App::new(&ctx);
            app.set_database_path(path.to_string_lossy().into_owned());
            inspect(&mut app, &ctx);
            let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
            assert!(texts.iter().any(|t| t == "Cannot open database"));
            assert!(texts.iter().any(|t| t == "File not found."));
            assert!(texts.iter().any(|t| t == "Check again"));
            assert!(!app.databases.open_enabled);
            assert!(app.databases.open_rect.unwrap().bottom() < 640.0);
            assert!(
                !texts
                    .iter()
                    .any(|t| t.contains("multiprocess") || t.contains("Selected database"))
            );
            crate::test_support::create(&path, &["CREATE TABLE ready(id INTEGER)"]);
            app.databases.inspection.recheck();
            let start = std::time::Instant::now();
            loop {
                app.poll_database_inspection(&ctx);
                if app.databases.inspection.allows_open(&app.database.path) {
                    break;
                }
                assert!(start.elapsed() < std::time::Duration::from_secs(5));
                std::thread::sleep(std::time::Duration::from_millis(2));
            }
            let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
            assert!(app.databases.open_enabled);
            assert!(!texts.iter().any(|t| t == "Cannot open database"));
            assert!(!texts.iter().any(|t| t == "Check again"));
            std::fs::remove_file(&path).unwrap();
            let _ = std::fs::remove_file(path.with_extension("db-wal"));
        }
    }

    #[test]
    fn engine_open_failure_stays_visible_until_retry_or_path_change() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("damaged.db");
        crate::test_support::create(&path, &["CREATE TABLE damaged(id INTEGER)"]);
        // Valid recognizable header, missing pages: preflight succeeds, engine fails.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(100);
        std::fs::write(&path, bytes).unwrap();
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let (ctx, mut app) = crate::app::test_ui::app();
        app.set_database_path(path.to_string_lossy().into_owned());
        inspect(&mut app, &ctx);
        assert!(app.databases.inspection.allows_open(&app.database.path));
        app.open(&ctx);
        let start = std::time::Instant::now();
        while app.databases.open_failure.is_none() {
            app.poll(&ctx);
            assert!(
                start.elapsed() < std::time::Duration::from_secs(5),
                "{}",
                app.database.status
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        let reason = app.databases.open_failure.as_ref().unwrap().message.clone();
        app.database.status = "Unrelated status change".into();
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
        assert!(texts.iter().any(|t| t == "Couldn’t open database"));
        assert!(texts.iter().any(|t| t == &reason));
        assert!(texts.iter().any(|t| t == "Retry"));
        assert!(app.databases.open_enabled);
        app.set_database_path(
            dir.path()
                .join("different.db")
                .to_string_lossy()
                .into_owned(),
        );
        assert!(app.databases.open_failure.is_none());
    }

    #[test]
    fn sidebar_keeps_access_controls_visible_in_both_themes() {
        for dark in [true, false] {
            for (width, height) in [
                (720.0, 480.0),
                (960.0, 640.0),
                (1280.0, 720.0),
                (1920.0, 1080.0),
            ] {
                let ctx = egui::Context::default();
                crate::appearance::theme(&ctx, dark);
                let mut app = App::new(&ctx);
                app.databases = Chooser::default();
                app.set_database_path("/work/recent.db".into());
                app.databases.entries.push(Entry {
                    path: app.database.path.clone().into(),
                    state: State::Remembered,
                    size: None,
                    last_opened: Some(1),
                    kind: crate::discovery::DatabaseKind::Unknown,
                });
                for i in 0..80 {
                    app.databases.entries.push(Entry {
                        path: format!("/work/{i}.db").into(),
                        state: if i == 0 {
                            State::Checked(Availability::Missing)
                        } else {
                            State::Found
                        },
                        size: None,
                        last_opened: None,
                        kind: crate::discovery::DatabaseKind::Unknown,
                    });
                }
                let size = egui::vec2(width, height);
                frame(&mut app, &ctx, vec![], size);
                let texts = frame(&mut app, &ctx, vec![], size);
                let open = app.databases.open_rect.unwrap();
                assert!(
                    app.database.ui.sidebar_rect.contains_rect(open),
                    "{width}x{height}: {open:?}, sidebar {:?}",
                    app.database.ui.sidebar_rect
                );
                assert!(app.databases.search_rect.unwrap().width() <= 800.0);
                assert!(app.databases.row_rects.len() <= (height / 34.0).ceil() as usize);
                assert!(texts.iter().any(|text| text == "recent.db"));
                assert_eq!(app.databases.entries[1].badge(), Some("Missing"));
                assert!(!app.databases.entries[1].selectable());
            }
        }
    }
}
