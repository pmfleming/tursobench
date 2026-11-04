//! Streaming interchange; destinations publish only after complete success.
use crate::{
    connection::{failure, from_engine, state, to_engine},
    sql,
};
use serde_json::{Value as Json, json};
use std::{
    fs::File,
    io::{BufRead, BufWriter, Read, Write},
    path::Path,
};
use turso::Connection;
use tursobench_core::{Bindings, Failure, TransactionState, TransferOutcome, Value};

fn io(e: impl std::fmt::Display) -> Failure {
    Failure {
        kind: tursobench_core::ErrorKind::Io,
        message: e.to_string(),
    }
}
pub(crate) fn line(w: &mut impl Write, value: &Json) -> Result<(), Failure> {
    serde_json::to_writer(&mut *w, value).map_err(io)?;
    w.write_all(b"\n").map_err(io)
}
fn cell(w: &mut impl Write, value: Value) -> Result<(), Failure> {
    let data = match value {
        Value::Null => json!({"type":"null"}),
        Value::Integer(n) => json!({"type":"integer","value":n.to_string()}),
        Value::Real(n) => json!({"type":"real","value":n.to_string()}),
        Value::Text(s) => {
            w.write_all(b"{\"type\":\"text\",\"value\":").map_err(io)?;
            serde_json::to_writer(&mut *w, &s).map_err(io)?;
            return w.write_all(b"}").map_err(io);
        }
        Value::Blob(bytes) => {
            w.write_all(b"{\"type\":\"blob\",\"hex\":\"").map_err(io)?;
            let digits = b"0123456789abcdef";
            let mut out = [0u8; 8192];
            for chunk in bytes.chunks(4096) {
                for (i, b) in chunk.iter().enumerate() {
                    out[i * 2] = digits[(b >> 4) as usize];
                    out[i * 2 + 1] = digits[(b & 15) as usize];
                }
                w.write_all(&out[..chunk.len() * 2]).map_err(io)?;
            }
            return w.write_all(b"\"}").map_err(io);
        }
        Value::Deferred(_) => {
            return Err(Failure::input(
                "Source exports require original engine values",
            ));
        }
    };
    serde_json::to_writer(w, &data).map_err(io)
}
pub(crate) async fn export(
    c: &Connection,
    path: &Path,
    text: &str,
    parameters: Bindings,
    scope: &str,
    cancel: impl Fn() -> bool,
) -> Result<TransferOutcome, Failure> {
    sql::validate_single(text)?;
    sql::validate_bindings(text, &parameters)?;
    if parameters.bytes() > 8 * 1024 * 1024 {
        return Err(Failure::input("Parameters exceed 8 MiB"));
    }
    if path.extension().is_none_or(|e| e != "jsonl") {
        return Err(Failure::input(
            "Choose a new .jsonl destination; SQL has not executed",
        ));
    }
    if std::fs::symlink_metadata(path).is_ok() {
        return Err(Failure::input("Destination exists; SQL has not executed"));
    }
    let parent = path
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    let mut writer = BufWriter::new(&mut temp);
    let mut outcome = stream(c, text, parameters, scope, &mut writer, cancel).await?;
    drop(writer);
    if outcome.error.is_none() {
        let result = (|| {
            temp.as_file().sync_all().map_err(io)?;
            temp.persist_noclobber(path).map_err(io)?;
            Ok::<_, Failure>(())
        })();
        if let Err(e) = result {
            outcome.error = Some(e);
        }
    }
    outcome.message = if let Some(e) = &outcome.error {
        format!(
            "Export not published. Statement finished: {}; transaction {:?}. {e}",
            outcome.executed, outcome.transaction
        )
    } else {
        format!(
            "Exported {} complete rows to {}",
            outcome.rows,
            path.display()
        )
    };
    Ok(outcome)
}

