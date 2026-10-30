#[path = "common/local_fixtures.rs"]
mod fixtures;
use std::{collections::BTreeMap, path::Path, time::Duration};
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session};

struct Client {
    session: Session,
    sequence: u64,
}
impl Client {
    fn open(path: &Path, access: Access) -> Self {
        let session = Session::open(path.into(), access, true, || {}).unwrap();
        let event = session
            .events
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(
            matches!(event.kind, EventKind::Opened),
            "open {}: {:?}",
            path.display(),
            event.kind
        );
        Self {
            session,
            sequence: 0,
        }
    }
    fn request(
        &mut self,
        operation: Operation,
    ) -> (ResultSet, Option<Table>, Option<CommitOutcome>) {
        self.sequence += 1;
        let ticket = self.session.ticket(self.sequence, 0);
        self.session.submit(Request { ticket, operation }).unwrap();
        let mut result = ResultSet::default();
        result.start(ticket);
        let mut table = None;
        loop {
            let event = self
                .session
                .events
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
            assert_eq!(event.ticket, ticket);
            match event.kind {
                EventKind::Failed(e) => panic!("request failed: {e}"),
                EventKind::Table(t) => table = Some(t),
                EventKind::EditsCommitted(outcome) => return (result, table, Some(outcome)),
                EventKind::Finished(_) => {
                    result.apply(event);
                    return (result, table, None);
                }
                _ => {
                    result.apply(event);
                }
            }
        }
    }
    fn query(&mut self, sql: &str, parameters: Vec<Value>) -> ResultSet {
        self.request(Operation::Execute {
            sql: sql.into(),
            parameters,
            budget: Budget::default(),
        })
        .0
    }
    fn sql(&mut self, sql: &str) -> ResultSet {
        let r = self.query(sql, vec![]);
        assert!(
            r.completion.as_ref().unwrap().error.is_none(),
            "{sql}: {:?}",
            r.completion
        );
        r
    }
    fn browse(&mut self, name: &str, offset: usize) -> (Table, ResultSet) {
        let (r, t, _) = self.request(Operation::Browse(Browse {
            table: name.into(),
            offset,
            sort: None,
            descending: false,
            filter: None,
        }));
        assert!(
            r.completion.as_ref().unwrap().error.is_none(),
            "{name}: {:?}",
            r.completion
        );
        (t.unwrap(), r)
    }
    fn commit(&mut self, table: Table, edits: Vec<Edit>) -> CommitOutcome {
        self.request(Operation::CommitEdits(DraftBatch { table, edits }))
            .2
            .unwrap()
    }
    fn close(self) {
        self.session.close();
        let event = self
            .session
            .events
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(
            matches!(event.kind, EventKind::Closed { rollback: Ok(()) }),
            "{:?}",
            event.kind
        );
    }
}
fn fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.db"));
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(fixtures::create(&path, name))
        .unwrap();
    (dir, path)
}
fn bytes(root: &Path) -> BTreeMap<std::path::PathBuf, Vec<u8>> {
    fn walk(root: &Path, path: &Path, into: &mut BTreeMap<std::path::PathBuf, Vec<u8>>) {
        for entry in std::fs::read_dir(path).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                walk(root, &path, into);
            } else {
                into.insert(
                    path.strip_prefix(root).unwrap().into(),
                    std::fs::read(path).unwrap(),
                );
            }
        }
    }
    let mut files = BTreeMap::new();
    walk(root, root, &mut files);
    files
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn matches(c: &mut Client, needle: &str) -> Vec<Vec<Value>> {
    let r = c.query(
        "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1) ORDER BY entry_id",
        vec![text(needle)],
    );
    assert!(r.completion.unwrap().error.is_none());
    r.rows
}

