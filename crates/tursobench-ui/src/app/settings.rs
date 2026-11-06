//! Application preferences live outside any database workspace.
use super::{
    App,
    icons::{self, Icon},
    panels::{card, section},
};
use crate::appearance::Mode;
use eframe::egui;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(super) enum Page {
    #[default]
    Appearance,
    Discovery,
    Resources,
    Queries,
}

#[derive(Default)]
pub(super) struct State {
    pub open: bool,
    pub page: Page,
    dirty: bool,
    displayed_font: Option<u16>,
}

impl App {
    pub(super) fn settings_window(&mut self, ctx: &egui::Context) {
        if !self.settings.open {
            self.save_settings();
            return;
        }
        let mut open = true;
        let scale = f32::from(self.preferences.font_size) / 14.0;
        let size = (egui::vec2(640.0, 540.0) * scale)
            .min(ctx.content_rect().size() - egui::vec2(32.0, 56.0));
        let previous_font = self
            .settings
            .displayed_font
            .replace(self.preferences.font_size);
        let mut window = egui::Window::new("Settings")
            .id(egui::Id::unique("application-settings"))
            .frame(super::presentation::window_frame(ctx))
            .default_pos(ctx.content_rect().center() - size * 0.5)
            .default_size(size)
            .min_size(egui::vec2(280.0, 240.0))
            .max_size(ctx.content_rect().size() - egui::vec2(24.0, 48.0))
            .collapsible(false)
            .open(&mut open);
        if let Some(previous) =
            previous_font.filter(|previous| *previous != self.preferences.font_size)
            && let Some(rect) =
                ctx.memory(|m| m.area_rect(egui::Id::unique("application-settings")))
        {
            let scaled = (rect.size()
                * (f32::from(self.preferences.font_size) / f32::from(previous)))
            .min(ctx.content_rect().size() - egui::vec2(32.0, 56.0));
            window = window.fixed_size(scaled);
        }
        window.show(ctx, |ui| self.settings_ui(ui));
        self.settings.open = open;
        if !open {
            self.save_settings();
        }
    }

    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        let previous_page = self.settings.page;
        let scale = f32::from(self.preferences.font_size) / 14.0;
        if ui.available_width() < 480.0 {
            ui.horizontal(|ui| {
                for (page, icon, label, name) in pages() {
                    if icons::control(ui, icon, name, "", self.settings.page == page, false)
                        .on_hover_text(label)
                        .clicked()
                    {
                        self.settings.page = page;
                    }
                }
            });
            ui.separator();
            self.settings_page(ui);
        } else {
            ui.horizontal_top(|ui| {
                let height = ui.available_height();
                ui.allocate_ui_with_layout(
                    egui::vec2(112.0 * scale, height),
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        for (page, icon, label, name) in pages() {
                            if icons::rail_button(
                                ui,
                                icon,
                                label,
                                name,
                                self.settings.page == page,
                                ui.available_width(),
                            )
                            .clicked()
                            {
                                self.settings.page = page;
                            }
                        }
                    },
                );
                ui.separator();
                ui.allocate_ui_with_layout(
                    ui.available_size(),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.settings_page(ui),
                );
            });
        }
        if previous_page != self.settings.page {
            self.save_settings();
        }
    }

    fn settings_page(&mut self, ui: &mut egui::Ui) {
        if self.settings.page == Page::Discovery {
            // Discovery manages its own folder-list scroll area.
            self.discovery_settings_ui(ui);
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt(("settings-page", self.settings.page as u8))
            .auto_shrink([false, false])
            .show(ui, |ui| match self.settings.page {
                Page::Appearance => self.appearance_settings(ui),
                Page::Resources => self.resource_settings(ui),
                Page::Queries => self.query_settings(ui),
                Page::Discovery => unreachable!(),
            });
    }

    pub(super) fn save_settings(&mut self) {
        if std::mem::take(&mut self.settings.dirty) {
            self.preferences.normalize();
            self.store.save(&self.preferences);
        }
    }

    fn setting_response(&mut self, response: egui::Response) {
        self.settings.dirty |= response.changed();
        if response.lost_focus()
            || response.drag_stopped()
            || response.changed() && !response.has_focus() && !response.dragged()
        {
            self.save_settings();
        }
    }

    fn appearance_settings(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.label("Theme");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.weak("Aa").on_hover_text(&self.database.ui.font_label);
                    for (mode, icon, name) in [
                        (Mode::Dark, Icon::Moon, "Dark theme"),
                        (Mode::Light, Icon::Sun, "Light theme"),
                        (Mode::System, Icon::Desktop, "Follow system"),
                    ] {
                        if icons::control(
                            ui,
                            icon,
                            name,
                            "",
                            self.preferences.appearance.unwrap_or_default() == mode,
                            false,
                        )
                        .clicked()
                        {
                            self.preferences.appearance = Some(mode);
                            self.apply_theme(ui.ctx());
                            self.settings.dirty = true;
                            self.save_settings();
                        }
                    }
                });
            });
            let before = self.preferences.font_size;
            let response = preference_row(
                ui,
                "Font size",
                "Type or drag to change text size",
                false,
                |ui| {
                    let increase = ui
                        .add_enabled_ui(self.preferences.font_size < 20, |ui| {
                            icons::button(ui, Icon::Plus, "Increase font size", "")
                        })
                        .inner
                        .clicked();
                    let size = ui.add(
                        egui::DragValue::new(&mut self.preferences.font_size)
                            .range(10..=20)
                            .suffix(" pt"),
                    );
                    size.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Font size")
                    });
                    let decrease = ui
                        .add_enabled_ui(self.preferences.font_size > 10, |ui| {
                            icons::button(ui, Icon::Minimize, "Decrease font size", "")
                        })
                        .inner
                        .clicked();
                    let reset = icons::button(ui, Icon::Undo, "Reset font size", "").clicked();
                    if increase {
                        self.preferences.font_size += 1;
                    }
                    if decrease {
                        self.preferences.font_size -= 1;
                    }
                    if reset {
                        self.preferences.font_size = 14;
                    }
                    size
                },
            );
            if before != self.preferences.font_size {
                self.apply_theme(ui.ctx());
                // Font size is global; refit columns in every retained workspace.
                for database in std::iter::once(&mut self.database).chain(self.parked.values_mut())
                {
                    for document in std::iter::once(&mut database.document)
                        .chain(database.documents.values_mut())
                    {
                        document.tools.columns.refit = true;
                        for columns in document.tools.column_layouts.values_mut() {
                            columns.refit = true;
                        }
                    }
                }
                self.settings.dirty = true;
                if !response.changed() {
                    self.save_settings();
                }
            }
            self.setting_response(response);
            let response = preference_row(
                ui,
                "NULL marker",
                "Display marker only; stored values are unchanged",
                false,
                |ui| {
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.preferences.null_display)
                            .char_limit(32)
                            .desired_width(90.0),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::TextInput, true, "NULL marker")
                    });
                    response
                },
            );
            self.setting_response(response);
        });
    }

    fn resource_settings(&mut self, ui: &mut egui::Ui) {
        card(ui, |ui| {
            let automatic =
                self.memory.automatic_budget_mib() / crate::resources::DATABASE_RESERVATION_MIB;
            let mut count = self.memory.limits(&self.preferences).database_limit;
            let maximum = usize::try_from(
                self.memory
                    .available_mib
                    .or(self.memory.total_mib)
                    .unwrap_or_else(|| self.memory.automatic_budget_mib().saturating_mul(4))
                    / crate::resources::DATABASE_RESERVATION_MIB,
            )
            .unwrap_or(usize::MAX)
            .max(count)
            .max(1);
            ui.horizontal(|ui| {
                ui.strong(format!("{count} databases"));
                reopen_mark(ui, "Changes apply to new opens. Existing work stays open.");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui
                        .add_enabled_ui(
                            self.preferences.database_limit.is_some()
                                || self.preferences.memory_budget_mib.is_some(),
                            |ui| icons::button(ui, Icon::Undo, "Reset database resources", ""),
                        )
                        .inner
                        .on_hover_text(format!(
                            "Restore the system recommendation: {automatic} databases"
                        ))
                        .clicked()
                    {
                        self.preferences.database_limit = None;
                        self.preferences.memory_budget_mib = None;
                        count = self.memory.limits(&self.preferences).database_limit;
                        self.settings.dirty = true;
                        self.save_settings();
                    }
                });
            });
            let minimum = usize::from(count > 0);
            let response = ui
                .scope(|ui| {
                    ui.spacing_mut().slider_width = ui.available_width();
                    let tokens = crate::appearance::Tokens::of(ui.ctx());
                    ui.visuals_mut().widgets.inactive.bg_fill = tokens.line_strong;
                    ui.visuals_mut().selection.bg_fill = tokens.primary;
                    ui.add(
                        egui::Slider::new(&mut count, minimum..=maximum)
                            .show_value(false)
                            .trailing_fill(true),
                    )
                })
                .inner;
            response.widget_info(|| {
                egui::WidgetInfo::slider(response.enabled(), count as f64, "Maximum open databases")
            });
            let detected = match (self.memory.available_mib, self.memory.total_mib) {
                (Some(available), Some(total)) => format!(
                    "At startup: {} available / {} total.{}",
                    crate::resources::format_mib(available),
                    crate::resources::format_mib(total),
                    if self.memory.process_limited {
                        " Process memory limits included."
                    } else {
                        ""
                    }
                ),
                _ => format!(
                    "Available memory could not be detected. Fallback budget: {}.",
                    crate::resources::format_mib(self.memory.automatic_budget_mib())
                ),
            };
            let recommendation = format!(
                "{detected} System recommendation: {automatic} databases. Uses 25% of memory available at startup. 512 MiB per database; actual use varies."
            );
            let track = response
                .rect
                .shrink2(egui::vec2(response.rect.height() / 2.0, 0.0));
            let fraction = ((automatic as f32 - minimum as f32)
                / (maximum.saturating_sub(minimum).max(1)) as f32)
                .clamp(0.0, 1.0);
            let x = egui::lerp(track.x_range(), fraction);
            ui.painter().vline(
                x,
                response.rect.y_range(),
                egui::Stroke::new(2.0, crate::appearance::Tokens::of(ui.ctx()).muted),
            );
            ui.interact(
                egui::Rect::from_center_size(
                    egui::pos2(x, response.rect.center().y),
                    egui::vec2(8.0, response.rect.height()),
                ),
                ui.scope_id().with("recommendation"),
                egui::Sense::hover(),
            )
            .on_hover_text(recommendation);
            if response.changed() {
                self.preferences.database_limit = Some(count.max(1));
                self.preferences.memory_budget_mib = Some(
                    (count as u64)
                        .saturating_mul(crate::resources::DATABASE_RESERVATION_MIB)
                        .max(1),
                );
            }
            self.setting_response(response);
            let estimated = (self.memory.limits(&self.preferences).database_limit as u64)
                .saturating_mul(crate::resources::DATABASE_RESERVATION_MIB);
            ui.horizontal_wrapped(|ui| {
                ui.strong(format!(
                    "{} × {} MiB ≈ {}",
                    self.memory.limits(&self.preferences).database_limit,
                    crate::resources::DATABASE_RESERVATION_MIB,
                    crate::resources::format_mib(estimated)
                ));
                ui.weak("estimated")
                    .on_hover_text("512 MiB per database; actual use varies.");
            });
            ui.small("Changes apply to new opens. Existing work stays open.");
        });
    }

    fn query_settings(&mut self, ui: &mut egui::Ui) {
        ui.spacing_mut().item_spacing.y = 3.0;
        section(ui, "Limits", |ui| {
            let response = preference_row(ui, "Rows", "Maximum retained query rows", false, |ui| {
                ui.add(egui::DragValue::new(&mut self.preferences.row_limit).range(1..=10_000))
            });
            self.setting_response(response);
            let response = preference_row(ui, "Slow ≥", "Slow query threshold", false, |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.preferences.slow_query_ms)
                        .range(1..=60_000)
                        .suffix(" ms"),
                )
            });
            self.setting_response(response);
            let response = preference_row(
                ui,
                "Busy wait",
                "Busy timeout; applies after reopening the database",
                true,
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.preferences.busy_timeout_ms)
                            .range(0..=5000)
                            .suffix(" ms"),
                    )
                },
            );
            self.setting_response(response);
            let response = preference_row(
                ui,
                "Deadline",
                "Statement deadline after reopen; 0 disables. Prior committed work is not rolled back.",
                true,
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.preferences.query_timeout_ms)
                            .range(0..=3_600_000)
                            .suffix(" ms"),
                    )
                },
            );
            self.setting_response(response);
            ui.small("Applies after reopening the database.");
        });
        section(ui, "SQL recovery", |ui| self.recovery_settings(ui));
        section(ui, "All databases", |ui| {
            if clear_queries_row(
                ui,
                Icon::History,
                "History",
                self.preferences.query_history.len(),
                "Clear history for all databases",
            ) {
                self.preferences.query_history.clear();
                self.store.save(&self.preferences);
            }
            if clear_queries_row(
                ui,
                Icon::Saved,
                "Saved queries",
                self.preferences.saved_queries.len(),
                "Clear saved queries for all databases",
            ) {
                self.preferences.saved_queries.clear();
                self.store.save(&self.preferences);
            }
        });
    }
}

