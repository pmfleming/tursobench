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
fn seek_paging_grouped_filters_counts_and_reference_pages_are_complete() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("paging.db");
    common::create(
        &path,
        &[
            "CREATE TABLE p(id INTEGER PRIMARY KEY)",
            "INSERT INTO p VALUES(1)",
            "CREATE TABLE c(id INTEGER PRIMARY KEY, p INTEGER REFERENCES p(id), label TEXT COLLATE NOCASE)",
            "CREATE INDEX c_p ON c(p)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<251) INSERT INTO c SELECT x,1,CASE WHEN x%3=0 THEN NULL ELSE 'same' END FROM n",
        ],
    );
    let mut client = Client::open(&path, Access::ReadOnly);
    let key = client
        .run(Operation::Inspect("c".into()))
        .info
        .unwrap()
        .foreign_keys
        .remove(0);
    let legacy = client
        .run(Operation::References {
            table: "c".into(),
            key: key.clone(),
            values: vec![Value::Integer(1)],
        })
        .result;
    assert_eq!(legacy.rows.len(), 100);
    assert_eq!(legacy.completion.unwrap().delivery, Delivery::Limited);
    let mut spec = Browse {
        table: "c".into(),
        offset: 0,
        sort: None,
        descending: false,
        filters: vec![],
        options: Default::default(),
    };
    spec.options.relation = Some(Box::new((key, vec![Value::Integer(1)])));
    let first = client.run(Operation::Browse(spec.clone())).result;
    spec.options.cursor = first.rows.last().cloned();
    spec.offset = 100;
    let second = client.run(Operation::Browse(spec.clone())).result;
    assert_eq!(second.rows[0][0], Value::Integer(101));
    spec.options.cursor = second.rows.first().cloned();
    spec.options.reverse = true;
    spec.offset = 0;
    let mut previous = client.run(Operation::Browse(spec.clone())).result.rows;
    previous.reverse();
    assert_eq!(previous, first.rows);
    spec.options.cursor = None;
    spec.options.reverse = false;
    spec.options.predicate = Some(Box::new(FilterExpr::Any(vec![
        FilterExpr::In {
            column: 0,
            values: vec![Value::Integer(2), Value::Integer(3)],
        },
        FilterExpr::Compare(Filter {
            column: 0,
            op: FilterOp::Greater,
            value: Value::Integer(249),
        }),
    ])));
    assert_eq!(
        client
            .run(Operation::CountFiltered(spec.clone()))
            .result
            .rows,
        vec![vec![Value::Integer(4)]]
    );
    assert_eq!(client.run(Operation::Browse(spec)).result.rows.len(), 4);
    let mut sorted = Browse {
        table: "c".into(),
        offset: 0,
        sort: Some(2),
        descending: false,
        filters: vec![],
        options: Default::default(),
    };
    sorted.options.sorts.push((1, true));
    let expected = client
        .sql("SELECT id,p,label FROM c ORDER BY label ASC,p DESC,id ASC")
        .rows;
    let mut all = Vec::new();
    loop {
        let page = client.run(Operation::Browse(sorted.clone())).result.rows;
        if page.is_empty() {
            break;
        }
        sorted.options.cursor = page.last().cloned();
        all.extend(page);
    }
    assert_eq!(all, expected);
    client.close();
}

