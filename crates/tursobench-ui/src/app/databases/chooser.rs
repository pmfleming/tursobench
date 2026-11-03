use super::{App, Chooser, Entry};
use eframe::egui;
use std::{path::Path, path::PathBuf};

const SEARCH_ID: &str = "database-search";
const ROW_HEIGHT: f32 = 60.0;

impl Chooser {
    fn rows_ui(
        &mut self,
        ui: &mut egui::Ui,
        filtered: &[usize],
        path: &str,
        picker_active: bool,
    ) -> (Option<PathBuf>, Option<PathBuf>) {
        let mut selected = None;
        let mut forget = None;
        #[cfg(test)]
        {
            self.row_rects.clear();
        }
        if filtered.is_empty() {
            ui.add_space(16.0);
            ui.label(if self.entries.is_empty() {
                "No databases found yet"
            } else {
                "No matching databases"
            });
            ui.small("Browse files or enter a database path below.");
            ui.add_space(16.0);
        } else {
            // Leave the access review and primary action visible on laptop screens.
            let reserve = if ui.available_width() < 400.0 || path.is_empty() {
                110.0
            } else {
                410.0
            };
            let minimum = if path.is_empty() { 96.0 } else { ROW_HEIGHT };
            let height = (ui.available_height() - reserve).clamp(minimum, 360.0);
            let stride = ROW_HEIGHT + ui.spacing().item_spacing.y;
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt("database-candidates")
                .max_height(height)
                .auto_shrink([false, true]);
            if std::mem::take(&mut self.reveal_highlight) {
                if let Some(index) = filtered
                    .iter()
                    .position(|&i| Some(&self.entries[i].path) == self.highlighted.as_ref())
                {
                    let top = index as f32 * stride;
                    if top < self.list_offset {
                        self.list_offset = top;
                    }
                    if top + ROW_HEIGHT > self.list_offset + height {
                        self.list_offset = (top + ROW_HEIGHT - height).max(0.0);
                    }
                }
                scroll = scroll.vertical_scroll_offset(self.list_offset);
            }
            let output = scroll.show_rows(ui, ROW_HEIGHT, filtered.len(), |ui, range| {
                for &index in &filtered[range] {
                    let entry = &self.entries[index];
                    let highlighted = self.highlighted.as_ref() == Some(&entry.path);
                    let selected_path = Path::new(path) == entry.path;
                    let available = entry.selectable() && !picker_active;
                    let (response, forget_clicked) =
                        entry.row_ui(ui, selected_path, highlighted && self.navigating, available);
                    #[cfg(test)]
                    self.row_rects.push((entry.path.clone(), response.rect));
                    if response.clicked() && available {
                        selected = Some(entry.path.clone());
                    }
                    if forget_clicked {
                        forget = Some(entry.path.clone());
                    }
                }
            });
            self.list_offset = output.state.offset.y;
        }
        (selected, forget)
    }

    fn filtered(&self) -> Vec<usize> {
        let query = self.filter.to_lowercase();
        self.entries
            .iter()
            .enumerate()
            .filter_map(|(index, entry)| {
                let path = entry.path.to_string_lossy().to_lowercase();
                query
                    .split_whitespace()
                    .all(|word| path.contains(word))
                    .then_some(index)
            })
            .collect()
    }

    fn reconcile_highlight(&mut self, filtered: &[usize]) {
        if !filtered.iter().any(|&i| {
            Some(&self.entries[i].path) == self.highlighted.as_ref() && self.entries[i].selectable()
        }) {
            self.highlighted = filtered
                .iter()
                .map(|&i| &self.entries[i])
                .find(|e| e.selectable())
                .map(|e| e.path.clone());
        }
    }

    fn move_highlight(&mut self, filtered: &[usize], direction: isize) {
        let selectable: Vec<_> = filtered
            .iter()
            .copied()
            .filter(|&i| self.entries[i].selectable())
            .collect();
        if selectable.is_empty() {
            return;
        }
        let current = selectable
            .iter()
            .position(|&i| Some(&self.entries[i].path) == self.highlighted.as_ref())
            .unwrap_or(0);
        if self.navigating && direction < 0 && current == 0 {
            self.navigating = false;
            return;
        }
        let next = if self.navigating {
            current
                .saturating_add_signed(direction)
                .min(selectable.len() - 1)
        } else {
            0
        };
        self.highlighted = Some(self.entries[selectable[next]].path.clone());
        self.navigating = true;
        self.reveal_highlight = true;
    }
}

