use super::{
    App, View,
    results::{CellEditor, CellKind},
};
use eframe::egui;
use tursobench_core::{Browse, Filter, FilterOp, Table, Value};
use tursobench_db::Operation;

/// This grammar produces bound predicates, never SQL text.
pub(super) fn parse_filter(input: &str, table: &Table) -> Result<Filter, String> {
    let input = input.trim();
    let (name, rest) = if let Some(s) = input.strip_prefix('"') {
        let mut name = String::new();
        let mut iter = s.char_indices().peekable();
        let mut end = None;
        while let Some((i, c)) = iter.next() {
            if c == '"' {
                if iter.peek().is_some_and(|(_, c)| *c == '"') {
                    iter.next();
                    name.push('"');
                } else {
                    end = Some(i + 2);
                    break;
                }
            } else {
                name.push(c);
            }
        }
        let end = end.ok_or("Unclosed quoted column name")?;
        (name, &input[end..])
    } else {
        let end = input
            .find(|c: char| c.is_whitespace() || "=!<>≠".contains(c))
            .unwrap_or(input.len());
        (input[..end].to_owned(), &input[end..])
    };
    let column = table
        .columns
        .iter()
        .position(|c| c.name.eq_ignore_ascii_case(&name))
        .ok_or_else(|| format!("Unknown column: {name}"))?;
    let rest = rest.trim();
    let lower = rest.to_lowercase();
    let (op, value) = if lower == "is null" {
        (FilterOp::IsNull, "")
    } else if lower == "is not null" {
        (FilterOp::IsNotNull, "")
    } else {
        let (token, op) = [
            ("!=", FilterOp::NotEqual),
            ("≠", FilterOp::NotEqual),
            ("<=", FilterOp::LessOrEqual),
            (">=", FilterOp::GreaterOrEqual),
            ("=", FilterOp::Equal),
            ("<", FilterOp::Less),
            (">", FilterOp::Greater),
            ("contains ", FilterOp::Contains),
        ]
        .into_iter()
        .find(|(t, _)| lower.starts_with(t))
        .ok_or("Expected =, !=, <, >, <=, >=, contains, is null or is not null")?;
        (op, rest[token.len()..].trim())
    };
    if op.unary() {
        return Ok(Filter {
            column,
            op,
            value: Value::Null,
        });
    }
    if value.is_empty() {
        return Err("Enter a value; use '' for empty text".into());
    }
    let value = if value.starts_with('\'') {
        if !value.ends_with('\'') || value.len() < 2 {
            return Err("Unclosed text value".into());
        }
        let body = &value[1..value.len() - 1];
        let mut out = String::new();
        let mut chars = body.chars();
        while let Some(c) = chars.next() {
            if c == '\'' && chars.next() != Some('\'') {
                return Err("Escape a quote as two quotes".into());
            }
            out.push(c);
        }
        Value::Text(out)
    } else if value.eq_ignore_ascii_case("null") {
        Value::Null
    } else if (value.starts_with("x'") || value.starts_with("X'")) && value.ends_with('\'') {
        CellKind::Blob.parse(&value[2..value.len() - 1])?
    } else if op == FilterOp::Contains {
        Value::Text(value.into())
    } else {
        let ty = table.columns[column].declared_type.to_uppercase();
        if ty.contains("INT") {
            CellKind::Integer.parse(value)?
        } else if ["REAL", "FLOA", "DOUB"].iter().any(|t| ty.contains(t)) {
            CellKind::Real.parse(value)?
        } else if ty.contains("NUM") || ty.contains("DEC") {
            CellKind::Integer
                .parse(value)
                .or_else(|_| CellKind::Real.parse(value))?
        } else {
            Value::Text(value.into())
        }
    };
    if op == FilterOp::Contains && !matches!(value, Value::Text(_)) {
        return Err("Contains requires text".into());
    }
    Ok(Filter { column, op, value })
}
pub(super) struct FilterEditor {
    index: usize,
    op: FilterOp,
    value: CellEditor,
}
impl FilterEditor {
    fn new(index: usize, filter: &Filter) -> Self {
        Self {
            index,
            op: filter.op,
            value: CellEditor::new(&filter.value),
        }
    }
    fn filter(&self, column: usize) -> Result<Filter, String> {
        Ok(Filter {
            column,
            op: self.op,
            value: if self.op.unary() {
                Value::Null
            } else {
                self.value.parse()?
            },
        })
    }
    fn ui(&mut self, ui: &mut egui::Ui, column: usize) -> Option<Result<FilterChange, String>> {
        egui::ComboBox::from_id_salt(("chip-op", self.index))
            .selected_text(self.op.label())
            .show_ui(ui, |ui| {
                for op in FilterOp::ALL {
                    ui.selectable_value(&mut self.op, op, op.label());
                }
            });
        self.value.type_picker(ui, ("chip-type", self.index), true);
        ui.text_edit_singleline(&mut self.value.input);
        ui.button("Apply").clicked().then(|| {
            self.filter(column)
                .map(|f| FilterChange::Replace(self.index, f))
        })
    }
}
enum FilterChange {
    Add(Filter),
    Replace(usize, Filter),
    Remove(usize),
}
impl FilterChange {
    fn apply(self, browse: &Browse) -> Result<Browse, String> {
        let mut next = browse.clone();
        match self {
            Self::Add(filter) => next.filters.push(filter),
            Self::Replace(i, filter) => {
                *next.filters.get_mut(i).ok_or("Filter no longer exists")? = filter
            }
            Self::Remove(i) => {
                if i >= next.filters.len() {
                    return Err("Filter no longer exists".into());
                }
                next.filters.remove(i);
            }
        }
        if next.filters.len() > 32
            || next.filters.iter().map(|f| f.value.bytes()).sum::<usize>() > 1024 * 1024
        {
            return Err("Filters exceed 32 predicates / 1 MiB".into());
        }
        next.offset = 0;
        Ok(next)
    }
}
fn filter_chips(
    ui: &mut egui::Ui,
    table: &Table,
    filters: &[Filter],
    editor: &mut Option<FilterEditor>,
) -> Option<Result<FilterChange, String>> {
    let mut action = None;
    egui::ScrollArea::horizontal()
        .id_salt("filter-tokens")
        .max_width((ui.available_width() - 100.0).max(0.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                for (i, f) in filters.iter().enumerate() {
                    let name = table.columns.get(f.column).map_or("?", |c| c.name.as_str());
                    let label = format!(
                        "{name} {} {}",
                        f.op.label(),
                        if f.op.unary() {
                            String::new()
                        } else {
                            f.value.preview()
                        }
                    );
                    let color = crate::appearance::Tokens::of(ui.ctx()).primary;
                    let chip = super::presentation::chip(ui, &label, color);
                    if chip.clicked() {
                        *editor = Some(FilterEditor::new(i, f));
                    }
                    egui::Popup::menu(&chip).show(|ui| {
                        if let Some(editor) = editor.as_mut().filter(|e| e.index == i) {
                            ui.label(format!("{name} · stored comparison type"));
                            if let Some(change) = editor.ui(ui, f.column) {
                                action = Some(change);
                            }
                        }
                    });
                    if ui
                        .small_button("×")
                        .on_hover_text("Remove filter")
                        .clicked()
                    {
                        action = Some(Ok(FilterChange::Remove(i)));
                    }
                }
            });
        });
    action
}
impl App {
    fn change_filters(&mut self, change: FilterChange) -> bool {
        let Some(browse) = &self.document.browse else {
            return false;
        };
        let next = match change.apply(browse) {
            Ok(next) => next,
            Err(error) => {
                self.document.tools.filter_error = error;
                return false;
            }
        };
        if self
            .submit(Operation::Browse(next.clone()), View::Browse)
            .is_none()
        {
            return false;
        }
        self.document.browse = Some(next);
        self.document.tools.filter_editor = None;
        self.document.tools.filter_error.clear();
        true
    }
    pub(super) fn add_filter(&mut self, filter: Filter) -> bool {
        self.change_filters(FilterChange::Add(filter))
    }
    pub(super) fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let (Some(table), Some(browse)) = (&self.document.table, &self.document.browse) else {
            return;
        };
        let enabled =
            !self.has_drafts() && self.commit_ticket.is_none() && self.ready && !self.closing;
        let action = ui
            .add_enabled_ui(enabled, |ui| {
                ui.horizontal(|ui| {
                    let mut action = filter_chips(
                        ui,
                        table,
                        &browse.filters,
                        &mut self.document.tools.filter_editor,
                    );
                    let response = ui.add(
                        egui::TextEdit::singleline(&mut self.document.filter)
                            .id_salt("token-input")
                            .hint_text("column = value")
                            .desired_width(ui.available_width().max(90.0))
                            .char_limit(65536),
                    );
                    if std::mem::take(&mut self.document.tools.filter_focus) {
                        response.request_focus();
                        self.document.ui.filter_open = self.workspace.enabled;
                    }
                    if response.has_focus() || response.lost_focus() {
                        if ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                        {
                            action = Some(
                                parse_filter(&self.document.filter, table).map(FilterChange::Add),
                            );
                        }
                        if self.document.filter.is_empty()
                            && !browse.filters.is_empty()
                            && ui.input_mut(|i| {
                                i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace)
                            })
                        {
                            action = Some(Ok(FilterChange::Remove(browse.filters.len() - 1)));
                            self.document.tools.filter_focus = true;
                        }
                    }
                    if !self.document.tools.filter_error.is_empty() {
                        ui.colored_label(ui.visuals().error_fg_color, "!")
                            .on_hover_text(&self.document.tools.filter_error);
                    }
                    action
                })
                .inner
            })
            .inner;
        match action {
            Some(Ok(change)) => {
                let add = matches!(change, FilterChange::Add(_));
                let replace = matches!(change, FilterChange::Replace(..));
                if self.change_filters(change) {
                    if add {
                        self.document.filter.clear();
                        self.document.tools.filter_focus = true;
                    }
                    if replace {
                        egui::Popup::close_all(ui.ctx());
                    }
                }
            }
            Some(Err(error)) => self.document.tools.filter_error = error,
            None => {}
        }
    }
    pub(super) fn column_facets(&mut self, ui: &mut egui::Ui, column: usize) {
        if self.document.view != View::Browse || self.document.browse.is_none() {
            ui.label("Facets require a table browse");
            return;
        }
        let sample = self.document.result.rows.len().min(1000);
        ui.label(format!("Top values · {sample} retained rows"));
        let mut counts = std::collections::BTreeMap::<String, (Value, usize)>::new();
        for value in self
            .document
            .result
            .rows
            .iter()
            .take(sample)
            .filter_map(|r| r.get(column))
        {
            if let Ok(key) = serde_json::to_string(value) {
                let entry = counts.entry(key).or_insert((value.clone(), 0));
                entry.1 += 1;
            }
        }
        let mut counts: Vec<_> = counts.into_values().collect();
        counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        for (value, n) in counts.into_iter().take(7) {
            if ui.button(format!("{}  · {n}", value.preview())).clicked() {
                self.add_filter(Filter {
                    column,
                    op: FilterOp::Equal,
                    value,
                });
                ui.close();
            }
        }
        if ui.button("Custom filter…").clicked() {
            if let Some(c) = self.document.result.columns.get(column) {
                self.document.filter = format!("{} = ", tursobench_core::quote_identifier(&c.name));
            }
            self.document.ui.filter_open = true;
            self.document.tools.filter_focus = true;
            ui.close();
        }
        if ui.button("Hide column").clicked() {
            self.document.tools.columns.hidden.insert(column);
            ui.close();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{FilterChange, FilterEditor, parse_filter};
    use tursobench_core::{Browse, FilterOp, Table, Value};
    fn table() -> Table {
        Table {
            name: "t".into(),
            is_table: true,
            has_rowid: true,
            columns: [("odd \" name", "TEXT"), ("n", "INTEGER"), ("r", "REAL")]
                .into_iter()
                .map(|(name, ty)| tursobench_core::TableColumn {
                    name: name.into(),
                    declared_type: ty.into(),
                    not_null: false,
                    default_sql: None,
                    primary_key: 0,
                    hidden: 0,
                })
                .collect(),
        }
    }
    #[test]
    fn tokens_preserve_identifiers_storage_types_and_operator_boundaries() {
        let t = table();
        for (input, column, op, value) in [
            (
                "\"odd \"\" name\" = 'it''s ✓'",
                0,
                FilterOp::Equal,
                Value::Text("it's ✓".into()),
            ),
            (
                "n>=-9007199254740993",
                1,
                FilterOp::GreaterOrEqual,
                Value::Integer(-9007199254740993),
            ),
            ("n<=3", 1, FilterOp::LessOrEqual, Value::Integer(3)),
            ("n = '003'", 1, FilterOp::Equal, Value::Text("003".into())),
            ("n IS NOT NULL", 1, FilterOp::IsNotNull, Value::Null),
            ("n is null", 1, FilterOp::IsNull, Value::Null),
            ("n=x'00ff'", 1, FilterOp::Equal, Value::Blob(vec![0, 255])),
            ("n = X''", 1, FilterOp::Equal, Value::Blob(vec![])),
            (
                "n contains '100%_'",
                1,
                FilterOp::Contains,
                Value::Text("100%_".into()),
            ),
            ("r < -1.5", 2, FilterOp::Less, Value::Real(-1.5)),
            ("n != null", 1, FilterOp::NotEqual, Value::Null),
            ("n≠3", 1, FilterOp::NotEqual, Value::Integer(3)),
            ("n > 1", 1, FilterOp::Greater, Value::Integer(1)),
        ] {
            let f = parse_filter(input, &t).unwrap();
            assert_eq!((f.column, f.op, &f.value), (column, op, &value), "{input}");
            let mut editor = FilterEditor::new(0, &f);
            assert_eq!(editor.filter(column).unwrap().value, value);
            editor.op = FilterOp::IsNull;
            editor.value.input = "invalid numeric/blob input".into();
            assert_eq!(editor.filter(column).unwrap().value, Value::Null);
        }
        for input in [
            "n=",
            "n>=oops",
            "n = 'bad'quote'",
            "n==1",
            "missing=1",
            "\"unclosed=1",
            "n contains null",
            "n=x'f'",
            "n=x'gg'",
            "r=NaN",
            "r=inf",
            "n=9223372036854775808",
            "n='unclosed",
            "n like 1",
        ] {
            assert!(parse_filter(input, &t).is_err(), "{input}");
        }
    }
    #[test]
    fn filter_changes_are_bounded_and_leave_the_source_untouched() {
        let filter = parse_filter("n=1", &table()).unwrap();
        let original = Browse {
            table: "t".into(),
            offset: 100,
            sort: Some(1),
            descending: true,
            filters: vec![filter.clone()],
        };
        let replacement = parse_filter("n=2", &table()).unwrap();
        for (change, values) in [
            (
                FilterChange::Add(replacement.clone()),
                vec![Value::Integer(1), Value::Integer(2)],
            ),
            (
                FilterChange::Replace(0, replacement),
                vec![Value::Integer(2)],
            ),
            (FilterChange::Remove(0), vec![]),
        ] {
            let next = change.apply(&original).unwrap();
            assert_eq!(
                next.filters
                    .into_iter()
                    .map(|f| f.value)
                    .collect::<Vec<_>>(),
                values
            );
            assert_eq!(
                (next.offset, next.sort, next.descending),
                (0, Some(1), true)
            );
        }
        assert!(FilterChange::Remove(1).apply(&original).is_err());
        assert!(
            FilterChange::Replace(1, filter.clone())
                .apply(&original)
                .is_err()
        );
        let full = Browse {
            filters: vec![filter.clone(); 32],
            ..original.clone()
        };
        assert!(FilterChange::Add(filter.clone()).apply(&full).is_err());
        let large = tursobench_core::Filter {
            value: Value::Blob(vec![0; 1024 * 1024]),
            ..filter
        };
        assert!(FilterChange::Add(large).apply(&original).is_err());
        assert_eq!(original.offset, 100);
        assert_eq!(original.filters[0].value, Value::Integer(1));
    }
}
