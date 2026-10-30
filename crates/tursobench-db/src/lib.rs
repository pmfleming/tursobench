//! Turso-only session worker. No engine handles cross the service boundary.
mod browse;
mod edits;
use browse::{browse_sql, describe};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
    time::Duration,
};
use turso::{Builder, Connection, params_from_iter};
use tursobench_core::*;

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const COMMAND_CAPACITY: usize = 2;
const EVENT_CAPACITY: usize = 8;

pub enum Operation {
    Execute {
        sql: String,
        parameters: Vec<Value>,
        budget: Budget,
    },
    Schema,
    Browse(Browse),
    CommitEdits(DraftBatch),
    Close,
}
pub struct Request {
    pub ticket: Ticket,
    pub operation: Operation,
}
pub struct Session {
    pub id: u64,
    commands: SyncSender<Request>,
    pub events: Receiver<Event>,
    discard: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
}
impl Session {
    pub fn open(
        path: PathBuf,
        access: Access,
        exclusive_use_confirmed: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, Failure> {
        if !exclusive_use_confirmed {
            return Err(Failure::input(
                "Only databases not in use by other applications are supported",
            ));
        }
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let (commands, rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let discard = Arc::new(AtomicU64::new(0));
        let closing = Arc::new(AtomicBool::new(false));
        let output = Output {
            tx,
            wake: Arc::new(wake),
            discard: discard.clone(),
            closing: closing.clone(),
        };
        std::thread::Builder::new()
            .name(format!("turso-session-{id}"))
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_time()
                    .build()
                    .expect("worker runtime");
                runtime.block_on(worker(id, path, access, rx, output));
            })
            .map_err(|e| Failure {
                kind: ErrorKind::Io,
                message: e.to_string(),
            })?;
        Ok(Self {
            id,
            commands,
            events,
            discard,
            closing,
        })
    }
    pub fn ticket(&self, request: u64, generation: u64) -> Ticket {
        Ticket {
            database: self.id,
            session: self.id,
            request,
            generation,
        }
    }
    pub fn submit(&self, request: Request) -> Result<(), Failure> {
        if request.ticket.session != self.id || request.ticket.database != self.id {
            return Err(Failure::input("Wrong session identity"));
        }
        self.commands
            .try_send(request)
            .map_err(|e| Failure::input(format!("Worker queue unavailable: {e}")))
    }
    pub fn discard_results(&self, request: u64) {
        self.discard.fetch_max(request, Ordering::Release);
    }
    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}

