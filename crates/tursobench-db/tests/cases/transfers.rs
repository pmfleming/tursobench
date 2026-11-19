use crate::common::{self, Client};
use tursobench_core::{Access, ImportFormat, ImportSpec, InputKind, TransferOutcome, Value};
use tursobench_db::Operation;

/// Imports typed JSONL into `table`, mapping every source column by name.
fn import(
    c: &mut Client,
    path: &std::path::Path,
    table: &str,
    columns: &[String],
    atomic: bool,
) -> TransferOutcome {
    let spec = ImportSpec {
        path: path.into(),
        table: table.into(),
        format: ImportFormat::JsonLines,
        source_columns: columns.to_vec(),
        mapping: columns.to_vec(),
        kinds: vec![],
        null_marker: "\\N".into(),
        atomic,
    };
    c.run(Operation::Import(spec)).transfer.unwrap()
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
    let preview = c
        .run(Operation::PreviewImport {
            path: output.clone(),
            format: ImportFormat::JsonLines,
            kinds: vec![],
            null_marker: "\\N".into(),
        })
        .preview
        .unwrap();
    let imported = import(&mut c, &output, "target", &preview.columns, true);
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
    let atomic = import(&mut c, &corrupt, "invalid", &preview.columns, true);
    assert!(atomic.error.is_some());
    assert_eq!(atomic.rows, 0);
    assert_eq!(c.count("invalid"), 0);
    let partial = import(&mut c, &corrupt, "partial", &preview.columns, false);
    assert!(partial.error.is_some());
    assert_eq!(partial.rows, 1000);
    assert_eq!(c.count("partial"), 1000);
    c.close();
}

#[test]
fn csv_mapping_preserves_quoted_multiline_text_null_and_blobs() {
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
    let kinds = vec![
        InputKind::Integer,
        InputKind::Text,
        InputKind::Text,
        InputKind::Blob,
    ];
    let preview = c
        .run(Operation::PreviewImport {
            path: input.clone(),
            format: ImportFormat::Csv,
            kinds: kinds.clone(),
            null_marker: "\\N".into(),
        })
        .preview
        .unwrap();
    assert_eq!(preview.rows.len(), 2);
    let imported = c
        .run(Operation::Import(ImportSpec {
            path: input,
            table: "target".into(),
            format: ImportFormat::Csv,
            source_columns: preview.columns,
            mapping: vec!["id".into(), "value".into(), "note".into(), "data".into()],
            kinds,
            null_marker: "\\N".into(),
            atomic: true,
        }))
        .transfer
        .unwrap();
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
    let table = c.run(Operation::Inspect("t".into())).info.unwrap().table;
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