#[test]
fn exact_project_schemas_open_browse_filter_page_and_preserve_readonly_files() {
    for name in fixtures::NAMES {
        let (dir, path) = fixture(name);
        let before = bytes(dir.path());
        let mut c = Client::open(&path, Access::ReadOnly);
        let schema = c.request(Operation::Schema).0;
        assert!(schema.completion.unwrap().error.is_none());
        for row in schema.rows {
            if row[0] == text("table") {
                let Value::Text(table) = &row[1] else {
                    panic!()
                };
                let (description, page) = c.browse(table, 0);
                assert!(description.is_table);
                assert_eq!(page.columns.len(), description.columns.len());
                assert!(page.rows.len() <= 100);
            }
        }
        let paged = match name {
            "scratchpad" => "buffer_ids",
            "agenda" => "items",
            "notifications" => "notifications",
            _ => "entries",
        };
        let (table, first) = c.browse(paged, 0);
        let (_, second) = c.browse(paged, 100);
        let (_, last) = c.browse(paged, 200);
        assert_eq!(
            (first.rows.len(), second.rows.len(), last.rows.len()),
            (100, 100, fixtures::ROWS - 200)
        );
        assert_ne!(first.rows[0], second.rows[0]);
        let key = table.keys()[0];
        let wanted = first.rows[0][key].clone();
        let (filtered, _, _) = c.request(Operation::Browse(Browse {
            table: paged.into(),
            offset: 0,
            sort: Some(key),
            descending: true,
            filter: Some((key, wanted)),
        }));
        assert_eq!(filtered.rows, vec![first.rows[0].clone()]);
        assert_eq!(
            c.query(&format!("DELETE FROM {}", quote_identifier(paged)), vec![])
                .completion
                .unwrap()
                .error
                .unwrap()
                .kind,
            ErrorKind::ReadOnly
        );
        if name == "clipboard" {
            assert_eq!(matches(&mut c, "needle"), vec![vec![text("entry-0000")]]);
            let large = c.sql("SELECT bytes FROM payloads WHERE entry_id='entry-0204'");
            assert!(large.rows.is_empty());
            assert_eq!(large.completion.unwrap().delivery, Delivery::Limited);
        }
        if name == "scratchpad" {
            assert_eq!(
                c.sql("SELECT length(body) FROM blobs ORDER BY bytes DESC LIMIT 1")
                    .rows,
                vec![vec![Value::Integer(1024 * 1024)]]
            );
        }
        if name == "notifications" {
            assert_eq!(first.rows[0][8], Value::Null);
        }
        c.close();
        assert_eq!(
            before,
            bytes(dir.path()),
            "read-only access changed {name} fixture files"
        );
        eprintln!(
            "PASS {name}: schema/all-table browse, typed page/filter, read-only rejection, byte preservation"
        );
    }
}

#[test]
fn native_fts_creation_queries_returning_and_engine_readonly_enforcement() {
    let (_dir, path) = fixture("agenda");
    let mut c = Client::open(&path, Access::ReadWrite);
    c.sql("CREATE TABLE documents(id INTEGER PRIMARY KEY, body TEXT NOT NULL) STRICT");
    c.sql("CREATE INDEX documents_fts ON documents USING fts(body)");
    assert_eq!(
        c.sql("INSERT INTO documents VALUES(1,'hello Turso'),(2,'hello world') RETURNING id")
            .rows
            .len(),
        2
    );
    assert_eq!(
        c.sql("SELECT id FROM documents WHERE fts_match(body,'Turso')")
            .rows,
        vec![vec![Value::Integer(1)]]
    );
    let plan = c.sql("EXPLAIN QUERY PLAN SELECT id FROM documents WHERE fts_match(body,'Turso')");
    assert!(
        plan.rows
            .iter()
            .flatten()
            .any(|v| matches!(v, Value::Text(s) if s.contains("fts")))
    );
    c.close();
    let mut c = Client::open(&path, Access::ReadOnly);
    assert_eq!(
        c.sql("SELECT id FROM documents WHERE fts_match(body,'Turso')")
            .rows
            .len(),
        1
    );
    assert_eq!(
        c.query(
            "CREATE INDEX denied_fts ON documents USING fts(body)",
            vec![]
        )
        .completion
        .unwrap()
        .error
        .unwrap()
        .kind,
        ErrorKind::ReadOnly
    );
    assert_eq!(
        c.query("UPDATE documents SET body='changed' WHERE id=1", vec![])
            .completion
            .unwrap()
            .error
            .unwrap()
            .kind,
        ErrorKind::ReadOnly
    );
    c.close();
}

