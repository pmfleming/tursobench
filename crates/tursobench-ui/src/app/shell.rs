use super::presentation::{self, Intent};
use super::tiling::{Kind, Layout};
use super::{
    App, View, diagram,
    icons::{self, Icon},
    ribbon::Action,
};
use eframe::egui;
use std::collections::BTreeSet;
use tursobench_core::TransactionState;
/// Space around a collapsed panel's line.
const FOLD_MARGIN: i8 = 4;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum ToolPanel {
    #[default]
    Schema,
    History,
    Saved,
    Diagnostics,
}
/// Presentation routes never own or duplicate the retained engine result.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum Nav {
    #[default]
    Databases,
    Overview,
    Table,
    Result,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum DetailMode {
    #[default]
    Cell,
    Record,
    Structure,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum TableView {
    #[default]
    Data,
    Structure,
}

pub(super) struct UiState {
    pub sidebar: ToolPanel,
    pub unfolded: Option<String>,
    pub hover_table: Option<String>,
    pub outcomes: bool,
    pub diagram: diagram::DiagramState,
    pub palette_open: bool,
    pub palette_focus: bool,
    pub palette_query: String,
    pub palette_index: usize,
    pub export_open: bool,
    pub access_dialog: bool,
    pub font_rx: Option<std::sync::mpsc::Receiver<(egui::FontDefinitions, String)>>,
    pub font_label: String,
    #[cfg(test)]
    pub workspace_rect: egui::Rect,
    #[cfg(test)]
    pub transaction_rects: Vec<egui::Rect>,
    #[cfg(test)]
    pub sidebar_rect: egui::Rect,
    #[cfg(test)]
    pub assistant_rect: egui::Rect,
    #[cfg(test)]
    pub grid_rect: egui::Rect,
    #[cfg(test)]
    pub visible_rows: usize,
    #[cfg(test)]
    pub ribbon_actions: Vec<egui::Rect>,
}
impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            sidebar: ToolPanel::Schema,
            unfolded: None,
            hover_table: None,
            outcomes: false,
            diagram: Default::default(),
            palette_open: false,
            palette_focus: false,
            palette_query: String::new(),
            palette_index: 0,
            export_open: false,
            access_dialog: false,
            font_rx: Some(crate::appearance::system_fonts(ctx.clone())),
            font_label: "Loading system fonts; built-in fallback available".into(),
            #[cfg(test)]
            workspace_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            transaction_rects: vec![],
            #[cfg(test)]
            sidebar_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            assistant_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            grid_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            visible_rows: 0,
            #[cfg(test)]
            ribbon_actions: vec![],
        }
    }
}
impl App {
    pub(super) fn diagram_ui(&mut self, ui: &mut egui::Ui) {
        let relatives = self
            .database
            .ui
            .hover_table
            .as_deref()
            .map(|t| self.related_tables(t))
            .unwrap_or_default();
        if let Some(name) = self.database.ui.diagram.show(ui, &relatives) {
            self.browse_table(name);
        }
    }

