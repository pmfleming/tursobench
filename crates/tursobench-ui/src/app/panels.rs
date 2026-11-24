//! Compact controls shared by the left navigator's tool panels.
use super::{
    App, View,
    icons::{self, Icon},
};
use eframe::egui;
use tursobench_db::{Diagnostic, Operation};

impl App {
    pub(super) fn saved_queries_ui(&mut self, ui: &mut egui::Ui) {
        if clear_heading(
            ui,
            ("Saved queries", "saved queries"),
            &self.database.tools.database_scope,
            &mut self.preferences.saved_queries,
            |q| &q.database,
        ) {
            self.store.save(&self.preferences);
        }
        ui.horizontal(|ui| self.save_query_ui(ui));
        search(
            ui,
            &mut self.database.tools.bookmark_search,
            "Search saved queries…",
        );
        ui.add_space(4.0);
        let query = self.database.tools.bookmark_search.to_lowercase();
        let mut chosen = None;
        let mut save = false;
        let mut visible = 0;
        let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 2.0);
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
            let (changed, action) = ui
                .push_id(("saved-query", index), |ui| {
                    let legacy = bookmark.database.is_empty();
                    saved_query_row(ui, &mut bookmark.name, legacy, &bookmark.sql)
                })
                .inner;
            save |= changed;
            if let Some(action) = action {
                chosen = Some((index, action));
            }
        }
        ui.spacing_mut().item_spacing.y = spacing;
        if visible == 0 {
            ui.weak("No saved queries");
        }
        save |= self.apply_query_action(
            chosen,
            |app| &mut app.preferences.saved_queries,
            |q| q.sql.clone(),
        );
        if save {
            self.store.save(&self.preferences);
        }
    }

    /// A name field and the button that saves the editor's SQL under it,
    /// or under the SQL's opening when unnamed.
    fn save_query_ui(&mut self, ui: &mut egui::Ui) {
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
    }

    pub(super) fn query_history_ui(&mut self, ui: &mut egui::Ui) {
        if clear_heading(
            ui,
            ("History", "history"),
            &self.database.tools.database_scope,
            &mut self.preferences.query_history,
            |q| &q.database,
        ) {
            self.store.save(&self.preferences);
        }
        let threshold = self.preferences.slow_query_ms;
        ui.horizontal(|ui| {
            let toggles = 2.0 * (28.0 + ui.spacing().item_spacing.x);
            ui.add_sized(
                [(ui.available_width() - toggles).max(40.0), 28.0],
                egui::TextEdit::singleline(&mut self.database.tools.history_search)
                    .hint_text("Search history…"),
            );
            for (selected, icon, name, detail) in [
                (
                    &mut self.database.tools.slow_only,
                    Icon::Filter,
                    "Slow queries only",
                    format!("{threshold} ms or slower · threshold is in Settings"),
                ),
                (
                    &mut self.database.tools.sort_slow,
                    Icon::Down,
                    "Slowest first",
                    "Otherwise most recent first".to_owned(),
                ),
            ] {
                if icons::control(ui, icon, name, "", *selected, false)
                    .on_hover_text(detail)
                    .clicked()
                {
                    *selected = !*selected;
                }
            }
        });
        ui.add_space(4.0);
        let query = self.database.tools.history_search.to_lowercase();
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
            ui.weak(if query.is_empty() && !self.database.tools.slow_only {
                "No history"
            } else {
                "No matching history"
            });
        }
        let mut chosen = None;
        let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 2.0);
        for (index, entry) in entries {
            if let Some(action) = ui
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
                chosen = Some((index, action));
            }
        }
        ui.spacing_mut().item_spacing.y = spacing;
        if self.apply_query_action(
            chosen,
            |app| &mut app.preferences.query_history,
            |h| h.sql.clone(),
        ) {
            self.store.save(&self.preferences);
        }
    }
    /// Load the chosen listed query into an editor, or delete it; true when `list` changed.
    fn apply_query_action<T>(
        &mut self,
        chosen: Option<(usize, QueryAction)>,
        list: impl Fn(&mut Self) -> &mut Vec<T>,
        sql: impl Fn(&T) -> String,
    ) -> bool {
        match chosen {
            Some((index, QueryAction::Load)) => {
                let sql = sql(&list(self)[index]);
                self.load_sql_window(sql);
                false
            }
            Some((index, QueryAction::Delete)) => {
                list(self).remove(index);
                true
            }
            None => false,
        }
    }

    pub(super) fn diagnostics_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Checks");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                icons::hint(
                    ui,
                    Icon::Lock,
                    20.0,
                    "Storage and validation checks only read the database",
                );
            });
        });
        let ready = self.database.ready && !self.database.closing;
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
            group(ui, title, |ui| {
                for &(diagnostic, icon, label) in checks {
                    let response = ui
                        .add_enabled_ui(ready, |ui| {
                            action_row(ui, icon, label, diagnostic.label(), "")
                        })
                        .inner
                        .on_hover_text(format!(
                            "Runs {} · the result opens in the SQL tile",
                            diagnostic.sql()
                        ));
                    if response.clicked() {
                        self.submit(Operation::Diagnostic(diagnostic), View::Diagnostics);
                    }
                }
            });
        }
        // Each of these opens its own window; unlike the checks, some can write.
        group(ui, "Tools", |ui| {
            for (tool, icon, label, name, shortcut, detail) in [
                (
                    Tool::Compare,
                    Icon::Compare,
                    "Compare schemas",
                    "Compare schemas",
                    "F7",
                    "Diff two open databases and draft a migration",
                ),
                (
                    Tool::Dashboard,
                    Icon::Gauge,
                    "PRAGMA dashboard",
                    "PRAGMA dashboard",
                    "",
                    "Effective settings and file sizes",
                ),
                (
                    Tool::Profile,
                    Icon::Profile,
                    "Data profiling",
                    "Data profiling",
                    "",
                    "NULLs, distinct values, ranges and top values per column",
                ),
                (
                    Tool::Transfer,
                    Icon::Transfer,
                    "Transfer & maintenance",
                    "Data transfer and maintenance",
                    "",
                    "Complete export, import, checkpoint and verified backup",
                ),
            ] {
                let enabled = tool == Tool::Compare || self.database.ready;
                let response = ui
                    .add_enabled_ui(enabled, |ui| action_row(ui, icon, label, name, shortcut))
                    .inner
                    .on_hover_text(detail);
                if response.clicked() {
                    match tool {
                        Tool::Compare => self.comparison.open = true,
                        Tool::Dashboard => self.database.tools.admin.open = true,
                        Tool::Profile => self.database.tools.profile.open = true,
                        Tool::Transfer => self.open_transfers(),
                    }
                }
            }
        });
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Tool {
    Compare,
    Dashboard,
    Profile,
    Transfer,
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

/// A list's heading, whose trash icon clears the `entries` that `database`
/// places in this one; true when it did.
fn clear_heading<T>(
    ui: &mut egui::Ui,
    (title, what): (&str, &str),
    scope: &str,
    entries: &mut Vec<T>,
    database: fn(&T) -> &str,
) -> bool {
    let path = std::path::Path::new(scope);
    let name = path
        .file_name()
        .unwrap_or(path.as_os_str())
        .to_string_lossy();
    let ours = |entry: &T| database(entry) == scope;
    let enabled = !scope.is_empty() && entries.iter().any(ours);
    let clear = heading_action(
        ui,
        title,
        Icon::Trash,
        &format!("Clear {what} for {name}"),
        enabled,
    );
    if clear {
        entries.retain(|entry| !ours(entry));
    }
    clear
}

/// A titled run of flat rows, like the navigator's own tree.
fn group(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    caption(ui, title);
    let spacing = std::mem::replace(&mut ui.spacing_mut().item_spacing.y, 2.0);
    contents(ui);
    ui.spacing_mut().item_spacing.y = spacing;
}

pub(super) fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    caption(ui, title);
    card(ui, contents);
}

