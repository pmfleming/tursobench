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
    pub sidebar_open: bool,
    pub unfolded: Option<String>,
    pub hover_table: Option<String>,
    pub nav: Nav,
    pub table_view: TableView,
    pub inspector: DetailMode,
    pub inspector_open: bool,
    pub inspector_detached: bool,
    pub outcomes: bool,
    pub diagram: diagram::DiagramState,
    pub map_list: bool,
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
            sidebar_open: false,
            unfolded: None,
            hover_table: None,
            nav: Nav::Databases,
            table_view: TableView::Data,
            inspector: DetailMode::Cell,
            inspector_open: false,
            inspector_detached: false,
            outcomes: false,
            diagram: Default::default(),
            map_list: false,
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
            bottom_inspector: width < 1400.0,
            sidebar_width: if width < 900.0 { 190.0 } else { 240.0 },
            inspector_width: if width >= 1800.0 { 370.0 } else { 310.0 },
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
            let panel = egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(layout.sidebar_width)
                .size_range(180.0..=300.0)
                .show(ui, |ui| self.navigation_sidebar(ui));
            #[cfg(test)]
            {
                self.ui.sidebar_rect = panel.response.rect;
            }
            #[cfg(not(test))]
            let _ = panel;
        }
        if self.ui.sql_open {
            let height = if self.ui.sql_expanded {
                ui.available_height() * 0.65
            } else {
                ui.available_height() * 0.30
            };
            egui::Panel::bottom("sql-editor-dock")
                .resizable(true)
                .default_size(height.max(130.0))
                .size_range(100.0..=(ui.available_height() * 0.75).max(100.0))
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sql-dock-scroll")
                        .show(ui, |ui| self.editor_ui(ui));
                });
        }
        if self.ui.inspector_open && !self.ui.inspector_detached {
            if layout.bottom_inspector {
                egui::Panel::bottom("inspector-bottom")
                    .resizable(true)
                    .default_size(180.0)
                    .size_range(90.0..=(ui.available_height() * 0.4).max(90.0))
                    .show(ui, |ui| self.docked_inspector(ui));
            } else {
                egui::Panel::right("inspector-right")
                    .resizable(true)
                    .default_size(layout.inspector_width)
                    .size_range(260.0..=420.0)
                    .show(ui, |ui| self.docked_inspector(ui));
            }
        }
        egui::CentralPanel::default().show(ui, |ui| self.workspace_ui(ui));
        if self.confirm_close {
            egui::Window::new("Close writable database?").anchor(egui::Align2::CENTER_CENTER,egui::Vec2::ZERO).collapsible(false).max_width((ctx.content_rect().width()-48.0).min(520.0)).show(&ctx,|ui|{
                ui.label("Local drafts will be discarded. Active execution will finish. Any remaining editor transaction will be explicitly rolled back. This does not undo committed writes.");
                if ui.button("Wait, roll back transaction, and close").clicked(){self.close();}
                if ui.button("Keep open").clicked(){self.confirm_close=false;self.quit=false;}
            });
        }
        self.edit_windows(&ctx);
        self.schema_windows(&ctx);
        self.inspection_surfaces(&ctx);
        self.command_palette(&ctx);
        self.access_dialog(&ctx);
        self.tool_window(&ctx);
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
        egui::Panel::bottom("status").exact_size(38.0).show(ui,|ui|{
            ui.horizontal(|ui|{
                ui.spacing_mut().item_spacing.x=4.0;
                if self.session.is_some() && icons::control(ui,if self.writable{Icon::Edit}else{Icon::Lock},if self.writable{"Editing"}else{"Read-only"},"",false,true).clicked(){self.ui.access_dialog=true;self.ui.access_ack=false;}
                let (state,color)=if self.uncertain_edit||self.transaction==TransactionState::Unknown{("UNKNOWN",ui.visuals().error_fg_color)}else if self.closing{("Closing",ui.visuals().warn_fg_color)}else if self.commit_ticket.is_some(){("Committing",ui.visuals().warn_fg_color)}else if self.active.is_some()||self.tools.count_request.is_some(){("Executing",ui.visuals().warn_fg_color)}else if self.ui.diagram.ticket.is_some(){("Loading schema",ui.visuals().weak_text_color())}else if !self.ready{("Disconnected",ui.visuals().weak_text_color())}else if self.result.completion.as_ref().is_some_and(|c|c.error.is_some()){("Failed",ui.visuals().error_fg_color)}else{("Connected",ui.visuals().selection.stroke.color)};
                ui.colored_label(color,state).on_hover_text(&self.status);
                if self.transaction!=TransactionState::Autocommit || self.ui.sql_open || self.writable&&self.ready {
                    let active=self.transaction!=TransactionState::Autocommit;
                    ui.colored_label(if active{ui.visuals().warn_fg_color}else{ui.visuals().weak_text_color()},format!("{:?}",self.transaction));
                    for sql in ["BEGIN","COMMIT","ROLLBACK"] {
                        let response=ui.add_enabled(self.ready&&!self.closing&&self.active.is_none()&&self.commit_ticket.is_none()&&!self.has_drafts(),egui::Button::new(sql)).on_hover_text("Persistent editor connection. Transactions also apply to read-only sessions.");
                        #[cfg(test)] self.ui.transaction_rects.push(response.rect);
                        if response.clicked(){self.submit(Operation::Execute{sql:sql.into(),parameters:vec![],budget:Budget::default()},View::Query);}
                    }
                }
                if self.has_drafts(){
                    let n=self.drafts.as_ref().map_or(0,|d|d.edits.len());
                    if ui.button(format!("Review {n}")).clicked(){self.show_preview=true;}
                }
                if self.active.is_some() && ui.button("Discard results").on_hover_text("Suppresses delivery only; execution and writes continue. Does not roll back.").clicked(){
                    if let (Some(s),Some(t))=(&self.session,self.active){s.discard_results(t.request);}
                    self.discard_ticket=self.active;self.result.rows.clear();self.status="Delivery suppressed; awaiting execution outcome".into();
                }
                if let Some(c)=&self.result.completion {
                    let outcome=if c.error.is_some(){"Failed"}else{match c.delivery{tursobench_core::Delivery::Complete=>"Complete",tursobench_core::Delivery::Limited=>"Limited",tursobench_core::Delivery::Discarded=>"Discarded"}};
                    if ui.available_width()>160.0 {ui.small(format!("{outcome} · {} ms",c.elapsed_ms)).on_hover_text(&self.status);}
                }
                if icons::control(ui,Icon::History,"Outcomes","",self.ui.outcomes,false).clicked(){self.ui.outcomes= !self.ui.outcomes;}
                if ui.available_width()>145.0{ui.add(egui::Label::new(&self.status).truncate()).on_hover_text(&self.status);}
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
        self.workspace_header(ui);
        if self.ui.nav == Nav::Overview {
            if self.ui.map_list {
                egui::ScrollArea::vertical()
                    .id_salt("catalog-fallback")
                    .show(ui, |ui| self.schema_panel(ui));
            } else {
                self.diagram_ui(ui);
            }
            return;
        }
        if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Structure {
            egui::ScrollArea::vertical()
                .id_salt("table-structure")
                .show(ui, |ui| self.structure_inspector(ui));
        } else {
            if self.view == View::Browse {
                self.filter_controls(ui);
            }
            self.grid_contents(ui);
        }
    }
    fn workspace_header(&mut self, ui: &mut egui::Ui) {
        let header = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if self.ui.nav != Nav::Overview {
                let can_back = if self.view == View::Browse {
                    !self.tools.back.is_empty()
                } else {
                    self.tools.result_return.is_some()
                };
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
            }
            let title = if self.ui.nav == Nav::Overview {
                "Overview".to_owned()
            } else {
                self.result_title().to_owned()
            };
            let title_width = if ui.available_width() < 600.0 {
                70.0
            } else {
                (ui.available_width() * 0.25).min(180.0)
            };
            ui.add_sized(
                [title_width, 28.0],
                egui::Label::new(egui::RichText::new(&title).strong()).truncate(),
            )
            .on_hover_text(&title);
            if self.ui.nav == Nav::Table {
                for (view, icon, name, key) in [
                    (TableView::Data, Icon::Data, "Data", "Ctrl 1"),
                    (TableView::Structure, Icon::Structure, "Structure", "Ctrl 2"),
                ] {
                    let selected = self.ui.table_view == view;
                    if icons::control(ui, icon, name, key, selected, selected).clicked() {
                        self.ui.table_view = view;
                    }
                }
            }
            if self.ui.nav == Nav::Overview {
                if icons::control(
                    ui,
                    Icon::Structure,
                    "List fallback",
                    "",
                    self.ui.map_list,
                    false,
                )
                .clicked()
                {
                    self.ui.map_list = !self.ui.map_list;
                }
                if icons::button(ui, Icon::Refresh, "Refresh schema", "F5").clicked() {
                    self.refresh_catalog();
                    self.load_diagram();
                }
                return;
            }
            let narrow = ui.available_width() < 380.0;
            if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Data && !narrow {
                if icons::control(
                    ui,
                    Icon::Filter,
                    "Add filter",
                    "Ctrl F",
                    self.ui.filter_open,
                    false,
                )
                .clicked()
                {
                    self.ui.filter_open = !self.ui.filter_open;
                }
                if ui
                    .add_enabled_ui(self.can_edit(), |ui| {
                        icons::button(ui, Icon::Plus, "Insert row", "")
                    })
                    .inner
                    .clicked()
                {
                    self.begin_insert();
                }
            }
            if !narrow
                && icons::button(ui, Icon::Export, "Export retained rows", "Ctrl E").clicked()
            {
                self.ui.export_open = true;
            }
            if !narrow
                && icons::button(ui, Icon::Copy, "Copy retained rows as TSV", "Ctrl Shift C")
                    .clicked()
            {
                self.copy_results(ui.ctx(), crate::export::Format::Tsv);
            }
            if ui
                .add_enabled_ui(
                    self.ready
                        && !self.closing
                        && !self.has_drafts()
                        && self.commit_ticket.is_none(),
                    |ui| icons::button(ui, Icon::Refresh, "Refresh", "F5"),
                )
                .inner
                .clicked()
            {
                if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Structure {
                    if let Some(name) = self.ui.diagram.selected.clone() {
                        self.request_inspection(name);
                    }
                    self.refresh_catalog();
                } else if self.view == View::Browse {
                    self.reload();
                }
            }
            let more = icons::button(ui, Icon::More, "More actions", "");
            egui::Popup::menu(&more).show(|ui| {
                if ui.button("Add typed filter").clicked() {
                    self.ui.filter_open = true;
                    ui.close();
                }
                if ui
                    .add_enabled(self.can_edit(), egui::Button::new("Insert row…"))
                    .clicked()
                {
                    self.begin_insert();
                    ui.close();
                }
                if ui.button("Export retained result…").clicked() {
                    self.ui.export_open = true;
                    ui.close();
                }
                if ui.button("Copy retained TSV").clicked() {
                    self.copy_results(ui.ctx(), crate::export::Format::Tsv);
                    ui.close();
                }
                if self.view == View::Browse
                    && ui
                        .add_enabled_ui(
                            self.ready && !self.closing && self.tools.count_request.is_none(),
                            |ui| {
                                icons::control(
                                    ui,
                                    Icon::Count,
                                    "Count all rows (full scan)",
                                    "",
                                    false,
                                    true,
                                )
                            },
                        )
                        .inner
                        .clicked()
                {
                    if let Some(table) = &self.table {
                        self.count_table(table.name.clone());
                    }
                    ui.close();
                }
                if self.has_drafts() && ui.button("Show staged data").clicked() {
                    self.ui.nav = Nav::Table;
                    self.ui.table_view = TableView::Data;
                    ui.close();
                }
                if ui.button("Overview").clicked() {
                    self.overview();
                    ui.close();
                }
            });
            if self.ui.nav == Nav::Table && self.ui.table_view == TableView::Data {
                self.page_controls(ui);
            }
        });
        #[cfg(test)]
        {
            self.ui.view_header = header.response.rect;
        }
        #[cfg(not(test))]
        let _ = header;
        ui.separator();
    }
    fn page_controls(&mut self, ui: &mut egui::Ui) {
        let Some(b) = self.browse.clone() else {
            ui.small("Lookup ≤100")
                .on_hover_text("Bound related-row lookup. Not a complete table.");
            return;
        };
        let enabled =
            self.ready && !self.closing && !self.has_drafts() && self.commit_ticket.is_none();
        if ui
            .add_enabled_ui(enabled && b.offset > 0, |ui| {
                icons::button(ui, Icon::Back, "Previous 100", "Page Up")
            })
            .inner
            .clicked()
        {
            self.browse.as_mut().unwrap().offset = b.offset.saturating_sub(100);
            self.reload();
        }
        ui.small(format!(
            "{}–{}",
            b.offset + usize::from(!self.result.rows.is_empty()),
            b.offset + self.result.rows.len()
        ))
        .on_hover_text("100 rows per page · OFFSET paging · not a snapshot");
        if ui
            .add_enabled_ui(
                enabled && self.result.rows.len() == 100 && b.offset < 1_000_000,
                |ui| icons::button(ui, Icon::Next, "Next 100", "Page Down"),
            )
            .inner
            .clicked()
        {
            self.browse.as_mut().unwrap().offset = (b.offset + 100).min(1_000_000);
            self.reload();
        }
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
                self.ui.inspector_open = false;
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
