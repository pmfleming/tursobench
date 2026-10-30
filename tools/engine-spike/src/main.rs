//! Disposable probes only. Never run this harness against valuable files.
use std::{
    process::Command,
    time::{Duration, Instant},
};
use turso::{Builder, Connection, Value, params_from_iter};

fn local(path: &str) -> Builder {
    Builder::new_local(path).experimental_index_method(true)
}

async fn scalar(c: &Connection, sql: &str) -> turso::Result<Value> {
    let mut r = c.query(sql, ()).await?;
    let value = r.next().await?.unwrap().get_value(0)?;
    while r.next().await?.is_some() {}
    Ok(value)
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.get(1).is_some_and(|a| a == "--child") {
        let db = local(&args[2])
            .read_only(true)
            .experimental_multiprocess_wal(args[3] == "true")
            .build()
            .await;
        match db {
            Ok(db) => println!(
                "child read: {:?}",
                scalar(&db.connect()?, "SELECT count(*) FROM sample").await
            ),
            Err(e) => println!("child open: {e}"),
        }
        return Ok(());
    }
    fn send_sync<T: Send + Sync>() {}
    send_sync::<Connection>();
    send_sync::<turso::Database>();
    send_sync::<turso::Statement>();
    println!("Send/Sync: database, connection, statement; current-thread Tokio runtime");
    let dir = tempfile::tempdir()?;
    let path = dir.path().join("probe.db");
    let p = path.to_str().unwrap();
    let db = local(p).build().await?;
    let mut c = db.connect()?;
    c.busy_timeout(Duration::from_millis(50))?;
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
    c.execute(
        "CREATE TABLE fts_probe(id INTEGER PRIMARY KEY, body TEXT NOT NULL)",
        (),
    )
    .await?;
    c.execute("CREATE INDEX probe_fts ON fts_probe USING fts(body)", ())
        .await?;
    c.execute("INSERT INTO fts_probe VALUES(1,'native search probe')", ())
        .await?;
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM fts_probe WHERE fts_match(body,'native')"
        )
        .await?,
        Value::Integer(1)
    );
    println!("native FTS + experimental index methods: creation/search passed");
    for pragma in [
        "PRAGMA table_xinfo('sample')",
        "PRAGMA table_info('sample')",
        "PRAGMA foreign_keys",
    ] {
        println!("{pragma}: {:?}", scalar(&c, pragma).await);
    }
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
    assert_eq!(
        scalar(&c, "SELECT count(*) FROM sample").await?,
        Value::Integer(1)
    );
    {
        let tx = c.transaction().await?;
        tx.execute("INSERT INTO sample(id) VALUES (3)", ()).await?;
    }
    println!(
        "dropped transaction before reuse autocommit: {:?}",
        c.is_autocommit()
    );
    assert_eq!(
        scalar(&c, "SELECT count(*) FROM sample").await?,
        Value::Integer(1)
    );
    let other = db.connect()?;
    other.busy_timeout(Duration::from_millis(50))?;
    c.execute("BEGIN IMMEDIATE", ()).await?;
    let start = Instant::now();
    println!(
        "contention: {:?} ({:?})",
        other.execute("INSERT INTO sample(id) VALUES (4)", ()).await,
        start.elapsed()
    );
    c.execute("ROLLBACK", ()).await?;
    let start = Instant::now();
    let mut r = c.query("WITH RECURSIVE x(n) AS (VALUES(1) UNION ALL SELECT n+1 FROM x WHERE n<100000) SELECT n FROM x", ()).await?;
    let mut count = 0;
    while r.next().await?.is_some() {
        count += 1;
    }
    assert_eq!(count, 100000);
    println!(
        "100k rows drained: {:?}; no public interrupt API",
        start.elapsed()
    );
    assert_eq!(
        scalar(&c, "SELECT length(zeroblob(1048576))").await?,
        Value::Integer(1048576)
    );
    let big = scalar(&c, "SELECT zeroblob(1048576)").await?;
    assert!(matches!(big, Value::Blob(b) if b.len() == 1048576));
    println!("1 MiB blob: materialized by engine before application can enforce budget");
    for enabled in [false, true] {
        let output = Command::new(std::env::current_exe()?)
            .args(["--child", p, &enabled.to_string()])
            .output()?;
        println!(
            "two-process viewer, multiprocess_wal={enabled}: {}{}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
    }
    drop(other);
    drop(s);
    drop(r);
    drop(c);
    drop(db);
    let before = std::fs::read(&path)?;
    let wal_before = std::fs::read(format!("{p}-wal")).ok();
    let ro = local(p).read_only(true).build().await?;
    let c = ro.connect()?;
    assert_eq!(
        scalar(&c, "SELECT count(*) FROM sample").await?,
        Value::Integer(1)
    );
    assert!(
        c.execute("INSERT INTO sample(id) VALUES (5)", ())
            .await
            .is_err()
    );
    assert_eq!(
        scalar(
            &c,
            "SELECT count(*) FROM fts_probe WHERE fts_match(body,'native')"
        )
        .await?,
        Value::Integer(1)
    );
    drop(c);
    drop(ro);
    assert_eq!(before, std::fs::read(&path)?);
    assert_eq!(wal_before, std::fs::read(format!("{p}-wal")).ok());
    println!("read-only FTS database reopen, write rejection, main/WAL byte preservation: passed");
    let missing = dir.path().join("missing.db");
    assert!(
        local(missing.to_str().unwrap())
            .read_only(true)
            .build()
            .await
            .is_err()
    );
    assert!(!missing.exists());
    let corrupt = dir.path().join("corrupt.db");
    std::fs::write(&corrupt, vec![0x42; 8192])?;
    println!(
        "corrupt open: {:?}",
        local(corrupt.to_str().unwrap())
            .read_only(true)
            .build()
            .await
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000))?;
        println!(
            "permission denied: {:?}",
            local(p).read_only(true).build().await
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    let sqlite = dir.path().join("sqlite.db");
    let output = Command::new("python3").args(["-c", "import sqlite3,sys; c=sqlite3.connect(sys.argv[1]); c.execute('CREATE TABLE sample(id INTEGER PRIMARY KEY, t TEXT)'); c.execute(\"INSERT INTO sample VALUES (1, 'sqlite fixture')\"); c.commit(); c.close()", sqlite.to_str().unwrap()]).output()?;
    assert!(output.status.success());
    let db = local(sqlite.to_str().unwrap())
        .read_only(true)
        .build()
        .await?;
    assert_eq!(
        scalar(&db.connect()?, "SELECT count(*) FROM sample").await?,
        Value::Integer(1)
    );
    println!("Python stdlib SQLite closed-file fixture opened through Turso: passed");
    println!(
        "PASS; fault-injected commit/rollback failures and old-engine fixtures remain unvalidated"
    );
    Ok(())
}
