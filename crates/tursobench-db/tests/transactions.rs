use std::time::Duration;
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session};
fn result(s: &Session, n: u64, op: Operation) -> ResultSet {
    let t = s.ticket(n, 0);
    s.submit(Request {
        ticket: t,
        operation: op,
    })
    .unwrap();
    let mut result = ResultSet::default();
    result.start(t);
    loop {
        let e = s.events.recv_timeout(Duration::from_secs(5)).unwrap();
        let done = matches!(e.kind, EventKind::Finished(_));
        result.apply(e);
        if done {
            return result;
        }
    }
}
fn sql(s: &Session, n: u64, sql: &str) -> ResultSet {
    result(
        s,
        n,
        Operation::Execute {
            sql: sql.into(),
            parameters: vec![],
            budget: Budget::default(),
        },
    )
}
#[test]
fn persistent_editor_isolated_browse_foreign_keys_and_close_rollback() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("tx.db");
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            db.connect()
                .unwrap()
                .execute("CREATE TABLE parent(id INTEGER PRIMARY KEY)", ())
                .await
                .unwrap();
        });
    let s = Session::open(path.clone(), Access::ReadWrite, true, || {}).unwrap();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Opened
    ));
    assert_eq!(
        sql(&s, 1, "BEGIN").completion.unwrap().transaction,
        TransactionState::Active
    );
    sql(
        &s,
        2,
        "CREATE TABLE child(id INTEGER PRIMARY KEY, p INTEGER REFERENCES parent(id))",
    );
    assert_eq!(
        sql(&s, 3, "INSERT INTO child VALUES(1,99)")
            .completion
            .unwrap()
            .error
            .unwrap()
            .kind,
        ErrorKind::Constraint
    );
    sql(&s, 4, "INSERT INTO parent VALUES (1)");
    let r = result(
        &s,
        5,
        Operation::Browse(Browse {
            table: "parent".into(),
            offset: 0,
            sort: None,
            descending: false,
            filter: None,
        }),
    );
    assert!(r.rows.is_empty());
    assert_eq!(r.completion.unwrap().transaction, TransactionState::Active);
    assert_eq!(sql(&s, 6, "SELECT * FROM parent").rows.len(), 1);
    assert!(sql(&s, 7, "COMMIT").completion.unwrap().error.is_none());
    assert!(sql(&s, 8, "COMMIT").completion.unwrap().error.is_some());
    sql(&s, 9, "BEGIN");
    sql(&s, 10, "INSERT INTO parent VALUES (2)");
    s.close();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
    let s = Session::open(path, Access::ReadOnly, true, || {}).unwrap();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Opened
    ));
    assert_eq!(
        sql(&s, 1, "SELECT * FROM parent").rows,
        vec![vec![Value::Integer(1)]]
    );
}
