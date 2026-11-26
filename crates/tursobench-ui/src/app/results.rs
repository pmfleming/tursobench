//! Result value editors, bounded value viewing, and clipboard/export presentation.
use super::{App, shell};
use eframe::egui;
use tursobench_core::{DeferredCell, EventKind, LargeKind, Ticket, Value};
use tursobench_db::Operation;

use crate::export::{self, Format};
#[derive(Clone, Copy, Default, PartialEq, Eq)]
pub(super) enum CellKind {
    Null,
    Integer,
    Real,
    #[default]
    Text,
    Blob,
}
impl CellKind {
    /// The storage class of a value; a deferred value is edited as text.
    pub(super) fn of(value: &Value) -> Self {
        match value {
            Value::Null => Self::Null,
            Value::Integer(_) => Self::Integer,
            Value::Real(_) => Self::Real,
            Value::Text(_) | Value::Deferred(_) => Self::Text,
            Value::Blob(_) => Self::Blob,
        }
    }
    /// Every kind with its label, in declaration order.
    const ALL: [(Self, &'static str); 5] = [
        (Self::Null, "NULL"),
        (Self::Integer, "Integer"),
        (Self::Real, "Real"),
        (Self::Text, "Text"),
        (Self::Blob, "Blob (hex)"),
    ];
    pub(super) fn label(self) -> &'static str {
        Self::ALL[self as usize].1
    }
    pub(super) fn parse(self, input: &str) -> Result<Value, String> {
        Ok(match self {
            Self::Null => Value::Null,
            Self::Integer => {
                Value::Integer(input.trim().parse::<i64>().map_err(|e| e.to_string())?)
            }
            Self::Real => {
                let n = input.trim().parse::<f64>().map_err(|e| e.to_string())?;
                if !n.is_finite() {
                    return Err("Edits require finite real values".into());
                }
                Value::Real(n)
            }
            Self::Text => Value::Text(input.into()),
            Self::Blob => {
                let s: String = input.chars().filter(|c| !c.is_ascii_whitespace()).collect();
                if !s.len().is_multiple_of(2) || !s.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err("Blob requires pairs of hexadecimal digits, no prefix".into());
                }
                Value::Blob(
                    (0..s.len())
                        .step_by(2)
                        .map(|i| u8::from_str_radix(&s[i..i + 2], 16))
                        .collect::<Result<_, _>>()
                        .map_err(|e| e.to_string())?,
                )
            }
        })
    }
}
#[derive(Default)]
pub(super) struct CellEditor {
    pub kind: CellKind,
    pub input: String,
}
impl CellEditor {
    pub fn new(value: &Value) -> Self {
        Self {
            kind: CellKind::of(value),
            input: match value {
                Value::Null => String::new(),
                Value::Blob(b) => export::hex(b),
                _ => export::cell_text(value),
            },
        }
    }
    pub fn inline(value: &Value) -> Option<Self> {
        (!matches!(value, Value::Blob(_) | Value::Deferred(_)) && value.bytes() <= 4096)
            .then(|| Self::new(value))
    }
    pub fn parse(&self) -> Result<Value, String> {
        self.kind.parse(&self.input)
    }
    /// The type picker, and an editor for the value unless it is NULL.
    pub fn form(&mut self, ui: &mut egui::Ui, id: &str) {
        self.type_picker(ui, id, true);
        if self.kind != CellKind::Null {
            ui.add(
                egui::TextEdit::multiline(&mut self.input)
                    .desired_rows(3)
                    .desired_width(f32::INFINITY),
            );
        }
    }
    pub fn type_picker(
        &mut self,
        ui: &mut egui::Ui,
        id: impl std::hash::Hash + std::fmt::Debug,
        blobs: bool,
    ) {
        egui::ComboBox::from_id_salt(id)
            .selected_text(self.kind.label())
            .show_ui(ui, |ui| {
                for (kind, label) in CellKind::ALL {
                    if blobs || kind != CellKind::Blob {
                        ui.selectable_value(&mut self.kind, kind, label);
                    }
                }
            });
    }
}
pub(super) fn code_editor(
    text: &mut dyn egui::TextBuffer,
    width: f32,
    rows: usize,
) -> egui::TextEdit<'_> {
    egui::TextEdit::multiline(text)
        .code_editor()
        .desired_width(width)
        .desired_rows(rows)
}
pub(super) fn code_view(ui: &mut egui::Ui, mut text: &str, rows: usize) {
    ui.add(code_editor(&mut text, ui.available_width(), rows));
}

