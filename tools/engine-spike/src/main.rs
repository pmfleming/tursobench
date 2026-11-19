//! Disposable probes only. Never run this harness against valuable files.
use std::{
    path::Path,
    process::Command,
    time::{Duration, Instant},
};
use turso::{Builder, Connection, Rows, Statement, Value, params_from_iter};

type Probe<T = ()> = Result<T, Box<dyn std::error::Error>>;
const FTS_COUNT: &str = "SELECT count(*) FROM fts_probe WHERE fts_match(body,'native')";
const SAMPLE_COUNT: &str = "SELECT count(*) FROM sample";

fn local(path: &str) -> Builder {
    Builder::new_local(path).experimental_index_method(true)
}
fn reader(path: &Path) -> Builder {
    local(path.to_str().unwrap()).read_only(true)
}

async fn scalar(c: &Connection, sql: &str) -> turso::Result<Value> {
    let mut r = c.query(sql, ()).await?;
    let value = r.next().await?.unwrap().get_value(0)?;
    while r.next().await?.is_some() {}
    Ok(value)
}
async fn count(c: &Connection, sql: &str) -> Probe<i64> {
    match scalar(c, sql).await? {
        Value::Integer(n) => Ok(n),
        other => Err(format!("{sql}: expected an integer, got {other:?}").into()),
    }
}