impl App {
    /// Keep the native search editor/cursor alive while its results have the
    /// browsing marker. Printable text and IME then return to editing directly.
    fn database_chooser_keys(&mut self, ctx: &egui::Context, filtered: &[usize]) -> (bool, bool) {
        let id = egui::Id::new(SEARCH_ID);
        if !ctx.memory(|m| m.has_focus(id)) || self.picker.is_some() {
            return (false, false);
        }
        if ctx.input(|i| {
            i.events.iter().any(|e| {
                matches!(
                    e,
                    egui::Event::Text(_) | egui::Event::Paste(_) | egui::Event::Ime(_)
                )
            })
        }) {
            self.databases.navigating = false;
        }
        let consume = |key| ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::ALT, egui::Key::Enter)) {
            return (true, false);
        }
        let down = consume(egui::Key::ArrowDown);
        let up = consume(egui::Key::ArrowUp);
        if down || up {
            // The first arrow after focus acquisition can already have queued
            // egui's spatial focus movement. This chooser owns these arrows.
            ctx.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
        }
        if down {
            self.databases.move_highlight(filtered, 1);
        }
        if up && self.databases.navigating {
            self.databases.move_highlight(filtered, -1);
        }
        if self.databases.navigating {
            if consume(egui::Key::PageDown) {
                self.databases.move_highlight(filtered, 5);
            }
            if consume(egui::Key::PageUp) {
                self.databases.move_highlight(filtered, -5);
            }
        }
        let activate = consume(egui::Key::Enter);
        if consume(egui::Key::Escape) {
            self.databases.navigating = false;
        }
        (false, activate)
    }

    fn database_search_field(&mut self, ui: &mut egui::Ui) -> bool {
        let (toolbar, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        let browse_rect = egui::Rect::from_min_max(
            egui::pos2(
                toolbar.right() - if toolbar.width() < 400.0 { 32.0 } else { 136.0 },
                toolbar.top(),
            ),
            toolbar.right_bottom(),
        );
        let rect = egui::Rect::from_min_max(
            toolbar.left_top(),
            egui::pos2(browse_rect.left() - 8.0, toolbar.bottom()),
        );
        let id = egui::Id::new(SEARCH_ID);
        if std::mem::take(&mut self.databases.focus_filter) {
            ui.memory_mut(|m| m.request_focus(id));
            self.databases.navigating = false;
        }
        let editing = ui.memory(|m| m.has_focus(id)) && !self.databases.navigating;
        ui.painter()
            .rect_filled(rect, 12.0, ui.visuals().extreme_bg_color);
        if editing {
            ui.painter().rect_stroke(
                rect,
                12.0,
                egui::Stroke::new(1.0, ui.visuals().selection.stroke.color),
                egui::StrokeKind::Inside,
            );
        }
        let icon_rect = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 25.0, rect.center().y),
            egui::vec2(22.0, 22.0),
        );
        search_icon(ui.painter(), icon_rect, ui.visuals().weak_text_color());
        let edit_rect = egui::Rect::from_min_max(
            egui::pos2(rect.left() + 46.0, rect.center().y - 14.0),
            egui::pos2(rect.right() - 12.0, rect.center().y + 14.0),
        );
        let changed = ui
            .scope(|ui| {
                // The result marker owns browsing focus; hide the retained editor's
                // caret until typing or Up from the first result returns to search.
                if self.databases.navigating {
                    ui.visuals_mut().text_cursor.stroke.color = egui::Color32::TRANSPARENT;
                }
                let response = ui.put(
                    edit_rect,
                    egui::TextEdit::singleline(&mut self.databases.filter)
                        .id(id)
                        .frame(egui::Frame::NONE)
                        .font(egui::FontId::proportional(16.0))
                        .margin(egui::Margin::ZERO)
                        .event_filter(egui::EventFilter {
                            vertical_arrows: true,
                            horizontal_arrows: true,
                            escape: true,
                            tab: false,
                        })
                        .hint_text("Search databases…")
                        .char_limit(256),
                );
                if response.clicked() {
                    self.databases.navigating = false;
                }
                response.changed()
            })
            .inner;
        if changed {
            self.databases.highlighted = None;
            self.databases.navigating = false;
            self.databases.list_offset = 0.0;
            self.databases.reveal_highlight = true;
        }
        let response = ui
            .add_enabled_ui(self.picker.is_none(), |ui| {
                ui.put(
                    browse_rect,
                    egui::Button::new(if toolbar.width() < 400.0 {
                        ""
                    } else {
                        "Browse files…"
                    })
                    .corner_radius(10.0),
                )
            })
            .inner;
        #[cfg(test)]
        {
            self.databases.search_rect = Some(rect);
            self.databases.browse_rect = Some(browse_rect);
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                ui.is_enabled(),
                "Browse database files",
            )
        });
        if toolbar.width() < 400.0 {
            let r = browse_rect.shrink(7.0);
            super::super::icons::paint(
                ui.painter(),
                r,
                super::super::icons::Icon::Folder,
                ui.visuals().text_color(),
            );
        }
        response
            .on_hover_text("Browse database files… · Alt+Enter")
            .clicked()
    }

    pub(in crate::app) fn database_chooser_ui(&mut self, ui: &mut egui::Ui) {
        let filtered = self.databases.filtered();
        self.databases.reconcile_highlight(&filtered);
        let (browse_key, activate) = self.database_chooser_keys(ui.ctx(), &filtered);
        let browse_click = self.database_search_field(ui);
        if browse_key || browse_click {
            self.browse_file(ui.ctx());
        }
        let filtered = self.databases.filtered();
        self.databases.reconcile_highlight(&filtered);
        // A text edit/paste and Enter may share a frame. Activate the result of
        // the edited query, never the stale result from before the input event.
        if activate && let Some(path) = self.databases.highlighted.clone() {
            self.select_database(&path);
            self.databases.navigating = true;
        }
        ui.add_space(6.0);

        let (selected, forget) =
            self.databases
                .rows_ui(ui, &filtered, &self.path, self.picker.is_some());
        if let Some(path) = selected {
            self.databases.highlighted = Some(path.clone());
            self.databases.navigating = true;
            self.select_database(&path);
            ui.memory_mut(|m| m.request_focus(egui::Id::new(SEARCH_ID)));
        }
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        self.database_search_status(ui, filtered.len());
    }

    fn database_search_status(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal_wrapped(|ui| {
            if self.databases.job.is_some() {
                ui.spinner();
                ui.small(if self.databases.cancelling {
                    "Stopping search…"
                } else {
                    "Finding local databases…"
                });
            } else {
                ui.small(format!(
                    "{} database{}",
                    count,
                    if count == 1 { "" } else { "s" }
                ));
            }
            if ui.available_width() > 400.0 {
                ui.small("Up/Down choose · Enter select");
            }
            let settings = ui.link("Search settings…");
            #[cfg(test)]
            {
                self.databases.settings_rect = Some(settings.rect);
            }
            if settings.clicked() {
                self.databases.settings_open = true;
            }
        });
        if let Some(message) = &self.databases.message {
            ui.label(message);
        }
        if let Some(summary) = &self.databases.summary {
            if let Some(error) = &summary.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            } else if summary.limited && self.databases.job.is_none() {
                ui.horizontal_wrapped(|ui| {
                    ui.small("Searched part of the configured locations.");
                    if ui
                        .add_enabled(self.picker.is_none(), egui::Button::new("Search a folder…"))
                        .clicked()
                    {
                        self.browse_search_folder(ui.ctx());
                    }
                });
            }
        }
        ui.add_space(8.0);
    }
}

