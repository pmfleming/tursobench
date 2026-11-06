use super::{
    App,
    icons::{self, Icon},
    sessions::DatabaseWorkspace,
};
use eframe::egui;
use std::path::Path;
use tursobench_core::TransactionState;

#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum Scope {
    #[default]
    AllOpen,
    ThisDatabase,
    Databases,
}

#[derive(Default)]
pub(super) struct Search {
    pub query: String,
    pub scope: Scope,
    pub index: usize,
    pub focus: bool,
    activate: bool,
}

enum Target {
    Database(u64),
    Object(u64, super::commands::Command),
    Available(std::path::PathBuf),
}
struct SearchResult {
    group: String,
    label: String,
    target: Target,
}

impl App {
    pub(super) fn database_navigation_keys(&mut self, ctx: &egui::Context) {
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::P))
            || !ctx.egui_wants_keyboard_input()
                && ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Slash))
        {
            self.sidebar_search.focus = true;
            self.database.ui.sidebar = super::shell::ToolPanel::Schema;
        }
    }

    pub(super) fn sidebar_search_ui(&mut self, ui: &mut egui::Ui) {
        self.sidebar_search.activate = ui
            .memory(|m| m.has_focus(egui::Id::unique("sidebar-search")))
            && ui.input(|i| i.key_pressed(egui::Key::Enter));
        ui.horizontal(|ui| {
            let width = (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
            let response = ui.add_sized(
                [width, 28.0],
                egui::TextEdit::singleline(&mut self.sidebar_search.query)
                    .id(egui::Id::unique("sidebar-search"))
                    .hint_text("Find in databases…"),
            );
            if std::mem::take(&mut self.sidebar_search.focus) {
                response.request_focus();
            }
            if response.changed() {
                self.sidebar_search.index = 0;
            }
            if response.has_focus() {
                self.database.tools.tree_focus = Some(response.id);
            }
            if icons::button(ui, Icon::Refresh, "Refresh schema", "F5").clicked() {
                self.refresh_catalog();
                self.load_diagram();
                for database in self.parked.values_mut() {
                    if self.sidebar_search.scope == Scope::AllOpen {
                        database.request_diagram();
                    }
                }
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            for (scope, label) in [
                (Scope::AllOpen, "All open"),
                (Scope::ThisDatabase, "This database"),
                (Scope::Databases, "Databases"),
            ] {
                if ui
                    .selectable_value(
                        &mut self.sidebar_search.scope,
                        scope,
                        egui::RichText::new(label).size(10.5),
                    )
                    .changed()
                {
                    self.sidebar_search.index = 0;
                }
            }
        });
    }

    fn sidebar_matches(&self) -> Vec<SearchResult> {
        let query = self.sidebar_search.query.trim();
        let scope = self.sidebar_search.scope;
        let mut results = Vec::new();
        for database in std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| {
                d.session.is_some() && (scope != Scope::ThisDatabase || d.id == self.database.id)
            })
        {
            let file = Path::new(&database.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if scope != Scope::ThisDatabase && super::commands::fuzzy(&database.path, query) {
                results.push(SearchResult {
                    group: file.clone(),
                    label: file.clone(),
                    target: Target::Database(database.id),
                });
            }
            if scope != Scope::Databases {
                let graph = database
                    .ui
                    .diagram
                    .data
                    .as_ref()
                    .filter(|_| !database.ui.diagram.stale);
                for object in super::commands::objects(&database.schema, graph) {
                    if super::commands::fuzzy(&object.label(), query) {
                        results.push(SearchResult {
                            group: file.clone(),
                            label: object.label(),
                            target: Target::Object(database.id, object.command()),
                        });
                    }
                }
            }
        }
        if scope != Scope::ThisDatabase {
            for path in self.available_database_paths(query) {
                results.push(SearchResult {
                    group: "Not open".into(),
                    label: path.display().to_string(),
                    target: Target::Available(path),
                });
            }
        }
        results
    }

    pub(super) fn sidebar_search_results_ui(&mut self, ui: &mut egui::Ui) {
        if self.sidebar_search.scope != Scope::Databases {
            let active = self.database.id;
            for database in std::iter::once(&mut self.database).chain(self.parked.values_mut()) {
                if (self.sidebar_search.scope == Scope::AllOpen || database.id == active)
                    && (database.ui.diagram.data.is_none() || database.ui.diagram.stale)
                    && database.ui.diagram.error.is_empty()
                {
                    database.request_diagram();
                }
            }
        }
        let mut results = self.sidebar_matches();
        let total = results.len();
        results.truncate(100);
        let focused = ui.memory(|m| m.has_focus(egui::Id::unique("sidebar-search")));
        let mut chosen = self
            .sidebar_search
            .activate
            .then_some(self.sidebar_search.index);
        if focused {
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                self.sidebar_search.index += 1;
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                self.sidebar_search.index = self.sidebar_search.index.saturating_sub(1);
                ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
            }
            if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                chosen = Some(self.sidebar_search.index);
            }
        }
        self.sidebar_search.index = self
            .sidebar_search
            .index
            .min(results.len().saturating_sub(1));
        egui::ScrollArea::vertical()
            .id_salt("sidebar-search-results")
            .show(ui, |ui| {
                let mut group = None;
                for (index, result) in results.iter().enumerate() {
                    if group != Some(&result.group) {
                        ui.small(&result.group);
                        group = Some(&result.group);
                    }
                    let enabled =
                        self.picker.is_none() && !self.database.confirm_close && !self.quit;
                    let response = ui.add_enabled(
                        enabled,
                        egui::Button::new(&result.label)
                            .truncate()
                            .selected(focused && index == self.sidebar_search.index)
                            .min_size(egui::vec2(ui.available_width(), 24.0)),
                    );
                    if focused && index == self.sidebar_search.index {
                        response.scroll_to_me(None);
                    }
                    if response.on_hover_text(&result.label).clicked() {
                        chosen = Some(index);
                    }
                }
                if results.is_empty() {
                    ui.weak("No matches. Try another scope or query.");
                }
                if total > 100 {
                    ui.weak("First 100 matches; refine the search.");
                }
                if self.sidebar_search.scope != Scope::Databases {
                    for database in std::iter::once(&self.database)
                        .chain(self.parked.values())
                        .filter(|d| {
                            d.session.is_some()
                                && (self.sidebar_search.scope == Scope::AllOpen
                                    || d.id == self.database.id)
                        })
                    {
                        if database.ui.diagram.ticket.is_some() {
                            ui.weak(format!("{}: columns still loading…", database.path));
                        }
                        if !database.ui.diagram.error.is_empty() {
                            ui.colored_label(
                                ui.visuals().warn_fg_color,
                                &database.ui.diagram.error,
                            );
                        }
                        if database.ui.diagram.data.as_ref().is_some_and(|g| g.limited) {
                            ui.weak("Column metadata limited; use SQL for omitted objects.");
                        }
                    }
                }
            });
        if let Some(index) = chosen.filter(|&i| i < results.len()) {
            if self.picker.is_some() || self.database.confirm_close || self.quit {
                return;
            }
            match results.swap_remove(index).target {
                Target::Database(id) => self.select_workspace(id),
                Target::Object(id, command) => {
                    self.select_workspace(id);
                    if self.database.id == id {
                        self.execute_command(ui.ctx(), command);
                    }
                }
                Target::Available(path) => self.request_database(ui.ctx(), path),
            }
            self.sidebar_search.query.clear();
            self.sidebar_search.index = 0;
        }
    }
}

