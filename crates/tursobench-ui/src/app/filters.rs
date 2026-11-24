use super::{
    App, View,
    results::{CellEditor, CellKind},
};
use eframe::egui;
use tursobench_core::{Browse, Filter, FilterExpr, FilterOp, Table, Value};
use tursobench_db::Operation;

/// Decode one SQL-quoted token, leaving its suffix for the grammar to validate.
fn quoted(input: &str, quote: char) -> Result<(String, &str), String> {
    let mut chars = input[quote.len_utf8()..].char_indices().peekable();
    let mut value = String::new();
    while let Some((i, c)) = chars.next() {
        if c != quote {
            value.push(c);
            continue;
        }
        if chars.peek().is_some_and(|(_, c)| *c == quote) {
            chars.next();
            value.push(quote);
        } else {
            return Ok((value, &input[i + 2 * quote.len_utf8()..]));
        }
    }
    Err("Unclosed quoted name or value".into())
}
fn filter_operator(rest: &str) -> Result<(FilterOp, &str), String> {
    let lower = rest.to_ascii_lowercase();
    match lower.as_str() {
        "is null" => return Ok((FilterOp::IsNull, "")),
        "is not null" => return Ok((FilterOp::IsNotNull, "")),
        _ => {}
    }
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
    Ok((op, rest[token.len()..].trim()))
}
fn filter_value(value: &str, op: FilterOp, declared_type: &str) -> Result<Value, String> {
    if op.unary() {
        return Ok(Value::Null);
    }
    if value.is_empty() {
        return Err("Enter a value; use '' for empty text".into());
    }
    if value.starts_with('\'') {
        let (text, rest) = quoted(value, '\'')?;
        return if rest.is_empty() {
            Ok(Value::Text(text))
        } else {
            Err("Escape a quote as two quotes".into())
        };
    }
    if value.eq_ignore_ascii_case("null") {
        return Ok(Value::Null);
    }
    if (value.starts_with("x'") || value.starts_with("X'")) && value.ends_with('\'') {
        return CellKind::Blob.parse(&value[2..value.len() - 1]);
    }
    if op == FilterOp::Contains {
        return Ok(Value::Text(value.into()));
    }
    let ty = declared_type.to_ascii_uppercase();
    if ty.contains("INT") {
        return CellKind::Integer.parse(value);
    }
    if ["REAL", "FLOA", "DOUB"].iter().any(|t| ty.contains(t)) {
        return CellKind::Real.parse(value);
    }
    if ty.contains("NUM") || ty.contains("DEC") {
        return CellKind::Integer
            .parse(value)
            .or_else(|_| CellKind::Real.parse(value));
    }
    if let Ok(n) = value.parse::<i64>() {
        return Ok(Value::Integer(n));
    }
    if value.contains(['.', 'e', 'E']) && value.parse::<f64>().is_ok() {
        return CellKind::Real.parse(value);
    }
    Ok(Value::Text(value.into()))
}
/// This grammar produces bound predicates, never SQL text.
pub(super) fn parse_filter(input: &str, table: &Table) -> Result<Filter, String> {
    let input = input.trim();
    let (name, rest) = if input.starts_with('"') {
        quoted(input, '"')?
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
    let (op, raw) = filter_operator(rest.trim())?;
    let value = filter_value(raw, op, &table.columns[column].declared_type)?;
    if op == FilterOp::Contains && !matches!(value, Value::Text(_)) {
        return Err("Contains requires text".into());
    }
    Ok(Filter { column, op, value })
}
/// The index after the quote opened at `start`. A doubled quote is part of
/// the text, and inside brackets a backslash escapes a JSON string's quote.
fn quoted_end(bytes: &[u8], start: usize, json: bool) -> usize {
    let quote = bytes[start];
    let mut i = start + 1;
    while i < bytes.len() {
        if json && quote == b'"' && bytes[i] == b'\\' || bytes[i..].starts_with(&[quote; 2]) {
            i += 2;
        } else if bytes[i] == quote {
            return i + 1;
        } else {
            i += 1;
        }
    }
    i
}
/// The ranges of `operator` as a whole word outside quotes, brackets and parentheses.
fn boolean_boundaries(input: &str, operator: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = input.as_bytes();
    let boundary = |i: usize| {
        bytes
            .get(i)
            .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_')
    };
    let (mut result, mut i, mut brackets, mut parens) = (Vec::new(), 0, 0i32, 0i32);
    while i < bytes.len() {
        match bytes[i] {
            b'\'' | b'"' => {
                i = quoted_end(bytes, i, brackets > 0);
                continue;
            }
            b'[' => brackets += 1,
            b']' => brackets -= 1,
            b'(' if brackets == 0 => parens += 1,
            b')' if brackets == 0 => parens -= 1,
            _ => {}
        }
        let end = i + operator.len();
        if brackets == 0
            && parens == 0
            && bytes
                .get(i..end)
                .is_some_and(|w| w.eq_ignore_ascii_case(operator.as_bytes()))
            && (i == 0 || boundary(i - 1))
            && boundary(end)
        {
            result.push(i..end);
            i = end;
        } else {
            i += 1;
        }
    }
    result
}
fn parse_expression(input: &str, table: &Table, depth: usize) -> Result<FilterExpr, String> {
    if depth > 8 || input.len() > 64 * 1024 {
        return Err("Expression exceeds nesting/size budget".into());
    }
    let input = input.trim();
    for operator in ["OR", "AND"] {
        let boundaries = boolean_boundaries(input, operator);
        if boundaries.is_empty() {
            continue;
        }
        let starts = std::iter::once(0).chain(boundaries.iter().map(|b| b.end));
        let ends = boundaries.iter().map(|b| b.start).chain([input.len()]);
        let parts = starts
            .zip(ends)
            .map(|(start, end)| parse_expression(&input[start..end], table, depth + 1))
            .collect::<Result<_, _>>()?;
        return Ok(if operator == "OR" {
            FilterExpr::Any(parts)
        } else {
            FilterExpr::All(parts)
        });
    }
    if input.starts_with('(') && input.ends_with(')') {
        return parse_expression(&input[1..input.len() - 1], table, depth + 1);
    }
    match boolean_boundaries(input, "IN").first() {
        Some(at) => parse_in(&input[..at.start], &input[at.end..], table),
        None => parse_filter(input, table).map(FilterExpr::Compare),
    }
}
/// `column IN [json values]`.
fn parse_in(column: &str, list: &str, table: &Table) -> Result<FilterExpr, String> {
    let field = parse_filter(&format!("{column} IS NULL"), table)?;
    let list: Vec<serde_json::Value> =
        serde_json::from_str(list.trim()).map_err(|e| e.to_string())?;
    if list.is_empty() || list.len() > 1000 {
        return Err("IN requires 1–1000 values".into());
    }
    Ok(FilterExpr::In {
        column: field.column,
        values: list
            .into_iter()
            .map(Value::from_input)
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
    })
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
    fn ui(&mut self, ui: &mut egui::Ui, column: usize) -> Action {
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
    Predicate(Option<FilterExpr>),
}
type Action = Option<Result<FilterChange, String>>;
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
            Self::Predicate(predicate) => next.options.predicate = predicate.map(Box::new),
        }
        if next.filters.len() > 32
            || next.filters.iter().map(|f| f.value.bytes()).sum::<usize>() > 1024 * 1024
        {
            return Err("Filters exceed 32 predicates / 1 MiB".into());
        }
        next.offset = 0;
        next.options.cursor = None;
        next.options.reverse = false;
        Ok(next)
    }
}
/// A filter's chip, its editor popup and its remove button.
fn filter_chip(
    ui: &mut egui::Ui,
    (i, f): (usize, &Filter),
    name: &str,
    editor: &mut Option<FilterEditor>,
) -> Action {
    let value = if f.op.unary() {
        String::new()
    } else {
        f.value.preview()
    };
    let color = crate::appearance::Tokens::of(ui.ctx()).primary;
    let chip = super::presentation::chip(ui, &format!("{name} {} {value}", f.op.label()), color);
    if chip.clicked() {
        *editor = Some(FilterEditor::new(i, f));
    }
    let edited = egui::Popup::menu(&chip).show(|ui| {
        let editor = editor.as_mut().filter(|e| e.index == i)?;
        ui.label(format!("{name} · stored comparison type"));
        editor.ui(ui, f.column)
    });
    let removed = ui.small_button("×").on_hover_text("Remove filter");
    if removed.clicked() {
        return Some(Ok(FilterChange::Remove(i)));
    }
    edited.and_then(|popup| popup.inner)
}
fn filter_chips(
    ui: &mut egui::Ui,
    table: &Table,
    filters: &[Filter],
    editor: &mut Option<FilterEditor>,
) -> Action {
    egui::ScrollArea::horizontal()
        .id_salt("filter-tokens")
        .max_width((ui.available_width() - 100.0).max(0.0))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                filters.iter().enumerate().fold(None, |action, chip| {
                    let name = table.columns.get(chip.1.column).map_or("?", |c| &c.name);
                    filter_chip(ui, chip, name, editor).or(action)
                })
            })
            .inner
        })
        .inner
}
/// Enter adds the typed filter; Backspace in an empty input removes the last
/// one and keeps the input focused.
fn filter_input_keys(
    ui: &egui::Ui,
    text: &str,
    table: &Table,
    filters: usize,
    refocus: &mut bool,
) -> Action {
    let key = |key| ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, key));
    let mut action = None;
    if key(egui::Key::Enter) {
        action = Some(parse_filter(text, table).map(FilterChange::Add));
    }
    if text.is_empty() && filters > 0 && key(egui::Key::Backspace) {
        action = Some(Ok(FilterChange::Remove(filters - 1)));
        *refocus = true;
    }
    action
}
/// The seven most frequent `values`, most frequent first.
fn top_values<'a>(values: impl Iterator<Item = &'a Value>) -> Vec<(Value, usize)> {
    let mut counts = std::collections::BTreeMap::<String, (Value, usize)>::new();
    for value in values {
        if let Ok(key) = serde_json::to_string(value) {
            counts.entry(key).or_insert((value.clone(), 0)).1 += 1;
        }
    }
    let mut counts: Vec<_> = counts.into_values().collect();
    counts.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
    counts.truncate(7);
    counts
}
impl App {
    fn change_filters(&mut self, change: FilterChange) -> bool {
        let Some(browse) = &self.database.document.browse else {
            return false;
        };
        let next = match change.apply(browse) {
            Ok(next) => next,
            Err(error) => {
                self.database.document.tools.filter_error = error;
                return false;
            }
        };
        if self
            .submit(Operation::Browse(next.clone()), View::Browse)
            .is_none()
        {
            return false;
        }
        self.database.tools.counts.clear();
        self.database.document.browse = Some(next);
        self.database.document.tools.filter_editor = None;
        self.database.document.tools.filter_error.clear();
        true
    }
    pub(super) fn add_filter(&mut self, filter: Filter) -> bool {
        self.change_filters(FilterChange::Add(filter))
    }
    pub(super) fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let expression = self.expression_controls(ui);
        match self.filter_bar(ui).or(expression) {
            Some(Ok(change)) => self.finish_filter_change(ui, change),
            Some(Err(error)) => self.database.document.tools.filter_error = error,
            None => {}
        }
    }
    /// The filter chips and the input that adds one.
    fn filter_bar(&mut self, ui: &mut egui::Ui) -> Action {
        let (Some(table), Some(browse)) = (
            &self.database.document.table,
            &self.database.document.browse,
        ) else {
            return None;
        };
        let enabled = self.requests_enabled();
        let tools = &mut self.database.document.tools;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                let chips = filter_chips(ui, table, &browse.filters, &mut tools.filter_editor);
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.database.document.filter)
                        .id_salt("token-input")
                        .hint_text("column = value")
                        .desired_width(ui.available_width().max(90.0))
                        .char_limit(65536),
                );
                if std::mem::take(&mut tools.filter_focus) {
                    response.request_focus();
                    self.database.document.ui.filter_open = self.database.workspace.enabled;
                }
                let typed = (response.has_focus() || response.lost_focus())
                    .then(|| {
                        filter_input_keys(
                            ui,
                            &self.database.document.filter,
                            table,
                            browse.filters.len(),
                            &mut tools.filter_focus,
                        )
                    })
                    .flatten();
                if !tools.filter_error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, "!")
                        .on_hover_text(&tools.filter_error);
                }
                typed.or(chips)
            })
            .inner
        })
        .inner
    }
    /// Apply a chip, input or expression change; an added filter clears and
    /// refocuses the input, and a replaced one closes its editor.
    fn finish_filter_change(&mut self, ui: &egui::Ui, change: FilterChange) {
        let add = matches!(change, FilterChange::Add(_));
        let replace = matches!(change, FilterChange::Replace(..));
        if !self.change_filters(change) {
            return;
        }
        if add {
            self.database.document.filter.clear();
            self.database.document.tools.filter_focus = true;
        }
        if replace {
            egui::Popup::close_all(ui.ctx());
        }
    }
    fn expression_controls(&mut self, ui: &mut egui::Ui) -> Action {
        let mut change = None;
        ui.menu_button("AND / OR / IN…", |ui| {
            ui.label("Group comparisons with parentheses, AND and OR. IN accepts a JSON list.");
            ui.label("Example: (id = 5 OR id IN [7,8]) AND name != 'hidden'");
            ui.text_edit_multiline(&mut self.database.document.tools.advanced_filter);
            if ui.button("Apply expression").clicked()
                && let Some(table) = &self.database.document.table
            {
                let expression = &self.database.document.tools.advanced_filter;
                change = Some(
                    parse_expression(expression, table, 0)
                        .map(|e| FilterChange::Predicate(Some(e))),
                );
            }
            if ui.button("Clear expression").clicked() {
                change = Some(Ok(FilterChange::Predicate(None)));
            }
        });
        change
    }
    pub(super) fn column_facets(&mut self, ui: &mut egui::Ui, column: usize) {
        if self.database.document.view != View::Browse || self.database.document.browse.is_none() {
            ui.label("Facets require a table browse");
            return;
        }
        let rows = &self.database.document.result.rows;
        let sample = rows.len().min(1000);
        ui.label(format!("Top values · {sample} retained rows"));
        for (value, n) in top_values(rows.iter().take(sample).filter_map(|r| r.get(column))) {
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
            if let Some(c) = self.database.document.result.columns.get(column) {
                self.database.document.filter =
                    format!("{} = ", tursobench_core::quote_identifier(&c.name));
            }
            self.database.document.ui.filter_open = true;
            self.database.document.tools.filter_focus = true;
            ui.close();
        }
        if ui.button("Hide column").clicked() {
            self.database.document.tools.columns.hidden.insert(column);
            ui.close();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::{FilterChange, FilterEditor, parse_filter};
    use tursobench_core::{Browse, FilterOp, Table, Value};
    #[test]
    fn grammar_groups_expressions_and_preserves_identifiers_types_and_boundaries() {
        let table = sample();
        assert!(super::parse_expression("(n = 5 OR n IN [7,8]) AND r > 1.0", &table, 0).is_ok());
        assert!(
            super::parse_expression(
                r#"n IN [{"type":"Blob","value":[0,255]}] OR n = 5"#,
                &table,
                0
            )
            .is_ok()
        );
        assert!(super::parse_expression("\"odd \"\" name\" = 'x OR y'", &table, 0).is_ok());
        let mut table = table;
        table.columns[1].declared_type.clear();
        assert_eq!(
            parse_filter("n = 5", &table).unwrap().value,
            Value::Integer(5)
        );
        assert_eq!(
            parse_filter("n = '5'", &table).unwrap().value,
            Value::Text("5".into())
        );
        let t = sample();
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
        let filter = parse_filter("n=1", &sample()).unwrap();
        let original = Browse {
            options: Default::default(),
            table: "t".into(),
            offset: 100,
            sort: Some(1),
            descending: true,
            filters: vec![filter.clone()],
        };
        let replacement = parse_filter("n=2", &sample()).unwrap();
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
            options: Default::default(),
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
    fn sample() -> Table {
        Table {
            name: "t".into(),
            is_table: true,
            has_rowid: true,
            binary_collations: true,
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
}
