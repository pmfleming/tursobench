//! Read-only tools for an external agent. They run on the session worker over a
//! dedicated `query_only` connection, so they see committed state, never join the
//! editor's manual transaction and cannot write even if the parser gate is wrong.
//! Replies travel over a per-call channel, not the UI event stream.
use crate::{
    Operation, Request,
    connection::{failure, from_engine},
    sql,
};
use serde_json::{Value as Json, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, SyncSender},
    },
    time::Duration,
};
use turso::Connection;
use tursobench_core::{Failure, Ticket, Value, quote_identifier};

/// Assistant request IDs never collide with the UI's, which count up from 1.
static NEXT_REQUEST: AtomicU64 = AtomicU64::new(1 << 62);
pub const MAX_ROWS: usize = 200;
pub const MAX_BYTES: usize = 256 * 1024;
pub const MAX_CELL_CHARS: usize = 2_000;
const MAX_OBJECTS: usize = 2_000;
const MAX_DDL: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Tool {
    ListTables,
    Describe(String),
    /// One SELECT, `WITH … SELECT`, or EXPLAIN [QUERY PLAN] of a SELECT.
    Query(String),
}
impl Tool {
    pub(crate) fn bytes(&self) -> usize {
        match self {
            Self::ListTables => 0,
            Self::Describe(s) | Self::Query(s) => s.len(),
        }
    }
}

pub type Reply = Result<Json, Failure>;

/// A cloneable, thread-safe way to call assistant tools on one session.
#[derive(Clone)]
pub struct Handle {
    pub(crate) id: u64,
    pub(crate) commands: SyncSender<Request>,
    pub(crate) discard: Arc<crate::DeliveryControl>,
    pub(crate) closing: Arc<AtomicBool>,
}
impl Handle {
    /// Blocks the calling (non-UI) thread until the worker answers or `timeout` passes.
    pub fn call(&self, tool: Tool, timeout: Duration) -> Reply {
        if self.closing.load(Ordering::Acquire) {
            return Err(Failure::input("The database session is closing"));
        }
        let request = NEXT_REQUEST.fetch_add(1, Ordering::Relaxed);
        let ticket = Ticket {
            database: self.id,
            session: self.id,
            request,
            generation: 0,
        };
        let (reply, answer) = mpsc::sync_channel(1);
        let operation = Operation::Assistant { tool, reply };
        let bytes = operation.bytes();
        {
            let mut pending = self.discard.requests.lock().unwrap();
            let total = pending.values().map(|(_, b)| b).sum::<usize>();
            if total.saturating_add(bytes) > crate::COMMAND_BYTES {
                return Err(Failure::input("The database queue is full; try again"));
            }
            pending.insert(request, (false, bytes));
        }
        if let Err(error) = self.commands.try_send(Request { ticket, operation }) {
            self.discard.finished(request);
            return Err(Failure::input(format!("The database is busy: {error}")));
        }
        match answer.recv_timeout(timeout) {
            Ok(reply) => reply,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                // Stop the read if it is still running; its late reply is dropped.
                self.discard.discard(request);
                Err(Failure::input("The database did not answer in time"))
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                Err(Failure::input("The database session closed"))
            }
        }
    }
}

/// Only plain reads pass; everything else is refused before preparation.
pub(crate) fn validate(text: &str) -> Result<(), Failure> {
    use turso_parser::ast::{Cmd, Stmt};
    sql::validate_single(text)?;
    sql::validate_coordinated(text)?;
    if identifiers(text)?
        .iter()
        .any(|name| name == "pragma_database_list")
    {
        return Err(location_error());
    }
    let command = turso_parser::parser::Parser::new(text.as_bytes())
        .next_cmd()
        .map_err(|e| Failure::input(e.to_string()))?;
    match command {
        Some(Cmd::Stmt(Stmt::Select(_)))
        | Some(Cmd::Explain(Stmt::Select(_)))
        | Some(Cmd::ExplainQueryPlan {
            stmt: Stmt::Select(_),
            ..
        }) => Ok(()),
        _ => Err(Failure {
            kind: tursobench_core::ErrorKind::ReadOnly,
            message: "The assistant may only run SELECT queries (optionally WITH or EXPLAIN \
                      QUERY PLAN). Use propose_sql for anything else; the user reviews and \
                      runs it."
                .into(),
        }),
    }
}

