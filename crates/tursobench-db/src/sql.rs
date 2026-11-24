//! Turso parser/lexer-backed editor support. Never split statements on semicolons.
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};
use turso_parser::{ast::Cmd, lexer::Lexer, parser::Parser, token::TokenType};
use tursobench_core::{Bindings, Failure};

/// The first command in `sql`; a parse error is an input failure.
pub(crate) fn parse_first(sql: &str) -> Result<Option<Cmd>, Failure> {
    Parser::new(sql.as_bytes())
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))
}
/// The first command in `sql`, if it parses.
pub(crate) fn first_command(sql: &str) -> Option<Cmd> {
    Parser::new(sql.as_bytes()).next_cmd().ok().flatten()
}

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
            let count = positional_count(sql)?;
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
/// Values `sql` takes: one per `?`, and at least the highest `?N`.
fn positional_count(sql: &str) -> Result<usize, Failure> {
    tokens(sql)
        .into_iter()
        .filter(|t| t.style == TokenStyle::Parameter)
        .try_fold(0usize, |count, token| match &sql[token.range] {
            "?" => count
                .checked_add(1)
                .ok_or_else(|| Failure::input("Too many parameters")),
            p => p[1..]
                .parse::<usize>()
                .map(|n| count.max(n))
                .map_err(|_| Failure::input("Invalid positional parameter")),
        })
}
/// Bindings that match `sql` and fit the 8 MiB statement parameter budget.
pub(crate) fn validate_parameters(sql: &str, bindings: &Bindings) -> Result<(), Failure> {
    validate_bindings(sql, bindings)?;
    if bindings.bytes() > 8 * 1024 * 1024 {
        return Err(Failure::input("Parameters exceed 8 MiB"));
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
    let ranges = statement_ranges(sql);
    let range = ranges
        .iter()
        .find(|r| r.contains(&selection.start))
        .or_else(|| ranges.last().filter(|r| selection.start >= r.end))
        .ok_or_else(|| Failure::input("No statement at cursor"))?;
    let statement = sql[range.clone()].to_owned();
    validate_single(&statement)?;
    Ok(statement)
}
/// Whether the lexer is inside a CREATE TRIGGER body, where semicolons
/// separate body statements rather than script statements.
#[derive(Default)]
struct TriggerBody {
    create: bool,
    trigger: bool,
    body: usize,
    cases: usize,
}
impl TriggerBody {
    fn keyword(&mut self, text: &str) {
        let is = |keyword: &str| text.eq_ignore_ascii_case(keyword);
        if is("CREATE") {
            self.create = true;
        }
        if self.create && is("TRIGGER") {
            self.trigger = true;
        }
        if !self.trigger {
            return;
        }
        if is("BEGIN") {
            self.body += 1;
        } else if is("CASE") {
            self.cases += 1;
        } else if is("END") {
            self.end();
        }
    }
    /// `END` closes an open CASE first, then the BEGIN body.
    fn end(&mut self) {
        if self.cases > 0 {
            self.cases -= 1;
        } else if self.body > 0 {
            self.body -= 1;
            self.trigger = self.body > 0;
        }
    }
    /// A semicolon ends the statement unless it is inside a trigger body.
    fn ends_statement(&mut self) -> bool {
        if self.trigger {
            return false;
        }
        self.create = false;
        true
    }
}
/// Lexer boundaries retain trigger bodies and quoted/comment semicolons even if another statement is malformed.
fn statement_ranges(sql: &str) -> Vec<Range<usize>> {
    let mut ranges = Vec::new();
    let mut start = 0;
    let mut trigger = TriggerBody::default();
    for token in tokens(sql) {
        let text = sql[token.range.clone()].trim();
        if token.style == TokenStyle::Keyword {
            trigger.keyword(text);
        }
        if text == ";" && trigger.ends_statement() {
            ranges.push(start..token.range.end);
            start = token.range.end;
        }
    }
    if !sql[start..].trim().is_empty() {
        ranges.push(start..sql.len());
    }
    ranges
}
pub fn script_statements(sql: &str) -> Result<Vec<String>, Failure> {
    if sql.len() > 1024 * 1024 {
        return Err(Failure::input("Script exceeds 1 MiB"));
    }
    let mut statements = Vec::new();
    for range in statement_ranges(sql) {
        let text = &sql[range];
        if parse_first(text)?.is_none() {
            continue;
        }
        validate_single(text)?;
        statements.push(text.to_owned());
        if statements.len() > 100 {
            return Err(Failure::input("Scripts are limited to 100 statements"));
        }
    }
    if statements.is_empty() {
        return Err(Failure::input("Enter at least one statement"));
    }
    Ok(statements)
}
pub(crate) fn transaction_control(sql: &str) -> bool {
    use turso_parser::ast::Stmt;
    matches!(
        first_command(sql),
        Some(Cmd::Stmt(
            Stmt::Begin { .. }
                | Stmt::Commit { .. }
                | Stmt::Rollback { .. }
                | Stmt::Release { .. }
                | Stmt::Savepoint { .. }
        ))
    )
}

pub fn explain_sql(sql: &str, bytecode: bool) -> Result<String, Failure> {
    validate_single(sql)?;
    use turso_parser::ast::Stmt;
    if !matches!(
        parse_first(sql)?,
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
    let mut parser = Parser::new(sql.as_bytes());
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
    use turso_parser::ast::Stmt;
    matches!(
        first_command(sql),
        Some(Cmd::Stmt(Stmt::Select(_)) | Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. })
    )
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Effect {
    Read,
    Data,
    Schema,
    Rollback,
}
impl Effect {
    pub(crate) fn writes(self) -> bool {
        matches!(self, Self::Data | Self::Schema)
    }
}

pub(crate) fn effect(sql: &str) -> Effect {
    use turso_parser::ast::Stmt;
    match first_command(sql) {
        Some(Cmd::Explain(_) | Cmd::ExplainQueryPlan { .. }) => Effect::Read,
        Some(Cmd::Stmt(stmt)) => match stmt {
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
    use turso_parser::ast::Stmt;
    if matches!(
        parse_first(sql)?,
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

/// Keep every execution surface inside the qualified shared-WAL model.
pub(crate) fn validate_coordinated(sql: &str) -> Result<(), Failure> {
    use turso_parser::ast::{Stmt, TransactionType};
    match parse_first(sql)? {
        Some(Cmd::Stmt(Stmt::Pragma {
            name,
            body: Some(_),
        })) if ["journal_mode", "locking_mode"]
            .iter()
            .any(|n| name.name.as_str().eq_ignore_ascii_case(n)) =>
        {
            Err(Failure::input(
                "Journal and locking modes are fixed for coordinated database sessions",
            ))
        }
        Some(Cmd::Stmt(Stmt::Attach { .. } | Stmt::Detach { .. } | Stmt::Vacuum { .. })) => {
            Err(Failure::input(
                "ATTACH, DETACH and VACUUM are unavailable in coordinated sessions; use the session export or verified backup actions",
            ))
        }
        Some(Cmd::Stmt(Stmt::Begin {
            typ: Some(TransactionType::Concurrent),
            ..
        })) => Err(Failure::input(
            "Shared WAL uses serialized writers; BEGIN CONCURRENT is unavailable",
        )),
        _ => Ok(()),
    }
}

/// The inline INTEGER PRIMARY KEY (except DESC) aliases rowid and cannot be NULL.
pub(crate) fn inline_rowid_key(ddl: &str) -> Option<String> {
    use turso_parser::ast::{ColumnConstraint, CreateTableBody, SortOrder, Stmt};
    let Some(Cmd::Stmt(Stmt::CreateTable {
        body: CreateTableBody::ColumnsAndConstraints {
            columns, options, ..
        },
        ..
    })) = first_command(ddl)
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
    use turso_parser::ast::{CreateTableBody, Stmt};
    matches!(first_command(ddl),
        Some(Cmd::Stmt(Stmt::CreateTable {
            body: CreateTableBody::ColumnsAndConstraints { options, .. }, ..
        })) if !options.contains_without_rowid())
}

/// Check the pinned engine's numeric conversion before publishing a supposedly lossless SQL literal.
pub fn checked_real_literal(n: f64) -> Result<String, Failure> {
    if !n.is_finite() {
        return Err(Failure::input(
            "SQL export requires finite reals; use typed JSONL",
        ));
    }
    for literal in [format!("{n:?}"), format!("{n:.17e}")] {
        if let turso_core::Numeric::Float(parsed) = turso_core::Numeric::from(literal.as_str())
            && f64::from(parsed).to_bits() == n.to_bits()
        {
            return Ok(literal);
        }
    }
    Err(Failure::input(
        "Turso 0.8.3 cannot reload this real literal exactly; use typed JSON/JSONL",
    ))
}
pub(crate) fn declared_types(ddl: &str) -> BTreeMap<String, String> {
    use turso_parser::ast::{CreateTableBody, Stmt};
    let Some(Cmd::Stmt(Stmt::CreateTable {
        body: CreateTableBody::ColumnsAndConstraints { columns, .. },
        ..
    })) = first_command(ddl)
    else {
        return BTreeMap::new();
    };
    columns
        .into_iter()
        .filter_map(|c| {
            c.col_type
                .map(|t| (c.col_name.as_str().to_ascii_lowercase(), t.to_string()))
        })
        .collect()
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
    #[test]
    fn cursor_statements_survive_unrelated_errors_and_scripts_keep_triggers() {
        let text = "BAD SYNTAX; SELECT 42; INVALID AGAIN";
        let cursor = text.find("42").unwrap();
        assert_eq!(
            selection_or_statement(text, cursor..cursor).unwrap().trim(),
            "SELECT 42;"
        );
        assert!(super::script_statements("SELECT 1; BAD SYNTAX").is_err());
        assert_eq!(super::script_statements("CREATE TRIGGER t AFTER INSERT ON x BEGIN UPDATE x SET v=CASE WHEN v=1 THEN 2 ELSE 3 END; DELETE FROM x; END; SELECT ';';").unwrap().len(),2);
    }
}