fn clear_queries_row(ui: &mut egui::Ui, icon: Icon, title: &str, count: usize, name: &str) -> bool {
    ui.push_id(title, |ui| {
        ui.horizontal_wrapped(|ui| {
            let (rect, _) = ui.allocate_exact_size(egui::vec2(16.0, 16.0), egui::Sense::hover());
            icons::paint(ui.painter(), rect, icon, ui.visuals().weak_text_color());
            ui.label(format!("{title} · {count}"));
            let response = ui.add_enabled(count > 0, egui::Button::new("Clear for all databases"));
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, count > 0, name));
            response.on_hover_text(name).clicked()
        })
        .inner
    })
    .inner
}

fn pages() -> [(Page, Icon, &'static str, &'static str); 4] {
    [
        (Page::Appearance, Icon::Sun, "Appearance", "Appearance"),
        (Page::Discovery, Icon::Search, "Discovery", "Discovery"),
        (
            Page::Resources,
            Icon::Database,
            "Resources",
            "Database resources",
        ),
        (
            Page::Queries,
            Icon::Sql,
            "Queries",
            "Queries and SQL recovery",
        ),
    ]
}

fn preference_row(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: &str,
    reopen: bool,
    control: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    ui.horizontal(|ui| {
        let label = ui.label(label).on_hover_text(tooltip);
        if reopen {
            reopen_mark(ui, tooltip);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            control(ui).labelled_by(label.id).on_hover_text(tooltip)
        })
        .inner
    })
    .inner
}

