//! Result value editors and clipboard/export presentation.
use super::{App, shell};
use eframe::egui;
use tursobench_core::{Ticket, Value};

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
#[cfg(test)]
mod tests {
    use super::{CellEditor, CellKind, Value, detail_text, egui, export};
    use crate::app::test_ui;
    use tursobench_core::Column;
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
}
