//! Real OS processes, disposable databases, and bounded protocol waits.
use crate::common;
use std::{
    io::{BufRead, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::mpsc,
    time::Duration,
};

struct Peer {
    child: Child,
    replies: mpsc::Receiver<String>,
}
impl Peer {
    fn start(path: &Path, mode: &str) -> (Self, String) {
        let p = Self::spawn(path, mode);
        let ready = p.reply();
        (p, ready)
    }
    fn spawn(path: &Path, mode: &str) -> Self {
        let mut child = Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "shared::peer", "--nocapture"])
            .env("TURSOBENCH_PEER_PATH", path)
            .env("TURSOBENCH_PEER_MODE", mode)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, replies) = mpsc::channel();
        std::thread::spawn(move || {
            for line in std::io::BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if let Some(reply) = line.strip_prefix("PEER|")
                    && tx.send(reply.to_owned()).is_err()
                {
                    break;
                }
            }
        });
        Self { child, replies }
    }
    fn reply(&self) -> String {
        self.replies
            .recv_timeout(Duration::from_secs(15))
            .expect("bounded peer reply")
    }
    fn sql(&mut self, sql: &str) -> String {
        self.send(sql);
        self.reply()
    }
    fn send(&mut self, sql: &str) {
        writeln!(self.child.stdin.as_mut().unwrap(), "{sql}").unwrap();
    }
    fn ok(&mut self, sql: &str) -> String {
        let reply = self.sql(sql);
        assert!(reply.starts_with("OK|"), "{sql}: {reply}");
        reply
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

#[test]
fn peer() {
    let Some(path) = std::env::var_os("TURSOBENCH_PEER_PATH") else {
        return;
    };
    let mode = std::env::var("TURSOBENCH_PEER_MODE").unwrap();
    common::block_on(async {
        let result = turso::Builder::new_local(path.to_str().unwrap())
            .experimental_index_method(true)
            .experimental_multiprocess_wal(mode != "legacy")
            .read_only(mode == "reader")
            .build()
            .await;
        let db = match result {
            Ok(db) => db,
            Err(e) => {
                println!("PEER|ERROR|{e}");
                return;
            }
        };
        let c = db.connect().unwrap();
        c.busy_timeout(Duration::from_millis(50)).unwrap();
        c.query_timeout(Duration::from_secs(3)).unwrap();
        println!(
            "PEER|READY|{}",
            db.is_multiprocess_wal_coordinated().unwrap()
        );
        for line in std::io::stdin().lock().lines() {
            let sql = line.unwrap();
            if sql == "CLOSE" {
                break;
            }
            let result = async {
                let mut rows = c.query(&sql, ()).await?;
                let mut values = Vec::new();
                while let Some(row) = rows.next().await? {
                    values.push(
                        (0..row.column_count())
                            .map(|i| row.get_value(i))
                            .collect::<turso::Result<Vec<_>>>()?,
                    );
                }
                Ok::<_, turso::Error>(values)
            }
            .await;
            match result {
                Ok(rows) => println!("PEER|OK|{rows:?}"),
                Err(e) => println!("PEER|ERROR|{e:?}"),
            }
        }
        drop(c);
        drop(db);
        println!("PEER|CLOSED");
    });
}

/// The one `items.value`, as the app reads it.
fn item(app: &mut common::Client) -> String {
    let rows = app.sql("SELECT value FROM items").rows;
    let [row] = &rows[..] else {
        panic!("one item expected: {rows:?}")
    };
    let [tursobench_core::Value::Text(value)] = &row[..] else {
        panic!("a text value expected: {row:?}")
    };
    value.clone()
}
/// Whether the app's next event announces an external commit with this `schema` flag.
fn external_change(app: &common::Client, schema: bool) -> bool {
    let kind = common::event(&app.session).kind;
    matches!(kind, tursobench_core::EventKind::Invalidated { schema: s, source_request: 0 } if s == schema)
}
fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    common::database(
        "shared.db",
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO items VALUES(1,'initial')",
        ],
    )
}

