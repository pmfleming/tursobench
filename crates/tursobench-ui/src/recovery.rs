//! Bounded recovery snapshots, owned by a worker; no user-database access.
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{
        Arc,
        mpsc::{self, Receiver, SyncSender, TrySendError},
    },
};

const MAX_BYTES: usize = 32 * 1024 * 1024;
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    pub id: u64,
    pub sql: String,
    pub selection: std::ops::Range<usize>,
}
#[derive(Clone, Serialize, Deserialize)]
struct Snapshot {
    version: u32,
    databases: BTreeMap<String, Vec<Document>>,
}
impl Default for Snapshot {
    fn default() -> Self {
        Self {
            version: 1,
            databases: BTreeMap::new(),
        }
    }
}
fn validate(s: &Snapshot) -> Result<(), String> {
    if s.version != 1
        || s.databases.len() > 64
        || s.databases.iter().any(|(key, docs)| {
            key.len() > 4096
                || docs.len() > 16
                || docs.iter().any(|d| {
                    d.sql.len() > 1024 * 1024
                        || d.selection.start > d.selection.end
                        || d.selection.end > d.sql.chars().count()
                })
                || docs
                    .iter()
                    .map(|d| d.id)
                    .collect::<std::collections::BTreeSet<_>>()
                    .len()
                    != docs.len()
        })
    {
        return Err(
            "Unsupported or oversized SQL recovery snapshot; previous data preserved".into(),
        );
    }
    Ok(())
}
fn read(path: &Path) -> Result<Snapshot, String> {
    let file = match std::fs::File::open(path) {
        Ok(f) => f,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Snapshot::default()),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take((MAX_BYTES + 1) as u64)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("SQL recovery exceeds 32 MiB".into());
    }
    let s = serde_json::from_slice(&bytes).map_err(|e| format!("SQL recovery: {e}"))?;
    validate(&s)?;
    Ok(s)
}
fn write(path: &Path, s: &Snapshot) -> Result<(), String> {
    validate(s)?;
    let bytes = serde_json::to_vec(s).map_err(|e| e.to_string())?;
    if bytes.len() > MAX_BYTES {
        return Err("SQL recovery exceeds 32 MiB; previous save preserved".into());
    }
    let parent = path.parent().ok_or("No recovery directory")?;
    let mut file = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    file.write_all(&bytes)
        .and_then(|()| file.as_file().sync_all())
        .map_err(|e| e.to_string())?;
    file.persist(path).map_err(|e| e.to_string())?;
    std::fs::File::open(parent)
        .and_then(|f| f.sync_all())
        .map_err(|e| e.to_string())
}
enum Command {
    Save(BTreeMap<String, Vec<Document>>),
    Clear,
}
pub struct Store {
    tx: Option<SyncSender<Command>>,
    results: Receiver<Result<Arc<Snapshot>, String>>,
    pending: BTreeMap<String, Vec<Document>>,
    clear: bool,
    snapshot: Arc<Snapshot>,
    pub error: Option<String>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Store {
    pub fn start(wake: impl Fn() + Send + 'static) -> Self {
        let path = if cfg!(test) {
            None
        } else {
            crate::storage::config_path()
                .ok()
                .map(|p| p.with_file_name("sql-recovery.json"))
        };
        Self::start_path(path, wake)
    }
    fn start_path(path: Option<PathBuf>, wake: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::sync_channel(1);
        let (done, results) = mpsc::channel();
        let (initial, loaded) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            // Hold an OS lock so simultaneous application instances cannot replace each other's recovery.
            let lock = path
                .as_ref()
                .map(|p| -> Result<std::fs::File, String> {
                    let parent = p.parent().ok_or("Missing recovery directory")?;
                    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                    let f = std::fs::OpenOptions::new()
                        .create(true)
                        .truncate(false)
                        .read(true)
                        .write(true)
                        .open(p.with_extension("lock"))
                        .map_err(|e| e.to_string())?;
                    f.try_lock()
                        .map_err(|e| format!("SQL recovery is in use: {e}"))?;
                    Ok(f)
                })
                .transpose();
            let mut state = lock
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|_| {
                    path.as_ref()
                        .map_or_else(|| Ok(Snapshot::default()), |p| read(p))
                })
                .map(Arc::new);
            let _ = initial.send(state.clone());
            while let Ok(command) = rx.recv() {
                let result = state.as_ref().map_err(Clone::clone).and_then(|s| {
                    let unchanged = match &command {
                        Command::Save(batch) => batch
                            .iter()
                            .all(|(key, docs)| s.databases.get(key) == Some(docs)),
                        Command::Clear => s.databases.is_empty(),
                    };
                    if unchanged {
                        return Ok(s.clone());
                    }
                    let mut next = (**s).clone();
                    match command {
                        Command::Save(batch) => next.databases.extend(batch),
                        Command::Clear => next.databases.clear(),
                    }
                    if let Some(path) = &path {
                        write(path, &next)?;
                    } else {
                        validate(&next)?;
                    }
                    Ok(Arc::new(next))
                });
                if let Ok(next) = &result {
                    state = Ok(next.clone());
                }
                if done.send(result).is_err() {
                    break;
                }
                wake();
            }
        });
        let initial = loaded.recv().unwrap_or_else(|e| Err(e.to_string()));
        let (snapshot, error) = match initial {
            Ok(s) => (s, None),
            Err(e) => (Arc::default(), Some(e)),
        };
        Self {
            tx: Some(tx),
            results,
            pending: BTreeMap::new(),
            clear: false,
            snapshot,
            error,
            thread: Some(thread),
        }
    }
    pub fn documents(&self, database: &str) -> Vec<Document> {
        self.snapshot
            .databases
            .get(database)
            .cloned()
            .unwrap_or_default()
    }
    pub fn save(&mut self, database: String, docs: Vec<Document>) {
        self.pending.insert(database, docs);
        self.poll();
    }
    pub fn clear(&mut self) {
        self.pending.clear();
        self.clear = true;
        self.poll();
    }
    pub fn poll(&mut self) {
        while let Ok(result) = self.results.try_recv() {
            match result {
                Ok(s) => {
                    self.snapshot = s;
                    self.error = None;
                }
                Err(e) => self.error = Some(e),
            }
        }
        let Some(tx) = &self.tx else {
            return;
        };
        let command = if self.clear {
            Some(Command::Clear)
        } else {
            (!self.pending.is_empty()).then(|| Command::Save(std::mem::take(&mut self.pending)))
        };
        if let Some(command) = command {
            match tx.try_send(command) {
                Ok(()) => self.clear = false,
                Err(TrySendError::Full(Command::Save(batch))) => self.pending.extend(batch),
                Err(TrySendError::Full(Command::Clear)) => {}
                Err(TrySendError::Disconnected(_)) => {
                    self.error = Some("SQL recovery worker stopped".into())
                }
            }
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            if self.clear {
                let _ = tx.send(Command::Clear);
            }
            if !self.pending.is_empty() {
                let _ = tx.send(Command::Save(std::mem::take(&mut self.pending)));
            }
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn pending_databases_share_a_write_and_unchanged_saves_reuse_the_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        let (wake, received) = mpsc::channel();
        let mut store = Store::start_path(Some(path.clone()), move || {
            let _ = wake.send(());
        });
        let docs = vec![Document {
            id: 1,
            sql: "SELECT 1".into(),
            selection: 0..0,
        }];
        store.pending.insert("a".into(), docs.clone());
        store.pending.insert("b".into(), docs.clone());
        store.poll();
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        store.poll();
        assert_eq!(read(&path).unwrap().databases.len(), 2);
        let saved = store.snapshot.clone();
        store.save("a".into(), docs);
        received
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        store.poll();
        assert!(Arc::ptr_eq(&saved, &store.snapshot));
    }
    #[test]
    fn restart_deletion_and_invalid_save_preserve_acknowledged_data() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        let docs = vec![Document {
            id: 7,
            sql: "SELECT 'λ'".into(),
            selection: 0..2,
        }];
        let mut store = Store::start_path(Some(path.clone()), || {});
        store.save("db".into(), docs.clone());
        drop(store);
        assert_eq!(read(&path).unwrap().databases["db"], docs);
        let mut invalid = read(&path).unwrap();
        invalid.databases.get_mut("db").unwrap()[0].selection = 0..100;
        assert!(write(&path, &invalid).is_err());
        assert_eq!(read(&path).unwrap().databases["db"], docs);
        let mut store = Store::start_path(Some(path.clone()), || {});
        store.save("db".into(), vec![]);
        drop(store);
        assert!(read(&path).unwrap().databases["db"].is_empty());
        let mut store = Store::start_path(Some(path.clone()), || {});
        store.clear();
        drop(store);
        assert!(read(&path).unwrap().databases.is_empty());
    }
    #[test]
    fn corrupt_or_locked_store_is_not_overwritten() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("recovery.json");
        std::fs::write(&path, b"corrupt").unwrap();
        let mut store = Store::start_path(Some(path.clone()), || {});
        assert!(store.error.is_some());
        store.save("db".into(), vec![]);
        let other = Store::start_path(Some(path.clone()), || {});
        assert!(other.error.is_some());
        drop(other);
        drop(store);
        assert_eq!(std::fs::read(path).unwrap(), b"corrupt");
    }
}
