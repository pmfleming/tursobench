//! Conservative index drafts for a deliberately narrow, validated query shape.
use crate::sql::{self, TokenStyle};
use tursobench_core::{Inspection, quote_identifier};

fn unquote(s: &str) -> String {
    s.strip_prefix('"')
        .and_then(|s| s.strip_suffix('"'))
        .unwrap_or(s)
        .replace("\"\"", "\"")
}
fn scalar(value: &str) -> bool {
    value.starts_with(['?', ':', '$', '@', '\''])
        || value.parse::<f64>().is_ok()
        || value.eq_ignore_ascii_case("NULL")
}
/// The ORDER BY column of a tail, `Some(None)` without one, or `None` for
/// any other tail.
fn order_by(tail: &[String]) -> Option<Option<String>> {
    if tail.is_empty() {
        return Some(None);
    }
    let simple = (tail.len() == 3 || tail.len() == 4)
        && tail[0].eq_ignore_ascii_case("ORDER")
        && tail[1].eq_ignore_ascii_case("BY")
        && (tail.len() == 3 || tail[3].eq_ignore_ascii_case("ASC"));
    simple.then(|| Some(unquote(&tail[2])))
}
/// The equality column, then any distinct ORDER BY column, of
/// `SELECT ... FROM table WHERE column = scalar [ORDER BY column [ASC]]`.
fn shape(ts: &[String], table: &str) -> Option<Vec<String>> {
    let complex = ts.iter().any(|t| {
        t == "("
            || matches!(
                t.to_uppercase().as_str(),
                "JOIN" | "WITH" | "OR" | "UNION" | "COLLATE"
            )
    });
    if !ts.first()?.eq_ignore_ascii_case("SELECT") || complex {
        return None;
    }
    let from = ts.iter().position(|t| t.eq_ignore_ascii_case("FROM"))?;
    if !unquote(ts.get(from + 1)?).eq_ignore_ascii_case(table)
        || !ts.get(from + 2)?.eq_ignore_ascii_case("WHERE")
    {
        return None;
    }
    let mut columns = vec![unquote(ts.get(from + 3)?)];
    if ts.get(from + 4)? != "=" || !scalar(ts.get(from + 5)?) {
        return None;
    }
    if let Some(order) = order_by(&ts[from + 6..])?
        && !columns.contains(&order)
    {
        columns.push(order);
    }
    Some(columns)
}
/// Whether the primary key or a full index already leads with `columns`.
fn covered(info: &Inspection, columns: &[String]) -> bool {
    let keys: Vec<_> = info
        .table
        .keys()
        .iter()
        .map(|i| info.table.columns[*i].name.as_str())
        .collect();
    keys.starts_with(&columns.iter().map(String::as_str).collect::<Vec<_>>())
        || info.indexes.iter().any(|i| {
            !i.partial
                && i.columns
                    .iter()
                    .take(columns.len())
                    .map(Option::as_deref)
                    .eq(columns.iter().map(|c| Some(c.as_str())))
        })
}

/// Only SELECT ... FROM one unaliased table WHERE column = scalar, with an
/// optional simple ORDER BY column. Complex predicates/collations stay manual.
pub fn index_draft(sql: &str, info: &Inspection) -> Option<String> {
    sql::validate_single(sql).ok()?;
    let ts: Vec<_> = sql::tokens(sql)
        .into_iter()
        .filter(|t| t.style != TokenStyle::Comment)
        .map(|t| sql[t.range].trim().to_string())
        .filter(|s| !s.is_empty() && s != ";")
        .collect();
    let columns = shape(&ts, &info.table.name)?;
    let visible = |name: &String| {
        info.table
            .columns
            .iter()
            .any(|c| c.name == *name && c.hidden == 0)
    };
    if !info.table.binary_collations || !columns.iter().all(visible) || covered(info, &columns) {
        return None;
    }
    let name = format!("idx_{}_{}", info.table.name, columns.join("_"));
    let ordering = if columns.len() > 1 {
        " and ordering"
    } else {
        ""
    };
    let quoted: Vec<_> = columns.iter().map(|c| quote_identifier(c)).collect();
    Some(format!(
        "-- Candidate for equality filtering{ordering}; verify with EXPLAIN before and after.\nCREATE INDEX {} ON {} ({});",
        quote_identifier(&name),
        quote_identifier(&info.table.name),
        quoted.join(", ")
    ))
}
#[cfg(test)]
mod tests {
    use super::index_draft;
    use tursobench_core::Inspection;
    #[test]
    fn drafts_are_quoted_and_existing_or_ambiguous_indexes_are_not_suggested() {
        let mut info = Inspection {
            table: tursobench_core::Table {
                name: "odd table".into(),
                columns: vec![tursobench_core::TableColumn {
                    name: "a\"b".into(),
                    declared_type: "TEXT".into(),
                    not_null: false,
                    primary_key: 0,
                    hidden: 0,
                    default_sql: None,
                }],
                is_table: true,
                has_rowid: true,
                binary_collations: true,
            },
            indexes: vec![],
            foreign_keys: vec![],
        };
        let sql = "SELECT * FROM \"odd table\" WHERE \"a\"\"b\" = ?1";
        assert!(index_draft(sql, &info).unwrap().contains("(\"a\"\"b\")"));
        assert!(index_draft("SELECT * FROM \"odd table\" t WHERE t.x = 1", &info).is_none());
        info.indexes.push(tursobench_core::IndexInfo {
            name: "existing".into(),
            unique: false,
            partial: false,
            columns: vec![Some("a\"b".into())],
        });
        assert!(index_draft(sql, &info).is_none());
        info.indexes[0].partial = true;
        assert!(index_draft(sql, &info).is_some());
    }
}
