use super::{
    App,
    icons::{self, Icon},
    shell::ToolPanel,
    tiling::Kind,
};
use eframe::egui;
use tursobench_core::TransactionState;

const DISCARD_HINT: &str = "Suppress incoming results only. SQL execution and writes continue; this does not roll back changes.";

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
    RunAll,
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
        self.database.document.active.is_some()
            || self.database.documents.values().any(|d| d.active.is_some())
    }
    pub(super) fn action_enabled(&self, action: Action) -> bool {
        match action {
            Action::ZoomOut => self.database.ui.diagram.camera.zoom() > super::diagram::MIN_ZOOM,
            Action::ZoomIn => self.database.ui.diagram.camera.zoom() < super::diagram::MAX_ZOOM,
            Action::Run
            | Action::RunAll
            | Action::Explain
            | Action::Bytecode
            | Action::Begin
            | Action::Commit
            | Action::Rollback => {
                self.requests_enabled()
                    && !self.executing()
                    && (!matches!(action, Action::Commit)
                        || self.database.transaction == TransactionState::Active)
            }
            Action::Count => {
                self.requests_enabled()
                    && self.database.document.kind == Kind::Data
                    && self.database.document.object.is_some()
            }
            Action::Refresh => self.requests_enabled(),
            Action::Insert => self.can_edit(),
            Action::CloneRecord | Action::DeleteRecord => {
                self.can_edit() && self.database.document.inspect.is_some()
            }
            Action::CopyRecord | Action::Inspect | Action::CopyValue => {
                self.database.document.inspect.is_some()
            }
            Action::CopyRows | Action::Export => !self.database.document.result.columns.is_empty(),
            Action::Discard => {
                self.database.document.active.is_some()
                    || self
                        .database
                        .documents
                        .values()
                        .any(|d| d.source == Some(self.database.document.id) && d.active.is_some())
            }
            _ => true,
        }
    }
    pub(super) fn dispatch_action(&mut self, action: Action, ctx: &egui::Context) {
        if !self.action_enabled(action) {
            return;
        }
        let document = &mut self.database.document;
        let diagram = &mut self.database.ui.diagram;
        match action {
            Action::NewSql => self.open_sql_window(true),
            Action::Refresh => self.refresh_pane(),
            Action::Filter => {
                document.ui.filter_open = !document.ui.filter_open;
                document.tools.filter_focus = document.ui.filter_open;
            }
            Action::FitColumns => {
                document.tools.columns.manual.clear();
                document.tools.columns.refit = true;
            }
            Action::ShowColumns => document.tools.columns.hidden.clear(),
            Action::Export => self.database.ui.export_open = true,
            Action::CopyRows => self.copy_results(ctx, crate::export::Format::Tsv),
            Action::Count => {
                if let Some(name) = document.object.clone() {
                    self.count_table(name);
                }
            }
            Action::Inspect => {
                if let Some((row, col)) = document.inspect {
                    self.open_record_window(row, col);
                }
            }
            Action::FitMap => diagram.camera.fit = true,
            Action::ArrangeMap => diagram.arrange_nodes(),
            Action::MapColumns => diagram.toggle_columns(),
            Action::SystemTables => diagram.toggle_system(),
            Action::Find => {
                self.sidebar_search.focus = true;
                self.database.ui.sidebar = ToolPanel::Schema;
            }
            Action::Zoom100 => diagram.camera.reset_zoom(),
            Action::ZoomOut => diagram.camera.step_zoom(1.0 / 1.25),
            Action::ZoomIn => diagram.camera.step_zoom(1.25),
            Action::Run => self.run_sql(),
            Action::RunAll => self.run_all(false),
            Action::Explain => self.run_current(Some(false)),
            Action::Bytecode => self.run_current(Some(true)),
            Action::Parameters => document.ui.params_open = !document.ui.params_open,
            Action::Completion => document.tools.complete = !document.tools.complete,
            Action::SaveSql => self.database.ui.sidebar = ToolPanel::Saved,
            Action::CopyDdl => {
                if let Some(ddl) = self.shown_ddl() {
                    ctx.copy_text(ddl);
                }
            }
            Action::CopyRecord | Action::CloneRecord | Action::DeleteRecord => {
                self.record_action(action, ctx);
            }
            Action::CopyValue => {
                if let Some((r, c)) = document.inspect
                    && let Some(value) = document.result.rows.get(r).and_then(|row| row.get(c))
                {
                    ctx.copy_text(crate::export::cell_text(value));
                }
            }
            Action::Retile => self.retile(),
            Action::Insert => self.begin_insert(),
            Action::Review => {
                self.focus_drafts();
                self.database.document.show_preview = true;
            }
            Action::Begin => self.run_transaction("BEGIN"),
            Action::Commit => self.run_transaction("COMMIT"),
            Action::Rollback => self.run_transaction("ROLLBACK"),
            Action::Discard => self.discard_results(),
        }
    }
    /// Refresh what the active pane shows: the schema for the map, the
    /// inspected object for Structure, and otherwise the rows.
    fn refresh_pane(&mut self) {
        let kind = if self.database.workspace.active == Kind::Structure {
            Kind::Structure
        } else {
            self.database.document.kind
        };
        match kind {
            Kind::Map => {
                self.refresh_catalog();
                self.load_diagram();
            }
            Kind::Structure => {
                if let Some(name) = self.database.document.object.clone() {
                    self.request_inspection(name);
                }
                self.refresh_catalog();
            }
            _ => self.reload(),
        }
    }
    /// The DDL of the object whose definition is shown, else of the open object.
    fn shown_ddl(&self) -> Option<String> {
        let document = &self.database.document;
        let name = document
            .tools
            .ddl
            .as_ref()
            .map(|o| o.name.as_str())
            .or(document.object.as_deref())?;
        self.database
            .schema
            .iter()
            .find(|o| o.name == name)
            .and_then(|o| o.ddl.clone())
    }
    fn record_action(&mut self, action: Action, ctx: &egui::Context) {
        let Some((row, _)) = self.database.document.inspect else {
            return;
        };
        match action {
            Action::CopyRecord => self.copy_row(ctx, row),
            Action::CloneRecord => self.clone_row(row),
            _ => self.stage_delete(row),
        }
    }
    /// Stop delivering the running result of this document, or of a result
    /// tile it feeds; the statement itself keeps running.
    fn discard_results(&mut self) {
        let focus = self.database.workspace.focused;
        let current = self.database.document.id;
        let target = if self.database.document.active.is_some() {
            Some(current)
        } else {
            self.database
                .documents
                .values()
                .find(|d| d.source == Some(current) && d.active.is_some())
                .map(|d| d.id)
        };
        let Some(id) = target else {
            return;
        };
        self.switch_document(id);
        if let (Some(session), Some(ticket)) =
            (&self.database.session, self.database.document.active)
        {
            session.discard_results(ticket.request);
            self.database.document.discard_ticket = Some(ticket);
            self.database.document.result.rows.clear();
            self.database.status = "Result delivery suppressed; SQL execution continues. This does not roll back writes.".into();
        }
        self.switch_document(focus);
    }
    pub(super) fn focus_drafts(&mut self) {
        let id = if self.database.document.has_drafts() {
            Some(self.database.document.id)
        } else {
            self.database
                .documents
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
            Action::Filter => Some(self.database.document.ui.filter_open),
            Action::MapColumns => Some(self.database.ui.diagram.all_columns),
            Action::SystemTables => Some(self.database.ui.diagram.show_system),
            Action::Parameters => Some(self.database.document.ui.params_open),
            Action::Completion => Some(self.database.document.tools.complete),
            _ => None,
        };
        let response = ui
            .add_enabled_ui(self.action_enabled(action), |ui| match action {
                Action::RunAll => self.run_all_menu(ui, true),
                Action::Zoom100 => zoom_button(ui, self.database.ui.diagram.camera.zoom(), label),
                Action::Run => icons::ribbon_button(ui, icon, label, action.shortcut(), selected)
                    .on_hover_text("Selection or current statement · Ctrl Shift Enter"),
                _ => icons::ribbon_button(ui, icon, label, action.shortcut(), selected),
            })
            .inner;
        #[cfg(test)]
        if ui.clip_rect().contains_rect(response.rect) {
            self.database.ui.ribbon_actions.push(response.rect);
        }
        if response.clicked() && action != Action::RunAll {
            // Export and the detached record inspector read the focused document later.
            if self.database.document.kind == Kind::Result {
                self.focus_window(self.database.document.id);
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
                    let hidden = &mut self.database.document.tools.columns.hidden;
                    for (i, column) in self.database.document.result.columns.iter().enumerate() {
                        ui.push_id(i, |ui| {
                            let mut shown = !hidden.contains(&i);
                            if crate::app::presentation::checkbox(ui, &mut shown, &column.name)
                                .changed()
                            {
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
        let current = self.database.document.id;
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
        let editor = self.database.document.id;
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
        if kind == Kind::Chat {
            self.assistant_header(ui, true);
            return;
        }
        if kind == Kind::Record
            && self.database.document.inspect.is_none()
            && !self.database.document.result.rows.is_empty()
        {
            self.database.document.inspect = Some((0, 0));
        }
        // Keep tile identity and window controls fixed while the action ribbon scrolls.
        // Never drop actions based on width or replace them with an overflow menu.
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = super::presentation::BAR_SPACING;
            ui.spacing_mut().button_padding.x = super::presentation::BAR_INSET;
            ui.style_mut().override_font_id = Some(crate::appearance::proportional(
                ui.ctx(),
                icons::RIBBON_TEXT_SIZE,
            ));
            ui.spacing_mut().interact_size.y = icons::RIBBON_BUTTON_SIZE;
            let show_restore = self.tiles_restorable();
            let controls_width = 2.0 * icons::RIBBON_BUTTON_SIZE
                + 20.0
                + if show_restore {
                    icons::RIBBON_BUTTON_SIZE + ui.spacing().item_spacing.x
                } else {
                    0.0
                };
            let leading = if kind == Kind::Data {
                (super::presentation::CONTENT_INSET - ui.spacing().button_padding.x).min(
                    (ui.available_width() - controls_width - 64.0)
                        .max(super::presentation::BAR_INSET),
                )
            } else {
                super::presentation::BAR_INSET
            };
            ui.add_space(leading);
            let show_title = matches!(kind, Kind::Data | Kind::Result);
            if show_title {
                let remaining = (ui.available_width() - controls_width).max(0.0);
                let label_width =
                    super::presentation::text_width(ui, kind.label(), icons::RIBBON_TEXT_SIZE);
                let mut title_width = (remaining * 0.25)
                    .max(label_width + 4.0)
                    .min(200.0)
                    .min((remaining - 32.0).max(0.0));
                if kind == Kind::Data {
                    title_width = title_width.max(48.0).min((remaining - 6.0).max(0.0));
                }
                ui.allocate_ui_with_layout(
                    egui::vec2(title_width, icons::RIBBON_BUTTON_SIZE),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| self.ribbon_title(ui, kind, available),
                );
            }
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
                                self.pane_actions(ui, kind, available);
                            });
                        });
                    // A small fade signals that more direct actions can be scrolled
                    // into view. It never covers the fixed window controls.
                    if scroll.content_size.x
                        > scroll.inner_rect.width() + scroll.state.clamped_offset().x + 1.0
                    {
                        let rect = scroll.inner_rect;
                        let width = rect.width().min(24.0);
                        for step in 0..12 {
                            let x = rect.right() - width + width * step as f32 / 12.0;
                            ui.painter().rect_filled(
                                egui::Rect::from_min_max(
                                    egui::pos2(x, rect.top()),
                                    egui::pos2(x + width / 12.0 + 0.5, rect.bottom()),
                                ),
                                0.0,
                                ui.visuals()
                                    .faint_bg_color
                                    .gamma_multiply(step as f32 / 12.0),
                            );
                        }
                    }
                },
            );
            self.pane_window_controls(ui, kind, show_restore);
        });
    }

    fn pane_actions(&mut self, ui: &mut egui::Ui, kind: Kind, available: bool) {
        ui.add_enabled_ui(available, |ui| {
            if kind == Kind::Record {
                self.ribbon_record_stepper(ui);
            }
            let mut group = None;
            for &(action, icon, label) in kind.ribbon_actions() {
                if group.is_some_and(|previous| previous != action.group()) {
                    ui.separator();
                }
                group = Some(action.group());
                self.ribbon_action(ui, action, icon, label);
            }
            match kind {
                Kind::Data | Kind::Result => self.ribbon_columns(ui, "Visible columns"),
                Kind::Sql if self.database.document.kind == Kind::Sql => {
                    self.ribbon_editors(ui);
                    self.ribbon_sql_results(ui);
                }
                _ => {}
            }
            if kind == Kind::Data {
                let edit = icons::ribbon_button(ui, Icon::EditCells, "Edit cells", "", None)
                    .on_hover_text("Paste, mapped new rows, typed fill, NULL and undo");
                egui::Popup::menu(&edit).show(|ui| self.spreadsheet_menu(ui));
            }
        });
        if available
            && matches!(kind, Kind::Data | Kind::Record)
            && self.database.document.has_drafts()
        {
            self.ribbon_action(ui, Action::Review, Icon::ReviewEdits, "Review staged edits");
        }
        if available
            && self.action_enabled(Action::Discard)
            && ui
                .add(
                    egui::Button::new("Discard results")
                        .min_size(egui::vec2(0.0, icons::RIBBON_BUTTON_SIZE)),
                )
                .on_hover_text(DISCARD_HINT)
                .clicked()
        {
            self.dispatch_action(Action::Discard, ui.ctx());
        }
    }

    fn pane_window_controls(&mut self, ui: &mut egui::Ui, kind: Kind, show_restore: bool) {
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.add_space(super::presentation::BAR_INSET);
            let expand = icons::ribbon_button(
                ui,
                Icon::Fit,
                if self.database.workspace.expanded == Some(kind) {
                    "Restore workspace"
                } else {
                    "Expand to workspace"
                },
                "Ctrl Alt Enter",
                None,
            );
            let minimize = icons::ribbon_button(ui, Icon::Minimize, "Minimize to header", "", None);
            if show_restore {
                self.ribbon_action(ui, Action::Retile, Icon::RestoreTiles, "Restore all tiles");
            }
            ui.separator();
            #[cfg(test)]
            self.database
                .workspace
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
        let title = if available && let Some(name) = &self.database.document.object {
            format!("{} · {name}", kind.label())
        } else {
            kind.label().to_owned()
        };
        let name_width = super::presentation::text_width(ui, kind.label(), icons::RIBBON_TEXT_SIZE);
        let visible = if ui.available_width() >= name_width + 64.0 {
            title.as_str()
        } else {
            kind.label()
        };
        ui.add(egui::Label::new(egui::RichText::new(visible).strong()).truncate())
            .on_hover_text(title);
    }

    fn ribbon_record_stepper(&mut self, ui: &mut egui::Ui) {
        let count = self.database.document.result.rows.len();
        let (row, column) = self.database.document.inspect.unwrap_or((0, 0));
        let mut position = if count == 0 { 0 } else { (row + 1).min(count) };
        let previous = ui.add_enabled(
            position > 1,
            egui::Button::new("‹").min_size(egui::vec2(20.0, icons::RIBBON_BUTTON_SIZE)),
        );
        previous.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                position > 1 && ui.is_enabled(),
                "Previous record",
            )
        });
        let mut changed = previous.on_hover_text("Previous loaded record").clicked();
        if changed {
            position -= 1;
        }
        let finish_position =
            ui.input(|i| i.key_pressed(egui::Key::Enter) || i.key_pressed(egui::Key::Escape));
        let response = ui.add_enabled(
            count > 0,
            egui::DragValue::new(&mut position)
                .range(usize::from(count > 0)..=count)
                .speed(0.0)
                .custom_formatter(move |n, _| format!("{} / {count}", n as usize))
                .custom_parser(|text| text.split('/').next()?.trim().parse().ok()),
        );
        response.widget_info(|| {
            egui::WidgetInfo::slider(
                count > 0 && ui.is_enabled(),
                position as f64,
                "Record position",
            )
        });
        if finish_position && (response.has_focus() || response.lost_focus()) {
            response.surrender_focus();
            ui.input_mut(|i| {
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter);
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape);
            });
        }
        changed |= response.changed();
        response.on_hover_text("Click to enter a loaded record number");
        let next = ui.add_enabled(
            position < count,
            egui::Button::new("›").min_size(egui::vec2(20.0, icons::RIBBON_BUTTON_SIZE)),
        );
        next.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::Role::Button,
                position < count && ui.is_enabled(),
                "Next record",
            )
        });
        if next.on_hover_text("Next loaded record").clicked() {
            position += 1;
            changed = true;
        }
        if changed && count > 0 {
            self.select_cell(position - 1, column, false);
            self.database.document.tools.record_scroll_to = Some(column);
        }
    }

    fn ribbon_table_selector(&mut self, ui: &mut egui::Ui, available: bool) {
        let current = self
            .database
            .document
            .object
            .as_deref()
            .filter(|_| available);
        let mut selected = current;
        let response = egui::ComboBox::from_id_salt("table-selector")
            .selected_text(current.unwrap_or("Choose table"))
            .width(ui.available_width())
            .truncate()
            .show_ui(ui, |ui| {
                ui.style_mut().override_font_id = None;
                ui.spacing_mut().interact_size.y = 28.0;
                let mut tables: Vec<_> = self
                    .database
                    .schema
                    .iter()
                    .filter(|object| matches!(object.kind.as_str(), "table" | "view"))
                    .collect();
                tables.sort_by(|a, b| a.name.cmp(&b.name));
                for table in tables {
                    ui.selectable_value(&mut selected, Some(table.name.as_str()), &table.name)
                        .on_hover_text(format!("{} · {}", table.name, table.kind));
                }
            })
            .response;
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::ComboBox, true, "Choose table"));
        response.on_hover_text(current.unwrap_or("Choose a table to browse"));
        if selected != current
            && let Some(name) = selected
        {
            self.browse_table(name.to_owned());
        }
    }
}

