use super::App;
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::{Path, PathBuf};

impl App {
    pub(in crate::app) fn discovery_settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .checkbox(&mut self.preferences.discovery.enabled, "At startup")
                .on_hover_text("Search for databases on startup")
                .changed()
            {
                if !self.preferences.discovery.enabled {
                    self.databases.cancel();
                }
                self.store.save(&self.preferences);
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.databases.job.is_some() {
                    if ui
                        .add_enabled_ui(!self.databases.cancelling, |ui| {
                            icons::button(ui, Icon::Close, "Stop search", "")
                        })
                        .inner
                        .clicked()
                    {
                        self.databases.cancel();
                    }
                    ui.spinner();
                    if ui.available_width() >= 80.0 {
                        ui.weak(if self.databases.cancelling {
                            "Stopping…"
                        } else {
                            "Searching…"
                        });
                    }
                } else if ui
                    .add_enabled_ui(self.picker.is_none(), |ui| {
                        icons::button(ui, Icon::Refresh, "Refresh databases", "")
                    })
                    .inner
                    .clicked()
                {
                    self.start_discovery(ui.ctx(), None);
                }
            });
        });
        ui.separator();

        ui.push_id("search-folders", |ui| {
            let roots = super::search_roots(&self.preferences.discovery);
            section_heading(ui, "Search folders", roots.len());
            let (add, browse) = folder_input(
                ui,
                &mut self.databases.folder_input,
                "Search folder path",
                "Add and search folder",
                Some(self.picker.is_none() && self.databases.job.is_none()),
            );
            if browse {
                self.browse_search_folder(ui.ctx());
            }
            if add {
                self.add_search_folder(PathBuf::from(&self.databases.folder_input));
            }
            let mut remove = None;
            egui::ScrollArea::vertical()
                .id_salt("search-folder-list")
                .max_height(180.0)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for root in &roots {
                        ui.push_id(&root.path, |ui| {
                            if folder_row(ui, &root.path, "Remove search folder") {
                                remove = Some(root.path.clone());
                            }
                        });
                    }
                });
            if let Some(path) = remove {
                self.remove_search_folder(path);
            }
        });
        ui.add_space(6.0);
        ui.separator();
        ui.push_id("excluded-folders", |ui| {
            section_heading(ui, "Excluded", self.preferences.discovery.excluded.len());
            let (add, _) = folder_input(
                ui,
                &mut self.databases.exclude_input,
                "Excluded folder path",
                "Exclude folder",
                None,
            );
            if add {
                self.add_exclusion(PathBuf::from(&self.databases.exclude_input));
            }
            let mut remove = None;
            for (index, path) in self.preferences.discovery.excluded.iter().enumerate() {
                ui.push_id(path, |ui| {
                    if folder_row(ui, path, "Remove exclusion") {
                        remove = Some(index);
                    }
                });
            }
            if let Some(index) = remove {
                self.preferences.discovery.excluded.remove(index);
                self.store.save(&self.preferences);
                self.databases.message = Some("Exclusion removed.".into());
            }
        });

        if let Some(summary) = &self.databases.summary {
            ui.separator();
            ui.horizontal_wrapped(|ui| {
                metric(ui, Icon::Database, summary.probes, "Headers checked");
                metric(ui, Icon::Folder, summary.directories, "Folders searched");
                if summary.unavailable > 0 {
                    metric(ui, Icon::Lock, summary.unavailable, "Unavailable locations");
                }
                if summary.skipped > 0 {
                    metric(ui, Icon::Filter, summary.skipped, "Skipped locations");
                }
                if summary.limited {
                    ui.colored_label(ui.visuals().warn_fg_color, "Limit reached");
                } else if summary.cancelled {
                    ui.weak("Stopped");
                }
            });
            if let Some(error) = &summary.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
        }
    }
}

fn section_heading(ui: &mut egui::Ui, title: &str, count: usize) {
    ui.horizontal(|ui| {
        ui.label(title);
        ui.weak(count.to_string());
    });
}