/// A muted, upper-case title above a group or section.
fn caption(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.label(
        egui::RichText::new(title.to_uppercase())
            .small()
            .strong()
            .color(crate::appearance::Tokens::of(ui.ctx()).muted),
    );
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

/// One line of text that ends in an ellipsis instead of leaving its column.
fn elided(
    ui: &egui::Ui,
    text: &str,
    font: egui::FontId,
    color: egui::Color32,
    width: f32,
) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_owned(), font, color);
    job.wrap = egui::text::TextWrapping::truncate_at_width(width.max(0.0));
    ui.painter().layout_job(job)
}

fn action_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    name: &str,
    shortcut: &str,
) -> egui::Response {
    let response = ui.add_sized(
        [ui.available_width(), 28.0],
        egui::Button::new("").frame_when_inactive(false),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), name));
    let rect = response.rect;
    let color = ui.style().interact(&response).fg_stroke.color;
    let mut right = rect.right() - 8.0;
    if !shortcut.is_empty() {
        let key = ui.painter().layout_no_wrap(
            shortcut.to_owned(),
            egui::TextStyle::Small.resolve(ui.style()),
            crate::appearance::Tokens::of(ui.ctx()).muted,
        );
        right -= key.size().x;
        ui.painter().galley(
            egui::pos2(right, rect.center().y - key.size().y / 2.0),
            key,
            color,
        );
        right -= 8.0;
    }
    let left = rect.left() + 30.0;
    let text = elided(
        ui,
        label,
        egui::TextStyle::Button.resolve(ui.style()),
        color,
        right - left,
    );
    ui.painter().galley(
        egui::pos2(left, rect.center().y - text.size().y / 2.0),
        text,
        color,
    );
    icons::paint(
        ui.painter(),
        egui::Rect::from_center_size(
            egui::pos2(rect.left() + 14.0, rect.center().y),
            egui::vec2(16.0, 16.0),
        ),
        icon,
        color,
    );
    response.on_hover_text(if shortcut.is_empty() {
        name.to_owned()
    } else {
        format!("{name} · {shortcut}")
    })
}

