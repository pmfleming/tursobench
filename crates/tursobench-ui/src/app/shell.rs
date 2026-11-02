use super::{
    App, View, diagram,
    icons::{self, Icon},
};
use eframe::egui;
use tursobench_core::{Budget, TransactionState};
use tursobench_db::Operation;

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
    Settings,
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
    pub nav: Nav,
    pub table_view: TableView,
    pub inspector: DetailMode,
    pub inspector_open: bool,
    pub inspector_initialized: bool,
    pub inspector_detached: bool,
    pub outcomes: bool,
    pub diagram: diagram::DiagramState,
    pub sql_open: bool,
    pub sql_expanded: bool,
    pub params_open: bool,
    pub filter_open: bool,
    pub palette_open: bool,
    pub palette_focus: bool,
    pub palette_query: String,
    pub palette_index: usize,
    pub export_open: bool,
    pub access_dialog: bool,
    pub access_ack: bool,
    pub font_rx: Option<std::sync::mpsc::Receiver<(egui::FontDefinitions, String)>>,
    pub font_label: String,
    #[cfg(test)]
    pub workspace_rect: egui::Rect,
    #[cfg(test)]
    pub transaction_rects: Vec<egui::Rect>,
    #[cfg(test)]
    pub view_header: egui::Rect,
    #[cfg(test)]
    pub sidebar_rect: egui::Rect,
    #[cfg(test)]
    pub grid_rect: egui::Rect,
    #[cfg(test)]
    pub visible_rows: usize,
}
impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            sidebar: ToolPanel::Schema,
            unfolded: None,
            hover_table: None,
            nav: Nav::Databases,
            table_view: TableView::Data,
            inspector: DetailMode::Cell,
            inspector_open: false,
            inspector_initialized: false,
            inspector_detached: false,
            outcomes: false,
            diagram: Default::default(),
            sql_open: false,
            sql_expanded: false,
            params_open: false,
            filter_open: false,
            palette_open: false,
            palette_focus: false,
            palette_query: String::new(),
            palette_index: 0,
            export_open: false,
            access_dialog: false,
            access_ack: false,
            font_rx: Some(crate::appearance::system_fonts(ctx.clone())),
            font_label: "Loading system fonts; built-in fallback available".into(),
            #[cfg(test)]
            workspace_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            transaction_rects: vec![],
            #[cfg(test)]
            view_header: egui::Rect::NOTHING,
            #[cfg(test)]
            sidebar_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            grid_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            visible_rows: 0,
        }
    }
}
#[derive(Debug)]
pub(super) struct Layout {
    pub bottom_inspector: bool,
    pub sidebar_width: f32,
    pub inspector_width: f32,
}
impl Layout {
    pub fn new(width: f32) -> Self {
        Self {
            bottom_inspector: width < 1440.0,
            sidebar_width: if width < 900.0 { 190.0 } else { 240.0 },
            inspector_width: if width >= 1920.0 { 360.0 } else { 320.0 },
        }
    }
}
impl App {
    pub(super) fn show_structure(&mut self) {
        self.ui.inspector = DetailMode::Structure;
        self.ui.inspector_open = true;
    }
    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        #[cfg(test)]
        {
            self.ui.transaction_rects.clear();
            self.ui.sidebar_rect = egui::Rect::NOTHING;
            self.ui.visible_rows = 0;
        }
        let layout = Layout::new(ui.available_width());
        self.status_bar(ui);
        if self.session.is_some() {
            egui::Panel::left("activity-rail")
                .exact_size(44.0)
                .resizable(false)
                .show(ui, |ui| self.activity_rail(ui));
        }
        {
            let panel = egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(if self.session.is_none() && ui.available_width() >= 900.0 {
                    300.0
                } else {
                    layout.sidebar_width
                })
                .size_range(180.0..=300.0)
                .show(ui, |ui| {
                    if self.session.is_some() {
                        self.navigation_sidebar(ui);
                    } else {
                        ui.heading("Databases");
                        self.database_chooser_ui(ui);
                    }
                });
            #[cfg(test)]
            {
                self.ui.sidebar_rect = panel.response.rect;
            }
            #[cfg(not(test))]
            let _ = panel;
        }
        if self.ui.nav == Nav::Table && !self.ui.inspector_initialized {
            self.ui.inspector_open = self
                .preferences
                .inspector_visibility
                .get(&self.tools.database_scope)
                .copied()
                .unwrap_or(!layout.bottom_inspector);
            self.ui.inspector_initialized = true;
        }
        if self.ui.inspector_open
            && !self.ui.inspector_detached
            && matches!(self.ui.nav, Nav::Table | Nav::Result)
        {
            if layout.bottom_inspector {
                egui::Area::new(egui::Id::new("inspector-overlay"))
                    .order(egui::Order::Foreground)
                    .anchor(egui::Align2::RIGHT_TOP, [-8.0, 8.0])
                    .show(&ctx, |ui| {
                        egui::Frame::popup(ui.style()).show(ui, |ui| {
                            ui.set_width(320.0_f32.min(ctx.content_rect().width() - 60.0));
                            ui.set_max_height((ctx.content_rect().height() - 65.0).max(100.0));
                            self.docked_inspector(ui);
                        });
                    });
            } else {
                egui::Panel::right("inspector-right")
                    .resizable(true)
                    .default_size(layout.inspector_width)
                    .size_range(260.0..=420.0)
                    .show(ui, |ui| self.docked_inspector(ui));
            }
        }
        if self.ui.sql_open {
            let height = if self.ui.sql_expanded {
                ui.available_height() * 0.65
            } else {
                ui.available_height() * 0.30
            };
            egui::Panel::bottom(if self.ui.sql_expanded {
                "sql-editor-expanded"
            } else {
                "sql-editor-dock"
            })
            .resizable(true)
            .default_size(height.max(130.0))
            .size_range(100.0..=(ui.available_height() * 0.75).max(100.0))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .id_salt("sql-dock-scroll")
                    .show(ui, |ui| self.editor_ui(ui));
            });
        }
        egui::CentralPanel::default().show(ui, |ui| self.workspace_ui(ui));
        if self.confirm_close {
            egui::Window::new("Close writable database?").anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).collapsible(false).max_width((ctx.content_rect().width()-48.0).min(520.0)).show(&ctx,|ui|{
                ui.label("Local drafts will be discarded. Active execution will finish. Any remaining editor transaction will be explicitly rolled back. This does not undo committed writes.");
                if ui.button("Wait, roll back transaction, and close").clicked(){self.close();}
                if ui.button("Keep open").clicked(){self.confirm_close=false;self.quit=false;self.pending_database=None;}
            });
        }
        self.edit_windows(&ctx);
        self.inspection_surfaces(&ctx);
        self.command_palette(&ctx);
        self.access_dialog(&ctx);

        if self.ui.outcomes {
            let mut open = true;
            egui::Window::new("Execution outcomes")
                .open(&mut open)
                .default_width(580.0)
                .max_width((ctx.content_rect().width() - 40.0).max(240.0))
                .max_height(ctx.content_rect().height() * 0.65)
                .vscroll(true)
                .show(&ctx, |ui| {
                    ui.label(&self.status);
                    for message in self.activity.iter().rev() {
                        ui.separator();
                        ui.label(message);
                    }
                });
            self.ui.outcomes = open;
        }
        if self.ui.export_open {
            let mut open = true;
            egui::Window::new("Export retained result")
                .open(&mut open)
                .default_width(600.0)
                .max_width((ctx.content_rect().width() - 40.0).max(240.0))
                .max_height(ctx.content_rect().height() * 0.8)
                .vscroll(true)
                .show(&ctx, |ui| self.usability_controls(ui));
            self.ui.export_open = open;
        }
    }
    fn status_bar(&mut self, ui: &mut egui::Ui) {
        egui::Panel::bottom("status").frame(egui::Frame::new().fill(ui.visuals().panel_fill).inner_margin(2.0)).exact_size(32.0).show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                if self.session.is_some() && icons::control(
                    ui, if self.writable { Icon::Edit } else { Icon::Lock },
                    if self.writable { "Editing" } else { "Read-only" }, "", false, true,
                ).on_hover_text("Turso 0.8.3 · FTS · click to close cleanly and change access").clicked() {
                    self.ui.access_dialog = true;
                    self.ui.access_ack = false;
                }
                let warn = ui.visuals().warn_fg_color;
                let error = ui.visuals().error_fg_color;
                let muted = ui.visuals().weak_text_color();
                let (state, color) = if self.uncertain_edit || self.transaction == TransactionState::Unknown {
                    ("UNKNOWN", error)
                } else if self.closing { ("Closing", warn)
                } else if self.commit_ticket.is_some() { ("Committing", warn)
                } else if self.active.is_some() || self.tools.count_request.is_some() { ("Executing", warn)
                } else if self.ui.diagram.ticket.is_some() { ("Loading schema", muted)
                } else if !self.ready { ("Disconnected", muted)
                } else if let Some(c) = &self.result.completion {
                    if c.error.is_some() { ("Failed", error) } else {
                        match c.delivery {
                            tursobench_core::Delivery::Limited => ("Limited", warn),
                            tursobench_core::Delivery::Discarded => ("Discarded", warn),
                            tursobench_core::Delivery::Complete => ("Connected", ui.visuals().selection.stroke.color),
                        }
                    }
                } else { ("Connected", ui.visuals().selection.stroke.color) };
                ui.colored_label(color, state).on_hover_text(&self.status);
                if self.transaction != TransactionState::Autocommit {
                    ui.colored_label(if self.transaction != TransactionState::Autocommit { warn } else { muted }, format!("{:?}", self.transaction));
                    for (label, sql) in [("Commit", "COMMIT"), ("Roll back", "ROLLBACK")] {
                        let enabled = self.ready && !self.closing && self.active.is_none() && self.commit_ticket.is_none() && !self.has_drafts();
                        let response = ui.add_enabled(enabled, egui::Button::new(label)).on_hover_text("Persistent editor connection. Transactions also apply to read-only sessions.");
                        #[cfg(test)] self.ui.transaction_rects.push(response.rect);
                        if response.clicked() { self.submit(Operation::Execute { sql: sql.into(), parameters: vec![], budget: Budget::default() }, View::Query); }
                    }
                }
                if self.has_drafts() {
                    let count = self.drafts.as_ref().map_or(0, |d| d.edits.len());
                    if ui.button(format!("Review {count}")).clicked() { self.show_preview = true; }
                }
                if self.active.is_some() && ui.button("Discard results").on_hover_text("Suppresses delivery only; execution and writes continue. Does not roll back.").clicked() {
                    if let (Some(session), Some(ticket)) = (&self.session, self.active) { session.discard_results(ticket.request); }
                    self.discard_ticket = self.active;
                    self.result.rows.clear();
                    self.status = "Delivery suppressed; awaiting execution outcome".into();
                }
                let summary=self.selection_summary();
                if !summary.is_empty() && ui.available_width()>160.0 {ui.add(egui::Label::new(summary).truncate());}
                if let Some(c) = &self.result.completion && ui.available_width() > 200.0 {
                    ui.small(format!("{:?} · {} ms", c.delivery, c.elapsed_ms)).on_hover_text(self.export_scope());
                }
                if icons::control(ui, Icon::History, "Outcomes", "", self.ui.outcomes, false).clicked() { self.ui.outcomes = !self.ui.outcomes; }

            });
        });
    }
    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        {
            self.ui.workspace_rect = ui.available_rect_before_wrap();
        }
        if self.session.is_none() && self.ui.nav == Nav::Databases {
            self.connect_ui(ui);
            return;
        }
        if self.session.is_some() && !self.ready {
            ui.spinner();
            ui.label("Opening database…");
            return;
        }
        if self.session.is_none() {
            ui.horizontal(|ui| {
                if icons::control(ui, Icon::Database, "Databases", "Ctrl O", false, true).clicked()
                {
                    self.ui.nav = Nav::Databases;
                }
                ui.label("Retained data · disconnected")
                    .on_hover_text(&self.tools.database_scope);
            });
        }
        if self.ui.nav == Nav::Settings {
            egui::ScrollArea::vertical().show(ui, |ui| self.settings_ui(ui));
            return;
        }
        self.workspace_header(ui);
        if self.ui.nav == Nav::Overview {
            self.diagram_ui(ui);
            return;
        }
        if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Structure {
            egui::ScrollArea::vertical()
                .id_salt("table-structure")
                .show(ui, |ui| self.structure_inspector(ui));
        } else {
            self.grid_contents(ui);
        }
    }
    fn workspace_header(&mut self, ui: &mut egui::Ui) {
        let header = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if self.ui.nav == Nav::Overview {
                ui.strong("Map");
                return;
            }
            let can_back = !self.tools.back.is_empty() || self.tools.result_return.is_some();
            if ui
                .add_enabled_ui(can_back, |ui| {
                    icons::button(ui, Icon::Back, "Back", "Alt Left")
                })
                .inner
                .clicked()
            {
                if self.view == View::Browse {
                    self.navigate_back();
                } else {
                    self.return_to_table();
                }
            }
            let title = self.result_title().to_owned();
            ui.add_sized(
                [
                    if ui.available_width() < 540.0 {
                        55.0
                    } else {
                        120.0
                    },
                    28.0,
                ],
                egui::Label::new(egui::RichText::new(&title).strong()).truncate(),
            )
            .on_hover_text(title);
            if self.ui.nav == Nav::Table {
                for (view, label) in [
                    (TableView::Data, "Data"),
                    (TableView::Structure, "Structure"),
                ] {
                    ui.selectable_value(&mut self.ui.table_view, view, label);
                }
            }
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let more = icons::button(ui, Icon::More, "More actions", "");
                egui::Popup::menu(&more).show(|ui| {
                    if icons::control(
                        ui,
                        Icon::Export,
                        "Export retained result…",
                        "Ctrl E",
                        false,
                        true,
                    )
                    .clicked()
                    {
                        self.ui.export_open = true;
                        ui.close();
                    }
                    if ui.button("Copy all retained rows as TSV").clicked() {
                        self.copy_results(ui.ctx(), crate::export::Format::Tsv);
                        ui.close();
                    }
                    if ui.button("Show hidden columns").clicked() {
                        self.tools.columns.hidden.clear();
                        ui.close();
                    }
                    if ui.button("Fit all column widths").clicked() {
                        self.tools.columns.manual.clear();
                        self.tools.columns.refit = true;
                        ui.close();
                    }
                    if self.has_drafts() && ui.button("Show staged data").clicked() {
                        self.ui.nav = Nav::Table;
                        self.ui.table_view = TableView::Data;
                        ui.close();
                    }
                });
                if icons::control(
                    ui,
                    Icon::Columns,
                    "Inspector",
                    "Space",
                    self.ui.inspector_open,
                    false,
                )
                .clicked()
                {
                    self.toggle_inspector();
                }
                if ui
                    .add_enabled_ui(
                        self.view == View::Browse
                            && self.ready
                            && !self.closing
                            && !self.has_drafts()
                            && self.commit_ticket.is_none(),
                        |ui| icons::button(ui, Icon::Refresh, "Refresh table", "F5"),
                    )
                    .inner
                    .clicked()
                {
                    self.reload();
                }
                if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Data {
                    self.page_controls(ui);
                    if ui.available_width() >= 120.0 {
                        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
                            self.filter_controls(ui)
                        });
                    } else {
                        let filter = icons::control(
                            ui,
                            Icon::Filter,
                            "Filter rows",
                            "Ctrl F",
                            self.browse.as_ref().is_some_and(|b| !b.filters.is_empty()),
                            false,
                        );
                        if self.ui.filter_open {
                            egui::Popup::open_id(
                                ui.ctx(),
                                egui::Popup::default_response_id(&filter),
                            );
                            self.ui.filter_open = false;
                        }
                        egui::Popup::menu(&filter).show(|ui| {
                            ui.set_min_width(300.0);
                            self.filter_controls(ui);
                        });
                    }
                }
            });
        });
        #[cfg(test)]
        {
            self.ui.view_header = header.response.rect;
        }
        #[cfg(not(test))]
        let _ = header;
        ui.separator();
    }
    pub(super) fn toggle_inspector(&mut self) {
        self.ui.inspector_open = !self.ui.inspector_open;
        self.ui.inspector_initialized = true;
        self.preferences
            .inspector_visibility
            .insert(self.tools.database_scope.clone(), self.ui.inspector_open);
        self.preferences.normalize();
        self.store.save(&self.preferences);
    }
    fn page_controls(&mut self, ui: &mut egui::Ui) {
        let Some(b) = self.browse.clone() else {
            ui.small("Lookup ≤100")
                .on_hover_text("Bound related-row lookup. Not a complete table.");
            return;
        };
        if let Some(n) = self.tools.counts.get(&b.table) {
            ui.small(format!("of {n} total"));
        } else if icons::control(ui, Icon::Count, "?", "", false, true)
            .on_hover_text("Count all table rows (full scan), independent of filters")
            .clicked()
        {
            self.count_table(b.table.clone());
        }
        ui.small(format!("{}–{}",b.offset+usize::from(!self.result.rows.is_empty()),b.offset+self.result.rows.len())).on_hover_text("100 rows per fetch · retained window · OFFSET is not a snapshot · scroll back to reload evicted pages");
    }
    fn result_title(&self) -> &str {
        match self.view {
            View::Browse => self.table.as_ref().map_or_else(
                || self.ui.diagram.selected.as_deref().unwrap_or("Table data"),
                |t| t.name.as_str(),
            ),
            View::Explain => "EXPLAIN result",
            View::Diagnostics => "Diagnostic result",
            View::Query => "Query result",
        }
    }
    fn docked_inspector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong(if self.ui.inspector == DetailMode::Structure {
                "Structure"
            } else {
                "Row detail"
            });
            if self.ui.inspector != DetailMode::Structure {
                if icons::control(
                    ui,
                    Icon::Edit,
                    "Typed cell",
                    "",
                    self.ui.inspector == DetailMode::Cell,
                    false,
                )
                .clicked()
                {
                    self.ui.inspector = DetailMode::Cell;
                }
                if icons::control(
                    ui,
                    Icon::Structure,
                    "Record",
                    "",
                    self.ui.inspector == DetailMode::Record,
                    false,
                )
                .clicked()
                {
                    self.ui.inspector = DetailMode::Record;
                }
            }
            if icons::button(ui, Icon::Popout, "Open inspector in window", "").clicked() {
                self.ui.inspector_detached = true;
            }
            if icons::button(ui, Icon::Close, "Close inspector", "Esc").clicked() {
                self.toggle_inspector();
            }
        });
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("docked-inspector-content")
            .show(ui, |ui| self.inspector_content(ui));
    }
    pub(super) fn inspector_content(&mut self, ui: &mut egui::Ui) {
        match self.ui.inspector {
            DetailMode::Cell => {
                if let Some((r, c)) = self.inspect {
                    self.cell_inspector(ui, r, c);
                } else {
                    ui.label("Select a cell to inspect or edit its typed value.");
                }
            }
            DetailMode::Record => {
                if let Some(r) = self.tools.detail.or(self.inspect.map(|(r, _)| r)) {
                    self.record_inspector(ui, r);
                } else {
                    ui.label("Select a row for record detail.");
                }
            }
            DetailMode::Structure => self.structure_inspector(ui),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sidebar_and_inspector_leave_room_for_content() {
        for width in [720.0, 960.0, 1280.0, 1366.0, 1440.0, 1920.0, 3440.0] {
            let l = Layout::new(width);
            let used = l.sidebar_width
                + if l.bottom_inspector {
                    0.0
                } else {
                    l.inspector_width
                };
            assert!(width - used >= 500.0, "{width}: {l:?}");
        }
    }
}
