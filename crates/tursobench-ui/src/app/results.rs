//! Result value editors, bounded value viewing, and clipboard/export presentation.
use super::{App, shell};
use eframe::egui;
use tursobench_core::{DeferredCell, EventKind, LargeKind, Ticket, Value};
use tursobench_db::{Operation, Request};

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
    pub(super) fn label(self) -> &'static str {
        match self {
            Self::Null => "NULL",
            Self::Integer => "Integer",
            Self::Real => "Real",
            Self::Text => "Text",
            Self::Blob => "Blob (hex)",
        }
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
            kind: match value {
                Value::Null => CellKind::Null,
                Value::Integer(_) => CellKind::Integer,
                Value::Real(_) => CellKind::Real,
                Value::Text(_) => CellKind::Text,
                Value::Blob(_) => CellKind::Blob,
                Value::Deferred(_) => CellKind::Text,
            },
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
    pub fn type_picker(
        &mut self,
        ui: &mut egui::Ui,
        id: impl std::hash::Hash + std::fmt::Debug,
        blobs: bool,
    ) {
        egui::ComboBox::from_id_salt(id)
            .selected_text(self.kind.label())
            .show_ui(ui, |ui| {
                for kind in [
                    CellKind::Null,
                    CellKind::Integer,
                    CellKind::Real,
                    CellKind::Text,
                    CellKind::Blob,
                ]
                .into_iter()
                .filter(|k| blobs || *k != CellKind::Blob)
                {
                    ui.selectable_value(&mut self.kind, kind, kind.label());
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
    // Preserve original numeric/string tokens and duplicate object keys.
    let mut out = String::new();
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    let mut previous = ' ';
    let newline = |out: &mut String, depth: usize| {
        out.push('\n');
        out.extend(std::iter::repeat_n(' ', depth * 2));
    };
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if quoted {
            out.push(c);
            if escaped {
                escaped = false;
            } else if c == '\\' {
                escaped = true;
            } else if c == '"' {
                quoted = false;
            }
        } else {
            match c {
                '"' => {
                    out.push(c);
                    quoted = true;
                }
                '{' | '[' => {
                    out.push(c);
                    depth += 1;
                    while chars.peek().is_some_and(|c| c.is_whitespace()) {
                        chars.next();
                    }
                    if !chars.peek().is_some_and(|c| *c == '}' || *c == ']') {
                        newline(&mut out, depth);
                    }
                }
                '}' | ']' => {
                    depth = depth.saturating_sub(1);
                    if previous != '{' && previous != '[' {
                        newline(&mut out, depth);
                    }
                    out.push(c);
                }
                ',' => {
                    out.push(c);
                    newline(&mut out, depth);
                }
                ':' => out.push_str(": "),
                c if c.is_whitespace() => continue,
                _ => out.push(c),
            }
            previous = c;
        }
        if out.len() > 128 * 1024 {
            return text.clone();
        }
    }
    out
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
#[cfg(test)]
mod tests {
    use super::{CellEditor, CellKind, Value, detail_text, export};
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
            assert_eq!(
                CellEditor::inline(&value).is_some(),
                !matches!(value, Value::Blob(_) | Value::Deferred(_))
            );
        }
        let overhead = Value::Text(String::new()).bytes();
        for (len, allowed) in [(4096 - overhead, true), (4097 - overhead, false)] {
            assert_eq!(
                CellEditor::inline(&Value::Text("a".repeat(len))).is_some(),
                allowed
            );
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
        let Some(session) = &self.database.session else {
            self.database.document.tools.large.message = "The source session is closed".into();
            return;
        };
        self.database.sequence += 1;
        let ticket = session.ticket(self.database.sequence, self.database.generation);
        match session.submit(Request { ticket, operation }) {
            Ok(()) => self.database.document.tools.large.ticket = Some(ticket),
            Err(e) => self.database.document.tools.large.message = e.to_string(),
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
        let Some(cell) = viewer.cell.clone() else {
            return;
        };
        let mut open = true;
        let mut load = false;
        let mut save = false;
        egui::Window::new("Full value").id(egui::Id::unique(("large-value",self.database.document.id))).open(&mut open).default_width(660.0).vscroll(true).show(ctx,|ui|{
            ui.label(format!("{:?} · {} bytes · immutable result snapshot",cell.kind,cell.bytes));
            ui.horizontal(|ui|{
                ui.label("Byte offset");
                ui.add(egui::DragValue::new(&mut viewer.offset).range(0..=cell.bytes));
                load=ui.add_enabled(viewer.ticket.is_none(),egui::Button::new("Load 16 KiB")).clicked();
            });
            let text=if cell.kind==LargeKind::Text {String::from_utf8_lossy(&viewer.data).into_owned()}else{crate::export::hex(&viewer.data)};
            ui.label("Find in this chunk");ui.text_edit_singleline(&mut viewer.search);
            if !viewer.search.is_empty(){ui.label(text.find(&viewer.search).map_or_else(||"No match in chunk".into(),|i|format!("Match at preview position {i}")));}
            code_view(ui,&text,12);
            ui.small("Chunk boundaries may split a UTF-8 character. Saving preserves the exact original bytes.");
            ui.text_edit_singleline(&mut viewer.path);
            save=ui.add_enabled(viewer.ticket.is_none()&&!viewer.path.is_empty()&&cell.handle.is_some(),egui::Button::new("Save full value to new file")).clicked();
            if viewer.ticket.is_some(){ui.spinner();}
            if !viewer.message.is_empty(){ui.label(&viewer.message);}
        });
        if !open {
            self.database.document.tools.large.cell = None;
            return;
        }
        if load {
            self.load_value_chunk();
        }
        if save && let Some(handle) = cell.handle {
            self.value_request(Operation::SaveValue {
                handle,
                path: self.database.document.tools.large.path.clone().into(),
            });
        }
    }
}
