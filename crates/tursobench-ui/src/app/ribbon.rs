use super::{
    App, View,
    icons::{self, Icon},
    shell::ToolPanel,
    tiling::Kind,
};
use eframe::egui;
use tursobench_core::{Budget, TransactionState};
use tursobench_db::Operation;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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
        let selected = match action {
            Action::Filter => Some(self.document.ui.filter_open),
            Action::MapColumns => Some(self.ui.diagram.all_columns),
            Action::SystemTables => Some(self.ui.diagram.show_system),
            Action::Parameters => Some(self.document.ui.params_open),
            Action::Completion => Some(self.document.tools.complete),
            _ => None,
        };
        let response = ui
            .add_enabled_ui(self.action_enabled(action), |ui| {
                icons::ribbon_button(ui, icon, label, action.shortcut(), selected)
            })
            .inner;
        #[cfg(test)]
        if ui.clip_rect().contains_rect(response.rect) {
            self.ui.ribbon_actions.push(response.rect);
        }
        if response.clicked() {
            self.dispatch_action(action, ui.ctx());
        }
    }

    fn ribbon_columns(&mut self, ui: &mut egui::Ui) {
        let response = icons::ribbon_button(ui, Icon::Columns, "Visible columns", "", None);
        // A dedicated selector, not a general-purpose action/overflow menu.
        egui::Popup::menu(&response).show(|ui| {
            ui.strong("Visible columns");
            egui::ScrollArea::vertical()
                .max_height(280.0)
                .show(ui, |ui| {
                    let hidden = &mut self.document.tools.columns.hidden;
                    for (i, column) in self.document.result.columns.iter().enumerate() {
                        ui.push_id(i, |ui| {
                            let mut shown = !hidden.contains(&i);
                            if ui.checkbox(&mut shown, &column.name).changed() {
                                if shown {
                                    hidden.remove(&i);
                                } else {
                                    hidden.insert(i);
                                }
                            }
                        });
                    }
                });
        });
    }

    fn ribbon_editors(&mut self, ui: &mut egui::Ui) {
        let current = self.document.id;
        let mut selected = current;
        egui::ComboBox::from_id_salt("open-editors")
            .selected_text(format!("Query {current}"))
            .width(100.0)
            .show_ui(ui, |ui| {
                for id in self.window_ids() {
                    if self.document_ref(id).is_some_and(|d| d.kind == Kind::Sql) {
                        ui.selectable_value(&mut selected, id, format!("Query {id}"));
                    }
                }
            })
            .response
            .on_hover_text("Open SQL editors");
        if selected != current {
            self.focus_window(selected);
        }
    }

    pub(super) fn pane_ribbon(&mut self, ui: &mut egui::Ui, kind: Kind, available: bool) {
        if kind == Kind::Record
            && self.document.inspect.is_none()
            && !self.document.result.rows.is_empty()
        {
            self.document.inspect = Some((0, 0));
        }
        // Keep tile identity and window controls fixed while the action ribbon scrolls.
        // Never drop actions based on width or replace them with an overflow menu.
        let stacked_title = ui.available_width() < 480.0;
        if stacked_title {
            ui.horizontal(|ui| self.ribbon_title(ui, kind, available));
        }
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            if !stacked_title {
                ui.allocate_ui_with_layout(
                    egui::vec2(110.0, icons::RIBBON_BUTTON_SIZE),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| self.ribbon_title(ui, kind, available),
                );
            }
            let controls_width = 2.0 * icons::RIBBON_BUTTON_SIZE + 6.0;
            let ribbon_width = (ui.available_width() - controls_width).max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ribbon_width, icons::RIBBON_BUTTON_SIZE + 12.0),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| {
                    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::solid();
                    egui::ScrollArea::horizontal()
                        .id_salt(("pane-actions", kind))
                        .max_width(ribbon_width)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(available, |ui| {
                                    for &(action, icon, label) in kind.ribbon_actions() {
                                        self.ribbon_action(ui, action, icon, label);
                                    }
                                    match kind {
                                        Kind::Data | Kind::Result => self.ribbon_columns(ui),
                                        Kind::Sql => self.ribbon_editors(ui),
                                        _ => {}
                                    }
                                });
                                ui.separator();
                                // Workspace navigation remains usable even in an empty tile.
                                for &(action, icon, label) in WORKSPACE_ACTIONS {
                                    let enabled = available
                                        || matches!(
                                            action,
                                            Action::Map
                                                | Action::Sql
                                                | Action::Review
                                                | Action::Retile
                                        );
                                    ui.add_enabled_ui(enabled, |ui| {
                                        self.ribbon_action(ui, action, icon, label);
                                    });
                                }
                                // Keep the control discoverable; disable it when there is no delivery to discard.
                                ui.add_enabled_ui(available, |ui| {
                                    self.ribbon_action(
                                        ui,
                                        Action::Discard,
                                        Icon::Close,
                                        "Discard result delivery (execution continues)",
                                    );
                                });
                            });
                        });
                },
            );
            let minimize = icons::ribbon_button(ui, Icon::Minimize, "Minimize to header", "", None);
            let expand = icons::ribbon_button(
                ui,
                Icon::Fit,
                if self.workspace.expanded == Some(kind) {
                    "Restore workspace"
                } else {
                    "Expand to workspace"
                },
                "Ctrl Alt Enter",
                None,
            );
            #[cfg(test)]
            self.workspace
                .controls
                .push((kind, minimize.rect, expand.rect));
            if minimize.clicked() {
                self.minimize_pane(kind);
            }
            if expand.clicked() {
                self.expand_pane(kind);
            }
        });
    }

    fn ribbon_title(&self, ui: &mut egui::Ui, kind: Kind, available: bool) {
        ui.add(egui::Label::new(egui::RichText::new(kind.label()).strong()).truncate())
            .on_hover_text(kind.label());
        if available && let Some(name) = &self.document.object {
            ui.add(egui::Label::new(egui::RichText::new(name).small()).truncate())
                .on_hover_text(name);
        }
    }
}

