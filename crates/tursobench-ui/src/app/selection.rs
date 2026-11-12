use super::{App, results::CellEditor, shell};
use eframe::egui;
use tursobench_core::{Ticket, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Selection {
    pub ticket: Option<Ticket>,
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}
pub(super) struct SummaryCache {
    selection: Selection,
    visible: Vec<usize>,
    rows: usize,
    text: String,
}
pub(super) struct InlineEdit {
    pub row: usize,
    pub column: usize,
    pub editor: CellEditor,
    pub focus: bool,
}
impl InlineEdit {
    pub fn submitted(
        &mut self,
        ui: &mut egui::Ui,
        response: &egui::Response,
    ) -> Option<Result<Value, String>> {
        if std::mem::take(&mut self.focus) {
            response.request_focus();
        }
        ((response.has_focus() || response.lost_focus())
            && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)))
        .then(|| self.editor.parse())
    }
}
impl App {
    pub(super) fn select_cell(&mut self, row: usize, column: usize, extend: bool) {
        if self
            .database
            .document
            .result
            .rows
            .get(row)
            .and_then(|r| r.get(column))
            .is_none()
        {
            return;
        }
        let anchor = self
            .database
            .document
            .tools
            .grid_selection
            .filter(|s| extend && s.ticket == self.database.document.result.current)
            .map_or((row, column), |s| s.anchor);
        self.database.document.tools.grid_selection = Some(Selection {
            ticket: self.database.document.result.current,
            anchor,
            cursor: (row, column),
        });
        let open = self.database.document.ui.inspector_open;
        self.open_cell(row, column);
        self.database.document.ui.inspector_open = open;
        self.database.document.tools.detail = Some(row);
        self.database.document.ui.inspector = shell::DetailMode::Record;
    }
    fn selection_bounds(&self) -> Option<(std::ops::Range<usize>, &[usize])> {
        let s = self
            .database
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.database.document.result.current)?;
        let rows = s.anchor.0.min(s.cursor.0)
            ..s.anchor
                .0
                .max(s.cursor.0)
                .saturating_add(1)
                .min(self.database.document.result.rows.len());
        if rows.is_empty() {
            return None;
        }
        let columns = &self.database.document.tools.columns;
        let position = |column| {
            (column < self.database.document.result.columns.len()
                && !columns.hidden.contains(&column))
            .then(|| columns.order.iter().position(|&c| c == column))
            .flatten()
        };
        let (a, b) = (position(s.anchor.1)?, position(s.cursor.1)?);
        Some((rows, &columns.order[a.min(b)..=a.max(b)]))
    }
    pub(super) fn cell_selected(&self, row: usize, column: usize) -> bool {
        self.selection_bounds().is_some_and(|(rows, cols)| {
            rows.contains(&row)
                && cols.contains(&column)
                && column < self.database.document.result.columns.len()
                && !self
                    .database
                    .document
                    .tools
                    .columns
                    .hidden
                    .contains(&column)
        })
    }
    pub(super) fn selected_cells(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        self.selection_bounds()
            .into_iter()
            .flat_map(move |(rows, columns)| {
                rows.flat_map(move |row| {
                    columns
                        .iter()
                        .copied()
                        .filter(move |c| {
                            *c < self.database.document.result.columns.len()
                                && !self.database.document.tools.columns.hidden.contains(c)
                        })
                        .map(move |c| (row, c))
                })
            })
    }
    pub(super) fn selection_tsv(&self) -> Result<String, String> {
        let mut text = String::new();
        let mut last = None;
        for (r, c) in self.selected_cells() {
            if let Some(previous) = last {
                text.push(if previous == r { '\t' } else { '\n' });
            }
            last = Some(r);
            if matches!(self.database.document.result.rows[r][c], Value::Deferred(_)) {
                return Err(
                    "Selection contains deferred cells; save the full value instead".into(),
                );
            }
            let value = &self.database.document.result.rows[r][c];
            let cell = match value {
                Value::Null => "\\N".into(),
                Value::Blob(bytes) => crate::export::hex(bytes),
                _ => crate::export::cell_text(value),
            };
            if cell.contains(['\t', '\n', '\r', '"'])
                || matches!(value, Value::Text(text) if text.is_empty() || text == "\\N")
            {
                text.push('"');
                text.push_str(&cell.replace('"', "\"\""));
                text.push('"');
            } else {
                text.push_str(&cell);
            }
            if text.len() > 2 * 1024 * 1024 {
                return Err("Selection exceeds 2 MiB clipboard budget; export instead".into());
            }
        }
        Ok(text)
    }
    pub(super) fn selection_summary(&mut self) -> &str {
        let Some(selection) = self
            .database
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.database.document.result.current)
        else {
            return "";
        };
        let rows = self.database.document.result.rows.len();
        let cached = self
            .database
            .document
            .tools
            .summary_cache
            .as_ref()
            .is_some_and(|c| {
                c.selection == selection
                    && c.rows == rows
                    && c.visible.iter().copied().eq(self
                        .database
                        .document
                        .tools
                        .columns
                        .visible(self.database.document.result.columns.len()))
            });
        if !cached {
            let text = summarize(
                self.selected_cells()
                    .filter_map(|(r, c)| self.database.document.result.rows.get(r)?.get(c)),
            );
            self.database.document.tools.summary_cache = Some(SummaryCache {
                selection,
                visible: self.visible_columns(),
                rows,
                text,
            });
        }
        self.database
            .document
            .tools
            .summary_cache
            .as_ref()
            .map_or("", |c| c.text.as_str())
    }
    pub(super) fn start_inline(&mut self, row: usize, column: usize) {
        if !self.can_edit() {
            self.open_cell(row, column);
            return;
        }
        let Some(original) = self.database.document.result.rows.get(row) else {
            return;
        };
        let Some(value) = self
            .database
            .document
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(original, column))
            .or_else(|| original.get(column))
        else {
            return;
        };
        let Some(editor) = CellEditor::inline(value) else {
            self.open_cell(row, column);
            return;
        };
        self.database.document.tools.inline = Some(InlineEdit {
            row,
            column,
            editor,
            focus: true,
        });
    }

    pub(super) fn grid_keys(&mut self, ctx: &egui::Context) {
        if self.database.document.tools.inline.is_some()
            || self.database.document.tools.spreadsheet.dialog.is_some()
            || !matches!(
                self.database.document.ui.nav,
                shell::Nav::Table | shell::Nav::Result
            )
            || self.database.document.ui.table_view != shell::TableView::Data
            || egui::Popup::is_any_open(ctx)
            || self.database.ui.palette_open
        {
            return;
        }
        if self.database.document.tools.grid_focus.is_none()
            || ctx.memory(|m| m.focused()) != self.database.document.tools.grid_focus
        {
            return;
        }
        // Native integrations send clipboard events instead of Ctrl+C/V key events.
        let clipboard = ctx.input_mut(|i| {
            let mut paste = None;
            let mut copy = false;
            i.events.retain(|event| match event {
                egui::Event::Paste(text) => {
                    paste = Some(text.clone());
                    false
                }
                egui::Event::Copy => {
                    copy = true;
                    false
                }
                _ => true,
            });
            (paste, copy)
        });
        if let Some(text) = clipboard.0 {
            if ctx.input(|i| i.modifiers.shift) {
                self.open_paste(true, text);
            } else {
                self.paste_cells(&text);
            }
            return;
        }
        if clipboard.1 {
            match self.selection_tsv() {
                Ok(text) => ctx.copy_text(text),
                Err(error) => self.database.status = error,
            }
            return;
        }
        if ctx.input_mut(|i| {
            i.consume_key(egui::Modifiers::CTRL | egui::Modifiers::SHIFT, egui::Key::V)
        }) {
            self.open_paste(true, String::new());
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::Z)) {
            self.undo_grid_edit();
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)) {
            self.fill_selection(Value::Null);
            return;
        }
        let Some(s) = self
            .database
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.database.document.result.current)
        else {
            return;
        };
        let cols = self.visible_columns();
        let Some(c) = cols.iter().position(|&c| c == s.cursor.1) else {
            return;
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::C)) {
            match self.selection_tsv() {
                Ok(text) => ctx.copy_text(text),
                Err(error) => self.database.status = error,
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            let keys = self.cell_references(s.cursor.0, s.cursor.1);
            if keys.len() == 1 {
                self.follow_key(s.cursor.0, (*keys[0]).clone());
            } else {
                self.start_inline(s.cursor.0, s.cursor.1);
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Space)) {
            self.toggle_inspector();
            return;
        }
        let shift = ctx.input(|i| i.modifiers.shift);
        let mods = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        for (key, dr, dc) in [
            (egui::Key::ArrowUp, -1, 0),
            (egui::Key::ArrowDown, 1, 0),
            (egui::Key::ArrowLeft, 0, -1),
            (egui::Key::ArrowRight, 0, 1),
            (egui::Key::PageDown, 25, 0),
            (egui::Key::PageUp, -25, 0),
        ] {
            if ctx.input_mut(|i| i.consume_key(mods, key)) {
                let row = s
                    .cursor
                    .0
                    .saturating_add_signed(dr)
                    .min(self.database.document.result.rows.len().saturating_sub(1));
                let col = c.saturating_add_signed(dc).min(cols.len() - 1);
                if key == egui::Key::PageDown && row == s.cursor.0 {
                    self.request_page(false);
                }
                if key == egui::Key::PageUp && row == 0 {
                    self.request_page(true);
                }
                self.select_cell(row, cols[col], shift);
                self.database.document.tools.scroll_to_cell = true;
                let stride = self
                    .database
                    .document
                    .tools
                    .row_stride
                    .max(super::grid::ROW_HEIGHT);
                let y = row as f32 * stride;
                let current = self.database.document.tools.grid_scroll.y;
                let next = if y < current {
                    y
                } else if y + stride > current + self.database.document.tools.grid_height {
                    (y + stride - self.database.document.tools.grid_height).max(0.0)
                } else {
                    current
                };
                if next != current {
                    self.database.document.tools.restore_scroll =
                        Some(egui::vec2(self.database.document.tools.grid_scroll.x, next));
                }
                break;
            }
        }
    }
}