async fn stream(
    c: &Connection,
    text: &str,
    parameters: Bindings,
    scope: &str,
    mut writer: &mut impl Write,
    cancel: impl Fn() -> bool,
) -> Result<TransferOutcome, Failure> {
    let mut stmt = c.prepare(text).await.map_err(failure)?;
    let columns = stmt.columns();
    if columns.len() > 512
        || columns
            .iter()
            .map(|c| c.name().len() + c.decl_type().map_or(0, str::len))
            .sum::<usize>()
            > 1024 * 1024
    {
        return Err(Failure::input(
            "Export metadata exceeds budget; SQL has not executed",
        ));
    }
    let params = match parameters {
        Bindings::Positional(v) => turso::params::Params::Positional(
            v.into_iter().map(to_engine).collect::<Result<_, _>>()?,
        ),
        Bindings::Named(v) => turso::params::Params::Named(
            v.into_iter()
                .map(|(k, v)| to_engine(v).map(|v| (k.into(), v)))
                .collect::<Result<_, _>>()?,
        ),
    };
    let mut outcome = TransferOutcome {
        rows: 0,
        changed: 0,
        transaction: state(c),
        error: None,
        message: String::new(),
        executed: false,
    };
    line(
        &mut writer,
        &json!({"record":"header","format":"tursobench-typed-jsonl","version":2,"engine":"turso-0.8.3","scope":scope,"rows_expected":null,"columns":columns.iter().map(|c|json!({"name":c.name(),"declared_type":c.decl_type()})).collect::<Vec<_>>()}),
    )?;
    let mut rows = stmt.query(params).await.map_err(failure)?;
    loop {
        match rows.next().await {
            Ok(Some(row)) => {
                outcome.rows += 1;
                if cancel() && outcome.error.is_none() {
                    outcome.error = Some(Failure::input(
                        "Export cancelled; preceding committed SQL is not undone",
                    ));
                }
                if outcome.error.is_none() {
                    let result = (|| {
                        writer
                            .write_all(b"{\"record\":\"row\",\"values\":[")
                            .map_err(io)?;
                        for i in 0..rows.column_count() {
                            if i > 0 {
                                writer.write_all(b",").map_err(io)?;
                            }
                            cell(&mut writer, from_engine(row.get_value(i).map_err(failure)?))?;
                        }
                        writer.write_all(b"]}\n").map_err(io)
                    })();
                    if let Err(e) = result {
                        outcome.error = Some(e);
                    }
                }
                if outcome.error.is_some() && sql::stoppable_read(text) {
                    break;
                }
            }
            Ok(None) => {
                outcome.executed = true;
                outcome.changed = stmt.n_change();
                break;
            }
            Err(e) => {
                outcome.error = Some(failure(e));
                break;
            }
        }
    }
    drop(rows);
    outcome.transaction = state(c);
    if outcome.error.is_none() {
        let result = (|| {
            line(&mut writer, &json!({"record":"end","rows":outcome.rows}))?;
            writer.flush().map_err(io)
        })();
        if let Err(e) = result {
            outcome.error = Some(e);
        }
    }
    Ok(outcome)
}

pub(crate) async fn checkpoint(c: &Connection) -> Result<(), Failure> {
    if state(c) != TransactionState::Autocommit {
        return Err(Failure::input(
            "Finish the editor transaction before checkpointing",
        ));
    }
    let mut rows = c
        .query("PRAGMA wal_checkpoint(TRUNCATE)", ())
        .await
        .map_err(failure)?;
    let row = rows.next().await.map_err(failure)?.ok_or_else(|| {
        Failure::input("Checkpoint returned no status; capability is unavailable")
    })?;
    let busy = row.get_value(0).map_err(failure)?;
    if busy != turso::Value::Integer(0) {
        return Err(Failure::input(
            "Checkpoint could not finish; backup is unavailable",
        ));
    }
    while rows.next().await.map_err(failure)?.is_some() {}
    Ok(())
}
pub(crate) async fn copy_closed(
    source: &Path,
    destination: &Path,
    settings: tursobench_core::ConnectionSettings,
) -> Result<(), Failure> {
    let wal = source
        .as_os_str()
        .to_os_string()
        .into_string()
        .map_err(|_| Failure::input("Non-UTF8 source path"))?
        + "-wal";
    if Path::new(&wal).try_exists().map_err(io)? && std::fs::metadata(wal).map_err(io)?.len() > 0 {
        return Err(Failure::input(
            "Closed source still has WAL data; refusing a main-file-only backup",
        ));
    }
    let parent = destination
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(io)?;
    std::io::copy(&mut File::open(source).map_err(io)?, &mut temp).map_err(io)?;
    temp.as_file().sync_all().map_err(io)?;
    {
        let db = turso::Builder::new_local(
            temp.path()
                .to_str()
                .ok_or_else(|| Failure::input("Non-UTF8 backup path"))?,
        )
        .experimental_index_method(true)
        .read_only(true)
        .build()
        .await
        .map_err(failure)?;
        let c = db.connect().map_err(failure)?;
        crate::connection::configure(&c, settings).await?;
        let mut rows = c
            .query("PRAGMA integrity_check", ())
            .await
            .map_err(failure)?;
        let mut count = 0;
        while let Some(row) = rows.next().await.map_err(failure)? {
            if row.get_value(0).map_err(failure)? != turso::Value::Text("ok".into()) {
                return Err(Failure::input("Backup failed integrity verification"));
            }
            count += 1;
        }
        if count != 1 {
            return Err(Failure::input(
                "Backup integrity check returned an unexpected result",
            ));
        }
    }
    temp.persist_noclobber(destination).map_err(io)?;
    Ok(())
}

