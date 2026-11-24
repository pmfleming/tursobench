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
    for (label, shortcut, action) in [
        ("Paste cells…", "Ctrl V", Action::PasteCells),
        ("Paste as new rows…", "Ctrl Shift V", Action::PasteRows),
        ("Fill selected cells…", "", Action::Fill),
        ("Set selected cells to NULL", "Backspace", Action::Null),
        ("Undo last staged edit", "Ctrl Z", Action::Undo),
    ] {
        let available = match action {
            Action::Undo => undo,
            Action::PasteRows => enabled,
            _ => enabled && selected,
        };
        if ui
            .add_enabled(available, egui::Button::new(label).shortcut_text(shortcut))
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
#[derive(Debug, Default, PartialEq)]
struct Field {
    text: String,
    quoted: bool,
}
#[derive(Debug)]
pub(super) struct Sheet {
    rows: Vec<Vec<Field>>,
}
/// Clipboard text read so far: complete rows, the open row and its open field.
#[derive(Default)]
struct Reader {
    rows: Vec<Vec<Field>>,
    row: Vec<Field>,
    field: Field,
    cells: usize,
    /// Inside a quoted field.
    inside: bool,
    /// A quoted field has closed; only a separator may follow.
    closed: bool,
}
impl Reader {
    fn end_field(&mut self) {
        self.row.push(std::mem::take(&mut self.field));
        self.cells += 1;
        self.closed = false;
    }
    fn end_row(&mut self) {
        self.end_field();
        self.rows.push(std::mem::take(&mut self.row));
    }
    fn read(
        &mut self,
        c: char,
        chars: &mut std::iter::Peekable<std::str::Chars>,
    ) -> Result<(), String> {
        match c {
            '"' if self.inside => {
                if chars.next_if_eq(&'"').is_some() {
                    self.field.text.push('"');
                } else {
                    self.inside = false;
                    self.closed = true;
                }
            }
            _ if self.inside => self.field.text.push(c),
            '\t' => self.end_field(),
            '\n' | '\r' => {
                if c == '\r' {
                    chars.next_if_eq(&'\n');
                }
                self.end_row();
            }
            '"' if self.field.text.is_empty() && !self.closed => {
                self.field.quoted = true;
                self.inside = true;
            }
            _ if self.closed => return Err("Unexpected text after closing clipboard quote".into()),
            // Quotes within an unquoted value are ordinary text (e.g. JSON).
            _ => self.field.text.push(c),
        }
        if self.field.text.len() > 64 * 1024 || self.cells > MAX_CELLS || self.rows.len() > 1000 {
            return Err("Clipboard exceeds 1,000 rows, 10,000 cells, or 64 KiB per cell".into());
        }
        Ok(())
    }
}
impl Sheet {
    /// Quoted TSV preserves tabs/newlines, doubled quotes, and literal NULL markers.
    pub fn parse(text: &str) -> Result<Self, String> {
        if text.is_empty() || text.len() > CLIPBOARD_BYTES {
            return Err("Clipboard must contain between 1 byte and 2 MiB".into());
        }
        let mut reader = Reader::default();
        let mut chars = text.chars().peekable();
        while let Some(c) = chars.next() {
            reader.read(c, &mut chars)?;
        }
        if reader.inside {
            return Err("Unclosed quoted clipboard field".into());
        }
        // A final line break ends the last row; otherwise it is still open.
        if !text.ends_with(['\n', '\r']) {
            reader.end_row();
        }
        let Reader { rows, cells, .. } = reader;
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
/// The storage class a column's declared type suggests, by SQLite's affinity rules.
fn declared_kind(column: &tursobench_core::TableColumn) -> CellKind {
    const AFFINITIES: [(&[&str], CellKind); 4] = [
        (&["INT"], CellKind::Integer),
        (&["CHAR", "CLOB", "TEXT"], CellKind::Text),
        (&["BLOB"], CellKind::Blob),
        (&["REAL", "FLOA", "DOUB"], CellKind::Real),
    ];
    let ty = column.declared_type.to_ascii_uppercase();
    AFFINITIES
        .iter()
        .find(|(names, _)| names.iter().any(|name| ty.contains(name)))
        .map_or(CellKind::Text, |&(_, kind)| kind)
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
/// What an undo checkpoint of the drafts retains, with an allowance for each
/// edit and column descriptor.
fn draft_bytes(draft: &Option<DraftBatch>) -> usize {
    draft.as_ref().map_or(0, |draft| {
        let column = std::mem::size_of::<tursobench_core::TableColumn>();
        draft.bytes() + draft.edits.len() * 128 + draft.table.columns.len() * column
    })
}
impl State {
    fn matches(&self, document: &Document) -> bool {
        self.revision == document.draft_revision && self.ticket == document.result.current
    }
}
impl Document {
    /// Remember the drafts as they are before a staged change, for undo.
    pub(super) fn checkpoint_drafts(&mut self) {
        let valid = self.tools.spreadsheet.matches(self);
        let bytes = draft_bytes(&self.drafts);
        let state = &mut self.tools.spreadsheet;
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
                draft: self.drafts.clone(),
                bytes,
            });
            state.undo_bytes += bytes;
        }
        state.ticket = self.result.current;
        state.revision = self.draft_revision.wrapping_add(1);
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
        let document = &self.database.document;
        let original = &document.result.rows[r];
        let drafts = document.drafts.as_ref();
        let staged = drafts.and_then(|d| d.displayed(original, c));
        let value = staged.unwrap_or(&original[c]);
        self.database.document.tools.spreadsheet.dialog =
            Some(Dialog::Fill(CellEditor::new(value)));
    }
    /// Where a pasted sheet lands: its first row and the visible columns from
    /// the selection's left, checked against the selection and the result.
    fn paste_target(&self, sheet: &Sheet) -> Result<(usize, Vec<usize>), String> {
        let selection = self
            .current_selection()
            .ok_or("Select a destination cell first")?;
        let selected = self.selected_cells().count();
        if selected == 0 {
            return Err("Select a destination cell first".into());
        }
        let columns = self.visible_columns();
        let position = |column| {
            let position = columns.iter().position(|&c| c == column);
            position.ok_or("Selected column is hidden")
        };
        let start = position(selection.anchor.1)?.min(position(selection.cursor.1)?);
        let row = selection.anchor.0.min(selection.cursor.0);
        let height = selection.anchor.0.abs_diff(selection.cursor.0) + 1;
        if selected > 1 && (height != sheet.rows.len() || selected / height != sheet.width()) {
            return Err("Clipboard dimensions must match the selected rectangle, or select a single starting cell".into());
        }
        if row + sheet.rows.len() > self.database.document.result.rows.len()
            || start + sheet.width() > columns.len()
        {
            return Err(
                "Clipboard extends beyond retained rows or visible columns; nothing staged".into(),
            );
        }
        Ok((row, columns[start..start + sheet.width()].to_vec()))
    }
    /// The sheet as updates of the rows and visible columns it covers, each
    /// field typed like the value it replaces.
    fn clipboard_updates(&self, sheet: &Sheet) -> Result<Vec<Edit>, String> {
        let (first, columns) = self.paste_target(sheet)?;
        let document = &self.database.document;
        let table = document.table.as_ref().ok_or("No editable table")?;
        let drafts = document.drafts.as_ref();
        let update = |(fields, r): (&Vec<Field>, usize)| {
            let original = &document.result.rows[r];
            if drafts.is_some_and(|d| d.deleted(original)) {
                let row = r + 1;
                return Err(format!(
                    "Row {row} is staged for deletion; revert it before pasting"
                ));
            }
            let change = |(field, &column): (&Field, &usize)| {
                let shown = drafts
                    .and_then(|d| d.displayed(original, column))
                    .unwrap_or(&original[column]);
                // A NULL takes its column's declared type; any other value keeps its own.
                let kind = match shown {
                    Value::Null => declared_kind(&table.columns[column]),
                    _ => CellKind::of(shown),
                };
                let name = &table.columns[column].name;
                let value =
                    cell_value(field, kind).map_err(|e| format!("Row {}, {name}: {e}", r + 1));
                Ok((column, value?))
            };
            let changes = fields
                .iter()
                .zip(&columns)
                .map(change)
                .collect::<Result<_, String>>()?;
            Ok(Edit::Update {
                original: original.clone(),
                changes,
            })
        };
        sheet.rows.iter().zip(first..).map(update).collect()
    }
    pub(super) fn clipboard_inserts(
        &self,
        sheet: &Sheet,
        mapping: &[Mapping],
    ) -> Result<Vec<Edit>, String> {
        let Some(table) = &self.database.document.table else {
            return Err("No editable table".into());
        };
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
        let document = &self.database.document;
        let drafts = document.drafts.as_ref();
        if cells
            .iter()
            .any(|&(r, _)| drafts.is_some_and(|d| d.deleted(&document.result.rows[r])))
        {
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
                    // Columns map in visible order, each typed as its column is declared.
                    let visible = self.visible_columns();
                    let table = self.database.document.table.as_ref();
                    let declared = |c: usize| table.map(|t| declared_kind(&t.columns[c]));
                    paste.mapping = (0..sheet.width())
                        .map(|i| {
                            let column = visible.get(i).copied();
                            let kind = column.and_then(declared).unwrap_or(CellKind::Text);
                            Mapping { column, kind }
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
            self.sheet_preview(ui, sheet, paste.new_rows, &mut paste.mapping);
        }
        if !paste.error.is_empty() {
            ui.colored_label(ui.visuals().error_fg_color, &paste.error);
        }
    }
    /// The parsed sheet's size, where each column goes when pasting new rows,
    /// and up to ten rows as they would be staged.
    fn sheet_preview(
        &self,
        ui: &mut egui::Ui,
        sheet: &Sheet,
        new_rows: bool,
        mapping: &mut [Mapping],
    ) {
        let (rows, columns) = (sheet.rows.len(), sheet.width());
        ui.label(format!(
            "{rows} rows × {columns} columns · preview shows up to 10 rows"
        ));
        if new_rows {
            ui.small(
                "Unmapped table columns use their defaults. Constraints are checked on commit.",
            );
            if let Some(table) = &self.database.document.table {
                for (i, mapping) in mapping.iter_mut().enumerate() {
                    ui.horizontal(|ui| mapping_ui(ui, table, i, mapping));
                }
            }
        }
        egui::ScrollArea::horizontal()
            .id_salt("paste-preview")
            .show(ui, |ui| {
                preview_grid(ui, sheet, new_rows.then_some(&*mapping))
            });
    }
    pub(super) fn spreadsheet_window(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.database.document.tools.spreadsheet.dialog.take() else {
            return;
        };
        let mut open = true;
        let mut prepared = None;
        let enabled = self.can_edit();
        egui::Window::new(dialog.title())
            .id(egui::Id::unique("spreadsheet-edit"))
            .default_pos(ctx.content_rect().center() - egui::vec2(320.0, 240.0))
            .open(&mut open)
            .default_width(640.0)
            .max_width((ctx.content_rect().width() - 32.0).max(240.0))
            .max_height(ctx.content_rect().height() * 0.85)
            .show(ctx, |ui| {
                let table = self.database.document.table.as_ref();
                ui.label(table.map_or("No table", |t| t.name.as_str()));
                ui.small("Changes are staged locally. Commit explicitly to write.");
                egui::ScrollArea::vertical()
                    .id_salt("spreadsheet-body")
                    .max_height((ctx.content_rect().height() * 0.85 - 180.0).max(80.0))
                    .show(ui, |ui| match &mut dialog {
                        Dialog::Fill(editor) => {
                            ui.label(format!("{} selected cells", self.selected_cells().count()));
                            editor.form(ui, "fill-type");
                        }
                        Dialog::Paste(paste) => self.paste_dialog_ui(ui, paste),
                    });
                ui.separator();
                prepared = self.stage_button_ui(ui, &mut dialog, enabled);
                if !enabled {
                    ui.label("Editing is unavailable for the current database state.");
                }
                ui.add(egui::Label::new(&self.database.status).truncate())
                    .on_hover_text(&self.database.status);
            });
        let applied = match prepared {
            Some(Prepared::Edits(edits)) => self.stage_spreadsheet(edits),
            Some(Prepared::Fill(value)) => self.fill_selection(value),
            None => false,
        };
        if open && !applied {
            self.database.document.tools.spreadsheet.dialog = Some(dialog);
        }
    }
    /// The dialog's stage button; a failure to prepare stays in the dialog.
    fn stage_button_ui(
        &mut self,
        ui: &mut egui::Ui,
        dialog: &mut Dialog,
        enabled: bool,
    ) -> Option<Prepared> {
        match dialog {
            Dialog::Fill(editor) => {
                if !ui
                    .add_enabled(enabled, egui::Button::new("Stage fill"))
                    .clicked()
                {
                    return None;
                }
                match editor.parse() {
                    Ok(value) => Some(Prepared::Fill(value)),
                    Err(e) => {
                        self.database.status = e;
                        None
                    }
                }
            }
            Dialog::Paste(paste) => {
                let clicked = ui
                    .add_enabled(
                        enabled && paste.sheet.is_some(),
                        egui::Button::new("Stage entire paste"),
                    )
                    .clicked();
                let sheet = paste.sheet.as_ref().filter(|_| clicked)?;
                let edits = if paste.new_rows {
                    self.clipboard_inserts(sheet, &paste.mapping)
                } else {
                    self.clipboard_updates(sheet)
                };
                match edits {
                    Ok(edits) => Some(Prepared::Edits(edits)),
                    Err(error) => {
                        paste.error = error;
                        None
                    }
                }
            }
        }
    }
}

/// What a dialog prepared to stage this frame.
enum Prepared {
    Edits(Vec<Edit>),
    Fill(Value),
}

impl Dialog {
    fn title(&self) -> &'static str {
        match self {
            Dialog::Paste(p) if p.new_rows => "Paste as new rows",
            Dialog::Paste(_) => "Paste cells",
            Dialog::Fill(_) => "Fill selected cells",
        }
    }
}

/// One clipboard column's destination and type; choosing a column resets
/// the type to that column's declared one.
fn mapping_ui(ui: &mut egui::Ui, table: &tursobench_core::Table, i: usize, mapping: &mut Mapping) {
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
                ui.selectable_value(&mut mapping.column, Some(c), &column.name);
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
}

/// Up to ten clipboard rows as they would be staged: typed values when
/// pasting new rows with `mappings`, otherwise the raw fields.
fn preview_grid(ui: &mut egui::Ui, sheet: &Sheet, mappings: Option<&[Mapping]>) {
    egui::Grid::new("paste-preview-grid")
        .striped(true)
        .show(ui, |ui| {
            for row in sheet.rows.iter().take(10) {
                for (i, field) in row.iter().enumerate() {
                    let displayed = preview_text(field, mappings.map(|m| m[i]));
                    ui.add_sized([140.0, 24.0], egui::Label::new(displayed).truncate());
                }
                ui.end_row();
            }
        });
}

fn preview_text(field: &Field, mapping: Option<Mapping>) -> String {
    let Some(mapping) = mapping else {
        return if !field.quoted && field.text == "\\N" {
            "NULL".into()
        } else if field.text.is_empty() {
            "(empty text)".into()
        } else {
            field.text.chars().take(100).collect()
        };
    };
    if mapping.column.is_none() {
        return "(skipped)".into();
    }
    match cell_value(field, mapping.kind) {
        Ok(Value::Text(s)) if s.is_empty() => "(empty text)".into(),
        Ok(value) => value.preview(),
        Err(e) => format!("Invalid: {e}"),
    }
}

#[cfg(test)]
mod tests {
    use super::{CLIPBOARD_BYTES, CellKind, Field, MAX_CELLS, Sheet, Value, cell_value};
    #[test]
    fn clipboard_quotes_empty_null_unicode_and_line_endings() {
        let sheet = Sheet::parse("\"a\tb\"\t\"line\nλ\"\t\"a\"\"b\"\t\t\\N\t\"\\N\"\r\n").unwrap();
        assert_eq!(sheet.rows.len(), 1);
        assert_eq!(sheet.rows[0][0].text, "a\tb");
        assert_eq!(sheet.rows[0][1].text, "line\nλ");
        assert_eq!(sheet.rows[0][2].text, "a\"b");
        for (i, kind, expected) in [
            (3, CellKind::Integer, Value::Text(String::new())),
            (4, CellKind::Text, Value::Null),
            (5, CellKind::Text, Value::Text("\\N".into())),
        ] {
            assert_eq!(cell_value(&sheet.rows[0][i], kind), Ok(expected));
        }
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
        for (text, kind, expected) in [
            (
                "9007199254740993",
                CellKind::Integer,
                Value::Integer(9007199254740993),
            ),
            ("00ff", CellKind::Blob, Value::Blob(vec![0, 255])),
            ("001", CellKind::Text, Value::Text("001".into())),
        ] {
            assert_eq!(cell_value(&field(text), kind), Ok(expected));
        }
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
