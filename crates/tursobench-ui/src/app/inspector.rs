use super::presentation::{self, Intent};
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
        presentation::notice(ui, Icon::Selection, Intent::Neutral, "Stored value");
        let text = typed(value);
        egui::ScrollArea::vertical()
            .id_salt("stored-cell")
            .max_height(180.0)
            .show(ui, |ui| {
                code_view(ui, &text, 3);
            });
        ui.horizontal_wrapped(|ui| {
            if ui
                .add(presentation::action(
                    ui,
                    "Copy stored value",
                    Intent::Primary,
                ))
                .clicked()
            {
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
            self.document
                .tools
                .cell_editor
                .type_picker(ui, "cell-kind", true);
            if self.document.tools.cell_editor.kind != CellKind::Null {
                ui.add(
                    egui::TextEdit::multiline(&mut self.document.tools.cell_editor.input)
                        .desired_width(ui.available_width())
                        .desired_rows(3),
                );
            }
            if presentation::footer(ui, |ui| {
                ui.add(presentation::action(
                    ui,
                    "Stage cell change",
                    Intent::Pending,
                ))
            })
            .inner
            .clicked()
            {
                match self.document.tools.cell_editor.parse() {
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
                    .button(
                        egui::RichText::new(format!(
                            "Parent: {} ({})",
                            key.target,
                            key.columns
                                .iter()
                                .map(|(c, _)| c.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ))
                        .color(crate::appearance::Tokens::of(ui.ctx()).relationship),
                    )
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
        if !self.workspace.enabled {
            ui.horizontal(|ui| {
                ui.strong(format!("Record {}", row + 1));
                if icons::button(ui, Icon::Copy, "Copy row as annotated TSV", "").clicked() {
                    self.copy_row(ui.ctx(), row);
                }
            });
        }
        let mut follow = None;
        if let Some(info) = self.document.browse_inspection() {
            for key in &info.foreign_keys {
                if ui
                    .add_enabled(
                        !self.has_drafts() && self.document.active.is_none(),
                        egui::Button::new(
                            egui::RichText::new(format!("Parent: {}", key.target))
                                .color(crate::appearance::Tokens::of(ui.ctx()).relationship),
                        )
                        .fill(
                            crate::appearance::Tokens::of(ui.ctx())
                                .relationship
                                .gamma_multiply(0.09),
                        ),
                    )
                    .clicked()
                {
                    follow = Some(key.clone());
                }
            }
        }
        if let Some(key) = follow {
            self.follow_key(row, key);
            return;
        }
        let mut inspect = None;
        let mut edit = None;
        let scroll_to = self.document.tools.record_scroll_to.take();
        if let Some(values) = self.document.result.rows.get(row) {
            let tokens = crate::appearance::Tokens::of(ui.ctx());
            let name_width = (ui.available_width() * 0.32).clamp(48.0, 124.0);
            let value_width = (ui.available_width() - name_width - 8.0).max(24.0);
            egui::Grid::new("compact-record")
                .num_columns(2)
                .spacing([8.0, 0.0])
                .show(ui, |ui| {
                    for (col, value) in values.iter().enumerate() {
                        let name = &self.document.result.columns[col].name;
                        let key = self.document.tools.inspection.as_ref().is_some_and(|i| {
                            i.table.columns.get(col).is_some_and(|c| c.primary_key > 0)
                        });
                        let foreign = self.document.tools.inspection.as_ref().is_some_and(|i| {
                            i.foreign_keys
                                .iter()
                                .any(|k| k.columns.iter().any(|(from, _)| from == name))
                        });
                        let label = ui
                            .allocate_ui_with_layout(
                                egui::vec2(name_width, 31.0),
                                egui::Layout::left_to_right(egui::Align::Center),
                                |ui| {
                                    if key || foreign {
                                        let (r, _) = ui.allocate_exact_size(
                                            egui::vec2(12.0, 12.0),
                                            egui::Sense::hover(),
                                        );
                                        icons::paint(
                                            ui.painter(),
                                            r,
                                            if key { Icon::Key } else { Icon::Link },
                                            tokens.relationship,
                                        );
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(name)
                                                .size(crate::appearance::text_size(ui.ctx(), 12.5))
                                                .color(tokens.muted),
                                        )
                                        .truncate(),
                                    );
                                },
                            )
                            .response;
                        if scroll_to == Some(col) {
                            label.scroll_to_me(Some(egui::Align::Center));
                        }
                        let shown = self
                            .document
                            .drafts
                            .as_ref()
                            .and_then(|d| d.displayed(values, col))
                            .unwrap_or(value);
                        if let Some(inline) = self.document.tools.inline.as_mut().filter(|e| {
                            e.row == row
                                && e.column == col
                                && (!self.workspace.enabled
                                    || self.workspace.active == super::tiling::Kind::Record)
                        }) {
                            let response = ui.add_sized(
                                [value_width, 31.0],
                                egui::TextEdit::singleline(&mut inline.editor.input),
                            );
                            match inline.submitted(ui, &response) {
                                Some(Ok(value)) => edit = Some((col, value)),
                                Some(Err(error)) => self.status = error,
                                None => {}
                            }
                        } else {
                            let response = ui
                                .add_sized(
                                    [value_width, 31.0],
                                    egui::Button::new(
                                        egui::RichText::new(shown.preview())
                                            .monospace()
                                            .size(crate::appearance::text_size(ui.ctx(), 12.5))
                                            .color(if shown != value {
                                                tokens.pending
                                            } else if foreign {
                                                tokens.relationship
                                            } else if matches!(shown, Value::Null) {
                                                tokens.faint
                                            } else {
                                                ui.visuals().text_color()
                                            }),
                                    )
                                    .selected(self.document.inspect == Some((row, col)))
                                    .frame_when_inactive(false)
                                    .truncate(),
                                )
                                .on_hover_text(format!(
                                    "{} · {}\n{}",
                                    name,
                                    kind_name(shown),
                                    detail_text(shown)
                                ));
                            if shown != value {
                                ui.painter().rect_filled(
                                    response.rect,
                                    0.0,
                                    tokens.pending.gamma_multiply(0.08),
                                );
                                ui.painter().vline(
                                    response.rect.left() + 1.5,
                                    response.rect.y_range(),
                                    egui::Stroke::new(3.0, tokens.pending),
                                );
                            }
                            ui.painter().hline(
                                response.rect.x_range(),
                                response.rect.bottom(),
                                egui::Stroke::new(1.0, tokens.line_soft),
                            );
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
            egui::Window::new("Cell detail")
                .frame(crate::app::presentation::window_frame(ctx))
                .default_pos(ctx.content_rect().center() - egui::vec2(280.0, 180.0))
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
            egui::Window::new("Keyboard and safety reference").frame(crate::app::presentation::window_frame(ctx)).default_pos(ctx.content_rect().center()-egui::vec2(280.0,180.0)).open(&mut open).default_width(560.0).show(ctx,|ui|{
                egui::ScrollArea::vertical().max_height(500.0).show(ui,|ui|{
                    for line in ["Navigator footer: Tables / History / Saved / Checks / Settings · F1: Help","Ctrl+K: tables / columns / commands · Ctrl+J: SQL view · Ctrl+1 / Ctrl+2: Data / Structure","Ctrl+O: database chooser · Folder icon / Alt+Enter: file dialog · F1: help · Ctrl+T: theme","Database chooser: Down enters results; Up/Down browses; Enter selects; typing resumes search","In SQL editor: Ctrl+Enter / F5 runs one statement; Ctrl+Shift+Enter runs selection/current statement; Ctrl+Space completes","Ctrl+Shift+C: retained TSV · Ctrl+E: export · Alt+Left: Back / return to table","Grid: arrows move, Shift+arrows extend, Space opens record detail, Ctrl+C copies selection","Double-click / Enter: inline stage · Escape: cancel · right-click: typed detail / filter / copy","Headers: drag to move, edge drag to resize, double-click edge to fit; funnel shows sampled values","Tiles have fixed positions. Minimize to a header or expand to fill the workspace.","Ctrl+Alt+Enter: expand/restore view · Ctrl+Tab: next view · left navigation remains visible","Scroll / PgDn: load bounded pages; Ctrl+F: typed tokens. Map camera is saved on close.","Exports/copies never rerun SQL and exclude staged overlays","Discard suppresses delivery; execution and writes continue until the worker reports completion","Closing waits for actual execution and rollback acknowledgement","Writable SQL autocommits unless you BEGIN; staged edits require explicit commit; Diff / SQL review is optional"]{ui.label(line);}
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
