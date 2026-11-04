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
        writeln!(self.child.stdin.as_mut().unwrap(), "{sql}").unwrap();
        self.reply()
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
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    rt.block_on(async {
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

fn fixture() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("shared.db");
    common::create(
        &path,
        &[
            "CREATE TABLE items(id INTEGER PRIMARY KEY, value TEXT)",
            "INSERT INTO items VALUES(1,'initial')",
        ],
    );
    (dir, path)
}

#[test]
fn coordinated_reader_tracks_commits_checkpoints_restart_and_preserves_source_bytes() {
    let (_dir, path) = fixture();
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    let (mut reader, ready) = Peer::start(&path, "reader");
    assert_eq!(ready, "READY|true");
    assert!(reader.ok("SELECT value FROM items").contains("initial"));
    for i in 0..12 {
        writer.ok(&format!("UPDATE items SET value='commit{i}'"));
        assert!(
            reader
                .ok("SELECT value FROM items")
                .contains(&format!("commit{i}"))
        );
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
fn readonly_flag_is_not_proof_of_coordination() {
    let (_dir, path) = fixture();
    let (_reader, ready) = Peer::start(&path, "reader");
    assert_eq!(ready, "READY|false");
}

#[test]
fn application_admission_blocks_legacy_writers_and_reserves_readonly_fallback() {
    use tursobench_core::{Access, Coordination, ErrorKind, EventKind};
    let (_dir, path) = fixture();
    {
        let (_legacy, _) = Peer::start(&path, "legacy");
        let usage = tursobench_db::processes_using(&path);
        assert!(!usage.applications.is_empty() || usage.incomplete);
        for access in [Access::ReadOnly, Access::ReadWrite] {
            let s = tursobench_db::Session::open(path.clone(), access, || {}).unwrap();
            assert!(
                matches!(common::event(&s).kind, EventKind::Failed(e) if e.kind == ErrorKind::Busy)
            );
        }
    }
    let s = tursobench_db::Session::open(path.clone(), Access::ReadOnly, || {}).unwrap();
    assert!(matches!(
        common::event(&s).kind,
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
    let denied = tursobench_db::Session::open(path.clone(), Access::ReadWrite, || {}).unwrap();
    assert!(
        matches!(common::event(&denied).kind, EventKind::Failed(e) if e.kind == ErrorKind::Busy)
    );
    s.close();
    assert!(matches!(
        common::event(&s).kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
    assert!(blocked.reply().starts_with("ERROR|"));
    drop(blocked);
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    let s = tursobench_db::Session::open(path.clone(), Access::ReadOnly, || {}).unwrap();
    assert!(matches!(
        common::event(&s).kind,
        EventKind::Opened(Coordination::SharedWal)
    ));
    writer.ok("UPDATE items SET value='visible'");
    let result = common::run(
        &s,
        1,
        tursobench_db::Operation::Execute {
            sql: "SELECT value FROM items".into(),
            parameters: vec![],
            budget: Default::default(),
        },
    );
    assert_eq!(
        result.result.rows,
        vec![vec![tursobench_core::Value::Text("visible".into())]]
    );
    s.close();
    assert!(matches!(
        common::event(&s).kind,
        EventKind::Closed { rollback: Ok(()) }
    ));
}

#[test]
fn engine_mixed_mode_open_probe() {
    let (_dir, path) = fixture();
    let (_legacy, ready) = Peer::start(&path, "legacy");
    assert_eq!(ready, "READY|false");
    let (_shared, reply) = Peer::start(&path, "writer");
    // The SDK supplies custom storage, bypassing the core's legacy lock probe.
    // Our admission layer must reject this case before calling the builder.
    assert_eq!(reply, "READY|true");
}

#[test]
fn external_commits_notify_data_and_schema_without_changing_manual_snapshots() {
    use tursobench_core::{Access, EventKind, Value};
    let (_dir, path) = fixture();
    let (mut writer, _) = Peer::start(&path, "writer");
    let mut app = common::Client::open(&path, Access::ReadWrite);
    app.sql("BEGIN");
    assert_eq!(
        app.sql("SELECT value FROM items").rows,
        vec![vec![Value::Text("initial".into())]]
    );
    writer.ok("UPDATE items SET value='external'");
    assert!(matches!(
        common::event(&app.session).kind,
        EventKind::Invalidated {
            schema: false,
            source_request: 0
        }
    ));
    assert_eq!(
        app.sql("SELECT value FROM items").rows,
        vec![vec![Value::Text("initial".into())]]
    );
    app.sql("ROLLBACK");
    assert_eq!(
        app.sql("SELECT value FROM items").rows,
        vec![vec![Value::Text("external".into())]]
    );
    writer.ok("ALTER TABLE items ADD COLUMN added INTEGER");
    assert!(matches!(
        common::event(&app.session).kind,
        EventKind::Invalidated {
            schema: true,
            source_request: 0
        }
    ));
    assert_eq!(
        app.sql("SELECT added FROM items").rows,
        vec![vec![Value::Null]]
    );
    app.close();
}

#[test]
fn coordinated_native_fts_reads_follow_external_commits() {
    let (_dir, path) = fixture();
    let (mut writer, ready) = Peer::start(&path, "writer");
    assert_eq!(ready, "READY|true");
    writer.ok("CREATE INDEX search ON items USING fts(value)");
    let (mut reader, ready) = Peer::start(&path, "reader");
    assert_eq!(ready, "READY|true");
    assert!(
        reader
            .ok("SELECT id FROM items WHERE fts_match(value,'initial')")
            .contains("Integer(1)")
    );
    writer.ok("UPDATE items SET value='replacement'");
    assert!(
        reader
            .ok("SELECT id FROM items WHERE fts_match(value,'replacement')")
            .contains("Integer(1)")
    );
    assert!(
        !reader
            .ok("SELECT id FROM items WHERE fts_match(value,'initial')")
            .contains("Integer(1)")
    );
}
