//! Compact controls shared by the left navigator's tool panels.
use super::{
    App, View,
    icons::{self, Icon},
};
use eframe::egui;
use tursobench_db::{Diagnostic, Operation};

impl App {
    fn query_database_name(&self) -> String {
        let path = std::path::Path::new(&self.database.tools.database_scope);
        path.file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned()
    }

    pub(super) fn saved_queries_ui(&mut self, ui: &mut egui::Ui) {
        if heading_action(
            ui,
            "Saved queries",
            Icon::Trash,
            &format!("Clear saved queries for {}", self.query_database_name()),
            !self.database.tools.database_scope.is_empty()
                && self
                    .preferences
                    .saved_queries
                    .iter()
                    .any(|q| q.database == self.database.tools.database_scope),
        ) {
            self.preferences
                .saved_queries
                .retain(|q| q.database != self.database.tools.database_scope);
            self.store.save(&self.preferences);
        }
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
            ui.add_sized(
                [width, 28.0],
                egui::TextEdit::singleline(&mut self.database.tools.bookmark_name)
                    .hint_text("Query name…")
                    .char_limit(64),
            )
            .on_hover_text("Name for the current editor SQL");
            let save = ui
                .add_enabled_ui(!self.database.document.sql.trim().is_empty(), |ui| {
                    icons::button(ui, Icon::Plus, "Save current SQL", "")
                })
                .inner;
            if save.clicked() {
                let name = if self.database.tools.bookmark_name.trim().is_empty() {
                    self.database.document.sql.chars().take(60).collect()
                } else {
                    self.database.tools.bookmark_name.clone()
                };
                self.preferences.bookmark(
                    &self.database.tools.database_scope,
                    &name,
                    &self.database.document.sql,
                );
                self.store.save(&self.preferences);
            }
        });
        search(
            ui,
            &mut self.database.tools.bookmark_search,
            "Search saved queries…",
        );
        ui.add_space(4.0);
        let query = self.database.tools.bookmark_search.to_lowercase();
        let mut load = None;
        let mut remove = None;
        let mut renamed = false;
        let mut visible = 0;
        for (index, bookmark) in self.preferences.saved_queries.iter_mut().enumerate() {
            if !(bookmark.database.is_empty()
                || bookmark.database == self.database.tools.database_scope)
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
                        match query_actions(ui, "Load saved query", "Delete saved query") {
                            Some(QueryAction::Load) => load = Some(bookmark.sql.clone()),
                            Some(QueryAction::Delete) => remove = Some(index),
                            None => {}
                        }
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
            &format!("Clear history for {}", self.query_database_name()),
            !self.database.tools.database_scope.is_empty()
                && self
                    .preferences
                    .query_history
                    .iter()
                    .any(|q| q.database == self.database.tools.database_scope),
        ) {
            self.preferences
                .query_history
                .retain(|q| q.database != self.database.tools.database_scope);
            self.store.save(&self.preferences);
        }
        search(
            ui,
            &mut self.database.tools.history_search,
            "Search history…",
        );
        ui.horizontal(|ui| {
            for (selected, icon, name) in [
                (
                    &mut self.database.tools.slow_only,
                    Icon::Filter,
                    "Slow queries only",
                ),
                (
                    &mut self.database.tools.sort_slow,
                    Icon::Down,
                    "Slowest first",
                ),
            ] {
                if icons::control(ui, icon, name, "", *selected, false).clicked() {
                    *selected = !*selected;
                }
            }
        });
        ui.add_space(4.0);
        let query = self.database.tools.history_search.to_lowercase();
        let threshold = self.preferences.slow_query_ms;
        let mut entries: Vec<_> = self
            .preferences
            .query_history
            .iter()
            .enumerate()
            .filter(|(_, h)| {
                (h.database.is_empty() || h.database == self.database.tools.database_scope)
                    && (query.is_empty() || h.sql.to_lowercase().contains(&query))
                    && (!self.database.tools.slow_only
                        || h.elapsed_ms.is_some_and(|ms| ms >= threshold))
            })
            .collect();
        if self.database.tools.sort_slow {
            entries.sort_by_key(|(_, h)| std::cmp::Reverse(h.elapsed_ms));
        }
        if entries.is_empty() {
            ui.weak("No history");
        }
        let mut load = None;
        let mut remove = None;
        for (index, entry) in entries {
            match ui
                .push_id(("history-query", index), |ui| {
                    HistoryRow {
                        sql: &entry.sql,
                        elapsed_ms: entry.elapsed_ms,
                        failed: entry.failed,
                        slow: entry.elapsed_ms.is_some_and(|ms| ms >= threshold),
                        legacy: entry.database.is_empty(),
                    }
                    .show(ui)
                })
                .inner
            {
                Some(QueryAction::Load) => load = Some(entry.sql.clone()),
                Some(QueryAction::Delete) => remove = Some(index),
                None => {}
            }
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
        if ui.button("Compare schemas… (F7)").clicked() {
            self.comparison.open = true;
        }
        if ui
            .add_enabled(self.database.ready, egui::Button::new("PRAGMA dashboard…"))
            .clicked()
        {
            self.database.tools.admin.open = true;
        }
        if ui
            .add_enabled(self.database.ready, egui::Button::new("Data profiling…"))
            .clicked()
        {
            self.database.tools.profile.open = true;
        }
        if ui
            .add_enabled(
                self.database.ready,
                egui::Button::new("Data transfer / maintenance…"),
            )
            .clicked()
        {
            self.open_transfers();
        }
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
                        .add_enabled_ui(self.database.ready && !self.database.closing, |ui| {
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

pub(super) fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(crate::appearance::Tokens::of(ui.ctx()).muted),
    );
    card(ui, contents);
}

pub(super) fn card<R>(ui: &mut egui::Ui, contents: impl FnOnce(&mut egui::Ui) -> R) -> R {
    egui::Frame::new()
        .fill(ui.visuals().faint_bg_color)
        .stroke(ui.visuals().widgets.noninteractive.bg_stroke)
        .corner_radius(9.0)
        .inner_margin(9.0)
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            contents(ui)
        })
        .inner
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