#[test]
fn coordinated_reader_tracks_commits_checkpoints_fts_restart_and_preserves_source_bytes() {
    let (_dir, path) = fixture();
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    writer.ok("CREATE INDEX search ON items USING fts(value)");
    let (mut reader, ready) = Peer::start(&path, "reader");
    assert_eq!(ready, "READY|true");
    assert!(reader.ok("SELECT value FROM items").contains("initial"));
    let found = |reader: &mut Peer, term: &str| {
        let sql = format!("SELECT id FROM items WHERE fts_match(value,'{term}')");
        reader.ok(&sql).contains("Integer(1)")
    };
    assert!(found(&mut reader, "initial"));
    for i in 0..12 {
        writer.ok(&format!("UPDATE items SET value='commit{i}'"));
        assert!(
            reader
                .ok("SELECT value FROM items")
                .contains(&format!("commit{i}"))
        );
        assert!(found(&mut reader, &format!("commit{i}")));
        assert!(!found(&mut reader, "initial"));
        writer.ok("PRAGMA wal_checkpoint(TRUNCATE)");
        assert!(
            reader
                .ok("SELECT value FROM items")
                .contains(&format!("commit{i}"))
        );
    }
    let before_db = std::fs::read(&path).unwrap();
    let wal = path.with_extension("db-wal");
    let before_wal = std::fs::read(&wal).ok();
    assert!(
        reader
            .sql("INSERT INTO items VALUES(2,'denied')")
            .starts_with("ERROR|")
    );
    reader.ok("SELECT * FROM items");
    assert_eq!(std::fs::read(&path).unwrap(), before_db);
    assert_eq!(std::fs::read(&wal).ok(), before_wal);
    writer.ok("BEGIN IMMEDIATE");
    writer.ok("UPDATE items SET value='uncommitted'");
    assert!(reader.ok("SELECT value FROM items").contains("commit11"));
    drop(writer); // Process death releases native locks; uncommitted WAL stays invisible.
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    writer.ok("UPDATE items SET value='recovered'");
    assert!(reader.ok("SELECT value FROM items").contains("recovered"));
    assert!(writer.ok("PRAGMA integrity_check").contains("ok"));
}

#[test]
fn access_preflight_reports_locks_without_writing_or_interfering_with_shared_writers() {
    use tursobench_core::{Access, ErrorKind};
    let (dir, path) = fixture();
    let files = common::Files::record(dir.path());
    assert!(tursobench_db::access::check_access(&path).is_ok());
    assert!(!dir.path().join("shared.db-tshm").exists());
    assert!(files.unchanged());
    {
        let (legacy, _) = Peer::start(&path, "legacy");
        let error = tursobench_db::access::check_access(&path).unwrap_err();
        assert_eq!(error.kind, ErrorKind::Busy);
        assert!(error.message.contains(&legacy.child.id().to_string()));
    }
    assert!(tursobench_db::access::check_access(&path).is_ok());
    {
        let (mut writer, _) = Peer::start(&path, "writer");
        writer.ok("BEGIN IMMEDIATE");
        writer.ok("UPDATE items SET value='pending'");
        for _ in 0..8 {
            assert!(tursobench_db::access::check_access(&path).is_ok());
        }
        writer.ok("ROLLBACK");
    }
    let app = common::Client::open(&path, Access::ReadOnly);
    assert_eq!(
        tursobench_db::access::check_access(&path).unwrap_err().kind,
        ErrorKind::Busy
    );
    app.close();
    assert!(tursobench_db::access::check_access(&path).is_ok());
}

