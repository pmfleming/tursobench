use serde_json::{Value as Json, json};
use std::{
    io::{BufWriter, Write},
    path::Path,
};
use tursobench_core::{Column, Value, quote_identifier};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Format {
    #[default]
    JsonLines,
    Json,
    Csv,
    Tsv,
    Sql,
}
impl Format {
    pub const ALL: [Self; 5] = [Self::JsonLines, Self::Json, Self::Csv, Self::Tsv, Self::Sql];
    pub fn label(self) -> &'static str {
        match self {
            Self::JsonLines => "Typed JSONL",
            Self::Json => "Typed JSON",
            Self::Csv => "Annotated CSV",
            Self::Tsv => "Annotated TSV",
            Self::Sql => "SQL INSERTs",
        }
    }
    pub fn extension(self) -> &'static str {
        match self {
            Self::JsonLines => "jsonl",
            Self::Json => "json",
            Self::Csv => "csv",
            Self::Tsv => "tsv",
            Self::Sql => "sql",
        }
    }
}
/// Atomic, new-file-only export. Never reruns SQL or writes the inspected database.
pub fn write_result_as(
    path: &Path,
    format: Format,
    scope: &str,
    columns: &[Column],
    rows: &[Vec<Value>],
    table: &str,
) -> Result<(), String> {
    if path.extension().is_none_or(|e| e != format.extension()) {
        return Err(format!("Choose a NEW .{} file", format.extension()));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut writer = BufWriter::new(&mut temp);
        render(&mut writer, format, scope, columns, rows, table)?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist_noclobber(path).map_err(|e| e.to_string())?;
    Ok(())
}
pub fn clipboard(
    format: Format,
    scope: &str,
    columns: &[Column],
    rows: &[Vec<Value>],
    table: &str,
) -> Result<String, String> {
    struct Limited(Vec<u8>);
    impl Write for Limited {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0.len().saturating_add(bytes.len()) > 2 * 1024 * 1024 {
                return Err(std::io::Error::other(
                    "Clipboard exceeds 2 MiB; export to a file instead",
                ));
            }
            self.0.extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut writer = Limited(Vec::new());
    render(&mut writer, format, scope, columns, rows, table)?;
    String::from_utf8(writer.0).map_err(|e| e.to_string())
}
fn header(scope: &str, columns: &[Column], count: usize) -> Json {
    json!({"record":"header", "format":"tursobench-typed-jsonl", "version":1, "engine":"turso-0.8.3", "scope":scope, "rows_expected":count, "columns":columns.iter().map(|c| json!({"name":c.name,"declared_type":c.declared_type})).collect::<Vec<_>>()})
}
fn render(
    w: &mut impl Write,
    format: Format,
    scope: &str,
    columns: &[Column],
    rows: &[Vec<Value>],
    table: &str,
) -> Result<(), String> {
    if rows.iter().any(|r| r.len() != columns.len()) {
        return Err("Result column/row widths do not match".into());
    }
    let error = |e: std::io::Error| e.to_string();
    match format {
        Format::JsonLines => {
            line(w, &header(scope, columns, rows.len()))?;
            for row in rows {
                line(
                    w,
                    &json!({"record":"row", "values":row.iter().map(encode).collect::<Vec<_>>()}),
                )?;
            }
            line(w, &json!({"record":"end", "rows":rows.len()}))?;
        }
        Format::Json => {
            w.write_all(b"{\"header\":").map_err(error)?;
            let mut h = header(scope, columns, rows.len());
            h["format"] = json!("tursobench-typed-json");
            serde_json::to_writer(&mut *w, &h).map_err(|e| e.to_string())?;
            w.write_all(b",\"rows\":[").map_err(error)?;
            for (i, row) in rows.iter().enumerate() {
                if i > 0 {
                    w.write_all(b",").map_err(error)?;
                }
                serde_json::to_writer(&mut *w, &row.iter().map(encode).collect::<Vec<_>>())
                    .map_err(|e| e.to_string())?;
            }
            writeln!(w, "],\"end\":{{\"rows\":{}}}}}", rows.len()).map_err(error)?;
        }
        Format::Csv | Format::Tsv => {
            let delimiter = if format == Format::Csv { ',' } else { '\t' };
            writeln!(w, "# TursoBench scope={} rows={}; NULL=\\N, blobs=X'hex', text formula-safe; skip this metadata line", scope.replace(['\r','\n'], " "), rows.len()).map_err(error)?;
            for (i, col) in columns.iter().enumerate() {
                if i > 0 {
                    write!(w, "{delimiter}").map_err(error)?;
                }
                csv_field(w, &spreadsheet_text(&col.name))?;
            }
            w.write_all(b"\r\n").map_err(error)?;
            for row in rows {
                for (i, v) in row.iter().enumerate() {
                    if i > 0 {
                        write!(w, "{delimiter}").map_err(error)?;
                    }
                    match v {
                        Value::Null => w.write_all(b"\\N").map_err(error)?,
                        Value::Text(s) => csv_field(w, &spreadsheet_text(s))?,
                        _ => csv_field(w, &cell_text(v))?,
                    }
                }
                w.write_all(b"\r\n").map_err(error)?;
            }
        }
        Format::Sql => {
            if table.is_empty() || table.len() > 1024 || columns.is_empty() {
                return Err("SQL export requires a target table and named columns".into());
            }
            let unique: std::collections::BTreeSet<_> =
                columns.iter().map(|c| c.name.to_lowercase()).collect();
            if unique.len() != columns.len() {
                return Err(
                    "Duplicate result column names cannot form an unambiguous INSERT".into(),
                );
            }
            writeln!(
                w,
                "-- TursoBench scope={} rows={}; target schema must already exist",
                scope.replace(['\r', '\n'], " "),
                rows.len()
            )
            .map_err(error)?;
            for row in rows {
                write!(
                    w,
                    "INSERT INTO {} ({}) VALUES (",
                    quote_identifier(table),
                    columns
                        .iter()
                        .map(|c| quote_identifier(&c.name))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
                .map_err(error)?;
                for (i, value) in row.iter().enumerate() {
                    if i > 0 {
                        w.write_all(b", ").map_err(error)?;
                    }
                    w.write_all(sql_literal(value)?.as_bytes()).map_err(error)?;
                }
                w.write_all(b");\n").map_err(error)?;
            }
        }
    }
    Ok(())
}
fn csv_field(w: &mut impl Write, value: &str) -> Result<(), String> {
    write!(w, "\"{}\"", value.replace('"', "\"\"")).map_err(|e| e.to_string())
}
fn spreadsheet_text(value: &str) -> String {
    if value.trim_start().starts_with(['=', '+', '-', '@']) {
        format!("'{value}")
    } else {
        value.into()
    }
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
pub fn cell_text(value: &Value) -> String {
    match value {
        Value::Null => "NULL".into(),
        Value::Integer(n) => n.to_string(),
        Value::Real(n) => n.to_string(),
        Value::Text(s) => s.clone(),
        Value::Blob(b) => format!("X'{}'", hex(b)),
    }
}
fn sql_literal(value: &Value) -> Result<String, String> {
    Ok(match value {
        Value::Null => "NULL".into(),
        Value::Integer(n) => n.to_string(),
        Value::Real(n) if n.is_finite() => format!("{n:?}"),
        Value::Real(_) => {
            return Err("Non-finite reals require typed JSON/JSONL, not SQL export".into());
        }
        Value::Text(s) if s.contains('\0') => format!("CAST(X'{}' AS TEXT)", hex(s.as_bytes())),
        Value::Text(s) => format!("'{}'", s.replace('\'', "''")),
        Value::Blob(b) => format!("X'{}'", hex(b)),
    })
}
fn line(writer: &mut impl Write, value: &Json) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())
}
fn encode(value: &Value) -> Json {
    match value {
        Value::Null => json!({"type":"null"}),
        Value::Integer(n) => json!({"type":"integer","value":n.to_string()}),
        Value::Real(n) => json!({"type":"real","value":n.to_string()}),
        Value::Text(s) => json!({"type":"text","value":s}),
        Value::Blob(b) => json!({"type":"blob","hex":hex(b)}),
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn columns(n: usize) -> Vec<Column> {
        (0..n)
            .map(|i| Column {
                name: format!("col{i}"),
                declared_type: None,
            })
            .collect()
    }
    #[test]
    fn formats_preserve_types_scope_and_atomic_publication() {
        let dir = tempfile::tempdir().unwrap();
        let rows = vec![vec![
            Value::Text("001".into()),
            Value::Null,
            Value::Blob(vec![0, 255]),
            Value::Real(1.0),
            Value::Text("a'\0b".into()),
        ]];
        let cols = columns(5);
        for format in Format::ALL {
            let path = dir.path().join(format!("result.{}", format.extension()));
            let text = clipboard(format, "table-page", &cols, &rows, "t\"x").unwrap();
            write_result_as(&path, format, "table-page", &cols, &rows, "t\"x").unwrap();
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert!(write_result_as(&path, format, "other", &[], &[], "t").is_err());
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
            assert!(text.contains("table-page"));
            match format {
                Format::JsonLines => {
                    let lines: Vec<Json> = text
                        .lines()
                        .map(|l| serde_json::from_str(l).unwrap())
                        .collect();
                    assert_eq!(lines[0]["scope"], "table-page");
                    assert_eq!(lines[1]["values"][0]["value"], "001");
                    assert_eq!(lines[1]["values"][2]["hex"], "00ff");
                    assert_eq!(lines[2], json!({"record":"end", "rows":1}));
                }
                Format::Json => {
                    let data: Json = serde_json::from_str(&text).unwrap();
                    assert_eq!(data["header"]["scope"], "table-page");
                    assert_eq!(data["rows"][0][0]["type"], "text");
                }
                Format::Sql => {
                    assert!(text.contains("\"t\"\"x\""));
                    assert!(text.contains("'001', NULL, X'00ff', 1.0, CAST(X'61270062' AS TEXT)"));
                }
                Format::Csv | Format::Tsv => {
                    let escaped = clipboard(
                        format,
                        "limited-query-result",
                        &columns(3),
                        &[vec![
                            Value::Text("=1+1".into()),
                            Value::Text("a,\"b\n".into()),
                            Value::Null,
                        ]],
                        "",
                    )
                    .unwrap();
                    assert!(escaped.contains("limited-query-result"));
                    assert!(escaped.contains("\"'=1+1\""));
                    assert!(escaped.contains("a,\"\"b\n"));
                    assert!(escaped.contains("\\N"));
                }
            }
        }
        for format in [Format::Json, Format::JsonLines] {
            let text = clipboard(
                format,
                "limited-query-result",
                &columns(2),
                &[vec![Value::Integer(i64::MAX), Value::Real(f64::INFINITY)]],
                "",
            )
            .unwrap();
            assert!(text.contains(&i64::MAX.to_string()));
            assert!(text.contains("inf"));
        }
        assert!(
            write_result_as(
                &dir.path().join("database.db"),
                Format::JsonLines,
                "table-page",
                &[],
                &[],
                "t"
            )
            .is_err()
        );
    }
    #[test]
    fn sql_export_round_trips_through_turso_with_quoted_names_and_nul_text() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("restore.db");
        crate::test_support::run(async {
            let db = turso::Builder::new_local(path.to_str().unwrap())
                .build()
                .await
                .unwrap();
            let c = db.connect().unwrap();
            c.execute("CREATE TABLE \"t\"\"x\"(col0 INTEGER PRIMARY KEY,col1 REAL,col2 TEXT,col3 BLOB,col4 TEXT) STRICT",()).await.unwrap();
            let expected = vec![
                vec![
                    Value::Integer(i64::MIN),
                    Value::Real(1.0),
                    Value::Text("001".into()),
                    Value::Blob(vec![]),
                    Value::Null,
                ],
                vec![
                    Value::Integer(i64::MAX),
                    Value::Real(-0.0),
                    Value::Text("a'\0λ\n".into()),
                    Value::Blob(vec![0, 255]),
                    Value::Null,
                ],
            ];
            for row in &expected {
                let sql = clipboard(
                    Format::Sql,
                    "table-page",
                    &columns(5),
                    std::slice::from_ref(row),
                    "t\"x",
                )
                .unwrap();
                c.execute(sql, ()).await.unwrap();
            }
            let mut rows = c
                .query("SELECT * FROM \"t\"\"x\" ORDER BY col0", ())
                .await
                .unwrap();
            for expected in expected {
                let row = rows.next().await.unwrap().unwrap();
                for (i, value) in expected.into_iter().enumerate() {
                    let value = match value {
                        Value::Null => turso::Value::Null,
                        Value::Integer(n) => turso::Value::Integer(n),
                        Value::Real(n) => turso::Value::Real(n),
                        Value::Text(s) => turso::Value::Text(s),
                        Value::Blob(b) => turso::Value::Blob(b),
                    };
                    assert_eq!(row.get_value(i).unwrap(), value);
                }
            }
            assert!(rows.next().await.unwrap().is_none());
        });
    }
    #[test]
    fn clipboard_budget_and_failed_export_do_not_publish() {
        assert!(
            clipboard(
                Format::Json,
                "test",
                &columns(1),
                &[vec![Value::Text("x".repeat(3 * 1024 * 1024))]],
                ""
            )
            .is_err()
        );
        let dir = tempfile::tempdir().unwrap();
        let mut duplicates = columns(2);
        duplicates[1].name = duplicates[0].name.to_uppercase();
        for (cols, rows, table) in [
            (columns(1), vec![vec![Value::Real(f64::NAN)]], "t"),
            (columns(1), vec![vec![]], "t"),
            (columns(1), vec![], ""),
            (vec![], vec![], "t"),
            (duplicates, vec![], "t"),
        ] {
            let path = dir.path().join("bad.sql");
            assert!(write_result_as(&path, Format::Sql, "test", &cols, &rows, table).is_err());
            assert!(!path.exists());
            assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 0);
        }
    }
}
