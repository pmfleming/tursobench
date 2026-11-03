//! Compact controls shared by the left navigator's tool panels.
use super::{
    App, View,
    icons::{self, Icon},
};
use crate::appearance::Mode;
use eframe::egui;
use tursobench_db::{Diagnostic, Operation};

impl App {
    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        if heading_action(ui, "Settings", Icon::Saved, "Save settings", true) {
            self.preferences.normalize();
            self.store.save(&self.preferences);
            self.status = "Saving preferences…".into();
        }
        section(ui, "Appearance", |ui| {
            ui.horizontal(|ui| {
                for (mode, icon, name) in [
                    (Mode::System, Icon::Desktop, "Follow system"),
                    (Mode::Light, Icon::Sun, "Light theme"),
                    (Mode::Dark, Icon::Moon, "Dark theme"),
                ] {
                    let response = icons::control(
                        ui,
                        icon,
                        name,
                        "",
                        self.preferences.appearance == Some(mode),
                        false,
                    );
                    if response.clicked() {
                        self.preferences.appearance = Some(mode);
                        self.preferences.legacy_appearance = false;
                        self.apply_theme(ui.ctx());
                        self.store.save(&self.preferences);
                    }
                }
                ui.weak("Aa").on_hover_text(&self.ui.font_label);
            });
            let before = self.preferences.font_size;
            ui.horizontal(|ui| {
                ui.label("Font size");
                if icons::button(ui, Icon::Undo, "Reset font size", "").clicked() {
                    self.preferences.font_size = 14;
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled_ui(self.preferences.font_size > 10, |ui| {
                        icons::button(ui, Icon::Minimize, "Decrease font size", "")
                    })
                    .inner
                    .clicked()
                {
                    self.preferences.font_size -= 1;
                }
                let size = ui.add(
                    egui::DragValue::new(&mut self.preferences.font_size)
                        .range(10..=20)
                        .suffix(" pt"),
                );
                size.widget_info(|| {
                    egui::WidgetInfo::labeled(egui::Role::SpinButton, true, "Font size")
                });
                size.on_hover_text("Type or drag to change text size");
                if ui
                    .add_enabled_ui(self.preferences.font_size < 20, |ui| {
                        icons::button(ui, Icon::Plus, "Increase font size", "")
                    })
                    .inner
                    .clicked()
                {
                    self.preferences.font_size += 1;
                }
            });
            if before != self.preferences.font_size {
                self.apply_theme(ui.ctx());
                for document in
                    std::iter::once(&mut self.document).chain(self.documents.values_mut())
                {
                    document.tools.columns.refit = true;
                    for columns in document.tools.column_layouts.values_mut() {
                        columns.refit = true;
                    }
                }
                self.store.save(&self.preferences);
            }
        });
        section(ui, "Queries", |ui| {
            setting(ui, "Rows", "Maximum retained query rows", |ui| {
                ui.add(egui::DragValue::new(&mut self.preferences.row_limit).range(1..=10_000))
            });
            setting(
                ui,
                "Busy wait",
                "Busy timeout; applies after reopening the database",
                |ui| {
                    ui.add(
                        egui::DragValue::new(&mut self.preferences.busy_timeout_ms)
                            .range(0..=5000)
                            .suffix(" ms"),
                    )
                },
            );
            setting(ui, "Slow ≥", "Slow query threshold", |ui| {
                ui.add(
                    egui::DragValue::new(&mut self.preferences.slow_query_ms)
                        .range(1..=60_000)
                        .suffix(" ms"),
                )
            });
        });
        section(ui, "Display", |ui| {
            setting(
                ui,
                "NULL",
                "Display marker only; stored values are unchanged",
                |ui| {
                    ui.add(
                        egui::TextEdit::singleline(&mut self.preferences.null_display)
                            .char_limit(32)
                            .desired_width(ui.available_width().min(90.0)),
                    )
                },
            );
        });
        section(ui, "Discovery", |ui| {
            self.discovery_settings_ui(ui);
            if let Some(message) = &self.databases.message {
                ui.label(message);
            }
        });
    }

