use crate::common::{self, Client};
use std::path::Path;
use tursobench_core::{
    Access, ImportFormat, ImportPreview, ImportSpec, InputKind, TransferOutcome, Value,
};
use tursobench_db::Operation;

/// Previews `path`, then imports it into `table`, mapping its source columns
/// to `mapping` in order, or by name when `mapping` is empty.
fn import(
    c: &mut Client,
    path: &Path,
    format: ImportFormat,
    kinds: &[InputKind],
    (table, mapping): (&str, &[&str]),
    atomic: bool,
) -> (ImportPreview, TransferOutcome) {
    let preview = c.run(Operation::PreviewImport {
        path: path.into(),
        format,
        kinds: kinds.to_vec(),
        null_marker: "\\N".into(),
    });
    let preview = preview.preview.expect("import preview");
    let source_columns = preview.columns.clone();
    let mapping = match mapping {
        [] => source_columns.clone(),
        names => names.iter().map(|&name| name.into()).collect(),
    };
    let spec = ImportSpec {
        path: path.into(),
        table: table.into(),
        format,
        source_columns,
        mapping,
        kinds: kinds.to_vec(),
        null_marker: "\\N".into(),
        atomic,
    };
    (preview, c.run(Operation::Import(spec)).transfer.unwrap())
}

#[test]
fn complete_export_import_and_corrupt_input_transactions_are_truthful() {
    let (dir, path) = common::database(
        "source.db",
        &[
            "CREATE TABLE source(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE TABLE target(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE TABLE invalid(id INTEGER PRIMARY KEY,value TEXT)",
            "CREATE TABLE partial(id INTEGER PRIMARY KEY,value TEXT)",
            "WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1205) INSERT INTO source SELECT x,CASE WHEN x=50 THEN hex(zeroblob(50000)) ELSE 'small' END FROM n",
        ],
    );
    let mut c = Client::open(&path, Access::ReadWrite);
    let output = dir.path().join("all.jsonl");
    let export = c
        .run(Operation::ExportTable {
            path: output.clone(),
            spec: common::browse("source"),
        })
        .transfer
        .unwrap();
    assert!(export.error.is_none(), "{}", export.message);
    assert_eq!(export.rows, 1205);
    let jsonl = |c: &mut Client, path: &Path, table: &str, atomic: bool| {
        import(c, path, ImportFormat::JsonLines, &[], (table, &[]), atomic).1
    };
    let imported = jsonl(&mut c, &output, "target", true);
    assert!(imported.error.is_none(), "{}", imported.message);
    assert_eq!(imported.rows, 1205);
    assert_eq!(
        c.sql("SELECT length(value) FROM target WHERE id=50").rows,
        vec![vec![Value::Integer(100000)]]
    );
    let corrupt = dir.path().join("incomplete.jsonl");
    let data = std::fs::read_to_string(&output).unwrap();
    std::fs::write(
        &corrupt,
        data.lines().take(1206).collect::<Vec<_>>().join("\n"),
    )
    .unwrap();
    let atomic = jsonl(&mut c, &corrupt, "invalid", true);
    assert!(atomic.error.is_some());
    assert_eq!(atomic.rows, 0);
    assert_eq!(c.count("invalid"), 0);
    let partial = jsonl(&mut c, &corrupt, "partial", false);
    assert!(partial.error.is_some());
    assert_eq!(partial.rows, 1000);
    assert_eq!(c.count("partial"), 1000);
    c.close();
}

