use crate::common::{self, execute, submit};
use std::{path::PathBuf, time::Duration};
use tursobench_core::{
    Access, Browse, BrowseOptions, Budget, ConnectionSettings, Delivery, ErrorKind, EventKind,
    ResultSet, Ticket, Value,
};
use tursobench_db::{Operation, Request, Session, validate_single};

const BILLION_SUM: &str = "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100000000) SELECT sum(x) FROM n";
fn fixture() -> (tempfile::TempDir, PathBuf) {
    common::database(
        "test.db",
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO items VALUES (1, 'hello'), (2, NULL)",
        ],
    )
}
fn run(s: &Session, n: u64, sql: &str, budget: Budget) -> ResultSet {
    common::run(s, n, execute(sql, budget)).result
}
/// `sql` under the default budget.
fn query(s: &Session, n: u64, sql: &str) -> ResultSet {
    run(s, n, sql, Budget::default())
}
fn count_items(s: &Session, n: u64, filter: &str) -> Vec<Vec<Value>> {
    let sql = format!("SELECT count(*) FROM items{filter}");
    query(s, n, &sql).rows
}
/// The next terminal completion, skipping streamed rows and metadata.
fn finished(s: &Session) -> tursobench_core::Completion {
    loop {
        if let EventKind::Finished(c) = common::event(s).kind {
            return c;
        }
    }
}

#[test]
fn deadlines_interrupt_cpu_queries_and_writes_and_connections_recover() {
    let (_dir, path) = fixture();
    let s = Session::open_with_settings(
        path,
        Access::ReadWrite,
        ConnectionSettings {
            query_timeout_ms: 10,
            ..ConnectionSettings::default()
        },
        || {},
    )
    .unwrap();
    assert!(matches!(common::event(&s).kind, EventKind::Opened(_)));
    let started = std::time::Instant::now();
    let result = query(&s, 1, BILLION_SUM);
    assert_eq!(
        result.completion.unwrap().error.unwrap().kind,
        ErrorKind::Interrupted
    );
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        query(&s, 2, "SELECT 42").rows,
        vec![vec![Value::Integer(42)]]
    );
    query(&s, 3, "BEGIN");
    query(&s, 4, "INSERT INTO items VALUES(3,'pending')");
    let result = query(&s, 5, BILLION_SUM);
    assert!(result.completion.unwrap().error.is_some());
    query(&s, 6, "ROLLBACK");
    assert_eq!(count_items(&s, 7, ""), vec![vec![Value::Integer(2)]]);
    let result = query(
        &s,
        8,
        "WITH RECURSIVE n(x) AS (VALUES(10) UNION ALL SELECT x+1 FROM n WHERE x<100000000) INSERT INTO items SELECT x,'large' FROM n",
    );
    assert_eq!(
        result.completion.unwrap().error.unwrap().kind,
        ErrorKind::Interrupted
    );
    assert_eq!(count_items(&s, 9, ""), vec![vec![Value::Integer(2)]]);
    common::close(&s);
}

#[test]
fn discard_unblocks_backpressure_and_interrupts_reads_before_their_first_row() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadOnly);
    let rows = Budget {
        rows: 10000,
        ..Budget::default()
    };
    submit(
        &s,
        1,
        execute(
            "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<10000) SELECT n FROM x",
            rows,
        ),
    );
    std::thread::sleep(Duration::from_millis(100));
    s.discard_results(1);
    let blocked = finished(&s);
    assert_eq!(blocked.delivery, Delivery::Discarded);
    assert!(blocked.rows_seen < 10000);

    submit(&s, 2, execute(BILLION_SUM, Budget::default()));
    assert!(matches!(common::event(&s).kind, EventKind::Columns(_)));
    std::thread::sleep(Duration::from_millis(20));
    let started = std::time::Instant::now();
    for id in 3..=17 {
        submit(&s, id, execute("SELECT 42", Budget::default()));
    }
    s.discard_results(2);
    assert_eq!(finished(&s).delivery, Delivery::Discarded);
    assert!(started.elapsed() < Duration::from_secs(3));
    for _ in 3..=17 {
        assert!(finished(&s).error.is_none());
    }
    common::close(&s);
}

