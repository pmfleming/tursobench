use crate::connection::{failure, from_engine};
use std::path::Path;
use turso::Connection;
use tursobench_core::{Access, Failure, Value};

#[derive(Clone, Copy, Debug)]
pub enum Change {
    EditorBusyTimeout(u64),
    EditorCacheKiB(u32),
}
pub(crate) async fn rows(
    editor: &Connection,
    browse: &Connection,
    path: &Path,
    access: Access,
    change: Option<Change>,
) -> Result<Vec<Vec<Value>>, Failure> {
    if let Some(change) = change {
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
        editor.execute(sql, ()).await.map_err(failure)?;
    }
    let mut rows = vec![];
    if change.is_some() {
        rows.push(vec![
            Value::Text("Change".into()),
            Value::Text("Editor connection".into()),
            Value::Text("Applied; effective settings follow".into()),
        ]);
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
            let result = async {
                let mut values = c
                    .query(format!("PRAGMA {pragma}"), ())
                    .await
                    .map_err(failure)?;
                let row = values
                    .next()
                    .await
                    .map_err(failure)?
                    .ok_or_else(|| Failure::input("Unsupported / no value returned"))?;
                row.get_value(0).map(from_engine).map_err(failure)
            }
            .await;
            rows.push(vec![
                Value::Text((*pragma).into()),
                Value::Text(scope.into()),
                result.unwrap_or_else(|e| Value::Text(format!("Unavailable: {e}"))),
            ]);
        }
    }
    for (label, suffix) in [
        ("Main file bytes", ""),
        ("WAL file bytes", "-wal"),
        ("Coordination file bytes", "-tshm"),
    ] {
        let mut file = path.as_os_str().to_os_string();
        file.push(suffix);
        let value = match std::fs::metadata(Path::new(&file)) {
            Ok(m) => Value::Integer(i64::try_from(m.len()).unwrap_or(i64::MAX)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Text("Not present".into()),
            Err(e) => Value::Text(e.to_string()),
        };
        rows.push(vec![
            Value::Text(label.into()),
            Value::Text("Filesystem snapshot".into()),
            value,
        ]);
    }
    Ok(rows)
}
