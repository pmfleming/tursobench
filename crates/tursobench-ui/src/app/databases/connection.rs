use super::App;
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn connect_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical()
            .id_salt("database-connection")
            .show(ui, |ui| {
                let width = ui.available_width().min(800.0);
                let inset = ((ui.available_width() - width) / 2.0).max(0.0);
                let rect = egui::Rect::from_min_size(
                    ui.cursor().min + egui::vec2(inset, 0.0),
                    egui::vec2(width, ui.available_height()),
                );
                ui.scope_builder(egui::UiBuilder::new().max_rect(rect), |ui| {
                        ui.set_width(width);
                        ui.add_space(16.0);
                        ui.heading("Open database");
                        ui.label("Choose a recent database or browse for a local file.");
                        ui.add_space(12.0);

                        if self.path.is_empty() {
                            let edit = ui.link("Enter a path…");
                            #[cfg(test)]
                            {
                                self.databases.edit_path_rect = Some(edit.rect);
                                self.databases.open_rect = None;
                                self.databases.open_enabled = false;
                            }
                            if edit.clicked() {
                                self.databases.editing_path = !self.databases.editing_path;
                            }
                            if self.databases.editing_path {
                                self.database_path_ui(ui, edit.clicked());
                            }
                        } else {
                            self.database_access_ui(ui);
                        }
                        if self.has_drafts() {
                            ui.colored_label(ui.visuals().error_fg_color, "Drafts retained after disconnect. Inspect the uncertain outcome before discarding or retrying.");
                            if ui.button("Review retained drafts…").clicked() {
                                self.document.show_preview = true;
                            }
                            if ui.button("Discard retained local drafts").clicked() {
                                self.document.drafts = None;
                            }
                        }
                });
            });
        let mut open = self.databases.settings_open;
        egui::Window::new("Search settings")
            .id(egui::Id::unique("database-search-settings"))
            .open(&mut open)
            .collapsible(false)
            .default_width(420.0)
            .min_width(280.0)
            .max_height(480.0)
            .vscroll(true)
            .show(ui.ctx(), |ui| {
                self.discovery_settings_ui(ui);
                if let Some(message) = &self.databases.message {
                    ui.label(message);
                }
            });
        self.databases.settings_open = open;
    }

    fn database_access_ui(&mut self, ui: &mut egui::Ui) {
        self.poll_database_inspection(ui.ctx());
        egui::Frame::new()
            .fill(ui.visuals().faint_bg_color)
            .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
            .corner_radius(10.0)
            .inner_margin(16.0)
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.small("Selected database");
                let path = Path::new(&self.path);
                ui.add(
                    egui::Label::new(egui::RichText::new(
                        path.file_name().unwrap_or(path.as_os_str()).to_string_lossy(),
                    ).strong()).truncate(),
                );
                ui.add(egui::Label::new(&self.path).truncate())
                    .on_hover_text(&self.path);
                let edit = ui.link(if self.databases.editing_path { "Done editing path" } else { "Edit path…" });
                #[cfg(test)]
                {
                    self.databases.edit_path_rect = Some(edit.rect);
                }
                if edit.clicked() {
                    self.databases.editing_path = !self.databases.editing_path;
                }
                if self.databases.editing_path {
                    self.database_path_ui(ui, edit.clicked());
                }
                let inspection = &mut self.databases.inspection;
                #[cfg(test)]
                { self.databases.override_rect = None; }
                if inspection.ready(&self.path) {
                    let kind = inspection.kind(&self.path);
                    ui.horizontal_wrapped(|ui| {
                        let response = ui.label(format!("{} · {}", kind.symbol(), kind.label()))
                            .on_hover_text(kind.explanation());
                        if let Some(error) = inspection.error() {
                            response.on_hover_text(format!("Header inspection unavailable: {error}. Opening will validate the file."));
                        }
                        if kind == crate::discovery::DatabaseKind::LikelySqlite {
                            let response = ui.checkbox(&mut inspection.override_sqlite, "Open with Turso anyway");
                            #[cfg(test)]
                            { self.databases.override_rect = Some(response.rect); }
                            response.on_hover_text("Override the engine guess for this selected file. Turso still validates compatibility; this does not convert the file.");
                        }
                    });
                } else {
                    ui.label("Checking database type…");
                }
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.label("Access");
                    ui.selectable_value(&mut self.writable, false, "Read only");
                    ui.selectable_value(&mut self.writable, true, "Read and write");
                });
                if self.writable {
                    ui.colored_label(ui.visuals().warn_fg_color, "Back up closed files with all required WAL state before trying writes.");
                }
                let acknowledgement = ui.checkbox(
                    &mut self.exclusive,
                    "No other application has this database open",
                );
                #[cfg(test)]
                {
                    self.databases.acknowledgement_rect = Some(acknowledgement.rect);
                }
                acknowledgement.on_hover_text("Exclusive use is required for both access modes. This acknowledgement is not an OS lock.");
                ui.add_space(8.0);
                let enabled = !self.path.is_empty()
                    && self.picker.is_none()
                    && self.exclusive
                    && self.databases.inspection.allows_open(&self.path)
                    && !self.has_drafts();
                let response = ui.add_enabled(
                    enabled,
                    egui::Button::new(egui::RichText::new("Open database").color(egui::Color32::WHITE))
                        .fill(egui::Color32::from_rgb(4, 120, 87))
                        .min_size(egui::vec2(150.0, 36.0)),
                );
                #[cfg(test)]
                {
                    self.databases.open_rect = Some(response.rect);
                    self.databases.open_enabled = response.enabled();
                }
                if response.clicked() {
                    self.open(ui.ctx());
                }
            });
    }

    fn database_path_ui(&mut self, ui: &mut egui::Ui, focus: bool) {
        let mut path = self.path.clone();
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
    use super::*;
    use crate::app::databases::{Chooser, Entry, State};
    use crate::discovery::Availability;

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        size: egui::Vec2,
    ) -> Vec<String> {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
                events,
                ..Default::default()
            },
            |ui| app.render(ui),
        );
        output.textures_delta.clear();
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
                vec![
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                egui::vec2(1280.0, 720.0),
            );
        }
    }

    fn inspect(app: &mut App, ctx: &egui::Context) {
        let started = std::time::Instant::now();
        loop {
            app.poll_database_inspection(ctx);
            if app.databases.inspection.ready(&app.path) {
                break;
            }
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.set_database_path(path.to_str().unwrap().into());
        app.exclusive = true;
        inspect(&mut app, &ctx);
        assert_eq!(
            app.databases.inspection.kind(&app.path),
            DatabaseKind::LikelySqlite
        );
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(texts.iter().any(|t| t.contains("Likely SQLite")));
        assert!(!app.databases.open_enabled);
        app.open(&ctx);
        assert!(app.session.is_none());
        assert!(app.status.contains("override"));
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
        while !app.ready
            || app.document.active.is_some()
            || app.catalog_ticket.is_some()
            || app.ui.diagram.ticket.is_some()
        {
            app.poll(&ctx);
            assert!(
                started.elapsed() < std::time::Duration::from_secs(5),
                "{}",
                app.status
            );
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        assert!(app.schema.iter().any(|t| t.name == "example"));
        app.close();
        while app.session.is_some() {
            app.poll(&ctx);
            assert!(started.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        // Consent belongs to a selection, not a lasting preference.
        app.set_database_path(dir.path().join("other.db").to_str().unwrap().into());
        assert!(!app.databases.inspection.override_sqlite);
        app.set_database_path(path.to_str().unwrap().into());
        inspect(&mut app, &ctx);
        assert!(!app.databases.inspection.allows_open(&app.path));
    }

    #[test]
    fn access_panel_requires_consent_and_path_editing_resets_it() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.set_database_path("/work/example.db".into());
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(!app.databases.open_enabled);
        let open = app.databases.open_rect.unwrap().center();
        click(&mut app, &ctx, open);
        assert!(app.session.is_none());

        let acknowledgement = app.databases.acknowledgement_rect.unwrap().center();
        click(&mut app, &ctx, acknowledgement);
        inspect(&mut app, &ctx);
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(app.exclusive && app.databases.open_enabled);
        assert!(!app.writable);
        let (_tx, rx) = std::sync::mpsc::sync_channel(1);
        app.picker = Some(rx);
        frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(!app.databases.open_enabled);
        app.picker = None;

        let edit = app.databases.edit_path_rect.unwrap().center();
        click(&mut app, &ctx, edit);
        assert!(app.databases.editing_path);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::unique("database-path"))));
        app.writable = true;
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("x".into())],
            egui::vec2(1280.0, 720.0),
        );
        assert_ne!(app.path, "/work/example.db");
        assert!(!app.exclusive, "path: {}", app.path);
        assert!(!app.writable);
        assert!(!app.databases.open_enabled);
        assert!(app.session.is_none());
    }

    #[test]
    fn search_settings_are_separate_from_access_review() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.set_database_path("/work/example.db".into());
        app.exclusive = true;
        app.writable = true;
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
        assert!(app.databases.settings_rect.unwrap().width() <= 30.0);
        let settings = app.databases.settings_rect.unwrap().center();
        click(&mut app, &ctx, settings);
        let texts = frame(&mut app, &ctx, vec![], egui::vec2(1280.0, 720.0));
        assert!(app.databases.settings_open);
        for visible in [
            "Search folders",
            "Folder excluded.",
            "Limit reached",
            "Search worker disconnected",
        ] {
            assert!(texts.iter().any(|text| text == visible), "{visible}");
        }
        assert_eq!(app.path, "/work/example.db");
        assert!(app.exclusive && app.writable);
        assert!(app.session.is_none());
    }

    #[test]
    fn chooser_keeps_access_controls_visible_in_both_themes() {
        for dark in [true, false] {
            for (width, height) in [(960.0, 640.0), (1280.0, 720.0), (1920.0, 1080.0)] {
                let ctx = egui::Context::default();
                crate::appearance::theme(&ctx, dark);
                let mut app = App::new(&ctx);
                app.databases = Chooser::default();
                app.set_database_path("/work/recent.db".into());
                app.databases.entries.push(Entry {
                    path: app.path.clone().into(),
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
                    app.ui.workspace_rect.contains_rect(open),
                    "{width}x{height}: {open:?}, workspace {:?}",
                    app.ui.workspace_rect
                );
                assert!(app.databases.search_rect.unwrap().width() <= 800.0);
                assert!(app.databases.row_rects.len() < 10);
                assert!(texts.iter().any(|text| text == "recent.db"));
                assert!(texts.iter().any(|text| text == "Missing"));
                assert!(!app.databases.entries[1].selectable());
            }
        }
    }
}
