use crate::common::{self, Client};
use std::time::Duration;
use tursobench_core::{Access, ErrorKind};
use tursobench_db::assistant::{MAX_ROWS, Tool};

const WAIT: Duration = Duration::from_secs(30);

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("assistant.db");
    common::create(
        &path,
        &[
            "CREATE TABLE owners(id INTEGER PRIMARY KEY, name TEXT NOT NULL)",
            "CREATE TABLE pets(id INTEGER PRIMARY KEY, owner INTEGER REFERENCES owners(id), \
             name TEXT, photo BLOB)",
            "CREATE INDEX pets_owner ON pets(owner)",
            "CREATE VIEW named AS SELECT name FROM pets",
            "INSERT INTO owners VALUES (1, 'Ada')",
            "INSERT INTO pets VALUES (1, 1, 'Rex', x'00ff'), (2, 1, NULL, NULL)",
        ],
    );
    (dir, path)
}

#[test]
fn tools_describe_and_read_committed_state_without_joining_the_editor_transaction() {
    let (_dir, path) = fixture();
    let mut client = Client::open(&path, Access::ReadWrite);
    let tools = client.session.assistant();

    let listed = tools.call(Tool::ListTables, WAIT).unwrap();
    let names: Vec<_> = listed["objects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|o| o["name"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(names, ["owners", "pets", "named"]);

    let pets = tools.call(Tool::Describe("pets".into()), WAIT).unwrap();
    assert_eq!(pets["columns"].as_array().unwrap().len(), 4);
    assert_eq!(pets["foreign_keys"][0]["references"], "owners");
    assert_eq!(pets["indexes"][0]["name"], "pets_owner");
    assert!(
        pets["ddl"]
            .as_str()
            .unwrap()
            .starts_with("CREATE TABLE pets")
    );

    let rows = tools
        .call(
            Tool::Query("SELECT name, photo FROM pets ORDER BY id".into()),
            WAIT,
        )
        .unwrap();
    assert_eq!(rows["columns"], serde_json::json!(["name", "photo"]));
    assert_eq!(rows["rows"][0][0], "Rex");
    assert_eq!(rows["rows"][0][1]["blob_bytes"], 2);
    assert!(rows["rows"][1][0].is_null());

    // The editor's uncommitted work stays invisible and its transaction stays open.
    client.sql("BEGIN");
    client.sql("INSERT INTO owners VALUES (2, 'Grace')");
    let count = tools
        .call(Tool::Query("SELECT count(*) FROM owners".into()), WAIT)
        .unwrap();
    assert_eq!(count["rows"][0][0], 1);
    let state = client.sql("SELECT count(*) FROM owners");
    assert_eq!(
        state.completion.unwrap().transaction,
        tursobench_core::TransactionState::Active
    );
    client.sql("ROLLBACK");
    client.close();
}

#[test]
fn writes_are_refused_by_the_gate_and_by_the_engine() {
    let (_dir, path) = fixture();
    let mut client = Client::open(&path, Access::ReadWrite);
    let tools = client.session.assistant();
    for sql in [
        "DELETE FROM pets",
        "WITH x AS (SELECT 1) INSERT INTO owners SELECT 9, 'x' FROM x",
        "PRAGMA query_only = 0",
        "SELECT 1; DELETE FROM pets",
    ] {
        let error = tools.call(Tool::Query(sql.into()), WAIT).unwrap_err();
        assert!(
            matches!(error.kind, ErrorKind::ReadOnly | ErrorKind::Input),
            "{sql}: {error:?}"
        );
    }
    let rows = client.sql("SELECT count(*) FROM pets");
    assert_eq!(rows.rows[0][0], tursobench_core::Value::Integer(2));
    client.close();
}

#[test]
fn the_database_location_is_not_readable() {
    let (_dir, path) = fixture();
    let client = Client::open(&path, Access::ReadOnly);
    let tools = client.session.assistant();
    for sql in [
        "SELECT * FROM pragma_database_list",
        "SELECT file FROM \"PRAGMA_DATABASE_LIST\"()",
        "SELECT file FROM [pragma_database_list]",
        "EXPLAIN SELECT * FROM pragma_database_list",
    ] {
        let error = tools.call(Tool::Query(sql.into()), WAIT).unwrap_err();
        assert!(error.message.contains("location"), "{sql}: {error:?}");
    }
    // Other table-valued pragmas stay available.
    let info = tools
        .call(
            Tool::Query("SELECT name FROM pragma_table_info('pets')".into()),
            WAIT,
        )
        .unwrap();
    assert_eq!(info["row_count"], 4);
    client.close();
}

#[test]
fn turso_query_only_rejects_writes_on_its_connection() {
    // The engine guard behind the parser gate, checked directly.
    let (_dir, path) = fixture();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    runtime.block_on(async {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        c.execute("PRAGMA query_only = 1", ()).await.unwrap();
        assert!(c.execute("DELETE FROM pets", ()).await.is_err());
        assert!(c.execute("CREATE TABLE z(a)", ()).await.is_err());
        let mut rows = c.query("SELECT count(*) FROM pets", ()).await.unwrap();
        let row = rows.next().await.unwrap().unwrap();
        assert_eq!(row.get_value(0).unwrap(), turso::Value::Integer(2));
    });
}

#[test]
fn results_are_bounded_and_calls_fail_cleanly_after_close() {
    let (_dir, path) = fixture();
    let client = Client::open(&path, Access::ReadOnly);
    let tools = client.session.assistant();
    let many = tools
        .call(
            Tool::Query(
                "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<5000) \
                 SELECT x, printf('%.3000c', 'a') FROM n"
                    .into(),
            ),
            WAIT,
        )
        .unwrap();
    assert!(many["limited"].as_bool().unwrap());
    assert!(many["rows"].as_array().unwrap().len() <= MAX_ROWS);
    assert!(many["truncated_cells"].as_u64().unwrap() > 0);
    assert!(tools.call(Tool::Describe("missing".into()), WAIT).is_err());
    client.close();
    assert!(tools.call(Tool::ListTables, WAIT).is_err());
}

#[test]
fn location_queries_hidden_in_views_are_refused() {
    let (_dir, path) = fixture();
    let mut client = Client::open(&path, Access::ReadWrite);
    client.sql("CREATE VIEW diagnostic AS SELECT file FROM pragma_database_list AS p");
    client.sql("CREATE VIEW nested AS SELECT * FROM diagnostic");
    client.sql(r#"CREATE VIEW "escaped""name" AS SELECT * FROM nested"#);
    client.sql("CREATE VIEW [diagnostic view] AS SELECT * FROM nested");
    let tools = client.session.assistant();
    for sql in [
        r#"SELECT * FROM "escaped""name""#,
        "SELECT * FROM [diagnostic view]",
        "SELECT * FROM diagnostic",
        "SELECT * FROM nested AS n",
        "SELECT * FROM 'nested'",
        "EXPLAIN SELECT * FROM nested",
        "EXPLAIN QUERY PLAN SELECT * FROM nested",
        "WITH x AS (SELECT * FROM nested) SELECT * FROM x",
    ] {
        let error = tools.call(Tool::Query(sql.into()), WAIT).unwrap_err();
        assert!(error.message.contains("location"), "{sql}: {error:?}");
    }
    // Unrelated views still work, and failed checks release their snapshot.
    assert!(
        tools
            .call(Tool::Query("SELECT * FROM named".into()), WAIT)
            .is_ok()
    );
    client.sql("DROP VIEW nested");
    client.sql("CREATE VIEW nested AS SELECT name FROM pets");
    assert!(
        tools
            .call(Tool::Query("SELECT * FROM nested".into()), WAIT)
            .is_ok()
    );
    client.close();
}
