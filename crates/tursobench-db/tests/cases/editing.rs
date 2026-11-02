use crate::common::{self, Client};
use tursobench_core::*;
use tursobench_db::Operation;
fn fixture() -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("edits.db");
    common::create(
        &path,
        &[
            "CREATE TABLE items(a INTEGER NOT NULL, b TEXT NOT NULL, data BLOB, label TEXT DEFAULT 'default', PRIMARY KEY(a,b))",
            "INSERT INTO items VALUES (1,'key',X'00FF',NULL), (2,'other',NULL,'')",
        ],
    );
    let client = Client::open(&path, Access::ReadWrite);
    (dir, client)
}
#[test]
fn composite_changed_keys_null_blob_and_defaults() {
    let (_dir, mut h) = fixture();
    h.sql(
        "CREATE TABLE nullable_keys(a TEXT, b INTEGER, data TEXT COLLATE NOCASE, PRIMARY KEY(a,b))",
    );
    h.sql("INSERT INTO nullable_keys VALUES('x',NULL,'a'),('x',NULL,'A')");
    let nullable = h.run(Operation::Browse(Browse {
        table: "nullable_keys".into(),
        offset: 0,
        sort: None,
        descending: false,
        filters: vec![],
    }));
    let ordered = nullable.result.rows;
    assert_eq!(ordered[0][2], Value::Text("A".into()));
    assert_eq!(ordered[1][2], Value::Text("a".into()));
    let (table, page) = h.browse("items", 0);
    let rows = page.rows;
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
    let (_, page) = h.browse("items", 0);
    let rows = page.rows;
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
        h.sql("SELECT label FROM items WHERE a=4").rows,
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
        h.sql("SELECT count(*) FROM items").rows,
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn conflict_and_mid_batch_failure_are_atomic() {
    let (_dir, mut h) = fixture();
    let (table, page) = h.browse("items", 0);
    let rows = page.rows;
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
        h.sql("SELECT label FROM items WHERE a=2").rows,
        vec![vec![Value::Text(String::new())]]
    );
    let (_, page) = h.browse("items", 0);
    let rows = page.rows;
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
        h.sql("SELECT count(*) FROM items").rows,
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn generated_edits_enforce_foreign_keys_even_if_editor_disables_them() {
    let (_dir, mut h) = fixture();
    h.sql("CREATE TABLE child(a INTEGER, b TEXT, FOREIGN KEY(a,b) REFERENCES items(a,b))");
    h.sql("INSERT INTO child VALUES(1, 'key')");
    h.sql("PRAGMA foreign_keys=OFF");
    let (table, page) = h.browse("items", 0);
    let rows = page.rows;
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
        h.sql("SELECT count(*) FROM items").rows,
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn editor_transaction_is_not_joined_and_schema_changes_rejected() {
    let (_dir, mut h) = fixture();
    let (table, page) = h.browse("items", 0);
    let rows = page.rows;
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
