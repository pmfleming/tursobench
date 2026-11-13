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
    pub navigating: bool,
    pub activate: bool,
    pub key_delta: Option<isize>,
    pub reveal: bool,
    pub available_highlight: Option<std::path::PathBuf>,
    pub available_collapsed: bool,
    pub tree_highlight: Option<(String, Option<String>)>,
}

enum Target {
    Database(u64),
    Object(u64, super::commands::Command),
    Available(usize),
}
struct SearchResult {
    group: String,
    label: String,
    target: Target,
    more: usize,
}

impl App {
    pub(super) fn database_navigation_keys(&mut self, ctx: &egui::Context) {
        if self.database.ui.palette_open {
            // The palette's footer offers Ctrl P, which hands over to the
            // sidebar search. Switching database would leave the palette
            // open over a database other than the one it was opened for.
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::P)) {
                self.database.ui.palette_open = false;
                self.sidebar_search.focus = true;
                self.database.ui.sidebar = super::shell::ToolPanel::Schema;
            }
            return;
        }
        for (key, direction) in [(egui::Key::PageDown, 1), (egui::Key::PageUp, -1)] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, key)) {
                self.cycle_database(direction);
            }
        }
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
        self.sidebar_search.key_delta = None;
        if ui.memory(|m| m.has_focus(egui::Id::unique("sidebar-search"))) {
            for (key, delta) in [
                (egui::Key::ArrowDown, 1),
                (egui::Key::ArrowUp, -1),
                (egui::Key::PageDown, 5),
                (egui::Key::PageUp, -5),
            ] {
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                    self.sidebar_search.key_delta = Some(delta);
                    ui.memory_mut(|m| m.move_focus(egui::FocusDirection::None));
                }
            }
        }
        ui.horizontal_wrapped(|ui| {
            ui.small("SEARCH").on_hover_text("Search databases");
            ui.spacing_mut().item_spacing.x = 3.0;
            for (scope, icon, tooltip) in [
                (
                    Scope::AllOpen,
                    Icon::Copy,
                    "All open: schema objects and available database files",
                ),
                (
                    Scope::ThisDatabase,
                    Icon::Database,
                    "This database: tables, columns and schema objects",
                ),
                (
                    Scope::Databases,
                    Icon::Search,
                    "Databases: names and paths, including unopened files",
                ),
            ] {
                if icons::control(
                    ui,
                    icon,
                    tooltip,
                    "",
                    self.sidebar_search.scope == scope,
                    false,
                )
                .clicked()
                {
                    self.sidebar_search.scope = scope;
                    self.sidebar_search.index = 0;
                }
            }
        });
        let response = ui.add_sized(
            [ui.available_width(), 28.0],
            egui::TextEdit::singleline(&mut self.sidebar_search.query)
                .id(egui::Id::unique("sidebar-search"))
                .event_filter(egui::EventFilter {
                    escape: true,
                    vertical_arrows: true,
                    horizontal_arrows: true,
                    tab: false,
                })
                .hint_text(match self.sidebar_search.scope {
                    Scope::AllOpen => "Find across databases…",
                    Scope::ThisDatabase => "Find tables, columns…",
                    Scope::Databases => "Find database names or paths…",
                }),
        );
        if std::mem::take(&mut self.sidebar_search.focus) {
            response.request_focus();
        }
        if response.changed() {
            self.sidebar_search.index = 0;
            self.sidebar_search.navigating = false;
        }
        if response.has_focus() {
            self.database.tools.tree_focus = Some(response.id);
        }
        #[cfg(test)]
        {
            self.databases.search_rect = Some(response.rect);
        }
        ui.separator();
    }

    pub(super) fn sidebar_selection_keys(
        &mut self,
        ui: &mut egui::Ui,
        count: usize,
    ) -> Option<usize> {
        if !ui.memory(|m| m.has_focus(egui::Id::unique("sidebar-search")))
            && !self.sidebar_search.activate
        {
            return None;
        }
        let results_mode = !self.sidebar_search.query.trim().is_empty();
        if let Some(delta) = self.sidebar_search.key_delta.take() {
            self.sidebar_search.reveal = true;
            if results_mode {
                self.sidebar_search.index = self.sidebar_search.index.saturating_add_signed(delta);
                self.sidebar_search.navigating = true;
            } else if delta < 0 && self.sidebar_search.index == 0 {
                self.sidebar_search.navigating = false;
            } else {
                self.sidebar_search.index = if self.sidebar_search.navigating {
                    self.sidebar_search.index.saturating_add_signed(delta)
                } else {
                    0
                };
                self.sidebar_search.navigating = true;
            }
        }
        self.sidebar_search.index = self.sidebar_search.index.min(count.saturating_sub(1));
        let enter = ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
            || self.sidebar_search.activate;
        let chosen = (enter && (results_mode || self.sidebar_search.navigating))
            .then_some(self.sidebar_search.index);
        chosen.filter(|&i| i < count)
    }

    pub(super) fn begin_database_browse(&mut self, ctx: &egui::Context) {
        if self.new_workspace(ctx) {
            self.sidebar_search.query.clear();
            self.browse_file(ctx);
        }
    }

    fn sidebar_matches(&self) -> Vec<SearchResult> {
        let query = self.sidebar_search.query.trim();
        let scope = self.sidebar_search.scope;
        let mut results = Vec::new();
        for database in std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| d.retained() && (scope != Scope::ThisDatabase || d.id == self.database.id))
        {
            let file = Path::new(&database.path)
                .file_name()
                .unwrap_or_default()
                .to_string_lossy()
                .into_owned();
            if scope != Scope::ThisDatabase
                && query
                    .to_lowercase()
                    .split_whitespace()
                    .all(|word| database.path.to_lowercase().contains(word))
            {
                results.push(SearchResult {
                    group: file.clone(),
                    label: file.clone(),
                    target: Target::Database(database.id),
                    more: 0,
                });
            }
            if scope != Scope::Databases {
                let graph = database
                    .ui
                    .diagram
                    .data
                    .as_ref()
                    .filter(|_| !database.ui.diagram.stale);
                let objects: Vec<_> = super::commands::objects(&database.schema, graph)
                    .filter(|object| {
                        if scope == Scope::ThisDatabase {
                            object.matches_name(query)
                        } else {
                            super::commands::fuzzy(&object.label(), query)
                        }
                    })
                    .collect();
                let limit = if scope == Scope::AllOpen { 6 } else { 100 };
                let extra = objects.len().saturating_sub(limit);
                for (index, object) in objects.into_iter().take(limit).enumerate() {
                    results.push(SearchResult {
                        group: file.clone(),
                        label: object.label(),
                        target: Target::Object(database.id, object.command()),
                        more: if index + 1 == limit { extra } else { 0 },
                    });
                }
            }
        }
        if scope != Scope::ThisDatabase {
            for (index, path) in self.available_search_results(query) {
                results.push(SearchResult {
                    group: "Not open".into(),
                    label: path,
                    target: Target::Available(index),
                    more: 0,
                });
            }
        }
        results
    }

    fn sidebar_target_enabled(&self, target: &Target) -> bool {
        self.picker.is_none()
            && !self.database.confirm_close
            && !self.quit
            && (!matches!(target, Target::Available(_)) || self.can_request_database())
    }
    fn sidebar_result_ui(
        &self,
        ui: &mut egui::Ui,
        result: &SearchResult,
        selected: bool,
    ) -> (egui::Response, Option<usize>) {
        let enabled = self.sidebar_target_enabled(&result.target);
        if let Target::Available(entry) = result.target {
            let (response, forget) = ui
                .push_id(("available-search", entry), |ui| {
                    self.available_result_ui(ui, entry, selected, enabled)
                })
                .inner;
            (response, forget.then_some(entry))
        } else {
            (
                ui.add_enabled(
                    enabled,
                    egui::Button::new(&result.label)
                        .truncate()
                        .selected(selected)
                        .min_size(egui::vec2(ui.available_width(), 24.0)),
                ),
                None,
            )
        }
    }
    fn sidebar_metadata_status(&self, ui: &mut egui::Ui) {
        if self.sidebar_search.scope != Scope::Databases {
            for database in std::iter::once(&self.database)
                .chain(self.parked.values())
                .filter(|d| {
                    d.session.is_some()
                        && (self.sidebar_search.scope == Scope::AllOpen || d.id == self.database.id)
                })
            {
                if database.ui.diagram.ticket.is_some() {
                    ui.weak(format!("{}: columns still loading…", database.path));
                }
                if !database.ui.diagram.error.is_empty() {
                    ui.colored_label(ui.visuals().warn_fg_color, &database.ui.diagram.error);
                }
                if database.ui.diagram.data.as_ref().is_some_and(|g| g.limited) {
                    ui.weak("Column metadata limited; use SQL for omitted objects.");
                }
            }
        }
    }
    fn select_sidebar_target(&mut self, ctx: &egui::Context, target: Target) {
        if !self.sidebar_target_enabled(&target) {
            return;
        }
        match target {
            Target::Database(id) => self.select_workspace(id),
            Target::Object(id, command) => {
                self.select_workspace(id);
                if self.database.id == id {
                    self.execute_command(ctx, command);
                }
            }
            Target::Available(index) => self.choose_available_result(ctx, index),
        }
        self.sidebar_search.query.clear();
        self.sidebar_search.index = 0;
        self.sidebar_search.navigating = false;
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
        let mut chosen = self.sidebar_selection_keys(ui, results.len());
        let mut forgotten = None;
        let tree_mode = self.sidebar_search.scope == Scope::ThisDatabase;
        self.sidebar_search.tree_highlight = if tree_mode && focused {
            results
                .get(self.sidebar_search.index)
                .and_then(|r| match &r.target {
                    Target::Object(_, super::commands::Command::Table(name, column)) => {
                        Some((name.clone(), column.clone()))
                    }
                    Target::Object(_, super::commands::Command::Definition(name)) => {
                        Some((name.clone(), None))
                    }
                    _ => None,
                })
        } else {
            None
        };
        egui::ScrollArea::vertical()
            .id_salt("sidebar-search-results")
            .show(ui, |ui| {
                if tree_mode {
                    self.schema_tree_ui(ui);
                }
                let mut group = None;
                for (index, result) in results.iter().enumerate().filter(|_| !tree_mode) {
                    if group != Some(&result.group) {
                        ui.small(&result.group);
                        group = Some(&result.group);
                    }
                    let (response, forget) = self.sidebar_result_ui(
                        ui,
                        result,
                        focused && index == self.sidebar_search.index,
                    );
                    forgotten = forget.or(forgotten);
                    if focused && index == self.sidebar_search.index {
                        response.scroll_to_me(None);
                    }
                    if response.on_hover_text(&result.label).clicked() {
                        chosen = Some(index);
                    }
                    if result.more > 0 {
                        ui.weak(format!(
                            "+ {} more in this database; refine the query",
                            result.more
                        ));
                    }
                }
                if results.is_empty() {
                    ui.weak("No matches. Try another scope or query.");
                }
                if total > 100 {
                    ui.weak("First 100 matches; refine the search.");
                }
                self.sidebar_metadata_status(ui);
            });
        if let Some(entry) = forgotten {
            self.forget_available_result(entry);
            return;
        }
        if let Some(index) = chosen.filter(|&i| i < results.len()) {
            self.select_sidebar_target(ui.ctx(), results.swap_remove(index).target);
        }
    }
}