const WORKSPACE_ACTIONS: &[(Action, Icon, &str)] = &[
    (Action::Data, Icon::Data, "Table data"),
    (Action::Structure, Icon::Structure, "Structure"),
    (Action::Map, Icon::Map, "Relationships"),
    (Action::Sql, Icon::Sql, "SQL"),
    (Action::Review, Icon::Edit, "Review staged edits"),
    (Action::Retile, Icon::Arrange, "Show all tiles"),
];

impl Action {
    fn shortcut(self) -> &'static str {
        use Action::*;
        match self {
            Refresh => "F5",
            Filter => "Ctrl F",
            Export => "Ctrl E",
            CopyRows => "Ctrl Shift C",
            Inspect => "Space",
            Find => "Ctrl K",
            Run => "Ctrl Enter",
            RunSelection => "Ctrl Shift Enter",
            Completion => "Ctrl Space",
            SaveSql => "Ctrl S",
            _ => "",
        }
    }
}

impl Kind {
    fn ribbon_actions(self) -> &'static [(Action, Icon, &'static str)] {
        use Action::*;
        match self {
            Kind::Data => &[
                (Refresh, Icon::Refresh, "Refresh"),
                (Filter, Icon::Filter, "Filter"),
                (FitColumns, Icon::Fit, "Fit columns"),
                (Export, Icon::Export, "Export"),
                (Inspect, Icon::Selection, "Inspect selected record"),
                (ShowColumns, Icon::Complete, "Show all columns"),
                (CopyRows, Icon::Copy, "Copy retained rows"),
                (Count, Icon::Count, "Count table rows (full scan)"),
                (Insert, Icon::Plus, "New row (staged)"),
            ],
            Kind::Result => &[
                (Export, Icon::Export, "Export"),
                (CopyRows, Icon::Copy, "Copy retained rows"),
                (FitColumns, Icon::Fit, "Fit columns"),
                (Inspect, Icon::Selection, "Inspect selected record"),
                (ShowColumns, Icon::Complete, "Show all columns"),
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
                (Refresh, Icon::Refresh, "Refresh schema"),
                (SystemTables, Icon::System, "Toggle system tables"),
                (Zoom100, Icon::Zoom100, "Reset zoom to 100%"),
            ],
            Kind::Sql => &[
                (Run, Icon::Sql, "Run"),
                (
                    RunSelection,
                    Icon::Selection,
                    "Run selection / statement at cursor",
                ),
                (Explain, Icon::Explain, "Explain query plan"),
                (Bytecode, Icon::Structure, "Explain bytecode"),
                (Parameters, Icon::Parameters, "Typed parameters"),
                (Completion, Icon::Complete, "Completions"),
                (SaveSql, Icon::Saved, "Save query"),
                (NewSql, Icon::Plus, "New SQL editor"),
                (Begin, Icon::Lock, "Begin editor transaction"),
                (Commit, Icon::Checks, "Commit transaction"),
                (Rollback, Icon::Undo, "Roll back transaction"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn narrow_ribbon_scrolls_to_a_direct_action_and_keeps_window_controls_fixed() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let mut app = App::new(&ctx);
        let mut time = 0.0;
        let mut render = |app: &mut App, events| {
            time += 0.1;
            app.workspace.controls.clear();
            let mut output = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 150.0),
                    )),
                    time: Some(time),
                    events,
                    ..Default::default()
                },
                |ui| app.pane_ribbon(ui, Kind::Map, true),
            );
            output.textures_delta.clear();
            output
        };
        render(&mut app, vec![]);
        render(&mut app, vec![]);
        let (_, minimize, expand) = app.workspace.controls[0];
        let pointer = egui::pos2(100.0, 50.0);
        render(
            &mut app,
            vec![
                egui::Event::PointerMoved(pointer),
                egui::Event::MouseWheel {
                    unit: egui::MouseWheelUnit::Point,
                    delta: egui::vec2(-240.0, 0.0),
                    phase: egui::TouchPhase::Move,
                    source: egui::MouseWheelSource::Unknown,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
        );
        for _ in 0..10 {
            render(&mut app, vec![]);
        }
        let output = render(&mut app, vec![]);
        let tree = output.platform_output.accesskit_update.unwrap();
        let bounds = tree
            .nodes
            .iter()
            .find_map(|(_, n)| {
                (n.label() == Some("Toggle system tables"))
                    .then(|| n.bounds())
                    .flatten()
            })
            .expect("direct system tables toggle");
        let target = egui::pos2(
            (bounds.x0 + bounds.x1) as f32 / 2.0,
            (bounds.y0 + bounds.y1) as f32 / 2.0,
        );
        assert!(
            target.x > 0.0 && target.x < minimize.left(),
            "action must be scrolled into view: {target:?}"
        );
        // The scrollbar reveal animation can shift vertical alignment by a fraction of a point.
        for (before, after) in [
            (minimize, app.workspace.controls[0].1),
            (expand, app.workspace.controls[0].2),
        ] {
            assert_eq!(before.left(), after.left());
            assert_eq!(before.size(), after.size());
            assert!((before.top() - after.top()).abs() < 1.0);
        }
        assert!(!app.ui.diagram.show_system);
        for pressed in [true, false] {
            render(
                &mut app,
                vec![
                    egui::Event::PointerMoved(target),
                    egui::Event::PointerButton {
                        pos: target,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
            );
        }
        assert!(
            app.ui.diagram.show_system,
            "visible ribbon button must dispatch its action without a popup"
        );
    }
}
