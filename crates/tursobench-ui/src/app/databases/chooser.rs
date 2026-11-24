use super::{App, Entry, presentation::ROW_HEIGHT};
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::{Path, PathBuf};

/// Vertical layout of the virtual list: fixed rows, with the access review's
/// measured height inserted after the row it belongs to.
#[derive(Clone, Copy)]
struct RowLayout {
    stride: f32,
    review: Option<(usize, f32)>,
}

impl RowLayout {
    /// Height of the list above a row; rows after the review sit lower by its height.
    fn offset(self, position: usize) -> f32 {
        position as f32 * self.stride
            + self
                .review
                .filter(|&(after, _)| position > after)
                .map_or(0.0, |(_, height)| height)
    }

    /// Index of the row at or below a height in the list; a height inside the
    /// review maps to the row that owns it.
    fn row_at(self, y: f32) -> usize {
        let y = y.max(0.0);
        match self.review {
            Some((after, height)) if y >= (after + 1) as f32 * self.stride => {
                (((y - height) / self.stride).floor() as usize).max(after)
            }
            _ => (y / self.stride).floor() as usize,
        }
    }
}

/// What the rows asked for this frame, applied once the list is drawn.
#[derive(Default)]
struct RowChoice {
    selected: Option<PathBuf>,
    forget: Option<PathBuf>,
}

impl App {
    pub(in crate::app) fn available_databases_ui(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        self.open_database_header(ui);
        let mut indices: Vec<_> = self
            .databases
            .entries
            .iter()
            .enumerate()
            .filter(|(_, entry)| self.listed_workspace(&entry.path).is_none())
            .map(|(index, _)| index)
            .collect();
        self.available_header(ui, indices.len());
        let can_select = self.can_request_database();
        #[cfg(test)]
        {
            self.databases.row_rects.clear();
            self.databases.rendered_candidates = 0;
        }
        // The access review opens directly under the row that was clicked, so
        // the list does not reorder. Its height is variable, so the virtual
        // list below reserves last frame's measurement after that row. A path
        // that is typed, browsed to or hidden by a collapsed list has no row
        // to sit under and is reviewed above the list instead.
        let reviewing = !self.database.retained() && !self.database.path.is_empty();
        let pending = reviewing
            .then(|| {
                indices
                    .iter()
                    .position(|&i| self.databases.entries[i].path == Path::new(&self.database.path))
            })
            .flatten();
        let in_place = pending
            .filter(|_| !self.sidebar_search.available_collapsed && !self.databases.editing_path);
        if !reviewing {
            self.databases.review_revealed = None;
        }
        let mut choice = RowChoice::default();
        let above = reviewing && in_place.is_none();
        if above {
            let entry = pending.map(|pending| indices.remove(pending));
            choice.forget = self.reviewed_row_above_list(ui, entry, can_select);
        }
        self.pending_database_ui(ui, in_place.is_some());
        if above {
            self.database_access_ui(ui);
        }
        let listed = !self.sidebar_search.available_collapsed;
        if listed {
            if !can_select && self.picker.is_none() && !self.database.confirm_close && !self.quit {
                ui.small(self.database_limit_message());
            }
            self.available_rows_ui(ui, &indices, in_place, can_select, &mut choice);
        }
        if let Some(path) = choice.forget {
            self.forget_database(&path);
        }
        if let Some(path) = choice.selected.filter(|_| can_select) {
            self.request_database(ui.ctx(), path);
        }
        if listed && self.databases.entries.is_empty() {
            ui.weak("No databases found. Browse files or enter a path.");
        }
    }

    fn open_database_header(&mut self, ui: &mut egui::Ui) {
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
    }

