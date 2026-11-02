use super::{
    App,
    shell::{Nav, TableView, ToolPanel},
};
use eframe::egui;

#[derive(Clone)]
enum Command {
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
        if self.session.is_none() && self.ui.nav == Nav::Databases {
            self.databases.focus_filter = true;
            return;
        }
        self.ui.palette_open = true;
        self.ui.palette_focus = true;
        self.ui.palette_query.clear();
        self.ui.palette_index = 0;
    }
    pub(super) fn toggle_sql(&mut self) {
        self.ui.sql_open = !self.ui.sql_open;
    }
    pub(super) fn command_palette(&mut self, ctx: &egui::Context) {
        if !self.ui.palette_open {
            return;
        }
        let mut open = true;
        let mut chosen = None;
        egui::Window::new("Search tables, columns and commands")
            .id(egui::Id::new("command-palette"))
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
                let mut items: Vec<(String, Command)> = vec![
                    ("Schema home · Map".into(), Command::Map),
                    ("List tables and views".into(), Command::List),
                    ("Toggle SQL editor · Ctrl J".into(), Command::Sql),
                    ("Open database · Ctrl O".into(), Command::Open),
                    ("Close database".into(), Command::Close),
                    ("Refresh schema map and catalog".into(), Command::Refresh),
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
                self.ui.palette_index = self.ui.palette_index.min(items.len().saturating_sub(1));
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown)) {
                    self.ui.palette_index =
                        (self.ui.palette_index + 1).min(items.len().saturating_sub(1));
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp)) {
                    self.ui.palette_index = self.ui.palette_index.saturating_sub(1);
                }
                if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
                    chosen = items
                        .get(self.ui.palette_index)
                        .map(|(_, action)| action.clone());
                }
                egui::ScrollArea::vertical()
                    .max_height((ctx.content_rect().height() - 240.0).max(100.0))
                    .show(ui, |ui| {
                        for (index, (label, action)) in items.iter().enumerate() {
                            let selected = index == self.ui.palette_index;
                            let response = ui.selectable_label(selected, label);
                            if selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                chosen = Some(action.clone());
                            }
                        }
                        if items.is_empty() {
                            ui.label("No matches in loaded metadata.");
                        }
                    });
                ui.small(
                    "Up/Down choose · Enter open · Esc close. Column search uses loaded map metadata.",
                );
            });
        self.ui.palette_open = open;
        if let Some(action) = chosen {
            self.ui.palette_open = false;
            match action {
                Command::Table(name, column) => {
                    let before = self.result.current;
                    self.activate_table(name.clone());
                    if self.result.current != before
                        || self.view == super::View::Browse
                            && self.table.as_ref().is_some_and(|t| t.name == name)
                    {
                        self.ui.nav = Nav::Table;
                        self.ui.table_view = TableView::Data;
                        self.tools.highlight_column = column.map(|column| (name, column));
                        self.tools.scroll_to_column = self.tools.highlight_column.is_some();
                    }
                }
                Command::Map | Command::List => {
                    self.overview();
                    self.ui.map_list = matches!(action, Command::List);

                    if self.ui.diagram.data.is_none() {
                        self.load_diagram();
                    }
                }
                Command::Sql => self.toggle_sql(),
                Command::Open => self.choose_file(ctx),
                Command::Close => self.request_close(),
                Command::Refresh => {
                    self.refresh_catalog();
                    self.load_diagram();
                }
                Command::Panel(panel) => {
                    self.ui.sidebar = panel;
                    self.ui.sidebar_open = true;
                }
                Command::Export => self.ui.export_open = true,
                Command::Review => self.show_preview = true,
                Command::Help => self.tools.help = true,
                Command::Theme => {
                    self.preferences.dark_mode = !self.preferences.dark_mode;
                    self.apply_theme(ctx);
                    self.store.save(&self.preferences);
                }
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