impl Action {
    fn group(self) -> u8 {
        match self {
            Action::Refresh
            | Action::Count
            | Action::FitMap
            | Action::ArrangeMap
            | Action::Run
            | Action::RunAll
            | Action::Explain
            | Action::Bytecode => 0,
            Action::Filter
            | Action::FitColumns
            | Action::ShowColumns
            | Action::Inspect
            | Action::MapColumns
            | Action::SystemTables
            | Action::Find
            | Action::Parameters
            | Action::Completion => 1,
            Action::Export
            | Action::CopyRows
            | Action::CopyDdl
            | Action::CopyValue
            | Action::CopyRecord
            | Action::SaveSql
            | Action::NewSql => 2,
            Action::ZoomOut | Action::Zoom100 | Action::ZoomIn => 4,
            _ => 3,
        }
    }
    fn shortcut(self) -> &'static str {
        match self {
            Action::Refresh => "F5",
            Action::Filter => "Ctrl F",
            Action::Export => "Ctrl E",
            Action::CopyRows => "Ctrl Shift C",
            Action::Inspect => "Space",
            Action::Find => "Ctrl P",
            Action::Run => "Ctrl Enter",
            Action::Completion => "Ctrl Space",
            Action::SaveSql => "Ctrl S",
            _ => "",
        }
    }
}

