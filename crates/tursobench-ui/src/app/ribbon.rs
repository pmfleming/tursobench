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
    ZoomOut,
    ZoomIn,
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
            ZoomOut => self.ui.diagram.camera_zoom() > super::diagram::MIN_ZOOM,
            ZoomIn => self.ui.diagram.camera_zoom() < super::diagram::MAX_ZOOM,
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
            _ => true,
        }
    }
    pub(super) fn dispatch_action(&mut self, action: Action, ctx: &egui::Context) {
        if !self.action_enabled(action) {
            return;
        }
        use Action::*;
        match action {
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
            ZoomOut => self.ui.diagram.step_camera_zoom(1.0 / 1.25),
            ZoomIn => self.ui.diagram.step_camera_zoom(1.25),
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
                if action == Action::Zoom100 {
                    let percent = self.ui.diagram.camera_zoom() * 100.0;
                    let text = if percent < 1.0 {
                        format!("{percent:.2}%")
                    } else {
                        format!("{percent:.0}%")
                    };
                    let response = ui.add(
                        egui::Button::new(egui::RichText::new(text).size(
                            crate::appearance::text_size(ui.ctx(), icons::RIBBON_TEXT_SIZE),
                        ))
                        .min_size(egui::vec2(
                            icons::RIBBON_BUTTON_SIZE,
                            icons::RIBBON_BUTTON_SIZE,
                        )),
                    );
                    response.widget_info(|| {
                        egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label)
                    });
                    response.on_hover_text("Current zoom · click to reset to 100%")
                } else {
                    icons::ribbon_button(ui, icon, label, action.shortcut(), selected)
                }
            })
            .inner;
        #[cfg(test)]
        if ui.clip_rect().contains_rect(response.rect) {
            self.ui.ribbon_actions.push(response.rect);
        }
        if response.clicked() {
            // Export and the detached record inspector read the focused document later.
            if self.document.kind == Kind::Result {
                self.focus_window(self.document.id);
            }
            self.dispatch_action(action, ui.ctx());
        }
    }

    fn ribbon_columns(&mut self, ui: &mut egui::Ui, label: &str) {
        let response = icons::ribbon_button(ui, Icon::Columns, label, "", None);
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

    fn ribbon_sql_results(&mut self, ui: &mut egui::Ui) {
        let editor = self.document.id;
        let Some(result) = self.sql_result_window(editor) else {
            return;
        };
        self.switch_document(result);
        ui.separator();
        ui.label("Results");
        ui.push_id(("result-actions", result), |ui| {
            for &(action, icon, label) in Kind::Result.ribbon_actions() {
                self.ribbon_action(ui, action, icon, label);
            }
            self.ribbon_columns(ui, "Visible result columns");
        });
        self.switch_document(editor);
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
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 3.0;
            ui.style_mut().override_font_id = Some(crate::appearance::proportional(ui.ctx(), icons::RIBBON_TEXT_SIZE));
            ui.spacing_mut().interact_size.y = icons::RIBBON_BUTTON_SIZE;
            let controls_width = 2.0 * icons::RIBBON_BUTTON_SIZE + 20.0;
            let remaining = (ui.available_width() - controls_width).max(0.0);
            let title_size = ui.painter().layout_no_wrap(
                kind.label().into(),
                crate::appearance::proportional(ui.ctx(), icons::RIBBON_TEXT_SIZE),
                ui.visuals().text_color(),
            ).size();
            let mut title_width = (remaining * 0.25)
                .max(title_size.x + 4.0)
                .min(200.0)
                .min((remaining - 32.0).max(0.0));
            if matches!(kind, Kind::Data | Kind::Record) {
                title_width = title_width.max(48.0).min((remaining - 6.0).max(0.0));
            }
            ui.allocate_ui_with_layout(
                egui::vec2(title_width, icons::RIBBON_BUTTON_SIZE),
                egui::Layout::left_to_right(egui::Align::Center),
                |ui| self.ribbon_title(ui, kind, available),
            );
            let ribbon_width = (ui.available_width() - controls_width).max(1.0);
            ui.allocate_ui_with_layout(
                egui::vec2(ribbon_width, icons::RIBBON_BUTTON_SIZE),
                egui::Layout::left_to_right(egui::Align::Min),
                |ui| {
                    ui.style_mut().spacing.scroll = egui::style::ScrollStyle::floating();
                    let scroll = egui::ScrollArea::horizontal()
                        .id_salt(("pane-actions", kind))
                        .max_width(ribbon_width)
                        .min_scrolled_width(0.0)
                        .auto_shrink([false, true])
                        .show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.add_enabled_ui(available, |ui| {
                                    if kind == Kind::Record {
                                        self.ribbon_record_slider(ui);
                                    }
                                    let mut group = None;
                                    for &(action, icon, label) in kind.ribbon_actions() {
                                        if group.is_some_and(|previous| previous != action.group()) { ui.separator(); }
                                        group = Some(action.group());
                                        self.ribbon_action(ui, action, icon, label);
                                    }
                                    match kind {
                                        Kind::Data | Kind::Result => {
                                            self.ribbon_columns(ui, "Visible columns")
                                        }
                                        Kind::Sql if self.document.kind == Kind::Sql => {
                                            self.ribbon_editors(ui);
                                            self.ribbon_sql_results(ui);
                                        }
                                        _ => {}
                                    }
                                });
                                if available
                                    && matches!(kind, Kind::Data | Kind::Record)
                                    && self.document.has_drafts()
                                {
                                    self.ribbon_action(
                                        ui,
                                        Action::Review,
                                        Icon::ReviewEdits,
                                        "Review staged edits",
                                    );
                                }
                                if !self.workspace.minimized.is_empty()
                                    || self.workspace.expanded.is_some()
                                {
                                    self.ribbon_action(
                                        ui,
                                        Action::Retile,
                                        Icon::Arrange,
                                        "Restore all tiles",
                                    );
                                }
                                if available && self.action_enabled(Action::Discard) {
                                    if ui
                                        .add(
                                            egui::Button::new("Discard results")
                                                .min_size(egui::vec2(0.0, icons::RIBBON_BUTTON_SIZE)),
                                        )
                                        .on_hover_text("Suppress incoming results only. SQL execution and writes continue; this does not roll back changes.")
                                        .clicked()
                                    {
                                        self.dispatch_action(Action::Discard, ui.ctx());
                                    }
                                }
                            });
                        });
                    // A small fade signals that more direct actions can be scrolled
                    // into view. It never covers the fixed window controls.
                    if scroll.content_size.x > scroll.inner_rect.width() + scroll.state.clamped_offset().x + 1.0 {
                        let rect = scroll.inner_rect;
                        let width = rect.width().min(24.0);
                        for step in 0..12 {
                            let x = rect.right() - width + width * step as f32 / 12.0;
                            ui.painter().rect_filled(egui::Rect::from_min_max(egui::pos2(x, rect.top()), egui::pos2(x + width / 12.0 + 0.5, rect.bottom())), 0.0,
                                ui.visuals().faint_bg_color.gamma_multiply(step as f32 / 12.0));
                        }
                    }
                },
            );
            ui.separator();
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

    fn ribbon_title(&mut self, ui: &mut egui::Ui, kind: Kind, available: bool) {
        if kind == Kind::Data {
            self.ribbon_table_selector(ui, available);
            return;
        }
        if kind == Kind::Record {
            self.ribbon_field_selector(ui, available);
            return;
        }
        let title = if available && let Some(name) = &self.document.object {
            format!("{} · {name}", kind.label())
        } else {
            kind.label().to_owned()
        };
        let name_width = ui
            .painter()
            .layout_no_wrap(
                kind.label().into(),
                crate::appearance::proportional(ui.ctx(), icons::RIBBON_TEXT_SIZE),
                ui.visuals().text_color(),
            )
            .size()
            .x;
        let visible = if ui.available_width() >= name_width + 64.0 {
            title.as_str()
        } else {
            kind.label()
        };
        ui.add(egui::Label::new(egui::RichText::new(visible).strong()).truncate())
            .on_hover_text(title);
    }

    fn ribbon_record_slider(&mut self, ui: &mut egui::Ui) {
        let count = self.document.result.rows.len();
        let (row, column) = self.document.inspect.unwrap_or((0, 0));
        let mut position = if count == 0 { 0 } else { (row + 1).min(count) };
        let response = ui
            .scope(|ui| {
                ui.spacing_mut().slider_width = 140.0;
                ui.add_enabled(
                    count > 1,
                    egui::Slider::new(&mut position, usize::from(count > 0)..=count)
                        .step_by(1.0)
                        .suffix(format!(" / {count}")),
                )
            })
            .inner;
        response.widget_info(|| {
            egui::WidgetInfo::slider(
                count > 1 && ui.is_enabled(),
                position as f64,
                "Record position",
            )
        });
        let changed = response.changed();
        response.on_hover_text("Move through loaded records while keeping the selected field. Drag the slider or enter a record number.");
        if changed && count > 0 {
            self.select_cell(position - 1, column, false);
            self.document.tools.record_scroll_to = Some(column);
        }
    }

    fn ribbon_field_selector(&mut self, ui: &mut egui::Ui, available: bool) {
        let current = self.document.inspect.filter(|(row, col)| {
            available
                && self.document.result.rows.get(*row).is_some()
                && self.document.result.columns.get(*col).is_some()
        });
        let mut selected = current.map(|(_, col)| col);
        let mut chosen = false;
        let name = selected
            .and_then(|col| self.document.result.columns.get(col))
            .map_or("Choose field", |column| column.name.as_str());
        let response = ui
            .add_enabled_ui(current.is_some(), |ui| {
                egui::ComboBox::from_id_salt("record-field-selector")
                    .selected_text(name)
                    .width(ui.available_width())
                    .truncate()
                    .show_ui(ui, |ui| {
                        ui.style_mut().override_font_id = None;
                        ui.spacing_mut().interact_size.y = 28.0;
                        for (col, column) in self.document.result.columns.iter().enumerate() {
                            ui.push_id(col, |ui| {
                                chosen |= ui
                                    .selectable_value(&mut selected, Some(col), &column.name)
                                    .clicked();
                            });
                        }
                    })
                    .response
            })
            .inner;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(egui::Role::ComboBox, current.is_some(), "Choose field")
        });
        response.on_hover_text(name);
        if let (Some((row, _)), Some(col)) = (current, selected)
            && chosen
        {
            self.open_cell(row, col);
            self.document.tools.record_scroll_to = Some(col);
        }
    }

    fn ribbon_table_selector(&mut self, ui: &mut egui::Ui, available: bool) {
        let current = available.then(|| self.document.object.clone()).flatten();
        let mut selected = current.clone();
        let response = egui::ComboBox::from_id_salt("table-selector")
            .selected_text(current.as_deref().unwrap_or("Choose table"))
            .width(ui.available_width())
            .truncate()
            .show_ui(ui, |ui| {
                ui.style_mut().override_font_id = None;
                ui.spacing_mut().interact_size.y = 28.0;
                let mut tables: Vec<_> = self
                    .schema
                    .iter()
                    .filter(|object| matches!(object.kind.as_str(), "table" | "view"))
                    .collect();
                tables.sort_by(|a, b| a.name.cmp(&b.name));
                for table in tables {
                    ui.selectable_value(&mut selected, Some(table.name.clone()), &table.name)
                        .on_hover_text(format!("{} · {}", table.name, table.kind));
                }
            })
            .response;
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Choose table"));
        response.on_hover_text(current.as_deref().unwrap_or("Choose a table to browse"));
        if selected != current
            && let Some(name) = selected
        {
            self.browse_table(name);
        }
    }
}

