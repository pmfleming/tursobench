//! Engine handles, execution, delivery budgets and transaction cleanup stay on the worker.
//!
//! This module routes requests; each request kind is handled beside its data:
//! reads build queries, scripts and transfers own their transaction rules, and
//! `execute` owns streaming delivery.
mod execute;
mod reads;
mod script;
mod transfer;

use crate::{
    Operation, Request,
    connection::{self, Engine, rollback, state},
    diagram, edits, inspection,
    observe::Watch,
    sql,
};
use execute::{ExecutionContext, Query, execute};
use std::{
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{Receiver, RecvTimeoutError, SyncSender, TrySendError},
    },
    time::Duration,
};
use turso::Connection;
use tursobench_core::{
    Access, ConnectionSettings, Event, EventKind, Failure, Ticket, TransactionState,
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
        self.discard.discarded(t.request) || self.closing()
    }
    fn closing(&self) -> bool {
        self.closing.load(Ordering::Acquire)
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
    /// Publishes a request's terminal event. Retiring it first acknowledges
    /// that it is no longer pending, so immediate ID reuse cannot race cleanup.
    fn finish(&self, ticket: Ticket, kind: EventKind) {
        self.discard.finished(ticket.request);
        self.send(ticket, kind);
    }
}
fn with_usage(mut failure: Failure, path: &Path) -> Failure {
    let usage = crate::processes_using(path);
    if !usage.applications.is_empty() {
        failure
            .message
            .push_str(&format!(" Also open in {}.", usage.applications.join(", ")));
    }
    failure
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
    let engine = match connection::open(path, access, settings).await {
        Ok(engine) => engine,
        Err(e) => {
            output.send(ticket, EventKind::Failed(with_usage(e, &source_path)));
            return;
        }
    };
    let mut watch = match Watch::start(&source_path, &engine.browse, engine.coordination).await {
        Ok(watch) => watch,
        Err(e) => {
            output.send(ticket, EventKind::Failed(e));
            return;
        }
    };
    if !output.send(ticket, EventKind::Opened(engine.coordination)) {
        return;
    }
    let mut worker = Worker::new(&source_path, &engine, access, settings, &output);
    let backup = worker.serve(ticket, &rx, &mut watch).await;
    drop(worker);
    let Engine {
        db,
        editor,
        browse,
        admission,
        ..
    } = engine;
    let editor_rollback = rollback(&editor).await;
    let cleanup = editor_rollback.and(rollback(&browse).await);
    drop(browse);
    drop(editor);
    drop(db);
    if let Some((request, destination)) = backup {
        let outcome =
            crate::transfers::publish_backup(&source_path, &destination, settings, &cleanup).await;
        output.finish(request, EventKind::TransferFinished(outcome));
    }
    // Closed acknowledges that both native and admission locks are released;
    // a user may immediately reopen in another access mode after this event.
    drop(admission);
    output.send(ticket, EventKind::Closed { rollback: cleanup });
}

