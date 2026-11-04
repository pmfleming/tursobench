//! Turso-only session worker. No engine handles cross the service boundary.
use tursobench_core::{
    Access, Bindings, Browse, Budget, ConnectionSettings, DraftBatch, ErrorKind, Event, Failure,
    Ticket, Value,
};
mod access;
mod browse;
mod observe;
pub use access::{ProcessUsage, check_access, processes_using};
mod connection;
mod diagram;
mod edits;
mod imports;
mod inspection;
pub mod preferences;
pub mod sql;
mod transfers;
mod validation;
mod values;
mod worker;
pub use sql::validate_single;
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc::{self, Receiver, SyncSender},
    },
};
use worker::{Output, worker};

static NEXT_SESSION: AtomicU64 = AtomicU64::new(1);
const COMMAND_CAPACITY: usize = 32;
const COMMAND_BYTES: usize = 16 * 1024 * 1024;
const EVENT_CAPACITY: usize = 8;

/// Only submitted, unfinished requests occupy this map. Cancellation targets one
/// request; a newer view must never suppress an older view's unrelated results.
#[derive(Default)]
struct DeliveryControl {
    requests: Mutex<std::collections::BTreeMap<u64, (bool, usize)>>,
    active_read: Mutex<Option<ActiveRead>>,
}
struct ActiveRead {
    request: u64,
    interrupt: Box<dyn Fn() + Send + Sync>,
}
impl DeliveryControl {
    fn discarded(&self, request: u64) -> bool {
        self.requests
            .lock()
            .unwrap()
            .get(&request)
            .map(|(discarded, _)| *discarded)
            .unwrap_or(false)
    }
    fn discard(&self, request: u64) {
        if let Some(discarded) = self.requests.lock().unwrap().get_mut(&request) {
            discarded.0 = true;
        }
        if let Some(active) = &*self.active_read.lock().unwrap()
            && active.request == request
        {
            (active.interrupt)();
        }
    }
    fn finished(&self, request: u64) {
        self.requests.lock().unwrap().remove(&request);
    }
}

