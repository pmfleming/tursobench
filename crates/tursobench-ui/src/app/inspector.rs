use super::presentation::{self, Intent};
use super::{
    App, View,
    icons::{self, Icon},
    results, shell,
};
use eframe::egui;
use tursobench_core::Value;

use results::{CellKind, code_view, detail_text};
enum RecordAction {
    Inspect(usize),
    Edit(usize),
    Stage(usize, Value),
    Revert(usize),
}
struct RecordField<'a> {
    column: usize,
    name: &'a str,
    stored: &'a Value,
    shown: &'a Value,
    foreign: bool,
    selected: bool,
}
impl RecordField<'_> {
    fn paint(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        name_width: f32,
        icon: Option<Icon>,
    ) -> egui::Rect {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if self.selected {
            ui.painter()
                .rect_filled(rect, 0.0, ui.visuals().selection.bg_fill);
        }
        if self.shown != self.stored {
            ui.painter()
                .rect_filled(rect, 0.0, tokens.pending.gamma_multiply(0.08));
            ui.painter().vline(
                rect.left() + 1.5,
                rect.y_range(),
                egui::Stroke::new(3.0, tokens.pending),
            );
        }
        ui.painter().hline(
            rect.x_range(),
            rect.bottom(),
            egui::Stroke::new(1.0, tokens.line_soft),
        );
        if self.selected {
            ui.painter().rect_stroke(
                rect,
                0.0,
                ui.visuals().selection.stroke,
                egui::StrokeKind::Inside,
            );
        }
        let label_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 24.0));
        if let Some(icon) = icon {
            icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    label_rect.left_center() + egui::vec2(10.0, 0.0),
                    egui::vec2(12.0, 12.0),
                ),
                icon,
                tokens.relationship,
            );
        }
        let name = egui::Rect::from_min_max(
            rect.min + egui::vec2(20.0, 0.0),
            egui::pos2(rect.left() + name_width, label_rect.bottom()),
        );
        presentation::text_at(ui, name, self.name, tokens.muted, 12.5);
        egui::Rect::from_min_max(
            egui::pos2(name.right() + 8.0, rect.top()),
            rect.max - egui::vec2(4.0, 0.0),
        )
    }
    fn value(
        &self,
        ui: &egui::Ui,
        rect: egui::Rect,
        style: super::columns::CellStyle,
        wrapped: Option<std::sync::Arc<egui::Galley>>,
    ) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if let Some(galley) = wrapped {
            ui.painter()
                .with_clip_rect(rect.intersect(ui.clip_rect()))
                .galley(
                    rect.min + egui::vec2(0.0, 4.0),
                    galley,
                    if self.shown != self.stored {
                        tokens.pending
                    } else {
                        ui.visuals().text_color()
                    },
                );
            return;
        }
        let compact = match self.shown {
            Value::Blob(bytes) => Some((Icon::Bytecode, byte_size(bytes.len() as u64))),
            Value::Deferred(cell) => Some((Icon::Export, byte_size(cell.bytes))),
            _ => None,
        };
        if let Some((icon, caption)) = compact {
            icons::paint(
                ui.painter(),
                egui::Rect::from_center_size(
                    rect.left_center() + egui::vec2(7.0, 0.0),
                    egui::vec2(13.0, 13.0),
                ),
                icon,
                tokens.faint,
            );
            presentation::text_at(
                ui,
                egui::Rect::from_min_max(rect.min + egui::vec2(20.0, 0.0), rect.max),
                &caption,
                tokens.faint,
                12.5,
            );
            return;
        }
        if self.shown == self.stored && style.paint(ui, rect, self.name, self.shown) {
            return;
        }
        presentation::text_at(
            ui,
            rect,
            &self.shown.preview(),
            if self.shown != self.stored {
                tokens.pending
            } else if self.foreign {
                tokens.relationship
            } else if matches!(self.shown, Value::Null) {
                tokens.faint
            } else {
                ui.visuals().text_color()
            },
            12.5,
        );
    }
}
fn byte_size(bytes: u64) -> String {
    if bytes < 1024 {
        return format!("{bytes} bytes");
    }
    let (unit, divisor) = if bytes >= 1024 * 1024 * 1024 {
        ("GB", 1024.0 * 1024.0 * 1024.0)
    } else if bytes >= 1024 * 1024 {
        ("MB", 1024.0 * 1024.0)
    } else {
        ("KB", 1024.0)
    };
    format!("{:.1} {unit}", bytes as f64 / divisor)
}
fn record_tooltip(value: &Value) -> String {
    match value {
        Value::Text(text) => {
            let mut chars = text.chars();
            let mut preview: String = chars.by_ref().take(4096).collect();
            if chars.next().is_some() {
                preview.push_str("\n… Open cell detail for the full value");
                preview
            } else {
                detail_text(value)
            }
        }
        Value::Blob(_) | Value::Deferred(_) => value.preview(),
        _ => detail_text(value),
    }
}
fn record_wrap(
    ui: &egui::Ui,
    value: &Value,
    width: f32,
    selected: bool,
    pending: bool,
) -> Option<std::sync::Arc<egui::Galley>> {
    let Value::Text(text) = value else {
        return None;
    };
    if !selected || text.chars().take(33).count() <= 32 && !text.contains('\n') {
        return None;
    }
    // Retained text can be megabytes long; the row only lays out a bounded preview.
    let text: String = text.chars().take(4096).collect();
    let mut job = egui::text::LayoutJob::simple(
        text,
        crate::appearance::proportional(ui.ctx(), 12.5),
        if pending {
            crate::appearance::Tokens::of(ui.ctx()).pending
        } else {
            ui.visuals().text_color()
        },
        width.max(16.0),
    );
    job.wrap.max_rows = 4;
    job.wrap.break_anywhere = true;
    Some(ui.painter().layout_job(job))
}