fn location_error() -> Failure {
    Failure {
        kind: tursobench_core::ErrorKind::ReadOnly,
        message: "The assistant cannot read the database's location".into(),
    }
}

// Include string tokens: SQLite also accepts single-quoted relation names.
// This deliberately errs on the side of refusing ambiguous references.
fn identifiers(text: &str) -> Result<Vec<String>, Failure> {
    use turso_parser::token::TokenType;
    let mut names = Vec::new();
    let mut bracket = None::<String>;
    for token in turso_parser::lexer::Lexer::new(text.as_bytes()) {
        let token = token.map_err(|e| Failure::input(e.to_string()))?;
        let name = std::str::from_utf8(token.value).map_err(|e| Failure::input(e.to_string()))?;
        if let Some(content) = &mut bracket {
            if token.token_type == TokenType::TK_RBRACKET {
                names.push(content.to_ascii_lowercase());
                bracket = None;
            } else {
                content.push_str(name);
            }
        } else if token.token_type == TokenType::TK_LBRACKET {
            bracket = Some(String::new());
        } else {
            names.push(
                turso_parser::ast::Name::from_string(name)
                    .as_str()
                    .to_ascii_lowercase(),
            );
        }
    }
    if bracket.is_some() {
        return Err(Failure::input("Unclosed bracket identifier"));
    }
    Ok(names)
}

/// Check transitive view dependencies in the same read snapshot as execution.
/// Inspecting only the submitted SQL misses names hidden behind views.
async fn validate_views(c: &Connection, text: &str) -> Result<(), Failure> {
    let mut rows = c
        .query(
            "SELECT name, sql FROM sqlite_schema WHERE type = 'view'",
            (),
        )
        .await
        .map_err(failure)?;
    let mut views = std::collections::HashMap::new();
    let mut bytes = 0usize;
    while let Some(row) = rows.next().await.map_err(failure)? {
        let turso::Value::Text(name) = row.get_value(0).map_err(failure)? else {
            return Err(Failure::input("Unexpected view name"));
        };
        let turso::Value::Text(ddl) = row.get_value(1).map_err(failure)? else {
            return Err(Failure::input("Unexpected view definition"));
        };
        bytes = bytes.saturating_add(name.len()).saturating_add(ddl.len());
        if bytes > crate::COMMAND_BYTES {
            return Err(Failure::input(
                "View definitions exceed the assistant's schema budget",
            ));
        }
        views.insert(name.to_ascii_lowercase(), ddl);
    }
    let mut pending = identifiers(text)?;
    let mut seen = std::collections::HashSet::new();
    while let Some(name) = pending.pop() {
        if name == "pragma_database_list" {
            return Err(location_error());
        }
        if seen.insert(name.clone())
            && let Some(ddl) = views.get(&name)
        {
            pending.extend(identifiers(ddl)?);
        }
    }
    Ok(())
}

pub(crate) async fn connect(
    db: &turso::Database,
    settings: tursobench_core::ConnectionSettings,
) -> Result<Connection, Failure> {
    let c = db.connect().map_err(failure)?;
    crate::connection::configure(&c, settings).await?;
    c.execute("PRAGMA query_only = 1", ())
        .await
        .map_err(failure)?;
    let mut rows = c.query("PRAGMA query_only", ()).await.map_err(failure)?;
    let enabled = rows
        .next()
        .await
        .map_err(failure)?
        .map(|row| row.get_value(0))
        .transpose()
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
    if enabled != Some(turso::Value::Integer(1)) {
        return Err(Failure::input(
            "The assistant's read-only connection is unavailable",
        ));
    }
    Ok(c)
}