pub enum Operation {
    ExportQuery {
        path: PathBuf,
        sql: String,
        parameters: Bindings,
    },
    ExportTable {
        path: PathBuf,
        spec: Browse,
    },
    PreviewImport {
        path: PathBuf,
        format: tursobench_core::ImportFormat,
        kinds: Vec<tursobench_core::InputKind>,
        null_marker: String,
    },
    Import(tursobench_core::ImportSpec),
    Checkpoint,
    BackupAndClose(PathBuf),
    Script {
        sql: String,
        parameters: Bindings,
        atomic: bool,
        budget: Budget,
    },
    ReadValue {
        handle: u64,
        offset: u64,
        bytes: usize,
    },
    SaveValue {
        handle: u64,
        path: PathBuf,
    },
    Definition(String),
    Execute {
        sql: String,
        parameters: Vec<Value>,
        budget: Budget,
    },
    ExecuteBound {
        sql: String,
        parameters: Bindings,
        budget: Budget,
    },
    Explain {
        sql: String,
        parameters: Bindings,
        bytecode: bool,
    },
    Inspect(String),
    Diagram,
    Lookup {
        table: String,
        keys: Vec<(String, Value)>,
    },
    References {
        table: String,
        key: tursobench_core::ForeignKey,
        values: Vec<Value>,
    },
    Peek {
        table: String,
        key: tursobench_core::ForeignKey,
        values: Vec<Value>,
    },
    ReferenceCount {
        table: String,
        key: tursobench_core::ForeignKey,
        values: Vec<Value>,
    },
    Count(String),
    CountFiltered(Browse),
    Diagnostic(Diagnostic),
    Schema,
    Browse(Browse),
    CommitEdits(DraftBatch),
    Close,
}
impl Operation {
    fn bytes(&self) -> usize {
        let values = |v: &[Value]| v.iter().map(Value::bytes).sum::<usize>();
        std::mem::size_of::<Self>()
            + match self {
                Self::Execute {
                    sql, parameters, ..
                } => sql.len() + values(parameters),
                Self::ExportQuery {
                    path,
                    sql,
                    parameters,
                } => path.as_os_str().len() + sql.len() + parameters.bytes(),
                Self::ExportTable { path, spec } => {
                    path.as_os_str().len()
                        + spec.table.len()
                        + spec.filters.iter().map(|f| f.value.bytes()).sum::<usize>()
                        + spec
                            .options
                            .predicate
                            .as_ref()
                            .map_or(0, |p| p.bytes().min(COMMAND_BYTES))
                }
                Self::Import(spec) => {
                    spec.path.as_os_str().len()
                        + spec.table.len()
                        + spec
                            .mapping
                            .iter()
                            .chain(&spec.source_columns)
                            .map(String::len)
                            .sum::<usize>()
                }
                Self::Script {
                    sql, parameters, ..
                }
                | Self::ExecuteBound {
                    sql, parameters, ..
                }
                | Self::Explain {
                    sql, parameters, ..
                } => sql.len() + parameters.bytes(),
                Self::Browse(b) | Self::CountFiltered(b) => {
                    b.table.len()
                        + b.filters.iter().map(|f| f.value.bytes()).sum::<usize>()
                        + b.options
                            .predicate
                            .as_ref()
                            .map_or(0, |e| e.bytes().min(COMMAND_BYTES))
                        + b.options.cursor.as_ref().map_or(0, |row| values(row))
                        + b.options.relation.as_deref().map_or(0, |(key, v)| {
                            key.target.len()
                                + values(v)
                                + key
                                    .columns
                                    .iter()
                                    .map(|(a, b)| a.len() + b.as_ref().map_or(0, String::len))
                                    .sum::<usize>()
                        })
                }
                Self::CommitEdits(batch) => {
                    batch.table.name.len()
                        + batch
                            .table
                            .columns
                            .iter()
                            .map(|c| {
                                c.name.len()
                                    + c.declared_type.len()
                                    + c.default_sql.as_ref().map_or(0, String::len)
                            })
                            .sum::<usize>()
                        + batch
                            .edits
                            .iter()
                            .map(|edit| match edit {
                                tursobench_core::Edit::Insert { values: v } => {
                                    v.iter().flatten().map(Value::bytes).sum()
                                }
                                tursobench_core::Edit::Update { original, changes } => {
                                    values(original)
                                        + changes.iter().map(|(_, v)| v.bytes()).sum::<usize>()
                                }
                                tursobench_core::Edit::Delete { original } => values(original),
                            })
                            .sum::<usize>()
                }
                Self::Lookup { table, keys } => {
                    table.len() + keys.iter().map(|(k, v)| k.len() + v.bytes()).sum::<usize>()
                }
                Self::References {
                    table,
                    key,
                    values: v,
                }
                | Self::ReferenceCount {
                    table,
                    key,
                    values: v,
                }
                | Self::Peek {
                    table,
                    key,
                    values: v,
                } => {
                    table.len()
                        + values(v)
                        + key.target.len()
                        + key.on_update.len()
                        + key.on_delete.len()
                        + key
                            .columns
                            .iter()
                            .map(|(a, b)| a.len() + b.as_ref().map_or(0, String::len))
                            .sum::<usize>()
                }
                Self::PreviewImport {
                    path,
                    kinds,
                    null_marker,
                    ..
                } => path.as_os_str().len() + kinds.len() + null_marker.len(),
                Self::SaveValue { path, .. } | Self::BackupAndClose(path) => path.as_os_str().len(),
                Self::Inspect(s) | Self::Count(s) | Self::Definition(s) => s.len(),
                _ => 0,
            }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Diagnostic {
    DatabaseList,
    JournalMode,
    PageCount,
    FreelistCount,
    Integrity,
    ForeignKeys,
}
impl Diagnostic {
    pub const ALL: [Self; 6] = [
        Self::DatabaseList,
        Self::JournalMode,
        Self::PageCount,
        Self::FreelistCount,
        Self::Integrity,
        Self::ForeignKeys,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::DatabaseList => "Database files",
            Self::JournalMode => "Journal mode",
            Self::PageCount => "Page count",
            Self::FreelistCount => "Free pages",
            Self::Integrity => "Integrity check (may be slow)",
            Self::ForeignKeys => "Foreign-key check (may be slow)",
        }
    }
    fn sql(self) -> &'static str {
        match self {
            Self::DatabaseList => "PRAGMA database_list",
            Self::JournalMode => "PRAGMA journal_mode",
            Self::PageCount => "PRAGMA page_count",
            Self::FreelistCount => "PRAGMA freelist_count",
            Self::Integrity => "PRAGMA integrity_check",
            Self::ForeignKeys => "PRAGMA foreign_key_check",
        }
    }
}
pub struct Request {
    pub ticket: Ticket,
    pub operation: Operation,
}
pub struct Session {
    pub id: u64,
    commands: SyncSender<Request>,
    pub events: Receiver<Event>,
    discard: Arc<DeliveryControl>,
    closing: Arc<AtomicBool>,
}
impl Session {
    pub fn open(
        path: PathBuf,
        access: Access,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, Failure> {
        Self::open_with_settings(path, access, ConnectionSettings::default(), wake)
    }
    pub fn open_with_settings(
        path: PathBuf,
        access: Access,
        settings: ConnectionSettings,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, Failure> {
        if settings.query_timeout_ms > 3_600_000 {
            return Err(Failure::input("Statement timeout must be 0–3600000 ms"));
        }
        if settings.busy_timeout_ms > 5000 {
            return Err(Failure::input("Busy timeout must be 0–5000 ms"));
        }
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let (commands, rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let discard = Arc::new(DeliveryControl::default());
        let closing = Arc::new(AtomicBool::new(false));
        let output = Output {
            capture: None,
            tx,
            wake: Arc::new(wake),
            discard: discard.clone(),
            closing: closing.clone(),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .map_err(|error| Failure {
                kind: ErrorKind::Io,
                message: error.to_string(),
            })?;
        std::thread::Builder::new()
            .name(format!("turso-session-{id}"))
            .spawn(move || {
                runtime.block_on(worker(id, path, access, settings, rx, output));
            })
            .map_err(|e| Failure {
                kind: ErrorKind::Io,
                message: e.to_string(),
            })?;
        Ok(Self {
            id,
            commands,
            events,
            discard,
            closing,
        })
    }
    pub fn ticket(&self, request: u64, generation: u64) -> Ticket {
        Ticket {
            database: self.id,
            session: self.id,
            request,
            generation,
        }
    }
    pub fn submit(&self, request: Request) -> Result<(), Failure> {
        if self.closing.load(Ordering::Acquire) {
            return Err(Failure::input("Session is closing"));
        }
        if request.ticket.session != self.id || request.ticket.database != self.id {
            return Err(Failure::input("Wrong session identity"));
        }
        let mut pending = self.discard.requests.lock().unwrap();
        let id = request.ticket.request;
        if pending.contains_key(&id) {
            return Err(Failure::input("Request is already pending"));
        }
        let bytes = request.operation.bytes();
        if bytes > COMMAND_BYTES
            || pending
                .values()
                .map(|(_, bytes)| bytes)
                .sum::<usize>()
                .saturating_add(bytes)
                > COMMAND_BYTES
        {
            return Err(Failure::input(
                "Pending requests exceed the 16 MiB queue budget",
            ));
        }
        pending.insert(id, (false, bytes));
        if let Err(error) = self.commands.try_send(request) {
            pending.remove(&id);
            return Err(Failure::input(format!("Worker queue unavailable: {error}")));
        }
        Ok(())
    }
    pub fn discard_results(&self, request: u64) {
        self.discard.discard(request);
    }
    pub fn close(&self) {
        self.closing.store(true, Ordering::Release);
        if let Some(active) = &*self.discard.active_read.lock().unwrap() {
            (active.interrupt)();
        }
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