impl App {
    fn occupied_database_slots(&self) -> usize {
        std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| {
                d.retained()
                    || d.id == self.database.id
                        && !d.path.is_empty()
                        && self.database_admission_available()
            })
            .count()
    }

    fn inline_close_confirmation_ui(&mut self, ui: &mut egui::Ui) {
        let color = ui.visuals().warn_fg_color;
        egui::Frame::new().fill(color.gamma_multiply(0.08))
            .stroke(egui::Stroke::new(1.0, color.gamma_multiply(0.4)))
            .corner_radius(6).inner_margin(8.0).show(ui, |ui| {
                let filename = Path::new(&self.database.path).file_name().unwrap_or_default().to_string_lossy();
                ui.add(egui::Label::new(egui::RichText::new(format!("Close {filename}?")).strong()).wrap());
                // Put the actions before the explanation so they remain reachable at
                // large font sizes and in the minimum-width sidebar.
                ui.horizontal_wrapped(|ui| {
                    if ui.button("Keep open").clicked() {
                        self.cancel_sidebar_close();
                    }
                    if ui.add(super::presentation::action(ui, "Roll back & close", super::presentation::Intent::Pending)).clicked() {
                        self.close();
                    }
                });
                let drafts = self.database.draft_count();
                if drafts > 0 { ui.small(format!("{drafts} staged edit{} will be discarded.", if drafts == 1 { "" } else { "s" })); }
                if self.database.transaction == TransactionState::Active { ui.small("The open transaction will be rolled back."); }
                ui.small("Waits for active execution and rolls back any remaining transaction. Committed writes are not undone.");
            });
    }

    fn open_databases_header_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.small("OPEN DATABASES");
            if self.database.session.is_some() && icons::button(ui, Icon::Refresh, "Refresh schema", "F5").clicked() {
                self.refresh_catalog(); self.load_diagram();
            }
            let slots = self.occupied_database_slots();
            let limits = self.memory.limits(&self.preferences);
            let hint = format!("{slots} of {} workspaces retained. {} admission budget at {} MiB per database. Adjust Settings → Database resources.", limits.database_limit, crate::resources::format_mib(limits.budget_mib), crate::resources::DATABASE_RESERVATION_MIB);
            ui.small(format!("{slots}/{}", limits.database_limit)).on_hover_text(&hint);
            if ui.available_width() >= 24.0 {
                let (rect,response) = ui.allocate_exact_size(egui::vec2(23.0,6.0),egui::Sense::hover());
                ui.painter().rect_filled(rect,2.0,ui.visuals().faint_bg_color);
                let fraction = if limits.database_limit == 0 { 0.0 } else { (slots as f32 / limits.database_limit as f32).min(1.0) };
                ui.painter().rect_filled(egui::Rect::from_min_size(rect.min,egui::vec2(rect.width()*fraction,rect.height())),2.0,ui.visuals().selection.stroke.color);
                response.on_hover_text(hint);
            }
        });
    }
    pub(super) fn open_databases_ui(&mut self, ui: &mut egui::Ui) {
        let mut ids: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|d| d.retained() || d.confirm_close)
            .map(|d| d.id)
            .collect();
        ids.sort_unstable();
        self.open_databases_header_ui(ui);
        if ids.is_empty() {
            ui.weak("Nothing open. Choose a database below.");
        }
        let enabled = self.picker.is_none() && !self.database.confirm_close && !self.quit;
        let mut select = None;
        let mut close = None;
        for id in ids {
            let active = id == self.database.id;
            let (selected, closed) = ui
                .push_id(("database-row", id), |ui| {
                    let database = if active {
                        &self.database
                    } else {
                        &self.parked[&id]
                    };
                    let result = open_database_row(ui, database, active, enabled);
                    if active && self.database.confirm_close {
                        self.inline_close_confirmation_ui(ui);
                    } else if active {
                        ui.indent(("active-database", id), |ui| {
                            if self.database.session.is_none() {
                                self.retained_database_ui(ui);
                            } else {
                                self.database_tools_ui(ui);
                                self.active_database_tree_ui(ui);
                            }
                        });
                    }
                    result
                })
                .inner;
            if selected {
                select = Some(id);
            }
            if closed {
                close = Some(id);
            }
        }
        if let Some(id) = select {
            self.select_workspace(id);
        }
        if let Some(id) = close {
            self.request_sidebar_close(id);
        }
    }
}
fn database_detail(database: &DatabaseWorkspace, drafts: usize) -> String {
    if database.closing {
        return "Closing…".into();
    }
    let mut detail = match database.transaction {
        TransactionState::Unknown => "Unknown transaction",
        TransactionState::Active => "Transaction",
        TransactionState::Autocommit => "",
    }
    .to_owned();
    if drafts > 0 {
        if !detail.is_empty() {
            detail.push_str(" · ");
        }
        detail.push_str(&format!(
            "{drafts} staged edit{}",
            if drafts == 1 { "" } else { "s" }
        ));
    }
    if !detail.is_empty() {
        return detail;
    }
    if database.session.is_none() {
        "Disconnected · retained data".into()
    } else {
        super::databases::folder_label(Path::new(&database.path))
    }
}
fn open_database_row(
    ui: &mut egui::Ui,
    database: &DatabaseWorkspace,
    active: bool,
    enabled: bool,
) -> (bool, bool) {
    egui::Frame::new()
        .fill(if active {
            ui.visuals().selection.bg_fill
        } else {
            ui.visuals().faint_bg_color
        })
        .corner_radius(6)
        .inner_margin(0.0)
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let response = open_database_button(ui, database, active, enabled);
                let selected = response.on_hover_text(&database.path).clicked();
                let closed = if database.closing {
                    ui.add_sized([28.0, 28.0], egui::Spinner::new());
                    false
                } else {
                    ui.add_enabled_ui(enabled, |ui| {
                        icons::button(
                            ui,
                            Icon::Close,
                            &format!("Close database: {}", database.path),
                            "",
                        )
                        .clicked()
                    })
                    .inner
                };
                (selected, closed)
            })
            .inner
        })
        .inner
}
fn open_database_button(
    ui: &mut egui::Ui,
    database: &DatabaseWorkspace,
    active: bool,
    enabled: bool,
) -> egui::Response {
    let width = (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
    let name = Path::new(&database.path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    let drafts = database.draft_count();
    let detail = database_detail(database, drafts);
    let response = ui
        .add_enabled_ui(enabled, |ui| {
            let (rect, response) =
                ui.allocate_exact_size(egui::vec2(width, 44.0), egui::Sense::click());
            let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            if response.has_focus() {
                painter.rect_stroke(
                    rect,
                    6.0,
                    ui.visuals().selection.stroke,
                    egui::StrokeKind::Inside,
                );
            }
            if active {
                painter.vline(
                    rect.left() + 1.0,
                    rect.y_range().shrink(5.0),
                    egui::Stroke::new(2.0, ui.visuals().selection.stroke.color),
                );
            }
            super::databases::opened_database_icon(
                ui,
                egui::Rect::from_center_size(
                    rect.left_center() + egui::vec2(14.0, 0.0),
                    egui::vec2(22.0, 22.0),
                ),
                enabled,
            );
            let access_rect = egui::Rect::from_center_size(
                rect.right_top() + egui::vec2(-12.0, 13.0),
                egui::Vec2::splat(16.0),
            );
            icons::access_indicator(ui, access_rect, database.writable, enabled);
            for (text, y, size, budget, color) in [
                (name.as_ref(), 5.0, 12.0, 56.0, ui.visuals().text_color()),
                (
                    detail.as_str(),
                    24.0,
                    10.0,
                    34.0,
                    if drafts > 0 || database.transaction != TransactionState::Autocommit {
                        ui.visuals().warn_fg_color
                    } else {
                        ui.visuals().weak_text_color()
                    },
                ),
            ] {
                let galley = egui::WidgetText::from(
                    egui::RichText::new(text).size(crate::appearance::text_size(ui.ctx(), size)),
                )
                .into_galley(
                    ui,
                    Some(egui::TextWrapMode::Truncate),
                    (width - budget).max(1.0),
                    egui::TextStyle::Body,
                );
                painter.galley(rect.min + egui::vec2(30.0, y), galley, color);
            }
            response
        })
        .inner;
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            response.enabled(),
            active,
            format!("Select database: {}", database.path),
        )
    });
    response
}
