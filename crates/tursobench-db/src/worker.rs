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
    Access, Bindings, Budget, Column, Completion, ConnectionSettings, Delivery, ErrorKind, Event,
    EventKind, Failure, Ticket, Value, quote_identifier,
};
#[derive(Clone)]
pub(super) struct Output {
    pub(super) capture: Option<Arc<std::sync::Mutex<Vec<Event>>>>,
    pub(super) tx: SyncSender<Event>,
    pub(super) wake: Arc<dyn Fn() + Send + Sync>,
    pub(super) discard: Arc<crate::DeliveryControl>,
    pub(super) closing: Arc<AtomicBool>,
}
impl Output {
    fn invalidated(&self, ticket: Ticket, schema: bool) {
        self.send(
            Ticket {
                request: 0,
                ..ticket
            },
            EventKind::Invalidated {
                schema,
                source_request: ticket.request,
            },
        );
    }
    fn discarded(&self, t: Ticket) -> bool {
        self.discard.discarded(t.request) || self.closing.load(Ordering::Acquire)
    }
    fn send(&self, ticket: Ticket, kind: EventKind) -> bool {
        let discardable = matches!(kind, EventKind::Rows(_) | EventKind::Columns(_));
        if discardable && self.discarded(ticket) {
            return true;
        }
        if let Some(events) = &self.capture {
            events.lock().unwrap().push(Event { ticket, kind });
            return true;
        }
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
) -> Result<
    (
        turso::Database,
        Connection,
        Connection,
        crate::access::Admission,
        tursobench_core::Coordination,
    ),
    Failure,
> {
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
    let text = path
        .to_str()
        .ok_or_else(|| Failure::input("Non-UTF8 paths are not supported by Turso's builder"))?;
    let mut admission = crate::access::Admission::acquire(&path)?;
    let path = text;
    if access == Access::ReadWrite {
        // Validate compatibility before permitting the engine's writable-open side effects.
        let preflight = Builder::new_local(path)
            .experimental_index_method(true)
            .experimental_multiprocess_wal(admission.reservation.is_none())
            .read_only(true)
            .build()
            .await
            .map_err(failure)?;
        let c = preflight.connect().map_err(failure)?;
        let mut r = c
            .query("SELECT name FROM sqlite_schema LIMIT 1", ())
            .await
            .map_err(failure)?;
        while r.next().await.map_err(failure)?.is_some() {}
        drop(r);
        drop(c);
        drop(preflight);
        // A writable open initializes/joins native coordination. The held
        // main-file lock excludes legacy writers across this transition.
        admission.reservation = None;
    }
    let shared = admission.reservation.is_none();
    let db = Builder::new_local(path)
        // Turso's native FTS indexes require both the Cargo `fts` feature and
        // index-method capability when opening existing files or creating indexes.
        .experimental_index_method(true)
        .experimental_multiprocess_wal(shared)
        .read_only(access == Access::ReadOnly)
        .build()
        .await
        .map_err(failure)?;
    let coordination = if shared {
        if !db.is_multiprocess_wal_coordinated().map_err(failure)? {
            return Err(Failure::input(
                "Shared WAL coordination is unavailable; no uncoordinated connection was admitted",
            ));
        }
        tursobench_core::Coordination::SharedWal
    } else {
        tursobench_core::Coordination::ProtectedReadOnly
    };
    // The engine now owns its native lifetime lock. Release the temporary
    // admission lock so maintenance can later reserve a fully closed mapping.
    admission.joined();
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
    Ok((db, editor, browse, admission, coordination))
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
    let source_path = std::fs::canonicalize(&path).unwrap_or_else(|_| path.clone());
    let (_db, editor, browse, _admission, coordination) = match open(path, access, settings).await {
        Ok(v) => v,
        Err(mut e) => {
            let usage = crate::processes_using(&source_path);
            if !usage.applications.is_empty() {
                e.message
                    .push_str(&format!(" Also open in {}.", usage.applications.join(", ")));
            }
            output.send(ticket, EventKind::Failed(e));
            return;
        }
    };
    let mut changes = if coordination == tursobench_core::Coordination::SharedWal {
        match crate::observe::Changes::new(&source_path, &browse).await {
            Ok(changes) => Some(changes),
            Err(e) => {
                output.send(ticket, EventKind::Failed(e));
                return;
            }
        }
    } else {
        None
    };
    let mut observed = std::time::Instant::now();
    let observation_interval = Duration::from_millis(500);
    if !output.send(ticket, EventKind::Opened(coordination)) {
        return;
    }
    let mut worker = Worker {
        path: &source_path,
        db: &_db,
        editor: &editor,
        browse: &browse,
        access,
        settings,
        output: &output,
        commits: edits::CommitState::default(),
        uncertain: false,
        depot: crate::values::Depot::default(),
        dirty_data: false,
        dirty_schema: false,
        assistant: None,
    };
    let mut backup = None;
    while !output.closing.load(Ordering::Acquire) {
        if observed.elapsed() >= observation_interval {
            observed = std::time::Instant::now();
            if let Some(changes) = &mut changes {
                match changes.poll(&browse).await {
                    Ok(Some(schema)) => {
                        output.invalidated(ticket, schema);
                    }
                    Ok(None) => {}
                    Err(e) if matches!(e.kind, ErrorKind::Busy | ErrorKind::Interrupted) => {}
                    Err(e) => {
                        output.send(ticket, EventKind::Failed(e));
                        break;
                    }
                }
            }
        }
        // Protected read-only sessions need no timer. Shared sessions wake only
        // for a command or the next external-change observation deadline.
        let received = if changes.is_some() {
            rx.recv_timeout(observation_interval.saturating_sub(observed.elapsed()))
        } else {
            rx.recv().map_err(|_| mpsc::RecvTimeoutError::Disconnected)
        };
        let request = match received {
            Ok(r) => r,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => break,
        };
        if output.closing.load(Ordering::Acquire) {
            break;
        }
        if let Operation::BackupAndClose(destination) = request.operation {
            let prepared = if worker.uncertain {
                Err(Failure {
                    kind: ErrorKind::Unknown,
                    message: "Prior outcome UNKNOWN; reopen and inspect before maintenance".into(),
                })
            } else if access != Access::ReadWrite {
                Err(Failure {
                    kind: ErrorKind::ReadOnly,
                    message: "Backup-and-close requires write access for an explicit checkpoint"
                        .into(),
                })
            } else if destination.exists() {
                Err(Failure::input("Backup destination already exists"))
            } else {
                crate::transfers::checkpoint(&editor).await
            };
            match prepared {
                Ok(()) => {
                    output.closing.store(true, Ordering::Release);
                    backup = Some((request.ticket, destination));
                    break;
                }
                Err(e) => {
                    output.discard.finished(request.ticket.request);
                    output.send(request.ticket, EventKind::Failed(e));
                    continue;
                }
            }
        }
        if let Operation::Assistant { tool, reply } = request.operation {
            let answer = worker.assistant(request.ticket, tool).await;
            output.discard.finished(request.ticket.request);
            // The caller may have timed out and gone; that is not an error here.
            let _ = reply.try_send(answer);
            continue;
        }
        let event = match worker.dispatch(request.ticket, request.operation).await {
            Ok(Some(event)) => event,
            Ok(None) => break,
            Err(error) => EventKind::Failed(error),
        };
        // A terminal event acknowledges that this request is no longer pending.
        // Retire it before publishing so immediate ID reuse cannot race cleanup.
        output.discard.finished(request.ticket.request);
        output.send(request.ticket, event);
    }
    drop(worker);
    let result = rollback(&editor).await;
    let browse_result = rollback(&browse).await;
    drop(browse);
    drop(editor);
    drop(_db);
    if let Some((request, destination)) = backup {
        let cleanup_failed = result.is_err() || browse_result.is_err();
        let result = match result.as_ref().and(browse_result.as_ref()) {
            Ok(()) => match crate::access::reserve(&source_path) {
                Ok(Some(_reservation)) => crate::transfers::copy_closed(&source_path, &destination, settings).await,
                Ok(None) => Err(Failure { kind: ErrorKind::Busy, message: "Backup not published: another shared connection is still open. Close it and retry from a new session.".into() }),
                Err(e) => Err(e),
            },
            Err(e) => Err(e.clone()),
        };
        let error = result.err();
        let message = if let Some(e) = &error {
            format!("Backup not published; session closed: {e}")
        } else {
            format!(
                "Verified backup saved to {}; source session closed",
                destination.display()
            )
        };
        output.discard.finished(request.request);
        output.send(
            request,
            EventKind::TransferFinished(tursobench_core::TransferOutcome {
                rows: 0,
                changed: 0,
                transaction: if cleanup_failed {
                    tursobench_core::TransactionState::Unknown
                } else {
                    tursobench_core::TransactionState::Autocommit
                },
                error,
                message,
                executed: false,
            }),
        );
    }
    // Closed acknowledges that both native and admission locks are released;
    // a user may immediately reopen in another access mode after this event.
    drop(_admission);
    output.send(
        ticket,
        EventKind::Closed {
            rollback: result.and(browse_result),
        },
    );
}
struct Query<'a> {
    connection: &'a Connection,
    sql: String,
    parameters: Bindings,
    budget: Budget,
}
impl<'a> Query<'a> {
    fn new(connection: &'a Connection, sql: String, parameters: Bindings, budget: Budget) -> Self {
        Self {
            connection,
            sql,
            parameters,
            budget,
        }
    }
}
#[derive(Clone, Copy, PartialEq)]
enum ReferenceRead {
    Rows,
    Count,
    Peek,
}

