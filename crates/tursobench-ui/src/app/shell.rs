use super::presentation::{self, Intent};
use super::{
    App, View, diagram,
    icons::{self, Icon},
    ribbon::Action,
};
use eframe::egui;
use tursobench_core::TransactionState;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum ToolPanel {
    #[default]
    Schema,
    History,
    Saved,
    Diagnostics,
    Settings,
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
                let panes = super::tiling::Kind::PANES;
                let index = panes
                    .iter()
                    .position(|kind| *kind == self.database.workspace.active)
                    .unwrap_or(0);
                self.focus_pane(panes[(index + 1) % panes.len()]);
            }
        }
        self.shortcuts(&ctx);
        #[cfg(test)]
        {
            self.database.ui.transaction_rects.clear();
            self.database.ui.ribbon_actions.clear();
            self.database.ui.sidebar_rect = egui::Rect::NOTHING;
            self.database.ui.visible_rows = 0;
        }
        let sidebar_width = if ui.available_width() < 900.0 {
            190.0
        } else {
            272.0
        };
        // Reserve full-height navigation before the workspace ribbon and status bar.
        {
            let panel = egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(sidebar_width)
                .size_range(180.0..=(ui.available_width() - 360.0).clamp(180.0, 600.0))
                .show(ui, |ui| {
                    egui::Panel::bottom("navigator-utilities")
                        .resizable(false)
                        .show(ui, |ui| {
                            if self.database.session.is_some() {
                                ui.small("Tools apply to the active database");
                            }
                            self.navigator_utilities(ui);
                        });
                    self.navigation_sidebar(ui);
                });
            #[cfg(test)]
            {
                self.database.ui.sidebar_rect = panel.response.rect;
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
        self.discovery_settings_window(&ctx);

        if self.database.ui.outcomes {
            let mut open = true;
            egui::Window::new("Execution outcomes")
                .frame(crate::app::presentation::window_frame(&ctx))
                .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 180.0))
                .open(&mut open)
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
            egui::Window::new("Export retained result")
                .frame(crate::app::presentation::window_frame(&ctx))
                .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 180.0))
                .open(&mut open)
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
                        self.status_details(ui)
                    });
                });
            });
    }
    fn connection_controls(&mut self, ui: &mut egui::Ui) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if self.database.session.is_some()
            && presentation::chip(
                ui,
                if self.database.writable {
                    "Editing"
                } else {
                    "Read-only"
                },
                if self.database.writable {
                    tokens.pending
                } else {
                    tokens.muted
                },
            )
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
            "Outcomes",
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
            .on_hover_text(self.export_scope());
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
#[derive(Default)]
pub(super) struct ViewUi {
    pub nav: Nav,
    pub table_view: TableView,
    pub inspector: DetailMode,
    pub inspector_open: bool,
    pub inspector_detached: bool,
    pub sql_open: bool,
    pub params_open: bool,
    pub filter_open: bool,
}
