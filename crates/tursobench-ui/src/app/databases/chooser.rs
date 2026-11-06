use super::{
    App, Entry,
    presentation::{database_icon, folder_label},
};
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn available_databases_ui(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            ui.small("OPEN DATABASE");
            let browse = ui
                .add_enabled_ui(self.can_request_database(), |ui| {
                    icons::button(ui, Icon::Folder, "Browse database files", "Ctrl O")
                })
                .inner;
            #[cfg(test)]
            {
                self.databases.browse_rect = Some(browse.rect);
            }
            if browse.clicked() {
                self.begin_database_browse(ui.ctx());
            }
        });
        let mut indices: Vec<_> = self
            .databases
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.listed_workspace(&entry.path).is_none())
            .map(|(index, _)| index)
            .collect();
        ui.horizontal(|ui| {
            if icons::button(
                ui,
                if self.sidebar_search.available_collapsed {
                    Icon::Next
                } else {
                    Icon::Down
                },
                if self.sidebar_search.available_collapsed {
                    "Expand available databases"
                } else {
                    "Collapse available databases"
                },
                "",
            )
            .clicked()
            {
                self.sidebar_search.available_collapsed = !self.sidebar_search.available_collapsed;
            }
            ui.small(format!("AVAILABLE · {}", indices.len()));
            if self.databases.job.is_some() {
                ui.spinner().on_hover_text("Finding local databases…");
            }
            let refresh = icons::button(ui, Icon::Refresh, "Refresh databases", "");
            #[cfg(test)]
            {
                self.databases.refresh_rect = Some(refresh.rect);
            }
            if refresh.clicked() {
                self.start_discovery(ui.ctx(), None);
            }
        });
        let can_select = self.can_request_database();
        let mut selected = None;
        let mut forget = None;
        #[cfg(test)]
        {
            self.databases.row_rects.clear();
            self.databases.rendered_candidates = 0;
        }
        // The variable-height access card is kept above the fixed-height list so
        // it cannot invalidate virtual row positions or disappear off-screen.
        if !self.database.retained() {
            if !self.database.path.is_empty() {
                let pending = indices.iter().position(|&i| {
                    self.databases.entries[i].path == Path::new(&self.database.path)
                });
                let synthetic;
                let entry = if let Some(pending) = pending {
                    &self.databases.entries[indices.remove(pending)]
                } else {
                    synthetic = Entry {
                        path: self.database.path.clone().into(),
                        state: super::State::Remembered,
                        size: None,
                        last_opened: None,
                        kind: crate::discovery::DatabaseKind::Unknown,
                    };
                    &synthetic
                };
                let (_row, remove) =
                    entry.row_ui(ui, true, false, can_select && entry.selectable());
                #[cfg(test)]
                if ui.is_rect_visible(_row.rect) {
                    self.databases
                        .row_rects
                        .push((entry.path.clone(), _row.rect));
                }
                if remove {
                    forget = Some(entry.path.clone());
                }
            }
            self.pending_database_ui(ui);
        }
        if self.sidebar_search.available_collapsed {
            return;
        }
        if !can_select && self.picker.is_none() && !self.database.confirm_close && !self.quit {
            ui.small(self.database_limit_message());
        }
        let selectable: Vec<_> = indices
            .iter()
            .copied()
            .filter(|&i| self.databases.entries[i].selectable())
            .collect();
        if self.sidebar_search.navigating
            && self.sidebar_search.key_delta.is_none()
            && let Some(path) = &self.sidebar_search.available_highlight
            && let Some(index) = selectable
                .iter()
                .position(|&i| self.databases.entries[i].path == *path)
        {
            self.sidebar_search.index = index;
        }
        if let Some(index) = self.sidebar_selection_keys(ui, selectable.len()) {
            selected = selectable
                .get(index)
                .map(|&i| self.databases.entries[i].path.clone());
        }
        let focused = ui.memory(|m| m.has_focus(egui::Id::unique("sidebar-search")));
        let highlighted = (focused && self.sidebar_search.navigating)
            .then(|| selectable.get(self.sidebar_search.index))
            .flatten()
            .copied();
        self.sidebar_search.available_highlight =
            highlighted.map(|i| self.databases.entries[i].path.clone());
        let row_height = 34.0;
        let stride = row_height + ui.spacing().item_spacing.y;
        let top = ui.cursor().min;
        if let Some(position) = (std::mem::take(&mut self.sidebar_search.reveal))
            .then_some(highlighted)
            .flatten()
            .and_then(|i| indices.iter().position(|&index| index == i))
        {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(
                    top + egui::vec2(0.0, position as f32 * stride),
                    egui::vec2(ui.available_width(), row_height),
                ),
                None,
            );
        }
        let start = (((ui.clip_rect().top() - top.y) / stride).floor().max(0.0) as usize)
            .min(indices.len());
        let end = ((((ui.clip_rect().bottom() - top.y) / stride).ceil().max(0.0) as usize) + 1)
            .min(indices.len())
            .max(start);
        ui.add_space(start as f32 * stride);
        ui.skip_ahead_auto_ids(start);
        for &index in &indices[start..end] {
            #[cfg(test)]
            {
                self.databases.rendered_candidates += 1;
            }
            let entry = &self.databases.entries[index];
            let (row, remove) = ui
                .push_id(&entry.path, |ui| {
                    entry.row_ui(
                        ui,
                        false,
                        highlighted == Some(index),
                        can_select && entry.selectable(),
                    )
                })
                .inner;
            #[cfg(test)]
            if ui.is_rect_visible(row.rect) {
                self.databases
                    .row_rects
                    .push((entry.path.clone(), row.rect));
            }
            if row.clicked() && can_select && entry.selectable() {
                selected = Some(entry.path.clone());
            }
            if remove {
                forget = Some(entry.path.clone());
            }
        }
        ui.add_space((indices.len() - end) as f32 * stride);
        ui.skip_ahead_auto_ids(indices.len() - end);
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        if let Some(path) = selected.filter(|_| can_select) {
            self.request_database(ui.ctx(), path);
        }
        if self.databases.entries.is_empty() {
            ui.weak("No databases found. Browse files or enter a path.");
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