struct Worker<'a> {
    path: &'a std::path::Path,
    db: &'a turso::Database,
    editor: &'a Connection,
    browse: &'a Connection,
    access: Access,
    settings: ConnectionSettings,
    output: &'a Output,
    commits: edits::CommitState,
    uncertain: bool,
    depot: crate::values::Depot,
    dirty_data: bool,
    dirty_schema: bool,
    assistant: Option<Connection>,
}
impl<'a> Worker<'a> {
    async fn assistant(
        &mut self,
        ticket: Ticket,
        tool: crate::assistant::Tool,
    ) -> crate::assistant::Reply {
        if self.assistant.is_none() {
            self.assistant = Some(crate::assistant::connect(self.db, self.settings).await?);
        }
        let connection = self.assistant.as_ref().expect("connected above");
        crate::assistant::run(connection, tool, ticket.request, &self.output.discard).await
    }
    fn note_completion(&mut self, ticket: Ticket, effect: sql::Effect, completion: &Completion) {
        self.uncertain |= completion.transaction == tursobench_core::TransactionState::Unknown;
        if completion.error.is_none() {
            match effect {
                sql::Effect::Rollback => {
                    self.dirty_data = false;
                    self.dirty_schema = false;
                }
                sql::Effect::Data => self.dirty_data = true,
                sql::Effect::Schema => {
                    self.dirty_data = true;
                    self.dirty_schema = true;
                }
                sql::Effect::Read => {}
            }
        }
        if completion.transaction == tursobench_core::TransactionState::Autocommit
            && self.dirty_data
        {
            self.output.invalidated(ticket, self.dirty_schema);
            self.dirty_data = false;
            self.dirty_schema = false;
        }
    }
    async fn script(
        &mut self,
        ticket: Ticket,
        text: String,
        parameters: Bindings,
        atomic: bool,
        budget: Budget,
        guard: Option<&tursobench_core::SchemaSnapshot>,
    ) -> Result<EventKind, Failure> {
        let statements = sql::script_statements(&text)?;
        for statement in &statements {
            sql::validate_coordinated(statement)?;
            if guard.is_some() && !crate::schema_compare::additive(statement) {
                return Err(Failure::input(
                    "Guarded migration drafts require additive DDL with an explicit main schema target; use a separate reviewed workflow for destructive/data changes",
                ));
            }
        }
        if self.uncertain
            && statements
                .iter()
                .any(|s| matches!(sql::effect(s), sql::Effect::Data | sql::Effect::Schema))
        {
            return Err(Failure::input(
                "Prior outcome UNKNOWN; reopen and inspect before further writes",
            ));
        }
        if atomic
            && (state(self.editor) != tursobench_core::TransactionState::Autocommit
                || statements.iter().any(|s| sql::transaction_control(s)))
        {
            return Err(Failure::input(
                "Atomic scripts require autocommit and cannot contain transaction commands",
            ));
        }
        if matches!(&parameters,Bindings::Positional(v) if !v.is_empty()) {
            return Err(Failure::input(
                "Use named bindings for parameterized scripts",
            ));
        }
        sql::validate_bindings(&text, &parameters)?;
        if self.access == Access::ReadOnly {
            for s in &statements {
                sql::validate_readonly(s)?;
            }
        }
        let started = std::time::Instant::now();
        let mut summary = Completion {
            elapsed_ms: 0,
            delivery: Delivery::Complete,
            rows_seen: 0,
            page_has_more: None,
            changed: 0,
            transaction: state(self.editor),
            error: None,
        };
        if self.output.discarded(ticket) {
            summary.delivery = Delivery::Discarded;
            return Ok(EventKind::Finished(summary));
        }
        if atomic {
            self.editor
                .execute(
                    if self.access == Access::ReadOnly {
                        "BEGIN"
                    } else {
                        "BEGIN IMMEDIATE"
                    },
                    (),
                )
                .await
                .map_err(failure)?;
        }
        if let Some(expected) = guard {
            let checked = crate::schema_compare::capture(self.editor).await.and_then(|current| {
                if &current == expected { Ok(()) } else { Err(Failure::input("Target schema changed since comparison; refresh and review a new draft")) }
            });
            if let Err(error) = checked {
                if let Err(cleanup) = rollback(self.editor).await {
                    self.uncertain = true;
                    return Err(Failure {
                        kind: ErrorKind::Unknown,
                        message: format!(
                            "Migration validation failed: {error}; rollback uncertain: {cleanup}"
                        ),
                    });
                }
                return Err(error);
            }
        }
        let mut retained_rows = 0usize;
        let mut retained_bytes = 0usize;
        for (index, statement) in statements.iter().enumerate() {
            let bindings = match &parameters {
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
            };
            let capture = Arc::new(std::sync::Mutex::new(Vec::new()));
            let output = Output {
                capture: Some(capture.clone()),
                ..self.output.clone()
            };
            let mut completion = execute(
                self.editor,
                ticket,
                statement,
                bindings,
                Budget {
                    rows: budget.rows.min(10_000).saturating_sub(retained_rows),
                    bytes: budget
                        .bytes
                        .min(8 * 1024 * 1024)
                        .saturating_sub(retained_bytes),
                    page: false,
                    ..budget
                },
                ExecutionContext {
                    out: &output,
                    access: self.access,
                    depot: &mut self.depot,
                    tail: false,
                },
            )
            .await;
            self.note_completion(ticket, sql::effect(statement), &completion);
            let mut columns = Vec::new();
            let mut rows = Vec::new();
            for event in capture.lock().unwrap().drain(..) {
                match event.kind {
                    EventKind::Columns(c) => columns = c,
                    EventKind::Rows(r) => rows.extend(r),
                    _ => {}
                }
            }
            let payload = columns
                .iter()
                .map(|c| c.name.len() + c.declared_type.as_ref().map_or(0, String::len))
                .sum::<usize>()
                + rows.iter().flatten().map(Value::bytes).sum::<usize>()
                + statement.len();
            if retained_bytes.saturating_add(payload) > 8 * 1024 * 1024 {
                rows.clear();
                columns.clear();
                completion.delivery = Delivery::Limited;
            }
            retained_bytes = retained_bytes.saturating_add(payload).min(8 * 1024 * 1024);
            retained_rows += rows.len();
            summary.rows_seen = summary.rows_seen.saturating_add(completion.rows_seen);
            summary.changed = summary.changed.saturating_add(completion.changed);
            summary.transaction = completion.transaction;
            if completion.delivery != Delivery::Complete {
                summary.delivery = completion.delivery;
            }
            summary.error = completion.error.clone();
            self.output.send(
                ticket,
                EventKind::ScriptResult(tursobench_core::ScriptResult {
                    index,
                    sql: statement.clone(),
                    columns,
                    rows,
                    completion,
                }),
            );
            if summary.error.is_some() || self.output.discarded(ticket) {
                break;
            }
        }
        let discarded = self.output.discarded(ticket);
        let mut rolled_back = false;
        if atomic {
            if summary.error.is_some() || discarded {
                match rollback(self.editor).await {
                    Ok(()) => {
                        rolled_back = true;
                        summary.changed = 0;
                        self.dirty_data = false;
                        self.dirty_schema = false;
                        summary.transaction = state(self.editor);
                    }
                    Err(e) => {
                        summary.transaction = tursobench_core::TransactionState::Unknown;
                        summary.error = Some(Failure {
                            kind: ErrorKind::Unknown,
                            message: format!("Atomic rollback outcome UNKNOWN: {e}"),
                        });
                    }
                }
            } else if let Err(e) = self.editor.execute("COMMIT", ()).await {
                summary.transaction = tursobench_core::TransactionState::Unknown;
                summary.error = Some(Failure {
                    kind: ErrorKind::Unknown,
                    message: format!(
                        "Atomic COMMIT outcome UNKNOWN; reopen and inspect before retry: {e}"
                    ),
                });
            } else {
                summary.transaction = state(self.editor);
                self.note_completion(ticket, sql::Effect::Read, &summary);
            }
        }
        if discarded {
            summary.delivery = Delivery::Discarded;
        }
        self.uncertain |= summary.transaction == tursobench_core::TransactionState::Unknown;
        summary.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
        self.output.send(
            ticket,
            EventKind::ScriptOutcome {
                atomic,
                rolled_back,
            },
        );
        Ok(EventKind::Finished(summary))
    }
    async fn export(
        &mut self,
        ticket: Ticket,
        path: PathBuf,
        text: String,
        parameters: Bindings,
        editor: bool,
    ) -> Result<EventKind, Failure> {
        sql::validate_coordinated(&text)?;
        if self.uncertain && matches!(sql::effect(&text), sql::Effect::Data | sql::Effect::Schema) {
            return Err(Failure::input(
                "Prior outcome UNKNOWN; reopen before executing writes",
            ));
        }
        if self.access == Access::ReadOnly {
            sql::validate_readonly(&text)?;
        }
        let connection = if editor { self.editor } else { self.browse };
        if sql::stoppable_read(&text) {
            let c = connection.clone();
            *self.output.discard.active_read.lock().unwrap() = Some(crate::ActiveRead {
                request: ticket.request,
                interrupt: Box::new(move || {
                    let _ = c.interrupt();
                }),
            });
        }
        let result = crate::transfers::export(
            connection,
            &path,
            &text,
            parameters,
            if editor {
                "complete-query-result"
            } else {
                "complete-table"
            },
            || self.output.discarded(ticket),
        )
        .await;
        *self.output.discard.active_read.lock().unwrap() = None;
        let mut outcome = result?;
        if editor {
            let completion = Completion {
                elapsed_ms: 0,
                delivery: Delivery::Complete,
                rows_seen: outcome.rows,
                page_has_more: None,
                changed: outcome.changed,
                transaction: outcome.transaction,
                error: if outcome.executed {
                    None
                } else {
                    outcome.error.clone()
                },
            };
            self.note_completion(ticket, sql::effect(&text), &completion);
        }
        outcome.transaction = state(self.editor);
        Ok(EventKind::TransferFinished(outcome))
    }
    async fn browse_query(
        &self,
        ticket: Ticket,
        spec: tursobench_core::Browse,
    ) -> Result<Query<'a>, Failure> {
        let table = describe(self.browse, &spec.table).await?;
        let relation = crate::browse::relationship(self.browse, &table, &spec).await?;
        let rows = if spec.options.page_rows == 0 {
            100
        } else {
            spec.options.page_rows
        };
        let bytes = if spec.options.page_bytes == 0 {
            4 * 1024 * 1024
        } else {
            spec.options.page_bytes
        };
        let (sql, parameters) = browse_sql(&table, spec, relation)?;
        self.output.send(ticket, EventKind::Table(table));
        Ok(Query::new(
            self.browse,
            sql,
            parameters.into(),
            Budget {
                rows,
                bytes,
                page: true,
                ..Budget::default()
            },
        ))
    }
    async fn reference_query(
        &self,
        ticket: Ticket,
        name: String,
        key: tursobench_core::ForeignKey,
        values: Vec<Value>,
        mode: ReferenceRead,
    ) -> Result<Query<'a>, Failure> {
        let child = inspection::inspect(self.browse, &name).await?;
        let peek = mode == ReferenceRead::Peek;
        if !child.foreign_keys.contains(&key)
            || peek && (key.columns.len() != values.len() || values.contains(&Value::Null))
        {
            return Err(Failure::input(if peek {
                "Relationship metadata changed or key is NULL"
            } else {
                "Relationship metadata changed; refresh the schema"
            }));
        }
        let parent = describe(self.browse, &key.target).await?;
        let default = Budget::default();
        if peek {
            let keys = key
                .columns
                .iter()
                .zip(values)
                .map(|((_, target), value)| {
                    target
                        .clone()
                        .map(|target| (target, value))
                        .ok_or_else(|| Failure::input("Unresolved parent key"))
                })
                .collect::<Result<Vec<_>, _>>()?;
            let (sql, parameters) = inspection::lookup_sql(&parent, keys)?;
            return Ok(Query::new(
                self.browse,
                sql,
                parameters.into(),
                Budget {
                    rows: 1,
                    bytes: 512 * 1024,
                    ..default
                },
            ));
        }
        let count = mode == ReferenceRead::Count;
        let sql = inspection::references_sql(&child.table, &parent, &key, &values, count)?;
        self.output.send(ticket, EventKind::Table(child.table));
        Ok(Query::new(
            self.browse,
            sql,
            values.into(),
            if count {
                default
            } else {
                Budget {
                    rows: 100,
                    ..default
                }
            },
        ))
    }
    async fn import(
        &mut self,
        ticket: Ticket,
        spec: tursobench_core::ImportSpec,
    ) -> Result<EventKind, Failure> {
        if self.uncertain {
            return Err(Failure::input(
                "Prior outcome UNKNOWN; reopen before importing",
            ));
        }
        if self.access != Access::ReadWrite
            || state(self.editor) != tursobench_core::TransactionState::Autocommit
        {
            return Err(Failure::input(
                "Import requires write access and editor autocommit",
            ));
        }
        let outcome = crate::imports::import(self.db, spec, self.settings, || {
            self.output.discarded(ticket)
        })
        .await?;
        self.uncertain |= outcome.transaction == tursobench_core::TransactionState::Unknown;
        if outcome.changed > 0 || outcome.transaction == tursobench_core::TransactionState::Unknown
        {
            self.output.invalidated(ticket, false);
        }
        Ok(EventKind::TransferFinished(outcome))
    }
    async fn export_table(
        &mut self,
        ticket: Ticket,
        path: PathBuf,
        mut spec: tursobench_core::Browse,
    ) -> Result<EventKind, Failure> {
        spec.offset = 0;
        spec.options.cursor = None;
        spec.options.reverse = false;
        let table = describe(self.browse, &spec.table).await?;
        let relation = crate::browse::relationship(self.browse, &table, &spec).await?;
        let (mut text, parameters) = browse_sql(&table, spec, relation)?;
        let end = text
            .rfind(" LIMIT ")
            .ok_or_else(|| Failure::input("Invalid controlled export query"))?;
        text.truncate(end);
        self.export(ticket, path, text, parameters.into(), false)
            .await
    }
    async fn checkpoint(&self) -> Result<EventKind, Failure> {
        if self.uncertain {
            return Err(Failure::input(
                "Prior outcome UNKNOWN; reopen before maintenance",
            ));
        }
        if self.access != Access::ReadWrite {
            return Err(Failure::input("Checkpoint requires write access"));
        }
        crate::transfers::checkpoint(self.editor).await?;
        Ok(EventKind::TransferFinished(
            tursobench_core::TransferOutcome {
                rows: 0,
                changed: 0,
                transaction: state(self.editor),
                error: None,
                message: "Checkpoint completed".into(),
                executed: false,
            },
        ))
    }
    async fn commit_edits(
        &mut self,
        ticket: Ticket,
        batch: tursobench_core::DraftBatch,
    ) -> EventKind {
        if self.uncertain {
            return EventKind::EditsCommitted(tursobench_core::CommitOutcome::Unknown(Failure {
                kind: ErrorKind::Unknown,
                message: "Prior outcome UNKNOWN; reopen before committing".into(),
            }));
        }
        let outcome = self
            .commits
            .run(edits::commit(self.db, self.access, self.settings, batch))
            .await;
        self.uncertain |= matches!(&outcome, tursobench_core::CommitOutcome::Unknown(_));
        if matches!(outcome, tursobench_core::CommitOutcome::Committed { .. }) {
            self.output.invalidated(ticket, false);
        }
        EventKind::EditsCommitted(outcome)
    }

    async fn dispatch(
        &mut self,
        ticket: Ticket,
        operation: Operation,
    ) -> Result<Option<EventKind>, Failure> {
        let effect = match &operation {
            Operation::Execute { sql, .. } | Operation::ExecuteBound { sql, .. } => {
                sql::effect(sql)
            }
            _ => sql::Effect::Read,
        };
        if self.uncertain && matches!(effect, sql::Effect::Data | sql::Effect::Schema) {
            return Err(Failure::input(
                "Prior outcome UNKNOWN; reopen before executing writes",
            ));
        }
        let tail_mode = matches!(&operation,Operation::Browse(spec) if spec.options.tail);
        let default = Budget::default();
        let Query { connection, sql, parameters, budget } = match operation {
            Operation::CaptureSchema => return crate::schema_compare::capture(self.browse).await.map(EventKind::SchemaSnapshot).map(Some),
            Operation::Migration {sql,expected} => {
                if self.access != Access::ReadWrite { return Err(Failure::input("Migration requires explicit read/write access")); }
                return self.script(ticket,sql,vec![].into(),true,default,Some(&expected)).await.map(Some);
            },
            Operation::ReadValue { handle, offset, bytes } => {
                let (bytes, total) = self.depot.read(handle, offset, bytes)?;
                return Ok(Some(EventKind::ValueChunk { handle, offset, bytes, total }));
            }
            Operation::SaveValue { handle, path } => {
                self.depot.save(handle, &path)?;
                return Ok(Some(EventKind::ValueSaved(path.display().to_string())));
            }
            Operation::PreviewImport { path, format, kinds, null_marker } => return crate::imports::preview(&path, format, kinds, null_marker).map(EventKind::ImportPreview).map(Some),
            Operation::Import(spec) => return self.import(ticket, spec).await.map(Some),
            Operation::ExportTable { path, spec } => return self.export_table(ticket, path, spec).await.map(Some),
            Operation::ExportQuery { path, sql, parameters } => return self.export(ticket, path, sql, parameters, true).await.map(Some),
            Operation::Checkpoint => return self.checkpoint().await.map(Some),
            Operation::BackupAndClose(_) => return Err(Failure::input("Backup must be coordinated by the session owner")),
            Operation::Script { sql, parameters, atomic, budget } => return self.script(ticket, sql, parameters, atomic, budget, None).await.map(Some),
            Operation::Close => return Ok(None),
            Operation::Assistant { .. } => return Err(Failure::input("Assistant tools are answered by the session loop")),
            Operation::Admin(change) => {
                if change.is_some() && self.uncertain { return Err(Failure::input("Prior outcome UNKNOWN; reopen before changing settings")); }
                let started = std::time::Instant::now();
                let rows = crate::admin::rows(self.editor,self.browse,self.path,self.access,change).await?;
                let count = rows.len();
                self.output.send(ticket,EventKind::Columns(["Setting","Scope","Effective value"].map(|name| Column {name:name.into(),declared_type:None}).into()));
                self.output.send(ticket,EventKind::Rows(rows));
                return Ok(Some(EventKind::Finished(Completion { elapsed_ms:started.elapsed().as_millis() as u64, delivery:Delivery::Complete,rows_seen:count as u64, page_has_more:None,changed:0,transaction:state(self.editor),error:None })));
            },
            Operation::Profile(request) => {
                let table = describe(self.browse, &request.table).await?;
                let stddev = self.browse.prepare("SELECT stddev(1)").await.is_ok();
                Query::new(self.browse, crate::profile::query(&table, &request, stddev)?, vec![].into(), default)
            },
            Operation::Diagram => return diagram::load(self.browse).await.map(EventKind::Diagram).map(Some),
            Operation::Inspect(name) => return inspection::inspect(self.browse, &name).await.map(EventKind::Inspection).map(Some),
            Operation::CommitEdits(batch) => return Ok(Some(self.commit_edits(ticket, batch).await)),
            Operation::Browse(spec) => self.browse_query(ticket, spec).await?,
            Operation::Lookup { table, keys } => {
                let table = describe(self.browse, &table).await?;
                let (sql, parameters) = inspection::lookup_sql(&table, keys)?;
                self.output.send(ticket, EventKind::Table(table));
                Query::new(self.browse, sql, parameters.into(), default)
            }
            Operation::References { table, key, values } => self.reference_query(ticket, table, key, values, ReferenceRead::Rows).await?,
            Operation::ReferenceCount { table, key, values } => self.reference_query(ticket, table, key, values, ReferenceRead::Count).await?,
            Operation::Peek { table, key, values } => self.reference_query(ticket, table, key, values, ReferenceRead::Peek).await?,
            Operation::CountFiltered(spec) => {
                let table = describe(self.browse, &spec.table).await?;
                let relation = crate::browse::relationship(self.browse, &table, &spec).await?;
                let (sql, params) = crate::browse::count_sql(&table, &spec, relation)?;
                Query::new(self.browse, sql, params.into(), default)
            }
            Operation::Count(name) => Query::new(self.browse, format!("SELECT count(*) AS row_count FROM {}", quote_identifier(&name)), vec![].into(), default),
            Operation::Definition(name) => Query::new(self.browse, "SELECT sql FROM sqlite_schema WHERE name=?1".into(), vec![Value::Text(name)].into(), default),
            Operation::Diagnostic(crate::Diagnostic::ForeignKeys) => Query::new(self.browse, crate::validation::foreign_keys(self.browse).await?, vec![].into(), default),
            Operation::Diagnostic(kind) => Query::new(self.browse, kind.sql().into(), vec![].into(), default),
            Operation::Schema => Query::new(self.browse, "SELECT type, name, tbl_name, CASE WHEN length(CAST(sql AS BLOB))<=16384 THEN sql ELSE NULL END AS sql FROM sqlite_schema WHERE name NOT GLOB 'sqlite_*' ORDER BY CASE type WHEN 'table' THEN 0 WHEN 'view' THEN 1 ELSE 2 END, name".into(), vec![].into(), Budget { rows: 10_000, bytes: 8*1024*1024, cell_bytes: 1024*1024, page: false }),
            Operation::Explain { sql, parameters, bytecode } => Query::new(self.editor, sql::explain_sql(&sql, bytecode)?, parameters, default),
            Operation::ExecuteBound { sql, parameters, budget } => Query::new(self.editor, sql, parameters, budget),
            Operation::Execute { sql, parameters, budget } => Query::new(self.editor, sql, parameters.into(), budget),
        };
        let mut completion = execute(
            connection,
            ticket,
            &sql,
            parameters,
            budget,
            ExecutionContext {
                out: self.output,
                access: self.access,
                depot: &mut self.depot,
                tail: tail_mode,
            },
        )
        .await;
        // Every result reports the persistent editor's state, including browse/diagnostics.
        completion.transaction = state(self.editor);
        self.note_completion(ticket, effect, &completion);
        Ok(Some(EventKind::Finished(completion)))
    }
}
struct ExecutionContext<'a> {
    out: &'a Output,
    access: Access,
    depot: &'a mut crate::values::Depot,
    tail: bool,
}
/// Retention and delivery budgets are independent of engine execution. Writes
/// drain their rows even when this buffer refuses further output.
struct ResultDelivery<'a> {
    out: &'a Output,
    ticket: Ticket,
    budget: Budget,
    tail: bool,
    chunk: Vec<Vec<Value>>,
    suffix: std::collections::VecDeque<(Vec<Value>, usize)>,
    kept: usize,
    bytes: usize,
    chunk_bytes: usize,
}
impl<'a> ResultDelivery<'a> {
    fn new(out: &'a Output, ticket: Ticket, budget: Budget, tail: bool) -> Self {
        Self {
            out,
            ticket,
            budget,
            tail,
            chunk: Vec::new(),
            suffix: Default::default(),
            kept: 0,
            bytes: 0,
            chunk_bytes: 0,
        }
    }
    fn push(
        &mut self,
        mut values: Vec<Value>,
        depot: &mut crate::values::Depot,
        completion: &mut Completion,
    ) -> bool {
        self.defer_cells(&mut values, depot);
        let size = values.iter().map(Value::bytes).sum::<usize>();
        if self.tail {
            self.retain_tail(values, size, completion);
            return true;
        }
        if self.kept >= self.budget.rows.min(10_000)
            || self.bytes + size > self.budget.bytes.min(8 * 1024 * 1024)
            || values
                .iter()
                .any(|v| v.bytes() > self.budget.cell_bytes.min(1024 * 1024))
        {
            if self.budget.page && self.kept > 0 {
                completion.page_has_more = Some(true);
            } else {
                completion.delivery = Delivery::Limited;
            }
            return false;
        }
        self.bytes += size;
        self.kept += 1;
        if !self.chunk(values, size) {
            completion.delivery = Delivery::Discarded;
        }
        true
    }
    fn defer_cells(&self, values: &mut [Value], depot: &mut crate::values::Depot) {
        if self.budget.cell_bytes == 0 {
            return;
        }
        for value in values.iter_mut() {
            if value.bytes() > self.budget.cell_bytes.min(1024 * 1024) {
                *value = depot.defer(std::mem::replace(value, Value::Null));
            }
        }
        if self.budget.page && values.iter().map(Value::bytes).sum::<usize>() > self.budget.bytes {
            for value in values {
                if matches!(value, Value::Text(_) | Value::Blob(_)) {
                    *value = depot.defer(std::mem::replace(value, Value::Null));
                }
            }
        }
    }
    fn retain_tail(&mut self, values: Vec<Value>, size: usize, completion: &mut Completion) {
        while self.bytes + size > self.budget.bytes
            || self.suffix.len() >= self.budget.rows.min(100)
        {
            let Some((_, evicted_bytes)) = self.suffix.pop_front() else {
                break;
            };
            self.bytes -= evicted_bytes;
            completion.page_has_more = Some(true);
        }
        if size > self.budget.bytes {
            completion.delivery = Delivery::Limited;
            return;
        }
        self.bytes += size;
        self.suffix.push_back((values, size));
    }
    fn chunk(&mut self, values: Vec<Value>, size: usize) -> bool {
        self.chunk_bytes += size;
        self.chunk.push(values);
        if self.chunk.len() < 64 && self.chunk_bytes < 64 * 1024 {
            return true;
        }
        self.chunk_bytes = 0;
        self.out.send(
            self.ticket,
            EventKind::Rows(std::mem::take(&mut self.chunk)),
        )
    }
    fn finish(mut self) {
        while let Some((values, size)) = self.suffix.pop_front() {
            self.chunk(values, size);
        }
        if !self.chunk.is_empty() {
            self.out.send(self.ticket, EventKind::Rows(self.chunk));
        }
    }
}

