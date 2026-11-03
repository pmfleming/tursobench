use super::{
    App, View,
    icons::{self, Icon},
    shell::{Nav, ToolPanel},
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
            Refresh => match self.document.kind {
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
                icons::action(ui, icon, label)
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
    pub(super) fn command_ribbon(&mut self, ui: &mut egui::Ui) {
        use Action::*;
        egui::Panel::top("command-ribbon")
            .frame(
                egui::Frame::new()
                    .fill(ui.visuals().panel_fill)
                    .inner_margin(6.0),
            )
            .show(ui, |ui| {
                let wide = ui.available_width() >= 1120.0;
                let narrow = ui.available_width() < 850.0;
                let compact = ui.available_width() < 650.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 8.0;
                    ui.vertical(|ui| {
                        ui.small(if self.writable {
                            "OPEN VIEW · EDITING"
                        } else {
                            "OPEN VIEW · READ ONLY"
                        });
                        ui.horizontal(|ui| {
                            if wide {
                                for (a, i, l) in [
                                    (Data, Icon::Data, "Data"),
                                    (Structure, Icon::Structure, "Structure"),
                                    (Map, Icon::Map, "Map"),
                                    (Sql, Icon::Sql, "SQL"),
                                ] {
                                    self.ribbon_action(ui, a, i, l);
                                }
                            } else {
                                let open = icons::action(ui, Icon::Plus, "Open view");
                                egui::Popup::menu(&open).show(|ui| {
                                    for (a, l) in [
                                        (Data, "Table data"),
                                        (Structure, "Table structure"),
                                        (Map, "Schema map"),
                                        (Sql, "SQL editor"),
                                        (NewSql, "New SQL editor"),
                                        (Inspect, "Selected record"),
                                    ] {
                                        self.ribbon_menu_action(ui, a, l);
                                    }
                                });
                            }
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.add_sized(
                            [if narrow { 170.0 } else { 260.0 }, 16.0],
                            egui::Label::new(egui::RichText::new(self.document.title()).small())
                                .truncate(),
                        );
                        ui.horizontal(|ui| {
                            let actions: Vec<_> = match self.document.kind {
                                Kind::Data => vec![
                                    (Refresh, Icon::Refresh, "Refresh"),
                                    (Filter, Icon::Filter, "Filter"),
                                    (FitColumns, Icon::Columns, "Fit columns"),
                                    (Export, Icon::Export, "Export"),
                                ],
                                Kind::Result => vec![
                                    (Export, Icon::Export, "Export"),
                                    (CopyRows, Icon::Copy, "Copy rows"),
                                    (FitColumns, Icon::Columns, "Fit columns"),
                                ],
                                Kind::Structure => vec![
                                    (CopyDdl, Icon::Copy, "Copy DDL"),
                                    (Refresh, Icon::Refresh, "Refresh"),
                                ],
                                Kind::Map => vec![
                                    (FitMap, Icon::Fit, "Fit map"),
                                    (ArrangeMap, Icon::Arrange, "Arrange nodes"),
                                    (MapColumns, Icon::Columns, "Columns"),
                                    (Find, Icon::Search, "Find"),
                                ],
                                Kind::Sql => vec![
                                    (Run, Icon::Sql, "Run"),
                                    (Explain, Icon::Explain, "Explain"),
                                    (Parameters, Icon::Parameters, "Parameters"),
                                ],
                                Kind::Record => vec![
                                    (CopyValue, Icon::Copy, "Copy value"),
                                    (Export, Icon::Export, "Export record"),
                                ],
                            };
                            for (a, i, l) in actions.iter().take(if wide {
                                actions.len()
                            } else if narrow {
                                1
                            } else {
                                2
                            }) {
                                self.ribbon_action(ui, *a, *i, l);
                            }
                            let more = icons::action(ui, Icon::More, "More");
                            egui::Popup::menu(&more).show(|ui| {
                                for (a, _, label) in actions {
                                    self.ribbon_menu_action(ui, a, label);
                                }
                                ui.separator();
                                match self.document.kind {
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
                                            for (i, column) in
                                                self.document.result.columns.iter().enumerate()
                                            {
                                                let mut shown = !self
                                                    .document
                                                    .tools
                                                    .columns
                                                    .hidden
                                                    .contains(&i);
                                                if ui.checkbox(&mut shown, &column.name).changed() {
                                                    if shown {
                                                        self.document
                                                            .tools
                                                            .columns
                                                            .hidden
                                                            .remove(&i);
                                                    } else {
                                                        self.document
                                                            .tools
                                                            .columns
                                                            .hidden
                                                            .insert(i);
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
                                        for (a, l) in [
                                            (RunSelection, "Run selection · Ctrl Shift Enter"),
                                            (Bytecode, "Explain bytecode"),
                                            (Completion, "Completions · Ctrl Space"),
                                            (SaveSql, "Save query · Ctrl S"),
                                            (NewSql, "New SQL editor"),
                                            (Begin, "Begin editor transaction"),
                                        ] {
                                            self.ribbon_menu_action(ui, a, l);
                                        }
                                    }
                                    _ => {}
                                }
                                if compact {
                                    ui.separator();
                                    self.ribbon_menu_action(ui, Retile, "Retile windows");
                                }
                                if self.executing() {
                                    ui.separator();
                                    self.ribbon_menu_action(
                                        ui,
                                        Discard,
                                        "Discard result delivery (execution continues)",
                                    );
                                }
                            });
                        });
                    });
                    ui.separator();
                    ui.vertical(|ui| {
                        ui.small("WORKSPACE");
                        ui.horizontal(|ui| {
                            self.ribbon_action(
                                ui,
                                Expand,
                                Icon::Fit,
                                if self.workspace.expanded.is_some() {
                                    "Restore"
                                } else {
                                    "Expand"
                                },
                            );
                            if !compact {
                                self.ribbon_action(ui, Retile, Icon::Arrange, "Retile");
                            }
                        });
                    });
                });
                if self.has_drafts()
                    || self.transaction != TransactionState::Autocommit
                    || self.commit_ticket.is_some()
                {
                    ui.separator();
                    ui.horizontal_wrapped(|ui| {
                        ui.strong(format!("Database session · {:?}", self.transaction));
                        if self.has_drafts() {
                            let count = self
                                .documents
                                .values()
                                .chain(std::iter::once(&self.document))
                                .filter_map(|d| d.drafts.as_ref())
                                .map(|d| d.edits.len())
                                .sum::<usize>();
                            self.ribbon_action(
                                ui,
                                Review,
                                Icon::Checks,
                                &format!("Review {count} edits"),
                            );
                        }
                        if self.transaction != TransactionState::Autocommit {
                            for (a, label) in [(Commit, "Commit"), (Rollback, "Roll back")] {
                                let response = ui
                                    .add_enabled_ui(self.action_enabled(a), |ui| {
                                        icons::action(ui, Icon::Checks, label)
                                    })
                                    .inner;
                                #[cfg(test)]
                                self.ui.transaction_rects.push(response.rect);
                                if response.clicked() {
                                    self.dispatch_action(a, ui.ctx());
                                }
                            }
                        }
                        if self.commit_ticket.is_some() {
                            ui.label("Committing staged edits…");
                        }
                    });
                }
            });
        // Settings is a temporary route; the ribbon remains bound to the retained view.
        if self.document.ui.nav == Nav::Settings {
            self.ui.settings_return.get_or_insert(Nav::Overview);
        }
    }
}