    pub(super) fn saved_queries_ui(&mut self, ui: &mut egui::Ui) {
        if heading_action(
            ui,
            "Saved queries",
            Icon::Trash,
            "Clear saved queries for all databases",
            !self.preferences.saved_queries.is_empty(),
        ) {
            self.preferences.saved_queries.clear();
            self.preferences.bookmarks.clear();
            self.store.save(&self.preferences);
        }
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
            ui.add_sized(
                [width, 28.0],
                egui::TextEdit::singleline(&mut self.tools.bookmark_name)
                    .hint_text("Query name…")
                    .char_limit(64),
            )
            .on_hover_text("Name for the current editor SQL");
            let save = ui
                .add_enabled_ui(!self.document.sql.trim().is_empty(), |ui| {
                    icons::button(ui, Icon::Plus, "Save current SQL", "")
                })
                .inner;
            if save.clicked() {
                let name = if self.tools.bookmark_name.trim().is_empty() {
                    self.document.sql.chars().take(60).collect()
                } else {
                    self.tools.bookmark_name.clone()
                };
                self.preferences
                    .bookmark(&self.tools.database_scope, &name, &self.document.sql);
                self.store.save(&self.preferences);
            }
        });
        search(ui, &mut self.tools.bookmark_search, "Search saved queries…");
        ui.add_space(4.0);
        let query = self.tools.bookmark_search.to_lowercase();
        let mut load = None;
        let mut remove = None;
        let mut renamed = false;
        let mut visible = 0;
        for (index, bookmark) in self.preferences.saved_queries.iter_mut().enumerate() {
            if !(bookmark.database.is_empty() || bookmark.database == self.tools.database_scope)
                || !format!("{} {}", bookmark.name, bookmark.sql)
                    .to_lowercase()
                    .contains(&query)
            {
                continue;
            }
            visible += 1;
            ui.push_id(("saved-query", index), |ui| {
                card(ui, |ui| {
                    let name = ui.add(
                        egui::TextEdit::singleline(&mut bookmark.name)
                            .char_limit(64)
                            .desired_width(ui.available_width())
                            .frame(egui::Frame::NONE),
                    );
                    name.widget_info(|| {
                        egui::WidgetInfo::labeled(
                            egui::Role::TextInput,
                            ui.is_enabled(),
                            "Rename saved query",
                        )
                    });
                    renamed |= name.changed();
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(sql_preview(&bookmark.sql))
                                .monospace()
                                .weak(),
                        )
                        .truncate(),
                    )
                    .on_hover_text(&bookmark.sql);
                    ui.horizontal(|ui| {
                        if bookmark.database.is_empty() {
                            ui.weak("Legacy")
                                .on_hover_text("Saved before database-scoped queries");
                        }
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, Icon::Trash, "Delete saved query", "").clicked() {
                                remove = Some(index);
                            }
                            if icons::button(ui, Icon::Sql, "Load saved query", "").clicked() {
                                load = Some(bookmark.sql.clone());
                            }
                        });
                    });
                });
            });
        }
        if visible == 0 {
            ui.weak("No saved queries");
        }
        if renamed {
            self.store.save(&self.preferences);
        }
        if let Some(index) = remove {
            self.preferences.saved_queries.remove(index);
            self.store.save(&self.preferences);
        }
        if let Some(sql) = load {
            self.load_sql_window(sql);
        }
    }

    pub(super) fn query_history_ui(&mut self, ui: &mut egui::Ui) {
        if heading_action(
            ui,
            "History",
            Icon::Trash,
            "Clear history for all databases",
            !self.preferences.query_history.is_empty(),
        ) {
            self.preferences.query_history.clear();
            self.preferences.history.clear();
            self.store.save(&self.preferences);
        }
        search(ui, &mut self.tools.history_search, "Search history…");
        ui.horizontal(|ui| {
            if icons::control(
                ui,
                Icon::Filter,
                "Slow queries only",
                "",
                self.tools.slow_only,
                false,
            )
            .clicked()
            {
                self.tools.slow_only = !self.tools.slow_only;
            }
            if icons::control(
                ui,
                Icon::Down,
                "Slowest first",
                "",
                self.tools.sort_slow,
                false,
            )
            .clicked()
            {
                self.tools.sort_slow = !self.tools.sort_slow;
            }
        });
        ui.add_space(4.0);
        let query = self.tools.history_search.to_lowercase();
        let threshold = self.preferences.slow_query_ms;
        let mut entries: Vec<_> = self
            .preferences
            .query_history
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                (h.database.is_empty() || h.database == self.tools.database_scope)
                    && h.sql.to_lowercase().contains(&query)
                    && (!self.tools.slow_only || h.elapsed_ms.is_some_and(|ms| ms >= threshold))
            })
            .collect();
        if self.tools.sort_slow {
            entries.sort_by_key(|(_, h)| std::cmp::Reverse(h.elapsed_ms));
        }
        if entries.is_empty() {
            ui.weak("No history");
        }
        let mut load = None;
        let mut remove = None;
        for (index, entry) in entries {
            ui.push_id(("history-query", index), |ui| {
                card(ui, |ui| {
                    let preview = ui
                        .add_sized(
                            [ui.available_width(), 24.0],
                            egui::Button::new(
                                egui::RichText::new(sql_preview(&entry.sql)).monospace(),
                            )
                            .truncate()
                            .frame(false),
                        )
                        .on_hover_text(&entry.sql);
                    if preview.clicked() {
                        load = Some(entry.sql.clone());
                    }
                    ui.horizontal(|ui| {
                        ui.small(entry.elapsed_ms.map_or("—".into(), |ms| format!("{ms} ms")));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if icons::button(ui, Icon::Trash, "Delete history entry", "").clicked()
                            {
                                remove = Some(index);
                            }
                            if icons::button(ui, Icon::Sql, "Load history query", "").clicked() {
                                load = Some(entry.sql.clone());
                            }
                        });
                    });
                    if entry.failed
                        || entry.elapsed_ms.is_some_and(|ms| ms >= threshold)
                        || entry.database.is_empty()
                    {
                        ui.horizontal_wrapped(|ui| {
                            if entry.failed {
                                ui.colored_label(ui.visuals().error_fg_color, "Failed");
                            }
                            if entry.elapsed_ms.is_some_and(|ms| ms >= threshold) {
                                ui.colored_label(ui.visuals().warn_fg_color, "Slow");
                            }
                            if entry.database.is_empty() {
                                ui.weak("Legacy");
                            }
                        });
                    }
                });
            });
        }
        if let Some(index) = remove {
            self.preferences.query_history.remove(index);
            self.store.save(&self.preferences);
        }
        if let Some(sql) = load {
            self.load_sql_window(sql);
        }
    }

    pub(super) fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Checks");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (rect, response) =
                    ui.allocate_exact_size(egui::vec2(20.0, 20.0), egui::Sense::hover());
                icons::paint(
                    ui.painter(),
                    rect,
                    Icon::Lock,
                    ui.visuals().weak_text_color(),
                );
                response.on_hover_text("Read-only checks");
            });
        });
        for (title, checks) in [
            (
                "Storage",
                &[
                    (Diagnostic::DatabaseList, Icon::Database, "Files"),
                    (Diagnostic::JournalMode, Icon::History, "Journal"),
                    (Diagnostic::PageCount, Icon::Data, "Pages"),
                    (Diagnostic::FreelistCount, Icon::Count, "Free pages"),
                ][..],
            ),
            (
                "Validation",
                &[
                    (Diagnostic::Integrity, Icon::Checks, "Integrity"),
                    (Diagnostic::ForeignKeys, Icon::Link, "Foreign keys"),
                ][..],
            ),
        ] {
            section(ui, title, |ui| {
                for &(diagnostic, icon, label) in checks {
                    let response = ui
                        .add_enabled_ui(self.ready && !self.closing, |ui| {
                            action_row(ui, icon, label, diagnostic.label())
                        })
                        .inner;
                    if response.clicked() {
                        self.submit(Operation::Diagnostic(diagnostic), View::Diagnostics);
                    }
                }
            });
        }
    }
}