#[test]
fn large_values_keep_rows_and_save_exact_original_bytes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.db");
    common::create(
        &path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<151) INSERT INTO t SELECT x,CASE WHEN x=50 THEN hex(zeroblob(50000)) ELSE 'small' END FROM n",
        ],
    );
    let mut c = Client::open(&path, Access::ReadOnly);
    let first = c.browse("t", 0).1;
    assert_eq!(first.rows.len(), 100);
    assert_eq!(first.completion.unwrap().delivery, Delivery::Complete);
    let Value::Deferred(cell) = &first.rows[49][1] else {
        panic!("large cell must be deferred")
    };
    let handle = cell.handle.unwrap();
    assert_eq!(cell.bytes, 100000);
    let chunk = c
        .run(Operation::ReadValue {
            handle,
            offset: 99990,
            bytes: 1024,
        })
        .chunk
        .unwrap();
    assert_eq!(chunk, (99990, b"0000000000".to_vec()));
    let destination = dir.path().join("value.txt");
    assert!(
        c.run(Operation::SaveValue {
            handle,
            path: destination.clone()
        })
        .saved
        .is_some()
    );
    assert_eq!(std::fs::read(&destination).unwrap(), vec![b'0'; 100000]);
    assert!(
        c.run(Operation::SaveValue {
            handle,
            path: destination
        })
        .failure
        .is_some()
    );
    let second = c.browse("t", 100).1;
    assert_eq!(second.rows.len(), 51);
    assert_eq!(second.rows[0][0], Value::Integer(101));
    c.close();
}

#[test]
fn wide_rows_signal_page_continuation_instead_of_stalling() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("wide.db");
    common::create(
        &path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<151) INSERT INTO t SELECT x,hex(zeroblob(25000)) FROM n",
        ],
    );
    let mut c = Client::open(&path, Access::ReadOnly);
    let first = c.browse("t", 0).1;
    let n = first.rows.len();
    assert!(n > 0 && n < 100);
    assert_eq!(
        first.completion.as_ref().unwrap().delivery,
        Delivery::Complete
    );
    assert_eq!(first.completion.unwrap().page_has_more, Some(true));
    let second = c.browse("t", n).1;
    assert_eq!(second.rows[0][0], Value::Integer(n as i64 + 1));
    assert_eq!(n + second.rows.len(), 151);
    c.close();
}

#[test]
fn scripts_report_partial_success_and_atomic_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("scripts.db");
    common::create(&path, &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
    let mut c = Client::open(&path, Access::ReadWrite);
    for atomic in [false, true] {
        c.sql("DELETE FROM t");
        let reply = c.run(Operation::Script {
            sql: "INSERT INTO t VALUES(:id); INSERT INTO t VALUES(:id); INSERT INTO t VALUES(2)"
                .into(),
            parameters: Bindings::Named(BTreeMap::from([(":id".into(), Value::Integer(1))])),
            atomic,
            budget: Budget::default(),
        });
        assert_eq!(reply.script.len(), 2);
        assert!(reply.script[0].completion.error.is_none());
        assert!(reply.script[1].completion.error.is_some());
        assert_eq!(reply.rolled_back, atomic);
        assert_eq!(
            c.sql("SELECT count(*) FROM t").rows,
            vec![vec![Value::Integer(if atomic { 0 } else { 1 })]]
        );
    }
    let reply = c.run(Operation::Script {
        sql: "INSERT INTO t VALUES(7); SELECT id FROM t".into(),
        parameters: vec![].into(),
        atomic: true,
        budget: Budget::default(),
    });
    assert!(reply.result.completion.unwrap().error.is_none());
    assert_eq!(reply.script[1].rows, vec![vec![Value::Integer(7)]]);
    let rejected = c.run(Operation::Script {
        sql: "BEGIN; INSERT INTO t VALUES(8)".into(),
        parameters: vec![].into(),
        atomic: true,
        budget: Budget::default(),
    });
    assert!(rejected.failure.is_some());
    c.close();
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
            options: Default::default(),
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

#[test]
fn suggested_index_executes_and_is_not_suggested_twice() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("advice.db");
    common::create(
        &path,
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)",
            "INSERT INTO items VALUES(1,'a'),(2,'b')",
        ],
    );
    let mut client = Client::open(&path, Access::ReadWrite);
    let info = client.run(Operation::Inspect("items".into())).info.unwrap();
    let sql = "SELECT * FROM items WHERE label = ?1";
    let draft = tursobench_db::advice::index_draft(sql, &info).unwrap();
    client.sql(&draft);
    let info = client.run(Operation::Inspect("items".into())).info.unwrap();
    assert!(tursobench_db::advice::index_draft(sql, &info).is_none());
    let plan = client.run(Operation::Explain {
        sql: sql.into(),
        parameters: vec![Value::Text("a".into())].into(),
        bytecode: false,
    });
    assert!(plan.result.completion.unwrap().error.is_none());
    assert!(
        plan.result
            .rows
            .iter()
            .flatten()
            .any(|v| matches!(v,Value::Text(s) if s.contains("idx_items_label")))
    );
    client.close();
}