pub(super) fn detail_text(value: &Value) -> String {
    let Value::Text(text) = value else {
        return export::cell_text(value);
    };
    if !matches!(
        serde_json::from_str::<serde_json::Value>(text),
        Ok(serde_json::Value::Object(_) | serde_json::Value::Array(_))
    ) {
        return text.clone();
    }
    let mut indented = Indenter::default();
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if indented.quoted {
            indented.string_char(c);
        } else {
            indented.structural(c, &mut chars);
        }
        if indented.out.len() > 128 * 1024 {
            return text.clone();
        }
    }
    indented.out
}
/// JSON re-indented token by token, so original numeric/string tokens and
/// duplicate object keys stay exactly as stored.
#[derive(Default)]
struct Indenter {
    out: String,
    depth: usize,
    /// Inside a string token, and just after a backslash in one.
    quoted: bool,
    escaped: bool,
    /// The last structural character, to keep empty containers on one line.
    previous: char,
}
impl Indenter {
    fn newline(&mut self) {
        self.out.push('\n');
        self.out.extend(std::iter::repeat_n(' ', self.depth * 2));
    }
    fn string_char(&mut self, c: char) {
        self.out.push(c);
        if self.escaped {
            self.escaped = false;
        } else if c == '\\' {
            self.escaped = true;
        } else if c == '"' {
            self.quoted = false;
        }
    }
    /// Brackets indent, commas break the line, and whitespace between tokens is dropped.
    fn structural(&mut self, c: char, chars: &mut std::iter::Peekable<std::str::Chars>) {
        match c {
            '"' => {
                self.out.push(c);
                self.quoted = true;
            }
            '{' | '[' => {
                self.out.push(c);
                self.depth += 1;
                while chars.next_if(|c| c.is_whitespace()).is_some() {}
                if !chars.peek().is_some_and(|c| matches!(c, '}' | ']')) {
                    self.newline();
                }
            }
            '}' | ']' => {
                self.depth = self.depth.saturating_sub(1);
                if !matches!(self.previous, '{' | '[') {
                    self.newline();
                }
                self.out.push(c);
            }
            ',' => {
                self.out.push(c);
                self.newline();
            }
            ':' => self.out.push_str(": "),
            c if c.is_whitespace() => return,
            _ => self.out.push(c),
        }
        self.previous = c;
    }
}
impl App {
    pub(super) fn record_history(&mut self, ticket: Ticket, elapsed_ms: Option<u64>, failed: bool) {
        if let Some((database, sql)) = self.database.tools.pending_history.remove(&ticket.request) {
            self.preferences
                .record_query(&database, &sql, elapsed_ms, failed);
            self.store.save(&self.preferences);
        }
    }
    pub(super) fn copy_results(&mut self, ctx: &egui::Context, format: Format) {
        if self.database.document.active.is_some()
            || self.database.document.result.completion.is_none()
        {
            self.database.status = "Wait for retained results before copying".into();
            return;
        }
        match export::clipboard(
            format,
            self.export_scope(),
            &self.database.document.result.columns,
            &self.database.document.result.rows,
            &self.database.document.tools.export_table,
        ) {
            Ok(text) => {
                ctx.copy_text(text);
                self.database.status =
                    "Copied retained engine rows with scope metadata (not staged overlays)".into();
            }
            Err(e) => self.database.status = e,
        }
    }
    pub(super) fn copy_cell(&self, ctx: &egui::Context, row: usize, column: usize) {
        let rows = &self.database.document.result.rows;
        if let Some(value) = rows.get(row).and_then(|values| values.get(column)) {
            ctx.copy_text(export::cell_text(value));
        }
    }
    pub(super) fn copy_row(&mut self, ctx: &egui::Context, row: usize) {
        let Some(values) = self.database.document.result.rows.get(row) else {
            return;
        };
        match export::clipboard(
            Format::Tsv,
            "single-retained-row",
            &self.database.document.result.columns,
            std::slice::from_ref(values),
            &self.database.document.tools.export_table,
        ) {
            Ok(text) => ctx.copy_text(text),
            Err(e) => self.database.status = e,
        }
    }
    pub(super) fn open_cell(&mut self, row: usize, column: usize) {
        if let Some(value) = self
            .database
            .document
            .result
            .rows
            .get(row)
            .and_then(|r| r.get(column))
        {
            if self.database.document.inspect != Some((row, column)) {
                self.database.document.tools.record_scroll_to = Some(column);
            }
            if self.database.document.view == super::View::Browse
                && let Some(table) = &self.database.document.table
                && let Some(header) = self.database.document.result.columns.get(column)
            {
                self.database.document.tools.highlight_column =
                    Some((table.name.clone(), header.name.clone()));
            }
            self.database.document.inspect = Some((row, column));
            self.database.document.ui.inspector = shell::DetailMode::Cell;
            self.database.document.ui.inspector_open = true;
            self.database.document.tools.cell_editor = CellEditor::new(value);
        }
    }
}
#[derive(Default)]
pub(super) struct Viewer {
    cell: Option<DeferredCell>,
    pub ticket: Option<Ticket>,
    offset: u64,
    data: Vec<u8>,
    path: String,
    search: String,
    message: String,
}
impl Viewer {
    /// A chunk of the value from a chosen offset, a search within it, and
    /// saving the whole value; returns whether to load the chunk or save.
    fn contents(&mut self, ui: &mut egui::Ui, cell: &DeferredCell) -> (bool, bool) {
        let idle = self.ticket.is_none();
        ui.label(format!(
            "{:?} · {} bytes · immutable result snapshot",
            cell.kind, cell.bytes
        ));
        let load = ui.horizontal(|ui| {
            ui.label("Byte offset");
            ui.add(egui::DragValue::new(&mut self.offset).range(0..=cell.bytes));
            ui.add_enabled(idle, egui::Button::new("Load 16 KiB"))
                .clicked()
        });
        let text = if cell.kind == LargeKind::Text {
            String::from_utf8_lossy(&self.data).into_owned()
        } else {
            export::hex(&self.data)
        };
        ui.label("Find in this chunk");
        ui.text_edit_singleline(&mut self.search);
        if !self.search.is_empty() {
            ui.label(text.find(&self.search).map_or_else(
                || "No match in chunk".into(),
                |i| format!("Match at preview position {i}"),
            ));
        }
        code_view(ui, &text, 12);
        ui.small("Chunk boundaries may split a UTF-8 character. Saving preserves the exact original bytes.");
        ui.text_edit_singleline(&mut self.path);
        let savable = idle && !self.path.is_empty() && cell.handle.is_some();
        let save = ui.add_enabled(savable, egui::Button::new("Save full value to new file"));
        if !idle {
            ui.spinner();
        }
        if !self.message.is_empty() {
            ui.label(&self.message);
        }
        (load.inner, save.clicked())
    }
}
impl App {
    pub(super) fn open_large_value(&mut self, cell: DeferredCell) {
        self.database.document.tools.large = Viewer {
            cell: Some(cell),
            ..Default::default()
        };
        self.load_value_chunk();
    }
    fn value_request(&mut self, operation: Operation) {
        if self.database.document.tools.large.ticket.is_some() {
            return;
        }
        let outcome = self.database.request(operation);
        let large = &mut self.database.document.tools.large;
        match outcome {
            Some(Ok(ticket)) => large.ticket = Some(ticket),
            Some(Err(e)) => large.message = e.to_string(),
            None => large.message = "The source session is closed".into(),
        }
    }
    fn load_value_chunk(&mut self) {
        let viewer = &self.database.document.tools.large;
        if let Some(handle) = viewer.cell.as_ref().and_then(|c| c.handle) {
            self.value_request(Operation::ReadValue {
                handle,
                offset: viewer.offset,
                bytes: 16 * 1024,
            });
        } else {
            self.database.document.tools.large.message =
                "Full value unavailable: it exceeded the spool limit or temporary storage failed"
                    .into();
        }
    }
    pub(super) fn value_event(&mut self, kind: EventKind) {
        let viewer = &mut self.database.document.tools.large;
        match kind {
            EventKind::ValueChunk {
                handle,
                offset,
                bytes,
                ..
            } if viewer
                .cell
                .as_ref()
                .is_some_and(|c| c.handle == Some(handle)) =>
            {
                viewer.offset = offset;
                viewer.data = bytes;
                viewer.message.clear();
            }
            EventKind::ValueSaved(path) => viewer.message = format!("Saved full value to {path}"),
            EventKind::Failed(e) => viewer.message = e.to_string(),
            _ => return,
        }
        viewer.ticket = None;
    }
    pub(super) fn large_value_ui(&mut self, ctx: &egui::Context) {
        let viewer = &mut self.database.document.tools.large;
        let Some(cell) = viewer.cell.take() else {
            return;
        };
        let mut open = true;
        let (mut load, mut save) = (false, false);
        egui::Window::new("Full value")
            .id(egui::Id::unique(("large-value", self.database.document.id)))
            .open(&mut open)
            .default_width(660.0)
            .vscroll(true)
            .show(ctx, |ui| (load, save) = viewer.contents(ui, &cell));
        // Closing the window forgets the value.
        if !open {
            return;
        }
        let handle = cell.handle;
        viewer.cell = Some(cell);
        if load {
            self.load_value_chunk();
        }
        if save && let Some(handle) = handle {
            self.value_request(Operation::SaveValue {
                handle,
                path: self.database.document.tools.large.path.clone().into(),
            });
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{
        CellEditor, CellKind, DeferredCell, EventKind, LargeKind, Value, detail_text, egui, export,
    };
    use crate::app::test_ui;
    use tursobench_core::{Column, Failure};
    #[test]
    fn json_detail_preserves_raw_numbers_duplicate_keys_and_string_tokens() {
        let raw = r#"{"n":123456789012345678901,"n":1,"s":"a\\\"b","empty":[]}"#;
        let formatted = detail_text(&Value::Text(raw.into()));
        assert!(formatted.contains("123456789012345678901"));
        assert_eq!(formatted.matches("\"n\"").count(), 2);
        assert!(formatted.contains("\n"));
        assert!(serde_json::from_str::<serde_json::Value>(&formatted).is_ok());
        assert_eq!(export::cell_text(&Value::Text(raw.into())), raw);
    }
    #[test]
    fn cell_form_keeps_explicit_types() {
        for value in [
            Value::Null,
            Value::Integer(i64::MIN),
            Value::Real(-1.25),
            Value::Text("001λ".into()),
            Value::Blob(vec![0, 255]),
        ] {
            let editor = CellEditor::new(&value);
            assert_eq!(editor.parse().unwrap(), value);
            let inline = !matches!(value, Value::Blob(_) | Value::Deferred(_));
            assert_eq!(CellEditor::inline(&value).is_some(), inline);
        }
        let overhead = Value::Text(String::new()).bytes();
        for (len, allowed) in [(4096 - overhead, true), (4097 - overhead, false)] {
            let text = Value::Text("a".repeat(len));
            assert_eq!(CellEditor::inline(&text).is_some(), allowed);
        }
        for (kind, input, expected) in [
            (CellKind::Text, "001", Value::Text("001".into())),
            (CellKind::Blob, "00 ff", Value::Blob(vec![0, 255])),
            (CellKind::Null, "ignored", Value::Null),
        ] {
            assert_eq!(kind.parse(input).unwrap(), expected);
        }
        for (kind, input) in [
            (CellKind::Blob, "λ"),
            (CellKind::Blob, "0"),
            (CellKind::Integer, "9223372036854775808"),
            (CellKind::Real, "NaN"),
            (CellKind::Real, "inf"),
        ] {
            assert!(kind.parse(input).is_err(), "{input}");
        }
    }
    #[test]
    fn copies_retained_rows_and_cells_and_ignores_rows_out_of_range() {
        let (ctx, mut app) = test_ui::app();
        let result = &mut app.database.document.result;
        result.columns = ["n", "label"]
            .map(|name| Column {
                name: name.into(),
                declared_type: None,
            })
            .into();
        result.rows = vec![vec![Value::Integer(7), Value::Text("tab\there".into())]];
        let output = test_ui::render(&ctx, egui::vec2(200.0, 100.0), vec![], |ui| {
            app.copy_row(ui.ctx(), 0);
            app.copy_cell(ui.ctx(), 0, 1);
            app.copy_row(ui.ctx(), 1);
            app.copy_cell(ui.ctx(), 0, 2);
        });
        let copied: Vec<_> = output
            .platform_output
            .commands
            .iter()
            .filter_map(|command| match command {
                egui::OutputCommand::CopyText(text) => Some(text.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(copied.len(), 2, "{copied:?}");
        let row: Vec<_> = copied[0].lines().collect();
        assert_eq!(row, ["\"n\"\t\"label\"", "\"7\"\t\"tab\there\""]);
        assert_eq!(copied[1], "tab\there");
    }
    #[test]
    fn large_values_load_by_handle_and_report_chunks_saves_and_failures() {
        let (ctx, mut app) = test_ui::app();
        let cell = |handle| DeferredCell {
            handle,
            kind: LargeKind::Text,
            bytes: 40_000,
            head: vec![],
        };
        app.open_large_value(cell(None));
        let message = |app: &crate::app::App| app.database.document.tools.large.message.clone();
        assert!(message(&app).starts_with("Full value unavailable"));
        app.open_large_value(cell(Some(7)));
        assert_eq!(message(&app), "The source session is closed");
        let chunk = |handle| EventKind::ValueChunk {
            handle,
            offset: 16,
            bytes: b"a chunk".to_vec(),
            total: 40_000,
        };
        app.value_event(chunk(8));
        assert!(app.database.document.tools.large.data.is_empty());
        app.value_event(chunk(7));
        let viewer = &app.database.document.tools.large;
        let loaded = (viewer.offset, &viewer.data[..], viewer.message.as_str());
        assert_eq!(loaded, (16, &b"a chunk"[..], ""));
        app.database.document.tools.large.search = "chunk".into();
        let size = egui::vec2(800.0, 600.0);
        let mut draw = |ui: &mut egui::Ui| app.large_value_ui(ui.ctx());
        test_ui::render(&ctx, size, vec![], &mut draw);
        let output = test_ui::render(&ctx, size, vec![], draw);
        let found = "Match at preview position 2";
        assert!(test_ui::texts(&output).any(|t| t.galley.text() == found));
        app.value_event(EventKind::ValueSaved("/tmp/value".into()));
        assert_eq!(message(&app), "Saved full value to /tmp/value");
        app.value_event(EventKind::Failed(Failure::input("disk full")));
        assert!(message(&app).contains("disk full"));
    }
}