fn heading_action(ui: &mut egui::Ui, title: &str, icon: Icon, name: &str, enabled: bool) -> bool {
    ui.horizontal(|ui| {
        ui.strong(title);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_enabled_ui(enabled, |ui| icons::button(ui, icon, name, ""))
                .inner
                .clicked()
        })
        .inner
    })
    .inner
}

fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(crate::appearance::Tokens::of(ui.ctx()).muted),
    );
    card(ui, contents);
}

fn card(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(9.0)
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui);
        });
}

fn setting(
    ui: &mut egui::Ui,
    label: &str,
    tooltip: &str,
    control: impl FnOnce(&mut egui::Ui) -> egui::Response,
) {
    ui.horizontal(|ui| {
        let label = ui.label(label).on_hover_text(tooltip);
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            control(ui).labelled_by(label.id).on_hover_text(tooltip);
        });
    });
}

fn search(ui: &mut egui::Ui, value: &mut String, hint: &str) {
    ui.add_sized(
        [ui.available_width(), 28.0],
        egui::TextEdit::singleline(value).hint_text(hint),
    );
}

fn action_row(ui: &mut egui::Ui, icon: Icon, label: &str, name: &str) -> egui::Response {
    let response = ui.add_sized([ui.available_width(), 32.0], egui::Button::new(""));
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
    ui.painter().text(
        egui::pos2(response.rect.left() + 34.0, response.rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        ui.style().interact(&response).fg_stroke.color,
    );
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(response.rect.left() + 16.0, response.rect.center().y),
            egui::vec2(16.0, 16.0),
        ),
        icon,
        ui.style().interact(&response).fg_stroke.color,
    );
    response.on_hover_text(name)
}

