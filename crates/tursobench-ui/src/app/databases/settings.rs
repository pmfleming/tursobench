use super::App;
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(in crate::app) enum FolderList {
    #[default]
    Search,
    Excluded,
}

impl App {
    /// Shared by Available and Settings; neither action changes search policy.
    pub(in crate::app) fn discovery_actions(&mut self, ui: &mut egui::Ui) -> egui::Response {
        if self.databases.job.is_some() {
            ui.spinner().on_hover_text(if self.databases.cancelling {
                "Stopping…"
            } else {
                "Searching…"
            });
            let response = ui
                .add_enabled_ui(!self.databases.cancelling, |ui| {
                    icons::button(ui, Icon::Close, "Stop search", "")
                })
                .inner;
            if response.clicked() {
                self.databases.cancel();
            }
            response
        } else {
            let response = ui
                .add_enabled_ui(self.picker.is_none(), |ui| {
                    icons::button(ui, Icon::Refresh, "Refresh databases", "")
                })
                .inner;
            if response.clicked() {
                self.start_discovery(ui.ctx(), None);
            }
            response
        }
    }

    pub(in crate::app) fn discovery_settings_ui(&mut self, ui: &mut egui::Ui) {
        let bottom = ui.available_rect_before_wrap().bottom();
        let footer = egui::Panel::bottom("discovery-results")
            .resizable(false)
            .frame(egui::Frame::NONE)
            .show(ui, |ui| {
                if let Some(summary) = &self.databases.summary {
                    discovery_summary(ui, summary);
                }
                if let Some(message) = &self.databases.message {
                    ui.add(egui::Label::new(message).truncate())
                        .on_hover_text(message);
                }
            });
        if (footer.response.rect.bottom() - bottom).abs() > 1.0 {
            ui.ctx().request_discard("Discovery results changed size");
        }
        ui.horizontal(|ui| {
            if crate::app::presentation::checkbox(
                ui,
                &mut self.preferences.discovery.enabled,
                "At startup",
            )
            .on_hover_text("Search for databases on startup")
            .changed()
            {
                if !self.preferences.discovery.enabled {
                    self.databases.cancel();
                }
                self.store.save(&self.preferences);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                self.discovery_actions(ui);
            });
        });
        let mut roots = super::search_roots(&self.preferences.discovery);
        ui.horizontal_wrapped(|ui| {
            for (list, title, count) in [
                (FolderList::Search, "Search folders", roots.len()),
                (
                    FolderList::Excluded,
                    "Excluded",
                    self.preferences.discovery.excluded.len(),
                ),
            ] {
                let response = ui.selectable_label(
                    self.databases.folder_list == list,
                    format!("{title} · {count}"),
                );
                response.widget_info(|| {
                    egui::WidgetInfo::selected(
                        egui::Role::Tab,
                        true,
                        self.databases.folder_list == list,
                        title,
                    )
                });
                if response.clicked() {
                    self.databases.folder_list = list;
                }
            }
        });
        let excluded = self.databases.folder_list == FolderList::Excluded;
        let (add, browse) = folder_input(
            ui,
            &mut self.databases.folder_input,
            if excluded {
                "Excluded folder path"
            } else {
                "Search folder path"
            },
            if excluded {
                "Exclude folder"
            } else {
                "Add and search folder"
            },
            if excluded {
                "Browse excluded folder"
            } else {
                "Browse search folder"
            },
            self.picker.is_none(),
        );
        if browse {
            self.browse_search_folder(ui.ctx());
        }
        if add {
            let path = PathBuf::from(&self.databases.folder_input);
            if excluded {
                self.add_exclusion(path);
            } else {
                self.add_search_folder(path);
            }
        }
        ui.add_space(4.0);
        let remove = egui::ScrollArea::vertical()
            .id_salt(("discovery-folders", excluded))
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if excluded {
                    if self.preferences.discovery.excluded.is_empty() {
                        ui.weak("No excluded folders");
                    }
                    folder_rows(
                        ui,
                        self.preferences
                            .discovery
                            .excluded
                            .iter()
                            .map(PathBuf::as_path),
                        "Remove exclusion",
                    )
                } else {
                    if roots.is_empty() {
                        ui.weak("No search folders");
                    }
                    folder_rows(
                        ui,
                        roots.iter().map(|r| r.path.as_path()),
                        "Remove search folder",
                    )
                }
            })
            .inner;
        if let Some(index) = remove {
            if excluded {
                self.preferences.discovery.excluded.remove(index);
                self.store.save(&self.preferences);
                self.databases.message = Some("Exclusion removed.".into());
            } else {
                self.remove_search_folder(roots.remove(index).path);
            }
        }
    }
}

