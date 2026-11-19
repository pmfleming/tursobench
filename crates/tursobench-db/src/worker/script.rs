//! Multi-statement scripts and guarded migrations, which run an additive script
//! only while the schema still matches the one captured for review. Both share
//! one retained budget and, when atomic, one editor transaction.
use super::{
    Output, Worker,
    execute::{ExecutionContext, MAX_BYTES, MAX_ROWS, Query, elapsed_ms, execute},
};
use crate::{
    connection::{failure, metadata_bytes, rollback},
    schema_compare, sql,
};
use std::sync::{Arc, Mutex};
use tursobench_core::{
    Access, Bindings, Budget, Column, Completion, Delivery, ErrorKind, Event, EventKind, Failure,
    SchemaSnapshot, ScriptResult, Ticket, TransactionState, Value,
};

/// Rows and bytes retained by the script's earlier statements.
#[derive(Default)]
struct Retained {
    rows: usize,
    bytes: usize,
}
impl Retained {
    fn budget(&self, budget: Budget) -> Budget {
        Budget {
            rows: budget.rows.min(MAX_ROWS).saturating_sub(self.rows),
            bytes: budget.bytes.min(MAX_BYTES).saturating_sub(self.bytes),
            page: false,
            ..budget
        }
    }
    /// Keeps a statement's result only while the whole script fits the byte budget.
    fn keep(&mut self, result: &mut ScriptResult) {
        let payload = metadata_bytes(&result.columns)
            + result
                .rows
                .iter()
                .flatten()
                .map(Value::bytes)
                .sum::<usize>()
            + result.sql.len();
        if self.bytes.saturating_add(payload) > MAX_BYTES {
            result.rows.clear();
            result.columns.clear();
            result.completion.delivery = Delivery::Limited;
        }
        self.bytes = self.bytes.saturating_add(payload).min(MAX_BYTES);
        self.rows += result.rows.len();
    }
}
fn bindings(parameters: &Bindings, statement: &str) -> Bindings {
    match parameters {
        Bindings::Positional(_) => Bindings::Positional(vec![]),
        Bindings::Named(values) => Bindings::Named(
            sql::parameter_names(statement)
                .into_iter()
                .map(|name| {
                    let value = values[&name].clone();
                    (name, value)
                })
                .collect(),
        ),
    }
}
fn captured(events: &Mutex<Vec<Event>>) -> (Vec<Column>, Vec<Vec<Value>>) {
    let mut columns = Vec::new();
    let mut rows = Vec::new();
    for event in events.lock().unwrap().drain(..) {
        match event.kind {
            EventKind::Columns(c) => columns = c,
            EventKind::Rows(r) => rows.extend(r),
            _ => {}
        }
    }
    (columns, rows)
}
fn fold(summary: &mut Completion, completion: &Completion) {
    summary.rows_seen = summary.rows_seen.saturating_add(completion.rows_seen);
    summary.changed = summary.changed.saturating_add(completion.changed);
    summary.transaction = completion.transaction;
    if completion.delivery != Delivery::Complete {
        summary.delivery = completion.delivery;
    }
    summary.error = completion.error.clone();
}
fn unknown(summary: &mut Completion, message: String) {
    summary.transaction = TransactionState::Unknown;
    summary.error = Some(Failure {
        kind: ErrorKind::Unknown,
        message,
    });
}