pub(crate) async fn run(
    c: &Connection,
    tool: Tool,
    request: u64,
    discard: &crate::DeliveryControl,
) -> Reply {
    match tool {
        Tool::ListTables => list(c).await,
        Tool::Describe(name) => describe(c, &name).await,
        Tool::Query(text) => {
            validate(&text)?;
            let interrupt = c.clone();
            *discard.active_read.lock().unwrap() = Some(crate::ActiveRead {
                request,
                interrupt: Box::new(move || {
                    let _ = interrupt.interrupt();
                }),
            });
            let result = query(c, &text, request, discard).await;
            *discard.active_read.lock().unwrap() = None;
            result
        }
    }
}

async fn list(c: &Connection) -> Reply {
    let mut rows = c
        .query(
            "SELECT type, name FROM sqlite_schema WHERE type IN ('table','view') \
             AND name NOT GLOB 'sqlite_*' AND name NOT GLOB '__turso_internal_*' \
             ORDER BY type, name",
            (),
        )
        .await
        .map_err(failure)?;
    let mut objects = vec![];
    let mut limited = false;
    while let Some(row) = rows.next().await.map_err(failure)? {
        if objects.len() == MAX_OBJECTS {
            limited = true;
            break;
        }
        let text = |i| match row.get_value(i) {
            Ok(turso::Value::Text(s)) => s,
            _ => String::new(),
        };
        objects.push(json!({"type": text(0), "name": text(1)}));
    }
    Ok(json!({"objects": objects, "limited": limited}))
}

async fn describe(c: &Connection, name: &str) -> Reply {
    let inspection = crate::inspection::inspect(c, name).await?;
    let mut rows = c
        .query(
            "SELECT sql FROM sqlite_schema WHERE name = ?1",
            vec![turso::Value::Text(name.into())],
        )
        .await
        .map_err(failure)?;
    let mut ddl = match rows.next().await.map_err(failure)? {
        Some(row) => match row.get_value(0).map_err(failure)? {
            turso::Value::Text(s) => s,
            _ => String::new(),
        },
        None => String::new(),
    };
    while rows.next().await.map_err(failure)?.is_some() {}
    let mut ddl_truncated = false;
    if ddl.len() > MAX_DDL {
        let mut end = MAX_DDL;
        while !ddl.is_char_boundary(end) {
            end -= 1;
        }
        ddl.truncate(end);
        ddl_truncated = true;
    }
    let table = &inspection.table;
    Ok(json!({
        "name": table.name,
        "kind": if table.is_table { "table" } else { "view" },
        "quoted_name": quote_identifier(&table.name),
        "columns": table.columns.iter().map(|c| json!({
            "name": c.name,
            "type": c.declared_type,
            "not_null": c.not_null,
            "primary_key_position": c.primary_key,
            "default": c.default_sql,
            "hidden": c.hidden != 0,
        })).collect::<Vec<_>>(),
        "indexes": inspection.indexes.iter().map(|i| json!({
            "name": i.name,
            "unique": i.unique,
            "partial": i.partial,
            "columns": i.columns,
        })).collect::<Vec<_>>(),
        "foreign_keys": inspection.foreign_keys.iter().map(|k| json!({
            "references": k.target,
            "columns": k.columns.iter().map(|(from, to)| json!({"from": from, "to": to})).collect::<Vec<_>>(),
            "on_update": k.on_update,
            "on_delete": k.on_delete,
        })).collect::<Vec<_>>(),
        "ddl": ddl,
        "ddl_truncated": ddl_truncated,
    }))
}

async fn query(
    c: &Connection,
    text: &str,
    request: u64,
    discard: &crate::DeliveryControl,
) -> Reply {
    c.execute("BEGIN", ()).await.map_err(failure)?;
    let result = async {
        validate_views(c, text).await?;
        query_in_snapshot(c, text, request, discard).await
    }
    .await;
    let rollback = c.execute("ROLLBACK", ()).await.map_err(failure);
    rollback?;
    result
}