    fn available_header(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal(|ui| {
            let collapsed = self.sidebar_search.available_collapsed;
            let (icon, name) = if collapsed {
                (Icon::Next, "Expand available databases")
            } else {
                (Icon::Down, "Collapse available databases")
            };
            if icons::button(ui, icon, name, "").clicked() {
                self.sidebar_search.available_collapsed = !collapsed;
            }
            ui.small(format!("AVAILABLE · {count}"));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let _action = self.discovery_actions(ui);
                #[cfg(test)]
                {
                    self.databases.refresh_rect = Some(_action.rect);
                }
            });
        });
    }

    /// The reviewed database's row when it cannot sit in the list: a listed
    /// entry, or a typed or browsed path. Returns a path to forget.
    fn reviewed_row_above_list(
        &mut self,
        ui: &mut egui::Ui,
        index: Option<usize>,
        can_select: bool,
    ) -> Option<PathBuf> {
        let typed;
        let entry = if let Some(index) = index {
            &self.databases.entries[index]
        } else {
            typed = Entry::remembered(self.database.path.clone().into(), None);
            &typed
        };
        let (_row, remove) = entry.row_ui(ui, true, false, can_select && entry.selectable());
        let path = entry.path.clone();
        #[cfg(test)]
        self.record_row(ui, &path, _row.rect);
        remove.then_some(path)
    }

    #[cfg(test)]
    fn record_row(&mut self, ui: &egui::Ui, path: &Path, rect: egui::Rect) {
        if ui.is_rect_visible(rect) {
            self.databases.row_rects.push((path.to_owned(), rect));
        }
    }

    /// Keyboard navigation from the sidebar search: the highlighted entry, and
    /// one chosen with Enter.
    fn highlight_available(
        &mut self,
        ui: &mut egui::Ui,
        indices: &[usize],
        choice: &mut RowChoice,
    ) -> Option<usize> {
        let entries = &self.databases.entries;
        let selectable: Vec<_> = indices
            .iter()
            .copied()
            .filter(|&i| entries[i].selectable())
            .collect();
        if self.sidebar_search.navigating
            && self.sidebar_search.key_delta.is_none()
            && let Some(path) = &self.sidebar_search.available_highlight
            && let Some(index) = selectable.iter().position(|&i| entries[i].path == *path)
        {
            self.sidebar_search.index = index;
        }
        if let Some(index) = self.sidebar_selection_keys(ui, selectable.len()) {
            choice.selected = selectable
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
        highlighted
    }

    /// The available databases as a virtual list: only rows near the visible
    /// area are laid out, the rest are reserved as space.
    fn available_rows_ui(
        &mut self,
        ui: &mut egui::Ui,
        indices: &[usize],
        in_place: Option<usize>,
        can_select: bool,
        choice: &mut RowChoice,
    ) {
        let highlighted = self.highlight_available(ui, indices, choice);
        let layout = RowLayout {
            stride: ROW_HEIGHT + ui.spacing().item_spacing.y,
            review: in_place.map(|position| (position, self.databases.review_height)),
        };
        let top = ui.cursor().min;
        if let Some(position) = (std::mem::take(&mut self.sidebar_search.reveal))
            .then_some(highlighted)
            .flatten()
            .and_then(|i| indices.iter().position(|&index| index == i))
        {
            ui.scroll_to_rect(
                egui::Rect::from_min_size(
                    top + egui::vec2(0.0, layout.offset(position)),
                    egui::vec2(ui.available_width(), ROW_HEIGHT),
                ),
                None,
            );
        }
        let start = layout
            .row_at(ui.clip_rect().top() - top.y)
            .min(indices.len());
        let end = (layout.row_at(ui.clip_rect().bottom() - top.y) + 2)
            .min(indices.len())
            .max(start);
        ui.add_space(layout.offset(start));
        ui.skip_ahead_auto_ids(start);
        for (position, &index) in indices.iter().enumerate().take(end).skip(start) {
            #[cfg(test)]
            {
                self.databases.rendered_candidates += 1;
            }
            let reviewed = in_place == Some(position);
            let entry = &self.databases.entries[index];
            let available = can_select && entry.selectable();
            let (row, remove) = ui
                .push_id(&entry.path, |ui| {
                    entry.row_ui(ui, reviewed, highlighted == Some(index), available)
                })
                .inner;
            let path = entry.path.clone();
            #[cfg(test)]
            self.record_row(ui, &path, row.rect);
            if remove {
                choice.forget = Some(path.clone());
            }
            if reviewed {
                self.review_in_place_ui(ui, row.rect, path);
            } else if row.clicked() && available {
                choice.selected = Some(path);
            }
        }
        ui.add_space(layout.offset(indices.len()) - layout.offset(end));
        ui.skip_ahead_auto_ids(indices.len() - end);
    }

    /// The access review under its row; it remembers its height for the
    /// virtual list and scrolls into view once per selection.
    fn review_in_place_ui(&mut self, ui: &mut egui::Ui, row: egui::Rect, path: PathBuf) {
        let before = ui.cursor().min.y;
        ui.push_id("database-review", |ui| self.database_access_ui(ui));
        let height = ui.cursor().min.y - before;
        if (height - self.databases.review_height).abs() > 0.5 {
            self.databases.review_height = height;
            ui.ctx().request_repaint();
        }
        if self.databases.review_revealed.as_ref() != Some(&path) {
            self.databases.review_revealed = Some(path);
            ui.scroll_to_rect(
                egui::Rect::from_min_max(row.min, egui::pos2(row.right(), before + height)),
                None,
            );
        }
    }
}
