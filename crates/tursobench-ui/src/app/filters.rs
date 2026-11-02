use super::{App, View, results::CellKind};
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
impl App {
    fn apply_filters(&mut self, mut next: Browse) -> bool {
        if next.filters.len() > 32
            || next.filters.iter().map(|f| f.value.bytes()).sum::<usize>() > 1024 * 1024
        {
            self.tools.filter_error = "Filters exceed 32 predicates / 1 MiB".into();
            return false;
        }
        next.offset = 0;
        if self
            .submit(Operation::Browse(next.clone()), View::Browse)
            .is_some()
        {
            self.browse = Some(next);
            self.tools.filter_error.clear();
            true
        } else {
            false
        }
    }
    pub(super) fn add_filter(&mut self, filter: Filter) -> bool {
        let Some(mut next) = self.browse.clone() else {
            return false;
        };
        next.filters.push(filter);
        self.apply_filters(next)
    }
    pub(super) fn filter_controls(&mut self, ui: &mut egui::Ui) {
        let (Some(table), Some(next)) = (self.table.clone(), self.browse.clone()) else {
            return;
        };
        let enabled =
            !self.has_drafts() && self.commit_ticket.is_none() && self.ready && !self.closing;
        ui.add_enabled_ui(enabled, |ui| {
            ui.horizontal(|ui| {
                // A horizontal scroller keeps chips from growing the header vertically.
                egui::ScrollArea::horizontal()
                    .id_salt("filter-tokens")
                    .max_width((ui.available_width() - 100.0).max(0.0))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            for (i, f) in next.filters.iter().enumerate() {
                                let name =
                                    table.columns.get(f.column).map_or("?", |c| c.name.as_str());
                                let label = format!(
                                    "{name} {} {}",
                                    f.op.label(),
                                    if f.op.unary() {
                                        String::new()
                                    } else {
                                        f.value.preview()
                                    }
                                );
                                let chip = ui.button(label);
                                if chip.clicked() {
                                    self.tools.filter_edit = Some(i);
                                    self.filter_column = f.column;
                                    self.filter_op = f.op;
                                    self.filter_kind = kind(&f.value);
                                    self.tools.filter_value = match &f.value {
                                        Value::Blob(b) => crate::export::hex(b),
                                        v => crate::export::cell_text(v),
                                    };
                                }
                                egui::Popup::menu(&chip).show(|ui| {
                                    if self.tools.filter_edit != Some(i) {
                                        return;
                                    }
                                    ui.label(format!("{name} · stored comparison type"));
                                    egui::ComboBox::from_id_salt(("chip-op", i))
                                        .selected_text(self.filter_op.label())
                                        .show_ui(ui, |ui| {
                                            for op in FilterOp::ALL {
                                                ui.selectable_value(
                                                    &mut self.filter_op,
                                                    op,
                                                    op.label(),
                                                );
                                            }
                                        });
                                    egui::ComboBox::from_id_salt(("chip-type", i))
                                        .selected_text(self.filter_kind.label())
                                        .show_ui(ui, |ui| {
                                            for k in [
                                                CellKind::Text,
                                                CellKind::Integer,
                                                CellKind::Real,
                                                CellKind::Null,
                                                CellKind::Blob,
                                            ] {
                                                ui.selectable_value(
                                                    &mut self.filter_kind,
                                                    k,
                                                    k.label(),
                                                );
                                            }
                                        });
                                    ui.text_edit_singleline(&mut self.tools.filter_value);
                                    if ui.button("Apply").clicked() {
                                        match if self.filter_op.unary() {
                                            Ok(Value::Null)
                                        } else {
                                            self.filter_kind.parse(&self.tools.filter_value)
                                        } {
                                            Ok(value) => {
                                                let mut b = next.clone();
                                                b.filters[i] = Filter {
                                                    column: f.column,
                                                    op: self.filter_op,
                                                    value,
                                                };
                                                if self.apply_filters(b) {
                                                    ui.close();
                                                }
                                            }
                                            Err(e) => self.tools.filter_error = e,
                                        }
                                    }
                                });
                                if ui
                                    .small_button("×")
                                    .on_hover_text("Remove filter")
                                    .clicked()
                                {
                                    let mut b = next.clone();
                                    b.filters.remove(i);
                                    self.apply_filters(b);
                                }
                            }
                        });
                    });
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.filter)
                        .id_salt("token-input")
                        .hint_text("column = value")
                        .desired_width(ui.available_width().max(90.0))
                        .char_limit(65536),
                );
                if self.tools.filter_focus {
                    response.request_focus();
                    self.tools.filter_focus = false;
                    self.ui.filter_open = false;
                }
                if (response.has_focus() || response.lost_focus())
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Enter))
                {
                    match parse_filter(&self.filter, &table) {
                        Ok(f) => {
                            if self.add_filter(f) {
                                self.filter.clear();
                                self.tools.filter_focus = true;
                            }
                        }
                        Err(e) => self.tools.filter_error = e,
                    }
                }
                if (response.has_focus() || response.lost_focus())
                    && self.filter.is_empty()
                    && !next.filters.is_empty()
                    && ui.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Backspace))
                {
                    let mut b = next.clone();
                    b.filters.pop();
                    self.apply_filters(b);
                    self.tools.filter_focus = true;
                }
                if !self.tools.filter_error.is_empty() {
                    ui.colored_label(ui.visuals().error_fg_color, "!")
                        .on_hover_text(&self.tools.filter_error);
                }
            });
        });
    }
    pub(super) fn column_facets(&mut self, ui: &mut egui::Ui, column: usize) {
        if self.view != View::Browse || self.browse.is_none() {
            ui.label("Facets require a table browse");
            return;
        }
        let sample = self.result.rows.len().min(1000);
        ui.label(format!("Top values · {sample} retained rows"));
        let mut counts = std::collections::BTreeMap::<String, (Value, usize)>::new();
        for value in self
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
            if let Some(c) = self.result.columns.get(column) {
                self.filter = format!("{} = ", tursobench_core::quote_identifier(&c.name));
            }
            self.ui.filter_open = true;
            self.tools.filter_focus = true;
            ui.close();
        }
        if ui.button("Hide column").clicked() {
            self.tools.columns.hidden.insert(column);
            ui.close();
        }
    }
}
fn kind(value: &Value) -> CellKind {
    match value {
        Value::Null => CellKind::Null,
        Value::Integer(_) => CellKind::Integer,
        Value::Real(_) => CellKind::Real,
        Value::Blob(_) => CellKind::Blob,
        Value::Text(_) => CellKind::Text,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn table() -> Table {
        Table {
            name: "t".into(),
            is_table: true,
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
        assert_eq!(
            parse_filter("\"odd \"\" name\" = 'it''s ✓'", &t)
                .unwrap()
                .value,
            Value::Text("it's ✓".into())
        );
        assert_eq!(
            parse_filter("n>=-9007199254740993", &t).unwrap().value,
            Value::Integer(-9007199254740993)
        );
        assert_eq!(parse_filter("n<=3", &t).unwrap().op, FilterOp::LessOrEqual);
        assert_eq!(
            parse_filter("n = '003'", &t).unwrap().value,
            Value::Text("003".into())
        );
        assert_eq!(
            parse_filter("n IS NOT NULL", &t).unwrap().op,
            FilterOp::IsNotNull
        );
        assert_eq!(
            parse_filter("n=x'00ff'", &t).unwrap().value,
            Value::Blob(vec![0, 255])
        );
        assert!(parse_filter("n=", &t).is_err());
        assert!(parse_filter("n>=oops", &t).is_err());
        assert!(parse_filter("n = 'bad'quote'", &t).is_err());
        assert!(parse_filter("n==1", &t).is_err());
        assert_eq!(
            parse_filter("n contains '100%_'", &t).unwrap().value,
            Value::Text("100%_".into())
        );
    }
}