/// One saved query: its editable name over a SQL preview, with load and
/// delete actions. Returns whether the name changed and any action.
fn saved_query_row(
    ui: &mut egui::Ui,
    name: &mut String,
    legacy: bool,
    sql: &str,
) -> (bool, Option<QueryAction>) {
    list_row(ui, None, |ui, active| {
        let name = ui.add(
            egui::TextEdit::singleline(name)
                .char_limit(64)
                .desired_width(ui.available_width())
                .margin(egui::Margin::ZERO)
                .frame(egui::Frame::NONE),
        );
        name.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::TextInput, ui.is_enabled(), "Rename saved query")
        });
        let action = query_actions(ui, active, "Load saved query", "Delete saved query", |ui| {
            if legacy {
                ui.small("Legacy")
                    .on_hover_text("Saved before database-scoped queries");
            }
            ui.add(
                egui::Label::new(
                    egui::RichText::new(sql_preview(sql))
                        .monospace()
                        .small()
                        .weak(),
                )
                .truncate(),
            )
            .on_hover_text(sql);
        });
        (name.changed(), action)
    })
}

/// A flat two-line entry. The closure learns whether the pointer is over the
/// row so per-row actions can recede until they are within reach.
fn list_row<R>(
    ui: &mut egui::Ui,
    edge: Option<egui::Color32>,
    contents: impl FnOnce(&mut egui::Ui, bool) -> R,
) -> R {
    const HEIGHT: f32 = 54.0;
    let expected =
        egui::Rect::from_min_size(ui.cursor().min, egui::vec2(ui.available_width(), HEIGHT));
    let active = ui.rect_contains_pointer(expected);
    let fill = ui.painter().add(egui::Shape::Noop);
    let frame = egui::Frame::new()
        .inner_margin(egui::Margin {
            left: 8,
            right: 2,
            top: 3,
            bottom: 3,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            contents(ui, active)
        });
    let rect = frame.response.rect;
    if active {
        ui.painter().set(
            fill,
            egui::Shape::rect_filled(rect, 6.0, ui.visuals().faint_bg_color),
        );
    }
    if let Some(color) = edge {
        ui.painter().rect_filled(
            egui::Rect::from_min_max(
                rect.left_top() + egui::vec2(0.0, 5.0),
                egui::pos2(rect.left() + 2.0, rect.bottom() - 5.0),
            ),
            1.0,
            color,
        );
    }
    frame.inner
}

