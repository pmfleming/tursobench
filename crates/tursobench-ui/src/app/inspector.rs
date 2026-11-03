use super::{
    App, View,
    icons::{self, Icon},
    results, shell,
};
use eframe::egui;
use tursobench_core::Value;

use results::{CellKind, code_view, detail_text};
impl App {
    pub(super) fn cell_inspector(&mut self, ui: &mut egui::Ui, row: usize, col: usize) {
        let Some(value) = self.document.result.rows.get(row).and_then(|r| r.get(col)) else {
            ui.label("Selection no longer belongs to the retained result");
            return;
        };
        ui.strong(
            self.document
                .result
                .columns
                .get(col)
                .map_or("Cell", |c| c.name.as_str()),
        );
        if self.document.view == View::Browse
            && let Some(table) = &self.document.table
        {
            ui.label(format!("Table: {}", table.name));
        }
        ui.small(format!(
            "Retained row {} · {} budgeted bytes · {:?} result",
            row + 1,
            value.bytes(),
            self.document.view
        ));
        let staged = self.document.drafts.as_ref().and_then(|d| {
            self.document
                .result
                .rows
                .get(row)
                .and_then(|r| d.displayed(r, col))
        });
        let typed = |value: &Value| match value {
            Value::Real(n) if !n.is_finite() => {
                format!("Real({n}) — inspectable/exportable, not editable as a non-finite real")
            }
            _ => serde_json::to_string_pretty(value).unwrap_or_default(),
        };
        if let Some(value) = staged {
            ui.colored_label(ui.visuals().warn_fg_color, "Staged value — not yet written");
            let text = typed(value);
            code_view(ui, &text, 3);
        }
        ui.label("Stored value");
        let text = typed(value);
        egui::ScrollArea::vertical()
            .id_salt("stored-cell")
            .max_height(180.0)
            .show(ui, |ui| {
                code_view(ui, &text, 3);
            });
        ui.horizontal_wrapped(|ui| {
            if ui.button("Copy stored value").clicked() {
                ui.ctx().copy_text(crate::export::cell_text(value));
            }
            if ui.button("Record").clicked() {
                self.document.tools.detail = Some(row);
                self.document.ui.inspector = shell::DetailMode::Record;
            }
        });
        if self.can_edit() {
            ui.separator();
            ui.strong("Change value · staged locally");
            egui::ComboBox::from_id_salt("cell-kind")
                .selected_text(self.document.tools.cell_kind.label())
                .show_ui(ui, |ui| {
                    for kind in [
                        CellKind::Null,
                        CellKind::Integer,
                        CellKind::Real,
                        CellKind::Text,
                        CellKind::Blob,
                    ] {
                        ui.selectable_value(&mut self.document.tools.cell_kind, kind, kind.label());
                    }
                });
            if self.document.tools.cell_kind != CellKind::Null {
                ui.add(
                    egui::TextEdit::multiline(&mut self.document.tools.cell_input)
                        .desired_width(ui.available_width())
                        .desired_rows(3),
                );
            }
            if ui.button("Stage cell change").clicked() {
                match self
                    .document
                    .tools
                    .cell_kind
                    .parse(&self.document.tools.cell_input)
                {
                    Ok(value) => self.stage_cell(row, col, value),
                    Err(e) => self.status = e,
                }
            }
            ui.horizontal_wrapped(|ui| {
                if icons::button(ui, Icon::Undo, "Revert cell draft", "").clicked()
                    && let (Some(d), Some(original)) = (
                        &mut self.document.drafts,
                        self.document.result.rows.get(row),
                    )
                {
                    d.revert_cell(original, col);
                }
                if ui.button("Revert row draft").clicked()
                    && let (Some(d), Some(original)) = (
                        &mut self.document.drafts,
                        self.document.result.rows.get(row),
                    )
                {
                    d.revert_row(original);
                }
                if icons::control(ui, Icon::Trash, "Stage row deletion", "", false, true).clicked()
                {
                    self.stage_delete(row);
                }
                if ui.button("Clone as new row…").clicked() {
                    self.clone_row(row);
                }
            });
        } else {
            ui.small("Editing requires writable access, editor autocommit, a complete controlled non-engine-managed table result with a proven key, and no pending/uncertain commit. Arbitrary SQL results remain read-only.");
        }
        if self.document.view == View::Browse
            && let Some(info) = &self.document.tools.inspection
        {
            ui.separator();
            ui.strong("References");
            let mut selected = None;
            for key in &info.foreign_keys {
                if ui
                    .button(format!(
                        "Parent: {} ({})",
                        key.target,
                        key.columns
                            .iter()
                            .map(|(c, _)| c.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                    .clicked()
                {
                    selected = Some(key.clone());
                }
            }
            if let Some(key) = selected {
                self.follow_key(row, key);
            }
            ui.small("NULL and unresolved keys are never guessed. Commit or discard drafts before following a reference.");
        }
    }
    pub(super) fn record_inspector(&mut self, ui: &mut egui::Ui, row: usize) {
        ui.strong(format!("Record {} · retained engine values", row + 1));
        if self.document.view == View::Browse
            && let Some(table) = &self.document.table
        {
            ui.label(format!("Table: {}", table.name));
        }
        if icons::button(ui, Icon::Copy, "Copy row as annotated TSV", "").clicked() {
            self.copy_row(ui.ctx(), row);
        }
        if let Some(info) = self
            .document
            .tools
            .inspection
            .as_ref()
            .filter(|i| {
                self.document.view == View::Browse
                    && self
                        .document
                        .table
                        .as_ref()
                        .is_some_and(|t| t.name == i.table.name)
            })
            .cloned()
        {
            for key in info.foreign_keys {
                if ui
                    .add_enabled(
                        !self.has_drafts() && self.document.active.is_none(),
                        egui::Button::new(format!("Parent: {}", key.target)),
                    )
                    .clicked()
                {
                    self.follow_key(row, key);
                    return;
                }
            }
        }
        let mut inspect = None;
        let mut edit = None;
        if let Some(values) = self.document.result.rows.get(row).cloned() {
            egui::Grid::new("compact-record")
                .num_columns(2)
                .spacing([8.0, 1.0])
                .show(ui, |ui| {
                    for (col, value) in values.iter().enumerate() {
                        let name = &self.document.result.columns[col].name;
                        ui.add_sized(
                            [100.0, 25.0],
                            egui::Label::new(egui::RichText::new(name).monospace()).truncate(),
                        );
                        let shown = self
                            .document
                            .drafts
                            .as_ref()
                            .and_then(|d| d.displayed(&values, col))
                            .unwrap_or(value);
                        if let Some(inline) = self
                            .document
                            .tools
                            .inline
                            .as_mut()
                            .filter(|e| e.row == row && e.column == col)
                        {
                            let response = ui.add_sized(
                                [ui.available_width().min(240.0), 25.0],
                                egui::TextEdit::singleline(&mut inline.input),
                            );
                            if inline.focus {
                                response.request_focus();
                                inline.focus = false;
                            }
                            if (response.has_focus() || response.lost_focus())
                                && ui.input_mut(|i| {
                                    i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)
                                })
                            {
                                match inline.kind.parse(&inline.input) {
                                    Ok(v) => edit = Some((col, v)),
                                    Err(e) => self.status = e,
                                }
                            }
                        } else {
                            let response = ui
                                .add_sized(
                                    [ui.available_width().min(240.0), 25.0],
                                    egui::Button::new(
                                        egui::RichText::new(shown.preview()).monospace(),
                                    )
                                    .frame_when_inactive(false)
                                    .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{} · {}\n{}",
                                    name,
                                    kind_name(shown),
                                    detail_text(shown)
                                ));
                            if response.clicked() {
                                inspect = Some(col);
                            }
                            if response.double_clicked() {
                                edit = Some((col, shown.clone()));
                            }
                        }
                        ui.end_row();
                    }
                });
        }
        if let Some(col) = inspect {
            self.open_cell(row, col);
        }
        if let Some((col, value)) = edit {
            if self.document.tools.inline.is_some() {
                self.stage_cell(row, col, value);
                self.document.tools.inline = None;
            } else {
                self.start_inline(row, col);
            }
        }
        self.incoming_references(ui, row);
    }
    fn incoming_references(&mut self, ui: &mut egui::Ui, row: usize) {
        self.related_list(ui, row);
    }
    pub(super) fn inspection_surfaces(&mut self, ctx: &egui::Context) {
        if self.document.ui.inspector_detached && self.document.ui.inspector_open {
            let mut open = true;
            egui::Window::new("DetailMode")
                .open(&mut open)
                .default_size([420.0, 500.0])
                .default_pos(egui::pos2(
                    (ctx.content_rect().center().x - 210.0).max(104.0),
                    ctx.content_rect().top() + 80.0,
                ))
                .max_height((ctx.content_rect().height() - 200.0).max(180.0))
                .show(ctx, |ui| {
                    if ui.button("Dock inspector").clicked() {
                        self.document.ui.inspector_detached = false;
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(self.document.ui.nav != shell::Nav::Overview, |ui| {
                            ui.selectable_value(
                                &mut self.document.ui.inspector,
                                shell::DetailMode::Cell,
                                "Cell",
                            );
                            ui.selectable_value(
                                &mut self.document.ui.inspector,
                                shell::DetailMode::Record,
                                "Record",
                            );
                        });
                        ui.selectable_value(
                            &mut self.document.ui.inspector,
                            shell::DetailMode::Structure,
                            "Structure",
                        );
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| self.inspector_content(ui));
                });
            self.document.ui.inspector_open = open;
        }
        if self.tools.help {
            let mut open = true;
            egui::Window::new("Keyboard and safety reference").open(&mut open).default_width(560.0).show(ctx,|ui|{
                egui::ScrollArea::vertical().max_height(500.0).show(ui,|ui|{
                    for line in ["Rail: Tables / Map / History / Saved / Checks; Settings opens in the workspace","Ctrl+K: tables / columns / commands · Ctrl+J: SQL view · Ctrl+1 / Ctrl+2: Data / Structure","Ctrl+O: database chooser · Folder icon / Alt+Enter: file dialog · F1: help · Ctrl+T: theme","Database chooser: Down enters results; Up/Down browses; Enter selects; typing resumes search","In SQL editor: Ctrl+Enter / F5 runs one statement; Ctrl+Shift+Enter runs selection/current statement; Ctrl+Space completes","Ctrl+Shift+C: retained TSV · Ctrl+E: export · Alt+Left: Back / return to table","Grid: arrows move, Shift+arrows extend, Space opens record detail, Ctrl+C copies selection","Double-click / Enter: inline stage · Escape: cancel · right-click: typed detail / filter / copy","Headers: drag to move, edge drag to resize, double-click edge to fit; funnel shows sampled values","Ctrl+Alt+Enter: expand/restore view · Ctrl+Tab: next view · right navigation remains visible","Scroll / PgDn: load bounded pages; Ctrl+F: typed tokens. Map camera is saved on close.","Exports/copies never rerun SQL and exclude staged overlays","Discard suppresses delivery; execution and writes continue until the worker reports completion","Closing waits for actual execution and rollback acknowledgement","Writable SQL autocommits unless you BEGIN; staged edits require review and explicit commit"]{ui.label(line);}
                });
            });
            self.tools.help = open;
        }
    }
}

fn kind_name(value: &Value) -> &'static str {
    match value {
        Value::Null => "NULL",
        Value::Integer(_) => "INTEGER",
        Value::Real(_) => "REAL",
        Value::Text(_) => "TEXT",
        Value::Blob(_) => "BLOB",
    }
}
