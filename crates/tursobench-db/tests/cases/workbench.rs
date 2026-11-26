use crate::common::{self, Client};
use std::collections::BTreeMap;
use tursobench_core::{
    Access, Bindings, Browse, Budget, Delivery, ErrorKind, Filter, FilterExpr, FilterOp, Value,
};
use tursobench_db::{Diagnostic, Operation};
fn fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    common::database(
        name,
        &[
            "CREATE TABLE parent(a INTEGER, b TEXT, payload TEXT, PRIMARY KEY(a,b)) STRICT",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a INTEGER,b TEXT,note TEXT,FOREIGN KEY(a,b) REFERENCES parent ON DELETE CASCADE) STRICT",
            "CREATE INDEX child_note_idx ON child(note) WHERE note IS NOT NULL",
            "CREATE INDEX child_fts ON child USING fts(note)",
            "INSERT INTO parent VALUES(7,'001','original')",
            "INSERT INTO child VALUES(1,7,'001','needle'),(2,NULL,NULL,NULL)",
        ],
    )
}

/// `t(id, value)` holding rows 1..=151, with `value` computed from row number `x`.
fn numbered(name: &str, value: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let fill = format!(
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<151) INSERT INTO t SELECT x,{value} FROM n"
    );
    common::database(
        name,
        &["CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)", &fill],
    )
}