fn reopen_mark(ui: &mut egui::Ui, tooltip: &str) {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(13.0, 13.0), egui::Sense::hover());
    icons::paint(
        ui.painter(),
        rect,
        Icon::Refresh,
        ui.visuals().weak_text_color(),
    );
    response.on_hover_text(tooltip);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::test_ui::{control, pointer, render};

    #[test]
    fn settings_pages_fit_narrow_and_wide_windows_with_scaled_text() {
        for dark in [false, true] {
            for font in [14, 20] {
                let (ctx, mut app) = crate::app::test_ui::app();
                app.preferences.appearance = Some(Mode::from_dark(dark));
                app.preferences.font_size = font;
                app.apply_theme(&ctx);
                app.preferences.query_timeout_ms = 3_600_000;
                app.preferences.null_display = "n".repeat(32);
                for width in [280.0, 440.0, 640.0, 900.0] {
                    for page in [
                        Page::Appearance,
                        Page::Discovery,
                        Page::Resources,
                        Page::Queries,
                    ] {
                        app.settings.page = page;
                        for _ in 0..2 {
                            let output = render(&ctx, egui::vec2(width, 600.0), vec![], |ui| {
                                app.settings_ui(ui)
                            });
                            for (_, node) in crate::app::test_ui::nodes(&output) {
                                if matches!(
                                    node.role(),
                                    egui::Role::Button
                                        | egui::Role::TextInput
                                        | egui::Role::Slider
                                        | egui::Role::SpinButton
                                ) && let Some(bounds) = node.bounds()
                                {
                                    assert!(
                                        bounds.x0 >= 0.0 && bounds.x1 <= f64::from(width) + 1.0,
                                        "{page:?}, {width}, {font}, {:?}: {bounds:?}",
                                        node.label()
                                    );
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn editing_a_preference_saves_when_switching_pages() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let size = egui::vec2(640.0, 540.0);
        let output = render(&ctx, size, vec![], |ui| app.settings_ui(ui));
        let bounds = crate::app::test_ui::nodes(&output)
            .iter()
            .find_map(|(_, node)| {
                (node.role() == egui::Role::TextInput)
                    .then(|| node.bounds())
                    .flatten()
            })
            .unwrap();
        let pos = egui::pos2(
            (bounds.x0 + bounds.x1) as f32 / 2.0,
            (bounds.y0 + bounds.y1) as f32 / 2.0,
        );
        for pressed in [true, false] {
            render(&ctx, size, pointer(pos, pressed), |ui| app.settings_ui(ui));
        }
        render(&ctx, size, vec![egui::Event::Text("marker".into())], |ui| {
            app.settings_ui(ui)
        });
        assert!(app.preferences.null_display.contains("marker"));
        assert!(app.settings.dirty);
        let output = render(&ctx, size, vec![], |ui| app.settings_ui(ui));
        let pos = control(&output, "Queries and SQL recovery").center();
        for pressed in [true, false] {
            render(&ctx, size, pointer(pos, pressed), |ui| app.settings_ui(ui));
        }
        assert_eq!(app.settings.page, Page::Queries);
        assert!(!app.settings.dirty);
    }
    #[test]
    fn settings_window_keeps_its_title_and_scales_when_font_changes() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let size = egui::vec2(1280.0, 720.0);
        app.settings.open = true;
        for _ in 0..3 {
            render(&ctx, size, vec![], |ui| app.render(ui));
        }
        for page in [
            Page::Appearance,
            Page::Discovery,
            Page::Resources,
            Page::Queries,
            Page::Discovery,
            Page::Queries,
        ] {
            app.settings.page = page;
            for _ in 0..3 {
                let output = render(&ctx, size, vec![], |ui| app.render(ui));
                let rect = ctx
                    .memory(|m| m.area_rect(egui::Id::unique("application-settings")))
                    .unwrap();
                assert!(output.shapes.iter().any(|s| matches!(&s.shape, egui::Shape::Text(t) if t.galley.job.text == "Settings" && rect.contains(t.pos))), "missing window title on {page:?}");
            }
        }
        let before = ctx
            .memory(|m| m.area_rect(egui::Id::unique("application-settings")))
            .unwrap();
        app.preferences.font_size = 20;
        app.apply_theme(&ctx);
        for _ in 0..3 {
            render(&ctx, size, vec![], |ui| app.render(ui));
        }
        let after = ctx
            .memory(|m| m.area_rect(egui::Id::unique("application-settings")))
            .unwrap();
        assert!(after.width() > before.width());
        assert!(after.width() <= size.x && after.height() <= size.y);
    }
}
