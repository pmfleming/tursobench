use crate::common::{self, Client};
use tursobench_core::*;
use tursobench_db::Operation;
#[test]
fn persistent_editor_isolated_browse_foreign_keys_and_close_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tx.db");
    common::create(&path, &["CREATE TABLE parent(id INTEGER PRIMARY KEY)"]);
    let mut client = Client::open(&path, Access::ReadWrite);
    assert_eq!(
        client.sql("BEGIN").completion.unwrap().transaction,
        TransactionState::Active
    );
    client.sql("CREATE TABLE child(id INTEGER PRIMARY KEY, p INTEGER REFERENCES parent(id))");
    assert_eq!(
        client
            .query("INSERT INTO child VALUES(1,99)", vec![])
            .completion
            .unwrap()
            .error
            .unwrap()
            .kind,
        ErrorKind::Constraint
    );
    client.sql("INSERT INTO parent VALUES (1)");
    let r = client.run(Operation::Browse(Browse {
        table: "parent".into(),
        offset: 0,
        sort: None,
        descending: false,
        filters: vec![],
    }));
    let r = r.result;
    assert!(r.rows.is_empty());
    assert_eq!(r.completion.unwrap().transaction, TransactionState::Active);
    assert_eq!(client.sql("SELECT * FROM parent").rows.len(), 1);
    client.sql("COMMIT");
    assert!(
        client
            .query("COMMIT", vec![])
            .completion
            .unwrap()
            .error
            .is_some()
    );
    client.sql("BEGIN");
    client.sql("INSERT INTO parent VALUES (2)");
    client.close();
    let mut client = Client::open(&path, Access::ReadOnly);
    assert_eq!(
        client.sql("SELECT * FROM parent").rows,
        vec![vec![Value::Integer(1)]]
    );
}