#[derive(Clone, Copy)]
enum QueryAction {
    Load,
    Delete,
}

fn query_actions(ui: &mut egui::Ui, load: &str, delete: &str) -> Option<QueryAction> {
    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
        let delete = icons::button(ui, Icon::Trash, delete, "").clicked();
        let load = icons::button(ui, Icon::Sql, load, "").clicked();
        if load {
            Some(QueryAction::Load)
        } else if delete {
            Some(QueryAction::Delete)
        } else {
            None
        }
    })
    .inner
}

struct HistoryRow<'a> {
    sql: &'a str,
    elapsed_ms: Option<u64>,
    failed: bool,
    slow: bool,
    legacy: bool,
}
impl HistoryRow<'_> {
    fn show(&self, ui: &mut egui::Ui) -> Option<QueryAction> {
        card(ui, |ui| {
            let preview = ui
                .add_sized(
                    [ui.available_width(), 24.0],
                    egui::Button::new(egui::RichText::new(sql_preview(self.sql)).monospace())
                        .truncate()
                        .frame(false),
                )
                .on_hover_text(self.sql);
            let mut action = preview.clicked().then_some(QueryAction::Load);
            ui.horizontal(|ui| {
                ui.small(self.elapsed_ms.map_or("—".into(), |ms| format!("{ms} ms")));
                action = query_actions(ui, "Load history query", "Delete history entry").or(action);
            });
            let badges = [
                (self.failed, "Failed", ui.visuals().error_fg_color),
                (self.slow, "Slow", ui.visuals().warn_fg_color),
                (self.legacy, "Legacy", ui.visuals().weak_text_color()),
            ];
            if badges.iter().any(|(visible, _, _)| *visible) {
                ui.horizontal_wrapped(|ui| {
                    for (visible, label, color) in badges {
                        if visible {
                            ui.colored_label(color, label);
                        }
                    }
                });
            }
            action
        })
    }
}

/// Bound work and allocation even when the first SQL word is very large.
fn sql_preview(sql: &str) -> String {
    let mut preview = String::with_capacity(sql.len().min(120));
    let mut remaining = 120;
    for (index, word) in sql.split_whitespace().take(24).enumerate() {
        if index > 0 {
            preview.push(' ');
            remaining -= 1;
        }
        for character in word.chars().take(remaining) {
            preview.push(character);
            remaining -= 1;
        }
        if remaining == 0 {
            break;
        }
    }
    preview
}

