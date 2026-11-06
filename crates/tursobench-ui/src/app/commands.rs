use super::presentation::{self, Intent};
use super::{
    App, View,
    ribbon::Action,
    shell::{Nav, TableView, ToolPanel},
    tiling::Kind,
};
use eframe::egui;

pub(super) enum Command {
    Table(String, Option<String>),
    Definition(String),
    Map,
    List,
    Sql,
    Open,
    Close,
    Refresh,
    Panel(ToolPanel),
    Settings,
    Help,
    Theme,
    Begin,
    Action(Action),
    Filter,
    Inspector,
    CopySelection,
    RefreshTable,
    Data,
    Structure,
    Back,
    CopyResults,
}

pub(super) fn fuzzy(label: &str, query: &str) -> bool {
    let lower = label.to_lowercase();
    let mut chars = lower.chars();
    query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|wanted| chars.by_ref().any(|c| c == wanted))
}

pub(super) struct ObjectRef<'a> {
    kind: &'a str,
    name: &'a str,
    column: Option<&'a str>,
}
impl ObjectRef<'_> {
    pub(super) fn matches_name(&self, query: &str) -> bool {
        fuzzy(
            &self
                .column
                .map_or_else(|| self.name.into(), |c| format!("{}.{}", self.name, c)),
            query,
        )
    }

    pub(super) fn label(&self) -> String {
        match self.column {
            Some(column) => format!("{}.{} · column", self.name, column),
            None => format!("{} · {}", self.name, self.kind),
        }
    }
    pub(super) fn command(&self) -> Command {
        match self.kind {
            "table" | "view" => Command::Table(self.name.into(), self.column.map(str::to_owned)),
            _ => Command::Definition(self.name.into()),
        }
    }
}
pub(super) fn objects<'a>(
    schema: &'a [tursobench_core::SchemaObject],
    graph: Option<&'a tursobench_core::SchemaDiagram>,
) -> impl Iterator<Item = ObjectRef<'a>> {
    let tables = schema.iter().map(|o| ObjectRef {
        kind: &o.kind,
        name: &o.name,
        column: None,
    });
    let columns = graph
        .into_iter()
        .flat_map(|g| &g.tables)
        .filter_map(|node| {
            node.inspection
                .as_ref()
                .map(|info| (&node.name, &info.table))
        })
        .flat_map(|(name, table)| {
            table.columns.iter().map(move |c| ObjectRef {
                kind: "table",
                name,
                column: Some(&c.name),
            })
        });
    tables.chain(columns)
}