impl App {
    pub(super) fn cell_inspector(&mut self, ui: &mut egui::Ui, row: usize, col: usize) {
        let Some(value) = self
            .database
            .document
            .result
            .rows
            .get(row)
            .and_then(|r| r.get(col))
        else {
            ui.label("Selection no longer belongs to the retained result");
            return;
        };
        ui.strong(
            self.database
                .document
                .result
                .columns
                .get(col)
                .map_or("Cell", |c| c.name.as_str()),
        );
        if self.database.document.view == View::Browse
            && let Some(table) = &self.database.document.table
        {
            ui.label(format!("Table: {}", table.name));
        }
        ui.small(format!(
            "Retained row {} · {} budgeted bytes · {:?} result",
            row + 1,
            value.bytes(),
            self.database.document.view
        ));
        let staged = self.database.document.drafts.as_ref().and_then(|d| {
            self.database
                .document
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
                self.database.document.tools.detail = Some(row);
                self.database.document.ui.inspector = shell::DetailMode::Record;
            }
        });
        if self.can_edit() {
            ui.separator();
            ui.strong("Change value · staged locally");
            self.database
                .document
                .tools
                .cell_editor
                .type_picker(ui, "cell-kind", true);
            if self.database.document.tools.cell_editor.kind != CellKind::Null {
                ui.add(
                    egui::TextEdit::multiline(&mut self.database.document.tools.cell_editor.input)
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
                match self.database.document.tools.cell_editor.parse() {
                    Ok(value) => self.stage_cell(row, col, value),
                    Err(e) => self.database.status = e,
                }
            }
            ui.horizontal_wrapped(|ui| {
                if icons::button(ui, Icon::Undo, "Revert cell draft", "").clicked() {
                    self.revert_grid_draft(row, Some(col));
                }
                if ui.button("Revert row draft").clicked() {
                    self.revert_grid_draft(row, None);
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
        if self.database.document.view == View::Browse
            && let Some(info) = &self.database.document.tools.inspection
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
        let wide = ui.available_width() >= 600.0;
        self.record_inspector_with_types(ui, row, wide);
    }
    pub(super) fn record_inspector_with_types(
        &mut self,
        ui: &mut egui::Ui,
        row: usize,
        wide: bool,
    ) {
        self.record_keys(ui.ctx(), row);
        if !self.database.workspace.enabled {
            ui.horizontal(|ui| {
                ui.strong(format!("Record {}", row + 1));
                if icons::button(ui, Icon::Copy, "Copy row as annotated TSV", "").clicked() {
                    self.copy_row(ui.ctx(), row);
                }
            });
        }
        if let Some(action) = self.record_fields(ui, row, wide) {
            match action {
                RecordAction::Revert(col) => {
                    self.revert_grid_draft(row, Some(col));
                }
                RecordAction::Inspect(col) => self.select_cell(row, col, false),
                RecordAction::Edit(col) => {
                    self.open_cell(row, col);
                    self.start_inline(row, col);
                    if self.database.document.tools.inline.is_none() {
                        if let Some(Value::Deferred(cell)) = self
                            .database
                            .document
                            .result
                            .rows
                            .get(row)
                            .and_then(|values| values.get(col))
                        {
                            self.open_large_value((**cell).clone());
                        } else {
                            self.database.document.ui.inspector_detached = true;
                        }
                    }
                }
                RecordAction::Stage(col, value) => {
                    self.stage_cell(row, col, value);
                    self.database.document.tools.inline = None;
                }
            }
        }
        if !self.database.workspace.enabled {
            ui.separator();
            self.record_relationships(ui, row);
        }
    }
    fn record_keys(&mut self, ctx: &egui::Context, row: usize) {
        if self.database.workspace.enabled
            && self.database.workspace.active != super::tiling::Kind::Record
            || self.database.document.tools.inline.is_some()
            || ctx.egui_wants_keyboard_input()
            || egui::Popup::is_any_open(ctx)
            || self.database.ui.palette_open
        {
            return;
        }
        let column = self.database.document.inspect.map_or(0, |(_, col)| col);
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            self.start_inline(row, column);
            return;
        }
        for (key, delta) in [(egui::Key::ArrowUp, -1), (egui::Key::ArrowDown, 1)] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                let col = column.saturating_add_signed(delta).min(
                    self.database
                        .document
                        .result
                        .columns
                        .len()
                        .saturating_sub(1),
                );
                self.select_cell(row, col, false);
                self.database.document.tools.record_scroll_to = Some(col);
                break;
            }
        }
    }
    /// A parent link is usable only when every component has a resolved target and a retained non-NULL value.
    pub(super) fn record_parent_ready(
        &self,
        row: usize,
        key: &tursobench_core::ForeignKey,
    ) -> bool {
        !key.columns.is_empty()
            && key.columns.iter().all(|(source, target)| {
                target.is_some()
                    && self
                        .database
                        .document
                        .table
                        .as_ref()
                        .and_then(|table| {
                            table
                                .columns
                                .iter()
                                .position(|c| c.name.eq_ignore_ascii_case(source))
                        })
                        .and_then(|col| self.database.document.result.rows.get(row)?.get(col))
                        .is_some_and(|v| !matches!(v, Value::Null | Value::Deferred(_)))
            })
    }
    fn record_fields(&mut self, ui: &mut egui::Ui, row: usize, wide: bool) -> Option<RecordAction> {
        let values = self.database.document.result.rows.get(row)?;
        let scroll_to = self.database.document.tools.record_scroll_to.take();
        let name_width =
            (ui.available_width() * 0.32).clamp(48.0, if wide { 180.0 } else { 124.0 });
        let type_width = if wide { 64.0 } else { 0.0 };
        let mut action = None;
        let row_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        for (col, value) in values.iter().enumerate() {
            let header = self.database.document.column_header(col);
            let field = RecordField {
                column: col,
                name: &self.database.document.result.columns[col].name,
                stored: value,
                shown: self
                    .database
                    .document
                    .drafts
                    .as_ref()
                    .and_then(|d| d.displayed(values, col))
                    .unwrap_or(value),
                foreign: header.is_foreign(),
                selected: self.database.document.inspect == Some((row, col)),
            };
            let available_value = (ui.available_width() - name_width - type_width - 12.0).max(16.0);
            let staged = field.shown != field.stored;
            let revert_width = if staged { 24.0 } else { 0.0 };
            let editing = self
                .database
                .document
                .tools
                .inline
                .as_ref()
                .is_some_and(|e| e.row == row && e.column == col);
            let wrapped = record_wrap(
                ui,
                field.shown,
                available_value - revert_width - 4.0,
                field.selected && !editing,
                staged,
            );
            let height = wrapped
                .as_ref()
                .map_or(24.0, |g| (g.size().y + 8.0).max(24.0));
            let (rect, response) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), height),
                egui::Sense::click(),
            );
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::Role::Button,
                    ui.is_enabled(),
                    field.selected,
                    format!(
                        "{} · {} · {}",
                        field.name,
                        kind_name(field.shown),
                        field.shown.preview()
                    ),
                )
            });
            if response.clicked() {
                response.request_focus();
            }
            if scroll_to == Some(col) {
                response.scroll_to_me(Some(egui::Align::Center));
            }
            let mut value_rect = field.paint(ui, rect, name_width, header.key_icon());
            if wide {
                let type_rect = egui::Rect::from_min_max(
                    egui::pos2(rect.right() - 64.0, rect.top()),
                    egui::pos2(rect.right() - 4.0, rect.top() + 24.0),
                );
                super::presentation::text_right_at(
                    ui,
                    type_rect,
                    header.declared_type().unwrap_or("VALUE"),
                    crate::appearance::Tokens::of(ui.ctx()).faint,
                    11.0,
                );
                value_rect.max.x -= 64.0;
            }
            if staged {
                let button_rect = egui::Rect::from_min_max(
                    egui::pos2(value_rect.right() - 24.0, rect.top()),
                    egui::pos2(value_rect.right(), rect.top() + 24.0),
                );
                value_rect.max.x -= 24.0;
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt(("revert", col))
                        .max_rect(button_rect),
                    |ui| {
                        if ui
                            .add_enabled_ui(
                                self.database.commit_ticket.is_none()
                                    && !self.database.uncertain_edit,
                                |ui| {
                                    icons::button(
                                        ui,
                                        Icon::Undo,
                                        &format!("Revert {} draft", field.name),
                                        "",
                                    )
                                },
                            )
                            .inner
                            .clicked()
                        {
                            action = Some(RecordAction::Revert(col));
                        }
                    },
                );
            }
            if let Some(inline) = self.database.document.tools.inline.as_mut().filter(|e| {
                e.row == row
                    && e.column == col
                    && (!self.database.workspace.enabled
                        || self.database.workspace.active == super::tiling::Kind::Record)
            }) {
                ui.scope_builder(
                    egui::UiBuilder::new()
                        .id_salt(("record-edit", col))
                        .max_rect(value_rect)
                        .layout(egui::Layout::right_to_left(egui::Align::Center)),
                    |ui| {
                        ui.spacing_mut().interact_size = egui::vec2(40.0, 20.0);
                        ui.spacing_mut().combo_width = 62.0;
                        inline.editor.type_picker(ui, "type", false);
                        let response = ui.add_sized(
                            [ui.available_width(), 20.0],
                            egui::TextEdit::singleline(&mut inline.editor.input).hint_text(
                                if inline.editor.kind == CellKind::Null {
                                    "NULL"
                                } else {
                                    ""
                                },
                            ),
                        );
                        match inline.submitted(ui, &response) {
                            Some(Ok(value)) => action = Some(RecordAction::Stage(col, value)),
                            Some(Err(error)) => self.database.status = error,
                            None => {}
                        }
                    },
                );
            } else {
                let style = self
                    .database
                    .document
                    .tools
                    .columns
                    .visuals
                    .get(&col)
                    .copied()
                    .unwrap_or_default();
                field.value(ui, value_rect, style, wrapped);
                if response.double_clicked() {
                    action = Some(RecordAction::Edit(field.column));
                } else if response.clicked() {
                    action = Some(RecordAction::Inspect(field.column));
                }
                response.on_hover_ui(|ui| {
                    ui.label(format!("{} · {}", field.name, kind_name(field.shown)));
                    ui.label(record_tooltip(field.shown));
                });
            }
        }
        ui.spacing_mut().item_spacing.y = row_spacing;
        action
    }
    pub(super) fn inspection_surfaces(&mut self, ctx: &egui::Context) {
        if self.database.document.ui.inspector_detached && self.database.document.ui.inspector_open
        {
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
                        self.database.document.ui.inspector_detached = false;
                    }
                    ui.horizontal_wrapped(|ui| {
                        ui.add_enabled_ui(
                            self.database.document.ui.nav != shell::Nav::Overview,
                            |ui| {
                                ui.selectable_value(
                                    &mut self.database.document.ui.inspector,
                                    shell::DetailMode::Cell,
                                    "Cell",
                                );
                                ui.selectable_value(
                                    &mut self.database.document.ui.inspector,
                                    shell::DetailMode::Record,
                                    "Record",
                                );
                            },
                        );
                        ui.selectable_value(
                            &mut self.database.document.ui.inspector,
                            shell::DetailMode::Structure,
                            "Structure",
                        );
                    });
                    egui::ScrollArea::vertical().show(ui, |ui| self.inspector_content(ui));
                });
            self.database.document.ui.inspector_open = open;
        }
        if self.database.tools.help {
            let mut open = true;
            egui::Window::new("Keyboard and safety reference")
                .frame(crate::app::presentation::window_frame(ctx))
                .default_pos(ctx.content_rect().center() - egui::vec2(300.0, 260.0))
                .open(&mut open)
                .default_width(600.0)
                .default_height((ctx.content_rect().height() - 150.0).clamp(160.0, 560.0))
                .max_width((ctx.content_rect().width() - 40.0).max(240.0))
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical()
                        .max_height((ctx.content_rect().height() - 150.0).clamp(160.0, 560.0))
                        .auto_shrink([false, true])
                        .show(ui, help_reference);
                });
            self.database.tools.help = open;
        }
    }
}

