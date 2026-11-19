//! Conservative external-change notifications; no uncommitted content is read.
use std::{
    path::{Path, PathBuf},
    sync::mpsc::{Receiver, RecvTimeoutError},
    time::{Duration, Instant, SystemTime},
};
use tursobench_core::{Coordination, ErrorKind, Failure};

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    identity: (u64, u64),
    changed: (i64, i64),
}
fn stamp(path: &Path) -> Result<Option<Stamp>, Failure> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Failure {
                kind: ErrorKind::Io,
                message: e.to_string(),
            });
        }
    };
    #[cfg(unix)]
    let (identity, changed) = {
        use std::os::unix::fs::MetadataExt;
        ((meta.dev(), meta.ino()), (meta.ctime(), meta.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let (identity, changed) = ((0, 0), (0, 0));
    Ok(Some(Stamp {
        length: meta.len(),
        modified: meta.modified().map_err(|e| Failure::input(e.to_string()))?,
        identity,
        changed,
    }))
}
async fn schema_version(c: &turso::Connection) -> Result<i64, Failure> {
    match crate::connection::pragma(c, "PRAGMA schema_version").await? {
        Some(turso::Value::Integer(version)) => Ok(version),
        Some(_) => Err(Failure::input("Invalid schema version")),
        None => Err(Failure::input("Schema version is unavailable")),
    }
}

pub(crate) struct Changes {
    path: PathBuf,
    wal: PathBuf,
    prior: (Option<Stamp>, Option<Stamp>),
    schema: i64,
}
impl Changes {
    pub(crate) async fn new(path: &Path, c: &turso::Connection) -> Result<Self, Failure> {
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        let schema = schema_version(c).await?;
        Ok(Self {
            path: path.into(),
            prior: (stamp(path)?, stamp(&wal)?),
            wal,
            schema,
        })
    }
    pub(crate) async fn poll(&mut self, c: &turso::Connection) -> Result<Option<bool>, Failure> {
        let next = (stamp(&self.path)?, stamp(&self.wal)?);
        if next.0.as_ref().map(|s| s.identity) != self.prior.0.as_ref().map(|s| s.identity) {
            return Err(Failure {
                kind: ErrorKind::Conflict,
                message: "Database file was removed or replaced. Reopen it before continuing."
                    .into(),
            });
        }
        if next == self.prior {
            return Ok(None);
        }
        // Browse stays in autocommit, preserving the editor's manual snapshot.
        let schema = schema_version(c).await?;
        let changed = schema != self.schema;
        self.schema = schema;
        self.prior = next;
        Ok(Some(changed))
    }
}

const INTERVAL: Duration = Duration::from_millis(500);
/// Paces change observation; only shared-WAL sessions observe external writers.
pub(crate) struct Watch {
    changes: Option<Changes>,
    observed: Instant,
}
impl Watch {
    pub(crate) async fn start(
        path: &Path,
        c: &turso::Connection,
        coordination: Coordination,
    ) -> Result<Self, Failure> {
        let changes = match coordination {
            Coordination::SharedWal => Some(Changes::new(path, c).await?),
            Coordination::ProtectedReadOnly => None,
        };
        Ok(Self {
            changes,
            observed: Instant::now(),
        })
    }
    /// Once per interval, `Some(schema)` when committed content changed.
    /// Busy or interrupted observations wait for the next interval.
    pub(crate) async fn poll(&mut self, c: &turso::Connection) -> Result<Option<bool>, Failure> {
        if self.observed.elapsed() < INTERVAL {
            return Ok(None);
        }
        self.observed = Instant::now();
        let Some(changes) = &mut self.changes else {
            return Ok(None);
        };
        match changes.poll(c).await {
            Err(e) if matches!(e.kind, ErrorKind::Busy | ErrorKind::Interrupted) => Ok(None),
            result => result,
        }
    }
    /// Protected read-only sessions need no timer. Shared sessions wake only
    /// for a command or the next external-change observation deadline.
    pub(crate) fn receive<T>(&self, rx: &Receiver<T>) -> Result<T, RecvTimeoutError> {
        if self.changes.is_some() {
            rx.recv_timeout(INTERVAL.saturating_sub(self.observed.elapsed()))
        } else {
            rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
        }
    }
}