fn folder_rows<'a>(
    ui: &mut egui::Ui,
    paths: impl Iterator<Item = &'a Path>,
    action: &str,
) -> Option<usize> {
    let mut remove = None;
    for (index, path) in paths.enumerate() {
        if ui.push_id(path, |ui| folder_row(ui, path, action)).inner {
            remove = Some(index);
        }
    }
    remove
}

fn discovery_summary(ui: &mut egui::Ui, summary: &crate::discovery::Summary) {
    ui.separator();
    ui.horizontal_wrapped(|ui| {
        metric(ui, Icon::Database, summary.probes, "Headers checked");
        metric(ui, Icon::Folder, summary.directories, "Folders searched");
        for (icon, count, name) in [
            (Icon::Lock, summary.unavailable, "Unavailable locations"),
            (Icon::Filter, summary.skipped, "Skipped locations"),
        ] {
            if count > 0 {
                metric(ui, icon, count, name);
            }
        }
        if summary.limited {
            ui.colored_label(ui.visuals().warn_fg_color, "Limit reached")
                .on_hover_text("Only part of the configured locations was searched");
        } else if summary.cancelled {
            ui.weak("Stopped");
        }
    });
    if let Some(error) = &summary.error {
        ui.add(
            egui::Label::new(egui::RichText::new(error).color(ui.visuals().warn_fg_color))
                .truncate(),
        )
        .on_hover_text(error);
    }
}

/// Keep the shared input and both actions on one line at 280 logical pixels.
fn folder_input(
    ui: &mut egui::Ui,
    value: &mut String,
    name: &str,
    action: &str,
    browse_name: &str,
    browse_enabled: bool,
) -> (bool, bool) {
    ui.horizontal(|ui| {
        let width = (ui.available_width() - 2.0 * (28.0 + ui.spacing().item_spacing.x)).max(40.0);
        let input = ui.add_sized(
            [width, 28.0],
            egui::TextEdit::singleline(value)
                .id_salt("path")
                .hint_text("/path/to/folder")
                .char_limit(4096),
        );
        input.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, ui.is_enabled(), name)
        });
        let enter = input.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let browse = ui
            .add_enabled_ui(browse_enabled, |ui| {
                icons::button(ui, Icon::Folder, browse_name, "")
            })
            .inner
            .clicked();
        let add = ui
            .add_enabled_ui(!value.trim().is_empty(), |ui| {
                icons::button(ui, Icon::Plus, action, "Enter")
            })
            .inner
            .clicked();
        (!value.trim().is_empty() && (add || enter), browse)
    })
    .inner
}

fn folder_row(ui: &mut egui::Ui, path: &Path, action: &str) -> bool {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .corner_radius(7.0)
        .inner_margin(egui::Margin::symmetric(4, 0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                small_icon(ui, Icon::Folder);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let remove = icons::button(ui, Icon::Close, action, "").clicked();
                    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                        ui.add(egui::Label::new(path.to_string_lossy()).truncate())
                            .on_hover_text(path.to_string_lossy());
                    });
                    remove
                })
                .inner
            })
            .inner
        })
        .inner
}

fn small_icon(ui: &mut egui::Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
    icons::paint(ui.painter(), rect, icon, ui.visuals().weak_text_color());
}

fn metric(ui: &mut egui::Ui, icon: Icon, count: usize, name: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        small_icon(ui, icon);
        let response = ui.weak(count.to_string());
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Label,
                ui.is_enabled(),
                format!("{name}: {count}"),
            )
        });
    })
    .response
    .on_hover_text(format!("{name}: {count}"));
}