struct Worker<'a> {
    path: &'a Path,
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
    fn new(
        path: &'a Path,
        engine: &'a Engine,
        access: Access,
        settings: ConnectionSettings,
        output: &'a Output,
    ) -> Self {
        Self {
            path,
            db: &engine.db,
            editor: &engine.editor,
            browse: &engine.browse,
            access,
            settings,
            output,
            commits: edits::CommitState::default(),
            uncertain: false,
            depot: crate::values::Depot::default(),
            dirty_data: false,
            dirty_schema: false,
            assistant: None,
        }
    }
    /// Answers requests until close, returning a prepared backup to publish
    /// once the engine has closed.
    async fn serve(
        &mut self,
        ticket: Ticket,
        rx: &Receiver<Request>,
        watch: &mut Watch,
    ) -> Option<(Ticket, PathBuf)> {
        while !self.output.closing() {
            match watch.poll(self.browse).await {
                Ok(Some(schema)) => self.output.invalidated(ticket, schema),
                Ok(None) => {}
                Err(e) => {
                    self.output.send(ticket, EventKind::Failed(e));
                    break;
                }
            }
            let Request { ticket, operation } = match watch.receive(rx) {
                Ok(request) => request,
                Err(RecvTimeoutError::Timeout) => continue,
                Err(RecvTimeoutError::Disconnected) => break,
            };
            if self.output.closing() {
                break;
            }
            let event = match operation {
                Operation::BackupAndClose(destination) => {
                    match self.prepare_backup(&destination).await {
                        Ok(()) => {
                            self.output.closing.store(true, Ordering::Release);
                            return Some((ticket, destination));
                        }
                        Err(e) => EventKind::Failed(e),
                    }
                }
                Operation::Assistant { tool, reply } => {
                    let answer = self.assistant(ticket, tool).await;
                    self.output.discard.finished(ticket.request);
                    // The caller may have timed out and gone; that is not an error here.
                    let _ = reply.try_send(answer);
                    continue;
                }
                operation => match self.dispatch(ticket, operation).await {
                    Ok(Some(event)) => event,
                    Ok(None) => break,
                    Err(error) => EventKind::Failed(error),
                },
            };
            self.output.finish(ticket, event);
        }
        None
    }
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
    /// Refuses further work after an outcome the session could not confirm.
    fn settled(&self, action: &str) -> Result<(), Failure> {
        if self.uncertain {
            return Err(Failure::input(format!("Prior outcome UNKNOWN; {action}")));
        }
        Ok(())
    }
    fn note_completion(
        &mut self,
        ticket: Ticket,
        effect: sql::Effect,
        completion: &tursobench_core::Completion,
    ) {
        self.uncertain |= completion.transaction == TransactionState::Unknown;
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
        if completion.transaction == TransactionState::Autocommit && self.dirty_data {
            self.output.invalidated(ticket, self.dirty_schema);
            self.dirty_data = false;
            self.dirty_schema = false;
        }
    }
    async fn commit_edits(
        &mut self,
        ticket: Ticket,
        batch: tursobench_core::DraftBatch,
    ) -> EventKind {
        use tursobench_core::CommitOutcome;
        if self.uncertain {
            return EventKind::EditsCommitted(CommitOutcome::Unknown(Failure {
                kind: tursobench_core::ErrorKind::Unknown,
                message: "Prior outcome UNKNOWN; reopen before committing".into(),
            }));
        }
        let outcome = self
            .commits
            .run(edits::commit(self.db, self.access, self.settings, batch))
            .await;
        self.uncertain |= matches!(&outcome, CommitOutcome::Unknown(_));
        if matches!(outcome, CommitOutcome::Committed { .. }) {
            self.output.invalidated(ticket, false);
        }
        EventKind::EditsCommitted(outcome)
    }
    /// Streams query-shaped requests through `execute`; `answer` handles the rest.
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
        if effect.writes() {
            self.settled("reopen before executing writes")?;
        }
        let tail = matches!(&operation, Operation::Browse(spec) if spec.options.tail);
        let query = match operation {
            Operation::Close => return Ok(None),
            Operation::Browse(spec) => self.browse_query(ticket, spec).await?,
            Operation::CountFiltered(spec) => self.count_filtered(&spec).await?,
            Operation::Lookup { table, keys } => self.lookup(ticket, &table, keys).await?,
            Operation::References { table, key, values } => {
                self.reference_query(ticket, table, key, values, reads::Reference::Rows)
                    .await?
            }
            Operation::ReferenceCount { table, key, values } => {
                self.reference_query(ticket, table, key, values, reads::Reference::Count)
                    .await?
            }
            Operation::Peek { table, key, values } => {
                self.reference_query(ticket, table, key, values, reads::Reference::Peek)
                    .await?
            }
            Operation::Profile(request) => self.profile(&request).await?,
            Operation::Diagnostic(kind) => self.diagnostic(kind).await?,
            Operation::Count(name) => self.count(&name),
            Operation::Definition(name) => self.definition(name),
            Operation::Schema => self.catalog(),
            Operation::Explain {
                sql,
                parameters,
                bytecode,
            } => Query::read(self.editor, sql::explain_sql(&sql, bytecode)?, parameters),
            Operation::ExecuteBound {
                sql,
                parameters,
                budget,
            } => Query::new(self.editor, sql, parameters, budget),
            Operation::Execute {
                sql,
                parameters,
                budget,
            } => Query::new(self.editor, sql, parameters.into(), budget),
            operation => return self.answer(ticket, operation).await.map(Some),
        };
        let mut completion = execute(
            ticket,
            query,
            ExecutionContext {
                out: self.output,
                access: self.access,
                depot: &mut self.depot,
                tail,
            },
        )
        .await;
        // Every result reports the persistent editor's state, including browse/diagnostics.
        completion.transaction = state(self.editor);
        self.note_completion(ticket, effect, &completion);
        Ok(Some(EventKind::Finished(completion)))
    }
    /// Requests answered by one event rather than a streamed result.
    async fn answer(&mut self, ticket: Ticket, operation: Operation) -> Result<EventKind, Failure> {
        match operation {
            Operation::CaptureSchema => crate::schema_compare::capture(self.browse)
                .await
                .map(EventKind::SchemaSnapshot),
            Operation::Migration { sql, expected } => self.migrate(ticket, sql, &expected).await,
            Operation::Script {
                sql,
                parameters,
                atomic,
                budget,
            } => {
                self.script(ticket, sql, parameters, atomic, budget, None)
                    .await
            }
            Operation::Admin(change) => self.admin(ticket, change).await,
            Operation::Diagram => diagram::load(self.browse).await.map(EventKind::Diagram),
            Operation::Inspect(name) => inspection::inspect(self.browse, &name)
                .await
                .map(EventKind::Inspection),
            Operation::CommitEdits(batch) => Ok(self.commit_edits(ticket, batch).await),
            Operation::ReadValue {
                handle,
                offset,
                bytes,
            } => {
                let (bytes, total) = self.depot.read(handle, offset, bytes)?;
                Ok(EventKind::ValueChunk {
                    handle,
                    offset,
                    bytes,
                    total,
                })
            }
            Operation::SaveValue { handle, path } => {
                self.depot.save(handle, &path)?;
                Ok(EventKind::ValueSaved(path.display().to_string()))
            }
            Operation::PreviewImport {
                path,
                format,
                kinds,
                null_marker,
            } => crate::imports::preview(&path, format, kinds, null_marker)
                .map(EventKind::ImportPreview),
            Operation::Import(spec) => self.import(ticket, spec).await,
            Operation::ExportTable { path, spec } => self.export_table(ticket, path, spec).await,
            Operation::ExportQuery {
                path,
                sql,
                parameters,
            } => self.export(ticket, path, sql, parameters, true).await,
            Operation::Checkpoint => self.checkpoint().await,
            _ => Err(Failure::input(
                "Streamed, backup and assistant requests are routed by the session loop",
            )),
        }
    }
}
