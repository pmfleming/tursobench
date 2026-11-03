use super::{
    App, DatabaseKind, Entry, State,
    presentation::{database_icon, folder_label},
};
use crate::app::icons::{self, Icon};
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn database_switcher_ui(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_enabled(
            self.ready && !self.closing,
            egui::Button::new("")
                .corner_radius(10)
                .min_size(egui::vec2(ui.available_width(), 52.0)),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                response.enabled(),
                format!("Switch database: {}", self.path),
            )
        });
        let current = Entry {
            path: self.path.clone().into(),
            state: State::Found,
            size: None,
            last_opened: Some(0),
            kind: DatabaseKind::Unknown,
        };
        let rect = response.rect;
        database_icon(
            ui,
            egui::Rect::from_center_size(
                egui::pos2(rect.left() + 24.0, rect.center().y),
                egui::vec2(32.0, 32.0),
            ),
            &current,
            response.enabled(),
        );
        let path = Path::new(&self.path);
        let filename = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy();
        let width = (rect.width() - 82.0).max(0.0);
        for (text, y, color) in [
            (
                egui::RichText::new(filename).strong(),
                8.0,
                ui.visuals().text_color(),
            ),
            (
                egui::RichText::new(folder_label(path)).small(),
                29.0,
                ui.visuals().weak_text_color(),
            ),
        ] {
            let galley = egui::WidgetText::from(text).into_galley(
                ui,
                Some(egui::TextWrapMode::Truncate),
                width,
                egui::TextStyle::Body,
            );
            ui.painter()
                .galley(rect.min + egui::vec2(50.0, y), galley, color);
        }
        icons::paint(
            ui.painter(),
            egui::Rect::from_center_size(
                egui::pos2(rect.right() - 17.0, rect.center().y),
                egui::vec2(16.0, 16.0),
            ),
            Icon::Down,
            ui.visuals().weak_text_color(),
        );
        let response = response.on_hover_text(&self.path);
        if response.clicked() {
            self.databases.switcher_filter.clear();
        }
        let width = 340.0_f32.min(ui.ctx().content_rect().width() - 32.0);
        egui::Popup::menu(&response)
            .width(width)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_width(width);
                self.recent_databases_ui(ui, response.clicked());
            });
    }

    fn recent_databases_ui(&mut self, ui: &mut egui::Ui, focus: bool) {
        ui.horizontal(|ui| {
            ui.strong("Recent");
            ui.weak(self.preferences.discovery.recent.len().to_string());
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if icons::button(ui, Icon::Disconnect, "Close database", "").clicked() {
                    self.pending_database = None;
                    self.request_close();
                    ui.close();
                }
                if icons::button(ui, Icon::Folder, "Choose database", "Ctrl O").clicked() {
                    self.pending_database = None;
                    self.request_close();
                    ui.close();
                }
            });
        });
        let search = ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut self.databases.switcher_filter)
                .id_salt("recent-database-search")
                .hint_text("Find database…"),
        );
        if focus {
            search.request_focus();
        }
        let query = self.databases.switcher_filter.to_lowercase();
        let recent: Vec<_> = self
            .preferences
            .discovery
            .recent
            .iter()
            .filter(|entry| entry.path.to_string_lossy().to_lowercase().contains(&query))
            .cloned()
            .collect();
        let mut select = None;
        let mut forget = None;
        if search.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && recent.len() == 1
        {
            select = Some(recent[0].path.clone());
        }
        let height = (ui.ctx().content_rect().height() - 180.0).clamp(120.0, 360.0);
        egui::ScrollArea::vertical()
            .id_salt("recent-databases")
            .max_height(height)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                if recent.is_empty() {
                    ui.weak("No matching databases");
                }
                for recent in recent {
                    let known = self
                        .databases
                        .entries
                        .iter()
                        .find(|entry| entry.path == recent.path);
                    let entry = Entry {
                        path: recent.path,
                        state: known.map_or(State::Remembered, |entry| entry.state),
                        size: known.and_then(|entry| entry.size),
                        last_opened: Some(recent.last_opened),
                        kind: known.map_or(DatabaseKind::Unknown, |entry| entry.kind),
                    };
                    let (row, remove) =
                        entry.row_ui(ui, entry.path == Path::new(&self.path), false, true);
                    if row.clicked() {
                        select = Some(entry.path.clone());
                    }
                    if remove {
                        forget = Some(entry.path);
                    }
                }
            });
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        if let Some(path) = select {
            if path != Path::new(&self.path) {
                // Keep the existing close/rollback acknowledgement and fresh access review.
                self.pending_database = Some(path);
                self.request_close();
            }
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, egui};

    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        crate::app::test_ui::render(ctx, egui::vec2(720.0, 480.0), events, |ui| {
            ui.set_width(220.0);
            app.database_switcher_ui(ui);
        })
    }

    #[test]
    fn recent_popup_is_bounded_and_filters_full_paths_with_keyboard_selection() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut app = App::new(&ctx);
            crate::appearance::theme(&ctx, dark);
            app.ready = true;
            app.writable = true;
            app.path = "/work/current/session.db".into();
            app.preferences
                .discovery
                .record_open(app.path.clone().into(), 1);
            let long_path = format!("/work/{}snapshot/session.db", "long-parent/".repeat(20));
            app.preferences
                .discovery
                .record_open(long_path.clone().into(), 2);
            for index in 0..20 {
                app.preferences
                    .discovery
                    .record_open(format!("/work/project-{index}/app.db").into(), 3);
            }
            frame(&mut app, &ctx, vec![]);
            let pos = egui::pos2(100.0, 25.0);
            for pressed in [true, false] {
                frame(&mut app, &ctx, crate::app::test_ui::pointer(pos, pressed));
            }
            let output = frame(&mut app, &ctx, vec![]);
            let nodes = &output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes;
            for name in ["Choose database", "Close database"] {
                assert!(nodes.iter().any(|(_, n)| n.label() == Some(name)));
            }
            for (_, node) in nodes {
                if node.role() == egui::Role::ListBoxOption
                    && let Some(rect) = node.bounds()
                {
                    assert!(rect.x1 - rect.x0 <= 340.0, "{rect:?}");
                }
            }
            frame(&mut app, &ctx, vec![egui::Event::Text("snapshot".into())]);
            let output = frame(&mut app, &ctx, vec![]);
            assert_eq!(app.databases.switcher_filter, "snapshot");
            let rows: Vec<_> = output
                .platform_output
                .accesskit_update
                .as_ref()
                .unwrap()
                .nodes
                .iter()
                .filter(|(_, node)| node.role() == egui::Role::ListBoxOption)
                .collect();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].1.label().unwrap().starts_with(&long_path));
            frame(
                &mut app,
                &ctx,
                vec![egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                }],
            );
            assert_eq!(app.pending_database, Some(long_path.into()));
            assert!(app.confirm_close);
            assert_eq!(app.path, "/work/current/session.db");
            assert!(!app.closing);
        }
    }
}