impl Kind {
    fn ribbon_actions(self) -> &'static [(Action, Icon, &'static str)] {
        match self {
            Kind::Chat => &[],
            Kind::Data => &[
                (Action::Refresh, Icon::Refresh, "Refresh"),
                (Action::Count, Icon::Count, "Count table rows (full scan)"),
                (Action::Filter, Icon::Filter, "Filter"),
                (Action::FitColumns, Icon::FitColumns, "Fit columns"),
                (Action::Inspect, Icon::Selection, "Inspect selected record"),
                (Action::ShowColumns, Icon::Complete, "Show all columns"),
                (Action::Export, Icon::Export, "Export"),
                (Action::CopyRows, Icon::Copy, "Copy retained rows"),
                (Action::Insert, Icon::Plus, "New row (staged)"),
            ],
            Kind::Result => &[
                (Action::Export, Icon::Export, "Export query results"),
                (Action::CopyRows, Icon::Copy, "Copy query results"),
                (Action::FitColumns, Icon::FitColumns, "Fit result columns"),
                (
                    Action::Inspect,
                    Icon::Selection,
                    "Inspect selected result record",
                ),
                (
                    Action::ShowColumns,
                    Icon::Complete,
                    "Show all result columns",
                ),
            ],
            Kind::Structure => &[
                (Action::CopyDdl, Icon::Copy, "Copy DDL"),
                (Action::Refresh, Icon::Refresh, "Refresh"),
            ],
            Kind::Map => &[
                (Action::FitMap, Icon::Map, "Fit relationships"),
                (Action::ArrangeMap, Icon::Arrange, "Arrange nodes"),
                (Action::MapColumns, Icon::Columns, "Columns"),
                (Action::Find, Icon::Search, "Find"),
                (Action::Refresh, Icon::Refresh, "Refresh schema"),
                (Action::SystemTables, Icon::System, "Toggle system tables"),
                (Action::ZoomOut, Icon::Minimize, "Zoom out"),
                (Action::Zoom100, Icon::Fit, "Reset zoom to 100%"),
                (Action::ZoomIn, Icon::Plus, "Zoom in"),
            ],
            Kind::Sql => &[
                (Action::Run, Icon::Run, "Run"),
                (Action::RunAll, Icon::RunAll, "Run all"),
                (Action::Explain, Icon::Explain, "Explain query plan"),
                (Action::Bytecode, Icon::Bytecode, "Explain bytecode"),
                (Action::Parameters, Icon::Parameters, "Typed parameters"),
                (Action::Completion, Icon::Complete, "Completions"),
                (Action::SaveSql, Icon::Saved, "Save query"),
                (Action::NewSql, Icon::Plus, "New SQL editor"),
                (
                    Action::Begin,
                    Icon::BeginTransaction,
                    "Begin editor transaction",
                ),
                (
                    Action::Commit,
                    Icon::CommitTransaction,
                    "Commit transaction",
                ),
                (Action::Rollback, Icon::Undo, "Roll back transaction"),
            ],
            Kind::Record => &[
                (Action::CopyRecord, Icon::Copy, "Copy record"),
                (Action::CloneRecord, Icon::CloneRecord, "Clone record"),
                (Action::DeleteRecord, Icon::Trash, "Stage deletion"),
                (Action::CopyValue, Icon::CopyValue, "Copy selected value"),
            ],
        }
    }
}