#[test]
fn csv_and_typed_json_imports_preserve_quoted_multiline_text_null_and_blobs() {
    let (dir, path) = common::database(
        "csv.db",
        &["CREATE TABLE target(id INTEGER PRIMARY KEY,value TEXT,note TEXT,data BLOB)"],
    );
    let input = dir.path().join("input.csv");
    std::fs::write(
        &input,
        "id,label,note,payload\r\n1,\"a,\"\"quote\"\"\r\nb\",\\N,X'00ff'\r\n2,001,\"\\N\",X''\r\n",
    )
    .unwrap();
    let mut c = Client::open(&path, Access::ReadWrite);
    let kinds = [
        InputKind::Integer,
        InputKind::Text,
        InputKind::Text,
        InputKind::Blob,
    ];
    let target = ("target", &["id", "value", "note", "data"][..]);
    let (preview, imported) = import(&mut c, &input, ImportFormat::Csv, &kinds, target, true);
    assert_eq!(preview.rows.len(), 2);
    assert!(imported.error.is_none(), "{}", imported.message);
    assert_eq!(
        c.sql("SELECT * FROM target ORDER BY id").rows,
        vec![
            vec![
                Value::Integer(1),
                Value::Text("a,\"quote\"\r\nb".into()),
                Value::Null,
                Value::Blob(vec![0, 255])
            ],
            vec![
                Value::Integer(2),
                Value::Text("001".into()),
                Value::Text("\\N".into()),
                Value::Blob(vec![])
            ]
        ]
    );
    let json = dir.path().join("input.json");
    let typed = |end: usize| {
        let row = r#"[{"type":"integer","value":"3"},{"type":"text","value":"json"},{"type":"null"},{"type":"blob","hex":"01"}]"#;
        let columns = r#"[{"name":"id"},{"name":"value"},{"name":"note"},{"name":"data"}]"#;
        format!(
            r#"{{"header":{{"format":"tursobench-typed-json","version":1,"columns":{columns}}},"rows":[{row}],"end":{{"rows":{end}}}}}"#
        )
    };
    std::fs::write(&json, typed(2)).unwrap();
    assert!(c.refuses(Operation::PreviewImport {
        path: json.clone(),
        format: ImportFormat::Json,
        kinds: vec![],
        null_marker: String::new(),
    }));
    std::fs::write(&json, typed(1)).unwrap();
    let (preview, imported) = import(
        &mut c,
        &json,
        ImportFormat::Json,
        &[],
        ("target", &[]),
        true,
    );
    assert_eq!(preview.rows.len(), 1);
    assert!(imported.error.is_none(), "{}", imported.message);
    assert_eq!(
        c.sql("SELECT * FROM target WHERE id=3").rows,
        vec![vec![
            Value::Integer(3),
            Value::Text("json".into()),
            Value::Null,
            Value::Blob(vec![1])
        ]]
    );
    c.close();
}

#[test]
fn execute_export_runs_a_write_once_and_backup_contains_committed_wal() {
    let (dir, path) = common::database(
        "source.db",
        &["CREATE TABLE t(id INTEGER PRIMARY KEY,value TEXT)"],
    );
    let mut c = Client::open(&path, Access::ReadWrite);
    let output = dir.path().join("returning.jsonl");
    let operation = || Operation::ExportQuery {
        path: output.clone(),
        sql: "INSERT INTO t(value) VALUES('once') RETURNING id".into(),
        parameters: vec![].into(),
    };
    let export = c.run(operation()).transfer.unwrap();
    assert!(export.error.is_none(), "{}", export.message);
    assert!(c.run(operation()).failure.is_some());
    assert_eq!(c.count("t"), 1);
    let checkpoint = c.run(Operation::Checkpoint).transfer.unwrap();
    assert!(checkpoint.error.is_none());
    c.sql("INSERT INTO t(value) VALUES('recent WAL row')");
    let backup = dir.path().join("backup.db");
    let result = c
        .run(Operation::BackupAndClose(backup.clone()))
        .transfer
        .unwrap();
    assert!(result.error.is_none(), "{}", result.message);
    c.close();
    let mut restored = Client::open(&backup, Access::ReadOnly);
    assert_eq!(restored.count("t"), 2);
    assert_eq!(
        restored.sql("SELECT value FROM t WHERE id=2").rows,
        vec![vec![Value::Text("recent WAL row".into())]]
    );
    restored.close();
}

#[test]
fn sql_real_literals_are_checked_against_actual_engine_roundtrips() {
    let (_dir, path) = common::database(
        "reals.db",
        &["CREATE TABLE t(id INTEGER PRIMARY KEY,value DECIMAL(10,2))"],
    );
    let mut c = Client::open(&path, Access::ReadOnly);
    let table = c.inspect("t").table;
    assert!(table.columns[1].declared_type.contains("10"));
    assert!(table.columns[1].declared_type.contains('2'));
    let mut seed = 0x123456789abcdef0u64;
    let mut verified = 0;
    for _ in 0..512 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let n = f64::from_bits(seed);
        if !n.is_finite() {
            continue;
        }
        match tursobench_db::sql::checked_real_literal(n) {
            Ok(literal) => {
                let result = c.sql(&format!("SELECT {literal}"));
                let Value::Real(actual) = result.rows[0][0] else {
                    panic!("real type lost")
                };
                assert_eq!(actual.to_bits(), n.to_bits());
                verified += 1;
            }
            Err(e) => assert!(e.message.contains("typed JSON")),
        }
    }
    assert!(verified > 100);
    c.close();
}