/// Child process: report whether a second process can read the probe file.
async fn child(path: &str, shared: &str) -> Probe {
    let builder = reader(Path::new(path)).experimental_multiprocess_wal(shared == "true");
    match builder.build().await {
        Ok(db) => println!(
            "child read: {:?}",
            scalar(&db.connect()?, SAMPLE_COUNT).await
        ),
        Err(e) => println!("child open: {e}"),
    }
    Ok(())
}
fn send_sync() {
    fn assert<T: Send + Sync>() {}
    assert::<Connection>();
    assert::<turso::Database>();
    assert::<Statement>();
    println!("Send/Sync: database, connection, statement; current-thread Tokio runtime");
}
/// Typed values round-trip through bindings and RETURNING; the returned
/// statement stays open for the later probes.
async fn typed_values(c: &Connection) -> Probe<Statement> {
    c.execute(
        "CREATE TABLE sample (id INTEGER PRIMARY KEY, t TEXT, b BLOB, n REAL)",
        (),
    )
    .await?;
    let values = vec![
        Value::Integer(1),
        Value::Text("λ; ' text".into()),
        Value::Blob(vec![0, 255]),
        Value::Null,
    ];
    let mut r = c
        .query(
            "INSERT INTO sample VALUES (?1, ?2, ?3, ?4) RETURNING *",
            params_from_iter(values.clone()),
        )
        .await?;
    let row = r.next().await?.unwrap();
    for (i, v) in values.iter().enumerate() {
        assert_eq!(&row.get_value(i)?, v);
    }
    assert!(r.next().await?.is_none());
    drop(r);
    let s = c
        .prepare("WITH x AS (SELECT * FROM sample) SELECT * FROM x WHERE 0")
        .await?;
    assert_eq!(s.column_count(), 4);
    println!("typed values, bindings, RETURNING, empty CTE metadata: passed");
    Ok(s)
}
async fn fts(c: &Connection) -> Probe {
    c.execute(
        "CREATE TABLE fts_probe(id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        (),
    )
    .await?;
    c.execute("CREATE INDEX probe_fts ON fts_probe USING fts(body)", ())
        .await?;
    c.execute("INSERT INTO fts_probe VALUES(1,'native search probe')", ())
        .await?;
    assert_eq!(count(c, FTS_COUNT).await?, 1);
    println!("native FTS + experimental index methods: creation/search passed");
    for pragma in [
        "PRAGMA table_xinfo('sample')",
        "PRAGMA table_info('sample')",
        "PRAGMA foreign_keys",
    ] {
        println!("{pragma}: {:?}", scalar(c, pragma).await);
    }
    Ok(())
}
/// Manual and dropped transactions both leave only the committed row.
async fn transactions(c: &mut Connection) -> Probe {
    c.execute("BEGIN", ()).await?;
    c.execute("INSERT INTO sample(id) VALUES (2)", ()).await?;
    assert!(!c.is_autocommit()?);
    assert!(
        c.execute("INSERT INTO sample(id) VALUES (1)", ())
            .await
            .is_err()
    );
    c.execute("ROLLBACK", ()).await?;
    assert!(c.is_autocommit()?);
    assert_eq!(count(c, SAMPLE_COUNT).await?, 1);
    {
        let tx = c.transaction().await?;
        tx.execute("INSERT INTO sample(id) VALUES (3)", ()).await?;
    }
    println!(
        "dropped transaction before reuse autocommit: {:?}",
        c.is_autocommit()
    );
    assert_eq!(count(c, SAMPLE_COUNT).await?, 1);
    Ok(())
}
async fn contention(c: &Connection, other: &Connection) -> Probe {
    other.busy_timeout(Duration::from_millis(50))?;
    c.execute("BEGIN IMMEDIATE", ()).await?;
    let start = Instant::now();
    println!(
        "contention: {:?} ({:?})",
        other.execute("INSERT INTO sample(id) VALUES (4)", ()).await,
        start.elapsed()
    );
    c.execute("ROLLBACK", ()).await?;
    Ok(())
}
/// Drains a large result and materializes a large blob; the drained rows
/// stay open for the later probes.
async fn volume(c: &Connection) -> Probe<Rows> {
    let start = Instant::now();
    let mut r = c.query("WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<100000) SELECT n FROM x", ()).await?;
    let mut rows = 0;
    while r.next().await?.is_some() {
        rows += 1;
    }
    assert_eq!(rows, 100000);
    println!(
        "100k rows drained: {:?}; no public interrupt API",
        start.elapsed()
    );
    assert_eq!(count(c, "SELECT length(zeroblob(1048576))").await?, 1048576);
    let big = scalar(c, "SELECT zeroblob(1048576)").await?;
    assert!(matches!(big, Value::Blob(b) if b.len() == 1048576));
    println!("1 MiB blob: materialized by engine before application can enforce budget");
    Ok(r)
}
fn two_process_viewers(path: &str) -> Probe {
    for enabled in [false, true] {
        let output = Command::new(std::env::current_exe()?)
            .args(["--child", path, &enabled.to_string()])
            .output()?;
        println!(
            "two-process viewer, multiprocess_wal={enabled}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    Ok(())
}
/// A read-only reopen searches FTS, rejects writes and leaves main/WAL bytes unchanged.
async fn read_only_reopen(path: &Path) -> Probe {
    let p = path.to_str().unwrap();
    let before = std::fs::read(path)?;
    let wal_before = std::fs::read(format!("{p}-wal")).ok();
    let ro = reader(path).build().await?;
    let c = ro.connect()?;
    assert_eq!(count(&c, SAMPLE_COUNT).await?, 1);
    assert!(
        c.execute("INSERT INTO sample(id) VALUES (5)", ())
            .await
            .is_err()
    );
    assert_eq!(count(&c, FTS_COUNT).await?, 1);
    drop(c);
    drop(ro);
    assert_eq!(before, std::fs::read(path)?);
    assert_eq!(wal_before, std::fs::read(format!("{p}-wal")).ok());
    println!("read-only FTS database reopen, write rejection, main/WAL byte preservation: passed");
    Ok(())
}
async fn open_failures(dir: &Path, path: &Path) -> Probe {
    let missing = dir.join("missing.db");
    assert!(reader(&missing).build().await.is_err());
    assert!(!missing.exists());
    let corrupt = dir.join("corrupt.db");
    std::fs::write(&corrupt, vec![0x42; 8192])?;
    println!("corrupt open: {:?}", reader(&corrupt).build().await);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o000))?;
        println!("permission denied: {:?}", reader(path).build().await);
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}
async fn sqlite_fixture(dir: &Path) -> Probe {
    let sqlite = dir.join("sqlite.db");
    let output = Command::new("python3").args(["-c", "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('CREATE TABLE sample(id INTEGER PRIMARY KEY, t TEXT)'); c.execute(\"INSERT INTO sample VALUES (1, 'sqlite fixture')\"); c.commit(); c.close()", sqlite.to_str().unwrap()]).output()?;
    assert!(output.status.success());
    let db = reader(&sqlite).build().await?;
    assert_eq!(count(&db.connect()?, SAMPLE_COUNT).await?, 1);
    println!("Python stdlib SQLite closed-file fixture opened through Turso: passed");
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Probe {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--child") {
        return child(&args[2], &args[3]).await;
    }
    send_sync();
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("probe.db");
    let p = path.to_str().unwrap();
    let db = local(p).build().await?;
    let mut c = db.connect()?;
    c.busy_timeout(Duration::from_millis(50))?;
    let s = typed_values(&c).await?;
    fts(&c).await?;
    transactions(&mut c).await?;
    let other = db.connect()?;
    contention(&c, &other).await?;
    let r = volume(&c).await?;
    // The writer, a prepared statement and drained rows stay open while
    // other processes try to read the same file.
    two_process_viewers(p)?;
    drop(other);
    drop(s);
    drop(r);
    drop(c);
    drop(db);
    read_only_reopen(&path).await?;
    open_failures(dir.path(), &path).await?;
    sqlite_fixture(dir.path()).await?;
    println!(
        "PASS; fault-injected commit/rollback failures and old-engine fixtures remain unvalidated"
    );
    Ok(())
}