#[test]
fn clipboard_fts_staged_edits_conflicts_constraints_and_transaction_rollback() {
    let (_dir, path) = fixture("clipboard");
    let mut c = Client::open(&path, Access::ReadWrite);
    let (table, original) = c.browse("search_content", 0);
    assert!(table.editable());
    let row = original.rows[0].clone();
    let outcome = c.commit(
        table.clone(),
        vec![Edit::Update {
            original: row.clone(),
            changes: vec![(1, text("changed marker")), (2, text("changed marker"))],
        }],
    );
    assert!(
        matches!(outcome, CommitOutcome::Committed { operations: 1 }),
        "{outcome:?}"
    );
    assert!(matches(&mut c, "needle").is_empty());
    assert_eq!(matches(&mut c, "marker"), vec![vec![text("entry-0000")]]);
    assert!(matches!(
        c.commit(table.clone(), vec![Edit::Delete { original: row }]),
        CommitOutcome::Rejected(Failure {
            kind: ErrorKind::Conflict,
            ..
        })
    ));
    let (_, current) = c.browse("search_content", 0);
    let outcome = c.commit(
        table.clone(),
        vec![
            Edit::Update {
                original: current.rows[0].clone(),
                changes: vec![(2, text("rollback token"))],
            },
            Edit::Insert {
                values: vec![
                    Some(text("missing-parent")),
                    Some(text("bad")),
                    Some(text("bad")),
                    Some(Value::Integer(1)),
                ],
            },
        ],
    );
    assert!(
        matches!(
            outcome,
            CommitOutcome::Rejected(Failure {
                kind: ErrorKind::Constraint,
                ..
            })
        ),
        "{outcome:?}"
    );
    assert!(matches(&mut c, "rollback").is_empty());
    assert_eq!(matches(&mut c, "marker").len(), 1);
    c.sql("BEGIN IMMEDIATE");
    c.sql(
        "UPDATE search_content SET indexed_content='uncommitted token' WHERE entry_id='entry-0000'",
    );
    assert_eq!(matches(&mut c, "uncommitted").len(), 1);
    let (_, page) = c.browse("search_content", 0);
    assert_eq!(page.rows[0][2], text("changed marker"));
    assert_eq!(
        page.completion.unwrap().transaction,
        TransactionState::Active
    );
    c.sql("ROLLBACK");
    assert!(matches(&mut c, "uncommitted").is_empty());
    assert_eq!(matches(&mut c, "marker").len(), 1);
    let outcome = c.commit(
        table.clone(),
        vec![Edit::Delete {
            original: current.rows[0].clone(),
        }],
    );
    assert!(
        matches!(outcome, CommitOutcome::Committed { .. }),
        "{outcome:?}"
    );
    assert!(matches(&mut c, "marker").is_empty());
    let outcome = c.commit(
        table,
        vec![Edit::Insert {
            values: vec![
                Some(text("entry-0000")),
                Some(text("inserted searchable")),
                Some(text("inserted searchable")),
                Some(Value::Integer(1)),
            ],
        }],
    );
    assert!(
        matches!(outcome, CommitOutcome::Committed { operations: 1 }),
        "{outcome:?}"
    );
    assert_eq!(
        matches(&mut c, "searchable"),
        vec![vec![text("entry-0000")]]
    );
    c.close();
    let mut c = Client::open(&path, Access::ReadOnly);
    assert!(matches(&mut c, "marker").is_empty());
    assert_eq!(
        matches(&mut c, "searchable"),
        vec![vec![text("entry-0000")]]
    );
    c.close();
}

#[test]
fn project_typed_edits_and_close_rollback() {
    for (name, table_name, column, value) in [
        (
            "scratchpad",
            "history",
            3,
            Value::Blob(vec![0, 255, 13, 10]),
        ),
        ("agenda", "items", 4, Value::Integer(2)),
        (
            "notifications",
            "notifications",
            8,
            Value::Integer(1700000002000),
        ),
    ] {
        let (_dir, path) = fixture(name);
        let mut c = Client::open(&path, Access::ReadWrite);
        let (table, before) = c.browse(table_name, 0);
        assert!(matches!(
            c.commit(
                table,
                vec![Edit::Update {
                    original: before.rows[0].clone(),
                    changes: vec![(column, value.clone())]
                }]
            ),
            CommitOutcome::Committed { .. }
        ));
        let (_, after) = c.browse(table_name, 0);
        assert_eq!(after.rows[0][column], value);
        c.sql("BEGIN");
        c.sql(&format!("DELETE FROM {}", quote_identifier(table_name)));
        c.close();
        let mut c = Client::open(&path, Access::ReadOnly);
        let (_, reopened) = c.browse(table_name, 0);
        assert_eq!(after.rows, reopened.rows);
        c.close();
    }
}