#[derive(Clone, Copy)]
enum QueryAction {
    Load,
    Delete,
}

/// The second line of a query row: `leading` on the left, load and delete on
/// the right. The actions stay visible but faint until the row is active.
fn query_actions(
    ui: &mut egui::Ui,
    active: bool,
    load: &str,
    delete: &str,
    leading: impl FnOnce(&mut egui::Ui),
) -> Option<QueryAction> {
    ui.horizontal(|ui| {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let action = ui
                .scope(|ui| {
                    if !active {
                        ui.visuals_mut().widgets.inactive.fg_stroke.color =
                            crate::appearance::Tokens::of(ui.ctx()).faint;
                    }
                    let delete = icons::button(ui, Icon::Trash, delete, "").clicked();
                    let load = icons::button(ui, Icon::Sql, load, "")
                        .on_hover_text("Into the SQL editor, without running it")
                        .clicked();
                    if load {
                        Some(QueryAction::Load)
                    } else if delete {
                        Some(QueryAction::Delete)
                    } else {
                        None
                    }
                })
                .inner;
            ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                leading(ui);
            });
            action
        })
        .inner
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
        let edge = if self.failed {
            Some(ui.visuals().error_fg_color)
        } else if self.slow {
            Some(ui.visuals().warn_fg_color)
        } else {
            None
        };
        list_row(ui, edge, |ui, active| {
            // SQL reads from the left edge, like the editor it loads into.
            let preview = ui
                .allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), 20.0),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.spacing_mut().button_padding = egui::Vec2::ZERO;
                        ui.spacing_mut().interact_size.y = 20.0;
                        ui.add(
                            egui::Button::new(
                                egui::RichText::new(sql_preview(self.sql)).monospace(),
                            )
                            .truncate()
                            .frame(false),
                        )
                    },
                )
                .inner
                .on_hover_text(format!(
                    "{}\n\nClick to load into the SQL editor without running it",
                    self.sql
                ));
            let action = preview.clicked().then_some(QueryAction::Load);
            query_actions(
                ui,
                active,
                "Load history query",
                "Delete history entry",
                |ui| {
                    ui.small(self.elapsed_ms.map_or("—".into(), |ms| format!("{ms} ms")));
                    for (visible, label, color, detail) in [
                        (
                            self.failed,
                            "Failed",
                            ui.visuals().error_fg_color,
                            "The statement returned an error",
                        ),
                        (
                            self.slow,
                            "Slow",
                            ui.visuals().warn_fg_color,
                            "At or above the slow-query threshold",
                        ),
                        (
                            self.legacy,
                            "Legacy",
                            ui.visuals().weak_text_color(),
                            "Recorded before history was kept per database",
                        ),
                    ] {
                        if visible {
                            ui.label(egui::RichText::new(label).small().color(color))
                                .on_hover_text(detail);
                        }
                    }
                },
            )
            .or(action)
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
    use crate::app::settings::{Page, assert_controls_fit};
    use crate::app::test_ui::{self, control, nodes, pointer};
    use crate::appearance::Mode;
    // Render individual pages without requiring an open database.
    #[derive(Clone, Copy, Debug, PartialEq)]
    enum TestPanel {
        Settings,
        History,
        Saved,
        Diagnostics,
    }
    use TestPanel::{Diagnostics, History, Saved, Settings};

    fn frame(
        app: &mut App,
        ctx: &egui::Context,
        panel: TestPanel,
        width: f32,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        test_ui::render(ctx, egui::vec2(width, 1400.0), events, |ui| match panel {
            Settings => app.settings_ui(ui),
            History => app.query_history_ui(ui),
            Saved => app.saved_queries_ui(ui),
            Diagnostics => app.diagnostics_ui(ui),
        })
    }

    fn click(app: &mut App, ctx: &egui::Context, panel: TestPanel, name: &str) {
        press(app, ctx, (panel, name), |rect| rect.center());
    }

    fn assert_disabled(output: &egui::FullOutput, name: &str) {
        let disabled =
            |(_, n): &(_, egui::accesskit::Node)| n.label() == Some(name) && n.is_disabled();
        assert!(nodes(output).iter().any(disabled), "{name}");
    }

    /// Press and release `name` at the point `at` picks in its bounds.
    fn press(
        app: &mut App,
        ctx: &egui::Context,
        (panel, name): (TestPanel, &str),
        at: impl Fn(egui::Rect) -> egui::Pos2,
    ) {
        let output = frame(app, ctx, panel, 240.0, vec![]);
        let pos = at(control(&output, name));
        for pressed in [true, false] {
            frame(app, ctx, panel, 240.0, pointer(pos, pressed));
        }
    }

    #[test]
    fn appearance_controls_apply_size_without_drift() {
        let (ctx, mut app) = test_ui::app();
        let output = frame(&mut app, &ctx, Settings, 240.0, vec![]);
        for name in [
            "Decrease font size",
            "Increase font size",
            "Reset font size",
        ] {
            control(&output, name).center();
        }
        click(&mut app, &ctx, Settings, "Increase font size");
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
        click(&mut app, &ctx, Settings, "Decrease font size");
        assert_eq!(app.preferences.font_size, 14);
        click(&mut app, &ctx, Settings, "Increase font size");
        click(&mut app, &ctx, Settings, "Reset font size");
        assert_eq!(app.preferences.font_size, 14);
        assert_eq!(crate::appearance::monospace(&ctx, 12.0).size, 12.0);
    }

    #[test]
    fn resource_slider_updates_admission_and_reset_restores_system_choice() {
        let (ctx, mut app) = test_ui::app();
        let panel = Settings;
        app.settings.page = Page::Resources;
        let slider = (panel, "Maximum open databases");
        assert_eq!(app.memory.limits(&app.preferences).database_limit, 8);
        press(&mut app, &ctx, slider, |s| {
            egui::pos2(s.right() - 1.0, s.center().y)
        });
        assert_eq!(app.preferences.database_limit, Some(32));
        assert_eq!(app.preferences.memory_budget_mib, Some(16 * 1024));
        assert_eq!(app.memory.limits(&app.preferences).database_limit, 32);
        press(&mut app, &ctx, slider, |s| {
            egui::pos2(s.left() + s.width() / 8.0, s.center().y)
        });
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
            let (ctx, mut app) = test_ui::app();
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
                for panel in [History, Saved, Diagnostics] {
                    frame(&mut app, &ctx, panel, width, vec![]);
                    let output = frame(&mut app, &ctx, panel, width, vec![]);
                    assert_controls_fit(&output, width, &format!("{panel:?}, width {width}"));
                    if panel == Diagnostics {
                        for diagnostic in Diagnostic::ALL {
                            control(&output, diagnostic.label());
                        }
                        // The four tool windows are rows too, not a separate button stack.
                        for tool in [
                            "Compare schemas",
                            "PRAGMA dashboard",
                            "Data profiling",
                            "Data transfer and maintenance",
                        ] {
                            control(&output, tool);
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
        let (ctx, mut app) = test_ui::app();
        app.database.tools.database_scope = "/work/current.db".into();
        for (db, sql, ms) in [
            ("/work/other.db", "SELECT 'other'", 900),
            ("/work/current.db", "SELECT 'fast'", 10),
            ("/work/current.db", "SELECT 'slow'", 900),
        ] {
            app.preferences.record_query(db, sql, Some(ms), false);
        }
        app.database.document.params = "[42]".into();
        let sequence = app.database.sequence;
        click(&mut app, &ctx, History, "Slow queries only");
        assert!(app.database.tools.slow_only);
        let output = frame(&mut app, &ctx, History, 240.0, vec![]);
        assert_eq!(
            nodes(&output)
                .iter()
                .filter(|(_, n)| n.label() == Some("Load history query"))
                .count(),
            1
        );
        click(&mut app, &ctx, History, "Load history query");
        assert_eq!(app.database.document.sql, "SELECT 'slow'");
        assert_eq!(app.database.document.params, "[]");
        assert_eq!(app.database.sequence, sequence);
        assert!(app.database.document.active.is_none());
        click(&mut app, &ctx, History, "Delete history entry");
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
        click(&mut app, &ctx, History, "Slow queries only");
        click(&mut app, &ctx, History, "Slowest first");
        let output = frame(&mut app, &ctx, History, 240.0, vec![]);
        let queries: Vec<_> = test_ui::texts(&output)
            .map(|t| t.galley.job.text.as_str())
            .filter(|text| text.starts_with("SELECT"))
            .collect();
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
        let output = frame(&mut app, &ctx, History, 240.0, vec![]);
        for badge in ["Failed", "Legacy"] {
            let mut texts = test_ui::texts(&output);
            assert!(texts.any(|t| t.galley.job.text == badge), "{badge}");
        }
        click(&mut app, &ctx, History, "Load history query");
        assert_eq!(app.database.document.sql, "SELECT 'λ legacy'");
        assert_eq!(app.database.sequence, sequence);
        click(&mut app, &ctx, History, "Delete history entry");
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
        click(&mut app, &ctx, Saved, "Save current SQL");
        assert_eq!(
            app.preferences.saved_queries[0].database,
            "/work/current.db"
        );
        assert_eq!(app.preferences.saved_queries[0].name, "Example");
        app.database.document.sql.clear();
        app.database.document.params = "[42]".into();
        click(&mut app, &ctx, Saved, "Load saved query");
        assert_eq!(app.database.document.sql, "SELECT 'saved'");
        assert_eq!(app.database.document.params, "[]");
        assert!(app.database.document.active.is_none());
        click(&mut app, &ctx, Saved, "Delete saved query");
        assert_eq!(app.preferences.saved_queries.len(), 1);
        assert_eq!(app.preferences.saved_queries[0].database, "/work/other.db");
        for (name, mode) in [
            ("Light theme", Mode::Light),
            ("Dark theme", Mode::Dark),
            ("Follow system", Mode::System),
        ] {
            click(&mut app, &ctx, Settings, name);
            assert_eq!(app.preferences.appearance, Some(mode));
        }
    }
    #[test]
    fn heading_clears_only_the_current_database_and_settings_clears_every_database() {
        let (ctx, mut app) = test_ui::app();
        for db in ["/work/current.db", "/work/other.db", ""] {
            app.preferences
                .record_query(db, "SELECT 1", Some(10), false);
            app.preferences.bookmark(db, "Example", "SELECT 1");
        }
        let lengths = |app: &App| {
            let preferences = &app.preferences;
            (
                preferences.query_history.len(),
                preferences.saved_queries.len(),
            )
        };
        app.database.tools.database_scope = "/work/current.db".into();
        // Filters affect the list, but clearing still applies to this entire file.
        app.database.tools.history_search = "no match".into();
        app.database.tools.bookmark_search = "no match".into();
        let headings = [
            (History, "Clear history for current.db"),
            (Saved, "Clear saved queries for current.db"),
        ];
        for (panel, name) in headings {
            click(&mut app, &ctx, panel, name);
        }
        assert_eq!(lengths(&app), (2, 2));
        for db in ["/work/other.db", ""] {
            let preferences = &app.preferences;
            assert!(preferences.query_history.iter().any(|q| q.database == db));
            assert!(preferences.saved_queries.iter().any(|q| q.database == db));
        }
        for (panel, name) in headings {
            assert_disabled(&frame(&mut app, &ctx, panel, 240.0, vec![]), name);
        }
        // A missing database scope must never classify legacy entries as local.
        app.database.tools.database_scope.clear();
        click(&mut app, &ctx, History, "Clear history for ");
        click(&mut app, &ctx, Saved, "Clear saved queries for ");
        assert_eq!(lengths(&app), (2, 2));
        assert!(app.database.session.is_none());
        app.settings.page = Page::Queries;
        let everywhere = [
            "Clear history for all databases",
            "Clear saved queries for all databases",
        ];
        for (name, left) in everywhere.into_iter().zip([(0, 2), (0, 0)]) {
            click(&mut app, &ctx, Settings, name);
            assert_eq!(lengths(&app), left, "{name}");
        }
        let output = frame(&mut app, &ctx, Settings, 240.0, vec![]);
        for name in everywhere {
            assert_disabled(&output, name);
        }
    }
}
