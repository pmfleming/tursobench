use std::{path::Path, time::Duration};
use tursobench_core::{
    Access, Browse, Budget, CommitOutcome, DraftBatch, Edit, ErrorKind, Event, EventKind, Failure,
    Inspection, ResultSet, SchemaDiagram, Table,
};
use tursobench_db::{Operation, Request, Session};

pub mod blob_fixtures;
pub mod local_fixtures;

mod fixture;
pub use fixture::{create, run as block_on};
/// Every file under a directory, to prove an operation left them byte-identical.
pub struct Files(
    std::path::PathBuf,
    std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
);
impl Files {
    pub fn record(root: &Path) -> Self {
        Self(root.into(), fixture::snapshot(root))
    }
    pub fn unchanged(&self) -> bool {
        fixture::snapshot(&self.0) == self.1
    }
}
pub fn event(session: &Session) -> Event {
    session
        .events
        .recv_timeout(Duration::from_secs(30))
        .expect("session event before timeout")
}
/// Starts a session, returning it with its first (open or failure) event.
pub fn start(path: &Path, access: Access) -> (Session, EventKind) {
    let session = Session::open(path.into(), access, || {}).unwrap();
    let kind = event(&session).kind;
    (session, kind)
}
pub fn opened(path: &Path, access: Access) -> Session {
    let (session, opened) = start(path, access);
    assert!(
        matches!(opened, EventKind::Opened(_)),
        "open {}: {opened:?}",
        path.display()
    );
    session
}
#[derive(Default)]
pub struct Reply {
    pub snapshot: Option<tursobench_core::SchemaSnapshot>,
    pub transfer: Option<tursobench_core::TransferOutcome>,
    pub preview: Option<tursobench_core::ImportPreview>,
    pub script: Vec<tursobench_core::ScriptResult>,
    pub rolled_back: bool,
    pub chunk: Option<(u64, Vec<u8>)>,
    pub saved: Option<String>,
    pub result: ResultSet,
    pub table: Option<Table>,
    pub info: Option<Inspection>,
    pub graph: Option<SchemaDiagram>,
    pub outcome: Option<CommitOutcome>,
    pub failure: Option<Failure>,
}
impl Reply {
    /// Panics unless the request and its statement both succeeded.
    pub fn assert_succeeded(&self) {
        assert!(self.failure.is_none(), "{:?}", self.failure);
        let completion = self.result.completion.as_ref().unwrap();
        assert!(completion.error.is_none(), "{completion:?}");
    }
}
/// The first page of `table`, unfiltered and in key order.
pub fn browse(table: &str) -> Browse {
    Browse {
        options: Default::default(),
        table: table.into(),
        offset: 0,
        sort: None,
        descending: false,
        filters: vec![],
    }
}
pub fn run(session: &Session, sequence: u64, operation: Operation) -> Reply {
    let ticket = session.ticket(sequence, 0);
    session.submit(Request { ticket, operation }).unwrap();
    let mut reply = Reply::default();
    reply.result.start(ticket);
    loop {
        let event = event(session);
        if matches!(event.kind, EventKind::Invalidated { .. }) {
            continue;
        }
        assert_eq!(event.ticket, ticket);
        // Script results, table metadata and streamed rows precede the answer.
        let terminal = !matches!(
            event.kind,
            EventKind::ScriptResult(_)
                | EventKind::ScriptOutcome { .. }
                | EventKind::Table(_)
                | EventKind::Columns(_)
                | EventKind::Rows(_)
        );
        match event.kind {
            EventKind::SchemaSnapshot(snapshot) => reply.snapshot = Some(snapshot),
            EventKind::ImportPreview(preview) => reply.preview = Some(preview),
            EventKind::TransferFinished(outcome) => reply.transfer = Some(outcome),
            EventKind::ScriptResult(result) => {
                reply.result.columns = result.columns.clone();
                reply.result.rows = result.rows.clone();
                reply.script.push(result);
            }
            EventKind::ScriptOutcome { rolled_back, .. } => reply.rolled_back = rolled_back,
            EventKind::ValueChunk { offset, bytes, .. } => reply.chunk = Some((offset, bytes)),
            EventKind::ValueSaved(path) => reply.saved = Some(path),
            EventKind::Table(table) => reply.table = Some(table),
            EventKind::Inspection(info) => reply.info = Some(info),
            EventKind::Diagram(graph) => reply.graph = Some(graph),
            EventKind::EditsCommitted(outcome) => reply.outcome = Some(outcome),
            EventKind::Failed(error) => reply.failure = Some(error),
            EventKind::Finished(_) | EventKind::Columns(_) | EventKind::Rows(_) => {
                reply.result.apply(event);
            }
            other => panic!("Unexpected response: {other:?}"),
        }
        if terminal {
            return reply;
        }
    }
}
pub struct Client {
    pub session: Session,
    sequence: u64,
}
impl Client {
    pub fn open(path: &Path, access: Access) -> Self {
        Self {
            session: opened(path, access),
            sequence: 0,
        }
    }
    pub fn run(&mut self, operation: Operation) -> Reply {
        self.sequence += 1;
        run(&self.session, self.sequence, operation)
    }
    pub fn query(&mut self, sql: &str, parameters: Vec<tursobench_core::Value>) -> ResultSet {
        let reply = self.run(Operation::Execute {
            sql: sql.into(),
            parameters,
            budget: Budget::default(),
        });
        assert!(reply.failure.is_none(), "{:?}", reply.failure);
        reply.result
    }
    /// The error `sql` ended with, if any.
    pub fn failure(&mut self, sql: &str) -> Option<Failure> {
        self.query(sql, vec![]).completion.unwrap().error
    }
    pub fn sql(&mut self, sql: &str) -> ResultSet {
        let result = self.query(sql, vec![]);
        assert!(
            result.completion.as_ref().unwrap().error.is_none(),
            "{sql}: {:?}",
            result.completion
        );
        result
    }
    /// The row count of `table` (an SQL table expression).
    pub fn count(&mut self, table: &str) -> i64 {
        let rows = self.sql(&format!("SELECT count(*) FROM {table}")).rows;
        match rows[..] {
            [ref row] => match row[..] {
                [tursobench_core::Value::Integer(n)] => n,
                ref other => panic!("count {table}: {other:?}"),
            },
            ref other => panic!("count {table}: {other:?}"),
        }
    }
    pub fn browse(&mut self, name: &str, offset: usize) -> (Table, ResultSet) {
        let reply = self.run(Operation::Browse(Browse {
            offset,
            ..browse(name)
        }));
        assert!(
            reply.result.completion.as_ref().unwrap().error.is_none(),
            "{name}: {:?}",
            reply.result.completion
        );
        (reply.table.unwrap(), reply.result)
    }
    pub fn commit(&mut self, table: Table, edits: Vec<Edit>) -> CommitOutcome {
        self.run(Operation::CommitEdits(DraftBatch { table, edits }))
            .outcome
            .unwrap()
    }
    /// Commits `changes` to the first row of `table`'s first page.
    pub fn update_first(
        &mut self,
        table: &str,
        changes: Vec<(usize, tursobench_core::Value)>,
    ) -> CommitOutcome {
        let (table, page) = self.browse(table, 0);
        let original = page.rows[0].clone();
        self.commit(table, vec![Edit::Update { original, changes }])
    }
    pub fn inspect(&mut self, table: &str) -> Inspection {
        let reply = self.run(Operation::Inspect(table.into()));
        reply
            .info
            .unwrap_or_else(|| panic!("{table}: {:?}", reply.failure))
    }
    /// Whether the worker refuses `operation` outright.
    pub fn refuses(&mut self, operation: Operation) -> bool {
        self.run(operation).failure.is_some()
    }
    pub fn diagram(&mut self) -> SchemaDiagram {
        self.run(Operation::Diagram).graph.unwrap()
    }
    pub fn close(self) {
        close(&self.session);
    }
}
/// Panics unless `outcome` committed, and exactly `operations` when given.
pub fn committed(outcome: &CommitOutcome, operations: Option<usize>) {
    assert!(
        matches!(outcome, CommitOutcome::Committed { operations: n } if operations.is_none_or(|o| o == *n)),
        "{outcome:?}"
    );
}
/// Panics unless `outcome` was rejected with `kind`.
pub fn rejected(outcome: &CommitOutcome, kind: ErrorKind) {
    assert!(
        matches!(outcome, CommitOutcome::Rejected(e) if e.kind == kind),
        "{outcome:?}"
    );
}
/// Closes `session` and waits for its clean rollback acknowledgement.
pub fn close(session: &Session) {
    session.close();
    loop {
        match event(session).kind {
            EventKind::Invalidated { .. } => continue,
            EventKind::Closed { rollback: Ok(()) } => break,
            other => panic!("Unexpected close response: {other:?}"),
        }
    }
}
/// An unbound script under the default budget.
pub fn script(sql: &str, atomic: bool) -> Operation {
    Operation::Script {
        sql: sql.into(),
        parameters: vec![].into(),
        atomic,
        budget: Budget::default(),
    }
}
/// Unbound SQL under `budget`.
pub fn execute(sql: &str, budget: Budget) -> Operation {
    Operation::Execute {
        sql: sql.into(),
        parameters: vec![],
        budget,
    }
}
pub fn submit(session: &Session, request: u64, operation: Operation) {
    let ticket = session.ticket(request, 0);
    session.submit(Request { ticket, operation }).unwrap();
}
/// A disposable database named `name`, created from `statements`. The
/// directory is removed when the returned guard drops.
pub fn database(name: &str, statements: &[&str]) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(name);
    create(&path, statements);
    (dir, path)
}
