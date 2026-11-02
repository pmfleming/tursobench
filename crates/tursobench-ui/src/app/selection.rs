use super::{App, results::CellKind, shell};
use eframe::egui;
use tursobench_core::{Ticket, Value};

#[derive(Clone, Copy)]
pub(super) struct Selection {
    pub ticket: Option<Ticket>,
    pub anchor: (usize, usize),
    pub cursor: (usize, usize),
}
pub(super) struct InlineEdit {
    pub row: usize,
    pub column: usize,
    pub kind: CellKind,
    pub input: String,
    pub focus: bool,
}
impl App {
    pub(super) fn select_cell(&mut self, row: usize, column: usize, extend: bool) {
        if self
            .result
            .rows
            .get(row)
            .and_then(|r| r.get(column))
            .is_none()
        {
            return;
        }
        let anchor = self
            .tools
            .grid_selection
            .filter(|s| extend && s.ticket == self.result.current)
            .map_or((row, column), |s| s.anchor);
        self.tools.grid_selection = Some(Selection {
            ticket: self.result.current,
            anchor,
            cursor: (row, column),
        });
        let open = self.ui.inspector_open;
        self.open_cell(row, column);
        self.ui.inspector_open = open;
        self.tools.detail = Some(row);
        self.ui.inspector = shell::DetailMode::Record;
    }
    pub(super) fn cell_selected(&self, row: usize, column: usize) -> bool {
        let Some(s) = self
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.result.current)
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
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.result.current)
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
                .min(self.result.rows.len().saturating_sub(1)))
            .flat_map(|row| {
                cols[a.min(b)..=a.max(b)]
                    .iter()
                    .copied()
                    .map(move |c| (row, c))
            })
            .collect()
    }
    pub(super) fn selection_summary(&self) -> String {
        let cells = self.selected_cells();
        if cells.is_empty() {
            return String::new();
        }
        let values: Vec<_> = cells
            .iter()
            .filter_map(|&(r, c)| self.result.rows.get(r)?.get(c))
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
        let Some(original) = self.result.rows.get(row) else {
            return;
        };
        let Some(value) = self
            .drafts
            .as_ref()
            .and_then(|d| d.displayed(original, column))
            .or_else(|| original.get(column))
        else {
            return;
        };
        if matches!(value, Value::Blob(_)) || value.bytes() > 4096 {
            self.open_cell(row, column);
            return;
        }
        let kind = match value {
            Value::Null => CellKind::Null,
            Value::Integer(_) => CellKind::Integer,
            Value::Real(_) => CellKind::Real,
            _ => CellKind::Text,
        };
        self.tools.inline = Some(InlineEdit {
            row,
            column,
            kind,
            input: if *value == Value::Null {
                String::new()
            } else {
                crate::export::cell_text(value)
            },
            focus: true,
        });
    }
    pub(super) fn grid_keys(&mut self, ctx: &egui::Context) {
        if self.tools.inline.is_some()
            || !matches!(self.ui.nav, shell::Nav::Table | shell::Nav::Result)
            || self.ui.table_view != shell::TableView::Data
            || egui::Popup::is_any_open(ctx)
            || self.ui.palette_open
        {
            return;
        }
        if self.tools.grid_focus.is_none() || ctx.memory(|m| m.focused()) != self.tools.grid_focus {
            return;
        }
        let Some(s) = self
            .tools
            .grid_selection
            .filter(|s| s.ticket == self.result.current)
        else {
            return;
        };
        let cols = self.visible_columns();
        let Some(c) = cols.iter().position(|&c| c == s.cursor.1) else {
            return;
        };
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::CTRL, egui::Key::C)) {
            let mut rows = Vec::<Vec<String>>::new();
            let mut last = None;
            let mut bytes = 0;
            for (r, c) in self.selected_cells() {
                if last != Some(r) {
                    rows.push(vec![]);
                    last = Some(r);
                }
                let text = crate::export::cell_text(&self.result.rows[r][c]);
                bytes += text.len() + 1;
                if bytes > 8 * 1024 * 1024 {
                    self.status = "Selection exceeds clipboard budget".into();
                    return;
                }
                rows.last_mut().unwrap().push(text);
            }
            ctx.copy_text(
                rows.into_iter()
                    .map(|r| r.join("\t"))
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
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
                    .min(self.result.rows.len().saturating_sub(1));
                let col = c.saturating_add_signed(dc).min(cols.len() - 1);
                self.select_cell(row, cols[col], shift);
                self.tools.scroll_to_cell = true;
                break;
            }
        }
    }
}