pub(crate) fn bounded_line(reader: &mut impl BufRead) -> Result<Option<Vec<u8>>, Failure> {
    let mut line = Vec::new();
    reader
        .take(16 * 1024 * 1024 + 1)
        .read_until(b'\n', &mut line)
        .map_err(io)?;
    if line.len() > 16 * 1024 * 1024 {
        return Err(Failure::input("Import record exceeds 16 MiB"));
    }
    Ok((!line.is_empty()).then_some(line))
}
pub(crate) fn decode(value: &Json) -> Result<Value, Failure> {
    let text = || {
        value
            .get("value")
            .and_then(Json::as_str)
            .ok_or_else(|| Failure::input("Typed value requires a string"))
    };
    match value.get("type").and_then(Json::as_str) {
        Some("null") => Ok(Value::Null),
        Some("integer") => text()?.parse().map(Value::Integer).map_err(io),
        Some("real") => text()?.parse::<f64>().map_err(io).and_then(|n| {
            if n.is_nan() {
                Err(Failure::input("NaN is not a storable SQL real"))
            } else {
                Ok(Value::Real(n))
            }
        }),
        Some("text") => Ok(Value::Text(text()?.into())),
        Some("blob") => {
            let h = value
                .get("hex")
                .and_then(Json::as_str)
                .ok_or_else(|| Failure::input("Blob requires hexadecimal text"))?;
            if !h.len().is_multiple_of(2) || !h.is_ascii() {
                return Err(Failure::input("Invalid blob hex"));
            }
            Ok(Value::Blob(
                (0..h.len())
                    .step_by(2)
                    .map(|i| u8::from_str_radix(&h[i..i + 2], 16).map_err(io))
                    .collect::<Result<_, _>>()?,
            ))
        }
        _ => Err(Failure::input(
            "Unknown typed cell; deferred values cannot be imported",
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct FullDisk(usize);
    impl Write for FullDisk {
        fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
            if self.0 == 0 {
                return Err(std::io::Error::other("injected disk full"));
            }
            let count = self.0.min(bytes.len());
            self.0 -= count;
            Ok(count)
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    #[test]
    fn output_failure_drains_returning_and_header_failure_does_not_execute() {
        tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async{
            let dir=tempfile::tempdir().unwrap();let db=turso::Builder::new_local(dir.path().join("full.db").to_str().unwrap()).build().await.unwrap();let c=db.connect().unwrap();
            c.execute("CREATE TABLE t(id INTEGER PRIMARY KEY)",()).await.unwrap();
            let sql="WITH RECURSIVE n(x) AS (VALUES(1) UNION ALL SELECT x+1 FROM n WHERE x<1000) INSERT INTO t SELECT x FROM n RETURNING id";
            let outcome=stream(&c,sql,vec![].into(),"complete-query-result",&mut FullDisk(512),||false).await.unwrap();
            assert!(outcome.error.is_some());assert!(outcome.executed);assert_eq!(outcome.changed,1000);assert_eq!(outcome.transaction,TransactionState::Autocommit);
            assert!(stream(&c,"INSERT INTO t VALUES(1001)",vec![].into(),"complete-query-result",&mut FullDisk(0),||false).await.is_err());
            let mut rows=c.query("SELECT count(*) FROM t",()).await.unwrap();assert_eq!(rows.next().await.unwrap().unwrap().get_value(0).unwrap(),turso::Value::Integer(1000));
        });
    }
}