/// Keep the input and its actions on one line, including in the settings sidebar.
fn folder_input(
    ui: &mut egui::Ui,
    value: &mut String,
    name: &str,
    action: &str,
    browse_enabled: Option<bool>,
) -> (bool, bool) {
    ui.horizontal(|ui| {
        let buttons = if browse_enabled.is_some() { 2.0 } else { 1.0 };
        let width =
            (ui.available_width() - buttons * (28.0 + ui.spacing().item_spacing.x)).max(40.0);
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
        let browse = browse_enabled.is_some_and(|enabled| {
            ui.add_enabled_ui(enabled, |ui| {
                icons::button(ui, Icon::Folder, "Browse search folder", "")
            })
            .inner
            .clicked()
        });
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
        .inner_margin(4.0)
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
    use super::{App, PathBuf, egui};
    use std::collections::HashMap;

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
        width: f32,
    ) -> HashMap<String, egui::Rect> {
        let output = crate::app::test_ui::render(ctx, egui::vec2(width, 600.0), events, |ui| {
            app.discovery_settings_ui(ui)
        });
        output
            .platform_output
            .accesskit_update
            .unwrap()
            .nodes
            .iter()
            .filter_map(|(_, node)| {
                let label = node.label()?;
                let bounds = node.bounds()?;
                Some((
                    label.to_owned(),
                    egui::Rect::from_min_max(
                        egui::pos2(bounds.x0 as f32, bounds.y0 as f32),
                        egui::pos2(bounds.x1 as f32, bounds.y1 as f32),
                    ),
                ))
            })
            .collect()
    }

    fn click(app: &mut App, ctx: &egui::Context, rect: egui::Rect) {
        let pos = rect.center();
        for pressed in [true, false] {
            frame(app, ctx, crate::app::test_ui::pointer(pos, pressed), 420.0);
        }
    }

    #[test]
    fn default_folders_are_listed_and_removal_survives_preference_reload() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
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
        click(&mut app, &ctx, nodes["Remove search folder"]);
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
    }

    #[test]
    fn folder_controls_support_keyboard_addition_and_pointer_removal() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
        app.preferences.discovery.roots.clear();
        app.preferences.discovery.removed_roots =
            super::super::search_roots(&app.preferences.discovery)
                .into_iter()
                .map(|r| r.path)
                .collect();
        app.preferences.discovery.excluded.clear();
        app.path = "/work/selected.db".into();
        app.exclusive = true;
        app.writable = true;

        for (input, path) in [
            ("Search folder path", "/work/projects"),
            ("Excluded folder path", "/work/projects/private"),
        ] {
            let nodes = frame(&mut app, &ctx, vec![], 420.0);
            click(&mut app, &ctx, nodes[input]);
            frame(
                &mut app,
                &ctx,
                vec![
                    egui::Event::Text(path.into()),
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: true,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
                    egui::Event::Key {
                        key: egui::Key::Enter,
                        physical_key: None,
                        pressed: false,
                        repeat: false,
                        modifiers: egui::Modifiers::NONE,
                    },
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
        assert!(app.databases.exclude_input.is_empty());
        for action in ["Remove search folder", "Remove exclusion"] {
            let nodes = frame(&mut app, &ctx, vec![], 420.0);
            click(&mut app, &ctx, nodes[action]);
        }
        assert!(app.preferences.discovery.roots.is_empty());
        assert!(app.preferences.discovery.excluded.is_empty());
        assert_eq!(app.path, "/work/selected.db");
        assert!(app.exclusive && app.writable);
        assert!(app.session.is_none());
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
                frame(&mut app, &ctx, vec![], width);
                let nodes = frame(&mut app, &ctx, vec![], width);
                for name in [
                    "Refresh databases",
                    "Search folder path",
                    "Browse search folder",
                    "Add and search folder",
                    "Excluded folder path",
                    "Exclude folder",
                    "Remove search folder",
                    "Remove exclusion",
                ] {
                    let rect = nodes
                        .get(name)
                        .unwrap_or_else(|| panic!("Missing accessible name: {name}"));
                    assert!(
                        rect.left() >= 0.0 && rect.right() <= width,
                        "{name}: {rect:?} at {width}"
                    );
                }
            }
        }
    }
}