    pub(super) fn show_structure(&mut self) {
        self.open_structure_window();
    }
    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if self.database.workspace.enabled {
            self.switch_document(self.database.workspace.focused);
            if ctx.input_mut(|i| {
                i.consume_key(
                    egui::Modifiers::CTRL | egui::Modifiers::ALT,
                    egui::Key::Enter,
                )
            }) {
                self.expand_window(self.database.workspace.focused);
            }
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Tab)) {
                // Left to right, top to bottom. A hidden assistant stays hidden.
                let workspace = &self.database.workspace;
                let mut panes = workspace.layout.order();
                panes.retain(|kind| {
                    *kind != super::tiling::Kind::Chat || !workspace.minimized.contains(kind)
                });
                let index = panes
                    .iter()
                    .position(|kind| *kind == workspace.active)
                    .map_or(0, |index| index + 1);
                self.focus_pane(panes[index % panes.len()]);
            }
        }
        self.shortcuts(&ctx);
        #[cfg(test)]
        {
            self.database.ui.transaction_rects.clear();
            self.database.ui.ribbon_actions.clear();
            self.database.ui.sidebar_rect = egui::Rect::NOTHING;
            self.database.ui.assistant_rect = egui::Rect::NOTHING;
            self.database.ui.visible_rows = 0;
        }
        let sidebar_width = if ui.available_width() < 900.0 {
            190.0
        } else {
            272.0
        };
        // A close confirmation lives in the sidebar, so it must stay reachable.
        let sidebar = self.preferences.panels.sidebar || self.database.confirm_close;
        // Reserve full-height navigation before the workspace ribbon and status bar.
        if sidebar {
            let panel = egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(sidebar_width)
                .size_range(180.0..=(ui.available_width() - 360.0).clamp(180.0, 600.0))
                .show(ui, |ui| {
                    egui::Panel::bottom("navigator-utilities")
                        .resizable(false)
                        .show(ui, |ui| {
                            self.application_controls(ui);
                        });
                    self.navigation_sidebar(ui);
                });
            #[cfg(test)]
            {
                self.database.ui.sidebar_rect = panel.response.rect;
            }
            #[cfg(not(test))]
            let _ = panel;
        } else if self.fold_panel(
            ui,
            egui::Panel::left("workspace-sidebar-line"),
            "Show databases",
        ) {
            self.toggle_sidebar();
        }
        // With a database open the assistant is a tile among the others. Until
        // then it keeps the right edge, beside the database chooser.
        if self.workspace_tiled() {
            // Drawn with the tiles in the central panel below.
        } else if !self.preferences.panels.assistant {
            if self.fold_panel(
                ui,
                egui::Panel::right("assistant-panel-line"),
                "Show Assistant",
            ) {
                self.toggle_assistant();
            }
        } else {
            let max = (ui.available_width() - 360.0).clamp(260.0, 720.0);
            let panel = egui::Panel::right("assistant-panel")
                .resizable(true)
                .default_size(372.0_f32.min(max))
                .size_range(260.0..=max)
                .show(ui, |ui| self.assistant_ui(ui));
            #[cfg(test)]
            {
                self.database.ui.assistant_rect = panel.response.rect;
            }
            #[cfg(not(test))]
            let _ = panel;
        }
        self.status_bar(ui);
        egui::CentralPanel::default().show(ui, |ui| self.workspace_ui(ui));
        self.edit_windows(&ctx);
        self.inspection_surfaces(&ctx);
        self.command_palette(&ctx);
        self.access_dialog(&ctx);
        self.settings_window(&ctx);

        if self.database.ui.outcomes {
            let mut open = true;
            presentation::dialog(&ctx, "Execution outcomes", &mut open)
                .default_width(580.0)
                .max_width((ctx.content_rect().width() - 40.0).max(240.0))
                .max_height(ctx.content_rect().height() * 0.65)
                .vscroll(true)
                .show(&ctx, |ui| {
                    presentation::notice(
                        ui,
                        Icon::History,
                        if self.database.uncertain_edit
                            || self.database.transaction == TransactionState::Unknown
                        {
                            Intent::Danger
                        } else {
                            Intent::Neutral
                        },
                        &self.database.status,
                    );
                    for message in self.database.activity.iter().rev() {
                        ui.separator();
                        ui.label(message);
                    }
                });
            self.database.ui.outcomes = open;
        }
        if self.database.ui.export_open {
            let mut open = true;
            presentation::dialog(&ctx, "Export retained result", &mut open)
                .default_width(600.0)
                .default_height(200.0)
                .max_width((ctx.content_rect().width() - 40.0).max(240.0))
                .max_height(ctx.content_rect().height() * 0.8)
                .vscroll(true)
                .show(&ctx, |ui| self.usability_controls(ui));
            self.database.ui.export_open = open;
        }
    }
    fn connection_status(&self, visuals: &egui::Visuals) -> (&'static str, egui::Color32) {
        let warn = visuals.warn_fg_color;
        let error = visuals.error_fg_color;
        let muted = visuals.weak_text_color();
        // First active condition wins: execution state precedes retained-result state.
        if let Some((_, label, color)) = [
            (
                self.database.uncertain_edit
                    || self.database.transaction == TransactionState::Unknown,
                "UNKNOWN",
                error,
            ),
            (self.database.closing, "Closing", warn),
            (self.database.commit_ticket.is_some(), "Committing", warn),
            (
                self.executing() || self.database.tools.count_request.is_some(),
                "Executing",
                warn,
            ),
            (
                self.database.ui.diagram.ticket.is_some(),
                "Loading schema",
                muted,
            ),
            (!self.database.ready, "Disconnected", muted),
        ]
        .into_iter()
        .find(|(active, _, _)| *active)
        {
            return (label, color);
        }
        if let Some(c) = &self.database.document.result.completion {
            if c.error.is_some() {
                ("Failed", error)
            } else {
                match c.delivery {
                    tursobench_core::Delivery::Limited => ("Limited", warn),
                    tursobench_core::Delivery::Discarded => ("Discarded", warn),
                    tursobench_core::Delivery::Complete => ("Connected", visuals.hyperlink_color),
                }
            }
        } else {
            ("Connected", visuals.hyperlink_color)
        }
    }
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let fill = if self.database.uncertain_edit
            || self.database.transaction == TransactionState::Unknown
        {
            crate::appearance::mix(ui.visuals().panel_fill, ui.visuals().error_fg_color, 0.13)
        } else {
            ui.visuals().panel_fill
        };
        egui::Panel::bottom("status")
            .frame(
                egui::Frame::new()
                    .fill(fill)
                    .inner_margin(egui::Margin::symmetric(8, 2)),
            )
            .exact_size(34.0)
            .show(ui, |ui| {
                ui.painter().hline(
                    ui.max_rect().x_range(),
                    ui.max_rect().top() - 2.0,
                    egui::Stroke::new(1.0, tokens.line),
                );
                ui.spacing_mut().interact_size.y = 24.0;
                ui.spacing_mut().button_padding = egui::vec2(8.0, 4.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    self.panel_toggles(ui);
                    self.connection_controls(ui);
                    self.transaction_controls(ui);
                    if self.has_drafts() {
                        let count: usize = self
                            .database
                            .documents
                            .values()
                            .chain(std::iter::once(&self.database.document))
                            .filter_map(|d| d.drafts.as_ref())
                            .map(|d| d.edits.len())
                            .sum();
                        if presentation::chip(ui, &format!("Review {count}"), tokens.pending)
                            .clicked()
                        {
                            self.dispatch_action(Action::Review, ui.ctx());
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if icons::control(
                            ui,
                            Icon::Assistant,
                            "Assistant",
                            "Ctrl I",
                            self.assistant_shown(),
                            false,
                        )
                        .clicked()
                        {
                            self.toggle_assistant();
                        }
                        if self.workspace_tiled() {
                            self.column_map(ui);
                        }
                        ui.separator();
                        self.status_details(ui)
                    });
                });
            });
    }
    /// Always visible, so a hidden sidebar or assistant can be brought back.
    fn panel_toggles(&mut self, ui: &mut egui::Ui) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let panels = self.preferences.panels;
            if icons::control(
                ui,
                Icon::Sidebar,
                "Sidebar",
                "Ctrl B",
                panels.sidebar,
                false,
            )
            .clicked()
            {
                self.toggle_sidebar();
            }
        });
        ui.separator();
    }
    /// A collapsed side panel: a full-height line that reopens it on click.
    fn fold_panel(&mut self, ui: &mut egui::Ui, panel: egui::Panel, name: &str) -> bool {
        panel
            .resizable(false)
            .show_separator_line(false)
            .exact_size(super::tiling::LINE + 2.0 * f32::from(FOLD_MARGIN))
            .frame(
                egui::Frame::side_top_panel(ui.style())
                    .inner_margin(egui::Margin::same(FOLD_MARGIN)),
            )
            .show(ui, |ui| {
                let rect = ui.available_rect_before_wrap();
                presentation::fold_line(ui, rect, name, name).clicked()
            })
            .inner
    }
    pub(super) fn toggle_sidebar(&mut self) {
        self.preferences.panels.sidebar = !self.preferences.panels.sidebar;
        self.save_panels();
    }
    /// Whether the tiles are on screen, rather than the database chooser or
    /// a database that is still opening.
    pub(super) fn workspace_tiled(&self) -> bool {
        match self.database.session {
            None => self.database.document.ui.nav != Nav::Databases,
            Some(_) => self.database.ready,
        }
    }
    /// The assistant is a tile; hidden, it is minimized. Before a database is
    /// open it is a side panel that remembers the same choice.
    pub(super) fn assistant_shown(&self) -> bool {
        if self.workspace_tiled() {
            !self
                .database
                .workspace
                .minimized
                .contains(&super::tiling::Kind::Chat)
        } else {
            self.preferences.panels.assistant
        }
    }
    pub(super) fn set_assistant_shown(&mut self, show: bool) {
        use super::tiling::Kind;
        self.preferences.panels.assistant = show;
        let workspace = &mut self.database.workspace;
        if show {
            workspace.minimized.remove(&Kind::Chat);
            self.assistant_focus();
        } else {
            workspace.minimized.insert(Kind::Chat);
            if workspace.expanded == Some(Kind::Chat) {
                workspace.expanded = None;
            }
        }
        self.save_panels();
        self.save_workspace();
    }
    pub(super) fn toggle_assistant(&mut self) {
        self.set_assistant_shown(!self.assistant_shown());
    }
    /// Whether the chat fills the workspace, like any expanded tile.
    pub(super) fn assistant_expanded(&self) -> bool {
        self.workspace_tiled()
            && self.database.workspace.expanded == Some(super::tiling::Kind::Chat)
    }
    /// Expand the chat to the workspace, or give the workspace back to the tiles.
    pub(super) fn toggle_assistant_expanded(&mut self) {
        self.expand_pane(super::tiling::Kind::Chat);
        if self.assistant_expanded() {
            self.assistant_focus();
        }
    }
    /// A small picture of the columns in the status bar. Each column folds to
    /// a line, or reopens, on a click.
    fn column_map(&mut self, ui: &mut egui::Ui) {
        let workspace = &self.database.workspace;
        let folds = workspace.layout.folds(&workspace.minimized);
        let mut toggled = None;
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            // The bar fills right to left, so the last column is added first.
            for (column, folded) in folds.into_iter().enumerate().rev() {
                let tiles = (&workspace.layout, &workspace.minimized, workspace.active);
                if column_map_button(ui, tiles, column, folded) {
                    toggled = Some(column);
                }
            }
        });
        if let Some(column) = toggled {
            self.toggle_column(column);
        }
    }
    fn save_panels(&mut self) {
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }
    /// Searching needs the sidebar, so the search shortcuts reveal it.
    pub(super) fn show_sidebar(&mut self) {
        if !self.preferences.panels.sidebar {
            self.toggle_sidebar();
        }
    }
    fn connection_controls(&mut self, ui: &mut egui::Ui) {
        if self.database.session.is_some()
            && icons::access_button(ui, self.database.writable, None)
                .on_hover_text(format!(
                    "Turso 0.8.3 · {} · click to close cleanly and change access",
                    self.database
                        .coordination
                        .filter(|_| self.database.ready)
                        .map_or("Connecting", |c| c.label())
                ))
                .clicked()
        {
            self.database.ui.access_dialog = true;
        }
        let (state, color) = self.connection_status(ui.visuals());
        let (dot, _) = ui.allocate_exact_size(egui::vec2(7.0, 7.0), egui::Sense::hover());
        ui.painter().circle_filled(dot.center(), 3.0, color);
        ui.colored_label(color, state)
            .on_hover_text(&self.database.status);
    }
    fn transaction_controls(&mut self, ui: &mut egui::Ui) {
        if self.database.transaction == TransactionState::Autocommit {
            return;
        }
        let color = if self.database.uncertain_edit
            || self.database.transaction == TransactionState::Unknown
        {
            ui.visuals().error_fg_color
        } else {
            crate::appearance::Tokens::of(ui.ctx()).pending
        };
        let transaction =
            presentation::chip(ui, &format!("    {:?}", self.database.transaction), color);
        let animate = ui.style().animation_time > 0.0;
        let pulse = if animate {
            ui.input(|i| 0.6 + 0.4 * (i.time as f32 * 3.0).sin().abs())
        } else {
            1.0
        };
        ui.painter().circle_filled(
            transaction.rect.left_center() + egui::vec2(10.0, 0.0),
            3.0,
            color.gamma_multiply(pulse),
        );
        transaction.on_hover_text("Persistent editor connection transaction");
        if animate {
            ui.ctx()
                .request_repaint_after(std::time::Duration::from_millis(80));
        }
        for (label, action) in [("Commit", Action::Commit), ("Roll back", Action::Rollback)] {
            let response = ui
                .add_enabled(self.action_enabled(action), egui::Button::new(label))
                .on_hover_text(
                    "Persistent editor connection. Transactions also apply to read-only sessions.",
                );
            #[cfg(test)]
            self.database.ui.transaction_rects.push(response.rect);
            if response.clicked() {
                self.dispatch_action(action, ui.ctx());
            }
        }
    }
    fn status_details(&mut self, ui: &mut egui::Ui) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if icons::control(
            ui,
            Icon::History,
            "Execution outcomes and recent activity",
            "",
            self.database.ui.outcomes,
            false,
        )
        .clicked()
        {
            self.database.ui.outcomes = !self.database.ui.outcomes;
        }
        if let Some(c) = &self.database.document.result.completion
            && ui.available_width() > 200.0
        {
            ui.add(egui::Label::new(
                egui::RichText::new(format!("{:?} · {} ms", c.delivery, c.elapsed_ms))
                    .monospace()
                    .small()
                    .color(tokens.faint),
            ))
            .on_hover_text(format!(
                "Last result in the focused tile: {} · {} ms\nExport scope: {}",
                format!("{:?}", c.delivery).to_lowercase(),
                c.elapsed_ms,
                self.export_scope()
            ));
        }
        let summary = self.selection_summary();
        if !summary.is_empty() && ui.available_width() > 100.0 {
            ui.allocate_ui_with_layout(
                egui::vec2(ui.available_width(), 24.0),
                egui::Layout::left_to_right(egui::Align::Center)
                    .with_main_align(egui::Align::Center),
                |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(summary).small().color(tokens.muted))
                            .truncate(),
                    );
                },
            );
        }
    }
    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        {
            self.database.ui.workspace_rect = ui.available_rect_before_wrap();
        }
        if self.database.session.is_none() && self.database.document.ui.nav == Nav::Databases {
            self.connect_ui(ui);
            return;
        }
        if self.database.session.is_some() && !self.database.ready {
            ui.spinner();
            ui.label("Opening database…");
            return;
        }
        if self.database.session.is_none() {
            ui.horizontal(|ui| {
                if icons::control(ui, Icon::Database, "Databases", "Ctrl O", false, true).clicked()
                {
                    self.database.document.ui.nav = Nav::Databases;
                }
                ui.label("Retained data · disconnected")
                    .on_hover_text(&self.database.tools.database_scope);
            });
        }
        self.tiled_workspace(ui);
    }
    pub(super) fn toggle_inspector(&mut self) {
        if let Some((r, c)) = self.database.document.inspect {
            self.open_record_window(r, c);
        } else {
            self.database.status = "Select a cell to open its record".into();
        }
    }
    pub(super) fn refresh_table(&mut self) {
        if self.database.document.view != View::Browse
            || self.database.document.ui.nav != Nav::Table
        {
            return;
        }
        if self.database.document.ui.table_view == TableView::Structure {
            if let Some(name) = self.database.ui.diagram.selected.clone() {
                self.request_inspection(name);
            }
            self.refresh_catalog();
        } else {
            self.reload();
        }
    }
    pub(super) fn inspector_content(&mut self, ui: &mut egui::Ui) {
        match self.database.document.ui.inspector {
            DetailMode::Cell => {
                if let Some((r, c)) = self.database.document.inspect {
                    self.cell_inspector(ui, r, c);
                } else {
                    ui.label("Select a cell to inspect or edit its typed value.");
                }
            }
            DetailMode::Record => {
                if let Some(r) = self.database.document.tools.detail.or(self
                    .database
                    .document
                    .inspect
                    .map(|(r, _)| r))
                {
                    self.record_inspector(ui, r);
                } else {
                    ui.label("Select a row for record detail.");
                }
            }
            DetailMode::Structure => self.structure_inspector(ui),
        }
    }
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Hash, Debug)]
pub(super) enum StructureSection {
    #[default]
    Columns,
    Indexes,
    ForeignKeys,
    Triggers,
    Ddl,
}
#[derive(Default)]
pub(super) struct ViewUi {
    pub structure_section: StructureSection,
    pub nav: Nav,
    pub table_view: TableView,
    pub inspector: DetailMode,
    pub inspector_open: bool,
    pub inspector_detached: bool,
    pub sql_open: bool,
    pub params_open: bool,
    pub filter_open: bool,
}

