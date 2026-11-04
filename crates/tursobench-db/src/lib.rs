//! Turso-only session worker. No engine handles cross the service boundary.
use tursobench_core::{
    Access, Bindings, Browse, Budget, ConnectionSettings, DraftBatch, ErrorKind, Event, Failure,
    Ticket, Value,
};
mod browse;
mod connection;
mod diagram;
mod edits;
mod inspection;
pub mod sql;
mod validation;
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
const COMMAND_CAPACITY: usize = 2;
const EVENT_CAPACITY: usize = 8;

/// Only submitted, unfinished requests occupy this map. Cancellation targets one
/// request; a newer view must never suppress an older view's unrelated results.
#[derive(Default)]
struct DeliveryControl {
    requests: Mutex<std::collections::BTreeMap<u64, bool>>,
}
impl DeliveryControl {
    fn discarded(&self, request: u64) -> bool {
        self.requests
            .lock()
            .unwrap()
            .get(&request)
            .copied()
            .unwrap_or(false)
    }
    fn discard(&self, request: u64) {
        if let Some(discarded) = self.requests.lock().unwrap().get_mut(&request) {
            *discarded = true;
        }
    }
    fn finished(&self, request: u64) {
        self.requests.lock().unwrap().remove(&request);
    }
}

pub enum Operation {
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
    Diagnostic(Diagnostic),
    Schema,
    Browse(Browse),
    CommitEdits(DraftBatch),
    Close,
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
        exclusive_use_confirmed: bool,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, Failure> {
        Self::open_with_settings(
            path,
            access,
            exclusive_use_confirmed,
            ConnectionSettings::default(),
            wake,
        )
    }
    pub fn open_with_settings(
        path: PathBuf,
        access: Access,
        exclusive_use_confirmed: bool,
        settings: ConnectionSettings,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> Result<Self, Failure> {
        if settings.busy_timeout_ms > 5000 {
            return Err(Failure::input("Busy timeout must be 0–5000 ms"));
        }
        if !exclusive_use_confirmed {
            return Err(Failure::input(
                "Only databases not in use by other applications are supported",
            ));
        }
        let id = NEXT_SESSION.fetch_add(1, Ordering::Relaxed);
        let (commands, rx) = mpsc::sync_channel(COMMAND_CAPACITY);
        let (tx, events) = mpsc::sync_channel(EVENT_CAPACITY);
        let discard = Arc::new(DeliveryControl::default());
        let closing = Arc::new(AtomicBool::new(false));
        let output = Output {
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
        pending.insert(id, false);
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
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        self.close();
    }
}
