use crate::common;
use std::{path::PathBuf, time::Duration};
use tursobench_core::*;
use tursobench_db::{Operation, Request, Session, validate_single};

fn fixture() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    common::create(
        &path,
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO items VALUES (1, 'hello'), (2, NULL)",
        ],
    );
    (dir, path)
}
fn run(s: &Session, n: u64, sql: &str, budget: Budget) -> ResultSet {
    common::run(
        s,
        n,
        Operation::Execute {
            sql: sql.into(),
            parameters: vec![],
            budget,
        },
    )
    .result
}

#[test]
fn discarding_a_later_view_does_not_suppress_an_earlier_request() {
    let (_dir, path) = fixture();
    let session = common::opened(&path, Access::ReadOnly);
    let first = session.ticket(1, 1);
    let second = session.ticket(2, 2);
    session.submit(Request { ticket: first, operation: Operation::Execute {
        sql: "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000) SELECT x FROM n".into(),
        parameters: vec![], budget: Budget { rows: 5000, ..Budget::default() },
    }}).unwrap();
    let mut earlier = ResultSet::default();
    earlier.start(first);
    earlier.apply(common::event(&session));
    session
        .submit(Request {
            ticket: second,
            operation: Operation::Execute {
                sql: "SELECT 42".into(),
                parameters: vec![],
                budget: Budget::default(),
            },
        })
        .unwrap();
    session.discard_results(second.request);
    let mut later = ResultSet::default();
    later.start(second);
    while earlier.completion.is_none() || later.completion.is_none() {
        let event = common::event(&session);
        if event.ticket == first {
            earlier.apply(event);
        } else {
            later.apply(event);
        }
    }
    assert_eq!(earlier.rows.len(), 5000);
    assert_eq!(earlier.completion.unwrap().delivery, Delivery::Complete);
    assert!(later.rows.is_empty());
    assert_eq!(later.completion.unwrap().delivery, Delivery::Discarded);
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
    for sql in [
        "-- empty".into(),
        "?".into(),
        "SELECT 1; ?".into(),
        " ".repeat(1024 * 1024 + 1),
    ] {
        assert!(validate_single(&sql).is_err(), "{sql:.40}");
    }
}
#[test]
fn real_readonly_bounded_results_and_cleanup() {
    let (_dir, path) = fixture();
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
    };
    let gate = Arc::new(Barrier::new(2));
    let armed = Arc::new(AtomicBool::new(false));
    let s = Session::open(path, Access::ReadOnly, true, {
        let gate = gate.clone();
        let armed = armed.clone();
        move || {
            if armed.swap(false, Ordering::AcqRel) {
                gate.wait();
            }
        }
    })
    .unwrap();
    assert!(matches!(common::event(&s).kind, EventKind::Opened));
    let r = run(&s, 1, "SELECT * FROM items ORDER BY id", Budget::default());
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[1][1], Value::Null);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Complete);
    let r = run(&s, 2, "DELETE FROM items", Budget::default());
    assert_eq!(
        r.completion.unwrap().error.unwrap().kind,
        ErrorKind::ReadOnly
    );
    for (index, budget) in [
        Budget {
            rows: 1,
            ..Budget::default()
        },
        Budget {
            rows: 0,
            ..Budget::default()
        },
        Budget {
            bytes: 0,
            ..Budget::default()
        },
        Budget {
            cell_bytes: 0,
            ..Budget::default()
        },
    ]
    .into_iter()
    .enumerate()
    {
        let r = run(&s, 10 + index as u64, "SELECT * FROM items", budget);
        assert_eq!(r.rows.len(), usize::from(index == 0));
        let completion = r.completion.unwrap();
        assert_eq!(completion.delivery, Delivery::Limited);
        assert_eq!(completion.rows_seen, 2);
    }
    let r = run(&s, 4, "SELECT zeroblob(100000)", Budget::default());
    assert!(r.rows.is_empty());
    assert_eq!(r.completion.unwrap().delivery, Delivery::Limited);
    let r = run(&s, 5, "SELECT * FROM items WHERE 0", Budget::default());
    assert_eq!(r.columns.len(), 2);
    assert!(r.rows.is_empty());
    for ticket in [
        Ticket {
            session: s.id + 1,
            ..s.ticket(20, 0)
        },
        Ticket {
            database: s.id + 1,
            ..s.ticket(20, 0)
        },
    ] {
        assert!(
            s.submit(Request {
                ticket,
                operation: Operation::Schema
            })
            .is_err()
        );
    }
    for (sql, parameters) in [
        (
            "SELECT ?1".into(),
            vec![Value::Blob(vec![0; 8 * 1024 * 1024])],
        ),
        (format!("SELECT {}", vec!["0"; 513].join(",")), vec![]),
    ] {
        let r = common::run(
            &s,
            21,
            Operation::Execute {
                sql,
                parameters,
                budget: Budget::default(),
            },
        )
        .result;
        assert_eq!(r.completion.unwrap().error.unwrap().kind, ErrorKind::Input);
    }
    // Hold the worker inside the wake callback after its terminal event is visible.
    // Reusing the ID must succeed even before that callback returns.
    armed.store(true, Ordering::Release);
    assert!(
        run(&s, 30, "?", Budget::default())
            .completion
            .unwrap()
            .error
            .is_some()
    );
    let reused = s.submit(Request {
        ticket: s.ticket(30, 0),
        operation: Operation::Schema,
    });
    gate.wait();
    reused.unwrap();
    while !matches!(common::event(&s).kind, EventKind::Finished(_)) {}
    s.close();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
}
#[test]
fn discard_unblocks_backpressure() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadOnly);
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
fn close_discards_queued_work_and_rejects_new_submissions() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadWrite);
    // 1,000 retained rows exceed the eight 64-row event slots, so execution
    // cannot finish while we receive only Columns. Keep draining cheap under coverage.
    s.submit(Request { ticket: s.ticket(1, 0), operation: Operation::Execute {
        sql: "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<1000) SELECT n FROM x".into(), parameters: vec![], budget: Budget::default()
    } }).unwrap();
    assert!(matches!(
        s.events.recv_timeout(Duration::from_secs(5)).unwrap().kind,
        EventKind::Columns(_)
    ));
    s.submit(Request {
        ticket: s.ticket(2, 0),
        operation: Operation::Execute {
            sql: "DELETE FROM items".into(),
            parameters: vec![],
            budget: Budget::default(),
        },
    })
    .unwrap();
    s.close();
    assert!(
        s.submit(Request {
            ticket: s.ticket(3, 0),
            operation: Operation::Schema
        })
        .is_err()
    );
    loop {
        if let EventKind::Closed { rollback } =
            s.events.recv_timeout(Duration::from_secs(10)).unwrap().kind
        {
            assert!(rollback.is_ok());
            break;
        }
    }
    let s = common::opened(&path, Access::ReadOnly);
    assert_eq!(
        run(&s, 1, "SELECT count(*) FROM items", Budget::default()).rows,
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn returning_is_drained_even_when_delivery_is_limited() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadWrite);
    let r = run(
        &s,
        1,
        "UPDATE items SET value='written' RETURNING id",
        Budget {
            rows: 1,
            ..Budget::default()
        },
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Limited);
    assert_eq!(
        run(
            &s,
            2,
            "SELECT count(*) FROM items WHERE value='written'",
            Budget::default()
        )
        .rows,
        vec![vec![Value::Integer(2)]]
    );
}
#[test]
fn invalid_open_inputs_never_create_or_rewrite_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typo.db");
    assert!(Session::open(path.clone(), Access::ReadOnly, false, || {}).is_err());
    assert!(
        Session::open_with_settings(
            path.clone(),
            Access::ReadOnly,
            true,
            ConnectionSettings {
                busy_timeout_ms: 5001
            },
            || {}
        )
        .is_err()
    );
    for contents in [None, Some(vec![]), Some(vec![0; 100])] {
        if let Some(bytes) = &contents {
            std::fs::write(&path, bytes).unwrap();
        }
        let s = Session::open(path.clone(), Access::ReadOnly, true, || {}).unwrap();
        assert!(matches!(common::event(&s).kind, EventKind::Failed(_)));
        assert_eq!(std::fs::read(&path).ok(), contents);
    }
    let s = Session::open(dir.path().into(), Access::ReadOnly, true, || {}).unwrap();
    assert!(matches!(common::event(&s).kind, EventKind::Failed(_)));
}
