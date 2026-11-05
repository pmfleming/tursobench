use super::{
    App, DatabaseKind, Entry, State,
    presentation::{database_icon, folder_label},
};
use crate::app::icons::{self, Icon};
use crate::app::presentation::{self, Intent};
use eframe::egui;
use std::path::Path;

impl App {
    pub(in crate::app) fn database_switcher_ui(&mut self, ui: &mut egui::Ui) {
        let response = ui.add_enabled(
            self.database.ready && !self.database.closing,
            egui::Button::new("")
                .corner_radius(10)
                .min_size(egui::vec2(ui.available_width(), 52.0)),
        );
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                response.enabled(),
                format!("Switch database: {}", self.database.path),
            )
        });
        let current = Entry {
            path: self.database.path.clone().into(),
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
        let path = Path::new(&self.database.path);
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
        let response = response.on_hover_text(&self.database.path);
        if response.clicked() {
            self.databases.switcher_filter.clear();
        }
        let width = 340.0_f32.min(ui.ctx().content_rect().width() - 32.0);
        let screen = ui.ctx().content_rect();
        let below = (screen.bottom() - response.rect.bottom() - 12.0).max(0.0);
        let above = (response.rect.top() - screen.top() - 12.0).max(0.0);
        let (align, height) = if below >= above {
            (egui::RectAlign::BOTTOM_START, below)
        } else {
            (egui::RectAlign::TOP_START, above)
        };
        egui::Popup::menu(&response)
            .width(width)
            .align(align)
            .align_alternatives(&[])
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.set_width(width);
                // Override the area's small initial height before sizing its
                // scroll view, otherwise short lists remain unnecessarily clipped.
                ui.set_max_height((height - 12.0).max(0.0));
                self.recent_databases_ui(ui, response.clicked());
            });
    }

    fn recent_databases_ui(&mut self, ui: &mut egui::Ui, focus: bool) {
        ui.horizontal(|ui| {
            let width = (ui.available_width() - ui.spacing().item_spacing.x) / 2.0;
            for (icon, label, intent) in [
                (Icon::Folder, "Open database…", Intent::Primary),
                (Icon::Disconnect, "Close database", Intent::Neutral),
            ] {
                let primary = matches!(intent, Intent::Primary);
                let tokens = crate::appearance::Tokens::of(ui.ctx());
                let color = if primary {
                    tokens.primary_ink
                } else {
                    ui.visuals().text_color()
                };
                let button = if primary {
                    presentation::action(ui, &format!("    {label}"), intent)
                } else {
                    egui::Button::new(egui::RichText::new(format!("    {label}")).color(color))
                        .fill(tokens.raised)
                        .stroke(egui::Stroke::new(1.0, tokens.line_strong))
                };
                let response = ui.add_sized([width, 38.0], button);
                response.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::Button, response.enabled(), label)
                });
                icons::paint(
                    ui.painter(),
                    egui::Rect::from_center_size(
                        egui::pos2(response.rect.left() + 17.0, response.rect.center().y),
                        egui::vec2(18.0, 18.0),
                    ),
                    icon,
                    color,
                );
                if response.clicked() {
                    self.database.pending_database = None;
                    if primary {
                        self.new_workspace(ui.ctx());
                    } else {
                        self.request_close();
                    }
                    ui.close();
                }
            }
        });
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.strong("Recent");
            ui.weak(self.preferences.discovery.recent.len().to_string());
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
        let height = ui.available_height().max(0.0);
        egui::ScrollArea::vertical()
            .id_salt("recent-databases")
            .max_height(height)
            .min_scrolled_height(0.0)
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
                    let (row, remove) = entry.row_ui(
                        ui,
                        entry.path == Path::new(&self.database.path),
                        false,
                        true,
                    );
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
            if path != Path::new(&self.database.path) {
                self.request_database(ui.ctx(), path);
            }
            ui.close();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{App, egui};
    use crate::app::test_ui::key_event;

    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>) -> egui::FullOutput {
        crate::app::test_ui::render(ctx, egui::vec2(720.0, 480.0), events, |ui| {
            ui.set_width(220.0);
            app.database_switcher_ui(ui);
        })
    }

    #[test]
    fn recent_popup_shows_short_lists_fully_and_close_uses_normal_contrast() {
        for dark in [false, true] {
            for (size, top) in [
                (egui::vec2(720.0, 480.0), 0.0),
                (egui::vec2(720.0, 480.0), 380.0),
                (egui::vec2(1280.0, 720.0), 0.0),
            ] {
                let ctx = egui::Context::default();
                ctx.enable_accesskit();
                crate::appearance::theme(&ctx, dark);
                let mut app = App::new(&ctx);
                app.database.ready = true;
                app.database.writable = true;
                app.database.path = "/work/current.db".into();
                for path in ["/work/current.db", "/work/other.db"] {
                    app.preferences.discovery.record_open(path.into(), 1);
                }
                let render = |app: &mut App, events| {
                    crate::app::test_ui::render(&ctx, size, events, |ui| {
                        ui.set_width(220.0);
                        ui.add_space(top);
                        app.database_switcher_ui(ui);
                    })
                };
                render(&mut app, vec![]);
                for pressed in [true, false] {
                    render(
                        &mut app,
                        crate::app::test_ui::pointer(egui::pos2(100.0, top + 25.0), pressed),
                    );
                }
                render(&mut app, vec![]);
                let output = render(&mut app, vec![]);
                let nodes = crate::app::test_ui::nodes(&output);
                let rows: Vec<_> = nodes
                    .iter()
                    .filter(|(_, n)| n.role() == egui::Role::ListBoxOption)
                    .map(|(_, n)| {
                        let b = n.bounds().unwrap();
                        egui::Rect::from_min_max(
                            egui::pos2(b.x0 as f32, b.y0 as f32),
                            egui::pos2(b.x1 as f32, b.y1 as f32),
                        )
                    })
                    .collect();
                assert_eq!(rows.len(), 2);
                for row in rows {
                    assert!(ctx.content_rect().contains_rect(row));
                    let shape=output.shapes.iter().find(|shape|matches!(&shape.shape,egui::Shape::Rect(rect) if rect.rect==row)).unwrap();
                    assert!(
                        shape.clip_rect.contains_rect(row),
                        "clipped recent row: {row:?}, clip {:?}",
                        shape.clip_rect
                    );
                }
                let close_text = output
                    .shapes
                    .iter()
                    .find_map(|shape| match &shape.shape {
                        egui::Shape::Text(text)
                            if text.galley.job.text.trim() == "Close database" =>
                        {
                            Some(text)
                        }
                        _ => None,
                    })
                    .unwrap();
                assert_eq!(
                    close_text.galley.job.sections[0].format.color,
                    ctx.global_style().visuals.text_color()
                );
                let close = crate::app::test_ui::control(&output, "Close database");
                for pressed in [true, false] {
                    render(
                        &mut app,
                        crate::app::test_ui::pointer(close.center(), pressed),
                    );
                }
                assert!(app.database.confirm_close);
            }
        }
    }

    #[test]
    fn recent_popup_is_bounded_and_filters_full_paths_with_keyboard_selection() {
        for dark in [false, true] {
            let (ctx, mut app) = crate::app::test_ui::app();
            crate::appearance::theme(&ctx, dark);
            app.database.ready = true;
            app.database.writable = true;
            app.database.path = "/work/current/session.db".into();
            app.preferences
                .discovery
                .record_open(app.database.path.clone().into(), 1);
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
            let nodes = crate::app::test_ui::nodes(&output);
            for name in ["Open database…", "Close database"] {
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
            let rows: Vec<_> = crate::app::test_ui::nodes(&output)
                .iter()
                .filter(|(_, node)| node.role() == egui::Role::ListBoxOption)
                .collect();
            assert_eq!(rows.len(), 1);
            assert!(rows[0].1.label().unwrap().starts_with(&long_path));
            frame(
                &mut app,
                &ctx,
                vec![key_event(egui::Key::Enter, true, egui::Modifiers::NONE)],
            );
            assert!(app.database.pending_database.is_none());
            assert!(!app.database.confirm_close);
            assert_eq!(app.database.path, long_path);
            assert!(!app.database.closing);
        }
    }
}