#[cfg(test)]
mod tests {
    use super::{App, Diagnostic, egui};
    use crate::{app::settings::Page, appearance::Mode};
    // Render individual pages without requiring an open database.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum TestPanel {
        Settings,
        History,
        Saved,
        Diagnostics,
    }

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        panel: TestPanel,
        width: f32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        crate::app::test_ui::render(ctx, egui::vec2(width, 1400.0), events, |ui| match panel {
            TestPanel::Settings => app.settings_ui(ui),
            TestPanel::History => app.query_history_ui(ui),
            TestPanel::Saved => app.saved_queries_ui(ui),
            TestPanel::Diagnostics => app.diagnostics_ui(ui),
        })
    }

    fn click(app: &mut App, ctx: &egui::Context, panel: TestPanel, name: &str) {
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
    fn appearance_controls_apply_size_without_drift() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let output = frame(&mut app, &ctx, TestPanel::Settings, 240.0, vec![]);
        for name in [
            "Decrease font size",
            "Increase font size",
            "Reset font size",
        ] {
            crate::app::test_ui::control(&output, name).center();
        }
        click(&mut app, &ctx, TestPanel::Settings, "Increase font size");
        assert_eq!(app.preferences.font_size, 15);
        assert_eq!(
            ctx.global_style().text_styles[&egui::TextStyle::Body].size,
            15.0
        );
        assert!(app.database.document.tools.columns.refit);
        let larger = crate::appearance::monospace(&ctx, 12.0).size;
        assert!(larger > 12.0);
        for mode in [Mode::Light, Mode::Dark, Mode::System] {
            app.preferences.appearance = Some(mode);
            app.apply_theme(&ctx);
            app.apply_theme(&ctx);
            assert_eq!(crate::appearance::monospace(&ctx, 12.0).size, larger);
        }
        click(&mut app, &ctx, TestPanel::Settings, "Decrease font size");
        assert_eq!(app.preferences.font_size, 14);
        click(&mut app, &ctx, TestPanel::Settings, "Increase font size");
        click(&mut app, &ctx, TestPanel::Settings, "Reset font size");
        assert_eq!(app.preferences.font_size, 14);
        assert_eq!(crate::appearance::monospace(&ctx, 12.0).size, 12.0);
    }

    #[test]
    fn resource_slider_updates_admission_and_reset_restores_system_choice() {
        let (ctx, mut app) = crate::app::test_ui::app();
        let panel = TestPanel::Settings;
        app.settings.page = Page::Resources;
        let output = frame(&mut app, &ctx, panel, 240.0, vec![]);
        let slider = crate::app::test_ui::control(&output, "Maximum open databases");
        assert_eq!(app.memory.limits(&app.preferences).database_limit, 8);
        let position = egui::pos2(slider.right() - 1.0, slider.center().y);
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                panel,
                240.0,
                crate::app::test_ui::pointer(position, pressed),
            );
        }
        assert_eq!(app.preferences.database_limit, Some(32));
        assert_eq!(app.preferences.memory_budget_mib, Some(16 * 1024));
        assert_eq!(app.memory.limits(&app.preferences).database_limit, 32);
        let output = frame(&mut app, &ctx, panel, 240.0, vec![]);
        let slider = crate::app::test_ui::control(&output, "Maximum open databases");
        let position = egui::pos2(slider.left() + slider.width() / 8.0, slider.center().y);
        for pressed in [true, false] {
            frame(
                &mut app,
                &ctx,
                panel,
                240.0,
                crate::app::test_ui::pointer(position, pressed),
            );
        }
        let limits = app.memory.limits(&app.preferences);
        assert!(limits.database_limit > 0 && limits.database_limit < 8);
        assert_eq!(limits.budget_mib, limits.database_limit as u64 * 512);
        click(&mut app, &ctx, panel, "Reset database resources");
        assert_eq!(app.preferences.memory_budget_mib, None);
        assert_eq!(app.preferences.database_limit, None);
        assert_eq!(app.memory.limits(&app.preferences).database_limit, 8);
    }

    #[test]
    fn navigator_controls_fit_narrow_panels_in_both_themes() {
        for dark in [false, true] {
            let (ctx, mut app) = crate::app::test_ui::app();
            crate::appearance::theme(&ctx, dark);
            app.database.tools.database_scope = "/work/current.db".into();
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
                for panel in [TestPanel::History, TestPanel::Saved, TestPanel::Diagnostics] {
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
                    if panel == TestPanel::Diagnostics {
                        for diagnostic in Diagnostic::ALL {
                            crate::app::test_ui::control(&output, diagnostic.label());
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn query_library_filters_sorts_loads_and_deletes_scoped_entries_without_execution() {
        assert_eq!(super::sql_preview(" SELECT\n\t λ  "), "SELECT λ");
        assert_eq!(super::sql_preview(&"λ".repeat(32_768)), "λ".repeat(120));
        assert_eq!(super::sql_preview(&"x ".repeat(25)), "x ".repeat(23) + "x");
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.tools.database_scope = "/work/current.db".into();
        app.preferences
            .record_query("/work/other.db", "SELECT 'other'", Some(900), false);
        app.preferences
            .record_query("/work/current.db", "SELECT 'fast'", Some(10), false);
        app.preferences
            .record_query("/work/current.db", "SELECT 'slow'", Some(900), false);
        app.database.document.params = "[42]".into();
        let sequence = app.database.sequence;
        click(&mut app, &ctx, TestPanel::History, "Slow queries only");
        assert!(app.database.tools.slow_only);
        let output = frame(&mut app, &ctx, TestPanel::History, 240.0, vec![]);
        assert_eq!(
            crate::app::test_ui::nodes(&output)
                .iter()
                .filter(|(_, n)| n.label() == Some("Load history query"))
                .count(),
            1
        );
        click(&mut app, &ctx, TestPanel::History, "Load history query");
        assert_eq!(app.database.document.sql, "SELECT 'slow'");
        assert_eq!(app.database.document.params, "[]");
        assert_eq!(app.database.sequence, sequence);
        assert!(app.database.document.active.is_none());
        click(&mut app, &ctx, TestPanel::History, "Delete history entry");
        assert_eq!(app.preferences.query_history.len(), 2);
        assert!(
            app.preferences
                .query_history
                .iter()
                .all(|h| h.sql != "SELECT 'slow'")
        );
        for (sql, elapsed) in [
            ("SELECT 'tie-a'", 900),
            ("SELECT 'tie-b'", 900),
            ("SELECT 'middle'", 600),
        ] {
            app.preferences
                .record_query("/work/current.db", sql, Some(elapsed), false);
        }
        app.preferences
            .record_query("", "SELECT 'λ legacy'", None, true);
        click(&mut app, &ctx, TestPanel::History, "Slow queries only");
        click(&mut app, &ctx, TestPanel::History, "Slowest first");
        let output = frame(&mut app, &ctx, TestPanel::History, 240.0, vec![]);
        let queries = output
            .shapes
            .iter()
            .filter_map(|shape| match &shape.shape {
                egui::Shape::Text(text) if text.galley.job.text.starts_with("SELECT") => {
                    Some(text.galley.job.text.as_str())
                }
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(
            queries,
            [
                "SELECT 'tie-b'",
                "SELECT 'tie-a'",
                "SELECT 'middle'",
                "SELECT 'fast'",
                "SELECT 'λ legacy'"
            ]
        );
        app.database.tools.history_search = "Λ LEGACY".into();
        let output = frame(&mut app, &ctx, TestPanel::History, 240.0, vec![]);
        for badge in ["Failed", "Legacy"] {
            assert!(output.shapes.iter().any(|shape| matches!(&shape.shape, egui::Shape::Text(text) if text.galley.job.text == badge)));
        }
        click(&mut app, &ctx, TestPanel::History, "Load history query");
        assert_eq!(app.database.document.sql, "SELECT 'λ legacy'");
        assert_eq!(app.database.sequence, sequence);
        click(&mut app, &ctx, TestPanel::History, "Delete history entry");
        assert!(
            app.preferences
                .query_history
                .iter()
                .all(|entry| !entry.database.is_empty())
        );
        app.database.document.sql = "SELECT 'saved'".into();
        app.database.tools.bookmark_name = "Example".into();
        app.preferences
            .bookmark("/work/other.db", "Other", "SELECT 'other'");
        click(&mut app, &ctx, TestPanel::Saved, "Save current SQL");
        assert_eq!(
            app.preferences.saved_queries[0].database,
            "/work/current.db"
        );
        assert_eq!(app.preferences.saved_queries[0].name, "Example");
        app.database.document.sql.clear();
        app.database.document.params = "[42]".into();
        click(&mut app, &ctx, TestPanel::Saved, "Load saved query");
        assert_eq!(app.database.document.sql, "SELECT 'saved'");
        assert_eq!(app.database.document.params, "[]");
        assert!(app.database.document.active.is_none());
        click(&mut app, &ctx, TestPanel::Saved, "Delete saved query");
        assert_eq!(app.preferences.saved_queries.len(), 1);
        assert_eq!(app.preferences.saved_queries[0].database, "/work/other.db");
        for (name, mode) in [
            ("Light theme", Mode::Light),
            ("Dark theme", Mode::Dark),
            ("Follow system", Mode::System),
        ] {
            click(&mut app, &ctx, TestPanel::Settings, name);
            assert_eq!(app.preferences.appearance, Some(mode));
        }
    }
    #[test]
    fn heading_clears_only_the_current_database_and_settings_clears_every_database() {
        let (ctx, mut app) = crate::app::test_ui::app();
        for db in ["/work/current.db", "/work/other.db", ""] {
            app.preferences
                .record_query(db, "SELECT 1", Some(10), false);
            app.preferences.bookmark(db, "Example", "SELECT 1");
        }
        app.database.tools.database_scope = "/work/current.db".into();
        // Filters affect the list, but clearing still applies to this entire file.
        app.database.tools.history_search = "no match".into();
        app.database.tools.bookmark_search = "no match".into();
        click(
            &mut app,
            &ctx,
            TestPanel::History,
            "Clear history for current.db",
        );
        click(
            &mut app,
            &ctx,
            TestPanel::Saved,
            "Clear saved queries for current.db",
        );
        assert_eq!(app.preferences.query_history.len(), 2);
        assert_eq!(app.preferences.saved_queries.len(), 2);
        for db in ["/work/other.db", ""] {
            assert!(
                app.preferences
                    .query_history
                    .iter()
                    .any(|q| q.database == db)
            );
            assert!(
                app.preferences
                    .saved_queries
                    .iter()
                    .any(|q| q.database == db)
            );
        }
        for (panel, name) in [
            (TestPanel::History, "Clear history for current.db"),
            (TestPanel::Saved, "Clear saved queries for current.db"),
        ] {
            let output = frame(&mut app, &ctx, panel, 240.0, vec![]);
            assert!(
                crate::app::test_ui::nodes(&output)
                    .iter()
                    .any(|(_, n)| n.label() == Some(name) && n.is_disabled())
            );
        }
        // A missing database scope must never classify legacy entries as local.
        app.database.tools.database_scope.clear();
        click(&mut app, &ctx, TestPanel::History, "Clear history for ");
        click(&mut app, &ctx, TestPanel::Saved, "Clear saved queries for ");
        assert_eq!(app.preferences.query_history.len(), 2);
        assert_eq!(app.preferences.saved_queries.len(), 2);
        assert!(app.database.session.is_none());
        app.settings.page = Page::Queries;
        click(
            &mut app,
            &ctx,
            TestPanel::Settings,
            "Clear history for all databases",
        );
        assert!(app.preferences.query_history.is_empty());
        assert_eq!(app.preferences.saved_queries.len(), 2);
        click(
            &mut app,
            &ctx,
            TestPanel::Settings,
            "Clear saved queries for all databases",
        );
        assert!(app.preferences.saved_queries.is_empty());
        let output = frame(&mut app, &ctx, TestPanel::Settings, 240.0, vec![]);
        for name in [
            "Clear history for all databases",
            "Clear saved queries for all databases",
        ] {
            assert!(
                crate::app::test_ui::nodes(&output)
                    .iter()
                    .any(|(_, n)| n.label() == Some(name) && n.is_disabled())
            );
        }
    }
}
