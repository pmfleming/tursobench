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
    selection: Option<Selection>,
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
            .document
            .tools
            .grid_selection
            .filter(|s| extend && s.ticket == self.document.result.current)
            .map_or((row, column), |s| s.anchor);
        self.document.tools.grid_selection = Some(Selection {
            ticket: self.document.result.current,
            anchor,
            cursor: (row, column),
        });
        let open = self.document.ui.inspector_open;
        self.open_cell(row, column);
        self.document.ui.inspector_open = open;
        self.document.tools.detail = Some(row);
        self.document.ui.inspector = shell::DetailMode::Record;
    }
    pub(super) fn cell_selected(&self, row: usize, column: usize) -> bool {
        let Some(s) = self
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.document.result.current)
        else {
            return false;
        };
        if row < s.anchor.0.min(s.cursor.0) || row > s.anchor.0.max(s.cursor.0) {
            return false;
        }
        let cols = self.visible_columns();
        let (Some(a), Some(b), Some(c)) = (
            cols.iter().position(|&c| c == s.anchor.1),
            cols.iter().position(|&c| c == s.cursor.1),
            cols.iter().position(|&c| c == column),
        ) else {
            return false;
        };
        (a.min(b)..=a.max(b)).contains(&c)
    }
    pub(super) fn selected_cells(&self) -> Vec<(usize, usize)> {
        let Some(s) = self
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.document.result.current)
        else {
            return vec![];
        };
        let cols = self.visible_columns();
        let (Some(a), Some(b)) = (
            cols.iter().position(|&c| c == s.anchor.1),
            cols.iter().position(|&c| c == s.cursor.1),
        ) else {
            return vec![];
        };
        (s.anchor.0.min(s.cursor.0)
            ..=s.anchor
                .0
                .max(s.cursor.0)
                .min(self.document.result.rows.len().saturating_sub(1)))
            .flat_map(|row| {
                cols[a.min(b)..=a.max(b)]
                    .iter()
                    .copied()
                    .map(move |c| (row, c))
            })
            .collect()
    }
    pub(super) fn selection_tsv(&self) -> Result<String, String> {
        let mut text = String::new();
        let mut last = None;
        for (r, c) in self.selected_cells() {
            if let Some(previous) = last {
                text.push(if previous == r { '\t' } else { '\n' });
            }
            last = Some(r);
            let cell = crate::export::cell_text(&self.document.result.rows[r][c]);
            if cell.contains(['\t', '\n', '\r', '"']) {
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
    pub(super) fn selection_summary(&mut self) -> String {
        let selection = self
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.document.result.current);
        if selection.is_none() {
            return String::new();
        }
        let visible = self.visible_columns();
        let rows = self.document.result.rows.len();
        if let Some(c) = &self.document.tools.summary_cache
            && c.selection == selection
            && c.visible == visible
            && c.rows == rows
        {
            return c.text.clone();
        }
        let text = self.compute_selection_summary();
        self.document.tools.summary_cache = Some(SummaryCache {
            selection,
            visible,
            rows,
            text: text.clone(),
        });
        text
    }
    fn compute_selection_summary(&self) -> String {
        let cells = self.selected_cells();
        if cells.is_empty() {
            return String::new();
        }
        let values: Vec<_> = cells
            .iter()
            .filter_map(|&(r, c)| self.document.result.rows.get(r)?.get(c))
            .collect();
        let integers: Vec<_> = values
            .iter()
            .filter_map(|v| {
                if let Value::Integer(n) = v {
                    Some(*n)
                } else {
                    None
                }
            })
            .collect();
        let reals: Vec<_> = values
            .iter()
            .filter_map(|v| {
                if let Value::Real(n) = v {
                    Some(*n)
                } else {
                    None
                }
            })
            .collect();
        let prefix = format!("{} cells", cells.len());
        if integers.is_empty() && reals.is_empty() {
            return prefix;
        }
        if reals.is_empty() {
            let sum: Option<i128> = integers
                .iter()
                .try_fold(0i128, |s, &n| s.checked_add(n as i128));
            let Some(sum) = sum else {
                return format!("{prefix} · sum overflow");
            };
            // Average is explicitly approximate; integer sums and extrema remain exact.
            format!(
                "{prefix} · {} numeric · Σ {sum} · avg ≈{} · min {} · max {}",
                integers.len(),
                sum as f64 / integers.len() as f64,
                integers.iter().min().unwrap(),
                integers.iter().max().unwrap()
            )
        } else {
            let numeric: Vec<_> = integers.iter().map(|&n| n as f64).chain(reals).collect();
            let sum: f64 = numeric.iter().sum();
            if !sum.is_finite() || numeric.iter().any(|n| !n.is_finite()) {
                return format!("{prefix} · numeric aggregate nonfinite");
            }
            format!(
                "{prefix} · {} numeric · Σ ≈{sum} · avg ≈{} · min ≈{} · max ≈{}",
                numeric.len(),
                sum / numeric.len() as f64,
                numeric.iter().copied().fold(f64::INFINITY, f64::min),
                numeric.iter().copied().fold(f64::NEG_INFINITY, f64::max)
            )
        }
    }
    pub(super) fn start_inline(&mut self, row: usize, column: usize) {
        if !self.can_edit() {
            self.open_cell(row, column);
            return;
        }
        let Some(original) = self.document.result.rows.get(row) else {
            return;
        };
        let Some(value) = self
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
        self.document.tools.inline = Some(InlineEdit {
            row,
            column,
            editor,
            focus: true,
        });
    }

    pub(super) fn grid_keys(&mut self, ctx: &egui::Context) {
        if self.document.tools.inline.is_some()
            || !matches!(self.document.ui.nav, shell::Nav::Table | shell::Nav::Result)
            || self.document.ui.table_view != shell::TableView::Data
            || egui::Popup::is_any_open(ctx)
            || self.ui.palette_open
        {
            return;
        }
        if self.document.tools.grid_focus.is_none()
            || ctx.memory(|m| m.focused()) != self.document.tools.grid_focus
        {
            return;
        }
        let Some(s) = self
            .document
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.document.result.current)
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
                Err(error) => self.status = error,
            }
            return;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter)) {
            let keys = self.cell_references(s.cursor.0, s.cursor.1);
            if keys.len() == 1 {
                self.follow_key(s.cursor.0, keys[0].clone());
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
                    .min(self.document.result.rows.len().saturating_sub(1));
                let col = c.saturating_add_signed(dc).min(cols.len() - 1);
                if key == egui::Key::PageDown && row == s.cursor.0 {
                    self.request_page(false);
                }
                if key == egui::Key::PageUp && row == 0 {
                    self.request_page(true);
                }
                self.select_cell(row, cols[col], shift);
                self.document.tools.scroll_to_cell = true;
                let y = row as f32 * 25.0;
                let current = self.document.tools.grid_scroll.y;
                let next = if y < current {
                    y
                } else if y + 25.0 > current + self.document.tools.grid_height {
                    (y + 25.0 - self.document.tools.grid_height).max(0.0)
                } else {
                    current
                };
                if next != current {
                    self.document.tools.restore_scroll =
                        Some(egui::vec2(self.document.tools.grid_scroll.x, next));
                }
                break;
            }
        }
    }
}
