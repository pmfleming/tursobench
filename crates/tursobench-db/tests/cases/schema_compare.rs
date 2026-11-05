use crate::common::{self, Client};
use std::sync::Arc;
use tursobench_core::{Access, SchemaSnapshot, TransactionState, Value};
use tursobench_db::{
    Operation,
    schema_compare::{Status, compare},
};

fn capture(client: &mut Client) -> Arc<SchemaSnapshot> {
    let reply = client.run(Operation::CaptureSchema);
    assert!(reply.failure.is_none(), "{:?}", reply.failure);
    Arc::new(reply.snapshot.unwrap())
}
#[test]
fn comparison_captures_readonly_schema_and_classifies_manual_changes() {
    let dir = tempfile::tempdir().unwrap();
    let desired = dir.path().join("desired.db");
    let target = dir.path().join("target.db");
    common::create(
        &desired,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT, extra TEXT)",
            "CREATE TABLE changed(id INTEGER PRIMARY KEY, value INTEGER NOT NULL)",
            "CREATE TABLE new_name(id INTEGER)",
            "CREATE INDEX idx ON t(value)",
            "CREATE VIEW v AS SELECT id FROM t",
            "CREATE TRIGGER tr AFTER INSERT ON t BEGIN UPDATE t SET value='x' WHERE id=NEW.id; END",
        ],
    );
    common::create(
        &target,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, value TEXT)",
            "CREATE TABLE changed(id INTEGER PRIMARY KEY, value TEXT)",
            "CREATE TABLE old_name(id INTEGER)",
        ],
    );
    let before = (
        common::snapshot(dir.path()),
        std::fs::read(&desired).unwrap(),
        std::fs::read(&target).unwrap(),
    );
    let mut a = Client::open(&desired, Access::ReadOnly);
    let mut b = Client::open(&target, Access::ReadOnly);
    let (a_schema, b_schema) = (capture(&mut a), capture(&mut b));
    let diff = compare(&a_schema, &b_schema);
    let table = diff.iter().find(|d| d.name == "t").unwrap();
    assert_eq!(table.status, Status::Modified);
    assert!(
        table
            .migration
            .as_ref()
            .unwrap()
            .contains("ADD COLUMN extra TEXT")
    );
    assert!(table.details[0].contains("extra"));
    let manual = diff.iter().find(|d| d.name == "changed").unwrap();
    assert!(manual.migration.is_none());
    assert!(manual.details[0].contains("NOT NULL"));
    assert!(manual.guidance.as_ref().unwrap().contains("data mapping"));
    for name in ["new_name", "idx", "v", "tr"] {
        let added = diff.iter().find(|d| d.name == name).unwrap();
        assert_eq!(added.status, Status::Added);
        assert!(added.migration.is_some(), "{name}");
    }
    let removed = diff.iter().find(|d| d.name == "old_name").unwrap();
    assert_eq!(removed.status, Status::Removed);
    assert!(removed.migration.is_none());
    assert!(
        removed
            .guidance
            .as_ref()
            .unwrap()
            .contains("Renames are never inferred")
    );
    assert!(
        compare(&a_schema, &a_schema)
            .iter()
            .all(|d| d.status == Status::Identical)
    );
    let denied = b.run(Operation::Migration {
        sql: "CREATE TABLE main.no_write(x)".into(),
        expected: b_schema,
    });
    assert!(denied.failure.unwrap().message.contains("read/write"));
    a.close();
    b.close();
    assert_eq!(before.0, common::snapshot(dir.path()));
    assert_eq!(before.1, std::fs::read(&desired).unwrap());
    assert_eq!(before.2, std::fs::read(&target).unwrap());
    // Execute the actual generated drafts in dependency order, refreshing the
    // expected target after each explicit per-object migration.
    let mut writable = Client::open(&target, Access::ReadWrite);
    for name in ["t", "new_name", "idx", "v", "tr"] {
        let sql = diff
            .iter()
            .find(|d| d.name == name)
            .unwrap()
            .migration
            .clone()
            .unwrap();
        let expected = capture(&mut writable);
        let applied = writable.run(Operation::Migration { sql, expected });
        assert!(applied.failure.is_none(), "{name}: {:?}", applied.failure);
        assert!(applied.result.completion.unwrap().error.is_none(), "{name}");
    }
    writable.sql("INSERT INTO t(id) VALUES(1)");
    assert_eq!(
        writable.sql("SELECT value FROM t").rows,
        vec![vec![Value::Text("x".into())]]
    );
    writable.close();
}
#[test]
fn migration_checks_schema_in_transaction_and_rolls_back_failed_batches() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("migration.db");
    common::create(
        &path,
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY)",
            "INSERT INTO t VALUES(1)",
        ],
    );
    let mut c = Client::open(&path, Access::ReadWrite);
    let original = capture(&mut c);
    let applied = c.run(Operation::Migration {
        sql: "ALTER TABLE main.t ADD COLUMN value TEXT; CREATE INDEX main.idx ON t(value);".into(),
        expected: original.clone(),
    });
    assert!(applied.failure.is_none(), "{:?}", applied.failure);
    assert!(applied.result.completion.unwrap().error.is_none());
    assert_eq!(
        c.sql("SELECT id,value FROM t").rows,
        vec![vec![Value::Integer(1), Value::Null]]
    );
    let stale = c.run(Operation::Migration {
        sql: "CREATE TABLE main.stale(x)".into(),
        expected: original,
    });
    assert!(stale.failure.unwrap().message.contains("schema changed"));
    assert_eq!(
        c.sql("SELECT count(*) FROM sqlite_schema WHERE name='stale'")
            .rows,
        vec![vec![Value::Integer(0)]]
    );
    let current = capture(&mut c);
    let failed = c.run(Operation::Migration {
        sql: "CREATE TABLE main.undone(x); CREATE TABLE main.t(x);".into(),
        expected: current.clone(),
    });
    assert!(failed.rolled_back);
    assert!(failed.result.completion.unwrap().error.is_some());
    assert_eq!(
        c.sql("SELECT count(*) FROM sqlite_schema WHERE name='undone'")
            .rows,
        vec![vec![Value::Integer(0)]]
    );
    assert_eq!(capture(&mut c), current);
    for sql in [
        "DROP TABLE t",
        "DELETE FROM t",
        "ALTER TABLE t RENAME TO renamed",
        "BEGIN",
        "CREATE TEMP TABLE bad(x)",
        "CREATE TABLE temp.bad(x)",
        "ALTER TABLE t ADD COLUMN unqualified TEXT",
    ] {
        let denied = c.run(Operation::Migration {
            sql: sql.into(),
            expected: current.clone(),
        });
        assert!(denied.failure.is_some(), "{sql}");
    }
    c.sql("BEGIN");
    let denied = c.run(Operation::Migration {
        sql: "CREATE TABLE main.nested(x)".into(),
        expected: current,
    });
    assert!(denied.failure.unwrap().message.contains("autocommit"));
    c.sql("ROLLBACK");
    assert_eq!(
        c.sql("SELECT count(*) FROM t")
            .completion
            .unwrap()
            .transaction,
        TransactionState::Autocommit
    );
    c.close();
}
