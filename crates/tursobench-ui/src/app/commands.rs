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
    ToggleSidebar,
    ToggleAssistant,
}

pub(super) struct PaletteItem {
    tier: u8,
    label: &'static str,
    shortcut: &'static str,
    /// Set when the command cannot run now; the row is shown greyed out.
    reason: Option<&'static str>,
    command: Command,
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

/// How well a palette entry answers a query; lower is better and `None`
/// leaves it out. Labels match from the start of a word, so "set" finds
/// Settings rather than the letters s…e…t scattered through another label.
/// Keywords are other words for the same thing ("inspector" for Record);
/// they rank below the label. A shortcut matches only from its start
/// ("f5", "ctrl j").
pub(super) fn rank(label: &str, shortcut: &str, keywords: &str, query: &str) -> Option<u8> {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return Some(0);
    }
    let label = label.to_lowercase();
    let split = |text: &str| -> Vec<String> {
        text.to_lowercase()
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let words = split(&label);
    let compact = |text: &str| text.split_whitespace().collect::<String>();
    // Every typed word starts one of `words`, in order: "ref tab".
    let word_starts = |words: &[String]| {
        let mut rest = words.iter();
        query
            .split_whitespace()
            .all(|part| rest.by_ref().any(|w| w.starts_with(part)))
    };
    // Initials: "tse" for Toggle SQL editor.
    let initials = || {
        let mut initials = words.iter().filter_map(|w| w.chars().next());
        !query.contains(' ') && query.chars().all(|q| initials.by_ref().any(|c| c == q))
    };
    if label.starts_with(&query) {
        Some(0)
    } else if words.iter().any(|w| w.starts_with(&query)) {
        Some(1)
    } else if label.contains(&query) {
        Some(2)
    } else if word_starts(&words) {
        Some(3)
    } else if initials() {
        Some(4)
    } else if !keywords.is_empty() && word_starts(&split(keywords)) {
        Some(5)
    } else if !shortcut.is_empty()
        && compact(&shortcut.to_lowercase()).starts_with(&compact(&query))
    {
        Some(6)
    } else {
        None
    }
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
        // The palette sits above everything, so Escape closes it before it
        // can cancel a cell edit or dialog the user cannot see beneath it.
        if self.database.ui.palette_open {
            self.database.ui.palette_open = false;
            return;
        }
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
        if self
            .database
            .document
            .tools
            .spreadsheet
            .dialog
            .take()
            .is_some()
        {
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
            self.toggle_palette();
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
            (Modifiers::CTRL, Key::B, Command::ToggleSidebar),
            (Modifiers::CTRL, Key::I, Command::ToggleAssistant),
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
    /// Ctrl K and the Commands button close the palette they opened.
    pub(super) fn toggle_palette(&mut self) {
        if self.database.ui.palette_open {
            self.database.ui.palette_open = false;
        } else {
            self.open_palette();
        }
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
        let just_opened = self.database.ui.palette_focus;
        let shown = egui::Window::new("Commands")
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
                        for (index, item) in items.iter().enumerate() {
                            let selected = index == self.database.ui.palette_index;
                            // Names read down the left edge; shortcuts line up on the right.
                            let mut button = egui::Button::new(item.label)
                                .selected(selected)
                                // The keyboard choice needs its fill even without hover.
                                .frame_when_inactive(selected)
                                .min_size(egui::vec2(ui.available_width(), 32.0));
                            // An empty shortcut would still add a space to the accessible name.
                            if !item.shortcut.is_empty() {
                                button = button.shortcut_text(
                                    egui::RichText::new(item.shortcut)
                                        .monospace()
                                        .size(crate::appearance::text_size(ui.ctx(), 11.0)),
                                );
                            }
                            let response = ui
                                .add_enabled(item.reason.is_none(), button)
                                .on_disabled_hover_text(item.reason.unwrap_or_default());
                            if selected {
                                response.scroll_to_me(Some(egui::Align::Center));
                            }
                            if response.clicked() {
                                chosen_index = Some(index);
                            }
                        }
                        if items.is_empty() {
                            ui.weak("No matching commands. Ctrl P searches tables and columns.");
                        }
                    });
                // An unavailable command stays put and explains itself instead.
                let reason = items
                    .get(self.database.ui.palette_index)
                    .and_then(|item| item.reason);
                chosen = chosen_index
                    .filter(|&i| items.get(i).is_some_and(|item| item.reason.is_none()))
                    .map(|i| items.swap_remove(i).command);
                presentation::footer(ui, |ui| match reason {
                    Some(reason) => {
                        ui.colored_label(ui.visuals().warn_fg_color, reason);
                    }
                    None => {
                        ui.small(
                            "Up/Down choose · Enter run · Esc close. Ctrl+P searches database objects.",
                        );
                    }
                });
            });
        // A click elsewhere dismisses the palette, as it would a menu. The
        // frame that opened it is skipped, since that click was outside it.
        if !just_opened
            && let Some(rect) = shown.map(|shown| shown.response.rect)
            && ctx.input(|i| {
                i.pointer.any_click() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p))
            })
        {
            open = false;
        }
        self.database.ui.palette_open = open;
        if let Some(action) = chosen {
            self.database.ui.palette_open = false;
            self.execute_command(ctx, action);
        }
    }
    fn palette_items(&self) -> Vec<PaletteItem> {
        // Labels use the names shown elsewhere in the app; keywords keep the
        // older or generic words searchable.
        let items: Vec<(&'static str, &'static str, &'static str, Command)> = vec![
            (
                "Relationships",
                "",
                "map diagram schema home foreign keys",
                Command::Map,
            ),
            ("Tables", "", "list views sidebar schema", Command::List),
            ("SQL editor", "Ctrl J", "query toggle", Command::Sql),
            ("Open database", "Ctrl O", "file browse", Command::Open),
            ("Close database", "", "", Command::Close),
            (
                "Refresh schema",
                "",
                "catalog map relationships reload",
                Command::Refresh,
            ),
            (
                "Refresh current table",
                "F5",
                "reload rows",
                Command::RefreshTable,
            ),
            ("BEGIN editor transaction", "", "start", Command::Begin),
            (
                "COMMIT editor transaction",
                "",
                "save end",
                Command::Action(Action::Commit),
            ),
            (
                "ROLLBACK editor transaction",
                "",
                "roll back undo abort cancel",
                Command::Action(Action::Rollback),
            ),
            ("Filter rows", "Ctrl F", "where search", Command::Filter),
            (
                "Open record",
                "Space",
                "inspector detail row",
                Command::Inspector,
            ),
            (
                "New row (staged)",
                "",
                "insert add",
                Command::Action(Action::Insert),
            ),
            (
                "Fit all columns",
                "",
                "width resize autosize",
                Command::Action(Action::FitColumns),
            ),
            (
                "Show hidden columns",
                "",
                "unhide visible",
                Command::Action(Action::ShowColumns),
            ),
            (
                "Copy selected cells",
                "Ctrl C",
                "clipboard",
                Command::CopySelection,
            ),
            (
                "History",
                "",
                "recent queries",
                Command::Panel(ToolPanel::History),
            ),
            (
                "Saved queries",
                "",
                "bookmarks",
                Command::Panel(ToolPanel::Saved),
            ),
            (
                "Database checks",
                "",
                "integrity diagnostics info pragma dashboard profiling compare schemas transfer maintenance backup",
                Command::Panel(ToolPanel::Diagnostics),
            ),
            (
                "Sidebar",
                "Ctrl B",
                "show hide toggle navigation panel",
                Command::ToggleSidebar,
            ),
            (
                "Assistant",
                "Ctrl I",
                "chat ai agent claude codex copilot ask show hide toggle panel",
                Command::ToggleAssistant,
            ),
            (
                "Settings",
                "Ctrl ,",
                "preferences options",
                Command::Settings,
            ),
            (
                "Export retained result",
                "Ctrl E",
                "copy save csv json download",
                Command::Action(Action::Export),
            ),
            (
                "Review staged edits",
                "",
                "pending changes",
                Command::Action(Action::Review),
            ),
            (
                "Toggle theme",
                "Ctrl T",
                "dark light appearance",
                Command::Theme,
            ),
            (
                "Keyboard and safety reference",
                "F1",
                "help shortcuts keys",
                Command::Help,
            ),
        ];
        let query = &self.database.ui.palette_query;
        let mut ranked: Vec<_> = items
            .into_iter()
            .filter_map(|(label, shortcut, keywords, command)| {
                rank(label, shortcut, keywords, query).map(|tier| PaletteItem {
                    tier,
                    label,
                    shortcut,
                    reason: self.unavailable(&command),
                    command,
                })
            })
            .collect();
        // Stable, so equally good matches keep the list's own order; what can
        // run now comes before what cannot, so Enter picks a working command.
        ranked.sort_by_key(|item| (item.tier, item.reason.is_some()));
        ranked.truncate(80);
        ranked
    }

    /// Why a palette command cannot run now, or `None` when it can.
    fn unavailable(&self, command: &Command) -> Option<&'static str> {
        use tursobench_core::TransactionState;
        let open = self.database.session.is_some() && self.database.ready;
        let table = self.database.document.view == View::Browse
            && self.database.document.ui.nav == Nav::Table;
        let busy = || {
            if self.has_drafts() {
                Some("Commit or discard staged edits first")
            } else {
                Some("Wait for the running statement to finish")
            }
        };
        match command {
            Command::Open
            | Command::Settings
            | Command::Help
            | Command::Theme
            | Command::ToggleSidebar
            | Command::ToggleAssistant => None,
            Command::Close if self.database.session.is_none() => Some("No database is open"),
            Command::Close => None,
            _ if !open => Some("Open a database first"),
            // Same rule as Ctrl F; otherwise the filter flag is set with nothing to show.
            Command::RefreshTable | Command::Filter if !table => Some("Open a table first"),
            Command::Action(Action::Review) if !self.has_drafts() => {
                Some("No staged edits to review")
            }
            Command::Inspector if self.database.document.inspect.is_none() => {
                Some("Select a cell first")
            }
            Command::CopySelection if self.selected_cells().next().is_none() => {
                Some("Select cells first")
            }
            Command::Begin if self.database.transaction != TransactionState::Autocommit => {
                Some("An editor transaction is already open")
            }
            Command::Action(Action::Commit | Action::Rollback)
                if self.database.transaction != TransactionState::Active =>
            {
                Some("No editor transaction is open")
            }
            Command::Begin | Command::Action(Action::Commit | Action::Rollback)
                if !self.action_enabled(Action::Begin) =>
            {
                busy()
            }
            Command::Action(Action::Insert) if !self.can_edit() => {
                if !self.database.writable {
                    Some("The database is open read-only")
                } else if !table {
                    Some("Open a table first")
                } else if self.database.transaction != TransactionState::Autocommit {
                    Some("Finish the editor transaction first")
                } else {
                    busy()
                }
            }
            Command::Action(Action::FitColumns)
                if self.database.document.result.columns.is_empty() =>
            {
                Some("Open a table first")
            }
            Command::Action(Action::ShowColumns)
                if self.database.document.tools.columns.hidden.is_empty() =>
            {
                Some("No columns are hidden")
            }
            Command::Action(Action::Export) if !self.action_enabled(Action::Export) => {
                Some("Nothing to export yet")
            }
            _ => None,
        }
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
            // An empty selection must not overwrite what is on the clipboard.
            Command::CopySelection if self.selected_cells().next().is_none() => {
                self.database.status = "Select cells to copy".into();
            }
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
            Command::ToggleSidebar => self.toggle_sidebar(),
            Command::ToggleAssistant => self.toggle_assistant(),
            Command::Settings => self.settings.open = true,
            Command::Panel(panel) => {
                self.show_sidebar();
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
    use super::{fuzzy, rank};
    #[test]
    fn palette_ranks_word_starts_before_scattered_letters() {
        // Shortcut text no longer lends letters: "set" is not Ctrl J's
        // "Toggle SQL editor · Ctrl J".
        assert_eq!(rank("Toggle SQL editor", "Ctrl J", "", "set"), None);
        assert_eq!(rank("Settings", "Ctrl ,", "", "set"), Some(0));
        assert_eq!(rank("Saved queries", "", "", "que"), Some(1));
        assert_eq!(rank("Refresh current table", "F5", "", "ref tab"), Some(3));
        assert_eq!(rank("Toggle SQL editor", "Ctrl J", "", "tse"), Some(4));
        assert_eq!(rank("Open record", "Space", "inspector", "insp"), Some(5));
        assert_eq!(rank("Refresh current table", "F5", "", "f5"), Some(6));
        assert_eq!(rank("Toggle SQL editor", "Ctrl J", "", "ctrl j"), Some(6));
        assert_eq!(rank("Close database", "", "", "run"), None);
        assert_eq!(rank("Anything", "", "", "  "), Some(0));
    }
    #[test]
    fn search_matches_subsequences_case_insensitively_and_unicode() {
        assert!(fuzzy("entries.echo_source_id", "E ec src"));
        assert!(fuzzy("λ_table.id", "λid"));
        assert!(!fuzzy("entries", "sqlite"));
    }
}