async fn query_in_snapshot(
    c: &Connection,
    text: &str,
    request: u64,
    discard: &crate::DeliveryControl,
) -> Reply {
    let mut stmt = c.prepare(text).await.map_err(failure)?;
    let columns: Vec<String> = stmt.columns().iter().map(|c| c.name().into()).collect();
    if columns.len() > 512 {
        return Err(Failure::input("The result has more than 512 columns"));
    }
    let mut rows = stmt.query(()).await.map_err(failure)?;
    let mut out = vec![];
    // Reserve the largest metadata envelope before admitting any rows. Count
    // JSON bytes, including escapes and column names, rather than cell payloads.
    let mut bytes = json!({
        "columns": columns,
        "rows": [],
        "row_count": MAX_ROWS,
        "limited": false,
        "truncated_cells": MAX_ROWS * 512,
        "note": limited_note(MAX_ROWS),
    })
    .to_string()
    .len();
    if bytes > MAX_BYTES {
        return Err(Failure::input(
            "The result's column names exceed the 256 KiB budget",
        ));
    }
    let mut limited = false;
    let mut truncated_cells = 0usize;
    while let Some(row) = rows.next().await.map_err(failure)? {
        if discard.discarded(request) {
            return Err(Failure::input("The query was stopped"));
        }
        if out.len() == MAX_ROWS {
            limited = true;
            break;
        }
        let mut cells = Vec::with_capacity(columns.len());
        let mut row_truncated = 0usize;
        for i in 0..columns.len() {
            let (cell, _, cut) = cell(from_engine(row.get_value(i).map_err(failure)?));
            row_truncated += usize::from(cut);
            cells.push(cell);
        }
        let row = Json::Array(cells);
        let row_bytes = row.to_string().len() + usize::from(!out.is_empty());
        if bytes.saturating_add(row_bytes) > MAX_BYTES {
            limited = true;
            break;
        }
        bytes += row_bytes;
        truncated_cells += row_truncated;
        out.push(row);
    }
    Ok(json!({
        "columns": columns,
        "rows": out,
        "row_count": out.len(),
        "limited": limited,
        "truncated_cells": truncated_cells,
        "note": if limited {
            limited_note(out.len())
        } else {
            String::new()
        },
    }))
}

fn limited_note(rows: usize) -> String {
    format!(
        "Only the first {rows} rows were returned; add LIMIT, select fewer columns or aggregate."
    )
}

/// JSON for one value, its approximate size and whether it was shortened.
fn cell(value: Value) -> (Json, usize, bool) {
    match value {
        Value::Null => (Json::Null, 4, false),
        Value::Integer(n) => (json!(n), 8, false),
        Value::Real(n) if n.is_finite() => (json!(n), 8, false),
        Value::Real(n) => (json!(n.to_string()), 8, false),
        Value::Text(s) => {
            if s.chars().count() > MAX_CELL_CHARS {
                let cut: String = s.chars().take(MAX_CELL_CHARS).collect();
                let size = cut.len();
                (
                    json!(format!("{cut}… [{} more bytes]", s.len() - size)),
                    size,
                    true,
                )
            } else {
                let size = s.len();
                (Json::String(s), size, false)
            }
        }
        Value::Blob(b) => (json!({"blob_bytes": b.len()}), 16, false),
        Value::Deferred(d) => (json!({"large_value_bytes": d.bytes}), 16, true),
    }
}

#[cfg(test)]
mod tests {
    use super::validate;
    #[test]
    fn only_plain_reads_pass_the_gate() {
        for ok in [
            "SELECT 1",
            "WITH x AS (SELECT 1) SELECT * FROM x",
            "EXPLAIN QUERY PLAN SELECT 1",
            "EXPLAIN SELECT 1",
            "select * from t -- trailing comment",
        ] {
            assert!(validate(ok).is_ok(), "{ok}");
        }
        for bad in [
            "INSERT INTO t VALUES(1)",
            "WITH x AS (SELECT 1) INSERT INTO t SELECT * FROM x",
            "UPDATE t SET a = 1",
            "DELETE FROM t",
            "DROP TABLE t",
            "CREATE TABLE u(a)",
            "PRAGMA query_only = 0",
            "PRAGMA table_info(t)",
            "BEGIN",
            "ATTACH 'x.db' AS x",
            "VACUUM INTO 'copy.db'",
            "EXPLAIN DELETE FROM t",
            "SELECT 1; DELETE FROM t",
            "SELEC 1",
            "",
        ] {
            assert!(validate(bad).is_err(), "{bad}");
        }
    }
}
