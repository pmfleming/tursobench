//! Turso parser/lexer-backed editor support. Never split statements on semicolons.
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};
use turso_parser::{lexer::Lexer, parser::Parser, token::TokenType};
use tursobench_core::{Bindings, Failure};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TokenStyle {
    Plain,
    Invalid,
    Keyword,
    String,
    Number,
    Comment,
    Parameter,
}
#[derive(Clone, Debug)]
pub struct SqlToken {
    pub range: Range<usize>,
    pub style: TokenStyle,
}
pub fn tokens(sql: &str) -> Vec<SqlToken> {
    let mut lexer = Lexer::new(sql.as_bytes());
    let mut result = Vec::new();
    while !lexer.remaining().is_empty() {
        let start = sql.len() - lexer.remaining().len();
        let Some(Ok(token)) = lexer.next() else {
            result.push(SqlToken {
                range: start..sql.len(),
                style: TokenStyle::Invalid,
            });
            break;
        };
        let style = match token.token_type {
            TokenType::TK_VARIABLE => TokenStyle::Parameter,
            TokenType::TK_STRING | TokenType::TK_BLOB => TokenStyle::String,
            TokenType::TK_INTEGER | TokenType::TK_FLOAT => TokenStyle::Number,
            _ if token.value.starts_with(b"--") || token.value.starts_with(b"/*") => {
                TokenStyle::Comment
            }
            _ if token.token_type != TokenType::TK_ID
                && token.value.first().is_some_and(u8::is_ascii_alphabetic) =>
            {
                TokenStyle::Keyword
            }
            _ => TokenStyle::Plain,
        };
        result.push(SqlToken {
            range: start..sql.len() - lexer.remaining().len(),
            style,
        });
    }
    result
}
pub fn parameter_names(sql: &str) -> Vec<String> {
    tokens(sql)
        .into_iter()
        .filter(|t| t.style == TokenStyle::Parameter)
        .map(|t| sql[t.range].to_owned())
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub fn validate_bindings(sql: &str, bindings: &Bindings) -> Result<(), Failure> {
    let params = parameter_names(sql);
    match bindings {
        Bindings::Named(values) => {
            if params.iter().any(|p| p.starts_with('?')) {
                return Err(Failure::input(
                    "Use a positional array for ? parameters; mixing named and positional bindings is not supported",
                ));
            }
            if params.iter().any(|p| !values.contains_key(p))
                || values.keys().any(|p| !params.contains(p))
            {
                return Err(Failure::input(format!(
                    "Named bindings must match exactly: {}",
                    params.join(", ")
                )));
            }
        }
        Bindings::Positional(values) => {
            if params.iter().any(|p| !p.starts_with('?')) {
                return Err(Failure::input(
                    "Use a JSON object with full parameter names for :name, $name or @name",
                ));
            }
            let mut count = 0usize;
            for token in tokens(sql)
                .into_iter()
                .filter(|t| t.style == TokenStyle::Parameter)
            {
                let p = &sql[token.range];
                count = if p == "?" {
                    count
                        .checked_add(1)
                        .ok_or_else(|| Failure::input("Too many parameters"))?
                } else {
                    count.max(
                        p[1..]
                            .parse::<usize>()
                            .map_err(|_| Failure::input("Invalid positional parameter"))?,
                    )
                };
            }
            if count != values.len() {
                return Err(Failure::input(format!(
                    "Expected {count} positional values, received {}",
                    values.len()
                )));
            }
        }
    }
    Ok(())
}
pub fn selection_or_statement(sql: &str, selection: Range<usize>) -> Result<String, Failure> {
    if sql.len() > 1024 * 1024
        || selection.end > sql.len()
        || selection.start > selection.end
        || !sql.is_char_boundary(selection.start)
        || !sql.is_char_boundary(selection.end)
    {
        return Err(Failure::input("Invalid SQL selection or SQL exceeds 1 MiB"));
    }
    if !selection.is_empty() {
        let selected = &sql[selection];
        validate_single(selected)?;
        return Ok(selected.to_owned());
    }
    let mut parser = Parser::new(sql.as_bytes());
    let mut ranges = Vec::new();
    let mut start = 0;
    while parser
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?
        .is_some()
    {
        let end = parser.offset();
        ranges.push(start..end);
        start = end;
    }
    let range = ranges
        .iter()
        .find(|r| r.contains(&selection.start))
        .or_else(|| ranges.last().filter(|r| selection.start >= r.end))
        .ok_or_else(|| Failure::input("No statement at cursor"))?;
    let statement = sql[range.clone()].to_owned();
    validate_single(&statement)?;
    Ok(statement)
}
pub fn explain_sql(sql: &str, bytecode: bool) -> Result<String, Failure> {
    validate_single(sql)?;
    use turso_parser::ast::{Cmd, Stmt};
    let command = Parser::new(sql.as_bytes())
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?;
    if !matches!(
        command,
        Some(Cmd::Stmt(
            Stmt::Select(_) | Stmt::Insert { .. } | Stmt::Update(_) | Stmt::Delete { .. }
        ))
    ) {
        return Err(Failure::input(
            "EXPLAIN supports SELECT/INSERT/UPDATE/DELETE only; transaction, DDL and PRAGMA commands are not executed",
        ));
    }
    Ok(format!(
        "{} {sql}",
        if bytecode {
            "EXPLAIN"
        } else {
            "EXPLAIN QUERY PLAN"
        }
    ))
}

pub fn named_template(sql: &str) -> BTreeMap<String, tursobench_core::Value> {
    parameter_names(sql)
        .into_iter()
        .filter(|p| !p.starts_with('?'))
        .map(|p| (p, tursobench_core::Value::Null))
        .collect()
}

pub fn validate_single(sql: &str) -> Result<(), Failure> {
    if sql.len() > 1024 * 1024 {
        return Err(Failure::input("SQL exceeds 1 MiB"));
    }
    let mut parser = turso_parser::parser::Parser::new(sql.as_bytes());
    if parser
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?
        .is_none()
    {
        return Err(Failure::input("Enter one SQL statement"));
    }
    if parser
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?
        .is_some()
    {
        return Err(Failure::input(
            "Scripts are not supported; execute one statement at a time",
        ));
    }
    Ok(())
}

pub(crate) fn stoppable_read(sql: &str) -> bool {
    use turso_parser::ast::{Cmd, Stmt};
    matches!(
        Parser::new(sql.as_bytes()).next_cmd(),
        Ok(Some(
            Cmd::Stmt(Stmt::Select(_)) | Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. }
        ))
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Read,
    Data,
    Schema,
    Rollback,
}

pub(crate) fn effect(sql: &str) -> Effect {
    use turso_parser::ast::{Cmd, Stmt};
    match Parser::new(sql.as_bytes()).next_cmd() {
        Ok(Some(Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. })) => Effect::Read,
        Ok(Some(Cmd::Stmt(stmt))) => match stmt {
            Stmt::Select(_)
            | Stmt::Begin { .. }
            | Stmt::Commit { .. }
            | Stmt::Release { .. }
            | Stmt::Savepoint { .. } => Effect::Read,
            Stmt::Rollback {
                savepoint_name: None,
                ..
            } => Effect::Rollback,
            Stmt::Rollback { .. } => Effect::Read,
            Stmt::Insert { .. } | Stmt::Update(_) | Stmt::Delete { .. } => Effect::Data,
            Stmt::Pragma { body: None, .. } => Effect::Read,
            _ => Effect::Schema,
        },
        _ => Effect::Read,
    }
}

