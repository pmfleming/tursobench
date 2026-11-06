use super::{
    App, Entry,
    presentation::{database_icon, folder_label},
};
use crate::app::{
    database_navigation::Scope,
    icons::{self, Icon},
};
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn available_databases_ui(&mut self, ui: &mut egui::Ui) {
        let mut paths: Vec<_> = self
            .databases
            .entries
            .iter()
            .filter(|entry| self.duplicate_workspace(&entry.path).is_none())
            .map(|entry| entry.path.clone())
            .collect();
        ui.horizontal(|ui| {
            ui.small(format!("AVAILABLE · {}", paths.len()));
            if self.databases.job.is_some() {
                ui.spinner().on_hover_text("Finding local databases…");
            }
            let settings = icons::button(ui, Icon::Settings, "Search settings", "");
            #[cfg(test)]
            {
                self.databases.settings_rect = Some(settings.rect);
            }
            if settings.clicked() {
                self.databases.settings_open = true;
            }
        });
        if self.database.session.is_none() {
            if self.database.path.is_empty() {
                self.pending_database_ui(ui);
            } else if !paths.iter().any(|p| p == Path::new(&self.database.path)) {
                paths.insert(0, self.database.path.clone().into());
            }
        }
        let can_select = self.can_request_database();
        if !can_select && self.picker.is_none() && !self.database.confirm_close && !self.quit {
            ui.small("4 of 4 workspaces retained. Close a database to open another.");
        }
        let mut selected = None;
        let mut forget = None;
        #[cfg(test)]
        {
            self.databases.row_rects.clear();
        }
        let focused = ui.memory(|m| m.has_focus(egui::Id::unique("sidebar-search")));
        let selectable: Vec<_> = paths
            .iter()
            .filter(|path| {
                self.databases
                    .entries
                    .iter()
                    .any(|e| e.path == **path && e.selectable())
            })
            .cloned()
            .collect();
        if let Some(index) = self.sidebar_selection_keys(ui, selectable.len()) {
            selected = selectable.get(index).cloned();
        }
        for path in paths {
            let entry = self
                .databases
                .entries
                .iter()
                .find(|e| e.path == path)
                .cloned()
                .unwrap_or_else(|| Entry {
                    path: path.clone(),
                    state: super::State::Remembered,
                    size: None,
                    last_opened: None,
                    kind: crate::discovery::DatabaseKind::Unknown,
                });
            let highlighted = focused
                && self.sidebar_search.navigating
                && selectable.get(self.sidebar_search.index) == Some(&path);
            let (row, remove) = entry.row_ui(
                ui,
                Path::new(&self.database.path) == path,
                highlighted,
                can_select && entry.selectable(),
            );
            if highlighted {
                row.scroll_to_me(None);
            }
            #[cfg(test)]
            {
                if ui.is_rect_visible(row.rect) {
                    self.databases.row_rects.push((path.clone(), row.rect));
                }
            }
            if row.clicked() && can_select && entry.selectable() {
                selected = Some(path.clone());
            }
            if remove {
                forget = Some(path.clone());
            }
            if self.database.session.is_none() && Path::new(&self.database.path) == path {
                self.pending_database_ui(ui);
            }
        }
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        if let Some(path) = selected.filter(|_| can_select) {
            self.request_database(ui.ctx(), path);
            self.sidebar_search.query.clear();
        }
        if self.databases.entries.is_empty() {
            ui.weak("No databases found. Browse files or enter a path.");
        }
        if ui.link("Find a database…").clicked() {
            self.sidebar_search.scope = Scope::Databases;
            self.sidebar_search.focus = true;
        }
    }
}