#[test]
fn preflight_preserves_existing_process_owned_posix_locks() {
    let (_dir, path) = fixture();
    let db = common::block_on(turso::Builder::new_local(path.to_str().unwrap()).build()).unwrap();
    assert_eq!(
        tursobench_db::access::check_access(&path).unwrap_err().kind,
        tursobench_core::ErrorKind::Busy
    );
    let (_legacy, reply) = Peer::start(&path, "legacy");
    assert!(
        reply.starts_with("ERROR|"),
        "preflight released the existing POSIX lock: {reply}"
    );
    drop(db);
    assert!(tursobench_db::access::check_access(&path).is_ok());
}

#[test]
fn close_ack_releases_all_admission_locks_and_protected_reads_preserve_content() {
    use tursobench_core::{Access, Coordination, EventKind};
    let (dir, path) = fixture();
    for _ in 0..8 {
        let files = common::Files::record(dir.path());
        let (s, opened) = common::start(&path, Access::ReadOnly);
        assert!(matches!(
            opened,
            EventKind::Opened(Coordination::ProtectedReadOnly)
        ));
        common::run(&s, 1, tursobench_db::Operation::Schema);
        common::close(&s);
        assert!(files.unchanged());
        // No delay: Closed promises every lock has already been released.
        let (_legacy, reply) = Peer::start(&path, "legacy");
        assert_eq!(reply, "READY|false");
    }
}

#[test]
fn hard_link_aliases_are_rejected_before_creating_coordination_sidecars() {
    use tursobench_core::{Access, EventKind};
    let (dir, path) = fixture();
    let alias = dir.path().join("alias.db");
    std::fs::hard_link(&path, &alias).unwrap();
    let files = common::Files::record(dir.path());
    for p in [&path, &alias] {
        let (_session, opened) = common::start(p, Access::ReadOnly);
        assert!(matches!(opened, EventKind::Failed(e) if e.message.contains("Hard-linked")));
    }
    assert!(!dir.path().join("shared.db-tshm").exists());
    assert!(!dir.path().join("alias.db-tshm").exists());
    assert!(files.unchanged());
}

#[test]
fn application_admission_blocks_legacy_writers_and_reserves_readonly_fallback() {
    use tursobench_core::{Access, Coordination, ErrorKind, EventKind};
    let (_dir, path) = fixture();
    {
        let (_legacy, _) = Peer::start(&path, "legacy");
        let usage = tursobench_db::access::processes_using(&path);
        assert!(!usage.applications.is_empty() || usage.incomplete);
        for access in [Access::ReadOnly, Access::ReadWrite] {
            let (_session, opened) = common::start(&path, access);
            assert!(matches!(opened, EventKind::Failed(e) if e.kind == ErrorKind::Busy));
        }
    }
    let (s, opened) = common::start(&path, Access::ReadOnly);
    assert!(matches!(
        opened,
        EventKind::Opened(Coordination::ProtectedReadOnly)
    ));
    let (_legacy, ready) = Peer::start(&path, "legacy");
    assert!(ready.starts_with("ERROR|"), "{ready}");
    let blocked = Peer::spawn(&path, "writer");
    assert!(
        blocked
            .replies
            .recv_timeout(Duration::from_millis(100))
            .is_err()
    );
    let (_denied, refused) = common::start(&path, Access::ReadWrite);
    assert!(matches!(refused, EventKind::Failed(e) if e.kind == ErrorKind::Busy));
    common::close(&s);
    assert!(blocked.reply().starts_with("ERROR|"));
    drop(blocked);
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    let (s, opened) = common::start(&path, Access::ReadOnly);
    assert!(matches!(opened, EventKind::Opened(Coordination::SharedWal)));
    writer.ok("UPDATE items SET value='visible'");
    let result = common::run(
        &s,
        1,
        common::execute("SELECT value FROM items", Default::default()),
    );
    assert_eq!(
        result.result.rows,
        vec![vec![tursobench_core::Value::Text("visible".into())]]
    );
    common::close(&s);
}