#[test]
fn catalog_preserves_large_definitions_and_more_than_one_thousand_indexes() {
    let (_dir, path) = common::database("catalog.db", &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
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
fn seek_paging_grouped_filters_counts_and_reference_pages_are_complete() {
    let (_dir, path) = common::database(
        "paging.db",
        &[
            "CREATE TABLE p(id INTEGER PRIMARY KEY)",
            "INSERT INTO p VALUES(1)",
            "CREATE TABLE c(id INTEGER PRIMARY KEY, p INTEGER REFERENCES p(id), label TEXT COLLATE NOCASE)",
            "CREATE INDEX c_p ON c(p)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<251) INSERT INTO c SELECT x,1,CASE WHEN x%3=0 THEN NULL ELSE 'same' END FROM n",
        ],
    );
    let mut client = Client::open(&path, Access::ReadOnly);
    let key = client.inspect("c").foreign_keys.remove(0);
    let legacy = client
        .run(Operation::References {
            table: "c".into(),
            key: key.clone(),
            values: vec![Value::Integer(1)],
        })
        .result;
    assert_eq!(legacy.rows.len(), 100);
    assert_eq!(legacy.completion.unwrap().delivery, Delivery::Limited);
    let mut spec = common::browse("c");
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
        sort: Some(2),
        ..common::browse("c")
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
    let (dir, path) = numbered(
        "large.db",
        "CASE WHEN x=50 THEN hex(zeroblob(50000)) ELSE 'small' END",
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
    let loaded = c.run(Operation::LoadValue { handle }).loaded.unwrap();
    assert_eq!(loaded, vec![b'0'; 100000]);
    let find = |c: &mut Client, needle: &[u8], from| {
        let operation = Operation::FindValue {
            handle,
            needle: needle.to_vec(),
            from,
        };
        c.run(operation).found.unwrap()
    };
    assert_eq!(find(&mut c, b"00", 99_000), Some(99_000));
    assert_eq!(find(&mut c, b"1", 0), None);
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
    let path = destination;
    assert!(c.refuses(Operation::SaveValue { handle, path }));
    let second = c.browse("t", 100).1;
    assert_eq!(second.rows.len(), 51);
    assert_eq!(second.rows[0][0], Value::Integer(101));
    c.close();
}

#[test]
fn deferred_blobs_keep_their_first_bytes_beside_the_handle() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("blobs.db");
    common::block_on(common::blob_fixtures::blobs(&path)).unwrap();
    let mut c = Client::open(&path, Access::ReadOnly);
    let rows = c.browse("blobs", 0).1.rows;
    let samples = common::blob_fixtures::blob_samples();
    assert_eq!(rows.len(), samples.len());
    for (row, (kind, data)) in rows.iter().zip(&samples) {
        assert_eq!(row[1], Value::Text((*kind).into()));
        match &row[2] {
            Value::Blob(bytes) => assert!(bytes == data && data.len() <= 64 * 1024, "{kind}"),
            Value::Deferred(cell) => {
                assert_eq!(cell.head, data[..64], "{kind}");
                assert_eq!(cell.bytes, data.len() as u64);
                let read = Operation::ReadValue {
                    handle: cell.handle.unwrap(),
                    offset: 0,
                    bytes: 64,
                };
                assert_eq!(c.run(read).chunk.unwrap(), (0, cell.head.clone()));
            }
            other => panic!("{kind}: {other:?}"),
        }
    }
    c.close();
}

#[test]
fn wide_rows_signal_page_continuation_instead_of_stalling() {
    let (_dir, path) = numbered("wide.db", "hex(zeroblob(25000))");
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
    let (_dir, path) = common::database("scripts.db", &["CREATE TABLE t(id INTEGER PRIMARY KEY)"]);
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
        assert_eq!(c.count("t"), i64::from(!atomic));
    }
    let reply = c.run(common::script(
        "INSERT INTO t VALUES(7); SELECT id FROM t",
        true,
    ));
    assert!(reply.result.completion.unwrap().error.is_none());
    assert_eq!(reply.script[1].rows, vec![vec![Value::Integer(7)]]);
    assert!(c.refuses(common::script("BEGIN; INSERT INTO t VALUES(8)", true)));
    c.close();
}

#[test]
fn native_metadata_related_lookup_and_diagnostics_preserve_readonly_bytes() {
    let (dir, path) = fixture("workbench.db");
    let files = common::Files::record(dir.path());
    let mut client = Client::open(&path, Access::ReadOnly);
    // Neither may create a file beside a read-only database.
    let beside = |file: &str| dir.path().join(file).display().to_string();
    for sql in [
        format!("VACUUM INTO '{}'", beside("copy.db")),
        format!("ATTACH '{}' AS other", beside("attached.db")),
    ] {
        assert_eq!(client.failure(&sql).unwrap().kind, ErrorKind::ReadOnly);
    }
    let info = client.inspect("child");
    assert_eq!(info.table.columns.len(), 4);
    assert!(info.indexes.iter().any(|i| i.name == "child_note_idx"
        && i.partial
        && i.columns == vec![Some("note".into())]));
    assert!(info.indexes.iter().any(|i| i.name == "child_fts"));
    assert_eq!(info.foreign_keys.len(), 1);
    assert_eq!(info.foreign_keys[0].on_delete, "CASCADE");
    assert!(client.refuses(Operation::Inspect("x".repeat(64 * 1024 + 1))));
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
    assert!(client.refuses(Operation::Lookup {
        table: "parent".into(),
        keys: vec![("a\" OR 1=1 --".into(), Value::Integer(7))]
    }));
    assert_eq!(
        client.run(Operation::Count("child".into())).result.rows,
        vec![vec![Value::Integer(2)]]
    );
    for diagnostic in Diagnostic::ALL {
        let reply = client.run(Operation::Diagnostic(diagnostic));
        reply.assert_succeeded();
        let result = reply.result;
        if diagnostic == Diagnostic::Integrity {
            assert_eq!(result.rows, vec![vec![Value::Text("ok".into())]]);
        }
        if diagnostic == Diagnostic::ForeignKeys {
            assert!(result.rows.iter().all(|r| r[3] == Value::Integer(0)));
        }
    }
    client.close();
    assert!(files.unchanged());
}
#[test]
fn diagram_reads_committed_isolated_quoted_cyclic_and_composite_tables_within_budget() {
    let (dir, path) = fixture("diagram.db");
    let mut client = Client::open(&path, Access::ReadWrite);
    client.sql("BEGIN");
    client.sql("CREATE TABLE pending_schema(id INTEGER PRIMARY KEY)");
    client.sql("ALTER TABLE child ADD COLUMN pending_column TEXT");
    let graph = client.diagram();
    assert!(!graph.tables.iter().any(|t| t.name == "pending_schema"));
    let child = graph.tables.iter().find(|t| t.name == "child").unwrap();
    assert_eq!(child.inspection.as_ref().unwrap().table.columns.len(), 4);
    client.sql("ROLLBACK");
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
    let files = common::Files::record(dir.path());
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
    assert!(files.unchanged());
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
    let (_dir, path) = fixture("workbench.db");
    let mut client = Client::open(&path, Access::ReadWrite);
    let parameters = Bindings::Named(BTreeMap::from([
        (
            ":payload".into(),
            Value::Text("'; DELETE FROM parent; --".into()),
        ),
        ("$id".into(), Value::Integer(7)),
    ]));
    let reply = client.run(Operation::ExecuteBound {
        sql: "SELECT :payload,:payload,$id".into(),
        parameters,
        budget: Budget::default(),
    });
    reply.assert_succeeded();
    assert_eq!(
        reply.result.rows[0],
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
        reply.assert_succeeded();
        assert!(!reply.result.rows.is_empty());
        assert_eq!(reply.result.completion.unwrap().changed, 0);
    }
    assert_eq!(client.count("child"), 2);
    for sql in [
        "PRAGMA foreign_keys=OFF",
        "BEGIN",
        "CREATE TABLE unwanted(x)",
        "DELETE FROM child; DELETE FROM parent",
    ] {
        assert!(client.refuses(Operation::Explain {
            sql: sql.into(),
            parameters: vec![].into(),
            bytecode: true
        }));
    }
    let missing = client.run(Operation::ExecuteBound {
        sql: "DELETE FROM child WHERE id=:id".into(),
        parameters: Bindings::Named(BTreeMap::new()),
        budget: Budget::default(),
    });
    assert!(missing.result.completion.unwrap().error.is_some());
    assert_eq!(client.count("child"), 2);
    client.close();
}

#[test]
fn typed_filter_chips_bind_values_escape_contains_and_combine_null_predicates() {
    let (dir, path) = common::database(
        "filters.db",
        &[
            r#"CREATE TABLE "filter rows"(id INTEGER PRIMARY KEY, "odd""col" TEXT, n INTEGER, bytes BLOB)"#,
            r#"INSERT INTO "filter rows" VALUES(1,'100%_\literal',1,X'00FF'),(2,'x'' OR 1=1 --',3,X'01'),(3,NULL,9223372036854775807,NULL)"#,
        ],
    );
    let files = common::Files::record(dir.path());
    let mut c = Client::open(&path, Access::ReadOnly);
    let mut run = |filters: Vec<Filter>| {
        c.run(Operation::Browse(Browse {
            filters,
            ..common::browse("filter rows")
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
        reply.assert_succeeded();
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
    assert!(files.unchanged());
}

#[test]
fn foreign_keys_use_parent_affinity_and_collation_in_checks_and_references() {
    let (dir, path) = common::database(
        "reverse.db",
        &[
            "CREATE TABLE parent(a INTEGER,b TEXT COLLATE NOCASE,PRIMARY KEY(a,b))",
            "CREATE TABLE child(id INTEGER PRIMARY KEY,a TEXT,b TEXT,FOREIGN KEY(a,b) REFERENCES parent)",
            "INSERT INTO parent VALUES(7,'Alpha')",
            "INSERT INTO child VALUES(1,'007','alpha'),(2,'7','ALPHA'),(3,NULL,NULL),(4,'8','orphan'),(5,NULL,'exempt')",
        ],
    );
    let files = common::Files::record(dir.path());
    let mut c = Client::open(&path, Access::ReadOnly);
    // Only the orphan violates; NULL keys are exempt and affinity/collation match.
    let check = c.run(Operation::Diagnostic(Diagnostic::ForeignKeys));
    check.assert_succeeded();
    let violations = Value::Integer(1);
    let checked_tables = Value::Integer(2);
    assert_eq!(check.result.rows.len(), 1);
    assert_eq!(check.result.rows[0][3..5], [violations, checked_tables]);
    let info = c.inspect("child");
    let key = info.foreign_keys[0].clone();
    let values = vec![Value::Integer(7), Value::Text("Alpha".into())];
    let reply = c.run(Operation::References {
        table: "child".into(),
        key: key.clone(),
        values: values.clone(),
    });
    reply.assert_succeeded();
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
    assert!(files.unchanged());
}

#[test]
fn suggested_index_executes_and_is_not_suggested_twice() {
    let (_dir, path) = common::database(
        "advice.db",
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, label TEXT)",
            "INSERT INTO items VALUES(1,'a'),(2,'b')",
        ],
    );
    let mut client = Client::open(&path, Access::ReadWrite);
    let info = client.inspect("items");
    let sql = "SELECT * FROM items WHERE label = ?1";
    let draft = tursobench_db::advice::index_draft(sql, &info).unwrap();
    client.sql(&draft);
    let info = client.inspect("items");
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
    let (dir, path) = common::database(
        "profile.db",
        &[
            "CREATE TABLE items(value)",
            "INSERT INTO items VALUES(1),(3),(NULL),('abc')",
            "CREATE TABLE empty(value TEXT)",
        ],
    );
    let files = common::Files::record(dir.path());
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
        reply.assert_succeeded();
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
        client.refuses(Operation::Profile(tursobench_db::profile::Request {
            table: "items".into(),
            column: Some("missing".into()),
            sample_rows: None
        }))
    );
    client.close();
    assert!(files.unchanged());
}

#[test]
fn admin_settings_readback_is_scoped_and_readonly_changes_are_rejected() {
    use tursobench_db::admin::Change;
    let (dir, path) = common::database("admin.db", &["CREATE TABLE t(id INTEGER)"]);
    let files = common::Files::record(dir.path());
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
    assert!(client.refuses(Operation::Admin(Some(Change::EditorBusyTimeout(123)))));
    client.close();
    assert!(files.unchanged());
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
    assert!(!client.refuses(Operation::Admin(Some(Change::EditorCacheKiB(1024)))));
    assert!(client.refuses(Operation::Admin(Some(Change::EditorCacheKiB(0)))));
    client.sql("BEGIN");
    assert!(client.refuses(Operation::Admin(Some(Change::EditorBusyTimeout(99)))));
    client.sql("ROLLBACK");
    client.close();
}