fn sql_preview(sql: &str) -> String {
    sql.split_whitespace()
        .take(24)
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(120)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::{App, Diagnostic, Mode, egui};
    use crate::app::shell::ToolPanel;

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        panel: ToolPanel,
        width: f32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        crate::app::test_ui::render(ctx, egui::vec2(width, 1400.0), events, |ui| match panel {
            ToolPanel::Settings => app.settings_ui(ui),
            ToolPanel::History => app.query_history_ui(ui),
            ToolPanel::Saved => app.saved_queries_ui(ui),
            ToolPanel::Diagnostics => app.diagnostics_ui(ui),
            _ => unreachable!(),
        })
    }

    fn click(app: &mut App, ctx: &egui::Context, panel: ToolPanel, name: &str) {
        let output = frame(app, ctx, panel, 240.0, vec![]);
        let pos = crate::app::test_ui::control(&output, name).center();
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                panel,
                240.0,
                crate::app::test_ui::pointer(pos, pressed),
            );
        }
    }

    #[test]
    fn settings_expose_discovery_and_font_controls_and_apply_size_without_drift() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
        let output = frame(&mut app, &ctx, ToolPanel::Settings, 240.0, vec![]);
        for name in [
            "Search folder path",
            "Excluded folder path",
            "Refresh databases",
            "Decrease font size",
            "Increase font size",
            "Reset font size",
        ] {
            crate::app::test_ui::control(&output, name).center();
        }
        click(&mut app, &ctx, ToolPanel::Settings, "Increase font size");
        assert_eq!(app.preferences.font_size, 15);
        assert_eq!(
            ctx.global_style().text_styles[&egui::TextStyle::Body].size,
            15.0
        );
        assert!(app.document.tools.columns.refit);
        let larger = crate::appearance::monospace(&ctx, 12.0).size;
        assert!(larger > 12.0);
        for mode in [Mode::Light, Mode::Dark, Mode::System] {
            app.preferences.appearance = Some(mode);
            app.apply_theme(&ctx);
            app.apply_theme(&ctx);
            assert_eq!(crate::appearance::monospace(&ctx, 12.0).size, larger);
        }
        click(&mut app, &ctx, ToolPanel::Settings, "Decrease font size");
        assert_eq!(app.preferences.font_size, 14);
        click(&mut app, &ctx, ToolPanel::Settings, "Increase font size");
        click(&mut app, &ctx, ToolPanel::Settings, "Reset font size");
        assert_eq!(app.preferences.font_size, 14);
        assert_eq!(crate::appearance::monospace(&ctx, 12.0).size, 12.0);
    }

    #[test]
    fn navigator_controls_fit_narrow_panels_in_both_themes() {
        for dark in [false, true] {
            let ctx = egui::Context::default();
            ctx.enable_accesskit();
            let mut app = App::new(&ctx);
            crate::appearance::theme(&ctx, dark);
            app.tools.database_scope = "/work/current.db".into();
            app.preferences.row_limit = 10_000;
            app.preferences.busy_timeout_ms = 5000;
            app.preferences.slow_query_ms = 60_000;
            app.preferences.null_display = "n".repeat(32);
            app.preferences.record_query(
                "/work/current.db",
                "SELECT 'a long query preview which must stay inside the sidebar'",
                Some(60000),
                true,
            );
            app.preferences
                .bookmark("/work/current.db", &"long name ".repeat(6), "SELECT 1");
            for width in [180.0, 240.0, 300.0] {
                for panel in [
                    ToolPanel::Settings,
                    ToolPanel::History,
                    ToolPanel::Saved,
                    ToolPanel::Diagnostics,
                ] {
                    frame(&mut app, &ctx, panel, width, vec![]);
                    let output = frame(&mut app, &ctx, panel, width, vec![]);
                    let tree = output.platform_output.accesskit_update.as_ref().unwrap();
                    for (_, node) in &tree.nodes {
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
                                "{panel:?}, width {width}, {:?}: {bounds:?}",
                                node.label()
                            );
                        }
                    }
                    if panel == ToolPanel::Diagnostics {
                        for diagnostic in Diagnostic::ALL {
                            crate::app::test_ui::control(&output, diagnostic.label());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn history_icons_filter_load_without_running_and_delete_only_the_chosen_entry() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
        app.tools.database_scope = "/work/current.db".into();
        app.preferences
            .record_query("/work/other.db", "SELECT 'other'", Some(900), false);
        app.preferences
            .record_query("/work/current.db", "SELECT 'fast'", Some(10), false);
        app.preferences
            .record_query("/work/current.db", "SELECT 'slow'", Some(900), false);
        app.document.params = "[42]".into();
        let sequence = app.sequence;
        click(&mut app, &ctx, ToolPanel::History, "Slow queries only");
        assert!(app.tools.slow_only);
        let output = frame(&mut app, &ctx, ToolPanel::History, 240.0, vec![]);
        assert_eq!(
            output
                .platform_output
                .accesskit_update
                .unwrap()
                .nodes
                .iter()
                .filter(|(_, n)| n.label() == Some("Load history query"))
                .count(),
            1
        );
        click(&mut app, &ctx, ToolPanel::History, "Load history query");
        assert_eq!(app.document.sql, "SELECT 'slow'");
        assert_eq!(app.document.params, "[]");
        assert_eq!(app.sequence, sequence);
        assert!(app.document.active.is_none());
        click(&mut app, &ctx, ToolPanel::History, "Delete history entry");
        assert_eq!(app.preferences.query_history.len(), 2);
        assert!(
            app.preferences
                .query_history
                .iter()
                .all(|h| h.sql != "SELECT 'slow'")
        );
    }

    #[test]
    fn saved_query_icons_keep_sql_and_scope_and_theme_controls_select_their_mode() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
        app.tools.database_scope = "/work/current.db".into();
        app.document.sql = "SELECT 'saved'".into();
        app.tools.bookmark_name = "Example".into();
        app.preferences
            .bookmark("/work/other.db", "Other", "SELECT 'other'");
        click(&mut app, &ctx, ToolPanel::Saved, "Save current SQL");
        assert_eq!(
            app.preferences.saved_queries[0].database,
            "/work/current.db"
        );
        assert_eq!(app.preferences.saved_queries[0].name, "Example");
        app.document.sql.clear();
        app.document.params = "[42]".into();
        click(&mut app, &ctx, ToolPanel::Saved, "Load saved query");
        assert_eq!(app.document.sql, "SELECT 'saved'");
        assert_eq!(app.document.params, "[]");
        assert!(app.document.active.is_none());
        click(&mut app, &ctx, ToolPanel::Saved, "Delete saved query");
        assert_eq!(app.preferences.saved_queries.len(), 1);
        assert_eq!(app.preferences.saved_queries[0].database, "/work/other.db");
        for (name, mode) in [
            ("Light theme", Mode::Light),
            ("Dark theme", Mode::Dark),
            ("Follow system", Mode::System),
        ] {
            click(&mut app, &ctx, ToolPanel::Settings, name);
            assert_eq!(app.preferences.appearance, Some(mode));
        }
    }
}