#[cfg(test)]
mod tests {
    use super::{App, FolderList, PathBuf, egui};
    use crate::app::test_ui::{control, key_event, pointer};

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
    ) -> egui::FullOutput {
        crate::app::test_ui::render(ctx, egui::vec2(width, 900.0), events, |ui| {
            app.discovery_settings_ui(ui)
        })
    }

    fn click(app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
        let pos = rect.center();
        for pressed in [true, false] {
            frame(app, ctx, pointer(pos, pressed), 420.0);
        }
    }

    #[test]
    fn folder_controls_add_remove_and_restore_saved_search_policy() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let roots = super::super::search_roots(&app.preferences.discovery);
        assert!(!roots.is_empty());
        frame(&mut app, &ctx, vec![], 420.0);
        let output = crate::app::test_ui::render(&ctx, egui::vec2(420.0, 600.0), vec![], |ui| {
            app.discovery_settings_ui(ui)
        });
        assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == roots[0].path.to_string_lossy())));
        // Isolate a default root so its removal control is unambiguous.
        app.preferences.discovery.removed_roots =
            roots.iter().skip(1).map(|r| r.path.clone()).collect();
        let nodes = frame(&mut app, &ctx, vec![], 420.0);
        click(&mut app, &ctx, control(&nodes, "Remove search folder"));
        app.preferences.discovery.normalize();
        let saved = serde_json::to_string(&app.preferences.discovery).unwrap();
        let restored = serde_json::from_str(&saved).unwrap();
        assert!(super::super::search_roots(&restored).is_empty());
        assert!(super::super::request(&restored, None).roots.is_empty());
        app.preferences.discovery = restored;
        app.add_search_folder(roots[0].path.clone());
        let restored = super::super::search_roots(&app.preferences.discovery);
        assert_eq!(restored.len(), 1);
        assert_eq!(restored[0].path, roots[0].path);
        assert!(restored[0].explicit);

        let roots = super::super::search_roots(&app.preferences.discovery);
        app.preferences
            .discovery
            .removed_roots
            .extend(roots.into_iter().map(|r| r.path));
        app.preferences.discovery.roots.clear();
        app.preferences.discovery.excluded.clear();
        app.database.path = "/work/selected.db".into();

        app.database.writable = true;

        for (input, path) in [
            ("Search folder path", "/work/projects"),
            ("Excluded folder path", "/work/projects/private"),
        ] {
            app.databases.folder_list = if input == "Excluded folder path" {
                FolderList::Excluded
            } else {
                FolderList::Search
            };
            let nodes = frame(&mut app, &ctx, vec![], 420.0);
            click(&mut app, &ctx, control(&nodes, input));
            frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::Text(path.into()),
                    key_event(egui::Key::Enter, true, egui::Modifiers::NONE),
                    key_event(egui::Key::Enter, false, egui::Modifiers::NONE),
                ],
                420.0,
            );
        }
        assert_eq!(
            app.preferences.discovery.roots,
            vec![PathBuf::from("/work/projects")]
        );
        assert_eq!(
            app.preferences.discovery.excluded,
            vec![PathBuf::from("/work/projects/private")]
        );
        assert!(app.databases.folder_input.is_empty());
        for action in ["Remove search folder", "Remove exclusion"] {
            app.databases.folder_list = if action == "Remove exclusion" {
                FolderList::Excluded
            } else {
                FolderList::Search
            };
            let nodes = frame(&mut app, &ctx, vec![], 420.0);
            click(&mut app, &ctx, control(&nodes, action));
        }
        assert!(app.preferences.discovery.roots.is_empty());
        assert!(app.preferences.discovery.excluded.is_empty());
        assert_eq!(app.database.path, "/work/selected.db");
        assert!(app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn named_controls_fit_narrow_settings_in_both_themes() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            crate::appearance::theme(&ctx, dark);
            let mut app = App::new(&ctx);
            app.preferences.discovery.roots = vec![PathBuf::from(format!(
                "/work/{}",
                "long-folder/".repeat(30)
            ))];
            app.preferences.discovery.excluded = vec![PathBuf::from("/work/private")];
            for width in [280.0, 420.0] {
                for (list, names) in [
                    (
                        FolderList::Search,
                        [
                            "Search folder path",
                            "Browse search folder",
                            "Add and search folder",
                            "Remove search folder",
                        ],
                    ),
                    (
                        FolderList::Excluded,
                        [
                            "Excluded folder path",
                            "Browse excluded folder",
                            "Exclude folder",
                            "Remove exclusion",
                        ],
                    ),
                ] {
                    app.databases.folder_list = list;
                    frame(&mut app, &ctx, vec![], width);
                    let nodes = frame(&mut app, &ctx, vec![], width);
                    for name in
                        names
                            .into_iter()
                            .chain(["Refresh databases", "Search folders", "Excluded"])
                    {
                        let rect = control(&nodes, name);
                        assert!(
                            rect.left() >= 0.0 && rect.right() <= width,
                            "{name}: {rect:?} at {width}"
                        );
                    }
                }
            }
            for (limited, cancelled, status) in
                [(false, true, "Stopped"), (true, true, "Limit reached")]
            {
                app.databases.summary = Some(crate::discovery::Summary {
                    probes: 7,
                    directories: 3,
                    unavailable: 2,
                    skipped: 1,
                    limited,
                    cancelled,
                    error: Some("Unreadable search root".into()),
                    ..Default::default()
                });
                let output = frame(&mut app, &ctx, vec![], 420.0);
                for name in [
                    "Headers checked: 7",
                    "Folders searched: 3",
                    "Unavailable locations: 2",
                    "Skipped locations: 1",
                ] {
                    assert!(
                        crate::app::test_ui::nodes(&output)
                            .iter()
                            .any(|(_, node)| node.label() == Some(name))
                    );
                }
                for expected in [status, "Unreadable search root"] {
                    assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == expected)), "missing {expected}");
                }
            }
        }
    }
    #[test]
    fn excluded_picker_populates_the_originating_tab_before_add() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.path = "/work/selected.db".into();
        app.database.writable = true;
        let roots = app.preferences.discovery.roots.clone();
        app.databases.picking_folder = Some(FolderList::Excluded);
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        app.picker = Some(rx);
        // The destination is captured before the picker opens.
        app.databases.folder_list = FolderList::Search;
        tx.send(Ok(Some("/work/private".into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.databases.folder_list, FolderList::Excluded);
        assert_eq!(app.databases.folder_input, "/work/private");
        assert!(app.preferences.discovery.excluded.is_empty());
        let output = frame(&mut app, &ctx, vec![], 420.0);
        click(&mut app, &ctx, control(&output, "Exclude folder"));
        assert_eq!(
            app.preferences.discovery.excluded,
            vec![PathBuf::from("/work/private")]
        );
        assert_eq!(app.preferences.discovery.roots, roots);
        assert!(app.databases.folder_input.is_empty());
        assert_eq!(app.database.path, "/work/selected.db");
        assert!(app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn results_and_input_stay_visible_when_the_folder_list_scrolls() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.preferences.discovery.roots = (0..80)
            .map(|i| PathBuf::from(format!("/work/folder-{i}")))
            .collect();
        app.databases.summary = Some(crate::discovery::Summary {
            probes: 300,
            directories: 1000,
            limited: true,
            ..Default::default()
        });
        app.databases.message = Some("Folder excluded.".into());
        let size = egui::vec2(420.0, 400.0);
        let mut draw = |events| {
            crate::app::test_ui::render(&ctx, size, events, |ui| app.discovery_settings_ui(ui))
        };
        draw(vec![]);
        let before = draw(vec![]);
        let footer = before
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "300" => Some(t.pos),
                _ => None,
            })
            .expect("metric is painted");
        let input = control(&before, "Search folder path");
        draw(vec![
            egui::Event::PointerMoved(egui::pos2(200.0, 240.0)),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -1000.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            },
        ]);
        for _ in 0..20 {
            draw(vec![]);
        }
        let after = draw(vec![]);
        let after_footer = after
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::Shape::Text(t) if t.galley.job.text == "300" => Some(t.pos),
                _ => None,
            })
            .expect("metric stays painted");
        assert_eq!(after_footer, footer);
        assert_eq!(control(&after, "Search folder path"), input);
        assert!(footer.y <= size.y);
        assert!(after.shapes.iter().any(
            |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == "Limit reached")
        ));
        assert!(!after.shapes.iter().any(
            |s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == "/work/folder-0")
        ));
    }

    #[test]
    fn available_refresh_and_stop_share_the_search_job_without_opening_settings() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let dir = tempfile::tempdir().unwrap();
        app.preferences.discovery.removed_roots =
            super::super::search_roots(&app.preferences.discovery)
                .into_iter()
                .map(|r| r.path)
                .collect();
        app.preferences.discovery.roots = vec![dir.path().into()];
        let size = egui::vec2(280.0, 600.0);
        let output =
            crate::app::test_ui::render(&ctx, size, vec![], |ui| app.available_databases_ui(ui));
        let pos = control(&output, "Refresh databases").center();
        for pressed in [true, false] {
            crate::app::test_ui::render(&ctx, size, pointer(pos, pressed), |ui| {
                app.available_databases_ui(ui)
            });
        }
        assert!(app.databases.job.is_some());
        let output =
            crate::app::test_ui::render(&ctx, size, vec![], |ui| app.available_databases_ui(ui));
        let pos = control(&output, "Stop search").center();
        for pressed in [true, false] {
            crate::app::test_ui::render(&ctx, size, pointer(pos, pressed), |ui| {
                app.available_databases_ui(ui)
            });
        }
        assert!(app.databases.cancelling);
        assert!(
            app.databases.job.is_some(),
            "keep the worker until cancellation is acknowledged"
        );
        assert!(!app.settings.open);
    }
}