struct Output {
    tx: SyncSender<Event>,
    wake: Arc<dyn Fn() + Send + Sync>,
    discard: Arc<AtomicU64>,
    closing: Arc<AtomicBool>,
}
impl Output {
    fn discarded(&self, t: Ticket) -> bool {
        self.discard.load(Ordering::Acquire) >= t.request || self.closing.load(Ordering::Acquire)
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
fn failure(e: turso::Error) -> Failure {
    let kind = match &e {
        turso::Error::Busy(_) | turso::Error::BusySnapshot(_) => ErrorKind::Busy,
        turso::Error::Readonly(_) => ErrorKind::ReadOnly,
        turso::Error::Constraint(_) => ErrorKind::Constraint,
        turso::Error::IoError(..) => ErrorKind::Io,
        _ => ErrorKind::Engine,
    };
    Failure {
        kind,
        message: e.to_string(),
    }
}
fn state(c: &Connection) -> TransactionState {
    match c.is_autocommit() {
        Ok(true) => TransactionState::Autocommit,
        Ok(false) => TransactionState::Active,
        Err(_) => TransactionState::Unknown,
    }
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
fn to_engine(v: Value) -> turso::Value {
    match v {
        Value::Null => turso::Value::Null,
        Value::Integer(n) => turso::Value::Integer(n),
        Value::Real(n) => turso::Value::Real(n),
        Value::Text(s) => turso::Value::Text(s),
        Value::Blob(b) => turso::Value::Blob(b),
    }
}
fn from_engine(v: turso::Value) -> Value {
    match v {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(n) => Value::Integer(n),
        turso::Value::Real(n) => Value::Real(n),
        turso::Value::Text(s) => Value::Text(s),
        turso::Value::Blob(b) => Value::Blob(b),
    }
}
async fn open(
    path: PathBuf,
    access: Access,
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
        .read_only(access == Access::ReadOnly)
        .build()
        .await
        .map_err(failure)?;
    let editor = db.connect().map_err(failure)?;
    let browse = db.connect().map_err(failure)?;
    for c in [&editor, &browse] {
        configure(c).await?;
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
async fn configure(c: &Connection) -> Result<(), Failure> {
    c.busy_timeout(Duration::from_millis(250))
        .map_err(failure)?;
    c.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(failure)?;
    let mut rows = c.query("PRAGMA foreign_keys", ()).await.map_err(failure)?;
    let enabled = rows
        .next()
        .await
        .map_err(failure)?
        .ok_or_else(|| Failure::input("Foreign key status unavailable"))?
        .get_value(0)
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
    if enabled != turso::Value::Integer(1) {
        return Err(Failure::input(
            "Foreign key enforcement could not be enabled",
        ));
    }
    Ok(())
}
async fn rollback(c: &Connection) -> Result<(), Failure> {
    match state(c) {
        TransactionState::Autocommit => Ok(()),
        _ => c.execute("ROLLBACK", ()).await.map(|_| ()).map_err(failure),
    }
}
async fn worker(id: u64, path: PathBuf, access: Access, rx: Receiver<Request>, output: Output) {
    let ticket = Ticket {
        database: id,
        session: id,
        request: 0,
        generation: 0,
    };
    let (_db, editor, browse) = match open(path, access).await {
        Ok(v) => v,
        Err(e) => {
            output.send(ticket, EventKind::Failed(e));
            return;
        }
    };
    if !output.send(ticket, EventKind::Opened) {
        return;
    }
    let mut uncertain_edit = false;
    while !output.closing.load(Ordering::Acquire) {
        let request = match rx.recv_timeout(Duration::from_millis(25)) {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        match request.operation {
            Operation::Close => break,
            Operation::CommitEdits(batch) => {
                let outcome = if uncertain_edit {
                    CommitOutcome::Unknown(Failure { kind: ErrorKind::Unknown, message: "A previous edit commit has an uncertain outcome; close and inspect before retrying".into() })
                } else {
                    edits::commit(&_db, access, batch).await
                };
                uncertain_edit |= matches!(outcome, CommitOutcome::Unknown(_));
                output.send(request.ticket, EventKind::EditsCommitted(outcome));
            }
            Operation::Browse(spec) => match describe(&browse, &spec.table).await {
                Ok(table) => match browse_sql(&table, &spec) {
                    Ok((sql, params)) => {
                        output.send(request.ticket, EventKind::Table(table));
                        let mut completion = execute(
                            &browse,
                            request.ticket,
                            &sql,
                            params,
                            Budget::default(),
                            &output,
                        )
                        .await;
                        completion.transaction = state(&editor);
                        output.send(request.ticket, EventKind::Finished(completion));
                    }
                    Err(e) => {
                        output.send(request.ticket, EventKind::Failed(e));
                    }
                },
                Err(e) => {
                    output.send(request.ticket, EventKind::Failed(e));
                }
            },
            Operation::Schema => {
                let mut completion = execute(&browse, request.ticket, "SELECT type, name, tbl_name, sql FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%' ORDER BY type, name", vec![], Budget::default(), &output).await;
                completion.transaction = state(&editor);
                output.send(request.ticket, EventKind::Finished(completion));
            }
            Operation::Execute {
                sql,
                parameters,
                budget,
            } => {
                let completion =
                    execute(&editor, request.ticket, &sql, parameters, budget, &output).await;
                output.send(request.ticket, EventKind::Finished(completion));
            }
        }
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
async fn execute(
    c: &Connection,
    ticket: Ticket,
    sql: &str,
    parameters: Vec<Value>,
    budget: Budget,
    out: &Output,
) -> Completion {
    let mut completion = Completion {
        delivery: Delivery::Complete,
        rows_seen: 0,
        changed: 0,
        transaction: state(c),
        error: None,
    };
    let result = async {
        validate_single(sql)?;
        if parameters.iter().map(Value::bytes).sum::<usize>() > 8 * 1024 * 1024 {
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
        let mut rows = stmt
            .query(params_from_iter(parameters.into_iter().map(to_engine)))
            .await
            .map_err(failure)?;
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
    completion.error = result.err();
    completion.transaction = state(c);
    if out.discarded(ticket) {
        completion.delivery = Delivery::Discarded;
    }
    completion
}