/// One column in the status bar's column map: a line when folded, otherwise
/// its tiles stacked as bars. Returns whether it was clicked.
fn column_map_button(
    ui: &mut egui::Ui,
    (layout, minimized, active): (&Layout, &BTreeSet<Kind>, Kind),
    column: usize,
    folded: bool,
) -> bool {
    const HEIGHT: f32 = 18.0;
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    let names = layout.names(column);
    let (rect, response) = ui.allocate_exact_size(
        egui::vec2(if folded { 6.0 } else { 12.0 }, 24.0),
        egui::Sense::click(),
    );
    response.widget_info(|| {
        egui::WidgetInfo::selected(
            egui::Role::Button,
            ui.is_enabled(),
            !folded,
            format!("Column: {names}"),
        )
    });
    let base = if response.hovered() {
        ui.visuals().text_color()
    } else {
        tokens.muted
    };
    let picture = egui::Rect::from_center_size(
        rect.center(),
        egui::vec2(if folded { 3.0 } else { rect.width() }, HEIGHT),
    );
    if folded {
        ui.painter().rect_filled(picture, 1.5, base);
    } else {
        let tiles = &layout.columns[column].tiles;
        let small = |kind: &Kind| minimized.contains(kind);
        let gaps = 2.0 * (tiles.len() - 1) as f32;
        let bars = 2.0 * tiles.iter().filter(|kind| small(kind)).count() as f32;
        let open = tiles.iter().filter(|kind| !small(kind)).count().max(1) as f32;
        let cell = ((HEIGHT - gaps - bars) / open).max(2.0);
        let mut y = picture.top();
        for kind in tiles {
            let height = if small(kind) { 2.0 } else { cell };
            let color = if *kind == active && !small(kind) {
                tokens.primary
            } else {
                base
            };
            let bar = egui::Rect::from_x_y_ranges(picture.x_range(), y..=y + height);
            ui.painter().rect_filled(bar, 1.5, color);
            y += height + 2.0;
        }
    }
    let hint = if folded { "Show" } else { "Fold" };
    response
        .on_hover_text(format!(
            "{hint} {names}\nCtrl L folds the focused tile's column"
        ))
        .on_hover_cursor(egui::CursorIcon::PointingHand)
        .clicked()
}