impl App {
    fn dismiss_surface(&mut self, ctx: &egui::Context) {
        if self.settings.open {
            if egui::Popup::is_any_open(ctx) {
                egui::Popup::close_all(ctx);
            } else {
                self.settings.open = false;
            }
            return;
        }
        if self.database.document.tools.inline.take().is_some() {
            return;
        }
        if egui::Popup::is_any_open(ctx) {
            egui::Popup::close_all(ctx);
        } else {
            // Close only the topmost surface, preserving Escape's priority.
            let was_confirming_close = self.database.confirm_close;
            let mut surfaces = vec![
                &mut self.database.ui.palette_open,
                &mut self.database.ui.access_dialog,
                &mut self.database.confirm_close,
                &mut self.database.document.show_insert,
                &mut self.database.document.show_preview,
                &mut self.database.ui.export_open,
                &mut self.database.ui.outcomes,
                &mut self.database.tools.help,
                &mut self.database.document.ui.filter_open,
            ];
            if !self.database.workspace.enabled {
                surfaces.extend([
                    &mut self.database.document.ui.inspector_open,
                    &mut self.database.document.ui.sql_open,
                ]);
            }
            if let Some(surface) = surfaces.into_iter().find(|open| **open) {
                *surface = false;
            } else {
                self.database.workspace.expanded = None;
            }
            if was_confirming_close && !self.database.confirm_close {
                self.cancel_sidebar_close();
            }
        }
    }
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        self.database_navigation_keys(ctx);
        self.grid_keys(ctx);
        let consumed = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if consumed(Modifiers::NONE, Key::Escape) {
            let search = egui::Id::unique("sidebar-search");
            if ctx.memory(|m| m.has_focus(search)) && !self.sidebar_search.query.is_empty() {
                self.sidebar_search.query.clear();
                self.sidebar_search.index = 0;
                self.sidebar_search.navigating = false;
            } else {
                self.dismiss_surface(ctx);
            }
            return;
        }
        if consumed(Modifiers::CTRL, Key::K) {
            egui::Popup::close_all(ctx);
            self.open_palette();
            return;
        }
        if self.database.ui.palette_open {
            return;
        }
        if self.database.workspace.enabled && self.database.workspace.active == Kind::Sql
            || !self.database.workspace.enabled && self.database.document.ui.sql_open
        {
            if consumed(Modifiers::CTRL | Modifiers::SHIFT, Key::Enter) {
                if self.database.workspace.enabled {
                    self.focus_pane(Kind::Sql);
                }
                self.run_current(None);
            } else if consumed(Modifiers::CTRL, Key::Enter) {
                if self.database.workspace.enabled {
                    self.focus_pane(Kind::Sql);
                }
                self.run_sql();
            }
            if consumed(Modifiers::CTRL, Key::Space) {
                if self.database.workspace.enabled {
                    self.focus_pane(Kind::Sql);
                }
                self.database.document.tools.complete = true;
            }
            if consumed(Modifiers::CTRL, Key::S) {
                if self.database.workspace.enabled {
                    self.focus_pane(Kind::Sql);
                }
                self.database.ui.sidebar = ToolPanel::Saved;
            }
        }
        if consumed(Modifiers::NONE, Key::F5) {
            if self.database.document.ui.sql_open
                && self.database.document.tools.sql_focus.is_some()
                && ctx.memory(|m| m.focused()) == self.database.document.tools.sql_focus
            {
                self.run_sql();
            } else if self.database.tools.tree_focus.is_some()
                && ctx.memory(|m| m.focused()) == self.database.tools.tree_focus
                || self.database.document.ui.nav == Nav::Overview
            {
                self.refresh_catalog();
                self.load_diagram();
            } else if self.database.document.view == View::Browse {
                self.refresh_table();
            }
        }
        for (modifiers, key, command) in [
            (Modifiers::CTRL, Key::J, Command::Sql),
            (Modifiers::CTRL, Key::E, Command::Action(Action::Export)),
            (Modifiers::CTRL, Key::F, Command::Filter),
            (Modifiers::CTRL, Key::Num1, Command::Data),
            (Modifiers::CTRL, Key::Num2, Command::Structure),
            (
                Modifiers::CTRL | Modifiers::SHIFT,
                Key::C,
                Command::CopyResults,
            ),
            (Modifiers::CTRL, Key::O, Command::Open),
            (Modifiers::CTRL, Key::T, Command::Theme),
            (Modifiers::CTRL, Key::Comma, Command::Settings),
            (Modifiers::ALT, Key::ArrowLeft, Command::Back),
        ] {
            if !consumed(modifiers, key) {
                continue;
            }
            // Earlier shortcuts in this frame may have changed the active document.
            let enabled = match command {
                Command::Filter => self.database.document.ui.nav == Nav::Table,
                Command::CopyResults => {
                    matches!(self.database.document.ui.nav, Nav::Table | Nav::Result)
                }
                _ => true,
            };
            if enabled {
                self.execute_command(ctx, command);
            }
        }
        if consumed(Modifiers::NONE, Key::F1) {
            self.database.tools.help = !self.database.tools.help;
        }
        if !ctx.egui_wants_keyboard_input()
            && self.requests_enabled()
            && self.database.document.ui.nav == Nav::Table
        {
            let delta = if consumed(Modifiers::NONE, Key::PageDown) {
                100
            } else if consumed(Modifiers::NONE, Key::PageUp) {
                -100
            } else {
                0
            };
            if delta != 0
                && let Some(b) = self.database.document.browse.as_mut()
            {
                b.offset = b.offset.saturating_add_signed(delta).min(1_000_000);
                self.reload();
            }
        }
    }
    pub(super) fn access_dialog(&mut self, ctx: &egui::Context) {
        if !self.database.ui.access_dialog {
            return;
        }
        let mut open = true;
        let access = super::icons::access_mode(ctx, !self.database.writable);
        egui::Window::new(format!("Reopen: {}?", access.name)).frame(crate::app::presentation::window_frame(ctx)).default_pos(ctx.content_rect().center()-egui::vec2(280.0,180.0))
            .open(&mut open).anchor(egui::Align2::LEFT_BOTTOM, [12.0,-48.0]).max_width((ctx.content_rect().width()-40.0).min(520.0)).collapsible(false).show(ctx, |ui| {
                presentation::notice(ui, access.icon, Intent::Pending,"The current session will close and roll back any open transaction before reopening. Local drafts will be discarded. Committed writes are not undone.");
                if !self.database.writable { ui.label("Back up the closed database with its WAL state before editing. SQL writes autocommit unless you BEGIN; grid edits are staged for review."); }
                ui.label("File locks and shared WAL coordination will be checked again when reopening.");
                if presentation::footer(ui, |ui| ui.add_enabled(self.database.ready && !self.database.closing && self.database.commit_ticket.is_none() && !self.database.uncertain_edit,
                    presentation::action(ui,"Close cleanly and reopen",Intent::Pending))).inner.clicked() {
                    self.database.pending_access = Some(!self.database.writable);
                    self.database.ui.access_dialog = false;
                    self.close();
                }
            });
        self.database.ui.access_dialog &= open;
    }
    pub(super) fn open_palette(&mut self) {
        self.database.ui.palette_open = true;
        self.database.ui.palette_focus = true;
        self.database.ui.palette_query.clear();
        self.database.ui.palette_index = 0;
    }
    pub(super) fn toggle_sql(&mut self) {
        if self.database.workspace.enabled {
            self.open_sql_window(false);
            return;
        }
        self.database.document.ui.sql_open = !self.database.document.ui.sql_open;
    }
    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.database.ui.palette_open {
            return;
        }
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Commands")
            .frame(crate::app::presentation::window_frame(ctx))
            .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 180.0))
            .id(egui::Id::unique("command-palette"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
            .default_width(580.0)
            .show(ctx, |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.database.ui.palette_query)
                        .id_salt("command-search")
                        .hint_text("Find an action…")
                        .desired_width(ui.available_width()),
                );
                if self.database.ui.palette_focus {
                    search.request_focus();
                    self.database.ui.palette_focus = false;
                }
                if search.changed() {
                    self.database.ui.palette_index = 0;
                }
                let mut items = self.palette_items();
                let mut chosen_index = None;
                self.database.ui.palette_index = self
                    .database
                    .ui
                    .palette_index
                    .min(items.len().saturating_sub(1));
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                    self.database.ui.palette_index =
                        (self.database.ui.palette_index + 1).min(items.len().saturating_sub(1));
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                    self.database.ui.palette_index =
                        self.database.ui.palette_index.saturating_sub(1);
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                    chosen_index = Some(self.database.ui.palette_index);
                }
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 240.0).max(100.0))
                    .show(ui, |ui| {
                        for (index, (label, _)) in items.iter().enumerate() {
                            let selected = index == self.database.ui.palette_index;
                            let response = ui.add_sized(
                                [ui.available_width(), 32.0],
                                egui::Button::new(label)
                                    .selected(selected)
                                    .frame_when_inactive(false),
                            );
                            if selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                chosen_index = Some(index);
                            }
                        }
                        if items.is_empty() {
                            ui.label("No matches in loaded metadata.");
                        }
                    });
                chosen = chosen_index
                    .filter(|&i| i < items.len())
                    .map(|i| items.swap_remove(i).1);
                presentation::footer(ui, |ui| {
                    ui.small(
                        "Up/Down choose · Enter run · Esc close. Ctrl+P searches database objects.",
                    );
                });
            });
        self.database.ui.palette_open = open;
        if let Some(action) = chosen {
            self.database.ui.palette_open = false;
            self.execute_command(ctx, action);
        }
    }
    fn palette_items(&self) -> Vec<(String, Command)> {
        let mut items: Vec<(String, Command)> = vec![
            ("Schema home · Map".into(), Command::Map),
            ("List tables and views".into(), Command::List),
            ("Toggle SQL editor · Ctrl J".into(), Command::Sql),
            ("Open database · Ctrl O".into(), Command::Open),
            ("Close database".into(), Command::Close),
            ("Refresh schema map and catalog".into(), Command::Refresh),
            ("Refresh current table".into(), Command::RefreshTable),
            ("BEGIN editor transaction".into(), Command::Begin),
            ("Filter rows · Ctrl F".into(), Command::Filter),
            ("Toggle inspector · Space".into(), Command::Inspector),
            ("New row (staged)".into(), Command::Action(Action::Insert)),
            (
                "Fit all columns".into(),
                Command::Action(Action::FitColumns),
            ),
            (
                "Show hidden columns".into(),
                Command::Action(Action::ShowColumns),
            ),
            (
                "Copy selected cells · Ctrl C".into(),
                Command::CopySelection,
            ),
            ("History".into(), Command::Panel(ToolPanel::History)),
            ("Saved queries".into(), Command::Panel(ToolPanel::Saved)),
            (
                "Database info and checks".into(),
                Command::Panel(ToolPanel::Diagnostics),
            ),
            ("Settings".into(), Command::Settings),
            (
                "Export and copy retained result".into(),
                Command::Action(Action::Export),
            ),
            (
                "Review staged edits".into(),
                Command::Action(Action::Review),
            ),
            ("Toggle theme".into(), Command::Theme),
            ("Keyboard shortcuts and help · F1".into(), Command::Help),
        ];
        items.retain(|(label, _)| fuzzy(label, &self.database.ui.palette_query));
        items.truncate(80);
        items
    }
    pub(super) fn execute_command(&mut self, ctx: &egui::Context, action: Command) {
        match action {
            Command::Definition(name) => {
                if let Some(object) = self
                    .database
                    .schema
                    .iter()
                    .find(|o| o.name == name)
                    .cloned()
                {
                    if self.database.workspace.enabled
                        && self
                            .create_window(Kind::Structure, Some(object.name.clone()), None, false)
                            .is_none()
                    {
                        return;
                    }
                    self.database.ui.diagram.selected = Some(object.table.clone());
                    self.database.document.tools.ddl = Some(object);
                    self.database.document.ui.nav = Nav::Table;
                    self.database.document.ui.table_view = TableView::Structure;
                }
            }
            Command::Table(name, column) => {
                let before = self.database.document.result.current;
                self.activate_table(name.clone());
                if self.database.document.result.current != before
                    || self.database.document.view == super::View::Browse
                        && self
                            .database
                            .document
                            .table
                            .as_ref()
                            .is_some_and(|t| t.name == name)
                {
                    self.database.document.ui.nav = Nav::Table;
                    self.database.document.ui.table_view = TableView::Data;
                    self.database.document.tools.highlight_column =
                        column.map(|column| (name, column));
                    self.database.document.tools.scroll_to_column =
                        self.database.document.tools.highlight_column.is_some();
                }
            }
            Command::List => {
                self.select_tool(ToolPanel::Schema);
            }
            Command::Map => {
                self.overview();
                self.database.ui.sidebar = ToolPanel::Schema;

                if self.database.ui.diagram.data.is_none() {
                    self.load_diagram();
                }
            }
            Command::Data => {
                if self.database.workspace.enabled
                    && let Some(name) = self
                        .database
                        .document
                        .object
                        .clone()
                        .or_else(|| self.database.ui.diagram.selected.clone())
                {
                    self.browse_table(name);
                }
                self.database.document.ui.table_view = TableView::Data;
            }
            Command::Structure => {
                if self.database.workspace.enabled {
                    self.open_structure_window();
                }
                self.database.document.ui.table_view = TableView::Structure;
            }
            Command::Back => {
                if self.database.document.view == super::View::Browse {
                    self.navigate_back();
                } else {
                    self.return_to_table();
                }
            }
            Command::CopyResults => self.copy_results(ctx, crate::export::Format::Tsv),
            Command::Sql => self.toggle_sql(),
            Command::Open => self.begin_database_browse(ctx),
            Command::Begin => {
                if self.database.transaction == tursobench_core::TransactionState::Autocommit {
                    self.run_text("BEGIN".into(), None);
                }
            }
            Command::Filter => {
                self.database.document.ui.filter_open = true;
                self.database.document.tools.filter_focus = true;
            }
            Command::Inspector => self.toggle_inspector(),
            Command::CopySelection => match self.selection_tsv() {
                Ok(text) => ctx.copy_text(text),
                Err(e) => self.database.status = e,
            },
            Command::RefreshTable => self.refresh_table(),
            Command::Close => self.request_sidebar_close(self.database.id),
            Command::Refresh => {
                self.refresh_catalog();
                self.load_diagram();
            }
            Command::Settings => self.settings.open = true,
            Command::Panel(panel) => {
                self.select_tool(panel);
            }
            Command::Action(action) => self.dispatch_action(action, ctx),
            Command::Help => self.database.tools.help = true,
            Command::Theme => {
                self.toggle_theme(ctx);
                self.store.save(&self.preferences);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fuzzy;
    #[test]
    fn search_matches_subsequences_case_insensitively_and_unicode() {
        assert!(fuzzy("entries.echo_source_id", "E ec src"));
        assert!(fuzzy("λ_table.id", "λid"));
        assert!(!fuzzy("entries", "sqlite"));
    }
}