#[test]
fn engine_mixed_mode_open_probe() {
    let (_dir, path) = fixture();
    // A read-only open alone is not proof of coordination.
    let (reader, ready) = Peer::start(&path, "reader");
    assert_eq!(ready, "READY|false");
    drop(reader);
    let (_legacy, ready) = Peer::start(&path, "legacy");
    assert_eq!(ready, "READY|false");
    let (_shared, reply) = Peer::start(&path, "writer");
    // The SDK supplies custom storage, bypassing the core's legacy lock probe.
    // Our admission layer must reject this case before calling the builder.
    assert_eq!(reply, "READY|true");
}

#[test]
fn external_commits_notify_data_and_schema_without_changing_manual_snapshots() {
    use tursobench_core::{Access, Value};
    let (_dir, path) = fixture();
    let (mut writer, _) = Peer::start(&path, "writer");
    let mut app = common::Client::open(&path, Access::ReadWrite);
    app.sql("BEGIN");
    assert_eq!(item(&mut app), "initial");
    writer.ok("UPDATE items SET value='external'");
    assert!(external_change(&app, false));
    assert_eq!(item(&mut app), "initial");
    app.sql("ROLLBACK");
    assert_eq!(item(&mut app), "external");
    writer.ok("ALTER TABLE items ADD COLUMN added INTEGER");
    assert!(external_change(&app, true));
    assert_eq!(
        app.sql("SELECT added FROM items").rows,
        vec![vec![Value::Null]]
    );
    app.close();
}

#[test]
fn shared_edits_report_busy_conflicts_and_refuse_live_peer_backup() {
    use tursobench_core::{Access, Edit, ErrorKind, EventKind, Value};
    use tursobench_db::Operation;
    let (dir, path) = fixture();
    let (mut writer, _) = Peer::start(&path, "writer");
    let mut app = common::Client::open(&path, Access::ReadWrite);
    let (table, result) = app.browse("items", 0);
    let original = result.rows[0].clone();
    writer.ok("BEGIN IMMEDIATE");
    writer.ok("UPDATE items SET value='pending'");
    let busy = app.failure("UPDATE items SET value='must not replay'");
    assert_eq!(busy.unwrap().kind, ErrorKind::Busy);
    let edit = Edit::Update {
        original: original.clone(),
        changes: vec![(1, Value::Text("draft".into()))],
    };
    common::rejected(
        &app.commit(table.clone(), vec![edit.clone()]),
        ErrorKind::Busy,
    );
    let reply = app.run(common::script("UPDATE items SET value='atomic'", true));
    assert_eq!(reply.failure.unwrap().kind, ErrorKind::Busy);
    let input = dir.path().join("contended.csv");
    std::fs::write(&input, "id,value\n2,imported\n").unwrap();
    let reply = app.run(Operation::Import(tursobench_core::ImportSpec {
        path: input,
        table: "items".into(),
        format: tursobench_core::ImportFormat::Csv,
        source_columns: vec!["id".into(), "value".into()],
        mapping: vec!["id".into(), "value".into()],
        kinds: vec![
            tursobench_core::InputKind::Integer,
            tursobench_core::InputKind::Text,
        ],
        null_marker: "\\N".into(),
        atomic: true,
    }));
    assert_eq!(reply.failure.unwrap().kind, ErrorKind::Busy);
    writer.ok("ROLLBACK");
    assert_eq!(item(&mut app), "initial");
    writer.ok("UPDATE items SET value='external'");
    common::rejected(&app.commit(table, vec![edit]), ErrorKind::Conflict);
    let committed = app.update_first("items", vec![(1, Value::Text("committed".into()))]);
    common::committed(&committed, None);
    assert!(writer.ok("SELECT value FROM items").contains("committed"));
    writer.ok("BEGIN");
    writer.ok("SELECT * FROM items");
    app.sql("UPDATE items SET value='newer'");
    assert_eq!(
        app.run(Operation::Checkpoint).failure.unwrap().kind,
        ErrorKind::Busy
    );
    writer.ok("ROLLBACK");
    assert!(app.run(Operation::Checkpoint).failure.is_none());
    let destination = dir.path().join("must-not-publish.db");
    let reply = app.run(Operation::BackupAndClose(destination.clone()));
    assert_eq!(reply.transfer.unwrap().error.unwrap().kind, ErrorKind::Busy);
    assert!(!destination.exists());
    loop {
        match common::event(&app.session).kind {
            EventKind::Invalidated { .. } => continue,
            EventKind::Closed { rollback: Ok(()) } => break,
            other => panic!("Unexpected backup close event: {other:?}"),
        }
    }
    writer.ok("UPDATE items SET value='still usable'");
    assert!(writer.ok("PRAGMA integrity_check").contains("ok"));
}

