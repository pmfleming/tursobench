//! Disposable release benchmark. No production/user paths are opened.
use std::time::Instant;
fn main() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(run());
}
async fn run() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bench.db");
    let db = turso::Builder::new_local(path.to_str().unwrap())
        .build()
        .await
        .unwrap();
    let c = db.connect().unwrap();
    c.execute(
        "CREATE TABLE t(id INTEGER PRIMARY KEY,name TEXT,payload TEXT)",
        (),
    )
    .await
    .unwrap();
    c.execute("WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000000) INSERT INTO t SELECT x,printf('name-%07d',x),'small' FROM n",()).await.unwrap();
    c.execute("CREATE INDEX by_name ON t(name)", ())
        .await
        .unwrap();
    let legacy = "id COLLATE BINARY,typeof(id),name COLLATE BINARY,typeof(name),payload COLLATE BINARY,typeof(payload)";
    for (name,sql) in [
        ("legacy_first",format!("SELECT * FROM t ORDER BY {legacy} LIMIT 100")),
        ("indexed_first","SELECT * FROM t ORDER BY id LIMIT 100".into()),
        ("legacy_sort","SELECT * FROM t ORDER BY name COLLATE BINARY,typeof(name),id COLLATE BINARY,typeof(id),payload COLLATE BINARY,typeof(payload) LIMIT 100".into()),
        ("indexed_sort","SELECT * FROM t ORDER BY name,id LIMIT 100".into()),
        ("legacy_deep",format!("SELECT * FROM t ORDER BY {legacy} LIMIT 100 OFFSET 500000")),
        ("offset_deep","SELECT * FROM t ORDER BY id LIMIT 100 OFFSET 500000".into()),
        ("seek_deep","SELECT * FROM t WHERE id>500000 ORDER BY id LIMIT 100".into()),
    ]{
        let mut samples=Vec::new();
        for _ in 0..7{let start=Instant::now();let mut rows=c.query(&sql,()).await.unwrap();let mut count=0;while let Some(row)=rows.next().await.unwrap(){std::hint::black_box(row.get_value(0).unwrap());count+=1;}assert_eq!(count,100);samples.push(start.elapsed().as_secs_f64()*1000.0);}
        samples.sort_by(f64::total_cmp);println!("{}",serde_json::json!({"case":name,"rows":1000000,"samples":7,"median_ms":samples[3]}));
    }
}
