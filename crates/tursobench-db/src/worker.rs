//! Engine handles, execution, delivery budgets and transaction cleanup stay on the worker.
use crate::{
    Operation, Request,
    browse::{browse_sql, describe},
    connection::{configure, failure, from_engine, rollback, state, to_engine},
    diagram, edits, inspection, sql,
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::Duration,
};
use turso::{Builder, Connection};
use tursobench_core::{
    Access, Bindings, Budget, Column, CommitOutcome, Completion, ConnectionSettings, Delivery,
    ErrorKind, Event, EventKind, Failure, Ticket, Value, quote_identifier,
};
pub(super) struct Output {
    pub(super) tx: SyncSender<Event>,
    pub(super) wake: Arc<dyn Fn() + Send + Sync>,
    pub(super) discard: Arc<crate::DeliveryControl>,
    pub(super) closing: Arc<AtomicBool>,
}
impl Output {
    fn discarded(&self, t: Ticket) -> bool {
        self.discard.discarded(t.request) || self.closing.load(Ordering::Acquire)
    }
    fn send(&self, ticket: Ticket, kind: EventKind) -> bool {
        let discardable = matches!(kind, EventKind::Rows(_) | EventKind::Columns(_));
        let mut event = Event { ticket, kind };
        loop {
            if discardable && self.discarded(ticket) {
                return true;
            }
            match self.tx.try_send(event) {
                Ok(()) => {
                    (self.wake)();
                    return true;
                }
                Err(TrySendError::Disconnected(_)) => return false,
                Err(TrySendError::Full(e)) => {
                    event = e;
                    (self.wake)();
                    std::thread::sleep(Duration::from_millis(2));
                }
            }
        }
    }
}
async fn open(
    path: PathBuf,
    access: Access,
    settings: ConnectionSettings,
) -> Result<(turso::Database, Connection, Connection), Failure> {
    let path = std::fs::canonicalize(path).map_err(|e| Failure {
        kind: ErrorKind::Io,
        message: e.to_string(),
    })?;
    if !path.is_file()
        || std::fs::metadata(&path)
            .map_err(|e| Failure::input(e.to_string()))?
            .len()
            == 0
    {
        return Err(Failure::input(
            "Select an existing non-empty regular database file",
        ));
    }
    let path = path
        .to_str()
        .ok_or_else(|| Failure::input("Non-UTF8 paths are not supported by Turso's builder"))?;
    let db = Builder::new_local(path)
        // Turso's native FTS indexes require both the Cargo `fts` feature and
        // index-method capability when opening existing files or creating indexes.
        .experimental_index_method(true)
        .read_only(access == Access::ReadOnly)
        .build()
        .await
        .map_err(failure)?;
    let editor = db.connect().map_err(failure)?;
    let browse = db.connect().map_err(failure)?;
    for c in [&editor, &browse] {
        configure(c, settings).await?;
    }
    // Force schema validation before reporting successful open.
    let mut rows = browse
        .query("SELECT name FROM sqlite_schema LIMIT 1", ())
        .await
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
    drop(rows);
    Ok((db, editor, browse))
}
pub(super) async fn worker(
    id: u64,
    path: PathBuf,
    access: Access,
    settings: ConnectionSettings,
    rx: Receiver<Request>,
    output: Output,
) {
    let ticket = Ticket {
        database: id,
        session: id,
        request: 0,
        generation: 0,
    };
    let (_db, editor, browse) = match open(path, access, settings).await {
        Ok(v) => v,
        Err(e) => {
            output.send(ticket, EventKind::Failed(e));
            return;
        }
    };
    if !output.send(ticket, EventKind::Opened) {
        return;
    }
    let mut worker = Worker {
        db: &_db,
        editor: &editor,
        browse: &browse,
        access,
        settings,
        output: &output,
        uncertain_edit: false,
    };
    while !output.closing.load(Ordering::Acquire) {
        let request = match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if output.closing.load(Ordering::Acquire) {
            break;
        }
        let event = match worker.dispatch(request.ticket, request.operation).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => EventKind::Failed(error),
        };
        output.send(request.ticket, event);
        output.discard.finished(request.ticket.request);
    }
    let result = rollback(&editor).await;
    let browse_result = rollback(&browse).await;
    drop(browse);
    drop(editor);
    drop(_db);
    output.send(
        ticket,
        EventKind::Closed {
            rollback: result.and(browse_result),
        },
    );
}
struct Worker<'a> {
    db: &'a turso::Database,
    editor: &'a Connection,
    browse: &'a Connection,
    access: Access,
    settings: ConnectionSettings,
    output: &'a Output,
    uncertain_edit: bool,
}
impl Worker<'_> {
    async fn dispatch(
        &mut self,
        ticket: Ticket,
        operation: Operation,
    ) -> Result<Option<EventKind>, Failure> {
        let default = Budget::default();
        let count_references = matches!(&operation, Operation::ReferenceCount { .. });
        let (connection, sql, parameters, budget) = match operation {
            Operation::Close => return Ok(None),
            Operation::Diagram => return diagram::load(self.browse).await.map(EventKind::Diagram).map(Some),
            Operation::Inspect(name) => return inspection::inspect(self.browse, &name).await.map(EventKind::Inspection).map(Some),
            Operation::CommitEdits(batch) => {
                let outcome = if self.uncertain_edit {
                    CommitOutcome::Unknown(Failure { kind: ErrorKind::Unknown, message: "A previous edit commit has an uncertain outcome; close and inspect before retrying".into() })
                } else {
                    edits::commit(self.db, self.access, self.settings, batch).await
                };
                self.uncertain_edit |= matches!(outcome, CommitOutcome::Unknown(_));
                return Ok(Some(EventKind::EditsCommitted(outcome)));
            }
            Operation::Browse(spec) => {
                let table = describe(self.browse, &spec.table).await?;
                let (sql, parameters) = browse_sql(&table, spec)?;
                self.output.send(ticket, EventKind::Table(table));
                (self.browse, sql, parameters.into(), Budget {rows:100,bytes:4*1024*1024,..default})
            }
            Operation::Lookup { table: name, keys } => {
                let table = describe(self.browse, &name).await?;
                let (sql, parameters) = inspection::lookup_sql(&table, keys)?;
                self.output.send(ticket, EventKind::Table(table));
                (self.browse, sql, parameters.into(), default)
            }
            Operation::References { table: name, key, values } | Operation::ReferenceCount { table: name,key,values } => {
                let child = inspection::inspect(self.browse, &name).await?;
                if !child.foreign_keys.contains(&key) { return Err(Failure::input("Relationship metadata changed; refresh the schema")); }
                let parent = describe(self.browse, &key.target).await?;
                let sql = inspection::references_sql(&child.table, &parent, &key, &values,count_references)?;
                self.output.send(ticket, EventKind::Table(child.table));
                (self.browse, sql, values.into(), default)
            }
            Operation::Peek {table:name,key,values} => {
                let child=inspection::inspect(self.browse,&name).await?;
                if !child.foreign_keys.contains(&key) || key.columns.len()!=values.len() || values.contains(&Value::Null) {return Err(Failure::input("Relationship metadata changed or key is NULL"));}
                let parent=describe(self.browse,&key.target).await?;
                let keys=key.columns.iter().zip(values).map(|((_,target),value)|target.clone().map(|target|(target,value)).ok_or_else(||Failure::input("Unresolved parent key"))).collect::<Result<Vec<_>,_>>()?;
                let (sql,parameters)=inspection::lookup_sql(&parent,keys)?;
                (self.browse,sql,parameters.into(),Budget{rows:1,bytes:512*1024,..default})
            }
            Operation::Count(name) => (self.browse, format!("SELECT count(*) AS row_count FROM {}", quote_identifier(&name)), vec![].into(), default),
            Operation::Diagnostic(kind) => (self.browse, kind.sql().into(), vec![].into(), default),
            Operation::Schema => (self.browse, "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY type, name".into(), vec![].into(), default),
            Operation::Explain { sql, parameters, bytecode } => (self.editor, sql::explain_sql(&sql, bytecode)?, parameters, default),
            Operation::ExecuteBound { sql, parameters, budget } => (self.editor, sql, parameters, budget),
            Operation::Execute { sql, parameters, budget } => (self.editor, sql, parameters.into(), budget),
        };
        let mut completion =
            execute(connection, ticket, &sql, parameters, budget, self.output).await;
        // Every result reports the persistent editor's state, including browse/diagnostics.
        completion.transaction = state(self.editor);
        Ok(Some(EventKind::Finished(completion)))
    }
}
async fn execute(
    c: &Connection,
    ticket: Ticket,
    sql: &str,
    parameters: Bindings,
    budget: Budget,
    out: &Output,
) -> Completion {
    let started = std::time::Instant::now();
    let mut completion = Completion {
        elapsed_ms: 0,
        delivery: Delivery::Complete,
        rows_seen: 0,
        changed: 0,
        transaction: state(c),
        error: None,
    };
    let result = async {
        sql::validate_single(sql)?;
        sql::validate_bindings(sql, &parameters)?;
        if parameters.bytes() > 8 * 1024 * 1024 {
            return Err(Failure::input("Parameters exceed 8 MiB"));
        }
        let mut stmt = c.prepare(sql).await.map_err(failure)?;
        let columns: Vec<_> = stmt
            .columns()
            .into_iter()
            .map(|c| Column {
                name: c.name().into(),
                declared_type: c.decl_type().map(str::to_owned),
            })
            .collect();
        if columns.len() > 512
            || columns
                .iter()
                .map(|c| c.name.len() + c.declared_type.as_ref().map_or(0, String::len))
                .sum::<usize>()
                > 1024 * 1024
        {
            return Err(Failure::input("Result metadata exceeds display budget"));
        }
        out.send(ticket, EventKind::Columns(columns));
        let parameters = match parameters {
            Bindings::Positional(values) => {
                turso::params::Params::Positional(values.into_iter().map(to_engine).collect())
            }
            Bindings::Named(values) => turso::params::Params::Named(
                values
                    .into_iter()
                    .map(|(name, value)| (name.into(), to_engine(value)))
                    .collect(),
            ),
        };
        let mut rows = stmt.query(parameters).await.map_err(failure)?;
        let mut kept = 0;
        let mut bytes = 0;
        let mut chunk = Vec::new();
        let mut chunk_bytes = 0;
        while let Some(row) = rows.next().await.map_err(failure)? {
            completion.rows_seen += 1;
            if out.discarded(ticket) {
                completion.delivery = Delivery::Discarded;
                chunk.clear();
                continue;
            }
            if completion.delivery == Delivery::Limited {
                continue;
            }
            let values: Vec<Value> = (0..rows.column_count())
                .map(|i| row.get_value(i).map(from_engine).map_err(failure))
                .collect::<Result<_, _>>()?;
            let size = values.iter().map(Value::bytes).sum::<usize>();
            if kept >= budget.rows.min(10_000)
                || bytes + size > budget.bytes.min(8 * 1024 * 1024)
                || values
                    .iter()
                    .any(|v| v.bytes() > budget.cell_bytes.min(64 * 1024))
            {
                completion.delivery = Delivery::Limited;
                continue;
            }
            bytes += size;
            kept += 1;
            chunk_bytes += size;
            chunk.push(values);
            if chunk.len() >= 64 || chunk_bytes >= 64 * 1024 {
                if !out.send(ticket, EventKind::Rows(std::mem::take(&mut chunk))) {
                    completion.delivery = Delivery::Discarded;
                }
                chunk_bytes = 0;
            }
        }
        if !chunk.is_empty() {
            out.send(ticket, EventKind::Rows(chunk));
        }
        completion.changed = stmt.n_change();
        Ok::<(), Failure>(())
    }
    .await;
    completion.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    completion.error = result.err();
    completion.transaction = state(c);
    if out.discarded(ticket) {
        completion.delivery = Delivery::Discarded;
    }
    completion
}
