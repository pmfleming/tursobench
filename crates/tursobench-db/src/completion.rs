//! Tolerant lexical scopes for incomplete SQL. Never performs database I/O.
use crate::sql::{TokenStyle, tokens};
use std::collections::{BTreeMap, BTreeSet};
use tursobench_core::quote_identifier;

pub type Catalog = BTreeMap<String, Vec<String>>;
#[derive(Clone)]
struct Token {
    name: String,
    upper: String,
    end: usize,
    scope: usize,
    identifier: bool,
}
#[derive(Default)]
struct Scope {
    parent: Option<usize>,
    aliases: BTreeMap<String, String>,
    ctes: Catalog,
}
fn identifier(raw: &str) -> String {
    if raw.starts_with('"') && raw.ends_with('"') {
        raw[1..raw.len() - 1].replace("\"\"", "\"")
    } else if raw.starts_with('`') && raw.ends_with('`') {
        raw[1..raw.len() - 1].replace("``", "`")
    } else if raw.starts_with('[') && raw.ends_with(']') {
        raw[1..raw.len() - 1].to_string()
    } else {
        raw.to_string()
    }
}
fn catalog_columns(catalog: &Catalog, name: &str) -> Vec<String> {
    catalog
        .iter()
        .find(|(key, _)| key.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.clone())
        .unwrap_or_default()
}
fn columns(scopes: &[Scope], mut scope: usize, catalog: &Catalog, table: &str) -> Vec<String> {
    loop {
        if let Some((_, v)) = scopes[scope]
            .ctes
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(table))
        {
            return v.clone();
        }
        match scopes[scope].parent {
            Some(parent) => scope = parent,
            None => break,
        }
    }
    catalog_columns(catalog, table)
}
fn alias(scopes: &[Scope], mut scope: usize, name: &str) -> Option<String> {
    loop {
        if let Some(table) = scopes[scope].aliases.get(&name.to_lowercase()) {
            return Some(table.clone());
        }
        scope = scopes[scope].parent?;
    }
}
/// Context-ranked, quoted identifier candidates followed by keyword/function fallbacks.
pub fn candidates(sql: &str, cursor: usize, prefix_start: usize, catalog: &Catalog) -> Vec<String> {
    if sql.len() > 64 * 1024
        || prefix_start > cursor
        || cursor > sql.len()
        || !sql.is_char_boundary(prefix_start)
        || !sql.is_char_boundary(cursor)
    {
        return vec![];
    }
    let mut scopes = vec![Scope::default()];
    let mut current = 0;
    let mut ts = Vec::new();
    let mut cursor_scope = 0;
    for t in tokens(sql) {
        let raw = sql[t.range.clone()].trim();
        if raw.is_empty()
            || matches!(
                t.style,
                TokenStyle::Comment | TokenStyle::String | TokenStyle::Parameter
            )
        {
            continue;
        }
        if raw == ")" {
            current = scopes[current].parent.unwrap_or(0);
        }
        if t.range.start < cursor {
            cursor_scope = current;
        }
        let is_id = matches!(t.style, TokenStyle::Plain)
            && (raw.starts_with(['"', '`', '['])
                || raw
                    .chars()
                    .next()
                    .is_some_and(|c| c.is_alphabetic() || c == '_'));
        ts.push(Token {
            name: identifier(raw),
            upper: raw.to_uppercase(),
            end: t.range.end,
            scope: current,
            identifier: is_id,
        });
        if raw == "(" {
            scopes.push(Scope {
                parent: Some(current),
                ..Default::default()
            });
            current = scopes.len() - 1;
            if t.range.end <= cursor {
                cursor_scope = current;
            }
        } else if raw == ";" {
            scopes.push(Scope::default());
            current = scopes.len() - 1;
            if t.range.end <= cursor {
                cursor_scope = current;
            }
        }
    }
    // FROM/JOIN/UPDATE/INTO declarations, including comma-separated FROM lists.
    let mut table_list = BTreeSet::new();
    for (i, t) in ts.iter().enumerate() {
        if matches!(
            t.upper.as_str(),
            "WHERE" | "ON" | "GROUP" | "ORDER" | "LIMIT" | "SET" | "SELECT" | "UNION"
        ) {
            table_list.remove(&t.scope);
        }
        let declaration = matches!(t.upper.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO")
            || (t.name == "," && table_list.contains(&t.scope));
        if !declaration {
            continue;
        }
        table_list.insert(t.scope);
        let Some(table) = ts.get(i + 1).filter(|n| n.scope == t.scope && n.identifier) else {
            continue;
        };
        let mut next = i + 2;
        let table = if ts.get(next).is_some_and(|n| n.name == ".") {
            next += 2;
            ts.get(next - 1).unwrap_or(table)
        } else {
            table
        };
        if ts.get(next).is_some_and(|n| n.upper == "AS") {
            next += 1;
        }
        let name = ts
            .get(next)
            .filter(|n| n.identifier && n.scope == t.scope)
            .unwrap_or(table);
        scopes[t.scope]
            .aliases
            .insert(name.name.to_lowercase(), table.name.clone());
    }
    // CTEs: explicit column lists or simple projected names/AS aliases. Unknown
    // expressions stay unknown instead of inventing a column name.
    for i in 0..ts.len() {
        if !ts[i].identifier {
            continue;
        }
        let name = &ts[i];
        let previous = i.checked_sub(1).and_then(|n| ts.get(n));
        if !previous.is_some_and(|p| matches!(p.upper.as_str(), "WITH" | "RECURSIVE" | ",")) {
            continue;
        }
        let mut next = i + 1;
        let mut projected = vec![];
        if ts.get(next).is_some_and(|t| t.name == "(") {
            next += 1;
            while let Some(t) = ts.get(next).filter(|t| t.name != ")") {
                if t.identifier {
                    projected.push(t.name.clone());
                }
                next += 1;
            }
            next += 1;
        }
        if ts.get(next).is_none_or(|t| t.upper != "AS")
            || ts.get(next + 1).is_none_or(|t| t.name != "(")
        {
            continue;
        }
        let Some(select) = ts.get(next + 2).filter(|t| t.upper == "SELECT") else {
            continue;
        };
        if projected.is_empty() {
            let projection: Vec<_> = ts[next + 3..]
                .iter()
                .take_while(|t| {
                    t.scope != name.scope && !(t.scope == select.scope && t.upper == "FROM")
                })
                .filter(|t| t.scope == select.scope)
                .collect();
            for expr in projection.split(|t| t.name == ",") {
                if let Some(pos) = expr.iter().position(|t| t.upper == "AS") {
                    if let Some(t) = expr.get(pos + 1).filter(|t| t.identifier) {
                        projected.push(t.name.clone());
                    }
                } else if expr.iter().any(|t| t.name == "*") {
                    for table in scopes[select.scope].aliases.values() {
                        projected.extend(columns(&scopes, select.scope, catalog, table));
                    }
                } else if (expr.len() == 1 || (expr.len() == 3 && expr[1].name == "."))
                    && let Some(t) = expr.last().filter(|t| t.identifier)
                {
                    projected.push(t.name.clone());
                }
            }
        }
        scopes[name.scope].ctes.insert(name.name.clone(), projected);
    }
    let prior: Vec<_> = ts.iter().filter(|t| t.end <= prefix_start).collect();
    let prefix = sql[prefix_start..cursor].to_lowercase();
    let mut values = Vec::new();
    let qualified = prior.last().is_some_and(|t| t.name == ".");
    if qualified {
        if let Some(qualifier) = prior.iter().rev().nth(1)
            && let Some(table) = alias(&scopes, cursor_scope, &qualifier.name)
        {
            values.extend(columns(&scopes, cursor_scope, catalog, &table));
        }
    } else {
        let tables_only = prior
            .last()
            .is_some_and(|t| matches!(t.upper.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO"));
        let mut scope = cursor_scope;
        loop {
            if !tables_only {
                for table in scopes[scope].aliases.values() {
                    values.extend(columns(&scopes, scope, catalog, table));
                }
            }
            values.extend(scopes[scope].ctes.keys().cloned());
            match scopes[scope].parent {
                Some(parent) => scope = parent,
                None => break,
            }
        }
        values.extend(catalog.keys().cloned());
    }
    let mut seen = BTreeSet::new();
    let mut result: Vec<_> = values
        .into_iter()
        .filter(|v| v.to_lowercase().starts_with(&prefix) && seen.insert(v.to_lowercase()))
        .map(|v| quote_identifier(&v))
        .collect();
    if !qualified {
        result.extend(
            [
                "SELECT",
                "FROM",
                "WHERE",
                "JOIN",
                "ON",
                "ORDER BY",
                "GROUP BY",
                "LIMIT",
                "INSERT INTO",
                "UPDATE",
                "DELETE FROM",
                "RETURNING",
                "NULL",
                "fts_match",
                "fts_score",
                "uuid4",
                "uuid7",
                "vector_distance_cos",
            ]
            .into_iter()
            .filter(|v| v.to_lowercase().starts_with(&prefix))
            .map(str::to_string),
        );
    }
    result.truncate(30);
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    fn complete(s: &str) -> Vec<String> {
        let cursor = s.find('|').unwrap();
        let sql = s.replace('|', "");
        let start = sql[..cursor]
            .char_indices()
            .rev()
            .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
            .map_or(0, |(i, c)| i + c.len_utf8());
        candidates(
            &sql,
            cursor,
            start,
            &[
                ("users".into(), vec!["id".into(), "name".into()]),
                ("orders".into(), vec!["total".into()]),
                ("odd table".into(), vec!["a\"b".into()]),
            ]
            .into(),
        )
    }
    #[test]
    fn aliases_quotes_join_and_incomplete_statements() {
        assert_eq!(
            complete("SELECT u.| FROM users AS u"),
            vec!["\"id\"", "\"name\""]
        );
        assert_eq!(
            complete("SELECT o.to| FROM users u JOIN orders o ON"),
            vec!["\"total\""]
        );
        assert_eq!(
            complete("SELECT x.| FROM \"odd table\" x"),
            vec!["\"a\"\"b\""]
        );
        assert!(complete("SELECT * FROM us|").contains(&"\"users\"".into()));
    }
    #[test]
    fn nested_scopes_shadow_and_do_not_leak_across_statements() {
        assert_eq!(
            complete("SELECT (SELECT u.| FROM orders u) FROM users u"),
            vec!["\"total\""]
        );
        assert!(
            complete("SELECT z.| FROM users u WHERE EXISTS (SELECT 1 FROM orders z)").is_empty()
        );
        assert!(complete("SELECT * FROM users u; SELECT u.|").is_empty());
        assert_eq!(
            complete("SELECT (SELECT u.| FROM orders o) FROM users u"),
            vec!["\"id\"", "\"name\""]
        );
    }
    #[test]
    fn cte_names_and_projected_columns() {
        assert_eq!(
            complete("WITH recent(uid) AS (SELECT id FROM users) SELECT r.| FROM recent r"),
            vec!["\"uid\""]
        );
        assert_eq!(
            complete("WITH recent AS (SELECT id AS uid, name FROM users) SELECT r.| FROM recent r"),
            vec!["\"uid\"", "\"name\""]
        );
        assert!(
            complete("WITH recent AS (SELECT * FROM users) SELECT * FROM re|")
                .contains(&"\"recent\"".into())
        );
    }
}
