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
use turso::Connection;
use tursobench_core::{
    ErrorKind, Failure, ImportFormat, ImportPreview, ImportSpec, InputKind, Table,
    TransactionState, TransferOutcome, Value, quote_identifier,
};

fn error(e: impl std::fmt::Display) -> Failure {
    Failure::input(e.to_string())
}
struct Field {
    text: String,
    quoted: bool,
}
/// A CSV record assembled byte by byte.
#[derive(Default)]
struct Record {
    fields: Vec<Field>,
    field: Vec<u8>,
    quoted: bool,
    inside: bool,
    closed: bool,
}
impl Record {
    fn end_field(&mut self) -> Result<(), Failure> {
        self.fields.push(Field {
            text: String::from_utf8(std::mem::take(&mut self.field)).map_err(error)?,
            quoted: self.quoted,
        });
        self.quoted = false;
        self.closed = false;
        Ok(())
    }
}
struct Csv {
    bytes: Peekable<std::io::Bytes<BufReader<File>>>,
    delimiter: u8,
}
impl Csv {
    fn next(&mut self) -> Result<Option<Vec<Field>>, Failure> {
        let mut record = Record::default();
        let mut size = 0;
        while let Some(b) = self.bytes.next().transpose().map_err(error)? {
            size += 1;
            if size > 16 * 1024 * 1024 || record.fields.len() >= 512 {
                return Err(error("CSV record exceeds 16 MiB / 512 columns"));
            }
            if record.inside {
                self.quoted(&mut record, b);
            } else if self.unquoted(&mut record, b)? {
                return Ok(Some(record.fields));
            }
        }
        if record.inside {
            return Err(error("Unclosed quoted CSV field"));
        }
        if size == 0 {
            return Ok(None);
        }
        record.end_field()?;
        Ok(Some(record.fields))
    }
    /// Consumes the next byte when it is `byte`.
    fn skip(&mut self, byte: u8) -> bool {
        let found = self
            .bytes
            .peek()
            .is_some_and(|v| matches!(v, Ok(b) if *b == byte));
        if found {
            self.bytes.next();
        }
        found
    }
    /// Outside quotes; true when `b` ends the record.
    fn unquoted(&mut self, record: &mut Record, b: u8) -> Result<bool, Failure> {
        let ends_record = b == b'\n' || b == b'\r';
        if ends_record || b == self.delimiter {
            record.end_field()?;
            if b == b'\r' {
                self.skip(b'\n');
            }
            return Ok(ends_record);
        }
        if b == b'"' && record.field.is_empty() && !record.closed {
            record.quoted = true;
            record.inside = true;
        } else if record.closed || b == b'"' {
            return Err(error("Unexpected text after CSV closing quote"));
        } else {
            record.field.push(b);
        }
        Ok(false)
    }
    /// Inside quotes, `""` is a literal quote and a lone `"` closes the field.
    fn quoted(&mut self, record: &mut Record, b: u8) {
        if b != b'"' {
            record.field.push(b);
        } else if self.skip(b'"') {
            record.field.push(b'"');
        } else {
            record.inside = false;
            record.closed = true;
        }
    }
}
fn csv_value(field: Field, kind: InputKind, null_marker: &str) -> Result<Value, Failure> {
    if !field.quoted && field.text == null_marker {
        return Ok(Value::Null);
    }
    match kind {
        InputKind::Text => Ok(Value::Text(field.text)),
        InputKind::Integer => field.text.parse().map(Value::Integer).map_err(error),
        InputKind::Real => field.text.parse::<f64>().map_err(error).and_then(|n| {
            if n.is_finite() {
                Ok(Value::Real(n))
            } else {
                Err(error("CSV real must be finite"))
            }
        }),
        InputKind::Blob => {
            let text = &field.text;
            let hex = text
                .strip_prefix("X'")
                .or_else(|| text.strip_prefix("x'"))
                .and_then(|s| s.strip_suffix('\''))
                .unwrap_or(text);
            decode(&serde_json::json!({"type":"blob","hex":hex}))
        }
    }
}
fn typed_values(row: &Json, missing: &str) -> Result<Vec<Value>, Failure> {
    row.as_array()
        .ok_or_else(|| error(missing))?
        .iter()
        .map(decode)
        .collect()
}
/// The next typed JSONL row, or `None` after a verified end record.
fn typed_line(
    reader: &mut BufReader<File>,
    seen: u64,
    expected: Option<u64>,
) -> Result<Option<Vec<Value>>, Failure> {
    let line = bounded_line(reader)?
        .ok_or_else(|| error("Typed JSONL is incomplete: missing end record"))?;
    let row: Json = serde_json::from_slice(&line).map_err(error)?;
    if row["record"] == "end" {
        if row["rows"].as_u64() != Some(seen) || expected.is_some_and(|n| n != seen) {
            return Err(error("Typed JSONL end count mismatch"));
        }
        while let Some(line) = bounded_line(reader)? {
            if !line.iter().all(u8::is_ascii_whitespace) {
                return Err(error("Unexpected data after typed JSONL end"));
            }
        }
        return Ok(None);
    }
    if row["record"] != "row" {
        return Err(error("Unknown typed JSONL record"));
    }
    typed_values(&row["values"], "Missing row values").map(Some)
}
enum Source {
    Lines(BufReader<File>),
    Json(std::vec::IntoIter<Json>),
    Csv(Csv),
}
impl Source {
    fn json_lines(file: File) -> Result<(Json, Self), Failure> {
        let mut reader = BufReader::new(file);
        let first =
            bounded_line(&mut reader)?.ok_or_else(|| error("Missing typed JSONL header"))?;
        let header: Json = serde_json::from_slice(&first).map_err(error)?;
        if header["format"] != "tursobench-typed-jsonl"
            || !matches!(header["version"].as_u64(), Some(1 | 2))
        {
            return Err(error("Unsupported typed JSONL format/version"));
        }
        Ok((header, Self::Lines(reader)))
    }
    fn json(file: File) -> Result<(Json, Self), Failure> {
        if file.metadata().map_err(error)?.len() > 16 * 1024 * 1024 {
            return Err(error(
                "Typed JSON exceeds 16 MiB; use JSONL for streaming imports",
            ));
        }
        let mut data: Json = serde_json::from_reader(BufReader::new(file)).map_err(error)?;
        let header = data["header"].take();
        if header["format"] != "tursobench-typed-json" || header["version"] != 1 {
            return Err(error("Unsupported typed JSON format/version"));
        }
        let Json::Array(rows) = data["rows"].take() else {
            return Err(error("Missing typed JSON rows"));
        };
        if data["end"]["rows"].as_u64() != Some(rows.len() as u64) {
            return Err(error("Typed JSON end count mismatch"));
        }
        Ok((header, Self::Json(rows.into_iter())))
    }
    /// The header row names the columns; CSV carries no scope of its own.
    fn csv(file: File, delimiter: u8) -> Result<(Json, Self), Failure> {
        let mut csv = Csv {
            bytes: BufReader::new(file).bytes().peekable(),
            delimiter,
        };
        let fields = csv.next()?.ok_or_else(|| error("Missing CSV/TSV header"))?;
        let columns = fields
            .into_iter()
            .map(|f| serde_json::json!({"name":f.text.trim_start_matches('\u{feff}')}))
            .collect::<Vec<_>>();
        let header = serde_json::json!({"columns":columns,"scope":"CSV/TSV input"});
        Ok((header, Self::Csv(csv)))
    }
}
fn header_columns(header: &Json) -> Result<Vec<String>, Failure> {
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
    Ok(columns)
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
        let (header, source) = match format {
            ImportFormat::JsonLines => Source::json_lines(file)?,
            ImportFormat::Json => Source::json(file)?,
            ImportFormat::Csv => Source::csv(file, b',')?,
            ImportFormat::Tsv => Source::csv(file, b'\t')?,
        };
        Ok(Self {
            columns: header_columns(&header)?,
            scope: header["scope"].as_str().unwrap_or_default().to_owned(),
            source,
            seen: 0,
            expected: header["rows_expected"].as_u64(),
            finished: false,
            kinds,
            null_marker,
        })
    }
    fn read(&mut self) -> Result<Option<Vec<Value>>, Failure> {
        let (seen, expected) = (self.seen, self.expected);
        match &mut self.source {
            Source::Lines(reader) => typed_line(reader, seen, expected),
            Source::Json(rows) => match rows.next() {
                Some(row) => typed_values(&row, "Invalid typed JSON row").map(Some),
                None if expected.is_some_and(|n| n != seen) => {
                    Err(error("Typed JSON expected row count mismatch"))
                }
                None => Ok(None),
            },
            Source::Csv(csv) => {
                let Some(fields) = csv.next()? else {
                    return Ok(None);
                };
                fields
                    .into_iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let kind = self.kinds.get(i).copied().unwrap_or_default();
                        csv_value(f, kind, &self.null_marker)
                    })
                    .collect::<Result<Vec<_>, _>>()
                    .map(Some)
            }
        }
    }
    fn next(&mut self) -> Result<Option<Vec<Value>>, Failure> {
        if self.finished {
            return Ok(None);
        }
        let Some(values) = self.read()? else {
            self.finished = true;
            return Ok(None);
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
        let Some(row) = reader.next()? else {
            break;
        };
        bytes += row.iter().map(Value::bytes).sum::<usize>();
        if bytes > 1024 * 1024 {
            break;
        }
        rows.push(row);
    }
    Ok(ImportPreview {
        columns: reader.columns,
        rows,
        scope: reader.scope,
    })
}
/// The mapped insert into a rowid table, prepared before the transaction.
struct Target {
    table: Table,
    indices: Vec<usize>,
    insert: turso::Statement,
    atomic: bool,
}
impl Target {
    async fn prepare(
        c: &Connection,
        name: &str,
        mapping: &[String],
        atomic: bool,
    ) -> Result<Self, Failure> {
        let table = crate::browse::describe(c, name).await?;
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
        for (i, name) in mapping.iter().enumerate().filter(|(_, n)| !n.is_empty()) {
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
        let placeholders = (1..=columns.len())
            .map(|i| format!("?{i}"))
            .collect::<Vec<_>>();
        let sql = format!(
            "INSERT INTO {} ({}) VALUES ({})",
            quote_identifier(&table.name),
            columns.join(","),
            placeholders.join(",")
        );
        let insert = c.prepare(&sql).await.map_err(failure)?;
        Ok(Self {
            table,
            indices,
            insert,
            atomic,
        })
    }
}
/// Rows committed so far and the open batch; `uncertain` marks a COMMIT
/// whose outcome is unknown.
#[derive(Default)]
struct Progress {
    committed: u64,
    pending: u64,
    bytes: usize,
    uncertain: bool,
}
impl Progress {
    async fn commit(&mut self, c: &Connection) -> Result<(), Failure> {
        if let Err(e) = c.execute("COMMIT", ()).await {
            self.uncertain = true;
            return Err(failure(e));
        }
        self.committed += self.pending;
        self.pending = 0;
        self.bytes = 0;
        Ok(())
    }
    fn outcome(
        self,
        mut failure: Option<Failure>,
        transaction: TransactionState,
    ) -> TransferOutcome {
        let committed = self.committed;
        let message = if self.uncertain {
            format!(
                "UNKNOWN import outcome; {committed} rows confirmed committed before the uncertain batch. Reopen and inspect before retry."
            )
        } else if let Some(e) = &failure {
            format!("Import stopped: {e}. {committed} rows committed; current batch rolled back.")
        } else {
            format!("Imported {committed} rows")
        };
        if self.uncertain {
            failure = Some(Failure {
                kind: ErrorKind::Unknown,
                message: message.clone(),
            });
        }
        TransferOutcome {
            rows: committed,
            changed: committed,
            transaction,
            error: failure,
            message,
            executed: true,
        }
    }
}
/// Inserts every row, committing batches unless the import is atomic.
async fn load(
    c: &Connection,
    target: &mut Target,
    reader: &mut Reader,
    progress: &mut Progress,
    cancel: impl Fn() -> bool,
) -> Result<(), Failure> {
    while let Some(row) = reader.next()? {
        if progress.pending == 0
            && crate::browse::describe(c, &target.table.name).await? != target.table
        {
            return Err(Failure {
                kind: ErrorKind::Conflict,
                message: "Import target schema changed between batches; preview the mapping again"
                    .into(),
            });
        }
        if cancel() {
            return Err(error("Import cancelled"));
        }
        progress.bytes += row.iter().map(Value::bytes).sum::<usize>();
        let params = target
            .indices
            .iter()
            .map(|i| to_engine(row[*i].clone()))
            .collect::<Result<Vec<_>, _>>()?;
        let changed = target
            .insert
            .execute(turso::params::Params::Positional(params))
            .await
            .map_err(failure)?;
        if changed != 1 {
            return Err(error(format!(
                "Import row {} affected {changed} rows instead of one",
                reader.seen
            )));
        }
        progress.pending += 1;
        if !target.atomic && (progress.pending >= 1000 || progress.bytes >= 8 * 1024 * 1024) {
            progress.commit(c).await?;
            c.execute("BEGIN IMMEDIATE", ()).await.map_err(failure)?;
        }
    }
    progress.commit(c).await
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
    let mut target = Target::prepare(&c, &spec.table, &spec.mapping, spec.atomic).await?;
    c.execute("BEGIN IMMEDIATE", ()).await.map_err(failure)?;
    let mut progress = Progress::default();
    let mut failure = load(&c, &mut target, &mut reader, &mut progress, cancel)
        .await
        .err();
    if failure.is_some()
        && let Err(e) = rollback(&c).await
    {
        progress.uncertain = true;
        failure = Some(e);
    }
    let transaction = if progress.uncertain {
        TransactionState::Unknown
    } else {
        state(&c)
    };
    Ok(progress.outcome(failure, transaction))
}
