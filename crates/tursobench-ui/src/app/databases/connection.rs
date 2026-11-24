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
                self.access_mode_ui(ui);
                ui.add(egui::Label::new(&self.database.path).truncate())
                    .on_hover_text(&self.database.path);
                self.sqlite_override_ui(ui);
                ui.add_space(8.0);
                if self.database.writable {
                    ui.small(
                        egui::RichText::new(
                            "Opening for writes may change the file. Back up first.",
                        )
                        .color(ui.visuals().warn_fg_color),
                    );
                }
                self.open_buttons_ui(ui);
                if self.open_issue_ui(ui) {
                    self.databases.open_failure = None;
                    self.databases.inspection.recheck();
                }
                ui.add_space(8.0);
            });
    }

    /// The file name beside the read-only and read/write choice; changing
    /// the mode clears an earlier open failure.
    fn access_mode_ui(&mut self, ui: &mut egui::Ui) {
        let path = Path::new(&self.database.path);
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let writable = self.database.writable;
        ui.horizontal(|ui| {
            let name_width =
                (ui.available_width() - 56.0 - 2.0 * ui.spacing().item_spacing.x).max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(name_width, 28.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.set_min_size(egui::vec2(name_width, 28.0));
                    ui.add(egui::Label::new(egui::RichText::new(name).strong()).truncate())
                },
            )
            .inner
            .on_hover_text(&self.database.path);
            for mode in [false, true] {
                if crate::app::icons::access_button(ui, mode, Some(self.database.writable == mode))
                    .clicked()
                {
                    self.database.writable = mode;
                }
            }
        });
        if self.database.writable != writable {
            self.databases.open_failure = None;
        }
    }

    /// A file that looks like SQLite opens only after the user overrides the guess.
    fn sqlite_override_ui(&mut self, ui: &mut egui::Ui) {
        let inspection = &mut self.databases.inspection;
        #[cfg(test)]
        {
            self.databases.override_rect = None;
        }
        if !inspection.likely_sqlite(&self.database.path) {
            return;
        }
        ui.horizontal_wrapped(|ui| {
            ui.weak("Likely SQLite");
            let response = crate::app::presentation::checkbox(
                ui,
                &mut inspection.override_sqlite,
                "Open with Turso anyway",
            );
            #[cfg(test)]
            {
                self.databases.override_rect = Some(response.rect);
            }
            response.on_hover_text("Override the engine guess for this selected file. Turso still validates compatibility; this does not convert the file.");
        });
    }

    fn open_buttons_ui(&mut self, ui: &mut egui::Ui) {
        let enabled = !self.database.path.is_empty()
            && self.picker.is_none()
            && self.database_admission_available()
            && self.databases.inspection.allows_open(&self.database.path)
            && !self.has_drafts();
        let label = if self.databases.open_failure.is_some() {
            "Retry"
        } else {
            "Open"
        };
        let response = ui
            .horizontal_wrapped(|ui| {
                let response = ui.add_enabled(
                    enabled,
                    crate::app::presentation::action(
                        ui,
                        label,
                        crate::app::presentation::Intent::Primary,
                    )
                    .min_size(egui::vec2(70.0, 30.0)),
                );
                if ui.button("Cancel").clicked() {
                    self.cancel_database_review();
                }
                response
            })
            .inner;
        #[cfg(test)]
        {
            self.databases.open_rect = Some(response.rect);
            self.databases.open_enabled = response.enabled();
        }
        if response.clicked() {
            self.open(ui.ctx());
        }
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
    use crate::app::databases::{
        Entry, State,
        tests::{scroll_down_at, turso_file, wait},
    };
    use crate::app::test_ui::{self, control, inspect_selected as inspect, nodes, pointer, render};
    use crate::discovery::Availability;

    const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> Vec<String> {
        let output = render(ctx, size, events, |ui| app.render(ui));
        test_ui::texts(&output)
            .map(|text| text.galley.job.text.clone())
            .collect()
    }

    fn click(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
        for pressed in [true, false] {
            frame(app, ctx, pointer(pos, pressed), SIZE);
        }
    }

    /// A fresh app drawn in the light or dark theme.
    fn themed(dark: bool) -> (egui::Context, App) {
        let (ctx, mut app) = test_ui::app();
        app.set_dark(&ctx, dark);
        (ctx, app)
    }

    #[test]
    fn sqlite_guess_can_be_overridden_and_opens_with_turso() {
        use std::io::{Seek, SeekFrom, Write};
        let (dir, path) = turso_file("shared-format.db");
        // A valid shared-format file whose writer metadata resembles SQLite.
        // This exercises the user correcting a false negative for Turso.
        let mut file = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
        file.seek(SeekFrom::Start(92)).unwrap();
        file.write_all(&1u32.to_be_bytes()).unwrap();
        file.write_all(&3_047_000u32.to_be_bytes()).unwrap();
        drop(file);
        let before = crate::test_support::snapshot(dir.path());
        let (ctx, mut app) = test_ui::app();
        app.set_database_path(path.to_str().unwrap().into());

        inspect(&mut app, &ctx);
        assert!(app.databases.inspection.likely_sqlite(&app.database.path));
        let texts = frame(&mut app, &ctx, vec![], SIZE);
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
        wait(
            &mut app,
            |app| app.poll(&ctx),
            |app| {
                app.database.ready
                    && app.database.document.active.is_none()
                    && app.database.catalog_ticket.is_none()
                    && app.database.ui.diagram.ticket.is_none()
            },
        );
        assert!(app.database.schema.iter().any(|t| t.name == "example"));
        app.close();
        wait(
            &mut app,
            |app| app.poll(&ctx),
            |app| app.database.session.is_none(),
        );
        // Consent belongs to a selection, not a lasting preference.
        app.set_database_path(dir.path().join("other.db").to_str().unwrap().into());
        assert!(!app.databases.inspection.override_sqlite);
        app.set_database_path(path.to_str().unwrap().into());
        inspect(&mut app, &ctx);
        assert!(!app.databases.inspection.allows_open(&app.database.path));
    }

    #[test]
    fn typed_path_gets_automatic_checks_and_picker_selection_resets_write_access() {
        let (dir, path) = turso_file("example.db");
        let (ctx, mut app) = test_ui::app();
        frame(&mut app, &ctx, vec![], SIZE);
        frame(&mut app, &ctx, vec![], SIZE);
        let edit = app.databases.edit_path_rect.unwrap().center();
        click(&mut app, &ctx, edit);
        let typed = egui::Event::Text(path.to_string_lossy().into_owned());
        frame(&mut app, &ctx, vec![typed], SIZE);
        assert_eq!(app.database.path, path.to_str().unwrap());
        frame(&mut app, &ctx, vec![], SIZE);
        assert!(!app.databases.open_enabled);
        let open = app.databases.open_rect.unwrap().center();
        click(&mut app, &ctx, open);
        assert!(app.database.session.is_none());

        inspect(&mut app, &ctx);
        let texts = frame(&mut app, &ctx, vec![], SIZE);
        assert!(app.databases.open_enabled);
        assert!(app.databases.edit_path_rect.is_none());
        assert!(
            !texts
                .iter()
                .any(|t| t == "Check again" || t == "Edit path…")
        );
        assert!(!app.database.writable);
        for (name, writable) in [("Read/write", true), ("Read only", false)] {
            let output = render(&ctx, SIZE, vec![], |ui| app.render(ui));
            click(&mut app, &ctx, control(&output, name).center());
            assert_eq!(app.database.writable, writable);
            let output = render(&ctx, SIZE, vec![], |ui| app.render(ui));
            assert!(
                nodes(&output)
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
        frame(&mut app, &ctx, vec![], SIZE);
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
        let (ctx, mut app) = test_ui::app();
        app.set_database_path("/work/example.db".into());

        app.database.writable = true;
        app.databases.message = Some("Folder excluded.".into());
        app.databases.summary = Some(crate::discovery::Summary {
            limited: true,
            error: Some("Search worker disconnected".into()),
            ..Default::default()
        });
        let texts = frame(&mut app, &ctx, vec![], SIZE);
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
        app.settings.open = true;
        app.settings.page = crate::app::settings::Page::Discovery;
        frame(&mut app, &ctx, vec![], SIZE);
        let texts = frame(&mut app, &ctx, vec![], SIZE);
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
            scroll_down_at(egui::pos2(
                settings_rect.center().x,
                settings_rect.bottom() - 20.0,
            )),
            SIZE,
        );
        for _ in 0..10 {
            frame(&mut app, &ctx, vec![], SIZE);
        }
        let texts = frame(&mut app, &ctx, vec![], SIZE);
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
            let (ctx, mut app) = themed(dark);
            app.set_database_path(path.to_string_lossy().into_owned());
            inspect(&mut app, &ctx);
            let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
            for text in ["Cannot open database", "File not found.", "Check again"] {
                assert!(texts.iter().any(|t| t == text), "{text}");
            }
            assert!(!app.databases.open_enabled);
            assert!(app.databases.open_rect.unwrap().bottom() < 640.0);
            assert!(
                !texts
                    .iter()
                    .any(|t| t.contains("multiprocess") || t.contains("Selected database"))
            );
            crate::test_support::create(&path, &["CREATE TABLE ready(id INTEGER)"]);
            app.databases.inspection.recheck();
            wait(
                &mut app,
                |app| app.poll_database_inspection(&ctx),
                |app| app.databases.inspection.allows_open(&app.database.path),
            );
            let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
            assert!(app.databases.open_enabled);
            assert!(
                !texts
                    .iter()
                    .any(|t| t == "Cannot open database" || t == "Check again")
            );
            std::fs::remove_file(&path).unwrap();
            let _ = std::fs::remove_file(path.with_extension("db-wal"));
        }
    }

    #[test]
    fn engine_open_failure_stays_visible_until_retry_or_path_change() {
        let (dir, path) = turso_file("damaged.db");
        // Valid recognizable header, missing pages: preflight succeeds, engine fails.
        let mut bytes = std::fs::read(&path).unwrap();
        bytes.truncate(100);
        std::fs::write(&path, bytes).unwrap();
        let _ = std::fs::remove_file(path.with_extension("db-wal"));
        let (ctx, mut app) = test_ui::app();
        app.set_database_path(path.to_string_lossy().into_owned());
        inspect(&mut app, &ctx);
        assert!(app.databases.inspection.allows_open(&app.database.path));
        app.open(&ctx);
        wait(
            &mut app,
            |app| app.poll(&ctx),
            |app| app.databases.open_failure.is_some(),
        );
        let reason = app.databases.open_failure.as_ref().unwrap().message.clone();
        app.database.status = "Unrelated status change".into();
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(960.0, 640.0));
        for text in ["Couldn’t open database", &reason, "Retry"] {
            assert!(texts.iter().any(|t| t == text), "{text}");
        }
        assert!(app.databases.open_enabled);
        let different = dir.path().join("different.db");
        app.set_database_path(different.to_string_lossy().into_owned());
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
                let (ctx, mut app) = themed(dark);
                app.set_database_path("/work/recent.db".into());
                let recent = Entry::remembered(app.database.path.clone().into(), Some(1));
                app.databases.entries.push(recent);
                for i in 0..80 {
                    let state = if i == 0 {
                        State::Checked(Availability::Missing)
                    } else {
                        State::Found
                    };
                    let entry = Entry::remembered(format!("/work/{i}.db").into(), None);
                    app.databases.entries.push(Entry { state, ..entry });
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