/// Integer sums/extrema stay exact; mixed-real aggregates are explicitly approximate.
fn summarize<'a>(values: impl IntoIterator<Item = &'a Value>) -> String {
    let (mut cells, mut numeric, mut real) = (0usize, 0usize, false);
    let mut exact = Some((0i128, i64::MAX, i64::MIN));
    let (mut sum, mut min, mut max) = (0.0, f64::INFINITY, f64::NEG_INFINITY);
    for value in values {
        cells += 1;
        let number = match value {
            Value::Integer(n) => {
                exact = exact.and_then(|(sum, min, max)| {
                    sum.checked_add(i128::from(*n))
                        .map(|sum| (sum, min.min(*n), max.max(*n)))
                });
                *n as f64
            }
            Value::Real(n) => {
                real = true;
                *n
            }
            _ => continue,
        };
        numeric += 1;
        sum += number;
        min = min.min(number);
        max = max.max(number);
    }
    if cells == 0 {
        return String::new();
    }
    let prefix = format!("{cells} cells");
    if numeric == 0 {
        return prefix;
    }
    if !real {
        let Some((sum, min, max)) = exact else {
            return format!("{prefix} · sum overflow");
        };
        return format!(
            "{prefix} · {numeric} numeric · Σ {sum} · avg ≈{} · min {min} · max {max}",
            sum as f64 / numeric as f64
        );
    }
    if !sum.is_finite() {
        return format!("{prefix} · numeric aggregate nonfinite");
    }
    format!(
        "{prefix} · {numeric} numeric · Σ ≈{sum} · avg ≈{} · min ≈{min} · max ≈{max}",
        sum / numeric as f64
    )
}