fn search_icon(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let center = rect.center() - egui::vec2(2.0, 2.0);
    painter.circle_stroke(center, 6.0, egui::Stroke::new(1.6, color));
    painter.line_segment(
        [
            center + egui::vec2(4.5, 4.5),
            rect.right_bottom() - egui::vec2(1.0, 1.0),
        ],
        egui::Stroke::new(1.6, color),
    );
}
fn database_icon(
    painter: &egui::Painter,
    rect: egui::Rect,
    color: egui::Color32,
    kind: crate::discovery::DatabaseKind,
) {
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.3, color),
        egui::StrokeKind::Inside,
    );
    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        kind.symbol(),
        egui::FontId::proportional(13.0),
        color,
    );
}

impl Entry {
    fn row_ui(
        &self,
        ui: &mut egui::Ui,
        selected_path: bool,
        highlighted: bool,
        available: bool,
    ) -> (egui::Response, bool) {
        let mut forget = false;
        let (rect, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ROW_HEIGHT),
            egui::Sense::hover(),
        );
        let response = ui.interact(rect, ui.id().with(&self.path), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::WidgetType::SelectableLabel,
                available,
                selected_path,
                format!("{} · {}", self.path.display(), self.kind.label()),
            )
        });
        let fill = if selected_path {
            ui.visuals().selection.bg_fill
        } else if response.hovered() || (highlighted) {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().faint_bg_color
        };
        ui.painter().rect_filled(rect, 10.0, fill);
        if highlighted && ui.memory(|m| m.has_focus(egui::Id::new(SEARCH_ID))) {
            ui.painter().rect_filled(
                egui::Rect::from_min_size(
                    rect.left_top() + egui::vec2(3.0, 13.0),
                    egui::vec2(3.0, 34.0),
                ),
                2.0,
                ui.visuals().selection.stroke.color,
            );
        }
        let color = if available {
            ui.visuals().text_color()
        } else {
            ui.visuals().weak_text_color()
        };
        let icon = egui::Rect::from_center_size(
            egui::pos2(rect.left() + 30.0, rect.center().y),
            egui::vec2(32.0, 32.0),
        );
        ui.painter()
            .rect_filled(icon, 9.0, ui.visuals().widgets.inactive.weak_bg_fill);
        let kind_color = match self.kind {
            crate::discovery::DatabaseKind::LikelyTurso => ui.visuals().hyperlink_color,
            crate::discovery::DatabaseKind::LikelySqlite => ui.visuals().warn_fg_color,
            crate::discovery::DatabaseKind::Unknown => color,
        };
        database_icon(ui.painter(), icon.shrink(6.0), kind_color, self.kind);
        let name = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        let title = egui::WidgetText::from(egui::RichText::new(name).size(15.0));
        let subtitle = egui::WidgetText::from(
            egui::RichText::new(self.path.parent().unwrap_or(&self.path).to_string_lossy())
                .size(12.0),
        );
        let width = (rect.width() - 76.0).max(0.0);
        let badge_color = if self.selectable() {
            ui.visuals().weak_text_color()
        } else {
            ui.visuals().warn_fg_color
        };
        let badge = egui::WidgetText::from(egui::RichText::new(self.badge()).size(12.0))
            .into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                width * 0.45,
                egui::TextStyle::Small,
            );
        let title_width = (width - badge.size().x - 12.0).max(0.0);
        ui.painter().galley(
            egui::pos2(rect.right() - 16.0 - badge.size().x, rect.top() + 13.0),
            badge,
            badge_color,
        );
        ui.painter().galley(
            rect.left_top() + egui::vec2(58.0, 11.0),
            title.into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                title_width,
                egui::TextStyle::Body,
            ),
            color,
        );
        ui.painter().galley(
            rect.left_top() + egui::vec2(58.0, 33.0),
            subtitle.into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                width,
                egui::TextStyle::Small,
            ),
            ui.visuals().weak_text_color(),
        );
        response.context_menu(|ui| {
            ui.label(self.label());
            ui.label(self.kind.label());
            ui.small(self.kind.explanation());
            if let Some(bytes) = self.size {
                ui.small(format!("{} KiB", bytes.div_ceil(1024)));
            }
            if ui.button("Forget database").clicked() {
                forget = true;
                ui.close();
            }
        });
        let response = response.on_hover_text(format!(
            "{}\n{}\n{}: {}{}",
            self.path.display(),
            self.label(),
            self.kind.label(),
            self.kind.explanation(),
            if self.path.to_str().is_none() {
                " · path unsupported by engine"
            } else {
                ""
            }
        ));
        (response, forget)
    }
}