/// The map's zoom as a percentage; clicking it resets to 100%.
fn zoom_button(ui: &mut egui::Ui, zoom: f32, label: &str) -> egui::Response {
    let percent = zoom * 100.0;
    let text = if percent < 1.0 {
        format!("{percent:.2}%")
    } else {
        format!("{percent:.0}%")
    };
    let response = ui.add(
        egui::Button::new(egui::RichText::new(text).size(crate::appearance::text_size(
            ui.ctx(),
            icons::RIBBON_TEXT_SIZE,
        )))
        .min_size(egui::Vec2::splat(icons::RIBBON_BUTTON_SIZE)),
    );
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    response.on_hover_text("Current zoom · click to reset to 100%")
}

#[cfg(test)]
mod tests {
    use super::{App, Kind, egui};

    #[test]
    fn narrow_ribbon_scrolls_to_a_direct_action_and_keeps_window_controls_fixed() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.workspace.expanded = Some(Kind::Map);
        let mut time = 0.0;
        let mut render = |app: &mut App, events| {
            time += 0.1;
            app.database.workspace.controls.clear();
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
        let output = render(&mut app, vec![]);
        let restore = crate::app::test_ui::control(&output, "Restore all tiles");
        let (_, minimize, expand) = app.database.workspace.controls[0];
        assert!(restore.right() <= minimize.left());
        assert!(minimize.left() - restore.right() <= 6.0);
        let pointer = egui::pos2(restore.left() - 30.0, minimize.center().y);
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
        let target = crate::app::test_ui::control(&output, "Toggle system tables").center();
        assert!(
            target.x > 0.0 && target.x < minimize.left(),
            "action must be scrolled into view: {target:?}"
        );
        // The scrollbar reveal animation can shift vertical alignment by a fraction of a point.
        for (before, after) in [
            (
                restore,
                crate::app::test_ui::control(&output, "Restore all tiles"),
            ),
            (minimize, app.database.workspace.controls[0].1),
            (expand, app.database.workspace.controls[0].2),
        ] {
            assert_eq!(before.left(), after.left());
            assert_eq!(before.size(), after.size());
            assert!((before.top() - after.top()).abs() < 1.0);
        }
        assert!(!app.database.ui.diagram.show_system);
        for pressed in [true, false] {
            render(&mut app, crate::app::test_ui::pointer(target, pressed));
        }
        assert!(
            app.database.ui.diagram.show_system,
            "visible ribbon button must dispatch its action without a popup"
        );
    }
}
