use crate::{
    connection::{configure, failure, rollback, state, to_engine},
    transfers::{bounded_line, decode},
};
use serde_json::Value as Json;
use std::{
    fs::File,
    io::{BufReader, Read},
    iter::Peekable,
    path::Path,
};
use tursobench_core::{
    Failure, ImportFormat, ImportPreview, ImportSpec, InputKind, TransactionState, TransferOutcome,
    Value, quote_identifier,
};

fn error(e: impl std::fmt::Display) -> Failure {
    Failure::input(e.to_string())
}
struct Field {
    text: String,
    quoted: bool,
}
struct Csv {
    bytes: Peekable<std::io::Bytes<BufReader<File>>>,
    delimiter: u8,
}
impl Csv {
    fn next(&mut self) -> Result<Option<Vec<Field>>, Failure> {
        let mut fields = Vec::new();
        let mut field = Vec::new();
        let mut quoted = false;
        let mut inside = false;
        let mut closed = false;
        let mut seen = false;
        let mut size = 0;
        loop {
            let next = self.bytes.next().transpose().map_err(error)?;
            let Some(b) = next else {
                if inside {
                    return Err(error("Unclosed quoted CSV field"));
                }
                if !seen {
                    return Ok(None);
                }
                fields.push(Field {
                    text: String::from_utf8(field).map_err(error)?,
                    quoted,
                });
                return Ok(Some(fields));
            };
            seen = true;
            size += 1;
            if size > 16 * 1024 * 1024 || fields.len() >= 512 {
                return Err(error("CSV record exceeds 16 MiB / 512 columns"));
            }
            if inside {
                if b == b'"' {
                    if self.bytes.peek().is_some_and(|v| matches!(v, Ok(b'"'))) {
                        self.bytes.next();
                        field.push(b'"');
                    } else {
                        inside = false;
                        closed = true;
                    }
                } else {
                    field.push(b);
                }
                continue;
            }
            if b == self.delimiter || b == b'\n' || b == b'\r' {
                fields.push(Field {
                    text: String::from_utf8(std::mem::take(&mut field)).map_err(error)?,
                    quoted,
                });
                quoted = false;
                closed = false;
                if b != self.delimiter {
                    if b == b'\r' && self.bytes.peek().is_some_and(|v| matches!(v, Ok(b'\n'))) {
                        self.bytes.next();
                    }
                    return Ok(Some(fields));
                }
            } else if b == b'"' && field.is_empty() && !closed {
                quoted = true;
                inside = true;
            } else if closed || b == b'"' {
                return Err(error("Unexpected text after CSV closing quote"));
            } else {
                field.push(b);
            }
        }
    }
}
enum Source {
    Lines(BufReader<File>),
    Json(std::vec::IntoIter<Json>),
    Csv(Csv),
}
struct Reader {
    columns: Vec<String>,
    scope: String,
    source: Source,
    seen: u64,
    expected: Option<u64>,
    finished: bool,
    kinds: Vec<InputKind>,
    null_marker: String,
}
impl Reader {
    fn open(
        path: &Path,
        format: ImportFormat,
        kinds: Vec<InputKind>,
        null_marker: String,
    ) -> Result<Self, Failure> {
        let file = File::open(path).map_err(error)?;
        let (header, source, scope) = match format {
            ImportFormat::JsonLines => {
                let mut reader = BufReader::new(file);
                let first = bounded_line(&mut reader)?
                    .ok_or_else(|| error("Missing typed JSONL header"))?;
                let header: Json = serde_json::from_slice(&first).map_err(error)?;
                if header["format"] != "tursobench-typed-jsonl"
                    || !matches!(header["version"].as_u64(), Some(1 | 2))
                {
                    return Err(error("Unsupported typed JSONL format/version"));
                }
                (header, Source::Lines(reader), String::new())
            }
            ImportFormat::Json => {
                if file.metadata().map_err(error)?.len() > 16 * 1024 * 1024 {
                    return Err(error(
                        "Typed JSON exceeds 16 MiB; use JSONL for streaming imports",
                    ));
                }
                let mut data: Json =
                    serde_json::from_reader(BufReader::new(file)).map_err(error)?;
                let header = data["header"].take();
                if header["format"] != "tursobench-typed-json" || header["version"] != 1 {
                    return Err(error("Unsupported typed JSON format/version"));
                }
                let rows = data["rows"]
                    .take()
                    .as_array()
                    .cloned()
                    .ok_or_else(|| error("Missing typed JSON rows"))?;
                if data["end"]["rows"].as_u64() != Some(rows.len() as u64) {
                    return Err(error("Typed JSON end count mismatch"));
                }
                (header, Source::Json(rows.into_iter()), String::new())
            }
            ImportFormat::Csv | ImportFormat::Tsv => {
                let mut csv = Csv {
                    bytes: BufReader::new(file).bytes().peekable(),
                    delimiter: if format == ImportFormat::Csv {
                        b','
                    } else {
                        b'\t'
                    },
                };
                let fields = csv.next()?.ok_or_else(|| error("Missing CSV/TSV header"))?;
                let columns = fields
                    .into_iter()
                    .map(|f| serde_json::json!({"name":f.text.trim_start_matches('\u{feff}')}))
                    .collect::<Vec<_>>();
                (
                    serde_json::json!({"columns":columns}),
                    Source::Csv(csv),
                    "CSV/TSV input".into(),
                )
            }
        };
        let columns = header["columns"]
            .as_array()
            .ok_or_else(|| error("Missing import columns"))?
            .iter()
            .map(|c| {
                c["name"]
                    .as_str()
                    .map(str::to_owned)
                    .ok_or_else(|| error("Invalid import column"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        if columns.is_empty()
            || columns.len() > 512
            || columns.iter().map(String::len).sum::<usize>() > 1024 * 1024
        {
            return Err(error("Import metadata exceeds budget"));
        }
        let scope = header["scope"].as_str().map_or(scope, str::to_owned);
        Ok(Self {
            columns,
            scope,
            source,
            seen: 0,
            expected: header["rows_expected"].as_u64(),
            finished: false,
            kinds,
            null_marker,
        })
    }
    fn next(&mut self) -> Result<Option<Vec<Value>>, Failure> {
        if self.finished {
            return Ok(None);
        }
        let values = match &mut self.source {
            Source::Lines(reader) => {
                let line = bounded_line(reader)?
                    .ok_or_else(|| error("Typed JSONL is incomplete: missing end record"))?;
                let row: Json = serde_json::from_slice(&line).map_err(error)?;
                if row["record"] == "end" {
                    if row["rows"].as_u64() != Some(self.seen)
                        || self.expected.is_some_and(|n| n != self.seen)
                    {
                        return Err(error("Typed JSONL end count mismatch"));
                    }
                    while let Some(line) = bounded_line(reader)? {
                        if !line.iter().all(u8::is_ascii_whitespace) {
                            return Err(error("Unexpected data after typed JSONL end"));
                        }
                    }
                    self.finished = true;
                    return Ok(None);
                }
                if row["record"] != "row" {
                    return Err(error("Unknown typed JSONL record"));
                }
                row["values"]
                    .as_array()
                    .ok_or_else(|| error("Missing row values"))?
                    .iter()
                    .map(decode)
                    .collect::<Result<Vec<_>, _>>()?
            }
            Source::Json(rows) => {
                let Some(row) = rows.next() else {
                    self.finished = true;
                    if self.expected.is_some_and(|n| n != self.seen) {
                        return Err(error("Typed JSON expected row count mismatch"));
                    }
                    return Ok(None);
                };
                row.as_array()
                    .ok_or_else(|| error("Invalid typed JSON row"))?
                    .iter()
                    .map(decode)
                    .collect::<Result<Vec<_>, _>>()?
            }
            Source::Csv(csv) => {
                let Some(fields) = csv.next()? else {
                    self.finished = true;
                    return Ok(None);
                };
                fields
                    .into_iter()
                    .enumerate()
                    .map(|(i, f)| {
                        if !f.quoted && f.text == self.null_marker {
                            return Ok(Value::Null);
                        }
                        match self.kinds.get(i).copied().unwrap_or_default() {
                            InputKind::Text => Ok(Value::Text(f.text)),
                            InputKind::Integer => f.text.parse().map(Value::Integer).map_err(error),
                            InputKind::Real => f.text.parse::<f64>().map_err(error).and_then(|n| {
                                if n.is_finite() {
                                    Ok(Value::Real(n))
                                } else {
                                    Err(error("CSV real must be finite"))
                                }
                            }),
                            InputKind::Blob => {
                                let h = f
                                    .text
                                    .strip_prefix("X'")
                                    .or_else(|| f.text.strip_prefix("x'"))
                                    .and_then(|s| s.strip_suffix('\''))
                                    .unwrap_or(&f.text);
                                decode(&serde_json::json!({"type":"blob","hex":h}))
                            }
                        }
                    })
                    .collect::<Result<Vec<_>, _>>()?
            }
        };
        if values.len() != self.columns.len() {
            return Err(error(format!(
                "Row {} width differs from header",
                self.seen + 1
            )));
        }
        if values.iter().map(Value::bytes).sum::<usize>() > 8 * 1024 * 1024 {
            return Err(error("Import row exceeds 8 MiB"));
        }
        self.seen += 1;
        Ok(Some(values))
    }
}
pub(crate) fn preview(
    path: &Path,
    format: ImportFormat,
    kinds: Vec<InputKind>,
    null_marker: String,
) -> Result<ImportPreview, Failure> {
    let mut reader = Reader::open(path, format, kinds, null_marker)?;
    let mut rows = Vec::new();
    let mut bytes = 0;
    for _ in 0..10 {
        if let Some(row) = reader.next()? {
            bytes += row.iter().map(Value::bytes).sum::<usize>();
            if bytes > 1024 * 1024 {
                break;
            }
            rows.push(row);
        } else {
            break;
        }
    }
    Ok(ImportPreview {
        columns: reader.columns,
        rows,
        scope: reader.scope,
    })
}
pub(crate) async fn import(
    db: &turso::Database,
    spec: ImportSpec,
    settings: tursobench_core::ConnectionSettings,
    cancel: impl Fn() -> bool,
) -> Result<TransferOutcome, Failure> {
    let mut reader = Reader::open(&spec.path, spec.format, spec.kinds, spec.null_marker)?;
    if reader.columns != spec.source_columns {
        return Err(error("Source header changed; preview the input again"));
    }
    if reader.columns.len() != spec.mapping.len() {
        return Err(error(
            "Import mapping width changed; preview the input again",
        ));
    }
    let c = db.connect().map_err(failure)?;
    configure(&c, settings).await?;
    let table = crate::browse::describe(&c, &spec.table).await?;
    if !table.is_table
        || !table.has_rowid
        || table.engine_managed()
        || table.columns.iter().any(|c| c.hidden != 0)
    {
        return Err(error("Unsupported import target"));
    }
    let mut columns = Vec::new();
    let mut indices = Vec::new();
    let mut unique = std::collections::BTreeSet::new();
    for (i, name) in spec.mapping.iter().enumerate() {
        if name.is_empty() {
            continue;
        }
        let col = table
            .columns
            .iter()
            .find(|c| c.name.eq_ignore_ascii_case(name) && c.hidden == 0)
            .ok_or_else(|| error("Mapped target column does not exist or is generated"))?;
        if !unique.insert(col.name.to_lowercase()) {
            return Err(error("Duplicate target column mapping"));
        }
        columns.push(quote_identifier(&col.name));
        indices.push(i);
    }
    if columns.is_empty() {
        return Err(error("Map at least one column"));
    }
    let sql = format!(
        "INSERT INTO {} ({}) VALUES ({})",
        quote_identifier(&table.name),
        columns.join(","),
        (1..=columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>()
            .join(",")
    );
    let mut insert = c.prepare(&sql).await.map_err(failure)?;
    c.execute("BEGIN IMMEDIATE", ()).await.map_err(failure)?;
    let mut committed = 0u64;
    let mut pending = 0u64;
    let mut bytes = 0usize;
    let mut uncertain = false;
    let result = async {
        while let Some(row) = reader.next()? {
            if cancel() {
                return Err(error("Import cancelled"));
            }
            bytes += row.iter().map(Value::bytes).sum::<usize>();
            let params = indices
                .iter()
                .map(|i| to_engine(row[*i].clone()))
                .collect::<Result<Vec<_>, _>>()?;
            let changed = insert
                .execute(turso::params::Params::Positional(params))
                .await
                .map_err(failure)?;
            if changed != 1 {
                return Err(error(format!(
                    "Import row {} affected {changed} rows instead of one",
                    reader.seen
                )));
            }
            pending += 1;
            if !spec.atomic && (pending >= 1000 || bytes >= 8 * 1024 * 1024) {
                if let Err(e) = c.execute("COMMIT", ()).await {
                    uncertain = true;
                    return Err(failure(e));
                }
                committed += pending;
                pending = 0;
                bytes = 0;
                c.execute("BEGIN IMMEDIATE", ()).await.map_err(failure)?;
            }
        }
        if let Err(e) = c.execute("COMMIT", ()).await {
            uncertain = true;
            return Err(failure(e));
        }
        committed += pending;
        Ok::<_, Failure>(())
    }
    .await;
    let mut failure = result.err();
    if failure.is_some()
        && let Err(e) = rollback(&c).await
    {
        uncertain = true;
        failure = Some(e);
    }
    let transaction = if uncertain {
        TransactionState::Unknown
    } else {
        state(&c)
    };
    let message = if uncertain {
        format!(
            "UNKNOWN import outcome; {committed} rows confirmed committed before the uncertain batch. Reopen and inspect before retry."
        )
    } else if let Some(e) = &failure {
        format!("Import stopped: {e}. {committed} rows committed; current batch rolled back.")
    } else {
        format!("Imported {committed} rows")
    };
    if uncertain {
        failure = Some(Failure {
            kind: tursobench_core::ErrorKind::Unknown,
            message: message.clone(),
        });
    }
    Ok(TransferOutcome {
        rows: committed,
        changed: committed,
        transaction,
        error: failure,
        message,
        executed: true,
    })
}