impl Worker<'_> {
    /// The schema a later guarded migration must still match.
    pub(super) async fn capture_schema(&self) -> Result<EventKind, Failure> {
        schema_compare::capture(self.browse)
            .await
            .map(EventKind::SchemaSnapshot)
    }
    pub(super) async fn script(
        &mut self,
        ticket: Ticket,
        text: String,
        parameters: Bindings,
        atomic: bool,
        budget: Budget,
        guard: Option<&SchemaSnapshot>,
    ) -> Result<EventKind, Failure> {
        let statements = self.statements(&text, &parameters, atomic, guard.is_some())?;
        let started = std::time::Instant::now();
        let mut summary = Completion::new(self.transaction());
        if self.output.discarded(ticket) {
            summary.delivery = Delivery::Discarded;
            return Ok(EventKind::Finished(summary));
        }
        if atomic {
            let begin = match self.access {
                Access::ReadOnly => "BEGIN",
                Access::ReadWrite => "BEGIN IMMEDIATE",
            };
            self.editor.execute(begin, ()).await.map_err(failure)?;
        }
        if let Some(expected) = guard {
            self.check_schema(expected).await?;
        }
        let mut retained = Retained::default();
        for (index, statement) in statements.into_iter().enumerate() {
            let bindings = bindings(&parameters, &statement);
            let query = Query::new(self.editor, statement, bindings, retained.budget(budget));
            let mut result = self.statement(ticket, index, query).await;
            retained.keep(&mut result);
            fold(&mut summary, &result.completion);
            self.output.send(ticket, EventKind::ScriptResult(result));
            if summary.error.is_some() || self.output.discarded(ticket) {
                break;
            }
        }
        let discarded = self.output.discarded(ticket);
        let rolled_back = atomic && self.end_atomic(ticket, &mut summary, discarded).await;
        if discarded {
            summary.delivery = Delivery::Discarded;
        }
        self.uncertain |= summary.transaction == TransactionState::Unknown;
        summary.elapsed_ms = elapsed_ms(started);
        self.output.send(
            ticket,
            EventKind::ScriptOutcome {
                atomic,
                rolled_back,
            },
        );
        Ok(EventKind::Finished(summary))
    }
    /// Parses and validates the whole script before any statement runs.
    fn statements(
        &self,
        text: &str,
        parameters: &Bindings,
        atomic: bool,
        guarded: bool,
    ) -> Result<Vec<String>, Failure> {
        if guarded && self.access != Access::ReadWrite {
            return Err(Failure::input(
                "Migration requires explicit read/write access",
            ));
        }
        let statements = sql::script_statements(text)?;
        for statement in &statements {
            sql::validate_coordinated(statement)?;
            if guarded && !schema_compare::additive(statement) {
                return Err(Failure::input(
                    "Guarded migration drafts require additive DDL with an explicit main schema target; use a separate reviewed workflow for destructive/data changes",
                ));
            }
        }
        if statements.iter().any(|s| sql::effect(s).writes()) {
            self.settled("reopen and inspect before further writes")?;
        }
        if atomic
            && (self.transaction() != TransactionState::Autocommit
                || statements.iter().any(|s| sql::transaction_control(s)))
        {
            return Err(Failure::input(
                "Atomic scripts require autocommit and cannot contain transaction commands",
            ));
        }
        if matches!(parameters, Bindings::Positional(v) if !v.is_empty()) {
            return Err(Failure::input(
                "Use named bindings for parameterized scripts",
            ));
        }
        sql::validate_bindings(text, parameters)?;
        if self.access == Access::ReadOnly {
            for s in &statements {
                sql::validate_readonly(s)?;
            }
        }
        Ok(statements)
    }
    /// Compares the target schema inside the migration transaction.
    async fn check_schema(&mut self, expected: &SchemaSnapshot) -> Result<(), Failure> {
        let error = match schema_compare::capture(self.editor).await {
            Ok(current) if &current == expected => return Ok(()),
            Ok(_) => Failure::input(
                "Target schema changed since comparison; refresh and review a new draft",
            ),
            Err(error) => error,
        };
        if let Err(cleanup) = rollback(self.editor).await {
            self.uncertain = true;
            return Err(Failure {
                kind: ErrorKind::Unknown,
                message: format!(
                    "Migration validation failed: {error}; rollback uncertain: {cleanup}"
                ),
            });
        }
        Err(error)
    }
    /// Runs one statement, capturing its result instead of streaming it.
    async fn statement(&mut self, ticket: Ticket, index: usize, query: Query<'_>) -> ScriptResult {
        let sql = query.sql.clone();
        let capture = Arc::new(Mutex::new(Vec::new()));
        let output = Output {
            capture: Some(capture.clone()),
            ..self.output.clone()
        };
        let context = ExecutionContext {
            out: &output,
            access: self.access,
            depot: &mut self.depot,
            tail: false,
        };
        let completion = execute(ticket, query, context).await;
        self.note_completion(ticket, sql::effect(&sql), &completion);
        let (columns, rows) = captured(&capture);
        ScriptResult {
            index,
            sql,
            columns,
            rows,
            completion,
        }
    }
    /// Commits a successful atomic script or rolls back a failed or discarded
    /// one; returns whether it rolled back.
    async fn end_atomic(
        &mut self,
        ticket: Ticket,
        summary: &mut Completion,
        discarded: bool,
    ) -> bool {
        if summary.error.is_some() || discarded {
            match rollback(self.editor).await {
                Ok(()) => {
                    summary.changed = 0;
                    self.dirty_data = false;
                    self.dirty_schema = false;
                    summary.transaction = self.transaction();
                    return true;
                }
                Err(e) => unknown(summary, format!("Atomic rollback outcome UNKNOWN: {e}")),
            }
        } else if let Err(e) = self.editor.execute("COMMIT", ()).await {
            unknown(
                summary,
                format!("Atomic COMMIT outcome UNKNOWN; reopen and inspect before retry: {e}"),
            );
        } else {
            summary.transaction = self.transaction();
            self.note_completion(ticket, sql::Effect::Read, summary);
        }
        false
    }
}
