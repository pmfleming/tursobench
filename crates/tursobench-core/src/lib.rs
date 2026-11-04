//! Engine-independent application data, not a multi-engine abstraction.
mod edits;
pub use edits::{BoundEdit, CommitOutcome, DraftBatch, Edit};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
    Deferred(Box<DeferredCell>),
}
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DeferredCell {
    pub handle: Option<u64>,
    pub kind: LargeKind,
    pub bytes: u64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LargeKind {
    Text,
    Blob,
}

impl Value {
    pub fn from_input(value: serde_json::Value) -> Result<Self, Failure> {
        Ok(match value {
            serde_json::Value::Null => Self::Null,
            serde_json::Value::Bool(v) => Self::Integer(i64::from(v)),
            serde_json::Value::String(v) => Self::Text(v),
            serde_json::Value::Number(n) => {
                if let Some(n) = n.as_i64() {
                    Self::Integer(n)
                } else if n.is_u64() {
                    return Err(Failure::input("Integer exceeds signed 64-bit range"));
                } else {
                    Self::Real(
                        n.as_f64()
                            .filter(|n| n.is_finite())
                            .ok_or_else(|| Failure::input("Invalid real number"))?,
                    )
                }
            }
            serde_json::Value::Object(_) => {
                serde_json::from_value(value).map_err(|e| Failure::input(e.to_string()))?
            }
            _ => {
                return Err(Failure::input(
                    "Use a tagged Blob for byte arrays; nested arrays are not scalar bindings",
                ));
            }
        })
    }
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Text(s) => s.len(),
                Self::Blob(b) => b.len(),
                Self::Deferred(_) => std::mem::size_of::<DeferredCell>(),
                _ => 0,
            }
    }
    pub fn preview(&self) -> String {
        match self {
            Self::Null => "NULL".into(),
            Self::Integer(n) => n.to_string(),
            Self::Real(n) => n.to_string(),
            Self::Text(s) => {
                let mut p: String = s.chars().take(100).collect();
                if s.chars().count() > 100 {
                    p.push('…');
                }
                p
            }
            Self::Blob(b) => format!("[blob: {} bytes]", b.len()),
            Self::Deferred(cell) => format!("[{:?}: {} bytes · deferred]", cell.kind, cell.bytes),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Bindings {
    Positional(Vec<Value>),
    Named(std::collections::BTreeMap<String, Value>),
}
impl From<Vec<Value>> for Bindings {
    fn from(values: Vec<Value>) -> Self {
        Self::Positional(values)
    }
}
impl Bindings {
    pub fn parse_json(input: &str) -> Result<Self, Failure> {
        if input.len() > 8 * 1024 * 1024 {
            return Err(Failure::input("Bindings exceed 8 MiB"));
        }
        match serde_json::from_str(input).map_err(|e| Failure::input(e.to_string()))? {
            serde_json::Value::Array(values) => Ok(Self::Positional(
                values
                    .into_iter()
                    .map(Value::from_input)
                    .collect::<Result<_, _>>()?,
            )),
            serde_json::Value::Object(values) => Ok(Self::Named(
                values
                    .into_iter()
                    .map(|(k, v)| Value::from_input(v).map(|v| (k, v)))
                    .collect::<Result<_, _>>()?,
            )),
            _ => Err(Failure::input("Bindings require an array or named object")),
        }
    }
    pub fn bytes(&self) -> usize {
        match self {
            Self::Positional(v) => v.iter().map(Value::bytes).sum(),
            Self::Named(v) => v.iter().map(|(k, v)| k.len() + v.bytes()).sum(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ticket {
    pub database: u64,
    pub session: u64,
    pub request: u64,
    pub generation: u64,
}
#[derive(Debug, Clone, Copy)]
pub struct ConnectionSettings {
    pub busy_timeout_ms: u64,
    pub query_timeout_ms: u64,
}
impl Default for ConnectionSettings {
    fn default() -> Self {
        Self {
            busy_timeout_ms: 250,
            query_timeout_ms: 30_000,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    ReadOnly,
    ReadWrite,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionState {
    Autocommit,
    Active,
    Unknown,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ErrorKind {
    Input,
    Io,
    Busy,
    ReadOnly,
    Constraint,
    Conflict,
    Engine,
    Unknown,
    Interrupted,
}
#[derive(Debug, Clone)]
pub struct Failure {
    pub kind: ErrorKind,
    pub message: String,
}
impl Failure {
    pub fn input(message: impl Into<String>) -> Self {
        Self {
            kind: ErrorKind::Input,
            message: message.into(),
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.message)
    }
}
impl std::error::Error for Failure {}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Column {
    pub name: String,
    pub declared_type: Option<String>,
}
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
    pub cell_bytes: usize,
    pub page: bool,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            rows: 1000,
            bytes: 8 * 1024 * 1024,
            cell_bytes: 64 * 1024,
            page: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Complete,
    Limited,
    Discarded,
}
#[derive(Debug, Clone)]
pub struct Completion {
    /// Worker execution plus delivery/backpressure time; excludes queue wait.
    pub elapsed_ms: u64,
    pub delivery: Delivery,
    pub rows_seen: u64,
    pub page_has_more: Option<bool>,
    pub changed: u64,
    pub transaction: TransactionState,
    pub error: Option<Failure>,
}
#[derive(Debug, Clone)]
pub struct ScriptResult {
    pub index: usize,
    pub sql: String,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub completion: Completion,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ImportFormat {
    #[default]
    JsonLines,
    Json,
    Csv,
    Tsv,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputKind {
    #[default]
    Text,
    Integer,
    Real,
    Blob,
}
#[derive(Debug, Clone)]
pub struct ImportSpec {
    pub source_columns: Vec<String>,
    pub path: std::path::PathBuf,
    pub table: String,
    pub format: ImportFormat,
    pub mapping: Vec<String>,
    pub kinds: Vec<InputKind>,
    pub null_marker: String,
    pub atomic: bool,
}
#[derive(Debug, Clone)]
pub struct ImportPreview {
    pub columns: Vec<String>,
    pub rows: Vec<Vec<Value>>,
    pub scope: String,
}
#[derive(Debug, Clone)]
pub struct TransferOutcome {
    pub rows: u64,
    pub changed: u64,
    pub transaction: TransactionState,
    pub error: Option<Failure>,
    pub message: String,
    pub executed: bool,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableColumn {
    pub name: String,
    pub declared_type: String,
    pub not_null: bool,
    pub primary_key: i64,
    pub hidden: i64,
    pub default_sql: Option<String>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Table {
    pub name: String,
    pub columns: Vec<TableColumn>,
    pub is_table: bool,
    /// True only when parsed schema establishes an ordinary rowid table.
    pub has_rowid: bool,
    pub binary_collations: bool,
}
impl Table {
    pub fn keys(&self) -> Vec<usize> {
        let mut keys: Vec<_> = self
            .columns
            .iter()
            .enumerate()
            .filter(|(_, c)| c.primary_key > 0)
            .map(|(i, _)| i)
            .collect();
        keys.sort_by_key(|i| self.columns[*i].primary_key);
        keys
    }
    pub fn engine_managed(&self) -> bool {
        let prefix = "__turso_internal_";
        self.name
            .get(..prefix.len())
            .is_some_and(|name| name.eq_ignore_ascii_case(prefix))
    }
    pub fn editable(&self) -> bool {
        self.is_table
            && self.has_rowid
            && !self.engine_managed()
            && self.columns.iter().any(|c| c.primary_key > 0)
            && self.columns.iter().all(|c| c.hidden == 0)
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Filter {
    pub column: usize,
    pub op: FilterOp,
    pub value: Value,
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum FilterOp {
    #[default]
    Equal,
    NotEqual,
    Less,
    Greater,
    LessOrEqual,
    GreaterOrEqual,
    Contains,
    IsNull,
    IsNotNull,
}
impl FilterOp {
    pub const ALL: [Self; 9] = [
        Self::Equal,
        Self::NotEqual,
        Self::Less,
        Self::Greater,
        Self::LessOrEqual,
        Self::GreaterOrEqual,
        Self::Contains,
        Self::IsNull,
        Self::IsNotNull,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Equal => "=",
            Self::NotEqual => "≠",
            Self::Less => "<",
            Self::Greater => ">",
            Self::LessOrEqual => "<=",
            Self::GreaterOrEqual => ">=",
            Self::Contains => "contains",
            Self::IsNull => "is null",
            Self::IsNotNull => "is not null",
        }
    }
    pub fn unary(self) -> bool {
        matches!(self, Self::IsNull | Self::IsNotNull)
    }
}
#[derive(Debug, Clone)]
pub struct Browse {
    pub options: BrowseOptions,
    pub table: String,
    pub offset: usize,
    pub sort: Option<usize>,
    pub descending: bool,
    pub filters: Vec<Filter>,
}

#[derive(Debug, Clone, Default)]
pub struct BrowseOptions {
    pub sorts: Vec<(usize, bool)>,
    pub predicate: Option<Box<FilterExpr>>,
    /// Original retained row used to seek in the same filter/order generation.
    pub cursor: Option<Vec<Value>>,
    pub reverse: bool,
    pub relation: Option<Box<(ForeignKey, Vec<Value>)>>,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum FilterExpr {
    Compare(Filter),
    In { column: usize, values: Vec<Value> },
    All(Vec<FilterExpr>),
    Any(Vec<FilterExpr>),
}
impl FilterExpr {
    pub fn bytes(&self) -> usize {
        fn walk(expr: &FilterExpr, depth: usize) -> usize {
            if depth > 8 {
                return 16 * 1024 * 1024;
            }
            match expr {
                FilterExpr::Compare(f) => f.value.bytes(),
                FilterExpr::In { values, .. } => values
                    .iter()
                    .map(Value::bytes)
                    .fold(0, usize::saturating_add),
                FilterExpr::All(parts) | FilterExpr::Any(parts) => parts
                    .iter()
                    .map(|p| walk(p, depth + 1))
                    .fold(0, usize::saturating_add),
            }
        }
        walk(self, 0)
    }
}

#[derive(Debug, Clone)]
pub struct SchemaObject {
    pub kind: String,
    pub name: String,
    pub table: String,
    pub ddl: Option<String>,
}
#[derive(Debug, Clone)]
pub struct IndexInfo {
    pub name: String,
    pub unique: bool,
    pub partial: bool,
    pub columns: Vec<Option<String>>,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForeignKey {
    pub id: i64,
    pub target: String,
    pub columns: Vec<(String, Option<String>)>,
    pub on_update: String,
    pub on_delete: String,
}
#[derive(Debug, Clone)]
pub struct Inspection {
    pub table: Table,
    pub indexes: Vec<IndexInfo>,
    pub foreign_keys: Vec<ForeignKey>,
}

#[derive(Debug, Clone)]
pub struct DiagramTable {
    pub name: String,
    pub inspection: Option<Inspection>,
    pub error: Option<String>,
}
#[derive(Debug, Clone, Default)]
pub struct SchemaDiagram {
    pub tables: Vec<DiagramTable>,
    /// Discovery/metadata budget reached; never label this as the entire schema.
    pub limited: bool,
}

#[derive(Debug)]
pub enum EventKind {
    ImportPreview(ImportPreview),
    TransferFinished(TransferOutcome),
    ScriptResult(ScriptResult),
    ScriptOutcome {
        atomic: bool,
        rolled_back: bool,
    },
    ValueChunk {
        handle: u64,
        offset: u64,
        bytes: Vec<u8>,
        total: u64,
    },
    ValueSaved(String),
    Invalidated {
        schema: bool,
        source_request: u64,
    },
    Diagram(SchemaDiagram),
    Inspection(Inspection),
    Table(Table),
    EditsCommitted(CommitOutcome),
    Opened,
    Columns(Vec<Column>),
    Rows(Vec<Vec<Value>>),
    Finished(Completion),
    Closed {
        rollback: Result<(), Failure>,
    },
    Failed(Failure),
}
#[derive(Debug)]
pub struct Event {
    pub ticket: Ticket,
    pub kind: EventKind,
}

#[derive(Default)]
pub struct ResultSet {
    pub current: Option<Ticket>,
    pub columns: Vec<Column>,
    pub rows: Vec<Vec<Value>>,
    pub completion: Option<Completion>,
}
impl ResultSet {
    pub fn start(&mut self, ticket: Ticket) {
        *self = Self {
            current: Some(ticket),
            ..Self::default()
        };
    }
    pub fn apply(&mut self, event: Event) -> bool {
        if self.current != Some(event.ticket) {
            return false;
        }
        match event.kind {
            EventKind::Columns(c) => self.columns = c,
            EventKind::Rows(r) => self.rows.extend(r),
            EventKind::Finished(c) => self.completion = Some(c),
            _ => return false,
        }
        true
    }
}
pub fn quote_identifier(s: &str) -> String {
    format!("\"{}\"", s.replace('"', "\"\""))
}

#[cfg(test)]
mod tests {
    use super::{Event, EventKind, ResultSet, Ticket, Value, quote_identifier};
    #[test]
    fn stale_events_cannot_replace_results() {
        let a = Ticket {
            database: 1,
            session: 1,
            request: 1,
            generation: 0,
        };
        let b = Ticket { generation: 1, ..a };
        let mut result = ResultSet::default();
        result.start(b);
        assert!(!result.apply(Event {
            ticket: a,
            kind: EventKind::Rows(vec![vec![Value::Null]])
        }));
        assert!(result.rows.is_empty());
    }
    #[test]
    fn types_and_quotes_are_preserved() {
        assert_eq!(quote_identifier("a\"b"), "\"a\"\"b\"");
        assert_ne!(Value::Null, Value::Text(String::new()));
        let value = Value::Blob(vec![0, 255]);
        assert_eq!(
            serde_json::from_str::<Value>(&serde_json::to_string(&value).unwrap()).unwrap(),
            value
        );
    }
    #[test]
    fn convenient_bindings_keep_integer_text_null_and_blob_types() {
        let super::Bindings::Positional(values) =
            super::Bindings::parse_json(r#"[1,"a",null,{"type":"Blob","value":[0,255]}]"#).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            values,
            vec![
                Value::Integer(1),
                Value::Text("a".into()),
                Value::Null,
                Value::Blob(vec![0, 255])
            ]
        );
        assert!(super::Bindings::parse_json("[9223372036854775808]").is_err());
        assert!(super::Bindings::parse_json("[[1,2]]").is_err());
    }
}
