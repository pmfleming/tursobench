use super::*;
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
pub(super) struct UiState {
    pub sidebar: Sidebar,
    pub sidebar_open: bool,
    pub workspace: Workspace,
    pub inspector: Inspector,
    pub inspector_open: bool,
    pub inspector_detached: bool,
    pub outcomes: bool,
    pub diagram: diagram::DiagramState,
    pub font_rx: Option<std::sync::mpsc::Receiver<(egui::FontDefinitions, String)>>,
    pub font_label: String,
    #[cfg(test)]
    pub workspace_rect: egui::Rect,
    #[cfg(test)]
    pub transaction_rects: Vec<egui::Rect>,
}
impl UiState {
    pub fn new(ctx: &egui::Context) -> Self {
        Self {
            sidebar: Sidebar::Schema,
            sidebar_open: true,
            workspace: Workspace::Connect,
            inspector: Inspector::Cell,
            inspector_open: false,
            inspector_detached: false,
            outcomes: false,
            diagram: Default::default(),
            font_rx: Some(crate::appearance::system_fonts(ctx.clone())),
            font_label: "Loading system fonts; built-in fallback available".into(),
            #[cfg(test)]
            workspace_rect: egui::Rect::NOTHING,
            #[cfg(test)]
            transaction_rects: vec![],
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
    pub fn new(width: f32, sidebar: bool, inspector: bool) -> Self {
        Self {
            show_sidebar: sidebar && (!inspector || width >= 1500.0),
            bottom_inspector: width < 1100.0,
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
        let sidebar_visible = Layout::new(
            width,
            self.ui.sidebar_open,
            self.ui.inspector_open && !self.ui.inspector_detached,
        )
        .show_sidebar;
        egui::Panel::top("connection").show(ui,|ui|{
            ui.horizontal_wrapped(|ui|{
                ui.strong("TursoBench");
                if self.session.is_some(){
                    let name=std::path::Path::new(&self.path).file_name().and_then(|s|s.to_str()).unwrap_or(&self.path);
                    ui.add(egui::Label::new(egui::RichText::new(name).monospace()).truncate()).on_hover_text(&self.path);
                    ui.colored_label(if self.writable{ui.visuals().warn_fg_color}else{ui.visuals().weak_text_color()},if self.writable{"READ / WRITE"}else{"READ ONLY"});
                    ui.small("Turso 0.8.3 · FTS").on_hover_text("Native FTS/index methods enabled. Other experimental flags remain off. Exclusive use is acknowledged, not OS-enforced.");
                    if ui.add_enabled(!self.closing,egui::Button::new("Close database…")).clicked(){self.request_close();}
                }else{ui.small("Turso-only · local files · read-only by default");}
                if ui.selectable_label(sidebar_visible,"Panels").clicked(){self.ui.sidebar_open = !sidebar_visible;if width<1500.0&&self.ui.sidebar_open&&!self.ui.inspector_detached{self.ui.inspector_open=false;}}
                if ui.selectable_label(self.ui.inspector_open,"Inspector").clicked(){self.ui.inspector_open = !self.ui.inspector_open;}
                if ui.button("Theme").clicked(){self.preferences.dark_mode = !self.preferences.dark_mode;self.apply_theme(&ctx);self.store.save(&self.preferences);}
                if ui.button("Help · F1").clicked(){self.tools.help=true;}
            });
            if self.ready||self.closing{
                ui.horizontal_wrapped(|ui|{
                    ui.label(format!("Editor transaction: {:?}",self.transaction));
                    for sql in ["BEGIN","COMMIT","ROLLBACK"] {
                        let response = ui.add_enabled(self.ready&&self.active.is_none()&&self.commit_ticket.is_none()&&!self.closing&&!self.has_drafts(),egui::Button::new(sql)).on_hover_text("Persistent SQL-editor connection; separate from browse and staged-edit connections");
                        #[cfg(test)] self.ui.transaction_rects.push(response.rect);
                        if response.clicked(){self.submit(Operation::Execute{sql:sql.into(),parameters:vec![],budget:Budget::default()},View::Query);}
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
                if self.has_drafts() {
                    ui.colored_label(
                        ui.visuals().warn_fg_color,
                        format!(
                            "{} staged operations",
                            self.drafts.as_ref().unwrap().edits.len()
                        ),
                    );
                    if ui.button("Review staged edits…").clicked() {
                        self.show_preview = true;
                    }
                    if ui.button("Show data").clicked() {
                        self.ui.workspace = Workspace::Data;
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
        egui::Panel::left("activity-rail")
            .exact_size(96.0)
            .show(ui, |ui| {
                for (sidebar, label) in [
                    (Sidebar::Schema, "Schema"),
                    (Sidebar::History, "History"),
                    (Sidebar::Saved, "Saved"),
                    (Sidebar::Diagnostics, "Checks"),
                    (Sidebar::Settings, "Settings"),
                ] {
                    if ui
                        .add_sized(
                            [80.0, 38.0],
                            egui::Button::new(egui::RichText::new(label).size(13.0))
                                .truncate()
                                .selected(self.ui.sidebar == sidebar && sidebar_visible),
                        )
                        .clicked()
                    {
                        self.ui.sidebar = sidebar;
                        self.ui.sidebar_open = true;
                        if width < 1500.0 && !self.ui.inspector_detached {
                            self.ui.inspector_open = false;
                        }
                    }
                }
                ui.separator();
                if ui
                    .add_sized(
                        [80.0, 38.0],
                        egui::Button::new(egui::RichText::new("Diagram").size(13.0))
                            .truncate()
                            .selected(self.ui.workspace == Workspace::Diagram),
                    )
                    .clicked()
                {
                    self.ui.workspace = Workspace::Diagram;
                    self.tools.ddl = None;
                    self.show_structure();
                    if self.ui.diagram.data.is_none() {
                        self.load_diagram();
                    }
                }
            });
        let layout = Layout::new(
            width,
            self.ui.sidebar_open,
            self.ui.inspector_open && !self.ui.inspector_detached,
        );
        if layout.show_sidebar {
            egui::Panel::left("workspace-sidebar")
                .resizable(true)
                .default_size(layout.sidebar_width)
                .size_range(190.0..=300.0)
                .show(ui, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("workspace-sidebar-scroll")
                        .show(ui, |ui| match self.ui.sidebar {
                            Sidebar::Schema => self.schema_panel(ui),
                            Sidebar::Diagnostics => self.diagnostics_ui(ui),
                            _ => self.history_controls(ui),
                        });
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
        egui::CentralPanel::default().show(ui, |ui| {
            #[cfg(test)]
            {
                self.ui.workspace_rect = ui.available_rect_before_wrap();
            }
            ui.horizontal_wrapped(|ui| {
                if self.session.is_none() { ui.selectable_value(&mut self.ui.workspace, Workspace::Connect, "Open database"); }
                ui.selectable_value(&mut self.ui.workspace, Workspace::Query, "SQL editor");
                ui.selectable_value(&mut self.ui.workspace, Workspace::Data, "Data / results");
                if ui
                    .selectable_value(&mut self.ui.workspace, Workspace::Diagram, "Diagram")
                    .clicked()
                {
                    self.tools.ddl = None;
                    self.show_structure();
                    if self.ui.diagram.data.is_none() {
                        self.load_diagram();
                    }
                }
            });
            ui.separator();
            if self.ui.workspace == Workspace::Connect && self.session.is_none() { self.connect_ui(ui); return; }
            if self.session.is_some() && !self.ready {
                ui.label("Opening database on the worker…");
                if self.ui.workspace == Workspace::Query { self.editor_ui(ui); }
                return;
            }
            if self.session.is_none() {
                ui.label("Not connected — edit SQL or inspect/copy/export retained results. Open a database to execute.");
                if !self.tools.database_scope.is_empty() && (self.result.current.is_some() || self.ui.diagram.data.is_some()) { ui.add(egui::Label::new(format!("Retained source: {}",self.tools.database_scope)).truncate()).on_hover_text(&self.tools.database_scope); }
            }
            if self.ui.workspace == Workspace::Diagram {
                self.diagram_ui(ui);
                return;
            }
            // One persistent editor and one retained result: presentation modes,
            // not pretend independent transactions or unbounded result tabs.
            egui::ScrollArea::vertical()
                .id_salt("workspace-content")
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    if self.ui.workspace == Workspace::Query {
                        self.editor_ui(ui);
                        ui.separator();
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(match self.view {
                            View::Browse => self
                                .table
                                .as_ref()
                                .map_or("Table data", |t| t.name.as_str()),
                            View::Schema => "Schema catalog",
                            View::Explain => "EXPLAIN result",
                            View::Diagnostics => "Diagnostic result",
                            View::Query => "Query result",
                        });
                        ui.small(self.export_scope());
                    });
                    if self.view == View::Browse {
                        ui.add_enabled_ui(self.ready && !self.closing, |ui| self.browse_controls(ui));
                        self.edit_controls(ui);
                    }
                    ui.collapsing("Export and copy retained results", |ui| {
                        self.usability_controls(ui)
                    });
                    self.grid(ui);
                });
        });
        if self.confirm_close {
            egui::Window::new("Close writable database?").collapsible(false).show(&ctx,|ui|{
            ui.label("Local drafts will be discarded. Active execution will finish. Any remaining editor transaction will be explicitly rolled back. This does not undo committed writes.");
            if ui.button("Wait, roll back transaction, and close").clicked(){self.close();}
            if ui.button("Keep open").clicked(){self.confirm_close=false;self.quit=false;}
        });
        }
        self.edit_windows(&ctx);
        self.schema_windows(&ctx);
        self.inspector_windows(&ctx);
    }
    fn connect_ui(&mut self, ui: &mut egui::Ui) {
        egui::ScrollArea::vertical().id_salt("database-connection").show(ui, |ui| {
            ui.set_max_width(840.0);
            ui.heading("Open database");
            ui.add_space(12.0);
            self.database_chooser_ui(ui);
            if self.path.is_empty() {
                egui::CollapsingHeader::new("Enter a path manually").show(ui, |ui| self.database_path_ui(ui));
            } else {
                ui.separator();
                ui.label("Selected database");
                self.database_path_ui(ui);
                ui.add_space(8.0);
                ui.horizontal_wrapped(|ui| {
                    ui.selectable_value(&mut self.writable, false, "Read only");
                    ui.selectable_value(&mut self.writable, true, "Read and write");
                });
                if self.writable {
                    ui.colored_label(ui.visuals().warn_fg_color, "Back up closed files with all required WAL state before trying writes.");
                }
                ui.checkbox(&mut self.exclusive, "No other application has this database open");
                ui.small("Exclusive use is required. This acknowledgement is not an OS lock.");
                if ui.add_enabled(self.picker.is_none() && self.exclusive && !self.has_drafts(), egui::Button::new("Open database")).clicked() {
                    self.open(ui.ctx());
                }
            }
            if self.has_drafts() {
                ui.colored_label(ui.visuals().error_fg_color, "Drafts retained after disconnect. Inspect the uncertain outcome before discarding or retrying.");
                if ui.button("Review retained drafts…").clicked() { self.show_preview = true; }
                if ui.button("Discard retained local drafts").clicked() { self.drafts = None; }
            }
        });
    }
    fn database_path_ui(&mut self, ui: &mut egui::Ui) {
        let mut path = self.path.clone();
        if ui
            .add(
                egui::TextEdit::singleline(&mut path)
                    .id(egui::Id::new("database-path"))
                    .desired_width(ui.available_width())
                    .hint_text("/path/to/existing.db"),
            )
            .changed()
        {
            self.set_database_path(path);
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
                        kind == Inspector::Structure || self.ui.workspace != Workspace::Diagram,
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
        if self.ui.workspace == Workspace::Diagram && self.ui.inspector != Inspector::Structure {
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