impl Entry {
    pub(super) fn row_ui(
        &self,
        ui: &mut egui::Ui,
        selected: bool,
        highlighted: bool,
        available: bool,
    ) -> (egui::Response, bool) {
        let (rect, response) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 34.0), egui::Sense::click());
        if !ui.is_rect_visible(rect) {
            return (response, false);
        }
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::ListBoxOption,
                available,
                selected,
                format!(
                    "{} · {}{}",
                    self.path.display(),
                    self.engine_label(),
                    self.badge().map(|b| format!(" · {b}")).unwrap_or_default()
                ),
            )
        });
        let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
        if selected || highlighted || response.hovered() || response.has_focus() {
            painter.rect_filled(
                rect,
                6.0,
                if selected || highlighted {
                    ui.visuals().selection.bg_fill
                } else {
                    ui.visuals().faint_bg_color
                },
            );
        }
        database_icon(
            ui,
            egui::Rect::from_center_size(
                rect.left_center() + egui::vec2(14.0, 0.0),
                egui::vec2(22.0, 22.0),
            ),
            self,
            available,
        );
        let name = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        let color = if available {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let badge = self.badge().or_else(|| {
            (self.kind == crate::discovery::DatabaseKind::LikelySqlite).then_some("SQLite")
        });
        let badge_width = if let Some(badge) = badge {
            let galley = painter.layout_no_wrap(
                badge.into(),
                crate::appearance::proportional(ui.ctx(), 9.0),
                ui.visuals().warn_fg_color,
            );
            let width = galley.size().x + 8.0;
            painter.galley(
                rect.right_center() - egui::vec2(width, galley.size().y / 2.0),
                galley,
                ui.visuals().warn_fg_color,
            );
            width
        } else {
            0.0
        };
        for (text, y, size, color, width) in [
            (
                name.into_owned(),
                1.0,
                12.0,
                color,
                (rect.width() - 34.0 - badge_width).max(1.0),
            ),
            (
                folder_label(&self.path),
                18.0,
                10.0,
                ui.visuals().weak_text_color(),
                (rect.width() - 34.0).max(1.0),
            ),
        ] {
            let galley = egui::WidgetText::from(
                egui::RichText::new(text).size(crate::appearance::text_size(ui.ctx(), size)),
            )
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                width,
                egui::TextStyle::Body,
            );
            painter.galley(rect.min + egui::vec2(30.0, y), galley, color);
        }
        let mut forget = false;
        response.context_menu(|ui| {
            ui.label(self.label());
            ui.label(self.engine_label());
            ui.small(self.kind.explanation());
            if let Some(bytes) = self.size {
                ui.small(format!("{} KiB", bytes.div_ceil(1024)));
            }
            if ui.button("Forget database").clicked() {
                forget = true;
                ui.close();
            }
        });
        (response.on_hover_text(self.path.to_string_lossy()), forget)
    }
}

#[cfg(test)]
mod tests {
    use super::{Entry, egui};
    use crate::{app::databases::State, discovery::DatabaseKind};

    #[test]
    fn row_tooltip_is_only_the_full_path_and_engine_status_stays_accessible() {
        let path = "/home/demo/.local/share/clip-daemon/history/history.db";
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            crate::appearance::theme(&ctx, dark);
            let entry = Entry {
                path: path.into(),
                state: State::Found,
                size: Some(4096),
                last_opened: Some(1),
                kind: DatabaseKind::LikelyTurso,
            };
            let mut output = egui::FullOutput::default();
            for time in [0.0, 1.0, 2.0, 3.0] {
                output = ctx.run_ui(
                    egui::RawInput {
                        time: Some(time),
                        screen_rect: Some(egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(600.0, 400.0),
                        )),
                        events: if time == 0.0 {
                            vec![egui::Event::PointerMoved(egui::pos2(100.0, 30.0))]
                        } else {
                            vec![]
                        },
                        ..Default::default()
                    },
                    |ui| {
                        entry.row_ui(ui, false, false, true);
                    },
                );
                output.textures_delta.clear();
            }
            let texts: Vec<_> = output
                .shapes
                .iter()
                .filter_map(|shape| match &shape.shape {
                    egui::Shape::Text(text) => Some(text.galley.job.text.as_str()),
                    _ => None,
                })
                .collect();
            assert_eq!(texts, vec!["history.db", "clip-daemon/history", path]);
            assert!(crate::app::test_ui::nodes(&output).iter().any(|(_, node)| {
                node.label() == Some(&format!("{path} · Opened with Turso"))
            }));
        }
    }
}