async fn execute(
    c: &Connection,
    ticket: Ticket,
    sql: &str,
    parameters: Bindings,
    budget: Budget,
    context: ExecutionContext<'_>,
) -> Completion {
    let ExecutionContext {
        out,
        access,
        depot,
        tail,
    } = context;
    let started = std::time::Instant::now();
    let stoppable = sql::stoppable_read(sql);
    if stoppable {
        let connection = c.clone();
        *out.discard.active_read.lock().unwrap() = Some(crate::ActiveRead {
            request: ticket.request,
            interrupt: Box::new(move || {
                let _ = connection.interrupt();
            }),
        });
    }
    let mut completion = Completion {
        elapsed_ms: 0,
        delivery: Delivery::Complete,
        rows_seen: 0,
        page_has_more: budget.page.then_some(false),
        changed: 0,
        transaction: state(c),
        error: None,
    };
    let mut delivery = ResultDelivery::new(out, ticket, budget, tail);
    let result = async {
        sql::validate_single(sql)?;
        if stoppable && out.discarded(ticket) {
            completion.delivery = Delivery::Discarded;
            return Ok(());
        }
        if access == Access::ReadOnly {
            sql::validate_readonly(sql)?;
        }
        sql::validate_coordinated(sql)?;
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
            Bindings::Positional(values) => turso::params::Params::Positional(
                values
                    .into_iter()
                    .map(to_engine)
                    .collect::<Result<_, _>>()?,
            ),
            Bindings::Named(values) => turso::params::Params::Named(
                values
                    .into_iter()
                    .map(|(name, value)| to_engine(value).map(|value| (name.into(), value)))
                    .collect::<Result<_, _>>()?,
            ),
        };
        if stoppable && out.discarded(ticket) {
            completion.delivery = Delivery::Discarded;
            return Ok(());
        }
        let mut rows = stmt.query(parameters).await.map_err(failure)?;
        while let Some(row) = rows.next().await.map_err(failure)? {
            completion.rows_seen += 1;
            if out.discarded(ticket) {
                completion.delivery = Delivery::Discarded;
                delivery.chunk.clear();
                if stoppable {
                    break;
                }
                continue;
            }
            if completion.delivery == Delivery::Limited {
                continue;
            }
            let values = (0..rows.column_count())
                .map(|i| row.get_value(i).map(from_engine).map_err(failure))
                .collect::<Result<_, _>>()?;
            if !delivery.push(values, depot, &mut completion) && stoppable {
                break;
            }
        }
        if budget.page && delivery.kept >= budget.rows {
            completion.page_has_more = Some(true);
        }
        completion.changed = stmt.n_change();
        Ok::<(), Failure>(())
    }
    .await;
    if stoppable {
        *out.discard.active_read.lock().unwrap() = None;
    }
    delivery.finish();
    completion.elapsed_ms = started.elapsed().as_millis().min(u64::MAX as u128) as u64;
    completion.error = result.err();
    completion.transaction = state(c);
    if out.discarded(ticket) {
        completion.delivery = Delivery::Discarded;
    }
    completion
}