/// A small presentation snapshot; rendering never swaps or copies result buffers.
struct DatabaseRow {
    id: u64,
    path: String,
    writable: bool,
    transaction: TransactionState,
    drafts: usize,
    closing: bool,
}

impl From<&DatabaseWorkspace> for DatabaseRow {
    fn from(database: &DatabaseWorkspace) -> Self {
        Self {
            id: database.id,
            path: database.path.clone(),
            writable: database.writable,
            transaction: database.transaction,
            drafts: database
                .documents
                .values()
                .chain(std::iter::once(&database.document))
                .filter(|document| document.has_drafts())
                .count(),
            closing: database.closing,
        }
    }
}

impl App {
    pub(super) fn open_databases_ui(&mut self, ui: &mut egui::Ui) {
        let mut rows: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|database| database.session.is_some())
            .map(DatabaseRow::from)
            .collect();
        rows.sort_by_key(|row| row.id);
        ui.horizontal(|ui| {
            ui.small("OPEN DATABASES");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small(format!("{}/{}", rows.len(), super::sessions::MAX_DATABASES))
                    .on_hover_text(
                        "Up to four database workspaces; access review reserves a slot.",
                    );
            });
        });
        let enabled = self.picker.is_none() && !self.database.confirm_close && !self.quit;
        let mut select = None;
        let mut close = None;
        for row in rows {
            ui.push_id(("database-row", row.id), |ui| {
                ui.horizontal(|ui| {
                    let width =
                        (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
                    let name = Path::new(&row.path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let detail = if row.closing {
                        "Closing…".into()
                    } else if row.transaction != TransactionState::Autocommit || row.drafts > 0 {
                        format!(
                            "{}{}{}",
                            if row.transaction == TransactionState::Unknown {
                                "Unknown transaction"
                            } else if row.transaction == TransactionState::Active {
                                "Transaction"
                            } else {
                                ""
                            },
                            if row.transaction != TransactionState::Autocommit && row.drafts > 0 {
                                " · "
                            } else {
                                ""
                            },
                            if row.drafts > 0 {
                                format!("{} staged documents", row.drafts)
                            } else {
                                String::new()
                            }
                        )
                    } else {
                        Path::new(&row.path)
                            .parent()
                            .unwrap_or(Path::new("."))
                            .display()
                            .to_string()
                    };
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            ui.add_sized(
                                [width, 44.0],
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  {}\n{}",
                                        name,
                                        if row.writable { "RW" } else { "RO" },
                                        detail
                                    ))
                                    .size(12.0),
                                )
                                .selected(row.id == self.database.id)
                                .truncate(),
                            )
                        })
                        .inner;
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            response.enabled(),
                            row.id == self.database.id,
                            format!("Select database: {}", row.path),
                        )
                    });
                    if response.on_hover_text(&row.path).clicked() {
                        select = Some(row.id);
                    }
                    ui.add_enabled_ui(enabled && !row.closing, |ui| {
                        if icons::button(
                            ui,
                            Icon::Close,
                            &format!("Close database: {}", row.path),
                            "",
                        )
                        .clicked()
                        {
                            close = Some(row.id);
                        }
                    });
                });
            });
        }
        if let Some(id) = select {
            self.select_workspace(id);
        }
        if let Some(id) = close {
            self.select_workspace(id);
            if self.database.id == id {
                self.request_close();
            }
        }
    }
}
