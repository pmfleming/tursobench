use crate::common::{self, Client, local_fixtures as fixtures};

use tursobench_core::{
    Access, Browse, Delivery, Edit, ErrorKind, TransactionState, Value, quote_identifier,
};
use tursobench_db::Operation;

fn fixture(name: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{name}.db"));
    common::block_on(fixtures::create(&path, name)).unwrap();
    (dir, path)
}
fn text(s: &str) -> Value {
    Value::Text(s.into())
}
fn matches(c: &mut Client, needle: &str) -> Vec<Vec<Value>> {
    let r = c.query(
        "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1) ORDER BY entry_id",
        vec![text(needle)],
    );
    assert!(r.completion.unwrap().error.is_none());
    r.rows
}

#[test]
fn exact_project_schemas_open_browse_filter_page_and_preserve_readonly_files() {
    for name in fixtures::NAMES {
        let (dir, path) = fixture(name);
        let files = common::Files::record(dir.path());
        let mut c = Client::open(&path, Access::ReadOnly);
        let schema = c.run(Operation::Schema).result;
        assert!(schema.completion.unwrap().error.is_none());
        for row in schema.rows.iter().filter(|row| row[0] == text("table")) {
            let Value::Text(table) = &row[1] else {
                panic!("{row:?}")
            };
            let (description, page) = c.browse(table, 0);
            assert!(description.is_table);
            assert_eq!(page.columns.len(), description.columns.len());
            assert!(page.rows.len() <= 100);
        }
        let paged = match name {
            "scratchpad" => "buffer_ids",
            "agenda" => "items",
            "notifications" => "notifications",
            _ => "entries",
        };
        let (table, first) = c.browse(paged, 0);
        let (_, second) = c.browse(paged, 100);
        let (_, last) = c.browse(paged, 200);
        assert_eq!(
            (first.rows.len(), second.rows.len(), last.rows.len()),
            (100, 100, fixtures::ROWS - 200)
        );
        assert_ne!(first.rows[0], second.rows[0]);
        let key = table.keys()[0];
        let wanted = first.rows[0][key].clone();
        let filtered = c.run(Operation::Browse(Browse {
            sort: Some(key),
            descending: true,
            filters: vec![tursobench_core::Filter {
                column: key,
                op: tursobench_core::FilterOp::Equal,
                value: wanted,
            }],
            ..common::browse(paged)
        }));
        assert_eq!(filtered.result.rows, vec![first.rows[0].clone()]);
        let delete = format!("DELETE FROM {}", quote_identifier(paged));
        assert_eq!(c.failure(&delete).unwrap().kind, ErrorKind::ReadOnly);
        match name {
            "clipboard" => {
                assert_eq!(matches(&mut c, "needle"), vec![vec![text("entry-0000")]]);
                let large = c.sql("SELECT bytes FROM payloads WHERE entry_id='entry-0204'");
                assert!(matches!(&large.rows[0][0],Value::Deferred(cell) if cell.handle.is_some()));
                assert_eq!(large.completion.unwrap().delivery, Delivery::Complete);
            }
            "scratchpad" => {
                let largest = "SELECT length(body) FROM blobs ORDER BY bytes DESC LIMIT 1";
                assert_eq!(c.sql(largest).rows, vec![vec![Value::Integer(1024 * 1024)]]);
            }
            "notifications" => assert_eq!(first.rows[0][8], Value::Null),
            _ => {}
        }
        c.close();
        assert!(
            files.unchanged(),
            "read-only access changed {name} fixture files"
        );
    }
}

#[test]
fn native_fts_creation_queries_returning_and_engine_readonly_enforcement() {
    let (_dir, path) = fixture("agenda");
    let mut c = Client::open(&path, Access::ReadWrite);
    c.sql("CREATE TABLE documents(id INTEGER PRIMARY KEY, body TEXT NOT NULL) STRICT");
    c.sql("CREATE INDEX documents_fts ON documents USING fts(body)");
    assert_eq!(
        c.sql("INSERT INTO documents VALUES(1,'hello Turso'),(2,'hello world') RETURNING id")
            .rows
            .len(),
        2
    );
    assert_eq!(
        c.sql("SELECT id FROM documents WHERE fts_match(body,'Turso')")
            .rows,
        vec![vec![Value::Integer(1)]]
    );
    let plan = c.sql("EXPLAIN QUERY PLAN SELECT id FROM documents WHERE fts_match(body,'Turso')");
    assert!(
        plan.rows
            .iter()
            .flatten()
            .any(|v| matches!(v, Value::Text(s) if s.contains("fts")))
    );
    c.close();
    let mut c = Client::open(&path, Access::ReadOnly);
    assert_eq!(
        c.sql("SELECT id FROM documents WHERE fts_match(body,'Turso')")
            .rows
            .len(),
        1
    );
    for denied in [
        "CREATE INDEX denied_fts ON documents USING fts(body)",
        "UPDATE documents SET body='changed' WHERE id=1",
    ] {
        assert_eq!(c.failure(denied).unwrap().kind, ErrorKind::ReadOnly);
    }
    c.close();
}

