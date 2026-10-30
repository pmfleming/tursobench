use std::time::Duration;
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session};
struct Harness {
    _dir: tempfile::TempDir,
    session: Session,
    n: u64,
}
impl Harness {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("edits.db");
        tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
            let db = turso::Builder::new_local(path.to_str().unwrap()).build().await.unwrap();
            let c = db.connect().unwrap();
            c.execute("CREATE TABLE items(a INTEGER NOT NULL, b TEXT NOT NULL, data BLOB, label TEXT DEFAULT 'default', PRIMARY KEY(a,b))", ()).await.unwrap();
            c.execute("INSERT INTO items VALUES (1,'key',X'00FF',NULL), (2,'other',NULL,'')", ()).await.unwrap();
        });
        let session = Session::open(path, Access::ReadWrite, true, || {}).unwrap();
        assert!(matches!(
            session
                .events
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .kind,
            EventKind::Opened
        ));
        Self {
            _dir: dir,
            session,
            n: 0,
        }
    }
    fn request(&mut self, operation: Operation) -> Vec<EventKind> {
        self.n += 1;
        self.session
            .submit(Request {
                ticket: self.session.ticket(self.n, 0),
                operation,
            })
            .unwrap();
        let mut events = Vec::new();
        loop {
            let e = self
                .session
                .events
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .kind;
            let done = matches!(
                e,
                EventKind::Finished(_) | EventKind::Failed(_) | EventKind::EditsCommitted(_)
            );
            events.push(e);
            if done {
                return events;
            }
        }
    }
    fn sql(&mut self, sql: &str) -> Vec<Vec<Value>> {
        self.request(Operation::Execute {
            sql: sql.into(),
            parameters: vec![],
            budget: Budget::default(),
        })
        .into_iter()
        .flat_map(|e| match e {
            EventKind::Rows(r) => r,
            EventKind::Finished(c) => {
                assert!(c.error.is_none(), "{:?}", c.error);
                vec![]
            }
            _ => vec![],
        })
        .collect()
    }
    fn browse(&mut self) -> (Table, Vec<Vec<Value>>) {
        let events = self.request(Operation::Browse(Browse {
            table: "items".into(),
            offset: 0,
            sort: None,
            descending: false,
            filter: None,
        }));
        let mut table = None;
        let mut rows = vec![];
        for e in events {
            match e {
                EventKind::Table(t) => table = Some(t),
                EventKind::Rows(r) => rows.extend(r),
                EventKind::Failed(e) => panic!("{e}"),
                _ => {}
            }
        }
        (table.unwrap(), rows)
    }
    fn commit(&mut self, table: Table, edits: Vec<Edit>) -> CommitOutcome {
        match self
            .request(Operation::CommitEdits(DraftBatch { table, edits }))
            .pop()
            .unwrap()
        {
            EventKind::EditsCommitted(c) => c,
            _ => panic!(),
        }
    }
}
#[test]
fn composite_changed_keys_null_blob_and_defaults() {
    let mut h = Harness::new();
    h.sql(
        "CREATE TABLE nullable_keys(a TEXT, b INTEGER, data TEXT COLLATE NOCASE, PRIMARY KEY(a,b))",
    );
    h.sql("INSERT INTO nullable_keys VALUES('x',NULL,'a'),('x',NULL,'A')");
    let nullable = h.request(Operation::Browse(Browse {
        table: "nullable_keys".into(),
        offset: 0,
        sort: None,
        descending: false,
        filter: None,
    }));
    let ordered: Vec<_> = nullable
        .into_iter()
        .flat_map(|e| match e {
            EventKind::Rows(r) => r,
            _ => vec![],
        })
        .collect();
    assert_eq!(ordered[0][2], Value::Text("A".into()));
    assert_eq!(ordered[1][2], Value::Text("a".into()));
    let (table, rows) = h.browse();
    assert!(table.editable());
    assert!(matches!(
        h.commit(
            table.clone(),
            vec![Edit::Update {
                original: rows[0].clone(),
                changes: vec![
                    (0, Value::Integer(3)),
                    (1, Value::Text("changed".into())),
                    (2, Value::Blob(vec![1, 2, 3])),
                    (3, Value::Text(String::new()))
                ]
            }]
        ),
        CommitOutcome::Committed { operations: 1 }
    ));
    let (_, rows) = h.browse();
    assert_eq!(rows[1][0], Value::Integer(3));
    assert_eq!(rows[1][2], Value::Blob(vec![1, 2, 3]));
    assert_eq!(rows[1][3], Value::Text(String::new()));
    assert!(matches!(
        h.commit(
            table.clone(),
            vec![Edit::Insert {
                values: vec![
                    Some(Value::Integer(4)),
                    Some(Value::Text("default".into())),
                    Some(Value::Null),
                    None
                ]
            }]
        ),
        CommitOutcome::Committed { .. }
    ));
    assert_eq!(
        h.sql("SELECT label FROM items WHERE a=4"),
        vec![vec![Value::Text("default".into())]]
    );
    assert!(matches!(
        h.commit(
            table,
            vec![Edit::Delete {
                original: rows[0].clone()
            }]
        ),
        CommitOutcome::Committed { .. }
    ));
    assert_eq!(
        h.sql("SELECT count(*) FROM items"),
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn conflict_and_mid_batch_failure_are_atomic() {
    let mut h = Harness::new();
    let (table, rows) = h.browse();
    h.sql("UPDATE items SET label='external change' WHERE a=1");
    let batch = vec![
        Edit::Update {
            original: rows[1].clone(),
            changes: vec![(3, Value::Text("must roll back".into()))],
        },
        Edit::Delete {
            original: rows[0].clone(),
        },
    ];
    assert!(matches!(
        h.commit(table.clone(), batch),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Conflict,
            ..
        })
    ));
    assert_eq!(
        h.sql("SELECT label FROM items WHERE a=2"),
        vec![vec![Value::Text(String::new())]]
    );
    let (_, rows) = h.browse();
    let batch = vec![
        Edit::Delete {
            original: rows[1].clone(),
        },
        Edit::Insert {
            values: vec![
                Some(Value::Integer(1)),
                Some(Value::Text("key".into())),
                None,
                None,
            ],
        },
    ];
    assert!(matches!(
        h.commit(table, batch),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Constraint,
            ..
        })
    ));
    assert_eq!(
        h.sql("SELECT count(*) FROM items"),
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn generated_edits_enforce_foreign_keys_even_if_editor_disables_them() {
    let mut h = Harness::new();
    h.sql("CREATE TABLE child(a INTEGER, b TEXT, FOREIGN KEY(a,b) REFERENCES items(a,b))");
    h.sql("INSERT INTO child VALUES(1, 'key')");
    h.sql("PRAGMA foreign_keys=OFF");
    let (table, rows) = h.browse();
    assert!(matches!(
        h.commit(
            table,
            vec![Edit::Delete {
                original: rows[0].clone()
            }]
        ),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Constraint,
            ..
        })
    ));
    assert_eq!(
        h.sql("SELECT count(*) FROM items"),
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn editor_transaction_is_not_joined_and_schema_changes_rejected() {
    let mut h = Harness::new();
    let (table, rows) = h.browse();
    h.sql("BEGIN IMMEDIATE");
    assert!(matches!(
        h.commit(
            table.clone(),
            vec![Edit::Delete {
                original: rows[0].clone()
            }]
        ),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Busy,
            ..
        })
    ));
    h.sql("ROLLBACK");
    h.sql("ALTER TABLE items ADD COLUMN extra TEXT");
    assert!(matches!(
        h.commit(
            table,
            vec![Edit::Delete {
                original: rows[0].clone()
            }]
        ),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Conflict,
            ..
        })
    ));
}
