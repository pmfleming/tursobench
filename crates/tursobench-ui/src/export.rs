use serde_json::{Value as Json, json};
use std::{
    io::{BufWriter, Write},
    path::Path,
};
use tursobench_core::{Column, Value};

/// New file only, atomic publication. Never reruns SQL or writes the database.
pub fn write_result(
    path: &Path,
    scope: &str,
    columns: &[Column],
    rows: &[Vec<Value>],
) -> Result<(), String> {
    if path.extension().is_none_or(|e| e != "jsonl") {
        return Err("Choose a new file with the .jsonl extension".into());
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    {
        let mut writer = BufWriter::new(&mut temp);
        line(
            &mut writer,
            &json!({"record":"header", "format":"tursobench-typed-jsonl", "version":1, "engine":"turso-0.8.3", "scope":scope, "rows_expected":rows.len(), "columns":columns.iter().map(|c| json!({"name":c.name, "declared_type":c.declared_type})).collect::<Vec<_>>()}),
        )?;
        for row in rows {
            line(
                &mut writer,
                &json!({"record":"row", "values":row.iter().map(encode).collect::<Vec<_>>()}),
            )?;
        }
        line(&mut writer, &json!({"record":"end", "rows":rows.len()}))?;
        writer.flush().map_err(|e| e.to_string())?;
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist_noclobber(path).map_err(|e| e.to_string())?;
    Ok(())
}
fn line(writer: &mut impl Write, value: &Json) -> Result<(), String> {
    serde_json::to_writer(&mut *writer, value).map_err(|e| e.to_string())?;
    writer.write_all(b"\n").map_err(|e| e.to_string())
}
fn encode(value: &Value) -> Json {
    match value {
        Value::Null => json!({"type":"null"}),
        Value::Integer(n) => json!({"type":"integer", "value":n.to_string()}),
        Value::Real(n) => json!({"type":"real", "value":n.to_string()}),
        Value::Text(s) => json!({"type":"text", "value":s}),
        Value::Blob(b) => {
            json!({"type":"blob", "hex":b.iter().map(|b| format!("{b:02x}")).collect::<String>()})
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn typed_stream_scope_end_marker_and_no_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("result.jsonl");
        write_result(
            &path,
            "limited-query-result",
            &[],
            &[vec![
                Value::Integer(i64::MAX),
                Value::Real(f64::INFINITY),
                Value::Null,
                Value::Text(String::new()),
                Value::Blob(vec![0, 255]),
            ]],
        )
        .unwrap();
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<Json> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines[0]["scope"], "limited-query-result");
        assert_eq!(lines[1]["values"][0]["value"], i64::MAX.to_string());
        assert_eq!(lines[1]["values"][1]["value"], "inf");
        assert_eq!(lines[1]["values"][4]["hex"], "00ff");
        assert_eq!(lines[2]["record"], "end");
        assert!(write_result(&path, "table-page", &[], &[]).is_err());
        assert_eq!(std::fs::read_to_string(path).unwrap(), text);
        assert!(write_result(&dir.path().join("database.db"), "table-page", &[], &[]).is_err());
    }
}