#[test]
fn clipboard_fts_staged_edits_conflicts_constraints_and_transaction_rollback() {
    let (_dir, path) = fixture("clipboard");
    let mut c = Client::open(&path, Access::ReadWrite);
    let (table, original) = c.browse("search_content", 0);
    assert!(table.editable());
    let row = original.rows[0].clone();
    let outcome = c.commit(
        table.clone(),
        vec![Edit::Update {
            original: row.clone(),
            changes: vec![(1, text("changed marker")), (2, text("changed marker"))],
        }],
    );
    common::committed(&outcome, Some(1));
    assert!(matches(&mut c, "needle").is_empty());
    assert_eq!(matches(&mut c, "marker"), vec![vec![text("entry-0000")]]);
    let stale = c.commit(table.clone(), vec![Edit::Delete { original: row }]);
    common::rejected(&stale, ErrorKind::Conflict);
    let (_, current) = c.browse("search_content", 0);
    let outcome = c.commit(
        table.clone(),
        vec![
            Edit::Update {
                original: current.rows[0].clone(),
                changes: vec![(2, text("rollback token"))],
            },
            Edit::Insert {
                values: vec![
                    Some(text("missing-parent")),
                    Some(text("bad")),
                    Some(text("bad")),
                    Some(Value::Integer(1)),
                ],
            },
        ],
    );
    common::rejected(&outcome, ErrorKind::Constraint);
    assert!(matches(&mut c, "rollback").is_empty());
    assert_eq!(matches(&mut c, "marker").len(), 1);
    c.sql("BEGIN IMMEDIATE");
    c.sql(
        "UPDATE search_content SET indexed_content='uncommitted token' WHERE entry_id='entry-0000'",
    );
    assert_eq!(matches(&mut c, "uncommitted").len(), 1);
    let (_, page) = c.browse("search_content", 0);
    assert_eq!(page.rows[0][2], text("changed marker"));
    assert_eq!(
        page.completion.unwrap().transaction,
        TransactionState::Active
    );
    c.sql("ROLLBACK");
    assert!(matches(&mut c, "uncommitted").is_empty());
    assert_eq!(matches(&mut c, "marker").len(), 1);
    let outcome = c.commit(
        table.clone(),
        vec![Edit::Delete {
            original: current.rows[0].clone(),
        }],
    );
    common::committed(&outcome, None);
    assert!(matches(&mut c, "marker").is_empty());
    let outcome = c.commit(
        table,
        vec![Edit::Insert {
            values: vec![
                Some(text("entry-0000")),
                Some(text("inserted searchable")),
                Some(text("inserted searchable")),
                Some(Value::Integer(1)),
            ],
        }],
    );
    common::committed(&outcome, Some(1));
    assert_eq!(
        matches(&mut c, "searchable"),
        vec![vec![text("entry-0000")]]
    );
    c.close();
    let mut c = Client::open(&path, Access::ReadOnly);
    assert!(matches(&mut c, "marker").is_empty());
    assert_eq!(
        matches(&mut c, "searchable"),
        vec![vec![text("entry-0000")]]
    );
    c.close();
}

#[test]
fn project_typed_edits_and_close_rollback() {
    for (name, table_name, column, value) in [
        (
            "scratchpad",
            "history",
            3,
            Value::Blob(vec![0, 255, 13, 10]),
        ),
        ("agenda", "items", 4, Value::Integer(2)),
        (
            "notifications",
            "notifications",
            8,
            Value::Integer(1700000002000),
        ),
    ] {
        let (_dir, path) = fixture(name);
        let mut c = Client::open(&path, Access::ReadWrite);
        let committed = c.update_first(table_name, vec![(column, value.clone())]);
        common::committed(&committed, None);
        let (_, after) = c.browse(table_name, 0);
        assert_eq!(after.rows[0][column], value);
        c.sql("BEGIN");
        c.sql(&format!("DELETE FROM {}", quote_identifier(table_name)));
        c.close();
        let mut c = Client::open(&path, Access::ReadOnly);
        let (_, reopened) = c.browse(table_name, 0);
        assert_eq!(after.rows, reopened.rows);
        c.close();
    }
}
