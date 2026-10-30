//! Engine-independent application data, not a multi-engine abstraction.
mod edits;
pub use edits::*;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value")]
pub enum Value {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}
impl Value {
    pub fn bytes(&self) -> usize {
        std::mem::size_of::<Self>()
            + match self {
                Self::Text(s) => s.len(),
                Self::Blob(b) => b.len(),
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

#[derive(Debug, Clone)]
pub struct Column {
    pub name: String,
    pub declared_type: Option<String>,
}
#[derive(Debug, Clone, Copy)]
pub struct Budget {
    pub rows: usize,
    pub bytes: usize,
    pub cell_bytes: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            rows: 1000,
            bytes: 8 * 1024 * 1024,
            cell_bytes: 64 * 1024,
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
    pub delivery: Delivery,
    pub rows_seen: u64,
    pub changed: u64,
    pub transaction: TransactionState,
    pub error: Option<Failure>,
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
    pub fn editable(&self) -> bool {
        self.is_table && !self.keys().is_empty() && self.columns.iter().all(|c| c.hidden == 0)
    }
}
#[derive(Debug, Clone)]
pub struct Browse {
    pub table: String,
    pub offset: usize,
    pub sort: Option<usize>,
    pub descending: bool,
    pub filter: Option<(usize, Value)>,
}

#[derive(Debug)]
pub enum EventKind {
    Table(Table),
    EditsCommitted(CommitOutcome),
    Opened,
    Columns(Vec<Column>),
    Rows(Vec<Vec<Value>>),
    Finished(Completion),
    Closed { rollback: Result<(), Failure> },
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
    use super::*;
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
}