/// Each row pairs keys or gestures with what they do; an empty key list is a note.
type HelpRow = (&'static [&'static str], &'static str);
const HELP: &[(&str, &[HelpRow])] = &[
    (
        "Anywhere",
        &[
            (&["F1"], "This reference"),
            (&["Ctrl K"], "Commands: find any action by name"),
            (
                &["Ctrl P", "/"],
                "Sidebar search across All open, This database or Databases · Up/Down chooses · Enter opens · Esc clears",
            ),
            (&["Ctrl O"], "Open a database file (also the folder icon)"),
            (
                &["Ctrl PgUp", "Ctrl PgDn"],
                "Cycle open databases · sidebar database rows switch and close",
            ),
            (&["Ctrl ,"], "Settings"),
            (&["Ctrl T"], "Toggle theme"),
            (&["F7"], "Compare two open schemas"),
            (&["Esc"], "Close the topmost dialog, menu or expanded tile"),
            (
                &[],
                "Tables / History / Saved / Checks sit under the selected database. Settings, commands and this reference sit at the foot of the sidebar. Hover any icon for its name.",
            ),
        ],
    ),
    (
        "Tiles",
        &[
            (&["Ctrl Tab"], "Focus the next tile"),
            (
                &["Ctrl Alt Enter"],
                "Expand the focused tile, or restore the workspace",
            ),
            (&["Ctrl J"], "SQL view"),
            (&["Ctrl 1", "Ctrl 2"], "Data / Structure"),
            (&["Alt Left"], "Back / return to table"),
            (&["F5"], "Refresh the focused view"),
            (
                &[],
                "Tiles have fixed positions. Drag a divider to resize; minimize to a header or expand to fill the workspace. Left navigation remains visible.",
            ),
        ],
    ),
    (
        "SQL editor",
        &[
            (&["Ctrl Enter", "F5"], "Run one statement"),
            (
                &["Ctrl Shift Enter"],
                "Run the selection or current statement",
            ),
            (&["Ctrl Space"], "Completions"),
            (&["Ctrl S"], "Save query"),
        ],
    ),
    (
        "Grid",
        &[
            (&["Arrows", "Shift Arrows"], "Move · extend the selection"),
            (&["Space"], "Record detail"),
            (
                &["Enter", "Double-click"],
                "Stage an inline edit, or follow a foreign key · Esc cancels",
            ),
            (&["Alt Click"], "Filter rows by the clicked value"),
            (&["Right-click"], "Typed detail / filter / copy"),
            (&["Ctrl F"], "Filter with typed tokens"),
            (&["Ctrl C"], "Copy the selection"),
            (&["Ctrl Shift C"], "Copy retained rows as TSV"),
            (&["Ctrl E"], "Export"),
            (&["PgDn", "PgUp", "Scroll"], "Load bounded pages"),
            (&["Ctrl V"], "Paste a rectangle"),
            (&["Ctrl Shift V"], "Mapped row paste"),
            (&["Backspace"], "Stage NULL in the selected cells"),
            (&["Ctrl Z"], "Undo one local edit"),
            (
                &[],
                "Table data → Edit cells: paste, mapped new rows, typed fill, NULL and undo.",
            ),
        ],
    ),
    (
        "Column headers",
        &[
            (&["Click"], "Sort · again to reverse · once more to clear"),
            (&["Shift Click"], "Add a sort level"),
            (&["Drag"], "Move the column"),
            (&["Drag edge"], "Resize · double-click the edge to fit"),
            (&["Funnel"], "Filter by sampled values"),
            (&["Right-click"], "Cell appearance, fit, move and hide"),
        ],
    ),
    (
        "Safety",
        &[
            (
                &[],
                "Exports/copies never rerun SQL and exclude staged overlays.",
            ),
            (
                &[],
                "Discard suppresses delivery; execution and writes continue until the worker reports completion.",
            ),
            (
                &[],
                "Writable SQL autocommits unless you BEGIN; staged edits require explicit commit; Diff / SQL review is optional.",
            ),
            (
                &[],
                "Close waits for rollback acknowledgement. Map camera is saved on close.",
            ),
        ],
    ),
];