pub(crate) fn validate_readonly(sql: &str) -> Result<(), Failure> {
    use turso_parser::ast::{Cmd, Stmt};
    let command = Parser::new(sql.as_bytes())
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?;
    if matches!(
        command,
        Some(Cmd::Stmt(Stmt::Attach { .. } | Stmt::Vacuum { .. }))
    ) {
        return Err(Failure {
            kind: tursobench_core::ErrorKind::ReadOnly,
            message:
                "Read-only sessions cannot attach databases or run VACUUM, including VACUUM INTO"
                    .into(),
        });
    }
    Ok(())
}

/// The inline INTEGER PRIMARY KEY (except DESC) aliases rowid and cannot be NULL.
pub(crate) fn inline_rowid_key(ddl: &str) -> Option<String> {
    use turso_parser::ast::{Cmd, ColumnConstraint, CreateTableBody, SortOrder, Stmt};
    let Ok(Some(Cmd::Stmt(Stmt::CreateTable {
        body: CreateTableBody::ColumnsAndConstraints {
            columns, options, ..
        },
        ..
    }))) = Parser::new(ddl.as_bytes()).next_cmd()
    else {
        return None;
    };
    if options.contains_without_rowid() {
        return None;
    }
    columns
        .into_iter()
        .find(|c| {
            c.col_type
                .as_ref()
                .is_some_and(|t| t.name.eq_ignore_ascii_case("INTEGER") && t.size.is_none())
                && c.constraints.iter().any(|c| {
                    matches!(
                        c.constraint,
                        ColumnConstraint::PrimaryKey {
                            order: None | Some(SortOrder::Asc),
                            ..
                        }
                    )
                })
        })
        .map(|c| c.col_name.as_str().to_owned())
}