impl Action {
    fn group(self) -> u8 {
        use Action::*;
        match self {
            Refresh | Count | FitMap | ArrangeMap | Run | RunSelection | Explain | Bytecode => 0,
            Filter | FitColumns | ShowColumns | Inspect | MapColumns | SystemTables | Find
            | Parameters | Completion => 1,
            Export | CopyRows | CopyDdl | CopyValue | CopyRecord | SaveSql | NewSql => 2,
            ZoomOut | Zoom100 | ZoomIn => 4,
            _ => 3,
        }
    }
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
                (Count, Icon::Count, "Count table rows (full scan)"),
                (Filter, Icon::Filter, "Filter"),
                (FitColumns, Icon::FitColumns, "Fit columns"),
                (Inspect, Icon::Selection, "Inspect selected record"),
                (ShowColumns, Icon::Complete, "Show all columns"),
                (Export, Icon::Export, "Export"),
                (CopyRows, Icon::Copy, "Copy retained rows"),
                (Insert, Icon::Plus, "New row (staged)"),
            ],
            Kind::Result => &[
                (Export, Icon::Export, "Export query results"),
                (CopyRows, Icon::Copy, "Copy query results"),
                (FitColumns, Icon::FitColumns, "Fit result columns"),
                (Inspect, Icon::Selection, "Inspect selected result record"),
                (ShowColumns, Icon::Complete, "Show all result columns"),
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
                (ZoomOut, Icon::Minimize, "Zoom out"),
                (Zoom100, Icon::Fit, "Reset zoom to 100%"),
                (ZoomIn, Icon::Plus, "Zoom in"),
            ],
            Kind::Sql => &[
                (Run, Icon::Run, "Run"),
                (
                    RunSelection,
                    Icon::Selection,
                    "Run selection / statement at cursor",
                ),
                (Explain, Icon::Explain, "Explain query plan"),
                (Bytecode, Icon::Bytecode, "Explain bytecode"),
                (Parameters, Icon::Parameters, "Typed parameters"),
                (Completion, Icon::Complete, "Completions"),
                (SaveSql, Icon::Saved, "Save query"),
                (NewSql, Icon::Plus, "New SQL editor"),
                (Begin, Icon::BeginTransaction, "Begin editor transaction"),
                (Commit, Icon::CommitTransaction, "Commit transaction"),
                (Rollback, Icon::Undo, "Roll back transaction"),
            ],
            Kind::Record => &[
                (CopyRecord, Icon::Copy, "Copy record"),
                (CloneRecord, Icon::CloneRecord, "Clone record"),
                (DeleteRecord, Icon::Trash, "Stage deletion"),
                (CopyValue, Icon::CopyValue, "Copy selected value"),
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
        let pointer = egui::pos2(minimize.left() - 30.0, minimize.center().y);
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