#[test]
fn profiles_exact_sampled_and_keyless_values_without_writing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("profile.db");
    common::create(
        &path,
        &[
            "CREATE TABLE items(value)",
            "INSERT INTO items VALUES(1),(3),(NULL),('abc')",
            "CREATE TABLE empty(value TEXT)",
        ],
    );
    let before = snapshot(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    for (table, sample, rows, nulls) in [
        ("items", None, 4, 1),
        ("items", Some(2), 2, 0),
        ("empty", None, 0, 0),
    ] {
        let reply = client.run(Operation::Profile(tursobench_db::profile::Request {
            table: table.into(),
            column: None,
            sample_rows: sample,
        }));
        assert!(reply.failure.is_none(), "{:?}", reply.failure);
        assert!(
            reply.result.completion.as_ref().unwrap().error.is_none(),
            "{:?}",
            reply.result.completion
        );
        for (metric, expected) in [("rows", rows), ("nulls", nulls)] {
            let row = reply
                .result
                .rows
                .iter()
                .find(|r| r[1] == Value::Text(metric.into()))
                .unwrap();
            assert_eq!(row[2], Value::Integer(expected));
        }
    }
    assert!(
        client
            .run(Operation::Profile(tursobench_db::profile::Request {
                table: "items".into(),
                column: Some("missing".into()),
                sample_rows: None
            }))
            .failure
            .is_some()
    );
    client.close();
    assert_eq!(before, snapshot(dir.path()));
}

#[test]
fn admin_settings_readback_is_scoped_and_readonly_changes_are_rejected() {
    use tursobench_db::admin::Change;
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("admin.db");
    common::create(&path, &["CREATE TABLE t(id INTEGER)"]);
    let before = snapshot(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    let dashboard = client.run(Operation::Admin(None));
    assert!(dashboard.failure.is_none());
    assert!(
        dashboard
            .result
            .rows
            .iter()
            .any(|r| r[0] == Value::Text("Main file bytes".into()))
    );
    assert!(
        client
            .run(Operation::Admin(Some(Change::EditorBusyTimeout(123))))
            .failure
            .is_some()
    );
    client.close();
    assert_eq!(before, snapshot(dir.path()));
    let mut client = Client::open(&path, Access::ReadWrite);
    let dashboard = client.run(Operation::Admin(Some(Change::EditorBusyTimeout(123))));
    assert!(dashboard.failure.is_none(), "{:?}", dashboard.failure);
    let timeouts: Vec<_> = dashboard
        .result
        .rows
        .iter()
        .filter(|r| r[0] == Value::Text("busy_timeout".into()))
        .collect();
    assert_eq!(timeouts[0][2], Value::Integer(123));
    assert_ne!(timeouts[1][2], Value::Integer(123));
    assert!(
        client
            .run(Operation::Admin(Some(Change::EditorCacheKiB(1024))))
            .failure
            .is_none()
    );
    assert!(
        client
            .run(Operation::Admin(Some(Change::EditorCacheKiB(0))))
            .failure
            .is_some()
    );
    client.sql("BEGIN");
    assert!(
        client
            .run(Operation::Admin(Some(Change::EditorBusyTimeout(99))))
            .failure
            .is_some()
    );
    client.sql("ROLLBACK");
    client.close();
}
