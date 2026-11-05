//! Tolerant lexical scopes for incomplete SQL. Never performs database I/O.
use crate::sql::{TokenStyle, tokens};
use std::collections::{BTreeMap, BTreeSet};
use tursobench_core::quote_identifier;

pub type Catalog = BTreeMap<String, Vec<String>>;
struct Token {
    name: String,
    upper: String,
    end: usize,
    scope: usize,
    identifier: bool,
}
impl Token {
    fn declares_table(&self) -> bool {
        matches!(self.upper.as_str(), "FROM" | "JOIN" | "UPDATE" | "INTO")
    }
}
#[derive(Default)]
struct Scope {
    parent: Option<usize>,
    aliases: BTreeMap<String, String>,
    ctes: Catalog,
}
fn identifier(raw: &str) -> String {
    match raw.as_bytes() {
        [b'"', .., b'"'] => raw[1..raw.len() - 1].replace("\"\"", "\""),
        [b'`', .., b'`'] => raw[1..raw.len() - 1].replace("``", "`"),
        _ => raw.into(),
    }
}
fn ancestors(scopes: &[Scope], scope: usize) -> impl Iterator<Item = &Scope> {
    std::iter::successors(Some(scope), |&i| scopes[i].parent).map(|i| &scopes[i])
}
fn columns<'a>(
    scopes: &'a [Scope],
    scope: usize,
    catalog: &'a Catalog,
    table: &str,
) -> &'a [String] {
    ancestors(scopes, scope)
        .map(|s| &s.ctes)
        .chain(std::iter::once(catalog))
        .flat_map(|c| c.iter())
        .find(|(name, _)| name.eq_ignore_ascii_case(table))
        .map_or(&[], |(_, v)| v)
}
fn scoped_tokens(sql: &str, cursor: usize) -> (Vec<Token>, Vec<Scope>, usize) {
    let mut scopes = vec![Scope::default()];
    let (mut current, mut cursor_scope) = (0, 0);
    let mut ts = Vec::new();
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
        ts.push(Token {
            name: identifier(raw),
            upper: raw.to_uppercase(),
            end: t.range.end,
            scope: current,
            identifier: t.style == TokenStyle::Plain
                && (raw.starts_with(['"', '`'])
                    || raw
                        .chars()
                        .next()
                        .is_some_and(|c| c.is_alphabetic() || c == '_')),
        });
        if matches!(raw, "(" | ";") {
            scopes.push(Scope {
                parent: (raw == "(").then_some(current),
                ..Default::default()
            });
            current = scopes.len() - 1;
            if t.range.end <= cursor {
                cursor_scope = current;
            }
        }
    }
    (ts, scopes, cursor_scope)
}
fn collect_aliases(ts: &[Token], scopes: &mut [Scope]) {
    let mut table_list = BTreeSet::new();
    for (i, t) in ts.iter().enumerate() {
        if matches!(
            t.upper.as_str(),
            "WHERE" | "ON" | "GROUP" | "ORDER" | "LIMIT" | "SET" | "SELECT" | "UNION"
        ) {
            table_list.remove(&t.scope);
        }
        if !(t.declares_table() || t.name == "," && table_list.contains(&t.scope)) {
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
        next += usize::from(ts.get(next).is_some_and(|n| n.upper == "AS"));
        let name = ts
            .get(next)
            .filter(|n| n.identifier && n.scope == t.scope)
            .unwrap_or(table);
        scopes[t.scope]
            .aliases
            .insert(name.name.to_lowercase(), table.name.clone());
    }
}
fn projection(expr: &[&Token], scopes: &[Scope], scope: usize, catalog: &Catalog) -> Vec<String> {
    if let Some(pos) = expr.iter().position(|t| t.upper == "AS") {
        return expr
            .get(pos + 1)
            .filter(|t| t.identifier)
            .map(|t| t.name.clone())
            .into_iter()
            .collect();
    }
    if expr.iter().any(|t| t.name == "*") {
        return scopes[scope]
            .aliases
            .values()
            .flat_map(|table| columns(scopes, scope, catalog, table))
            .cloned()
            .collect();
    }
    if expr.len() == 1 || expr.len() == 3 && expr[1].name == "." {
        return expr
            .last()
            .filter(|t| t.identifier)
            .map(|t| t.name.clone())
            .into_iter()
            .collect();
    }
    vec![]
}
fn cte_columns(ts: &[Token], i: usize, scopes: &[Scope], catalog: &Catalog) -> Option<Vec<String>> {
    let name = &ts[i];
    let previous = i.checked_sub(1).and_then(|n| ts.get(n))?;
    if !name.identifier || !matches!(previous.upper.as_str(), "WITH" | "RECURSIVE" | ",") {
        return None;
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
    if ts.get(next)?.upper != "AS" || ts.get(next + 1)?.name != "(" {
        return None;
    }
    let select = ts.get(next + 2).filter(|t| t.upper == "SELECT")?;
    if !projected.is_empty() {
        return Some(projected);
    }
    let exprs: Vec<_> = ts[next + 3..]
        .iter()
        .take_while(|t| t.scope != name.scope && !(t.scope == select.scope && t.upper == "FROM"))
        .filter(|t| t.scope == select.scope)
        .collect();
    Some(
        exprs
            .split(|t| t.name == ",")
            .flat_map(|expr| projection(expr, scopes, select.scope, catalog))
            .collect(),
    )
}
fn names<'a>(
    ts: &[Token],
    scopes: &'a [Scope],
    scope: usize,
    prefix_start: usize,
    catalog: &'a Catalog,
) -> (Vec<&'a str>, bool) {
    let mut prior = ts.iter().rev().filter(|t| t.end <= prefix_start);
    let last = prior.next();
    if last.is_some_and(|t| t.name == ".") {
        let table = prior.next().and_then(|q| {
            ancestors(scopes, scope).find_map(|s| s.aliases.get(&q.name.to_lowercase()))
        });
        return (
            table.map_or(vec![], |t| {
                columns(scopes, scope, catalog, t)
                    .iter()
                    .map(String::as_str)
                    .collect()
            }),
            true,
        );
    }
    let tables_only = last.is_some_and(Token::declares_table);
    let mut values = vec![];
    for (id, s) in
        std::iter::successors(Some(scope), |&i| scopes[i].parent).map(|i| (i, &scopes[i]))
    {
        if !tables_only {
            values.extend(
                s.aliases
                    .values()
                    .flat_map(|t| columns(scopes, id, catalog, t))
                    .map(String::as_str),
            );
        }
        values.extend(s.ctes.keys().map(String::as_str));
    }
    values.extend(catalog.keys().map(String::as_str));
    (values, false)
}
/// Context-ranked, quoted identifier candidates followed by keyword/function fallbacks.
pub fn candidates(sql: &str, cursor: usize, prefix_start: usize, catalog: &Catalog) -> Vec<String> {
    if sql.len() > 64 * 1024
        || prefix_start > cursor
        || !sql.is_char_boundary(prefix_start)
        || !sql.is_char_boundary(cursor)
    {
        return vec![];
    }
    let (ts, mut scopes, scope) = scoped_tokens(sql, cursor);
    collect_aliases(&ts, &mut scopes);
    for i in 0..ts.len() {
        if let Some(projected) = cte_columns(&ts, i, &scopes, catalog) {
            scopes[ts[i].scope]
                .ctes
                .insert(ts[i].name.clone(), projected);
        }
    }
    let (values, qualified) = names(&ts, &scopes, scope, prefix_start, catalog);
    let prefix = sql[prefix_start..cursor].to_lowercase();
    let mut seen = BTreeSet::new();
    let mut result: Vec<_> = values
        .into_iter()
        .filter(|v| {
            let lower = v.to_lowercase();
            lower.starts_with(&prefix) && seen.insert(lower)
        })
        .map(quote_identifier)
        .take(30)
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
    use super::{Catalog, candidates};
    #[test]
    fn completion_preserves_scope_quoting_and_incomplete_input_boundaries() {
        let catalog: Catalog = [
            ("users".into(), vec!["id".into(), "name".into()]),
            ("orders".into(), vec!["total".into()]),
            ("odd table".into(), vec!["a\"b".into()]),
        ]
        .into();
        for (text, expected) in [
            ("SELECT u.| FROM users AS u", vec!["\"id\"", "\"name\""]),
            (
                "SELECT o.to| FROM users u JOIN orders o ON",
                vec!["\"total\""],
            ),
            ("SELECT x.| FROM \"odd table\" x", vec!["\"a\"\"b\""]),
            ("SELECT * FROM us|", vec!["\"users\""]),
            (
                "WITH recent AS (SELECT * FROM users) SELECT * FROM re|",
                vec!["\"recent\"", "RETURNING"],
            ),
            (
                "SELECT (SELECT u.| FROM orders u) FROM users u",
                vec!["\"total\""],
            ),
            (
                "SELECT z.| FROM users u WHERE EXISTS (SELECT 1 FROM orders z)",
                vec![],
            ),
            ("SELECT * FROM users u; SELECT u.|", vec![]),
            (
                "SELECT (SELECT u.| FROM orders o) FROM users u",
                vec!["\"id\"", "\"name\""],
            ),
            (
                "WITH recent(uid) AS (SELECT id FROM users) SELECT r.| FROM recent r",
                vec!["\"uid\""],
            ),
            (
                "WITH recent AS (SELECT id AS uid, name FROM users) SELECT r.| FROM recent r",
                vec!["\"uid\"", "\"name\""],
            ),
            (
                "WITH recent AS (SELECT * FROM users) SELECT r.| FROM recent r",
                vec!["\"id\"", "\"name\""],
            ),
            (
                "SELECT λ.| FROM main.users AS λ",
                vec!["\"id\"", "\"name\""],
            ),
            ("SELECT x.| FROM \"", vec![]),
            ("SELECT x.| FROM `", vec![]),
            ("SELECT x.| FROM [users] x", vec![]),
            ("SELECT x.| FROM `users` x", vec!["\"id\"", "\"name\""]),
            ("SELECT o.| FROM users u, orders o", vec!["\"total\""]),
            (
                "WITH r AS (SELECT id+1 FROM users) SELECT r.| FROM r",
                vec![],
            ),
        ] {
            let cursor = text.find('|').unwrap();
            let sql = text.replace('|', "");
            let start = sql[..cursor]
                .char_indices()
                .rev()
                .find(|(_, c)| !c.is_alphanumeric() && *c != '_')
                .map_or(0, |(i, c)| i + c.len_utf8());
            assert_eq!(
                candidates(&sql, cursor, start, &catalog),
                expected,
                "{text}"
            );
        }
        for (sql, cursor, start) in [
            ("λ", 1, 0),
            ("λ", 2, 1),
            ("a", 2, 0),
            ("a", 0, 1),
            (&"x".repeat(65537), 0, 0),
        ] {
            assert!(candidates(sql, cursor, start, &catalog).is_empty());
        }
        assert!(candidates("", 0, 0, &catalog).len() <= 30);
    }
}
