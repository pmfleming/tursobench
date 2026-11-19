use crate::connection::{failure, from_engine};
use std::path::Path;
use turso::Connection;
use tursobench_core::{Access, Failure, Value};

/// Header of the rows `rows` reports.
pub(crate) const COLUMNS: [&str; 3] = ["Setting", "Scope", "Effective value"];
#[derive(Clone, Copy, Debug)]
pub enum Change {
    EditorBusyTimeout(u64),
    EditorCacheKiB(u32),
}
async fn apply(editor: &Connection, access: Access, change: Change) -> Result<(), Failure> {
    if access != Access::ReadWrite {
        return Err(Failure::input(
            "Changing PRAGMAs requires explicit read/write access",
        ));
    }
    if !editor.is_autocommit().map_err(failure)? {
        return Err(Failure::input(
            "Finish the editor transaction before changing settings",
        ));
    }
    let sql = match change {
        Change::EditorBusyTimeout(ms) if ms <= 5000 => format!("PRAGMA busy_timeout={ms}"),
        Change::EditorCacheKiB(kib) if (64..=65536).contains(&kib) => {
            format!("PRAGMA cache_size=-{kib}")
        }
        _ => {
            return Err(Failure::input(
                "Busy timeout must be 0–5000 ms; cache size must be 64–65536 KiB",
            ));
        }
    };
    editor.execute(sql, ()).await.map(|_| ()).map_err(failure)
}
fn row(setting: &str, scope: &str, value: Value) -> Vec<Value> {
    vec![
        Value::Text(setting.into()),
        Value::Text(scope.into()),
        value,
    ]
}
async fn setting(c: &Connection, pragma: &str) -> Value {
    let value = crate::connection::pragma(c, &format!("PRAGMA {pragma}"))
        .await
        .and_then(|v| v.ok_or_else(|| Failure::input("Unsupported / no value returned")));
    value.map_or_else(|e| Value::Text(format!("Unavailable: {e}")), from_engine)
}
fn file_size(path: &Path, suffix: &str) -> Value {
    let mut file = path.as_os_str().to_os_string();
    file.push(suffix);
    match std::fs::metadata(Path::new(&file)) {
        Ok(m) => Value::Integer(i64::try_from(m.len()).unwrap_or(i64::MAX)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Text("Not present".into()),
        Err(e) => Value::Text(e.to_string()),
    }
}
pub(crate) async fn rows(
    editor: &Connection,
    browse: &Connection,
    path: &Path,
    access: Access,
    change: Option<Change>,
) -> Result<Vec<Vec<Value>>, Failure> {
    let mut rows = vec![];
    if let Some(change) = change {
        apply(editor, access, change).await?;
        let applied = Value::Text("Applied; effective settings follow".into());
        rows.push(row("Change", "Editor connection", applied));
    }
    for (scope, c, pragmas) in [
        (
            "Editor connection (cache capacity affects shared pager)",
            editor,
            &[
                "busy_timeout",
                "cache_size",
                "foreign_keys",
                "query_only",
                "synchronous",
            ][..],
        ),
        (
            "Browse connection",
            browse,
            &["busy_timeout", "cache_size", "foreign_keys"][..],
        ),
        (
            "Database",
            browse,
            &[
                "journal_mode",
                "page_size",
                "page_count",
                "freelist_count",
                "schema_version",
            ][..],
        ),
    ] {
        for pragma in pragmas {
            rows.push(row(pragma, scope, setting(c, pragma).await));
        }
    }
    for (label, suffix) in [
        ("Main file bytes", ""),
        ("WAL file bytes", "-wal"),
        ("Coordination file bytes", "-tshm"),
    ] {
        rows.push(row(label, "Filesystem snapshot", file_size(path, suffix)));
    }
    Ok(rows)
}
