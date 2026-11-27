use super::presentation::{self, Intent};
use super::{
    App, View,
    icons::{self, Icon},
    results, shell,
};
use eframe::egui;
use tursobench_core::Value;

use super::value::{Lens, Preview, Summary};
use results::{code_view, detail_text};
enum RecordAction {
    Inspect(usize),
    Edit(usize),
    Stage(usize, Value),
    Revert(usize),
    /// Read the column's blobs through this lens from now on.
    Lens(usize, Lens),
    /// Open the field in the value view.
    Open(usize),
    /// Load the field's large value whole for its preview.
    Load(usize),
}
/// The widths every field of a record shares: its name and, when wide, its declared type.
#[derive(Clone, Copy)]
struct FieldLayout {
    name_width: f32,
    wide: bool,
}
struct RecordField<'a> {
    column: usize,
    name: &'a str,
    stored: &'a Value,
    shown: &'a Value,
    foreign: bool,
    selected: bool,
    /// The column's name or type says it holds vectors.
    vector: bool,
}
impl RecordField<'_> {
    fn staged(&self) -> bool {
        self.shown != self.stored
    }
    fn describe(&self, ui: &egui::Ui, response: &egui::Response) {
        let kind = kind_name(self.shown);
        let shown = Summary::of(ui.ctx(), self.shown)
            .map_or_else(|| self.shown.preview(), |summary| summary.text());
        let text = format!("{} · {kind} · {shown}", self.name);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, ui.is_enabled(), self.selected, &text)
        });
    }
    /// A double-click edits the field and a click inspects it.
    fn action(&self, response: &egui::Response) -> Option<RecordAction> {
        if response.double_clicked() {
            Some(RecordAction::Edit(self.column))
        } else {
            response
                .clicked()
                .then_some(RecordAction::Inspect(self.column))
        }
    }
    fn paint(
        &self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        name_width: f32,
        icon: Option<Icon>,
    ) -> egui::Rect {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        let (painter, selection) = (ui.painter(), &ui.visuals().selection);
        if self.selected {
            painter.rect_filled(rect, 0.0, selection.bg_fill);
        }
        if self.staged() {
            painter.rect_filled(rect, 0.0, tokens.pending.gamma_multiply(0.08));
            let bar = egui::Stroke::new(3.0, tokens.pending);
            painter.vline(rect.left() + 1.5, rect.y_range(), bar);
        }
        let line = egui::Stroke::new(1.0, tokens.line_soft);
        painter.hline(rect.x_range(), rect.bottom(), line);
        if self.selected {
            painter.rect_stroke(rect, 0.0, selection.stroke, egui::StrokeKind::Inside);
        }
        let label_rect = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 24.0));
        if let Some(icon) = icon {
            let at = label_rect.left_center() + egui::vec2(10.0, 0.0);
            presentation::icon_at(painter, at, 12.0, icon, tokens.relationship);
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
        thumb: Option<(egui::TextureId, egui::Vec2)>,
    ) {
        let tokens = crate::appearance::Tokens::of(ui.ctx());
        if let Some(galley) = wrapped {
            let color = if self.staged() {
                tokens.pending
            } else {
                ui.visuals().text_color()
            };
            let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
            painter.galley(rect.min + egui::vec2(0.0, 4.0), galley, color);
            return;
        }
        if let Some(summary) = Summary::for_column(ui.ctx(), self.shown, self.vector) {
            let color = if self.staged() {
                tokens.pending
            } else {
                ui.visuals().text_color()
            };
            let line = egui::Rect::from_min_size(rect.min, egui::vec2(rect.width(), 24.0));
            summary.paint(ui, line, color, false, thumb);
            return;
        }
        if !self.staged() && style.paint(ui, rect, self.name, self.shown) {
            return;
        }
        presentation::text_at(
            ui,
            rect,
            &self.shown.preview(),
            if self.staged() {
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
fn record_tooltip(ctx: &egui::Context, value: &Value) -> String {
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
        Value::Blob(_) | Value::Deferred(_) => Summary::of(ctx, value)
            .map(|summary| summary.description.clone())
            .unwrap_or_default(),
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

/// At the right of a previewed field's first line: the lenses it could use,
/// as selectable icons, then the icon that opens it. Returns what was clicked.
fn preview_tools(
    ui: &mut egui::Ui,
    row: egui::Rect,
    value: &mut egui::Rect,
    col: usize,
    preview: &Preview,
) -> Option<RecordAction> {
    let slot = |value: &mut egui::Rect, gap: f32| {
        value.max.x -= gap;
        let rect = egui::Rect::from_min_max(
            egui::pos2(value.right() - 24.0, row.top()),
            egui::pos2(value.right(), row.top() + 24.0),
        );
        value.max.x -= 24.0;
        rect
    };
    let mut child = line_child(ui, slot(value, 0.0), ("open", col));
    let open = icons::line_control(&mut child, Icon::Fit, "Open value", "Enter", None);
    let mut clicked = open.clicked().then_some(RecordAction::Open(col));
    if preview.offered.len() < 2 {
        return clicked;
    }
    for (i, &lens) in preview.offered.iter().rev().enumerate() {
        let rect = slot(value, if i == 0 { 6.0 } else { 0.0 });
        let selected = lens == preview.lens;
        let mut child = line_child(ui, rect, ("lens", col, lens.name()));
        let response =
            icons::line_control(&mut child, lens.icon(), lens.name(), "", Some(selected));
        if response.clicked() {
            clicked = Some(RecordAction::Lens(col, lens));
        }
    }
    clicked
}
/// A record field's declared type, at the right of its first line.
fn type_label(ui: &egui::Ui, row: egui::Rect, declared: &str) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(row.right() - 64.0, row.top()),
        egui::pos2(row.right() - 4.0, row.top() + 24.0),
    );
    let color = crate::appearance::Tokens::of(ui.ctx()).faint;
    super::presentation::text_right_at(ui, rect, declared, color, 11.0);
}
/// The button at the right of a staged field's value that reverts its draft.
/// Returns whether it was clicked.
fn revert_button(
    ui: &mut egui::Ui,
    row: egui::Rect,
    value: egui::Rect,
    col: usize,
    name: &str,
    enabled: bool,
) -> bool {
    let rect = egui::Rect::from_min_max(
        egui::pos2(value.right() - 24.0, row.top()),
        egui::pos2(value.right(), row.top() + 24.0),
    );
    let label = format!("Revert {name} draft");
    let mut child = line_child(ui, rect, ("revert", col));
    child
        .add_enabled_ui(enabled, |ui| icons::button(ui, Icon::Undo, &label, ""))
        .inner
        .clicked()
}
/// A child for a control on a field's first line. It allocates nothing in
/// the field list, so a field that grows below the line keeps its height.
fn line_child(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    salt: impl std::hash::Hash + std::fmt::Debug,
) -> egui::Ui {
    ui.new_child(egui::UiBuilder::new().id_salt(salt).max_rect(rect))
}
/// A value as typed JSON; a non-finite real, which JSON cannot hold, says so.
fn typed_text(value: &Value) -> String {
    match value {
        Value::Real(n) if !n.is_finite() => {
            format!("Real({n}) — inspectable/exportable, not editable as a non-finite real")
        }
        _ => serde_json::to_string_pretty(value).unwrap_or_default(),
    }
}
impl App {
    pub(super) fn cell_inspector(&mut self, ui: &mut egui::Ui, row: usize, col: usize) {
        let document = &self.database.document;
        let values = document.result.rows.get(row);
        let Some(value) = values.and_then(|r| r.get(col)) else {
            ui.label("Selection no longer belongs to the retained result");
            return;
        };
        let columns = &document.result.columns;
        ui.strong(columns.get(col).map_or("Cell", |c| c.name.as_str()));
        if document.view == View::Browse
            && let Some(table) = &document.table
        {
            ui.label(format!("Table: {}", table.name));
        }
        ui.small(format!(
            "Retained row {} · {} budgeted bytes · {:?} result",
            row + 1,
            value.bytes(),
            document.view
        ));
        let drafts = document.drafts.as_ref();
        let staged = drafts.zip(values).and_then(|(d, r)| d.displayed(r, col));
        if let Some(value) = staged {
            ui.colored_label(ui.visuals().warn_fg_color, "Staged value — not yet written");
            code_view(ui, &typed_text(value), 3);
        }
        presentation::notice(ui, Icon::Selection, Intent::Neutral, "Stored value");
        let text = typed_text(value);
        egui::ScrollArea::vertical()
            .id_salt("stored-cell")
            .max_height(180.0)
            .show(ui, |ui| {
                code_view(ui, &text, 3);
            });
        let (copy, record) = ui
            .horizontal_wrapped(|ui| {
                let copy = presentation::action(ui, "Copy stored value", Intent::Primary);
                (ui.add(copy).clicked(), ui.button("Record").clicked())
            })
            .inner;
        if copy {
            self.copy_cell(ui.ctx(), row, col);
        }
        if record {
            self.database.document.tools.detail = Some(row);
            self.database.document.ui.inspector = shell::DetailMode::Record;
        }
        if self.can_edit() {
            self.cell_editing(ui, row, col);
        } else {
            ui.small("Editing requires writable access, editor autocommit, a complete controlled non-engine-managed table result with a proven key, and no pending/uncertain commit. Arbitrary SQL results remain read-only.");
        }
        if self.database.document.view == View::Browse {
            self.parent_links(ui, row);
        }
    }
    /// Stage a typed value for the cell, or revert, delete or clone its row.
    fn cell_editing(&mut self, ui: &mut egui::Ui, row: usize, col: usize) {
        ui.separator();
        ui.strong("Change value · staged locally");
        let editor = &mut self.database.document.tools.cell_editor;
        editor.form(ui, "cell-kind");
        let stage = presentation::footer(ui, |ui| {
            ui.add(presentation::action(
                ui,
                "Stage cell change",
                Intent::Pending,
            ))
        });
        if stage.inner.clicked() {
            match editor.parse() {
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
            if icons::control(ui, Icon::Trash, "Stage row deletion", "", false, true).clicked() {
                self.stage_delete(row);
            }
            if ui.button("Clone as new row…").clicked() {
                self.clone_row(row);
            }
        });
    }
    /// The browsed table's foreign keys, each followed to the row's parent on a click.
    fn parent_links(&mut self, ui: &mut egui::Ui, row: usize) {
        if let Some(info) = &self.database.document.tools.inspection {
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
        match self.record_fields(ui, row, wide) {
            Some(RecordAction::Revert(col)) => self.revert_grid_draft(row, Some(col)),
            Some(RecordAction::Inspect(col)) => self.select_cell(row, col, false),
            Some(RecordAction::Edit(col)) => self.edit_record_field(row, col),
            Some(RecordAction::Stage(col, value)) => {
                self.stage_cell(row, col, value);
                self.database.document.tools.inline = None;
            }
            Some(RecordAction::Lens(col, lens)) => {
                self.database
                    .document
                    .tools
                    .columns
                    .lenses
                    .insert(col, lens);
            }
            Some(RecordAction::Open(col)) => self.open_value(row, col),
            Some(RecordAction::Load(col)) => self.load_field(row, col),
            None => {}
        }
        if !self.database.workspace.enabled {
            ui.separator();
            self.record_relationships(ui, row);
        }
    }
    /// A double-clicked field is edited in place; a blob or deferred value
    /// opens in the value view, and any other in the detached cell editor.
    fn edit_record_field(&mut self, row: usize, col: usize) {
        // The value shown decides: a blob staged as NULL edits as NULL.
        if let Some(Value::Blob(_) | Value::Deferred(_)) = self.field_value(row, col) {
            self.open_value(row, col);
            return;
        }
        self.open_cell(row, col);
        self.start_inline(row, col);
        if self.database.document.tools.inline.is_none() {
            self.database.document.ui.inspector_detached = true;
        }
    }
    /// Whether the record tile is the workspace's active tile.
    pub(super) fn record_tile_active(&self) -> bool {
        self.database.workspace.active == super::tiling::Kind::Record
    }
    fn record_keys(&mut self, ctx: &egui::Context, row: usize) {
        if self.database.workspace.enabled && !self.record_tile_active()
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
        let columns = self.database.document.result.columns.len();
        for (key, delta) in [(egui::Key::ArrowUp, -1), (egui::Key::ArrowDown, 1)] {
            if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key)) {
                let col = column
                    .saturating_add_signed(delta)
                    .min(columns.saturating_sub(1));
                self.select_cell(row, col, false);
                self.database.document.tools.record_scroll_to = Some(col);
                break;
            }
        }
    }
    fn record_fields(&mut self, ui: &mut egui::Ui, row: usize, wide: bool) -> Option<RecordAction> {
        let columns = self.database.document.result.rows.get(row)?.len();
        let scroll_to = self.database.document.tools.record_scroll_to.take();
        let layout = FieldLayout {
            name_width: (ui.available_width() * 0.32).clamp(48.0, if wide { 180.0 } else { 124.0 }),
            wide,
        };
        let document = &mut self.database.document;
        let (result, drafts) = (document.result.current, document.draft_revision);
        document.tools.values.images.keep_row(result, drafts, row);
        // The selected field's loaded value is kept; any other is dropped.
        let held = document
            .inspect
            .filter(|(r, _)| *r == row)
            .and_then(|(r, c)| {
                document
                    .result
                    .displayed_value(document.drafts.as_ref(), r, c)
            })
            .and_then(super::value::loadable);
        document.tools.values.keep_held(held);
        let row_spacing = ui.spacing().item_spacing.y;
        ui.spacing_mut().item_spacing.y = 0.0;
        let mut action = None;
        for col in 0..columns {
            action = self
                .record_field(ui, row, col, layout, scroll_to == Some(col))
                .or(action);
        }
        ui.spacing_mut().item_spacing.y = row_spacing;
        action
    }
    /// One field: its name, key icon and type, then its value — wrapped when
    /// selected — or its inline editor, with a revert button while staged.
    fn record_field(
        &mut self,
        ui: &mut egui::Ui,
        row: usize,
        col: usize,
        layout: FieldLayout,
        scroll: bool,
    ) -> Option<RecordAction> {
        let preview = self.field_preview(ui.ctx(), row, col);
        let thumb = self.record_thumbnail(ui.ctx(), row, col);
        // Field paths keep these borrows apart from the inline editor used below.
        let values = &self.database.document.result.rows[row];
        let header = self.database.document.column_header(col);
        let drafts = self.database.document.drafts.as_ref();
        let field = RecordField {
            column: col,
            name: &self.database.document.result.columns[col].name,
            stored: &values[col],
            shown: drafts
                .and_then(|d| d.displayed(values, col))
                .unwrap_or(&values[col]),
            foreign: header.is_foreign(),
            vector: super::value::vector_hint(&header.column.name, header.declared_type()),
            selected: self.database.document.inspect == Some((row, col)),
        };
        let editing = self.editing_at(row, col);
        let staged = field.staged();
        let type_width = if layout.wide { 64.0 } else { 0.0 };
        let value_width = (ui.available_width() - layout.name_width - type_width - 12.0).max(16.0)
            - if staged { 24.0 } else { 0.0 };
        let wrapped = record_wrap(
            ui,
            field.shown,
            value_width - 4.0,
            field.selected && !editing,
            staged,
        );
        let height = match (&preview, &wrapped) {
            (Some(_), _) => 24.0 + super::value::PREVIEW_HEIGHT + 6.0,
            (None, Some(galley)) => (galley.size().y + 8.0).max(24.0),
            (None, None) => 24.0,
        };
        let (rect, response) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), height),
            egui::Sense::click(),
        );
        field.describe(ui, &response);
        if response.clicked() {
            response.request_focus();
        }
        if scroll {
            response.scroll_to_me(Some(egui::Align::Center));
        }
        let mut value_rect = field.paint(ui, rect, layout.name_width, header.key_icon());
        if layout.wide {
            type_label(ui, rect, header.declared_type().unwrap_or("VALUE"));
            value_rect.max.x -= 64.0;
        }
        let mut action = None;
        if staged {
            let enabled = self.database.commit_ticket.is_none() && !self.database.uncertain_edit;
            if revert_button(ui, rect, value_rect, col, field.name, enabled) {
                action = Some(RecordAction::Revert(col));
            }
            value_rect.max.x -= 24.0;
        }
        if let Some(preview) = &preview {
            action = preview_tools(ui, rect, &mut value_rect, col, preview).or(action);
            let body = egui::Rect::from_min_max(
                rect.min + egui::vec2(8.0, 26.0),
                rect.max - egui::vec2(8.0, 4.0),
            );
            preview.paint(ui, body, staged);
            let area = ui.interact(
                body,
                ui.scope_id().with(("preview", col)),
                egui::Sense::hover(),
            );
            area.widget_info(|| {
                egui::WidgetInfo::labeled(egui::Role::Label, true, preview.describe())
            });
            if preview.controls(ui, body, col) {
                action = Some(RecordAction::Load(col));
            }
        }
        let here = editing && (!self.database.workspace.enabled || self.record_tile_active());
        if here && let Some(inline) = self.database.document.tools.inline.as_mut() {
            return match inline.record_field(ui, value_rect, col) {
                Some(Ok(value)) => Some(RecordAction::Stage(col, value)),
                Some(Err(error)) => {
                    self.database.status = error;
                    action
                }
                None => action,
            };
        }
        let style = self.database.document.tools.columns.style(col);
        field.value(ui, value_rect, style, wrapped, thumb);
        let action = field.action(&response).or(action);
        response.on_hover_ui(|ui| {
            ui.label(format!("{} · {}", field.name, kind_name(field.shown)));
            ui.label(record_tooltip(ui.ctx(), field.shown));
        });
        action
    }
    /// The selected field's preview, when it holds a blob or deferred value;
    /// a decode under way for any other value stops.
    fn field_preview(&mut self, ctx: &egui::Context, row: usize, col: usize) -> Option<Preview> {
        if self.database.document.inspect != Some((row, col)) {
            return None;
        }
        let preview = (!self.editing_at(row, col))
            .then(|| self.record_preview(ctx, row, col))
            .flatten();
        if preview.as_ref().is_none_or(|p| !p.decodes()) {
            self.database.document.tools.values.idle();
        }
        // A database set to load large values reads one its lens needs at once.
        if preview.as_ref().is_some_and(Preview::offers_load)
            && self.preferences.database(&self.database_key()).auto_load
        {
            self.load_field(row, col);
        }
        preview
    }
    pub(super) fn inspection_surfaces(&mut self, ctx: &egui::Context) {
        if self.database.document.ui.inspector_detached && self.database.document.ui.inspector_open
        {
            let mut open = true;
            presentation::dialog(ctx, "Cell detail", &mut open)
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
                    let state = &mut self.database.document.ui;
                    let rows = state.nav != shell::Nav::Overview;
                    ui.horizontal_wrapped(|ui| detail_modes(ui, &mut state.inspector, rows));
                    if self.value_in_dialog()
                        && self.database.document.ui.inspector == shell::DetailMode::Record
                    {
                        // The value view pages and lays out to a fixed height.
                        let height = (ctx.content_rect().height() - 260.0).clamp(240.0, 560.0);
                        let size = egui::vec2(ui.available_width(), height);
                        ui.allocate_ui(size, |ui| {
                            ui.set_min_height(height);
                            self.value_view_ui(ui, true);
                        });
                    } else {
                        egui::ScrollArea::vertical().show(ui, |ui| self.inspector_content(ui));
                    }
                });
            self.database.document.ui.inspector_open = open;
            if !open && self.value_in_dialog() {
                self.close_value();
            }
        }
        if self.database.tools.help {
            let mut open = true;
            presentation::dialog(ctx, "Keyboard and safety reference", &mut open)
                .default_pos(ctx.content_rect().center() - egui::vec2(300.0, 260.0))
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
            (&["Ctrl K"], "Commands: common actions by name"),
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
            (&["Ctrl B"], "Show or hide the sidebar"),
            (
                &["Ctrl I"],
                "Show or hide the assistant: chat with your own signed-in agent CLI",
            ),
            (&["Ctrl T"], "Toggle theme"),
            (&["F7"], "Compare two open schemas"),
            (&["Esc"], "Close the topmost dialog, menu or expanded tile"),
            (
                &[],
                "Tables / History / Saved / Checks sit under the selected database. Settings, commands and this reference sit at the foot of the sidebar. The sidebar and assistant toggles sit at the left of the status bar. Hover any icon for its name.",
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
        "Record value",
        &[
            (
                &["Enter", "Double-click"],
                "Open a blob or large value in the record tile · Esc returns",
            ),
            (&["Arrows", "Shift Arrows"], "Move · extend the selection"),
            (&["PgUp", "PgDn", "Home", "End"], "Page · line start or end"),
            (&["Ctrl Home", "Ctrl End"], "First or last byte"),
            (&["Tab"], "Swap between the bytes and characters columns"),
            (&["Ctrl F"], "Find bytes (00 ff) or text after the cursor"),
            (&["Ctrl G"], "Go to an offset · or click the footer offset"),
            (&["Ctrl C"], "Copy the selection as shown"),
            (
                &["0–9", "a–f", "Insert", "Delete"],
                "Overwrite or insert bytes in an editable blob · staged, never written until commit",
            ),
            (
                &[],
                "Lenses read the same bytes as hex, text, a tree, an image, a vector or inflated contents; choosing one never changes the value.",
            ),
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

/// The detached inspector's modes; without rows there is no cell or record to show.
fn detail_modes(ui: &mut egui::Ui, inspector: &mut shell::DetailMode, rows: bool) {
    for (mode, label, enabled) in [
        (shell::DetailMode::Cell, "Cell", rows),
        (shell::DetailMode::Record, "Record", rows),
        (shell::DetailMode::Structure, "Structure", true),
    ] {
        ui.add_enabled_ui(enabled, |ui| ui.selectable_value(inspector, mode, label));
    }
}
/// The width of the keycap column in the keyboard reference.
const HELP_KEYS: f32 = 160.0;
fn help_reference(ui: &mut egui::Ui) {
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
            ui.horizontal_top(|ui| help_row(ui, keys, description));
        }
    }
}
/// Keycaps in their column, then what they do; a note without keys lines up
/// with the descriptions unless the window is narrow.
fn help_row(ui: &mut egui::Ui, keys: &[&str], description: &str) {
    let narrow = ui.available_width() < HELP_KEYS + 160.0;
    if keys.is_empty() {
        if !narrow {
            ui.add_space(HELP_KEYS + ui.spacing().item_spacing.x);
        }
    } else {
        ui.allocate_ui_with_layout(
            egui::vec2(HELP_KEYS, 0.0),
            egui::Layout::left_to_right(egui::Align::Center).with_main_wrap(true),
            |ui| {
                if !narrow {
                    ui.set_min_width(HELP_KEYS);
                }
                ui.spacing_mut().item_spacing = egui::vec2(4.0, 3.0);
                for key in keys {
                    presentation::keycap(ui, key);
                }
            },
        );
    }
    ui.add(egui::Label::new(description).wrap());
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
    use super::{Value, detail_text, egui, record_tooltip, record_wrap};
    #[test]
    fn long_selected_text_is_bounded_and_byte_captions_keep_sizes() {
        let ctx = egui::Context::default();
        let large = Value::Text("λ".repeat(100_000));
        let tooltip = record_tooltip(&ctx, &large);
        assert!(tooltip.starts_with(&"λ".repeat(4096)));
        assert!(tooltip.ends_with("full value"));
        assert!(tooltip.len() < 8300);
        assert_eq!(
            record_tooltip(&ctx, &Value::Blob(vec![0; 100_000])),
            "97.7 KB in no recognised format"
        );
        let json = Value::Text(r#"{"n":123456789012345678901,"n":1}"#.into());
        assert_eq!(record_tooltip(&ctx, &json), detail_text(&json));
        crate::app::test_ui::render(&ctx, egui::vec2(396.0, 244.0), vec![], |ui| {
            let value = Value::Text("long λ text\n".repeat(100_000));
            let galley = record_wrap(ui, &value, 200.0, true, false).unwrap();
            assert!(galley.rows.len() <= 4);
            assert!(galley.job.text.len() < 8192);
            assert!(record_wrap(ui, &value, 200.0, false, false).is_none());
        });
    }
}
