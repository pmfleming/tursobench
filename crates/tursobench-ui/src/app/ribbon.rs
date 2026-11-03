use super::{
    App, View,
    icons::{self, Icon},
    shell::ToolPanel,
    tiling::Kind,
};
use eframe::egui;
use tursobench_core::{Budget, TransactionState};
use tursobench_db::Operation;

#[derive(Clone, Copy)]
pub(super) enum Action {
    Data,
    Structure,
    Map,
    Sql,
    NewSql,
    Refresh,
    Filter,
    FitColumns,
    ShowColumns,
    Export,
    CopyRows,
    Count,
    Inspect,
    FitMap,
    ArrangeMap,
    MapColumns,
    SystemTables,
    Find,
    Zoom100,
    Run,
    RunSelection,
    Explain,
    Bytecode,
    Parameters,
    Completion,
    SaveSql,
    CopyDdl,
    CopyValue,
    CopyRecord,
    CloneRecord,
    DeleteRecord,
    Expand,
    Retile,
    Insert,
    Review,
    Begin,
    Commit,
    Rollback,
    Discard,
}
impl App {
    pub(super) fn executing(&self) -> bool {
        self.document.active.is_some() || self.documents.values().any(|d| d.active.is_some())
    }
    fn action_enabled(&self, action: Action) -> bool {
        use Action::*;
        match action {
            Run | RunSelection | Explain | Bytecode | Begin | Commit | Rollback => {
                self.ready
                    && !self.closing
                    && !self.has_drafts()
                    && self.commit_ticket.is_none()
                    && !self.executing()
                    && (!matches!(action, Commit) || self.transaction == TransactionState::Active)
            }
            Count => {
                self.ready
                    && !self.closing
                    && !self.has_drafts()
                    && self.commit_ticket.is_none()
                    && self.document.kind == Kind::Data
                    && self.document.object.is_some()
            }
            Refresh => {
                self.ready && !self.closing && !self.has_drafts() && self.commit_ticket.is_none()
            }
            Insert => self.can_edit(),
            CloneRecord | DeleteRecord => self.can_edit() && self.document.inspect.is_some(),
            CopyRecord => self.document.inspect.is_some(),
            Inspect | CopyValue => self.document.inspect.is_some(),
            CopyRows | Export => !self.document.result.columns.is_empty(),
            Discard => {
                self.document.active.is_some()
                    || self
                        .documents
                        .values()
                        .any(|d| d.source == Some(self.document.id) && d.active.is_some())
            }
            Structure | Data => {
                self.document.object.is_some() || self.ui.diagram.selected.is_some()
            }
            _ => true,
        }
    }
    pub(super) fn dispatch_action(&mut self, action: Action, ctx: &egui::Context) {
        if !self.action_enabled(action) {
            return;
        }
        use Action::*;
        match action {
            Data => {
                if let Some(name) = self
                    .document
                    .object
                    .clone()
                    .or_else(|| self.ui.diagram.selected.clone())
                {
                    self.browse_table(name);
                }
            }
            Structure => self.open_structure_window(),
            Map => self.overview(),
            Sql => self.open_sql_window(false),
            NewSql => self.open_sql_window(true),
            Refresh => match if self.workspace.active == Kind::Structure {
                Kind::Structure
            } else {
                self.document.kind
            } {
                Kind::Map => {
                    self.refresh_catalog();
                    self.load_diagram();
                }
                Kind::Structure => {
                    if let Some(name) = self.document.object.clone() {
                        self.request_inspection(name);
                    }
                    self.refresh_catalog();
                }
                _ => self.reload(),
            },
            Filter => {
                self.document.ui.filter_open = !self.document.ui.filter_open;
                self.document.tools.filter_focus = self.document.ui.filter_open;
            }
            FitColumns => {
                self.document.tools.columns.manual.clear();
                self.document.tools.columns.refit = true;
            }
            ShowColumns => self.document.tools.columns.hidden.clear(),
            Export => self.ui.export_open = true,
            CopyRows => self.copy_results(ctx, crate::export::Format::Tsv),
            Count => {
                if let Some(name) = self.document.object.clone() {
                    self.count_table(name);
                }
            }
            Inspect => {
                if let Some((row, col)) = self.document.inspect {
                    self.open_record_window(row, col);
                }
            }
            FitMap => self.ui.diagram.fit = true,
            ArrangeMap => self.ui.diagram.arrange_nodes(),
            MapColumns => self.ui.diagram.toggle_columns(),
            SystemTables => self.ui.diagram.toggle_system(),
            Find => self.open_palette(),
            Zoom100 => self.ui.diagram.reset_camera_zoom(),
            Run => self.run_sql(),
            RunSelection => self.run_selection(),
            Explain => self.run_text(self.document.sql.clone(), Some(false)),
            Bytecode => self.run_text(self.document.sql.clone(), Some(true)),
            Parameters => self.document.ui.params_open = !self.document.ui.params_open,
            Completion => self.document.tools.complete = !self.document.tools.complete,
            SaveSql => self.ui.sidebar = ToolPanel::Saved,
            CopyDdl => {
                let name = self
                    .document
                    .tools
                    .ddl
                    .as_ref()
                    .map(|o| o.name.as_str())
                    .or(self.document.object.as_deref());
                if let Some(ddl) = self
                    .schema
                    .iter()
                    .find(|o| Some(o.name.as_str()) == name)
                    .and_then(|o| o.ddl.clone())
                {
                    ctx.copy_text(ddl);
                }
            }
            CopyRecord | CloneRecord | DeleteRecord => {
                if let Some((row, _)) = self.document.inspect {
                    match action {
                        CopyRecord => self.copy_row(ctx, row),
                        CloneRecord => self.clone_row(row),
                        _ => self.stage_delete(row),
                    }
                }
            }
            CopyValue => {
                if let Some((r, c)) = self.document.inspect
                    && let Some(value) = self.document.result.rows.get(r).and_then(|row| row.get(c))
                {
                    ctx.copy_text(crate::export::cell_text(value));
                }
            }
            Expand => self.expand_window(self.workspace.focused),
            Retile => self.retile(),
            Insert => self.begin_insert(),
            Review => {
                self.focus_drafts();
                self.document.show_preview = true;
            }
            Begin | Commit | Rollback => {
                let sql = match action {
                    Begin => "BEGIN",
                    Commit => "COMMIT",
                    _ => "ROLLBACK",
                };
                self.submit(
                    Operation::Execute {
                        sql: sql.into(),
                        parameters: vec![],
                        budget: Budget::default(),
                    },
                    View::Query,
                );
            }
            Discard => {
                let focus = self.workspace.focused;
                let target = if self.document.active.is_some() {
                    Some(self.document.id)
                } else {
                    self.documents
                        .values()
                        .find(|d| d.source == Some(self.document.id) && d.active.is_some())
                        .map(|d| d.id)
                };
                if let Some(id) = target {
                    self.switch_document(id);
                    if let (Some(session), Some(ticket)) = (&self.session, self.document.active) {
                        session.discard_results(ticket.request);
                        self.document.discard_ticket = Some(ticket);
                        self.document.result.rows.clear();
                        self.status="Result delivery suppressed; SQL execution continues. This does not roll back writes.".into();
                    }
                    self.switch_document(focus);
                }
            }
        }
    }
    pub(super) fn focus_drafts(&mut self) {
        let id = if self.document.has_drafts() {
            Some(self.document.id)
        } else {
            self.documents
                .values()
                .find(|d| d.has_drafts())
                .map(|d| d.id)
        };
        if let Some(id) = id {
            self.focus_window(id);
        }
    }
    fn ribbon_action(&mut self, ui: &mut egui::Ui, action: Action, icon: Icon, label: &str) {
        let response = ui
            .add_enabled_ui(self.action_enabled(action), |ui| {
                icons::button(ui, icon, label, "")
            })
            .inner;
        #[cfg(test)]
        self.ui.ribbon_actions.push(response.rect);
        if response.clicked() {
            self.dispatch_action(action, ui.ctx());
        }
    }
    fn ribbon_menu_action(&mut self, ui: &mut egui::Ui, action: Action, label: &str) {
        if ui
            .add_enabled(self.action_enabled(action), egui::Button::new(label))
            .clicked()
        {
            self.dispatch_action(action, ui.ctx());
            ui.close();
        }
    }
    fn ribbon_more(&mut self, ui: &mut egui::Ui, kind: Kind, actions: &[(Action, Icon, &str)]) {
        use Action::*;
        for &(a, _, label) in actions {
            self.ribbon_menu_action(ui, a, label);
        }
        ui.separator();
        ui.menu_button("Workspace", |ui| {
            for (a, label) in [
                (Data, "Table data"),
                (Structure, "Structure"),
                (Map, "Relationships"),
                (Sql, "SQL"),
                (Expand, "Expand / restore"),
                (Review, "Review staged edits"),
            ] {
                self.ribbon_menu_action(ui, a, label);
            }
        });
        match kind {
            Kind::Data | Kind::Result => {
                for (a, l) in [
                    (Inspect, "Inspect selected record · Space"),
                    (ShowColumns, "Show all columns"),
                    (CopyRows, "Copy retained rows · Ctrl Shift C"),
                    (Count, "Count table rows (full scan)"),
                    (Insert, "New row (staged)"),
                ] {
                    self.ribbon_menu_action(ui, a, l);
                }
                ui.menu_button("Visible columns", |ui| {
                    let hidden = &mut self.document.tools.columns.hidden;
                    for (i, column) in self.document.result.columns.iter().enumerate() {
                        let mut shown = !hidden.contains(&i);
                        if ui.checkbox(&mut shown, &column.name).changed() {
                            if shown {
                                hidden.remove(&i);
                            } else {
                                hidden.insert(i);
                            }
                        }
                    }
                });
            }
            Kind::Map => {
                for (a, l) in [
                    (Refresh, "Refresh schema"),
                    (SystemTables, "Toggle system tables"),
                    (Zoom100, "Reset zoom to 100%"),
                ] {
                    self.ribbon_menu_action(ui, a, l);
                }
            }
            Kind::Sql => {
                ui.menu_button("Open editors", |ui| {
                    let editors: Vec<_> = self
                        .window_ids()
                        .into_iter()
                        .filter(|id| self.document_ref(*id).is_some_and(|d| d.kind == Kind::Sql))
                        .collect();
                    for id in editors {
                        if ui.button(format!("Query {id}")).clicked() {
                            self.focus_window(id);
                            ui.close();
                        }
                    }
                });
                for (a, l) in [
                    (RunSelection, "Run selection · Ctrl Shift Enter"),
                    (Bytecode, "Explain bytecode"),
                    (Completion, "Completions · Ctrl Space"),
                    (SaveSql, "Save query · Ctrl S"),
                    (NewSql, "New SQL editor"),
                    (Begin, "Begin editor transaction"),
                    (Commit, "Commit transaction"),
                    (Rollback, "Roll back transaction"),
                ] {
                    self.ribbon_menu_action(ui, a, l);
                }
            }
            _ => {}
        }

        ui.separator();
        self.ribbon_menu_action(ui, Retile, "Show all tiles");
        if self.executing() {
            ui.separator();
            self.ribbon_menu_action(ui, Discard, "Discard result delivery (execution continues)");
        }
    }
    pub(super) fn pane_ribbon(&mut self, ui: &mut egui::Ui, kind: Kind, available: bool) {
        let actions = kind.ribbon_actions();
        if kind == Kind::Record
            && self.document.inspect.is_none()
            && !self.document.result.rows.is_empty()
        {
            self.document.inspect = Some((0, 0));
        }
        let title_width = (if kind == Kind::Map {
            110.0_f32
        } else {
            86.0_f32
        })
        .min((ui.available_width() - 124.0).max(40.0));
        let slots = ((ui.available_width() - title_width - 124.0).max(0.0) / 32.0) as usize;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.add_sized(
                [title_width, 28.0],
                egui::Label::new(egui::RichText::new(kind.label()).strong()).truncate(),
            );
            ui.separator();
            ui.add_enabled_ui(available, |ui| {
                for &(action, icon, label) in actions.iter().take(slots) {
                    self.ribbon_action(ui, action, icon, label);
                }
                let more = icons::button(ui, Icon::More, "More actions", "");
                egui::Popup::menu(&more).show(|ui| self.ribbon_more(ui, kind, actions));
            });
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let expand = icons::button(
                    ui,
                    Icon::Fit,
                    if self.workspace.expanded == Some(kind) {
                        "Restore workspace"
                    } else {
                        "Expand to workspace"
                    },
                    "Ctrl Alt Enter",
                );
                if expand.clicked() {
                    self.expand_pane(kind);
                }
                let minimize = icons::button(ui, Icon::Minimize, "Minimize to header", "");
                #[cfg(test)]
                self.workspace
                    .controls
                    .push((kind, minimize.rect, expand.rect));
                if minimize.clicked() {
                    self.minimize_pane(kind);
                }
                if available && let Some(name) = &self.document.object {
                    ui.add(egui::Label::new(egui::RichText::new(name).small()).truncate());
                }
            });
        });
    }
}

