use crate::common::{self, Client, snapshot};
use std::{collections::BTreeMap, path::Path};
use tursobench_core::*;
use tursobench_db::{Diagnostic, Operation};
fn fixture(path: &Path) {
    common::create(
        path,
        &[
            "CREATE TABLE parent(a INTEGER, b TEXT, payload TEXT, PRIMARY KEY(a,b)) STRICT",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b TEXT,note TEXT,FOREIGN KEY(a,b) REFERENCES parent ON DELETE CASCADE) STRICT",
            "CREATE INDEX child_note_idx ON child(note) WHERE note IS NOT NULL",
            "CREATE INDEX child_fts ON child USING fts(note)",
            "INSERT INTO parent VALUES(7,'001','original')",
            "INSERT INTO child VALUES(1,7,'001','needle'),(2,NULL,NULL,NULL)",
        ],
    );
}

#[test]
fn foreign_key_check_detects_orphans_and_respects_parent_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("orphans.db");
    common::create(
        &path,
        &[
            "CREATE TABLE p(a INTEGER, b TEXT COLLATE NOCASE, PRIMARY KEY(a,b))",
            "CREATE TABLE c(id INTEGER PRIMARY KEY,a TEXT,b TEXT,FOREIGN KEY(a,b) REFERENCES p)",
            "INSERT INTO p VALUES(7,'Alpha')",
            "INSERT INTO c VALUES(1,'007','alpha'),(2,'8','orphan'),(3,NULL,'exempt')",
        ],
    );
    let mut client = Client::open(&path, Access::ReadOnly);
    let result = client.run(Operation::Diagnostic(Diagnostic::ForeignKeys));
    assert!(result.failure.is_none(), "{:?}", result.failure);
    assert!(result.result.completion.as_ref().unwrap().error.is_none());
    assert_eq!(result.result.rows.len(), 1);
    assert_eq!(result.result.rows[0][3], Value::Integer(1));
    assert_eq!(result.result.rows[0][4], Value::Integer(2));
    client.close();
}

#[test]
fn catalog_preserves_large_definitions_and_more_than_one_thousand_indexes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("catalog.db");
    common::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
    let mut client = Client::open(&path, Access::ReadWrite);
    for n in 0..1001 {
        client.sql(&format!("CREATE INDEX i{n} ON t(id)"));
    }
    client.sql(&format!(
        "CREATE TABLE large(id INTEGER PRIMARY KEY, payload TEXT DEFAULT '{}')",
        "x".repeat(76_000)
    ));
    let result = client.run(Operation::Schema).result;
    assert_eq!(
        result.completion.as_ref().unwrap().delivery,
        Delivery::Complete
    );
    assert!(
        result
            .rows
            .iter()
            .any(|r| r[1] == Value::Text("large".into()))
    );
    assert!(result.rows.iter().any(|r| r[1] == Value::Text("t".into())));
    assert_eq!(result.rows.len(), 1003);
    client.close();
}

