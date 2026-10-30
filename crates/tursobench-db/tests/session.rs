use std::{path::PathBuf, time::Duration};
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session, validate_single};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let c = db.connect().unwrap();
            c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)", ())
                .await
                .unwrap();
            c.execute("INSERT INTO items VALUES (1, 'hello'), (2, NULL)", ())
                .await
                .unwrap();
        });
    (dir, path)
}
fn opened(path: PathBuf) -> Session {
    let s = Session::open(path, Access::ReadOnly, true, || {}).unwrap();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Opened
    ));
    s
}
fn run(s: &Session, n: u64, sql: &str, budget: Budget) -> ResultSet {
    let ticket = s.ticket(n, 0);
    s.submit(Request {
        ticket,
        operation: Operation::Execute {
            sql: sql.into(),
            parameters: vec![],
            budget,
        },
    })
    .unwrap();
    let mut results = ResultSet::default();
    results.start(ticket);
    loop {
        let e = s.events.recv_timeout(Duration::from_secs(10)).unwrap();
        let done = matches!(e.kind, EventKind::Finished(_));
        results.apply(e);
        if done {
            break;
        }
    }
    results
}
#[test]
fn parser_not_semicolon_splitting() {
    assert!(validate_single("SELECT ';' /* ; */;").is_ok());
    assert!(
        validate_single(
            "CREATE TRIGGER t AFTER INSERT ON items BEGIN UPDATE items SET value=';'; END;"
        )
        .is_ok()
    );
    assert!(validate_single("SELECT 1; DELETE FROM items").is_err());
    assert!(validate_single("-- empty").is_err());
}
#[test]
fn real_readonly_bounded_results_and_cleanup() {
    let (_dir, path) = fixture();
    let s = opened(path);
    let r = run(&s, 1, "SELECT * FROM items ORDER BY id", Budget::default());
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[1][1], Value::Null);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Complete);
    let r = run(&s, 2, "DELETE FROM items", Budget::default());
    assert_eq!(
        r.completion.unwrap().error.unwrap().kind,
        ErrorKind::ReadOnly
    );
    let r = run(
        &s,
        3,
        "SELECT * FROM items",
        Budget {
            rows: 1,
            ..Budget::default()
        },
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Limited);
    let r = run(&s, 4, "SELECT zeroblob(100000)", Budget::default());
    assert!(r.rows.is_empty());
    assert_eq!(r.completion.unwrap().delivery, Delivery::Limited);
    let r = run(&s, 5, "SELECT * FROM items WHERE 0", Budget::default());
    assert_eq!(r.columns.len(), 2);
    assert!(r.rows.is_empty());
    s.close();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
}
#[test]
fn discard_unblocks_backpressure() {
    let (_dir, path) = fixture();
    let s = opened(path);
    s.submit(Request { ticket: s.ticket(1, 0), operation: Operation::Execute { sql: "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<10000) SELECT n FROM x".into(), parameters: vec![], budget: Budget { rows: 10000, ..Budget::default() } } }).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    s.discard_results(1);
    loop {
        if let EventKind::Finished(c) = s.events.recv_timeout(Duration::from_secs(10)).unwrap().kind
        {
            assert_eq!(c.delivery, Delivery::Discarded);
            assert_eq!(c.rows_seen, 10000);
            break;
        }
    }
}
#[test]
fn missing_path_never_created() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typo.db");
    let s = Session::open(path.clone(), Access::ReadOnly, true, || {}).unwrap();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Failed(_)
    ));
    assert!(!path.exists());
}