/// Conservatively establish physical row identity from the matching engine parser.
pub(crate) fn has_rowid(ddl: &str) -> bool {
    use turso_parser::ast::{Cmd, CreateTableBody, Stmt};
    matches!(Parser::new(ddl.as_bytes()).next_cmd(),
        Ok(Some(Cmd::Stmt(Stmt::CreateTable {
            body: CreateTableBody::ColumnsAndConstraints { options, .. }, ..
        }))) if !options.contains_without_rowid())
}

#[cfg(test)]
mod tests {
    use super::{named_template, parameter_names, selection_or_statement, validate_bindings};
    use std::collections::BTreeMap;
    use tursobench_core::Bindings;
    #[test]
    fn selection_handles_unicode_comments_and_trigger_bodies() {
        let sql = "SELECT 'λ;'; /* ; */ CREATE TRIGGER t AFTER INSERT ON x BEGIN UPDATE x SET v=';'; DELETE FROM x; END; SELECT 3;";
        let cursor = sql.find("UPDATE").unwrap();
        let selected = selection_or_statement(sql, cursor..cursor).unwrap();
        assert!(selected.contains("CREATE TRIGGER"));
        assert!(selected.contains("DELETE FROM x"));
        assert!(!selected.contains("SELECT 3"));
        assert!(selection_or_statement(sql, 0..sql.len()).is_err());
        assert_eq!(
            selection_or_statement("SELECT 'λ'", 0..0).unwrap(),
            "SELECT 'λ'"
        );
        assert!(selection_or_statement("SELECT 'λ'", 9..9).is_err());
    }
    #[test]
    fn bindings_ignore_literals_comments_and_repeat_names() {
        let sql = "SELECT :name, :name, $other, ':ignored' /* @ignored */";
        assert_eq!(parameter_names(sql), vec!["$other", ":name"]);
        assert!(validate_bindings(sql, &Bindings::Named(named_template(sql))).is_ok());
        assert!(validate_bindings(sql, &Bindings::Named(BTreeMap::new())).is_err());
        assert!(
            validate_bindings(
                "SELECT ?2, ?",
                &Bindings::Positional(vec![tursobench_core::Value::Null; 3])
            )
            .is_ok()
        );
    }
    #[test]
    fn rowid_capability_uses_structure_not_text_matches() {
        assert!(super::has_rowid(
            "CREATE TABLE t(id INTEGER PRIMARY KEY, x TEXT DEFAULT 'WITHOUT ROWID')"
        ));
        assert!(!super::has_rowid(
            "CREATE TABLE t(id TEXT PRIMARY KEY) WITHOUT ROWID"
        ));
        assert!(!super::has_rowid("CREATE VIEW t AS SELECT 1"));
        assert!(!super::has_rowid("invalid schema"));
    }
}