#[test]
fn readonly_vacuum_into_and_attach_do_not_create_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("source.db");
    common::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
    let before = snapshot(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    for sql in [
        format!("VACUUM INTO '{}'", dir.path().join("copy.db").display()),
        format!(
            "ATTACH '{}' AS other",
            dir.path().join("attached.db").display()
        ),
    ] {
        let result = client.query(&sql, vec![]);
        assert_eq!(
            result.completion.unwrap().error.unwrap().kind,
            ErrorKind::ReadOnly
        );
    }
    client.close();
    assert_eq!(snapshot(dir.path()), before);
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
            assert!(result.rows.iter().all(|r| r[3] == Value::Integer(0)));
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

#[test]
fn typed_filter_chips_bind_values_escape_contains_and_combine_null_predicates() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("filters.db");
    common::create(
        &path,
        &[
            r#"CREATE TABLE "filter rows"(id INTEGER PRIMARY KEY, "odd""col" TEXT, n INTEGER, bytes BLOB)"#,
            r#"INSERT INTO "filter rows" VALUES(1,'100%_\literal',1,X'00FF'),(2,'x'' OR 1=1 --',3,X'01'),(3,NULL,9223372036854775807,NULL)"#,
        ],
    );
    let before = snapshot(dir.path());
    let mut c = Client::open(&path, Access::ReadOnly);
    let mut run = |filters: Vec<Filter>| {
        c.run(Operation::Browse(Browse {
            table: "filter rows".into(),
            offset: 0,
            sort: None,
            descending: false,
            filters,
        }))
    };
    let f = |column, op, value| Filter { column, op, value };
    for (filters, ids) in [
        (
            vec![f(1, FilterOp::Contains, Value::Text("%_\\".into()))],
            vec![1],
        ),
        (
            vec![f(1, FilterOp::Equal, Value::Text("x' OR 1=1 --".into()))],
            vec![2],
        ),
        (
            vec![f(3, FilterOp::Equal, Value::Blob(vec![0, 255]))],
            vec![1],
        ),
        (
            vec![f(2, FilterOp::Equal, Value::Integer(i64::MAX))],
            vec![3],
        ),
        (
            vec![f(2, FilterOp::LessOrEqual, Value::Integer(3))],
            vec![1, 2],
        ),
        (
            vec![f(2, FilterOp::GreaterOrEqual, Value::Integer(3))],
            vec![2, 3],
        ),
        (vec![f(1, FilterOp::IsNull, Value::Null)], vec![3]),
        (vec![f(1, FilterOp::Equal, Value::Null)], vec![3]),
        (vec![f(1, FilterOp::NotEqual, Value::Null)], vec![1, 2]),
        (
            vec![
                f(1, FilterOp::IsNotNull, Value::Null),
                f(0, FilterOp::Equal, Value::Integer(2)),
            ],
            vec![2],
        ),
        (
            vec![
                f(2, FilterOp::Greater, Value::Integer(1)),
                f(2, FilterOp::Less, Value::Integer(4)),
            ],
            vec![2],
        ),
    ] {
        let reply = run(filters);
        assert!(reply.failure.is_none(), "{:?}", reply.failure);
        assert!(
            reply.result.completion.as_ref().unwrap().error.is_none(),
            "{:?}",
            reply.result.completion
        );
        assert_eq!(
            reply
                .result
                .rows
                .iter()
                .map(|r| r[0].clone())
                .collect::<Vec<_>>(),
            ids.into_iter().map(Value::Integer).collect::<Vec<_>>()
        );
    }
    assert!(
        run(vec![f(0, FilterOp::Contains, Value::Integer(1))])
            .failure
            .is_some()
    );
    assert!(
        run(vec![f(99, FilterOp::Equal, Value::Integer(1))])
            .failure
            .is_some()
    );
    assert!(
        run(vec![f(0, FilterOp::Equal, Value::Integer(1)); 33])
            .failure
            .is_some()
    );
    c.close();
    assert_eq!(snapshot(dir.path()), before);
}

#[test]
fn reverse_references_use_parent_affinity_and_collation_and_reject_changed_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("reverse.db");
    common::create(
        &path,
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT COLLATE NOCASE,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a TEXT,b TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'Alpha')",
            "INSERT INTO child VALUES(1,'007','alpha'),(2,'7','ALPHA'),(3,NULL,NULL)",
        ],
    );
    let before = snapshot(dir.path());
    let mut c = Client::open(&path, Access::ReadOnly);
    let info = c.run(Operation::Inspect("child".into())).info.unwrap();
    let key = info.foreign_keys[0].clone();
    let values = vec![Value::Integer(7), Value::Text("Alpha".into())];
    let reply = c.run(Operation::References {
        table: "child".into(),
        key: key.clone(),
        values: values.clone(),
    });
    assert!(reply.failure.is_none(), "{:?}", reply.failure);
    assert!(
        reply.result.completion.as_ref().unwrap().error.is_none(),
        "{:?}",
        reply.result.completion
    );
    assert_eq!(reply.result.rows.len(), 2);
    assert_eq!(reply.table.unwrap().name, "child");
    let count = c.run(Operation::ReferenceCount {
        table: "child".into(),
        key: key.clone(),
        values: values.clone(),
    });
    assert_eq!(count.result.rows, vec![vec![Value::Integer(2)]]);
    let peek = c.run(Operation::Peek {
        table: "child".into(),
        key: key.clone(),
        values: vec![Value::Text("007".into()), Value::Text("alpha".into())],
    });
    assert_eq!(
        peek.result.rows,
        vec![vec![Value::Integer(7), Value::Text("Alpha".into())]]
    );
    let mut changed = key;
    changed.columns[0].0 = "id".into();
    assert!(
        c.run(Operation::References {
            table: "child".into(),
            key: changed,
            values
        })
        .failure
        .is_some()
    );
    c.close();
    assert_eq!(snapshot(dir.path()), before);
}