impl Kind {
    fn ribbon_actions(self) -> &'static [(Action, Icon, &'static str)] {
        use Action::*;
        match self {
            Kind::Data => &[
                (Refresh, Icon::Refresh, "Refresh"),
                (Filter, Icon::Filter, "Filter"),
                (FitColumns, Icon::Columns, "Fit columns"),
                (Export, Icon::Export, "Export"),
            ],
            Kind::Result => &[
                (Export, Icon::Export, "Export"),
                (CopyRows, Icon::Copy, "Copy rows"),
                (FitColumns, Icon::Columns, "Fit columns"),
            ],
            Kind::Structure => &[
                (CopyDdl, Icon::Copy, "Copy DDL"),
                (Refresh, Icon::Refresh, "Refresh"),
            ],
            Kind::Map => &[
                (FitMap, Icon::Map, "Fit relationships"),
                (ArrangeMap, Icon::Arrange, "Arrange nodes"),
                (MapColumns, Icon::Columns, "Columns"),
                (Find, Icon::Search, "Find"),
            ],
            Kind::Sql => &[
                (Run, Icon::Sql, "Run"),
                (Explain, Icon::Explain, "Explain"),
                (Parameters, Icon::Parameters, "Parameters"),
            ],
            Kind::Record => &[
                (CopyRecord, Icon::Copy, "Copy record"),
                (CloneRecord, Icon::Plus, "Clone record"),
                (DeleteRecord, Icon::Trash, "Stage deletion"),
                (CopyValue, Icon::Selection, "Copy selected value"),
            ],
        }
    }
}