#[test]
fn simultaneous_shared_writes_have_no_lost_or_replayed_updates_and_keep_fts_consistent() {
    use tursobench_core::{Access, ErrorKind, Value};
    let (_dir, path) = fixture();
    let (mut writer, _) = Peer::start(&path, "writer");
    writer.ok("CREATE TABLE counters(id INTEGER PRIMARY KEY, n INTEGER)");
    writer.ok("INSERT INTO counters VALUES(1,0)");
    writer.ok("INSERT INTO items VALUES(2,'second')");
    writer.ok("CREATE INDEX search ON items USING fts(value)");
    let mut app = common::Client::open(&path, Access::ReadWrite);
    // The app's lifetime main-file lock also rejects a later legacy writer.
    let (_legacy, reply) = Peer::start(&path, "legacy");
    assert!(reply.starts_with("ERROR|"));
    let mut successes = 0i64;
    for i in 0..40 {
        writer.send("UPDATE counters SET n=n+1");
        let reply = app.query("UPDATE counters SET n=n+1", vec![]);
        match reply.completion.unwrap().error {
            None => successes += 1,
            Some(e) => assert_eq!(e.kind, ErrorKind::Busy),
        }
        let reply = writer.reply();
        if reply.starts_with("OK|") {
            successes += 1
        } else {
            assert!(reply.contains("Busy"), "{reply}");
        }
        writer.ok(&format!("UPDATE items SET value='writer{i}' WHERE id=1"));
        app.sql(&format!("UPDATE items SET value='editor{i}' WHERE id=2"));
        assert_eq!(
            app.sql(&format!(
                "SELECT id FROM items WHERE fts_match(value,'writer{i}')"
            ))
            .rows,
            vec![vec![Value::Integer(1)]]
        );
        assert!(
            writer
                .ok(&format!(
                    "SELECT id FROM items WHERE fts_match(value,'editor{i}')"
                ))
                .contains("Integer(2)")
        );
    }
    assert!(successes > 0);
    assert_eq!(
        app.sql("SELECT n FROM counters").rows,
        vec![vec![Value::Integer(successes)]]
    );
    assert_eq!(
        app.sql("PRAGMA integrity_check").rows,
        vec![vec![Value::Text("ok".into())]]
    );
    app.close();
}

#[test]
fn coordinated_policy_covers_single_script_and_export_before_execution() {
    use tursobench_core::{Access, Bindings};
    use tursobench_db::Operation;
    let (dir, path) = fixture();
    let mut app = common::Client::open(&path, Access::ReadWrite);
    for sql in [
        "PRAGMA journal_mode=DELETE",
        "PRAGMA main.\"locking_mode\"(EXCLUSIVE)",
        "BEGIN CONCURRENT",
    ] {
        assert!(app.failure(sql).is_some());
        let reply = app.run(common::script(
            &format!("UPDATE items SET value='denied'; {sql}"),
            false,
        ));
        assert!(reply.failure.is_some());
        let destination = dir.path().join("denied.jsonl");
        assert!(
            app.run(Operation::ExportQuery {
                path: destination.clone(),
                sql: sql.into(),
                parameters: Bindings::Positional(vec![])
            })
            .failure
            .is_some()
        );
        assert!(!destination.exists());
    }
    assert_eq!(item(&mut app), "initial");
    app.close();
}
