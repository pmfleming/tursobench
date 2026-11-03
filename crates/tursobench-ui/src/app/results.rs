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
pub(super) fn code_view(ui: &mut egui::Ui, mut text: &str, rows: usize) {
    ui.add(
        egui::TextEdit::multiline(&mut text)
            .code_editor()
            .desired_width(ui.available_width())
            .desired_rows(rows),
    );
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
        if let Some((database, sql)) = self.tools.pending_history.remove(&ticket.request) {
            self.preferences
                .record_query(&database, &sql, elapsed_ms, failed);
            self.store.save(&self.preferences);
        }
    }
    pub(super) fn copy_results(&mut self, ctx: &egui::Context, format: Format) {
        if self.document.active.is_some() || self.document.result.completion.is_none() {
            self.status = "Wait for retained results before copying".into();
            return;
        }
        match export::clipboard(
            format,
            self.export_scope(),
            &self.document.result.columns,
            &self.document.result.rows,
            &self.document.tools.export_table,
        ) {
            Ok(text) => {
                ctx.copy_text(text);
                self.status =
                    "Copied retained engine rows with scope metadata (not staged overlays)".into();
            }
            Err(e) => self.status = e,
        }
    }
    pub(super) fn copy_row(&mut self, ctx: &egui::Context, row: usize) {
        let Some(values) = self.document.result.rows.get(row) else {
            return;
        };
        match export::clipboard(
            Format::Tsv,
            "single-retained-row",
            &self.document.result.columns,
            std::slice::from_ref(values),
            &self.document.tools.export_table,
        ) {
            Ok(text) => ctx.copy_text(text),
            Err(e) => self.status = e,
        }
    }
    pub(super) fn open_cell(&mut self, row: usize, column: usize) {
        if let Some(value) = self
            .document
            .result
            .rows
            .get(row)
            .and_then(|r| r.get(column))
        {
            self.document.inspect = Some((row, column));
            self.document.ui.inspector = shell::DetailMode::Cell;
            self.document.ui.inspector_open = true;
            self.document.tools.cell_kind = match value {
                Value::Null => CellKind::Null,
                Value::Integer(_) => CellKind::Integer,
                Value::Real(_) => CellKind::Real,
                Value::Text(_) => CellKind::Text,
                Value::Blob(_) => CellKind::Blob,
            };
            self.document.tools.cell_input = match value {
                Value::Null => String::new(),
                Value::Blob(b) => export::hex(b),
                _ => export::cell_text(value),
            };
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
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
        assert_eq!(
            CellKind::Text.parse("001").unwrap(),
            Value::Text("001".into())
        );
        assert_eq!(
            CellKind::Blob.parse("00 ff").unwrap(),
            Value::Blob(vec![0, 255])
        );
        assert!(CellKind::Blob.parse("λ").is_err());
        assert!(CellKind::Integer.parse("9223372036854775808").is_err());
        assert!(CellKind::Real.parse("NaN").is_err());
    }
}
