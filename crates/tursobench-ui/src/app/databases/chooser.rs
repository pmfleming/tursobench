use super::{
    App, Chooser, Entry,
    presentation::{database_icon, folder_label},
};
use crate::app::icons::{self, Icon};
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
            self.list_offset = output.state.clamped_offset().y;
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
        let id = egui::Id::unique(SEARCH_ID);
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
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 44.0), egui::Sense::hover());
        let browse_rect = egui::Rect::from_min_max(
            egui::pos2(rect.right() - 40.0, rect.top() + 4.0),
            rect.right_bottom() - egui::vec2(4.0, 4.0),
        );
        let id = egui::Id::unique(SEARCH_ID);
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
            egui::pos2(browse_rect.left() - 4.0, rect.center().y + 14.0),
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
                        .font(crate::appearance::proportional(ui.ctx(), 16.0))
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
                    egui::Button::new("").frame(false).corner_radius(8.0),
                )
            })
            .inner;
        #[cfg(test)]
        {
            self.databases.search_rect = Some(rect);
            self.databases.browse_rect = Some(browse_rect);
        }
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), "Browse database files")
        });
        super::super::icons::paint(
            ui.painter(),
            browse_rect.shrink(7.0),
            super::super::icons::Icon::Folder,
            if response.enabled() {
                ui.visuals().text_color()
            } else {
                ui.visuals().weak_text_color()
            },
        );
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
            ui.memory_mut(|m| m.request_focus(egui::Id::unique(SEARCH_ID)));
        }
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        self.database_search_status(ui, filtered.len());
    }

    fn database_search_status(&mut self, ui: &mut egui::Ui, count: usize) {
        ui.horizontal(|ui| {
            ui.small(format!(
                "{} database{}",
                count,
                if count == 1 { "" } else { "s" }
            ));
            if self.databases.job.is_some() {
                ui.spinner().on_hover_text(if self.databases.cancelling {
                    "Stopping search…"
                } else {
                    "Finding local databases…"
                });
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings = icons::button(ui, Icon::Settings, "Search settings", "");
                #[cfg(test)]
                {
                    self.databases.settings_rect = Some(settings.rect);
                }
                if settings.clicked() {
                    self.databases.settings_open = true;
                }
            });
        });
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

impl Entry {
    pub(super) fn row_ui(
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
        let response = ui.interact(rect, ui.scope_id().with(&self.path), egui::Sense::click());
        response.widget_info(|| {
            egui::WidgetInfo::selected(
                egui::Role::ListBoxOption,
                available,
                selected_path,
                format!(
                    "{} · {}{}",
                    self.path.display(),
                    self.engine_label(),
                    self.badge()
                        .map(|badge| format!(" · {badge}"))
                        .unwrap_or_default()
                ),
            )
        });
        let fill = if selected_path {
            ui.visuals().selection.bg_fill
        } else if response.hovered() || highlighted || response.has_focus() {
            ui.visuals().widgets.hovered.weak_bg_fill
        } else {
            ui.visuals().faint_bg_color
        };
        ui.painter().rect_filled(rect, 10.0, fill);
        if (highlighted && ui.memory(|m| m.has_focus(egui::Id::unique(SEARCH_ID))))
            || response.has_focus()
        {
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
        database_icon(ui, icon, self, available);
        let name = self
            .path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy();
        let title = egui::WidgetText::from(
            egui::RichText::new(name).size(crate::appearance::text_size(ui.ctx(), 15.0)),
        );
        let subtitle = egui::WidgetText::from(
            egui::RichText::new(folder_label(&self.path))
                .size(crate::appearance::text_size(ui.ctx(), 12.0)),
        );
        let width = (rect.width() - 76.0).max(0.0);
        let title_width = if let Some(badge) = self.badge() {
            let badge = egui::WidgetText::from(
                egui::RichText::new(badge).size(crate::appearance::text_size(ui.ctx(), 12.0)),
            )
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
                ui.visuals().warn_fg_color,
            );
            title_width
        } else {
            width
        };
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
            ui.label(self.engine_label());
            if self.engine_label() != self.kind.label() {
                ui.label(self.kind.label());
            }
            ui.small(self.kind.explanation());
            if let Some(bytes) = self.size {
                ui.small(format!("{} KiB", bytes.div_ceil(1024)));
            }
            if ui.button("Forget database").clicked() {
                forget = true;
                ui.close();
            }
        });
        let response = response.on_hover_text(self.path.to_string_lossy());
        (response, forget)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
            assert!(
                output
                    .platform_output
                    .accesskit_update
                    .unwrap()
                    .nodes
                    .iter()
                    .any(|(_, node)| {
                        node.label() == Some(&format!("{path} · Opened with Turso"))
                    })
            );
        }
    }
}