#[cfg(test)]
mod tests {
    use super::summarize;
    use tursobench_core::Value::{Integer, Null, Real, Text};
    #[test]
    fn aggregates_preserve_integer_precision_and_handle_empty_mixed_and_nonfinite_values() {
        for (values, expected) in [
            (vec![], String::new()),
            (vec![Text("text".into()), Null], "2 cells".into()),
            (vec![Integer(i64::MAX), Integer(i64::MIN)], format!("2 cells · 2 numeric · Σ -1 · avg ≈-0.5 · min {} · max {}", i64::MIN, i64::MAX)),
            (vec![Integer(9007199254740993), Integer(9007199254740995)], "2 cells · 2 numeric · Σ 18014398509481988 · avg ≈9007199254740994 · min 9007199254740993 · max 9007199254740995".into()),
            (vec![Null, Integer(2), Real(1.0)], "3 cells · 2 numeric · Σ ≈3 · avg ≈1.5 · min ≈1 · max ≈2".into()),
            (vec![Real(f64::NAN)], "1 cells · numeric aggregate nonfinite".into()),
            (vec![Real(f64::INFINITY)], "1 cells · numeric aggregate nonfinite".into()),
            (vec![Real(f64::MAX), Real(f64::MAX)], "2 cells · numeric aggregate nonfinite".into()),
        ] { assert_eq!(summarize(&values), expected, "{values:?}"); }
    }
}
