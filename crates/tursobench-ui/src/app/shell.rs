use super::{App, View, diagram};
use eframe::egui;
use tursobench_core::Budget;
use tursobench_db::Operation;

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum Sidebar {
    #[default]
    Schema,
    History,
    Saved,
    Diagnostics,
    Settings,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum Workspace {
    #[default]
    Connect,
    Query,
    Data,
    Diagram,
}
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub(super) enum Inspector {
    #[default]
    Cell,
    Record,
    Structure,
}
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum TableView {
    #[default]
    Data,
    Structure,
}
#[derive(Default)]
pub(super) struct Drawer {
    pub open: bool,
    pub expanded: bool,
    pub view: TableView,
}
pub(super) struct UiState {
    pub sidebar: Sidebar,
    pub unfolded: Option<String>,
    pub hover_table: Option<String>,
    pub sidebar_open: bool,
    pub workspace: Workspace,
    pub inspector: Inspector,
    pub inspector_open: bool,
    pub inspector_detached: bool,
    pub outcomes: bool,
    pub diagram: diagram::DiagramState,
    pub drawer: Drawer,
    pub map_list: bool,
    pub sql_open: bool,
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
    pub drawer_header: egui::Rect,
}
impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            sidebar: Sidebar::Schema,
            unfolded: None,
            hover_table: None,
            sidebar_open: false,
            workspace: Workspace::Connect,
            inspector: Inspector::Cell,
            inspector_open: false,
            inspector_detached: false,
            outcomes: false,
            diagram: Default::default(),
            drawer: Default::default(),
            map_list: false,
            sql_open: false,
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
            drawer_header: egui::Rect::NOTHING,
        }
    }
}
#[derive(Debug)]
pub(super) struct Layout {
    pub show_sidebar: bool,
    pub bottom_inspector: bool,
    pub sidebar_width: f32,
    pub inspector_width: f32,
}
impl Layout {
    pub fn new(width: f32, sidebar: bool, _inspector: bool) -> Self {
        Self {
            show_sidebar: sidebar,
            bottom_inspector: width < 1400.0,
            sidebar_width: if width >= 1800.0 { 260.0 } else { 220.0 },
            inspector_width: if width >= 1800.0 { 370.0 } else { 310.0 },
        }
    }
}
impl App {
    pub(super) fn show_structure(&mut self) {
        self.ui.inspector = Inspector::Structure;
        self.ui.inspector_open = true;
    }
    pub(super) fn shell(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        self.shortcuts(&ctx);
        #[cfg(test)]
        self.ui.transaction_rects.clear();
        let width = ui.available_width();
        let height = ui.available_height();
        self.connection_bar(ui);
        self.status_bar(ui, height);

        let layout = Layout::new(
            width,
            self.session.is_some(),
            self.ui.inspector_open && !self.ui.inspector_detached,
        );
        if layout.show_sidebar {
            egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(layout.sidebar_width)
                .size_range(190.0..=300.0)
                .show(ui, |ui| {
                    self.navigation_sidebar(ui);
                });
        }
        if self.ui.inspector_open && !self.ui.inspector_detached {
            if layout.bottom_inspector {
                egui::Panel::bottom("inspector-bottom")
                    .resizable(true)
                    .default_size(180.0)
                    .size_range(100.0..=(height * 0.4).max(100.0))
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
            egui::Window::new("Close writable database?").collapsible(false).show(&ctx,|ui|{
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
        if self.ui.export_open {
            let mut open = true;
            egui::Window::new("Export retained result")
                .open(&mut open)
                .default_width(600.0)
                .show(&ctx, |ui| self.usability_controls(ui));
            self.ui.export_open = open;
        }
    }
    fn connection_bar(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        egui::Panel::top("connection").show(ui,|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.strong("TursoBench");
                if self.session.is_some(){
                    let name=std::path::Path::new(&self.path).file_name().and_then(|s|s.to_str()).unwrap_or(&self.path);
                    ui.add(egui::Label::new(egui::RichText::new(name).monospace()).truncate()).on_hover_text(&self.path);
                    if ui.button(if self.writable{"Editing · lock"}else{"Read-only · unlock"}).clicked() { self.ui.access_dialog = true; self.ui.access_ack = false; }
                    ui.small("Turso 0.8.3 · FTS").on_hover_text("Native FTS/index methods enabled. Other experimental flags remain off. Exclusive use is acknowledged, not OS-enforced.");
                    if ui.add_enabled(!self.closing,egui::Button::new("Close database…")).clicked(){self.request_close();}
                }else{ui.small("Turso-only · local files · read-only by default");}
                if self.session.is_some() && ui.selectable_label(self.ui.workspace == Workspace::Diagram, "Schema home").clicked() {
                    self.ui.workspace = Workspace::Diagram;
                    self.ui.drawer.expanded = false;
                    if self.ui.diagram.data.is_none() { self.load_diagram(); }
                }
                if ui.button("Search · Ctrl K").clicked() { self.open_palette(); }
                if ui.selectable_label(self.ui.sql_open || self.ui.workspace == Workspace::Query, "SQL · Ctrl J").clicked() { self.toggle_sql(); }
                ui.menu_button("Tools", |ui| {
                    for (label, sidebar) in [("History", Sidebar::History), ("Saved queries", Sidebar::Saved), ("Database checks", Sidebar::Diagnostics), ("Settings", Sidebar::Settings)] {
                        if ui.button(label).clicked() {
                            self.ui.sidebar = sidebar;
                            self.ui.sidebar_open = true;
                            self.ui.inspector_open = false;
                            ui.close();
                        }
                    }
                    if ui.button("Export retained result…").clicked() { self.ui.export_open = true; ui.close(); }
                    if ui.button("Inspect retained row").clicked() { self.ui.inspector_open = true; ui.close(); }
                    if ui.button("Theme").clicked() { self.preferences.dark_mode = !self.preferences.dark_mode; self.apply_theme(&ctx); self.store.save(&self.preferences); ui.close(); }
                    if ui.button("Help · F1").clicked() { self.tools.help = true; ui.close(); }
                });
            });
            if self.ready||self.closing{
                ui.horizontal_wrapped(|ui|{
                    if self.writable || self.ui.sql_open || self.ui.workspace == Workspace::Query || self.transaction != tursobench_core::TransactionState::Autocommit {
                    ui.label(format!("Editor transaction: {:?}",self.transaction));
                    for sql in ["BEGIN","COMMIT","ROLLBACK"] {
                        let response = ui.add_enabled(self.ready&&self.active.is_none()&&self.commit_ticket.is_none()&&!self.closing&&!self.has_drafts(),egui::Button::new(sql)).on_hover_text("Persistent SQL-editor connection; separate from browse and staged-edit connections");
                        #[cfg(test)] self.ui.transaction_rects.push(response.rect);
                        if response.clicked(){self.submit(Operation::Execute{sql:sql.into(),parameters:vec![],budget:Budget::default()},View::Query);}
                    }
                    }
                    if self.active.is_some(){
                        if self.discard_ticket==self.active{ui.colored_label(ui.visuals().warn_fg_color,"Delivery suppressed; awaiting execution outcome");}
                        else if ui.button("Discard results").on_hover_text("Stops delivery only. Does NOT interrupt execution, cancel writes or roll back.").clicked(){if let(Some(s),Some(t))=(&self.session,self.active){s.discard_results(t.request);}self.discard_ticket=self.active;self.result.rows.clear();self.status="Discard requested; waiting for the actual execution outcome".into();}
                    }
                    if self.closing{ui.colored_label(ui.visuals().warn_fg_color,"Closing — waiting for execution and cleanup acknowledgement");}
                });
            }
            if self.uncertain_edit { ui.colored_label(ui.visuals().error_fg_color,"UNKNOWN commit outcome — no automatic retry"); }
        });
    }
    fn status_bar(&mut self, ui: &mut egui::Ui, height: f32) {
        egui::Panel::bottom("status").show(ui, |ui| {
            ui.horizontal_wrapped(|ui| {
                ui.small(if self.closing {
                    "Closing"
                } else if self.commit_ticket.is_some() {
                    "Committing"
                } else if self.active.is_some() {
                    "Executing / awaiting outcome"
                } else if self.ui.diagram.ticket.is_some() {
                    "Loading diagram"
                } else if self.ready {
                    "Connected"
                } else {
                    "Not connected"
                });
                if let Some(draft) = self.drafts.as_ref().filter(|d| !d.edits.is_empty()) {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!("{} staged operations", draft.edits.len()),
                    );
                    if ui.button("Review staged edits…").clicked() {
                        self.show_preview = true;
                    }
                    if ui.button("Show data").clicked() {
                        self.ui.workspace = Workspace::Diagram;
                        self.ui.drawer.open = true;
                        self.ui.drawer.expanded = true;
                        self.ui.drawer.view = TableView::Data;
                    }
                }
                if let Some(c) = &self.result.completion {
                    ui.small(format!(
                        "Last result: {} ms · {:?} · {} retained rows",
                        c.elapsed_ms,
                        c.delivery,
                        self.result.rows.len()
                    ));
                    if c.elapsed_ms >= self.preferences.slow_query_ms {
                        ui.colored_label(ui.visuals().warn_fg_color, "Slow result");
                    }
                }
                if ui
                    .selectable_label(
                        self.ui.outcomes,
                        format!("Outcomes ({})", self.activity.len()),
                    )
                    .clicked()
                {
                    self.ui.outcomes = !self.ui.outcomes;
                }
            });
            ui.add(egui::Label::new(&self.status).truncate())
                .on_hover_text(&self.status);
            if self.ui.outcomes {
                egui::ScrollArea::vertical()
                    .id_salt("outcomes")
                    .max_height((height * 0.22).min(180.0))
                    .show(ui, |ui| {
                        for message in self.activity.iter().rev() {
                            ui.label(message);
                        }
                    });
            }
        });
    }
    fn workspace_ui(&mut self, ui: &mut egui::Ui) {
        #[cfg(test)]
        {
            self.ui.workspace_rect = ui.available_rect_before_wrap();
        }
        if self.session.is_none() {
            ui.horizontal_wrapped(|ui| {
                ui.selectable_value(&mut self.ui.workspace, Workspace::Connect, "Open database");
                ui.selectable_value(&mut self.ui.workspace, Workspace::Query, "SQL editor");
                ui.selectable_value(&mut self.ui.workspace, Workspace::Data, "Retained results");
                if self.ui.diagram.data.is_some() {
                    ui.selectable_value(&mut self.ui.workspace, Workspace::Diagram, "Retained map");
                }
            });
            ui.separator();
        }
        if self.ui.workspace == Workspace::Connect && self.session.is_none() {
            self.connect_ui(ui);
            return;
        }
        if self.session.is_some() && !self.ready {
            ui.label("Opening database on the worker…");
            if self.ui.workspace == Workspace::Query {
                self.editor_ui(ui);
            }
            return;
        }
        if self.session.is_none() {
            ui.label("Not connected — edit SQL or inspect/copy/export retained results. Open a database to execute.");
            if !self.tools.database_scope.is_empty()
                && (self.result.current.is_some() || self.ui.diagram.data.is_some())
            {
                ui.add(
                    egui::Label::new(format!("Retained source: {}", self.tools.database_scope))
                        .truncate(),
                )
                .on_hover_text(&self.tools.database_scope);
            }
        }
        if self.ui.workspace == Workspace::Diagram {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.ui.map_list, false, "Map");
                ui.selectable_value(&mut self.ui.map_list, true, "List");
            });
            if self.ui.drawer.open {
                if self.ui.drawer.expanded {
                    self.table_drawer(ui);
                    return;
                }
                let height = ui.available_height();
                egui::Panel::bottom("table-drawer")
                    .resizable(true)
                    .default_size(height * 0.48)
                    .size_range(120.0..=(height * 0.75).max(120.0))
                    .show(ui, |ui| self.table_drawer(ui));
            }
            if self.ui.map_list {
                egui::ScrollArea::vertical()
                    .id_salt("map-object-list")
                    .show(ui, |ui| self.schema_panel(ui));
            } else {
                self.diagram_ui(ui);
            }
            return;
        }
        self.result_workspace(ui);
    }
    fn table_drawer(&mut self, ui: &mut egui::Ui) {
        let header = ui.horizontal_wrapped(|ui| {
            ui.strong(if self.view == View::Browse {
                self.ui.diagram.selected.as_deref().unwrap_or("Table")
            } else {
                self.result_title()
            });
            if self.view == View::Browse {
                ui.selectable_value(&mut self.ui.drawer.view, TableView::Data, "Data");
                ui.selectable_value(&mut self.ui.drawer.view, TableView::Structure, "Structure");
            }
            if self.ui.sql_open && ui.button("Hide SQL").clicked() {
                self.ui.sql_open = false;
            }
            if ui
                .button(if self.ui.drawer.expanded {
                    "Return to map"
                } else {
                    "Expand"
                })
                .clicked()
            {
                self.ui.drawer.expanded = !self.ui.drawer.expanded;
            }
            if ui.button("Close drawer").clicked() {
                self.ui.drawer.open = false;
                self.ui.drawer.expanded = false;
                self.ui.sql_open = false;
            }
        });
        #[cfg(test)]
        {
            self.ui.drawer_header = header.response.rect;
        }
        #[cfg(not(test))]
        let _ = header;
        ui.separator();
        if self.ui.drawer.view == TableView::Structure && self.view == View::Browse {
            egui::ScrollArea::vertical()
                .id_salt("drawer-structure")
                .show(ui, |ui| self.structure_inspector(ui));
        } else {
            self.result_workspace(ui);
        }
    }
    fn result_workspace(&mut self, ui: &mut egui::Ui) {
        // One persistent editor and one retained result: presentation modes,
        // not pretend independent transactions or unbounded result tabs.
        egui::ScrollArea::vertical()
            .id_salt("workspace-content")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                if self.ui.workspace == Workspace::Query || self.ui.sql_open {
                    ui.horizontal(|ui| {
                        for (label, sidebar) in
                            [("History", Sidebar::History), ("Saved", Sidebar::Saved)]
                        {
                            if ui.button(label).clicked() {
                                self.ui.sidebar = sidebar;
                                self.ui.sidebar_open = true;
                                self.ui.inspector_open = false;
                            }
                        }
                    });
                    self.editor_ui(ui);
                    ui.separator();
                }
                ui.horizontal_wrapped(|ui| {
                    ui.strong(self.result_title());
                    ui.small(self.export_scope());
                });
                if self.view == View::Browse {
                    ui.add_enabled_ui(self.ready && !self.closing, |ui| self.browse_controls(ui));
                    self.edit_controls(ui);
                }
                ui.menu_button("Result actions", |ui| {
                    if self.view != View::Browse
                        && self.tools.result_return.is_some()
                        && ui.button("Return to table").clicked()
                    {
                        self.return_to_table();
                        ui.close();
                    }
                    if ui.button("Export / copy…").clicked() {
                        self.ui.export_open = true;
                        ui.close();
                    }
                    if self.view == View::Browse
                        && ui.button("Count table rows (full scan)").clicked()
                    {
                        if let Some(table) = &self.table {
                            self.submit(Operation::Count(table.name.clone()), View::Diagnostics);
                        }
                        ui.close();
                    }
                });
                self.grid_contents(ui);
            });
    }
    fn result_title(&self) -> &str {
        match self.view {
            View::Browse => self
                .table
                .as_ref()
                .map_or("Table data", |t| t.name.as_str()),
            View::Explain => "EXPLAIN result",
            View::Diagnostics => "Diagnostic result",
            View::Query => "Query result",
        }
    }
    fn docked_inspector(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            for (kind, label) in [
                (Inspector::Cell, "Cell"),
                (Inspector::Record, "Record"),
                (Inspector::Structure, "Structure"),
            ] {
                if ui
                    .add_enabled(
                        kind == Inspector::Structure
                            || self.ui.workspace != Workspace::Diagram
                            || self.ui.drawer.open,
                        egui::Button::new(label).selected(self.ui.inspector == kind),
                    )
                    .clicked()
                {
                    self.ui.inspector = kind;
                }
            }
            if ui.small_button("Pop out").clicked() {
                self.ui.inspector_detached = true;
            }
            if ui.small_button("Hide").clicked() {
                self.ui.inspector_open = false;
            }
        });
        ui.separator();
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .id_salt("docked-inspector-content")
            .show(ui, |ui| self.inspector_content(ui));
    }
    pub(super) fn inspector_content(&mut self, ui: &mut egui::Ui) {
        if self.ui.workspace == Workspace::Diagram
            && !self.ui.drawer.open
            && self.ui.inspector != Inspector::Structure
        {
            ui.label("Choose Data / results or SQL editor to inspect retained row values. Diagram selection is schema metadata, not a data row.");
            return;
        }
        match self.ui.inspector {
            Inspector::Cell => {
                if let Some((row, col)) = self.inspect {
                    self.cell_inspector(ui, row, col);
                } else {
                    ui.label("Select a result cell to inspect its exact type, copy it or stage a change.");
                }
            }
            Inspector::Record => {
                if let Some(row) = self.tools.detail.or(self.inspect.map(|(r, _)| r)) {
                    self.record_inspector(ui, row);
                } else {
                    ui.label("Double-click a result row for record detail.");
                }
            }
            Inspector::Structure => self.structure_inspector(ui),
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn laptop_and_widescreen_panes_leave_room_for_content() {
        for width in [960.0, 1280.0, 1366.0, 1440.0, 1920.0, 2560.0, 3440.0] {
            let l = Layout::new(width, true, true);
            let used = 96.0
                + if l.show_sidebar { l.sidebar_width } else { 0.0 }
                + if l.bottom_inspector {
                    0.0
                } else {
                    l.inspector_width
                };
            assert!(width - used >= 540.0, "{width}: {l:?}");
        }
    }
}
