//! Bounded clipboard operations are prepared in full, then staged as one undoable edit.
use super::{
    App,
    document::Document,
    results::{CellEditor, CellKind},
};
use eframe::egui;
use std::collections::VecDeque;
use tursobench_core::{DraftBatch, Edit, Ticket, Value};

const CLIPBOARD_BYTES: usize = 2 * 1024 * 1024;
const MAX_CELLS: usize = 10_000;
const UNDO_BYTES: usize = 16 * 1024 * 1024;

#[derive(Default)]
pub(super) struct State {
    pub dialog: Option<Dialog>,
    undo: VecDeque<Checkpoint>,
    undo_bytes: usize,
    revision: u64,
    ticket: Option<Ticket>,
}
struct Checkpoint {
    draft: Option<DraftBatch>,
    bytes: usize,
}
#[derive(Clone, Copy)]
pub(super) enum Action {
    PasteCells,
    PasteRows,
    Fill,
    Null,
    Undo,
}

pub(super) fn menu(ui: &mut egui::Ui, enabled: bool, selected: bool, undo: bool) -> Option<Action> {
    for (label, action) in [
        ("Paste cells…", Action::PasteCells),
        ("Paste as new rows…", Action::PasteRows),
        ("Fill selected cells…", Action::Fill),
        ("Set selected cells to NULL", Action::Null),
        ("Undo last staged edit · Ctrl+Z", Action::Undo),
    ] {
        let available = match action {
            Action::Undo => undo,
            Action::PasteRows => enabled,
            _ => enabled && selected,
        };
        if ui
            .add_enabled(available, egui::Button::new(label))
            .clicked()
        {
            ui.close();
            return Some(action);
        }
    }
    None
}

