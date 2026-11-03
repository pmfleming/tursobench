use super::{
    App, View,
    shell::{Nav, TableView, ToolPanel},
};
use eframe::egui;

pub(super) enum Command {
    Table(String, Option<String>),
    Map,
    List,
    Sql,
    Open,
    Close,
    Refresh,
    Panel(ToolPanel),
    Export,
    Review,
    Help,
    Theme,
    Begin,
    Filter,
    Inspector,
    Insert,
    FitColumns,
    ShowColumns,
    CopySelection,
    RefreshTable,
    Data,
    Structure,
    Back,
    CopyResults,
}

fn fuzzy(label: &str, query: &str) -> bool {
    let lower = label.to_lowercase();
    let mut chars = lower.chars();
    query
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .all(|wanted| chars.by_ref().any(|c| c == wanted))
}

impl App {
    fn dismiss_surface(&mut self, ctx: &egui::Context) {
        if self.workspace.close_pending.take().is_some() {
            return;
        }
        if self.document.tools.inline.take().is_some() {
            return;
        }
        if egui::Popup::is_any_open(ctx) {
            egui::Popup::close_all(ctx);
        } else {
            // Close only the topmost surface, preserving Escape's priority.
            let was_confirming_close = self.confirm_close;
            let mut surfaces = vec![
                &mut self.ui.palette_open,
                &mut self.ui.access_dialog,
                &mut self.confirm_close,
                &mut self.document.show_insert,
                &mut self.document.show_preview,
                &mut self.ui.export_open,
                &mut self.ui.outcomes,
                &mut self.tools.help,
                &mut self.document.ui.filter_open,
            ];
            if !self.workspace.enabled {
                surfaces.extend([
                    &mut self.document.ui.inspector_open,
                    &mut self.document.ui.sql_open,
                ]);
            }
            if let Some(surface) = surfaces.into_iter().find(|open| **open) {
                *surface = false;
            } else {
                self.workspace.expanded = None;
            }
            if was_confirming_close && !self.confirm_close {
                self.pending_database = None;
                self.quit = false;
            }
        }
    }
    pub(super) fn shortcuts(&mut self, ctx: &egui::Context) {
        use egui::{Key, Modifiers};
        self.grid_keys(ctx);
        let consumed = |mods, key| ctx.input_mut(|i| i.consume_key(mods, key));
        if consumed(Modifiers::NONE, Key::Escape) {
            self.dismiss_surface(ctx);
            return;
        }
        if consumed(Modifiers::CTRL, Key::K) {
            egui::Popup::close_all(ctx);
            self.open_palette();
            return;
        }
        if self.ui.palette_open {
            return;
        }
        if self.document.ui.sql_open
            && (!self.workspace.enabled || self.document.kind == super::tiling::Kind::Sql)
        {
            if consumed(Modifiers::CTRL | Modifiers::SHIFT, Key::Enter) {
                self.run_selection();
            } else if consumed(Modifiers::CTRL, Key::Enter) {
                self.run_sql();
            }
            if consumed(Modifiers::CTRL, Key::Space) {
                self.document.tools.complete = true;
            }
            if consumed(Modifiers::CTRL, Key::S) {
                self.ui.sidebar = ToolPanel::Saved;
            }
        }
        if consumed(Modifiers::NONE, Key::F5) {
            if self.document.ui.sql_open
                && self.document.tools.sql_focus.is_some()
                && ctx.memory(|m| m.focused()) == self.document.tools.sql_focus
            {
                self.run_sql();
            } else if self.tools.tree_focus.is_some()
                && ctx.memory(|m| m.focused()) == self.tools.tree_focus
                || self.document.ui.nav == Nav::Overview
            {
                self.refresh_catalog();
                self.load_diagram();
            } else if self.document.view == View::Browse {
                self.refresh_table();
            }
        }
        for (modifiers, key, command) in [
            (Modifiers::CTRL, Key::J, Command::Sql),
            (Modifiers::CTRL, Key::E, Command::Export),
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
            (
                Modifiers::CTRL,
                Key::Comma,
                Command::Panel(ToolPanel::Settings),
            ),
            (Modifiers::ALT, Key::ArrowLeft, Command::Back),
        ] {
            if !consumed(modifiers, key) {
                continue;
            }
            // Earlier shortcuts in this frame may have changed the active document.
            let enabled = match command {
                Command::Filter => self.document.ui.nav == Nav::Table,
                Command::CopyResults => matches!(self.document.ui.nav, Nav::Table | Nav::Result),
                _ => true,
            };
            if enabled {
                self.execute_command(ctx, command);
            }
        }
        if consumed(Modifiers::NONE, Key::F1) {
            self.tools.help = !self.tools.help;
        }
        if !ctx.egui_wants_keyboard_input()
            && !self.has_drafts()
            && self.commit_ticket.is_none()
            && self.ready
            && !self.closing
            && self.document.ui.nav == Nav::Table
        {
            let delta = if consumed(Modifiers::NONE, Key::PageDown) {
                100
            } else if consumed(Modifiers::NONE, Key::PageUp) {
                -100
            } else {
                0
            };
            if delta != 0
                && let Some(b) = self.document.browse.as_mut()
            {
                b.offset = b.offset.saturating_add_signed(delta).min(1_000_000);
                self.reload();
            }
        }
    }
    pub(super) fn access_dialog(&mut self, ctx: &egui::Context) {
        if !self.ui.access_dialog {
            return;
        }
        let mut open = true;
        egui::Window::new(if self.writable { "Reopen read-only?" } else { "Enable editing?" })
            .open(&mut open).anchor(egui::Align2::LEFT_BOTTOM, [12.0,-48.0]).max_width((ctx.content_rect().width()-40.0).min(520.0)).collapsible(false).show(ctx, |ui| {
                ui.label("The current session will close and roll back any open transaction before reopening. Local drafts will be discarded. Committed writes are not undone.");
                if !self.writable { ui.label("Back up the closed database with its WAL state before editing. SQL writes autocommit unless you BEGIN; grid edits are staged for review."); }
                ui.checkbox(&mut self.ui.access_ack, "No other application has this database open");
                if ui.add_enabled(self.ui.access_ack && self.ready && !self.closing && self.commit_ticket.is_none() && !self.uncertain_edit,
                    egui::Button::new("Close cleanly and reopen")).clicked() {
                    self.pending_access = Some(!self.writable);
                    self.ui.access_dialog = false;
                    self.close();
                }
            });
        self.ui.access_dialog &= open;
    }
    pub(super) fn open_palette(&mut self) {
        if self.session.is_none() && self.document.ui.nav == Nav::Databases {
            self.databases.focus_filter = true;
            return;
        }
        self.ui.palette_open = true;
        self.ui.palette_focus = true;
        self.ui.palette_query.clear();
        self.ui.palette_index = 0;
    }
    pub(super) fn toggle_sql(&mut self) {
        if self.workspace.enabled {
            self.open_sql_window(false);
            return;
        }
        self.document.ui.sql_open = !self.document.ui.sql_open;
    }
    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.ui.palette_open {
            return;
        }
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Search tables, columns and commands")
            .id(egui::Id::unique("command-palette"))
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_TOP, [0.0, 70.0])
            .default_width(580.0)
            .show(ctx, |ui| {
                let search = ui.add(
                    egui::TextEdit::singleline(&mut self.ui.palette_query)
                        .id_salt("command-search")
                        .hint_text("Table, column, or action…")
                        .desired_width(ui.available_width()),
                );
                if self.ui.palette_focus {
                    search.request_focus();
                    self.ui.palette_focus = false;
                }
                if search.changed() {
                    self.ui.palette_index = 0;
                }
                let mut items = self.palette_items();
                let mut chosen_index = None;
                self.ui.palette_index = self.ui.palette_index.min(items.len().saturating_sub(1));
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                    self.ui.palette_index =
                        (self.ui.palette_index + 1).min(items.len().saturating_sub(1));
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                    self.ui.palette_index = self.ui.palette_index.saturating_sub(1);
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                    chosen_index = Some(self.ui.palette_index);
                }
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 240.0).max(100.0))
                    .show(ui, |ui| {
                        for (index, (label, _)) in items.iter().enumerate() {
                            let selected = index == self.ui.palette_index;
                            let response = ui.selectable_label(selected, label);
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
                chosen = chosen_index.filter(|&i| i < items.len()).map(|i| items.swap_remove(i).1);
                ui.small(
                    "Up/Down choose · Enter open · Esc close. Column search uses loaded map metadata.",
                );
            });
        self.ui.palette_open = open;
        if let Some(action) = chosen {
            self.ui.palette_open = false;
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
            ("New row (staged)".into(), Command::Insert),
            ("Fit all columns".into(), Command::FitColumns),
            ("Show hidden columns".into(), Command::ShowColumns),
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
            ("Settings".into(), Command::Panel(ToolPanel::Settings)),
            ("Export and copy retained result".into(), Command::Export),
            ("Review staged edits".into(), Command::Review),
            ("Toggle theme".into(), Command::Theme),
            ("Keyboard shortcuts and help · F1".into(), Command::Help),
        ];
        items.extend(
            self.schema
                .iter()
                .filter(|o| o.kind == "table" || o.kind == "view")
                .map(|o| {
                    (
                        format!("{} · {}", o.name, o.kind),
                        Command::Table(o.name.clone(), None),
                    )
                }),
        );
        if !self.ui.palette_query.is_empty()
            && let Some(graph) = &self.ui.diagram.data
        {
            for node in &graph.tables {
                if let Some(info) = &node.inspection {
                    items.extend(info.table.columns.iter().map(|c| {
                        (
                            format!("{}.{} · column", node.name, c.name),
                            Command::Table(node.name.clone(), Some(c.name.clone())),
                        )
                    }));
                }
            }
        }
        items.retain(|(label, _)| fuzzy(label, &self.ui.palette_query));
        items.truncate(80);
        items
    }
    pub(super) fn execute_command(&mut self, ctx: &egui::Context, action: Command) {
        match action {
            Command::Table(name, column) => {
                let before = self.document.result.current;
                self.activate_table(name.clone());
                if self.document.result.current != before
                    || self.document.view == super::View::Browse
                        && self.document.table.as_ref().is_some_and(|t| t.name == name)
                {
                    self.document.ui.nav = Nav::Table;
                    self.document.ui.table_view = TableView::Data;
                    self.document.tools.highlight_column = column.map(|column| (name, column));
                    self.document.tools.scroll_to_column =
                        self.document.tools.highlight_column.is_some();
                }
            }
            Command::List => {
                self.select_tool(ToolPanel::Schema);
            }
            Command::Map => {
                self.overview();
                self.ui.sidebar = ToolPanel::Schema;

                if self.ui.diagram.data.is_none() {
                    self.load_diagram();
                }
            }
            Command::Data => {
                if self.workspace.enabled
                    && let Some(name) = self
                        .document
                        .object
                        .clone()
                        .or_else(|| self.ui.diagram.selected.clone())
                {
                    self.browse_table(name);
                }
                self.document.ui.table_view = TableView::Data;
            }
            Command::Structure => {
                if self.workspace.enabled {
                    self.open_structure_window();
                }
                self.document.ui.table_view = TableView::Structure;
            }
            Command::Back => {
                if self.document.view == super::View::Browse {
                    self.navigate_back();
                } else {
                    self.return_to_table();
                }
            }
            Command::CopyResults => self.copy_results(ctx, crate::export::Format::Tsv),
            Command::Sql => self.toggle_sql(),
            Command::Open => {
                if self.session.is_some() {
                    self.request_close();
                } else {
                    self.choose_file(ctx);
                }
            }
            Command::Begin => {
                if self.transaction == tursobench_core::TransactionState::Autocommit {
                    self.run_text("BEGIN".into(), None);
                }
            }
            Command::Filter => {
                self.document.ui.filter_open = true;
                self.document.tools.filter_focus = true;
            }
            Command::Inspector => self.toggle_inspector(),
            Command::Insert => self.begin_insert(),
            Command::FitColumns => {
                self.document.tools.columns.manual.clear();
                self.document.tools.columns.refit = true;
            }
            Command::ShowColumns => self.document.tools.columns.hidden.clear(),
            Command::CopySelection => match self.selection_tsv() {
                Ok(text) => ctx.copy_text(text),
                Err(e) => self.status = e,
            },
            Command::RefreshTable => self.refresh_table(),
            Command::Close => self.request_close(),
            Command::Refresh => {
                self.refresh_catalog();
                self.load_diagram();
            }
            Command::Panel(panel) => {
                self.select_tool(panel);
            }
            Command::Export => self.ui.export_open = true,
            Command::Review => {
                self.focus_drafts();
                self.document.show_preview = true;
            }
            Command::Help => self.tools.help = true,
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
