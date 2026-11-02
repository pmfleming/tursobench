use std::{path::Path, time::Duration};
use tursobench_core::{
    Access, Browse, Budget, CommitOutcome, DraftBatch, Edit, Event, EventKind, Failure, Inspection,
    ResultSet, SchemaDiagram, Table,
};
use tursobench_db::{Operation, Request, Session};

pub mod local_fixtures;

mod fixture;
pub use fixture::{create, snapshot};
pub fn event(session: &Session) -> Event {
    session
        .events
        .recv_timeout(Duration::from_secs(30))
        .expect("session event before timeout")
}
pub fn opened(path: &Path, access: Access) -> Session {
    let session = Session::open(path.into(), access, true, || {}).unwrap();
    assert!(
        matches!(event(&session).kind, EventKind::Opened),
        "open {}",
        path.display()
    );
    session
}
#[derive(Default)]
pub struct Reply {
    pub result: ResultSet,
    pub table: Option<Table>,
    pub info: Option<Inspection>,
    pub graph: Option<SchemaDiagram>,
    pub outcome: Option<CommitOutcome>,
    pub failure: Option<Failure>,
}
pub fn run(session: &Session, sequence: u64, operation: Operation) -> Reply {
    let ticket = session.ticket(sequence, 0);
    session.submit(Request { ticket, operation }).unwrap();
    let mut reply = Reply::default();
    reply.result.start(ticket);
    loop {
        let event = event(session);
        assert_eq!(event.ticket, ticket);
        match event.kind {
            EventKind::Table(table) => {
                reply.table = Some(table);
                continue;
            }
            EventKind::Inspection(info) => reply.info = Some(info),
            EventKind::Diagram(graph) => reply.graph = Some(graph),
            EventKind::EditsCommitted(outcome) => reply.outcome = Some(outcome),
            EventKind::Failed(error) => reply.failure = Some(error),
            EventKind::Finished(_) => {
                reply.result.apply(event);
            }
            EventKind::Columns(_) | EventKind::Rows(_) => {
                reply.result.apply(event);
                continue;
            }
            other => panic!("Unexpected response: {other:?}"),
        }
        return reply;
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
    pub fn sql(&mut self, sql: &str) -> ResultSet {
        let result = self.query(sql, vec![]);
        assert!(
            result.completion.as_ref().unwrap().error.is_none(),
            "{sql}: {:?}",
            result.completion
        );
        result
    }
    pub fn browse(&mut self, name: &str, offset: usize) -> (Table, ResultSet) {
        let reply = self.run(Operation::Browse(Browse {
            table: name.into(),
            offset,
            sort: None,
            descending: false,
            filter: None,
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
    pub fn diagram(&mut self) -> SchemaDiagram {
        self.run(Operation::Diagram).graph.unwrap()
    }
    pub fn close(self) {
        self.session.close();
        assert!(matches!(
            event(&self.session).kind,
            EventKind::Closed { rollback: Ok(()) }
        ));
    }
}