#[test]
fn discarding_a_later_view_does_not_suppress_an_earlier_request() {
    let (_dir, path) = fixture();
    let session = common::opened(&path, Access::ReadOnly);
    let first = session.ticket(1, 1);
    let second = session.ticket(2, 2);
    let rows = Budget {
        rows: 5000,
        ..Budget::default()
    };
    session.submit(Request { ticket: first, operation: execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000) SELECT x FROM n", rows) }).unwrap();
    let mut earlier = ResultSet::default();
    earlier.start(first);
    earlier.apply(common::event(&session));
    let operation = execute("SELECT 42", Budget::default());
    session
        .submit(Request {
            ticket: second,
            operation,
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
fn mid_result_engine_error_keeps_the_unsent_successful_rows() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadOnly);
    let result = query(
        &s,
        1,
        "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<100) SELECT x,json_extract(CASE WHEN x=70 THEN 'bad' ELSE '{}' END,'$') FROM n",
    );
    assert!(result.completion.unwrap().error.is_some());
    assert_eq!(result.rows.len(), 69);
    assert_eq!(result.rows[68][0], Value::Integer(69));
    common::close(&s);
}
/// Each limit alone refuses the second row (or the first, without paging).
fn assert_row_limits(s: &Session) {
    let limits = [
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
    ];
    for (index, budget) in limits.into_iter().enumerate() {
        let r = run(s, 10 + index as u64, "SELECT * FROM items", budget);
        assert_eq!(r.rows.len(), usize::from(index == 0));
        let completion = r.completion.unwrap();
        assert_eq!(completion.delivery, Delivery::Limited);
        assert!((1..=2).contains(&completion.rows_seen));
    }
}
/// Paged byte budgets report continuation instead of truncation once a row is kept.
fn assert_page_budgets(s: &Session) {
    let unit = Value::Integer(0).bytes();
    for (index, (bytes, page, kept, delivery, more)) in [
        (2 * unit, false, 2, Delivery::Complete, None),
        (2 * unit, true, 2, Delivery::Complete, Some(true)),
        (unit, true, 1, Delivery::Complete, Some(true)),
        (unit - 1, true, 0, Delivery::Limited, Some(false)),
    ]
    .into_iter()
    .enumerate()
    {
        let budget = Budget {
            rows: 2,
            bytes,
            page,
            ..Budget::default()
        };
        let r = run(
            s,
            30 + index as u64,
            "SELECT id FROM items ORDER BY id",
            budget,
        );
        let expected: Vec<_> = (1..=kept).map(|n| vec![Value::Integer(n)]).collect();
        assert_eq!(r.rows, expected);
        let completion = r.completion.unwrap();
        assert_eq!(
            (completion.delivery, completion.page_has_more),
            (delivery, more)
        );
    }
    let options = BrowseOptions {
        tail: true,
        page_rows: 2,
        page_bytes: 3 * unit,
        ..Default::default()
    };
    let tail = common::run(
        s,
        40,
        Operation::Browse(Browse {
            options,
            ..common::browse("items")
        }),
    )
    .result;
    assert_eq!(tail.rows, vec![vec![Value::Integer(2), Value::Null]]);
    assert_eq!(tail.completion.unwrap().page_has_more, Some(true));
}
#[test]
fn real_readonly_bounded_results_and_cleanup() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadOnly);
    let r = query(&s, 1, "SELECT * FROM items ORDER BY id");
    assert_eq!(r.rows.len(), 2);
    assert_eq!(r.rows[1][1], Value::Null);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Complete);
    let r = query(&s, 2, "DELETE FROM items");
    assert_eq!(
        r.completion.unwrap().error.unwrap().kind,
        ErrorKind::ReadOnly
    );
    assert_row_limits(&s);
    let r = query(&s, 4, "SELECT zeroblob(100000)");
    assert!(
        matches!(&r.rows[0][0],Value::Deferred(cell) if cell.bytes==100000 && cell.handle.is_some())
    );
    assert_eq!(r.completion.unwrap().delivery, Delivery::Complete);
    let r = query(&s, 5, "SELECT * FROM items WHERE 0");
    assert_eq!(r.columns.len(), 2);
    assert!(r.rows.is_empty());
    assert_page_budgets(&s);
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
        let operation = Operation::Schema;
        assert!(s.submit(Request { ticket, operation }).is_err());
    }
    let oversized = [
        (
            "SELECT ?1".into(),
            vec![Value::Blob(vec![0; 8 * 1024 * 1024])],
        ),
        (format!("SELECT {}", vec!["0"; 513].join(",")), vec![]),
    ];
    for (sql, parameters) in oversized {
        let operation = Operation::Execute {
            sql,
            parameters,
            budget: Budget::default(),
        };
        let r = common::run(&s, 21, operation).result;
        assert_eq!(r.completion.unwrap().error.unwrap().kind, ErrorKind::Input);
    }
    common::close(&s);
}
#[test]
fn request_ids_are_reusable_once_their_terminal_event_is_visible() {
    let (_dir, path) = fixture();
    // Acknowledge the open wake before arming. Receiving an event alone does
    // not mean its wake callback has returned.
    use std::sync::{
        Arc, Barrier,
        atomic::{AtomicBool, Ordering},
        mpsc,
    };
    let gate = Arc::new(Barrier::new(2));
    let armed = Arc::new(AtomicBool::new(false));
    let (woke, wake_done) = mpsc::channel();
    let s = Session::open(path, Access::ReadOnly, {
        let gate = gate.clone();
        let armed = armed.clone();
        move || {
            if armed.swap(false, Ordering::AcqRel) {
                gate.wait();
            }
            let _ = woke.send(());
        }
    })
    .unwrap();
    let opened = common::event(&s);
    assert!(matches!(opened.kind, EventKind::Opened(_)), "{opened:?}");
    wake_done.recv_timeout(Duration::from_secs(5)).unwrap();
    // Hold the worker inside the wake callback after its terminal event is visible.
    // Reusing the ID must succeed even before that callback returns.
    armed.store(true, Ordering::Release);
    let failed = query(&s, 30, "?");
    assert!(failed.completion.unwrap().error.is_some());
    let reused = s.submit(Request {
        ticket: s.ticket(30, 0),
        operation: Operation::Schema,
    });
    gate.wait();
    reused.unwrap();
    finished(&s);
    common::close(&s);
}
#[test]
fn close_discards_queued_work_and_rejects_new_submissions() {
    let (_dir, path) = fixture();
    let s = common::opened(&path, Access::ReadWrite);
    // 1,000 retained rows exceed the eight 64-row event slots, so execution
    // cannot finish while we receive only Columns. Keep draining cheap under coverage.
    submit(
        &s,
        1,
        execute(
            "WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<1000) SELECT n FROM x",
            Budget::default(),
        ),
    );
    assert!(matches!(common::event(&s).kind, EventKind::Columns(_)));
    submit(&s, 2, execute("DELETE FROM items", Budget::default()));
    s.close();
    let operation = Operation::Schema;
    let ticket = s.ticket(3, 0);
    assert!(s.submit(Request { ticket, operation }).is_err());
    loop {
        if let EventKind::Closed { rollback } = common::event(&s).kind {
            assert!(rollback.is_ok());
            break;
        }
    }
    let s = common::opened(&path, Access::ReadWrite);
    assert_eq!(count_items(&s, 1, ""), vec![vec![Value::Integer(2)]]);
    // RETURNING rows beyond the delivery budget are still drained and written.
    let limited = Budget {
        rows: 1,
        ..Budget::default()
    };
    let r = run(
        &s,
        2,
        "UPDATE items SET value='written' RETURNING id",
        limited,
    );
    assert_eq!(r.rows.len(), 1);
    assert_eq!(r.completion.unwrap().delivery, Delivery::Limited);
    let written = count_items(&s, 3, " WHERE value='written'");
    assert_eq!(written, vec![vec![Value::Integer(2)]]);
}
#[test]
fn invalid_open_inputs_never_create_or_rewrite_files() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("typo.db");
    assert!(
        Session::open_with_settings(
            path.clone(),
            Access::ReadOnly,
            ConnectionSettings {
                busy_timeout_ms: 5001,
                ..ConnectionSettings::default()
            },
            || {}
        )
        .is_err()
    );
    for contents in [None, Some(vec![]), Some(vec![0; 100])] {
        if let Some(bytes) = &contents {
            std::fs::write(&path, bytes).unwrap();
        }
        let (_session, failed) = common::start(&path, Access::ReadOnly);
        assert!(matches!(failed, EventKind::Failed(_)));
        assert_eq!(std::fs::read(&path).ok(), contents);
    }
    let (_session, failed) = common::start(dir.path(), Access::ReadOnly);
    assert!(matches!(failed, EventKind::Failed(_)));
}
