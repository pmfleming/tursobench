use std::{collections::BTreeMap, path::Path, time::Duration};
use tursobench_core::*;
use tursobench_db::{Diagnostic, Operation, Request, Session};
fn fixture(path: &Path) {
    tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async{
    let db=turso::Builder::new_local(path.to_str().unwrap()).experimental_index_method(true).build().await.unwrap();let c=db.connect().unwrap();
    for sql in [
        "CREATE TABLE parent(a INTEGER, b TEXT, payload TEXT, PRIMARY KEY(a,b)) STRICT",
        "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b TEXT,note TEXT,FOREIGN KEY(a,b) REFERENCES parent ON DELETE CASCADE) STRICT",
        "CREATE INDEX child_note_idx ON child(note) WHERE note IS NOT NULL",
        "CREATE INDEX child_fts ON child USING fts(note)",
        "INSERT INTO parent VALUES(7,'001','original')",
        "INSERT INTO child VALUES(1,7,'001','needle'),(2,NULL,NULL,NULL)",
    ]{c.execute(sql,()).await.unwrap();}
});
}
struct Client {
    session: Session,
    sequence: u64,
}
struct Reply {
    result: ResultSet,
    info: Option<Inspection>,
    failure: Option<Failure>,
}
impl Client {
    fn open(path: &Path, access: Access) -> Self {
        let session = Session::open(path.into(), access, true, || {}).unwrap();
        assert!(matches!(
            session
                .events
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .kind,
            EventKind::Opened
        ));
        Self {
            session,
            sequence: 0,
        }
    }
    fn run(&mut self, operation: Operation) -> Reply {
        self.sequence += 1;
        let ticket = self.session.ticket(self.sequence, 0);
        self.session.submit(Request { ticket, operation }).unwrap();
        let mut result = ResultSet::default();
        result.start(ticket);
        loop {
            let event = self
                .session
                .events
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            assert_eq!(event.ticket, ticket);
            match event.kind {
                EventKind::Inspection(info) => {
                    return Reply {
                        result,
                        info: Some(info),
                        failure: None,
                    };
                }
                EventKind::Failed(e) => {
                    return Reply {
                        result,
                        info: None,
                        failure: Some(e),
                    };
                }
                _ => {
                    let done = matches!(event.kind, EventKind::Finished(_));
                    result.apply(event);
                    if done {
                        return Reply {
                            result,
                            info: None,
                            failure: None,
                        };
                    }
                }
            }
        }
    }
    fn sql(&mut self, sql: &str) -> ResultSet {
        let reply = self.run(Operation::Execute {
            sql: sql.into(),
            parameters: vec![],
            budget: Budget::default(),
        });
        assert!(reply.failure.is_none());
        assert!(
            reply.result.completion.as_ref().unwrap().error.is_none(),
            "{}: {:?}",
            sql,
            reply.result.completion
        );
        reply.result
    }
    fn diagram(&mut self) -> SchemaDiagram {
        self.sequence += 1;
        let ticket = self.session.ticket(self.sequence, 0);
        self.session
            .submit(Request {
                ticket,
                operation: Operation::Diagram,
            })
            .unwrap();
        let event = self
            .session
            .events
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert_eq!(event.ticket, ticket);
        match event.kind {
            EventKind::Diagram(graph) => graph,
            other => panic!("Expected diagram: {other:?}"),
        }
    }
    fn close(self) {
        self.session.close();
        assert!(matches!(
            self.session
                .events
                .recv_timeout(Duration::from_secs(5))
                .unwrap()
                .kind,
            EventKind::Closed { rollback: Ok(()) }
        ));
    }
}
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|entry| {
            let e = entry.unwrap();
            (
                e.file_name().to_string_lossy().into_owned(),
                std::fs::read(e.path()).unwrap(),
            )
        })
        .collect()
}
#[test]
fn native_metadata_related_lookup_and_diagnostics_preserve_readonly_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workbench.db");
    fixture(&path);
    let before = snapshot(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    let reply = client.run(Operation::Inspect("child".into()));
    assert!(reply.failure.is_none(), "{:?}", reply.failure);
    let info = reply.info.unwrap();
    assert_eq!(info.table.columns.len(), 4);
    assert!(info.indexes.iter().any(|i| i.name == "child_note_idx"
        && i.partial
        && i.columns == vec![Some("note".into())]));
    assert!(info.indexes.iter().any(|i| i.name == "child_fts"));
    assert_eq!(info.foreign_keys.len(), 1);
    assert_eq!(info.foreign_keys[0].on_delete, "CASCADE");
    assert!(
        client
            .run(Operation::Inspect("x".repeat(64 * 1024 + 1)))
            .failure
            .is_some()
    );
    assert_eq!(
        info.foreign_keys[0].columns,
        vec![
            ("a".into(), Some("a".into())),
            ("b".into(), Some("b".into()))
        ]
    );
    let related = client.run(Operation::Lookup {
        table: "parent".into(),
        keys: vec![
            ("a".into(), Value::Integer(7)),
            ("b".into(), Value::Text("001".into())),
        ],
    });
    assert_eq!(
        related.result.rows,
        vec![vec![
            Value::Integer(7),
            Value::Text("001".into()),
            Value::Text("original".into())
        ]]
    );
    assert!(
        client
            .run(Operation::Lookup {
                table: "parent".into(),
                keys: vec![("a\" OR 1=1 --".into(), Value::Integer(7))]
            })
            .failure
            .is_some()
    );
    assert_eq!(
        client.run(Operation::Count("child".into())).result.rows,
        vec![vec![Value::Integer(2)]]
    );
    for diagnostic in Diagnostic::ALL {
        let result = client.run(Operation::Diagnostic(diagnostic)).result;
        assert!(
            result.completion.as_ref().unwrap().error.is_none(),
            "{diagnostic:?}: {:?}",
            result.completion
        );
        if diagnostic == Diagnostic::Integrity {
            assert_eq!(result.rows, vec![vec![Value::Text("ok".into())]]);
        }
        if diagnostic == Diagnostic::ForeignKeys {
            assert!(result.rows.is_empty());
        }
    }
    client.close();
    assert_eq!(snapshot(dir.path()), before);
}
#[test]
fn diagram_includes_isolated_quoted_cyclic_and_composite_tables_without_writes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("diagram.db");
    fixture(&path);
    let mut client = Client::open(&path, Access::ReadWrite);
    for sql in [
        "CREATE TABLE isolated(value TEXT)",
        "CREATE TABLE sqlitex_visible(id INTEGER PRIMARY KEY)",
        "CREATE TABLE self_ref(id INTEGER PRIMARY KEY, parent_id INTEGER REFERENCES self_ref(id))",
        "CREATE TABLE cycle_a(id INTEGER PRIMARY KEY, b INTEGER REFERENCES cycle_b(id))",
        "CREATE TABLE cycle_b(id INTEGER PRIMARY KEY, a INTEGER REFERENCES cycle_a(id))",
        "CREATE TABLE \"quoted \"\" table\"(id INTEGER PRIMARY KEY, token TEXT UNIQUE)",
        "CREATE TABLE unresolved(id INTEGER PRIMARY KEY, absent INTEGER REFERENCES missing_parent)",
        "CREATE TABLE uppercase_ref(a INTEGER, b TEXT, FOREIGN KEY(a,b) REFERENCES PARENT)",
        "CREATE TABLE \"résumé\"(\"clé\" TEXT PRIMARY KEY)",
        "CREATE TABLE \"élément\"(\"parent\" TEXT REFERENCES \"résumé\"(\"clé\"))",
    ] {
        client.sql(sql);
    }
    client.close();
    let before = snapshot(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    let graph = client.diagram();
    assert!(!graph.limited);
    assert_eq!(
        graph.tables.len(),
        13,
        "{:?}",
        graph.tables.iter().map(|t| &t.name).collect::<Vec<_>>()
    );
    let internal = graph
        .tables
        .iter()
        .find(|t| t.name.starts_with("__turso_internal_fts_"))
        .unwrap()
        .inspection
        .as_ref()
        .unwrap();
    assert!(internal.table.engine_managed());
    assert!(!internal.table.editable());
    assert!(graph.tables.iter().all(|t| t.error.is_none()));
    let info = |name: &str| {
        graph
            .tables
            .iter()
            .find(|t| t.name == name)
            .unwrap()
            .inspection
            .as_ref()
            .unwrap()
    };
    assert!(info("isolated").table.keys().is_empty());
    assert_eq!(info("élément").foreign_keys[0].target, "résumé");
    assert_eq!(
        info("élément").foreign_keys[0].columns,
        vec![("parent".into(), Some("clé".into()))]
    );
    assert_eq!(info("self_ref").foreign_keys[0].target, "self_ref");
    assert_eq!(info("cycle_a").foreign_keys[0].target, "cycle_b");
    assert_eq!(info("cycle_b").foreign_keys[0].target, "cycle_a");
    assert!(info("quoted \" table").indexes.iter().any(|i| i.unique));
    assert_eq!(
        info("child").foreign_keys[0].columns,
        vec![
            ("a".into(), Some("a".into())),
            ("b".into(), Some("b".into()))
        ]
    );
    assert_eq!(
        info("uppercase_ref").foreign_keys[0].columns,
        info("child").foreign_keys[0].columns
    );
    assert_eq!(
        info("unresolved").foreign_keys[0].columns,
        vec![("absent".into(), None)]
    );
    client.close();
    assert_eq!(snapshot(dir.path()), before);
}
#[test]
fn diagram_uses_committed_schema_not_the_editor_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("snapshot.db");
    fixture(&path);
    let mut client = Client::open(&path, Access::ReadWrite);
    client.sql("BEGIN");
    client.sql("CREATE TABLE pending_schema(id INTEGER PRIMARY KEY)");
    client.sql("ALTER TABLE child ADD COLUMN pending_column TEXT");
    let graph = client.diagram();
    assert!(!graph.tables.iter().any(|t| t.name == "pending_schema"));
    assert_eq!(
        graph
            .tables
            .iter()
            .find(|t| t.name == "child")
            .unwrap()
            .inspection
            .as_ref()
            .unwrap()
            .table
            .columns
            .len(),
        4
    );
    client.sql("ROLLBACK");
    client.close();
}
#[test]
fn diagram_marks_discovery_budget_and_keeps_result_ownership_separate() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("budget.db");
    fixture(&path);
    let mut client = Client::open(&path, Access::ReadWrite);
    client.sql("BEGIN");
    for i in 0..511 {
        client.sql(&format!("CREATE TABLE t{i:04}(id INTEGER PRIMARY KEY)"));
    }
    client.sql("COMMIT");
    let retained = client.sql("SELECT 42 AS retained");
    let graph = client.diagram();
    assert!(graph.limited);
    assert_eq!(graph.tables.len(), 512);
    assert_eq!(retained.rows, vec![vec![Value::Integer(42)]]);
    client.close();
}
#[test]
fn named_parameters_and_explain_are_bound_and_non_mutating() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workbench.db");
    fixture(&path);
    let mut client = Client::open(&path, Access::ReadWrite);
    let parameters = Bindings::Named(BTreeMap::from([
        (
            ":payload".into(),
            Value::Text("'; DELETE FROM parent; --".into()),
        ),
        ("$id".into(), Value::Integer(7)),
    ]));
    let result = client
        .run(Operation::ExecuteBound {
            sql: "SELECT :payload,:payload,$id".into(),
            parameters,
            budget: Budget::default(),
        })
        .result;
    assert!(
        result.completion.as_ref().unwrap().error.is_none(),
        "{:?}",
        result.completion
    );
    assert_eq!(
        result.rows[0],
        vec![
            Value::Text("'; DELETE FROM parent; --".into()),
            Value::Text("'; DELETE FROM parent; --".into()),
            Value::Integer(7)
        ]
    );
    let blob = client.run(Operation::ExecuteBound {
        sql: "SELECT @blob, @nil, @real".into(),
        parameters: Bindings::Named(BTreeMap::from([
            ("@blob".into(), Value::Blob(vec![0, 255])),
            ("@nil".into(), Value::Null),
            ("@real".into(), Value::Real(1.5)),
        ])),
        budget: Budget::default(),
    });
    assert_eq!(
        blob.result.rows,
        vec![vec![
            Value::Blob(vec![0, 255]),
            Value::Null,
            Value::Real(1.5)
        ]]
    );
    for bytecode in [false, true] {
        let reply = client.run(Operation::Explain {
            sql: "DELETE FROM child WHERE id=:id".into(),
            parameters: Bindings::Named(BTreeMap::from([(":id".into(), Value::Integer(1))])),
            bytecode,
        });
        assert!(reply.failure.is_none());
        assert!(
            reply.result.completion.as_ref().unwrap().error.is_none(),
            "{:?}",
            reply.result.completion
        );
        assert!(!reply.result.rows.is_empty());
        assert_eq!(reply.result.completion.unwrap().changed, 0);
    }
    assert_eq!(
        client.sql("SELECT count(*) FROM child").rows,
        vec![vec![Value::Integer(2)]]
    );
    for sql in [
        "PRAGMA foreign_keys=OFF",
        "BEGIN",
        "CREATE TABLE unwanted(x)",
        "DELETE FROM child; DELETE FROM parent",
    ] {
        assert!(
            client
                .run(Operation::Explain {
                    sql: sql.into(),
                    parameters: vec![].into(),
                    bytecode: true
                })
                .failure
                .is_some()
        );
    }
    let missing = client.run(Operation::ExecuteBound {
        sql: "DELETE FROM child WHERE id=:id".into(),
        parameters: Bindings::Named(BTreeMap::new()),
        budget: Budget::default(),
    });
    assert!(missing.result.completion.unwrap().error.is_some());
    assert_eq!(
        client.sql("SELECT count(*) FROM child").rows,
        vec![vec![Value::Integer(2)]]
    );
    client.close();
}