pub(super) enum Dialog {
    Paste(Paste),
    Fill(CellEditor),
}
pub(super) struct Paste {
    pub text: String,
    pub new_rows: bool,
    sheet: Option<Sheet>,
    pub mapping: Vec<Mapping>,
    error: String,
    focus: bool,
}
#[derive(Clone, Copy)]
pub(super) struct Mapping {
    pub column: Option<usize>,
    pub kind: CellKind,
}
#[derive(Debug, PartialEq)]
struct Field {
    text: String,
    quoted: bool,
}
#[derive(Debug)]
pub(super) struct Sheet {
    rows: Vec<Vec<Field>>,
}
impl Sheet {
    /// Quoted TSV preserves tabs/newlines, doubled quotes, and literal NULL markers.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > CLIPBOARD_BYTES {
            return Err("Clipboard must contain between 1 byte and 2 MiB".into());
        }
        let mut chars = text.chars().peekable();
        let mut rows = Vec::new();
        let mut row = Vec::new();
        let mut value = String::new();
        let (mut quoted, mut inside, mut closed, mut pending) = (false, false, false, false);
        let mut cells = 0;
        while let Some(c) = chars.next() {
            pending = true;
            if inside {
                if c == '"' {
                    if chars.peek() == Some(&'"') {
                        chars.next();
                        value.push('"');
                    } else {
                        inside = false;
                        closed = true;
                    }
                } else {
                    value.push(c);
                }
            } else if matches!(c, '\t' | '\n' | '\r') {
                row.push(Field {
                    text: std::mem::take(&mut value),
                    quoted,
                });
                cells += 1;
                quoted = false;
                closed = false;
                if c != '\t' {
                    if c == '\r' && chars.peek() == Some(&'\n') {
                        chars.next();
                    }
                    rows.push(std::mem::take(&mut row));
                    pending = false;
                }
            } else if c == '"' && value.is_empty() && !closed {
                quoted = true;
                inside = true;
            } else if closed {
                return Err("Unexpected text after closing clipboard quote".into());
            } else {
                // Quotes within an unquoted value are ordinary text (e.g. JSON).
                value.push(c);
            }
            if value.len() > 64 * 1024 || cells > MAX_CELLS || rows.len() > 1000 {
                return Err(
                    "Clipboard exceeds 1,000 rows, 10,000 cells, or 64 KiB per cell".into(),
                );
            }
        }
        if inside {
            return Err("Unclosed quoted clipboard field".into());
        }
        if pending {
            row.push(Field {
                text: value,
                quoted,
            });
            cells += 1;
            rows.push(row);
        }
        if rows.is_empty() || rows.len() > 1000 || cells > MAX_CELLS {
            return Err("Clipboard exceeds 1,000 rows or 10,000 cells".into());
        }
        let width = rows[0].len();
        if rows.iter().any(|row| row.len() != width) {
            return Err("Clipboard rows must all have the same number of columns".into());
        }
        Ok(Self { rows })
    }
    pub fn width(&self) -> usize {
        self.rows[0].len()
    }
}
fn declared_kind(column: &tursobench_core::TableColumn) -> CellKind {
    let ty = column.declared_type.to_ascii_uppercase();
    if ty.contains("INT") {
        CellKind::Integer
    } else if ty.contains("CHAR") || ty.contains("CLOB") || ty.contains("TEXT") {
        CellKind::Text
    } else if ty.contains("BLOB") {
        CellKind::Blob
    } else if ty.contains("REAL") || ty.contains("FLOA") || ty.contains("DOUB") {
        CellKind::Real
    } else {
        CellKind::Text
    }
}
fn cell_value(field: &Field, kind: CellKind) -> Result<Value, String> {
    if !field.quoted && field.text == "\\N" {
        return Ok(Value::Null);
    }
    // Never silently turn an empty spreadsheet field into NULL or numeric zero.
    if field.text.is_empty() {
        return Ok(Value::Text(String::new()));
    }
    kind.parse(&field.text)
}
fn draft_bytes(draft: &Option<DraftBatch>) -> usize {
    draft.as_ref().map_or(0, |draft| {
        draft
            .edits
            .iter()
            .map(|edit| match edit {
                Edit::Insert { values } => values.iter().flatten().map(Value::bytes).sum::<usize>(),
                Edit::Update { original, changes } => {
                    original.iter().map(Value::bytes).sum::<usize>()
                        + changes.iter().map(|(_, v)| v.bytes()).sum::<usize>()
                }
                Edit::Delete { original } => original.iter().map(Value::bytes).sum::<usize>(),
            })
            .sum::<usize>()
            + draft.edits.len() * 128
            + draft.table.name.len()
            + draft
                .table
                .columns
                .iter()
                .map(|column| {
                    std::mem::size_of_val(column)
                        + column.name.len()
                        + column.declared_type.len()
                        + column.default_sql.as_ref().map_or(0, String::len)
                })
                .sum::<usize>()
    })
}
impl State {
    fn matches(&self, document: &Document) -> bool {
        self.revision == document.draft_revision && self.ticket == document.result.current
    }
    pub fn checkpoint(document: &mut Document) {
        let valid = document.tools.spreadsheet.matches(document);
        let bytes = draft_bytes(&document.drafts);
        let state = &mut document.tools.spreadsheet;
        if !valid {
            state.undo.clear();
            state.undo_bytes = 0;
        }
        while state.undo.len() >= 32 || state.undo_bytes + bytes > UNDO_BYTES {
            let Some(old) = state.undo.pop_front() else {
                break;
            };
            state.undo_bytes -= old.bytes;
        }
        if bytes <= UNDO_BYTES {
            state.undo.push_back(Checkpoint {
                draft: document.drafts.clone(),
                bytes,
            });
            state.undo_bytes += bytes;
        }
        state.ticket = document.result.current;
        state.revision = document.draft_revision.wrapping_add(1);
    }
}
impl App {
    pub(super) fn can_undo_grid_edit(&self) -> bool {
        let document = &self.database.document;
        self.can_edit()
            && document.tools.spreadsheet.matches(document)
            && !document.tools.spreadsheet.undo.is_empty()
    }
    pub(super) fn undo_grid_edit(&mut self) {
        if !self.can_undo_grid_edit() {
            return;
        }
        let document = &mut self.database.document;
        let state = &mut document.tools.spreadsheet;
        if let Some(checkpoint) = state.undo.pop_back() {
            state.undo_bytes -= checkpoint.bytes;
            document.drafts = checkpoint.draft;
            document.draft_revision = document.draft_revision.wrapping_add(1);
            state.revision = document.draft_revision;
            self.database.status = "Undid one local edit operation; database unchanged".into();
        }
    }
    pub(super) fn open_paste(&mut self, new_rows: bool, text: String) {
        if !self.can_edit() {
            return;
        }
        self.database.document.tools.spreadsheet.dialog = Some(Dialog::Paste(Paste {
            text,
            new_rows,
            sheet: None,
            mapping: vec![],
            error: String::new(),
            focus: true,
        }));
    }
    pub(super) fn open_fill(&mut self) {
        if !self.can_edit() {
            return;
        }
        let Some((r, c)) = self.selected_cells().next() else {
            return;
        };
        let original = &self.database.document.result.rows[r];
        let value = self
            .database
            .document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(original, c))
            .unwrap_or(&original[c]);
        self.database.document.tools.spreadsheet.dialog =
            Some(Dialog::Fill(CellEditor::new(value)));
    }
    fn clipboard_updates(&self, sheet: &Sheet) -> Result<Vec<Edit>, String> {
        let document = &self.database.document;
        let selection = document
            .tools
            .grid_selection
            .filter(|s| s.ticket == document.result.current)
            .ok_or("Select a destination cell first")?;
        let columns = self.visible_columns();
        let selected: Vec<_> = self.selected_cells().collect();
        if selected.is_empty() {
            return Err("Select a destination cell first".into());
        }
        let row = selection.anchor.0.min(selection.cursor.0);
        let start = columns
            .iter()
            .position(|c| *c == selection.anchor.1)
            .ok_or("Selected column is hidden")?
            .min(
                columns
                    .iter()
                    .position(|c| *c == selection.cursor.1)
                    .ok_or("Selected column is hidden")?,
            );
        let height = selection.anchor.0.abs_diff(selection.cursor.0) + 1;
        let width = selected.len() / height;
        if selected.len() > 1 && (height != sheet.rows.len() || width != sheet.width()) {
            return Err("Clipboard dimensions must match the selected rectangle, or select a single starting cell".into());
        }
        if row + sheet.rows.len() > document.result.rows.len()
            || start + sheet.width() > columns.len()
        {
            return Err(
                "Clipboard extends beyond retained rows or visible columns; nothing staged".into(),
            );
        }
        let table = document.table.as_ref().ok_or("No editable table")?;
        sheet
            .rows
            .iter()
            .enumerate()
            .map(|(r, fields)| {
                let original = &document.result.rows[row + r];
                if document
                    .drafts
                    .as_ref()
                    .is_some_and(|d| d.deleted(original))
                {
                    return Err(format!(
                        "Row {} is staged for deletion; revert it before pasting",
                        row + r + 1
                    ));
                }
                let changes = fields
                    .iter()
                    .enumerate()
                    .map(|(c, field)| {
                        let column = columns[start + c];
                        let displayed = document
                            .drafts
                            .as_ref()
                            .and_then(|d| d.displayed(original, column))
                            .unwrap_or(&original[column]);
                        let kind = if matches!(displayed, Value::Null) {
                            declared_kind(&table.columns[column])
                        } else {
                            CellEditor::new(displayed).kind
                        };
                        cell_value(field, kind)
                            .map(|value| (column, value))
                            .map_err(|e| {
                                format!("Row {}, {}: {e}", row + r + 1, table.columns[column].name)
                            })
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(Edit::Update {
                    original: original.clone(),
                    changes,
                })
            })
            .collect()
    }
    pub(super) fn clipboard_inserts(
        &self,
        sheet: &Sheet,
        mapping: &[Mapping],
    ) -> Result<Vec<Edit>, String> {
        let table = self
            .database
            .document
            .table
            .as_ref()
            .ok_or("No editable table")?;
        if mapping.len() != sheet.width() {
            return Err("Map every clipboard column or choose Skip".into());
        }
        let mut seen = std::collections::HashSet::new();
        for m in mapping {
            if let Some(c) = m.column
                && (c >= table.columns.len() || table.columns[c].hidden != 0 || !seen.insert(c))
            {
                return Err("Each writable destination column may be mapped only once".into());
            }
        }
        if seen.is_empty() {
            return Err("Map at least one destination column".into());
        }
        sheet
            .rows
            .iter()
            .enumerate()
            .map(|(r, fields)| {
                let mut values = vec![None; table.columns.len()];
                for (field, m) in fields.iter().zip(mapping) {
                    if let Some(c) = m.column {
                        values[c] = Some(cell_value(field, m.kind).map_err(|e| {
                            format!("Clipboard row {}, {}: {e}", r + 1, table.columns[c].name)
                        })?);
                    }
                }
                Ok(Edit::Insert { values })
            })
            .collect()
    }
    pub(super) fn stage_spreadsheet(&mut self, edits: Vec<Edit>) -> bool {
        self.stage(|draft| {
            for edit in edits {
                match edit {
                    Edit::Update { original, changes } => {
                        for (column, value) in changes {
                            draft.stage_cell(original.clone(), column, value);
                        }
                    }
                    insert => draft.edits.push(insert),
                }
            }
        })
    }
    pub(super) fn paste_cells(&mut self, text: &str) -> bool {
        if !self.can_edit() {
            self.database.status = "Paste requires an editable table in read/write mode".into();
            return false;
        }
        match Sheet::parse(text).and_then(|s| self.clipboard_updates(&s)) {
            Ok(edits) => self.stage_spreadsheet(edits),
            Err(e) => {
                self.database.status = e;
                false
            }
        }
    }
    pub(super) fn fill_selection(&mut self, value: Value) -> bool {
        if !self.can_edit() {
            return false;
        }
        let cells = self.selected_cells().collect::<Vec<_>>();
        if cells.is_empty() || cells.len() > MAX_CELLS {
            self.database.status = "Select between 1 and 10,000 cells to fill".into();
            return false;
        }
        if value.bytes().saturating_mul(cells.len()) > 8 * 1024 * 1024 {
            self.database.status = "Fill exceeds the 8 MiB edit budget; nothing staged".into();
            return false;
        }
        if cells.iter().any(|(r, _)| {
            self.database
                .document
                .drafts
                .as_ref()
                .is_some_and(|d| d.deleted(&self.database.document.result.rows[*r]))
        }) {
            self.database.status =
                "Selection includes a row staged for deletion; revert it before filling".into();
            return false;
        }
        let mut rows = std::collections::BTreeMap::<usize, Vec<(usize, Value)>>::new();
        for (r, c) in cells {
            rows.entry(r).or_default().push((c, value.clone()));
        }
        let edits = rows
            .into_iter()
            .map(|(r, changes)| Edit::Update {
                original: self.database.document.result.rows[r].clone(),
                changes,
            })
            .collect();
        self.stage_spreadsheet(edits)
    }
    /// The menu also lives in the table ribbon, so empty tables can receive pasted rows.
    pub(super) fn spreadsheet_menu(&mut self, ui: &mut egui::Ui) {
        let selected = self.selected_cells().next().is_some();
        if let Some(action) = menu(ui, self.can_edit(), selected, self.can_undo_grid_edit()) {
            self.spreadsheet_action(action);
        }
    }
    pub(super) fn spreadsheet_action(&mut self, action: Action) {
        match action {
            Action::PasteCells => self.open_paste(false, String::new()),
            Action::PasteRows => self.open_paste(true, String::new()),
            Action::Fill => self.open_fill(),
            Action::Null => {
                self.fill_selection(Value::Null);
            }
            Action::Undo => self.undo_grid_edit(),
        }
    }
    pub(super) fn preview_clipboard(&self, paste: &mut Paste) {
        match Sheet::parse(&paste.text) {
            Ok(sheet) => {
                if paste.mapping.len() != sheet.width() {
                    let visible = self.visible_columns();
                    paste.mapping = (0..sheet.width())
                        .map(|i| {
                            let column = visible.get(i).copied();
                            Mapping {
                                column,
                                kind: column
                                    .and_then(|c| {
                                        self.database
                                            .document
                                            .table
                                            .as_ref()
                                            .map(|t| declared_kind(&t.columns[c]))
                                    })
                                    .unwrap_or(CellKind::Text),
                            }
                        })
                        .collect();
                }
                paste.sheet = Some(sheet);
                paste.error.clear();
            }
            Err(error) => {
                paste.sheet = None;
                paste.error = error;
            }
        }
    }
    fn paste_dialog_ui(&self, ui: &mut egui::Ui, paste: &mut Paste) {
        ui.label("Paste tab-separated clipboard data here (Ctrl+V). No header row.");
        let response = ui.add(
            egui::TextEdit::multiline(&mut paste.text)
                .desired_rows(4)
                .desired_width(f32::INFINITY),
        );
        if std::mem::take(&mut paste.focus) {
            response.request_focus();
        }
        if response.changed() {
            paste.sheet = None;
            paste.error.clear();
        }
        ui.small("Unquoted \\N = SQL NULL. Empty fields = empty text. Quote \\N to keep it as text. Blobs use hex pairs.");
        if ui.button("Preview clipboard").clicked() {
            self.preview_clipboard(paste);
        }
        if let Some(sheet) = &paste.sheet {
            ui.label(format!(
                "{} rows × {} columns · preview shows up to 10 rows",
                sheet.rows.len(),
                sheet.width()
            ));
            if paste.new_rows {
                ui.small(
                    "Unmapped table columns use their defaults. Constraints are checked on commit.",
                );
                if let Some(table) = &self.database.document.table {
                    for (i, mapping) in paste.mapping.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.label(format!("Clipboard {} →", i + 1));
                            let before = mapping.column;
                            egui::ComboBox::from_id_salt(("paste-column", i))
                                .selected_text(
                                    mapping
                                        .column
                                        .map_or("Skip", |c| table.columns[c].name.as_str()),
                                )
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut mapping.column, None, "Skip");
                                    for (c, column) in table
                                        .columns
                                        .iter()
                                        .enumerate()
                                        .filter(|(_, c)| c.hidden == 0)
                                    {
                                        ui.selectable_value(
                                            &mut mapping.column,
                                            Some(c),
                                            &column.name,
                                        );
                                    }
                                });
                            if before != mapping.column {
                                mapping.kind = mapping
                                    .column
                                    .map_or(CellKind::Text, |c| declared_kind(&table.columns[c]));
                            }
                            let mut editor = CellEditor {
                                kind: mapping.kind,
                                input: String::new(),
                            };
                            editor.type_picker(ui, ("paste-kind", i), true);
                            mapping.kind = editor.kind;
                        });
                    }
                }
            }
            egui::ScrollArea::horizontal()
                .id_salt("paste-preview")
                .show(ui, |ui| {
                    egui::Grid::new("paste-preview-grid")
                        .striped(true)
                        .show(ui, |ui| {
                            for row in sheet.rows.iter().take(10) {
                                for (i, field) in row.iter().enumerate() {
                                    let displayed = if paste.new_rows {
                                        let mapping = &paste.mapping[i];
                                        if mapping.column.is_none() {
                                            "(skipped)".into()
                                        } else {
                                            cell_value(field, mapping.kind)
                                                .map(|v| match &v {
                                                    Value::Text(s) if s.is_empty() => {
                                                        "(empty text)".into()
                                                    }
                                                    _ => v.preview(),
                                                })
                                                .unwrap_or_else(|e| format!("Invalid: {e}"))
                                        }
                                    } else if !field.quoted && field.text == "\\N" {
                                        "NULL".into()
                                    } else if field.text.is_empty() {
                                        "(empty text)".into()
                                    } else {
                                        field.text.chars().take(100).collect()
                                    };
                                    ui.add_sized(
                                        [140.0, 24.0],
                                        egui::Label::new(displayed).truncate(),
                                    );
                                }
                                ui.end_row();
                            }
                        });
                });
        }
        if !paste.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &paste.error);
        }
    }
    pub(super) fn spreadsheet_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.database.document.tools.spreadsheet.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut stage = None;
        let mut fill = None;
        let enabled = self.can_edit();
        let title = match &dialog {
            Dialog::Paste(p) if p.new_rows => "Paste as new rows",
            Dialog::Paste(_) => "Paste cells",
            Dialog::Fill(_) => "Fill selected cells",
        };
        egui::Window::new(title)
            .id(egui::Id::unique("spreadsheet-edit"))
            .default_pos(ctx.content_rect().center() - egui::vec2(320.0, 240.0))
            .open(&mut open)
            .default_width(640.0)
            .max_width((ctx.content_rect().width() - 32.0).max(240.0))
            .max_height(ctx.content_rect().height() * 0.85)
            .show(ctx, |ui| {
                ui.label(
                    self.database
                        .document
                        .table
                        .as_ref()
                        .map_or("No table", |t| t.name.as_str()),
                );
                ui.small("Changes are staged locally. Commit explicitly to write.");
                egui::ScrollArea::vertical()
                    .id_salt("spreadsheet-body")
                    .max_height((ctx.content_rect().height() * 0.85 - 180.0).max(80.0))
                    .show(ui, |ui| match &mut dialog {
                        Dialog::Fill(editor) => {
                            ui.label(format!("{} selected cells", self.selected_cells().count()));
                            editor.type_picker(ui, "fill-type", true);
                            if editor.kind != CellKind::Null {
                                ui.add(
                                    egui::TextEdit::multiline(&mut editor.input)
                                        .desired_rows(3)
                                        .desired_width(f32::INFINITY),
                                );
                            }
                        }
                        Dialog::Paste(paste) => self.paste_dialog_ui(ui, paste),
                    });
                ui.separator();
                match &mut dialog {
                    Dialog::Fill(editor) => {
                        if ui
                            .add_enabled(enabled, egui::Button::new("Stage fill"))
                            .clicked()
                        {
                            match editor.parse() {
                                Ok(value) => fill = Some(value),
                                Err(e) => self.database.status = e,
                            }
                        }
                    }
                    Dialog::Paste(paste) => {
                        if ui
                            .add_enabled(
                                enabled && paste.sheet.is_some(),
                                egui::Button::new("Stage entire paste"),
                            )
                            .clicked()
                            && let Some(sheet) = &paste.sheet
                        {
                            match if paste.new_rows {
                                self.clipboard_inserts(sheet, &paste.mapping)
                            } else {
                                self.clipboard_updates(sheet)
                            } {
                                Ok(prepared) => stage = Some(prepared),
                                Err(error) => paste.error = error,
                            }
                        }
                    }
                }
                if !enabled {
                    ui.label("Editing is unavailable for the current database state.");
                }
                ui.add(egui::Label::new(&self.database.status).truncate())
                    .on_hover_text(&self.database.status);
            });
        let applied = stage.is_some_and(|edits| self.stage_spreadsheet(edits))
            || fill.is_some_and(|value| self.fill_selection(value));
        if open && !applied {
            self.database.document.tools.spreadsheet.dialog = Some(dialog);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn clipboard_quotes_empty_null_unicode_and_line_endings() {
        let sheet = Sheet::parse("\"a\tb\"\t\"line\nλ\"\t\"a\"\"b\"\t\t\\N\t\"\\N\"\r\n").unwrap();
        assert_eq!(sheet.rows.len(), 1);
        assert_eq!(sheet.rows[0][0].text, "a\tb");
        assert_eq!(sheet.rows[0][1].text, "line\nλ");
        assert_eq!(sheet.rows[0][2].text, "a\"b");
        assert_eq!(
            cell_value(&sheet.rows[0][3], CellKind::Integer).unwrap(),
            Value::Text(String::new())
        );
        assert_eq!(
            cell_value(&sheet.rows[0][4], CellKind::Text).unwrap(),
            Value::Null
        );
        assert_eq!(
            cell_value(&sheet.rows[0][5], CellKind::Text).unwrap(),
            Value::Text("\\N".into())
        );
        assert_eq!(
            Sheet::parse("a\tb\n\n").unwrap_err(),
            "Clipboard rows must all have the same number of columns"
        );
        assert_eq!(Sheet::parse("a\r\nb\rc\n").unwrap().rows.len(), 3);
        assert_eq!(
            Sheet::parse("{\"n\":1}").unwrap().rows[0][0].text,
            "{\"n\":1}"
        );
        for invalid in ["", "\"unclosed", "\"closed\"x", "a\tb\nc"] {
            assert!(Sheet::parse(invalid).is_err(), "{invalid}");
        }
        assert!(Sheet::parse(&"x".repeat(CLIPBOARD_BYTES + 1)).is_err());
        assert!(Sheet::parse(&"x\n".repeat(1001)).is_err());
        assert!(Sheet::parse(&"x\t".repeat(MAX_CELLS)).is_err());
        assert!(Sheet::parse(&"x".repeat(64 * 1024 + 1)).is_err());
    }
    #[test]
    fn typed_clipboard_values_keep_exact_integers_and_reject_bad_numbers() {
        let field = |text: &str| Field {
            text: text.into(),
            quoted: false,
        };
        assert_eq!(
            cell_value(&field("9007199254740993"), CellKind::Integer).unwrap(),
            Value::Integer(9007199254740993)
        );
        assert_eq!(
            cell_value(&field("00ff"), CellKind::Blob).unwrap(),
            Value::Blob(vec![0, 255])
        );
        assert_eq!(
            cell_value(&field("001"), CellKind::Text).unwrap(),
            Value::Text("001".into())
        );
        for (text, kind) in [
            ("9223372036854775808", CellKind::Integer),
            ("1.5", CellKind::Integer),
            ("NaN", CellKind::Real),
            ("inf", CellKind::Real),
            ("ffg", CellKind::Blob),
        ] {
            assert!(cell_value(&field(text), kind).is_err());
        }
    }
}
