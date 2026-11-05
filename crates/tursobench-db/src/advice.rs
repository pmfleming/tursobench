//! Conservative index drafts for a deliberately narrow, validated query shape.
use crate::sql::{self, TokenStyle};
use tursobench_core::{Inspection, quote_identifier};

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
    if !ts.first()?.eq_ignore_ascii_case("SELECT")
        || ts.iter().any(|t| {
            matches!(
                t.to_uppercase().as_str(),
                "JOIN" | "WITH" | "OR" | "UNION" | "COLLATE"
            ) || t == "("
        })
    {
        return None;
    }
    let from = ts.iter().position(|t| t.eq_ignore_ascii_case("FROM"))?;
    let name = |s: &str| {
        s.strip_prefix('"')
            .and_then(|s| s.strip_suffix('"'))
            .unwrap_or(s)
            .replace("\"\"", "\"")
    };
    if !name(ts.get(from + 1)?).eq_ignore_ascii_case(&info.table.name)
        || !ts.get(from + 2)?.eq_ignore_ascii_case("WHERE")
    {
        return None;
    }
    let column = name(ts.get(from + 3)?);
    if ts.get(from + 4)? != "=" {
        return None;
    }
    let value = ts.get(from + 5)?;
    if !(value.starts_with(['?', ':', '$', '@', '\''])
        || value.parse::<f64>().is_ok()
        || value.eq_ignore_ascii_case("NULL"))
    {
        return None;
    }
    let mut columns = vec![column];
    let tail = &ts[from + 6..];
    if !tail.is_empty() {
        if !(tail.len() == 3 || tail.len() == 4)
            || !tail[0].eq_ignore_ascii_case("ORDER")
            || !tail[1].eq_ignore_ascii_case("BY")
            || (tail.len() == 4 && !tail[3].eq_ignore_ascii_case("ASC"))
        {
            return None;
        }
        let order = name(&tail[2]);
        if !columns.contains(&order) {
            columns.push(order);
        }
    }
    if !info.table.binary_collations
        || columns.iter().any(|name| {
            !info
                .table
                .columns
                .iter()
                .any(|c| c.name == *name && c.hidden == 0)
        })
    {
        return None;
    }
    let keys: Vec<_> = info
        .table
        .keys()
        .iter()
        .map(|i| info.table.columns[*i].name.as_str())
        .collect();
    if keys.starts_with(&columns.iter().map(String::as_str).collect::<Vec<_>>())
        || info.indexes.iter().any(|i| {
            !i.partial
                && i.columns
                    .iter()
                    .take(columns.len())
                    .map(Option::as_deref)
                    .eq(columns.iter().map(|c| Some(c.as_str())))
        })
    {
        return None;
    }
    let name = format!("idx_{}_{}", info.table.name, columns.join("_"));
    Some(format!(
        "-- Candidate for equality filtering{}; verify with EXPLAIN before and after.\nCREATE INDEX {} ON {} ({});",
        if columns.len() > 1 {
            " and ordering"
        } else {
            ""
        },
        quote_identifier(&name),
        quote_identifier(&info.table.name),
        columns
            .iter()
            .map(|c| quote_identifier(c))
            .collect::<Vec<_>>()
            .join(", ")
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
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