fn help_reference(ui: &mut egui::Ui) {
    const KEYS: f32 = 160.0;
    let tokens = crate::appearance::Tokens::of(ui.ctx());
    for (index, (title, rows)) in HELP.iter().enumerate() {
        if index > 0 {
            ui.add_space(6.0);
        }
        ui.label(
            egui::RichText::new(title.to_uppercase())
                .size(crate::appearance::text_size(ui.ctx(), 10.5))
                .strong()
                .color(tokens.muted),
        );
        for (keys, description) in *rows {
            ui.horizontal_top(|ui| {
                let narrow = ui.available_width() < KEYS + 160.0;
                if !keys.is_empty() {
                    ui.allocate_ui_with_layout(
                        egui::vec2(KEYS, 0.0),
                        egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
                        |ui| {
                            if !narrow {
                                ui.set_min_width(KEYS);
                            }
                            ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                            for key in *keys {
                                crate::app::presentation::keycap(ui, key);
                            }
                        },
                    );
                } else if !narrow {
                    ui.add_space(KEYS + ui.spacing().item_spacing.x);
                }
                ui.add(egui::Label::new(*description).wrap());
            });
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
        Value::Deferred(_) => "DEFERRED",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn long_selected_text_is_bounded_and_byte_captions_keep_sizes() {
        assert_eq!(byte_size(3), "3 bytes");
        assert_eq!(byte_size(4_404_019), "4.2 MB");
        let large = Value::Text("λ".repeat(100_000));
        let tooltip = record_tooltip(&large);
        assert!(tooltip.starts_with(&"λ".repeat(4096)));
        assert!(tooltip.ends_with("full value"));
        assert!(tooltip.len() < 8300);
        assert_eq!(
            record_tooltip(&Value::Blob(vec![0; 100_000])),
            "[blob: 100000 bytes]"
        );
        let json = Value::Text(r#"{"n":123456789012345678901,"n":1}"#.into());
        assert_eq!(record_tooltip(&json), detail_text(&json));
        let ctx = egui::Context::default();
        crate::app::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
            let value = Value::Text("long λ text\n".repeat(100_000));
            let galley = record_wrap(ui, &value, 200.0, true, false).unwrap();
            assert!(galley.rows.len() <= 4);
            assert!(galley.job.text.len() < 8192);
            assert!(record_wrap(ui, &value, 200.0, false, false).is_none());
        });
    }
}
