use super::{
    App,
    results::{CellEditor, CellKind},
    shell,
};
use eframe::egui;
use tursobench_core::{Ticket, Value};

#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) struct Selection {
    pub ticket: Option<Ticket>,
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}
/// A key the grid handles, whether it acts on the cursor cell, and what it does.
type GridShortcut = (
    bool,
    egui::Modifiers,
    egui::Key,
    fn(&mut App, &egui::Context),
);
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
    /// A record field edited in place: its type and value, parsed on Enter.
    pub fn record_field(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        col: usize,
    ) -> Option<Result<Value, String>> {
        ui.scope_builder(
            egui::UiBuilder::new()
                .id_salt(("record-edit", col))
                .max_rect(rect)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| {
                ui.spacing_mut().interact_size = egui::vec2(40.0, 20.0);
                ui.spacing_mut().combo_width = 62.0;
                self.editor.type_picker(ui, "type", false);
                let hint = if self.editor.kind == CellKind::Null {
                    "NULL"
                } else {
                    ""
                };
                let response = ui.add_sized(
                    [ui.available_width(), 20.0],
                    egui::TextEdit::singleline(&mut self.editor.input).hint_text(hint),
                );
                self.submitted(ui, &response)
            },
        )
        .inner
    }
}
impl App {
    /// The grid selection, while it still belongs to the retained result.
    pub(super) fn current_selection(&self) -> Option<Selection> {
        let document = &self.database.document;
        let selection = document.tools.grid_selection;
        selection.filter(|s| s.ticket == document.result.current)
    }
    pub(super) fn select_cell(&mut self, row: usize, column: usize, extend: bool) {
        let rows = &self.database.document.result.rows;
        if rows.get(row).and_then(|r| r.get(column)).is_none() {
            return;
        }
        let anchor = self
            .current_selection()
            .filter(|_| extend)
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
        let s = self.current_selection()?;
        let document = &self.database.document;
        let last = s.anchor.0.max(s.cursor.0).saturating_add(1);
        let rows = s.anchor.0.min(s.cursor.0)..last.min(document.result.rows.len());
        if rows.is_empty() {
            return None;
        }
        let columns = &document.tools.columns;
        let position = |column| {
            columns
                .shown(column, document.result.columns.len())
                .then(|| columns.order.iter().position(|&c| c == column))
                .flatten()
        };
        let (a, b) = (position(s.anchor.1)?, position(s.cursor.1)?);
        Some((rows, &columns.order[a.min(b)..=a.max(b)]))
    }
    pub(super) fn cell_selected(&self, row: usize, column: usize) -> bool {
        let document = &self.database.document;
        let shown = document
            .tools
            .columns
            .shown(column, document.result.columns.len());
        shown
            && self
                .selection_bounds()
                .is_some_and(|(rows, cols)| rows.contains(&row) && cols.contains(&column))
    }
    pub(super) fn selected_cells(&self) -> impl Iterator<Item = (usize, usize)> + '_ {
        let document = &self.database.document;
        let count = document.result.columns.len();
        self.selection_bounds()
            .into_iter()
            .flat_map(move |(rows, columns)| {
                rows.flat_map(move |row| {
                    columns
                        .iter()
                        .copied()
                        .filter(move |&c| document.tools.columns.shown(c, count))
                        .map(move |c| (row, c))
                })
            })
    }
    pub(super) fn selection_tsv(&self) -> Result<String, String> {
        let rows = &self.database.document.result.rows;
        let mut text = String::new();
        let mut last = None;
        for (r, c) in self.selected_cells() {
            let value = &rows[r][c];
            if matches!(value, Value::Deferred(_)) {
                return Err(
                    "Selection contains deferred cells; save the full value instead".into(),
                );
            }
            if let Some(previous) = last {
                text.push(if previous == r { '\t' } else { '\n' });
            }
            last = Some(r);
            text.push_str(&tsv_field(value));
            if text.len() > 2 * 1024 * 1024 {
                return Err("Selection exceeds 2 MiB clipboard budget; export instead".into());
            }
        }
        Ok(text)
    }
    pub(super) fn selection_summary(&mut self) -> &str {
        let Some(selection) = self.current_selection() else {
            return "";
        };
        let document = &self.database.document;
        let rows = document.result.rows.len();
        let visible = document
            .tools
            .columns
            .visible(document.result.columns.len());
        let cached = document.tools.summary_cache.as_ref().is_some_and(|c| {
            c.selection == selection && c.rows == rows && c.visible.iter().copied().eq(visible)
        });
        if !cached {
            let values = self
                .selected_cells()
                .filter_map(|(r, c)| self.database.document.result.rows.get(r)?.get(c));
            let text = summarize(values);
            self.database.document.tools.summary_cache = Some(SummaryCache {
                selection,
                visible: self.visible_columns(),
                rows,
                text,
            });
        }
        let cache = self.database.document.tools.summary_cache.as_ref();
        cache.map_or("", |c| c.text.as_str())
    }
    /// Whether the cell is being edited in place.
    pub(super) fn editing_at(&self, row: usize, column: usize) -> bool {
        let inline = self.database.document.tools.inline.as_ref();
        inline.is_some_and(|e| e.row == row && e.column == column)
    }
    pub(super) fn start_inline(&mut self, row: usize, column: usize) {
        // A blob or deferred value opens in the record tile's value view.
        let rows = &self.database.document.result.rows;
        if let Some(Value::Blob(_) | Value::Deferred(_)) = rows.get(row).and_then(|r| r.get(column))
        {
            self.open_value(row, column);
            return;
        }
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

    /// Whether the focused grid of a table's data or a result takes the keys.
    fn grid_takes_keys(&self, ctx: &egui::Context) -> bool {
        let document = &self.database.document;
        document.tools.inline.is_none()
            && document.tools.spreadsheet.dialog.is_none()
            && matches!(document.ui.nav, shell::Nav::Table | shell::Nav::Result)
            && document.ui.table_view == shell::TableView::Data
            && !egui::Popup::is_any_open(ctx)
            && !self.database.ui.palette_open
            && document.tools.grid_focus.is_some()
            && ctx.memory(|m| m.focused()) == document.tools.grid_focus
    }
    pub(super) fn grid_keys(&mut self, ctx: &egui::Context) {
        if !self.grid_takes_keys(ctx) {
            return;
        }
        let (paste, copy) = take_clipboard_events(ctx);
        if let Some(text) = paste {
            if ctx.input(|i| i.modifiers.shift) {
                self.open_paste(true, text);
            } else {
                self.paste_cells(&text);
            }
            return;
        }
        if copy {
            self.copy_selection(ctx);
            return;
        }
        use egui::{Key, Modifiers};
        // Edits apply to the selection; the rest act on its cursor, in a shown column.
        let cursor = self.current_selection().map(|s| s.cursor);
        let cols = self.visible_columns();
        let c = cursor.and_then(|(_, col)| cols.iter().position(|&c| c == col));
        let shortcuts: [GridShortcut; 6] = [
            (
                false,
                Modifiers::CTRL | Modifiers::SHIFT,
                Key::V,
                |app, _| app.open_paste(true, String::new()),
            ),
            (false, Modifiers::CTRL, Key::Z, |app, _| {
                app.undo_grid_edit()
            }),
            (false, Modifiers::NONE, Key::Backspace, |app, _| {
                app.fill_selection(Value::Null);
            }),
            (true, Modifiers::CTRL, Key::C, Self::copy_selection),
            (true, Modifiers::NONE, Key::Enter, |app, _| {
                app.activate_cursor()
            }),
            (true, Modifiers::NONE, Key::Space, |app, _| {
                app.toggle_inspector()
            }),
        ];
        let pressed = shortcuts.iter().find(|(on_cursor, modifiers, key, _)| {
            (!on_cursor || c.is_some()) && ctx.input_mut(|i| i.consume_key(*modifiers, *key))
        });
        match (pressed, cursor, c) {
            (Some((.., act)), ..) => act(self, ctx),
            (None, Some((row, _)), Some(c)) => self.move_cursor(ctx, row, &cols, c),
            _ => {}
        }
    }
    /// Enter follows the cursor cell's only reference, or else edits the cell in place.
    fn activate_cursor(&mut self) {
        let Some((row, col)) = self.current_selection().map(|s| s.cursor) else {
            return;
        };
        let only = match self.cell_references(row, col).as_slice() {
            [key] => Some((*key).clone()),
            _ => None,
        };
        match only {
            Some(key) => self.follow_key(row, key),
            None => self.start_inline(row, col),
        }
    }
    fn copy_selection(&mut self, ctx: &egui::Context) {
        match self.selection_tsv() {
            Ok(text) => ctx.copy_text(text),
            Err(error) => self.database.status = error,
        }
    }
    /// Arrows move the cursor a cell and Page keys 25 rows, Shift extending the
    /// selection; paging past either end loads the neighbouring page.
    fn move_cursor(&mut self, ctx: &egui::Context, row: usize, cols: &[usize], c: usize) {
        let shift = ctx.input(|i| i.modifiers.shift);
        let mods = if shift {
            egui::Modifiers::SHIFT
        } else {
            egui::Modifiers::NONE
        };
        let Some((key, dr, dc)) = [
            (egui::Key::ArrowUp, -1, 0),
            (egui::Key::ArrowDown, 1, 0),
            (egui::Key::ArrowLeft, 0, -1),
            (egui::Key::ArrowRight, 0, 1),
            (egui::Key::PageDown, 25, 0),
            (egui::Key::PageUp, -25, 0),
        ]
        .into_iter()
        .find(|(key, _, _)| ctx.input_mut(|i| i.consume_key(mods, *key))) else {
            return;
        };
        let last = self.database.document.result.rows.len().saturating_sub(1);
        let next = row.saturating_add_signed(dr).min(last);
        let col = c.saturating_add_signed(dc).min(cols.len() - 1);
        if key == egui::Key::PageDown && next == row {
            self.request_page(false);
        }
        if key == egui::Key::PageUp && next == 0 {
            self.request_page(true);
        }
        self.select_cell(next, cols[col], shift);
        self.scroll_to_row(next);
    }
    /// Scroll the grid just enough to show `row`.
    fn scroll_to_row(&mut self, row: usize) {
        let tools = &mut self.database.document.tools;
        tools.scroll_to_cell = true;
        let stride = tools.row_stride.max(super::grid::ROW_HEIGHT);
        let y = row as f32 * stride;
        let current = tools.grid_scroll.y;
        let next = if y < current {
            y
        } else if y + stride > current + tools.grid_height {
            (y + stride - tools.grid_height).max(0.0)
        } else {
            current
        };
        if next != current {
            tools.restore_scroll = Some(egui::vec2(tools.grid_scroll.x, next));
        }
    }
}
/// A cell as a clipboard field: NULL as `\N` and blobs as hex, quoted when it
/// holds separators or quotes or would otherwise read back as NULL or nothing.
fn tsv_field(value: &Value) -> String {
    let cell = match value {
        Value::Null => "\\N".into(),
        Value::Blob(bytes) => crate::export::hex(bytes),
        _ => crate::export::cell_text(value),
    };
    let ambiguous = matches!(value, Value::Text(text) if text.is_empty() || text == "\\N");
    if ambiguous || cell.contains(['\t', '\n', '\r', '"']) {
        format!("\"{}\"", cell.replace('"', "\"\""))
    } else {
        cell
    }
}
/// Native integrations send clipboard events instead of Ctrl+C/V key events;
/// take them so nothing else handles them too.
fn take_clipboard_events(ctx: &egui::Context) -> (Option<String>, bool) {
    ctx.input_mut(|i| {
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
    })
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
