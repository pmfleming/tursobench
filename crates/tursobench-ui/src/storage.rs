use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CachedDatabase {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RecentDatabase {
    pub path: PathBuf,
    pub last_opened: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscoveryPreferences {
    pub enabled: bool,
    pub roots: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
    pub recent: Vec<RecentDatabase>,
    pub cached: Vec<CachedDatabase>,
}
impl Default for DiscoveryPreferences {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: vec![],
            excluded: vec![],
            recent: vec![],
            cached: vec![],
        }
    }
}
impl DiscoveryPreferences {
    pub fn record_open(&mut self, path: PathBuf, last_opened: u64) {
        let Some(path) = normalized_absolute(&path) else {
            return;
        };
        self.recent.retain(|entry| entry.path != path);
        self.recent.insert(0, RecentDatabase { path, last_opened });
        self.normalize();
    }
    pub fn remember(&mut self, entry: CachedDatabase) {
        self.cached.retain(|old| old.path != entry.path);
        self.cached.insert(0, entry);
        self.normalize();
    }
    pub fn normalize(&mut self) {
        normalize_paths(&mut self.roots, 32, 32 * 1024, false);
        normalize_paths(&mut self.excluded, 64, 32 * 1024, true);
        let mut seen = HashSet::new();
        let mut bytes = 0;
        self.recent.retain_mut(|entry| {
            let Some(path) = persistable_path(&entry.path) else {
                return false;
            };
            bytes += path.as_os_str().len();
            entry.path = path;
            bytes <= 64 * 1024 && seen.insert(entry.path.clone())
        });
        self.recent.truncate(50);
        seen.clear();
        bytes = 0;
        self.cached.retain_mut(|entry| {
            let Some(path) = persistable_path(&entry.path) else {
                return false;
            };
            bytes += path.as_os_str().len();
            entry.path = path;
            bytes <= 256 * 1024 && seen.insert(entry.path.clone())
        });
        self.cached.truncate(200);
    }
}

/// Lexical normalization only: preferences and UI construction never stat paths
/// (a path could refer to a sleeping disk or an automount).
pub fn normalized_absolute(path: &Path) -> Option<PathBuf> {
    use std::path::Component;
    if !path.is_absolute() {
        return None;
    }
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return None;
                }
            }
            other => result.push(other),
        }
    }
    Some(result)
}
fn persistable_path(path: &Path) -> Option<PathBuf> {
    let normalized = normalized_absolute(path)?;
    normalized.to_str().filter(|s| s.len() <= 4096)?;
    Some(normalized)
}
fn normalize_paths(paths: &mut Vec<PathBuf>, count: usize, byte_limit: usize, allow_root: bool) {
    let mut seen = HashSet::new();
    let mut bytes = 0;
    paths.retain_mut(|path| {
        let Some(normalized) = persistable_path(path) else {
            return false;
        };
        if !allow_root && normalized.parent().is_none() {
            return false;
        }
        bytes += normalized.as_os_str().len();
        *path = normalized;
        bytes <= byte_limit && seen.insert(path.clone())
    });
    paths.truncate(count);
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub database: String,
    pub sql: String,
    pub elapsed_ms: Option<u64>,
    pub failed: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub database: String,
    pub name: String,
    pub sql: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub history: Vec<String>,
    pub bookmarks: Vec<String>,
    pub row_limit: usize,
    pub busy_timeout_ms: u64,
    pub query_history: Vec<HistoryEntry>,
    pub saved_queries: Vec<Bookmark>,
    pub dark_mode: bool,
    pub slow_query_ms: u64,
    pub null_display: String,
    pub discovery: DiscoveryPreferences,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            history: vec![],
            bookmarks: vec![],
            row_limit: 1000,
            busy_timeout_ms: 250,
            query_history: vec![],
            saved_queries: vec![],
            dark_mode: true,
            slow_query_ms: 500,
            null_display: "NULL".into(),
            discovery: DiscoveryPreferences::default(),
        }
    }
}
impl Preferences {
    pub fn record_query(
        &mut self,
        database: &str,
        sql: &str,
        elapsed_ms: Option<u64>,
        failed: bool,
    ) {
        if sql.len() > 64 * 1024 || database.len() > 4096 {
            return;
        }
        self.query_history
            .retain(|h| h.sql != sql || h.database != database);
        self.query_history.insert(
            0,
            HistoryEntry {
                database: database.into(),
                sql: sql.into(),
                elapsed_ms,
                failed,
            },
        );
        self.normalize();
    }
    pub fn bookmark(&mut self, database: &str, name: &str, sql: &str) {
        self.saved_queries.insert(
            0,
            Bookmark {
                database: database.into(),
                name: name.into(),
                sql: sql.into(),
            },
        );
        self.normalize();
    }
    fn migrate_legacy(&mut self) {
        for sql in std::mem::take(&mut self.history).into_iter().rev() {
            self.record_query("", &sql, None, false);
        }
        for sql in std::mem::take(&mut self.bookmarks).into_iter().rev() {
            let name: String = sql.chars().take(60).collect();
            self.bookmark("", &name, &sql);
        }
    }
    pub fn normalize(&mut self) {
        self.discovery.normalize();
        self.row_limit = self.row_limit.clamp(1, 10_000);
        self.busy_timeout_ms = self.busy_timeout_ms.min(5000);
        self.slow_query_ms = self.slow_query_ms.clamp(1, 60_000);
        self.null_display = self.null_display.chars().take(32).collect();
        self.query_history
            .retain(|h| h.sql.len() <= 64 * 1024 && h.database.len() <= 4096);
        self.query_history.truncate(100);
        while self
            .query_history
            .iter()
            .map(|h| h.sql.len() + h.database.len())
            .sum::<usize>()
            > 512 * 1024
        {
            self.query_history.pop();
        }
        self.saved_queries.retain(|b| {
            b.sql.len() <= 64 * 1024 && b.database.len() <= 4096 && b.name.len() <= 256
        });
        self.saved_queries.truncate(100);
        while self
            .saved_queries
            .iter()
            .map(|b| b.sql.len() + b.database.len() + b.name.len())
            .sum::<usize>()
            > 512 * 1024
        {
            self.saved_queries.pop();
        }
        for list in [&mut self.history, &mut self.bookmarks] {
            list.retain(|s| s.len() <= 64 * 1024);
            list.truncate(100);
            while list.iter().map(String::len).sum::<usize>() > 512 * 1024 {
                list.pop();
            }
        }
    }
}
pub struct Store {
    tx: SyncSender<Preferences>,
    pub results: Receiver<Result<(), String>>,
    pending: Option<Preferences>,
}
impl Store {
    pub fn start(wake: impl Fn() + Send + 'static) -> (Self, Preferences, Option<String>) {
        let path = if cfg!(test) {
            Err("Preference persistence disabled in UI tests".into())
        } else {
            config_path()
        };
        let loaded = path.as_ref().map_err(Clone::clone).and_then(|p| load(p));
        let (prefs, error) = match loaded {
            Ok(p) => (p, None),
            Err(e) => (Preferences::default(), Some(e)),
        };
        let (tx, rx) = mpsc::sync_channel::<Preferences>(1);
        let (done, results) = mpsc::channel();
        std::thread::spawn(move || {
            while let Ok(prefs) = rx.recv() {
                let result = path
                    .as_ref()
                    .map_err(Clone::clone)
                    .and_then(|path| save(path, &prefs));
                if done.send(result).is_err() {
                    break;
                }
                wake();
            }
        });
        (
            Self {
                tx,
                results,
                pending: None,
            },
            prefs,
            error,
        )
    }
    pub fn save(&mut self, prefs: &Preferences) {
        self.pending = Some(prefs.clone());
        self.flush();
    }
    pub fn flush(&mut self) {
        if let Some(prefs) = self.pending.take() {
            match self.tx.try_send(prefs) {
                Ok(()) => {}
                Err(TrySendError::Full(p)) => self.pending = Some(p),
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }
}
fn config_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or("No application configuration directory available")?;
    Ok(root.join("tursobench/preferences.json"))
}
fn load(path: &Path) -> Result<Preferences, String> {
    if !path.exists() {
        return Ok(Preferences::default());
    }
    if path.metadata().map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
        return Err("Preferences exceed 2 MiB".into());
    }
    let mut p: Preferences =
        serde_json::from_slice(&std::fs::read(path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    p.migrate_legacy();
    p.normalize();
    Ok(p)
}
fn save(path: &Path, prefs: &Preferences) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing preferences directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    serde_json::to_writer_pretty(&mut temp, prefs).map_err(|e| e.to_string())?;
    temp.flush().map_err(|e| e.to_string())?;
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn discovery_preferences_migrate_bound_and_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        fs_write(
            &path,
            r#"{"dark_mode":false,"discovery":{"enabled":false}}"#,
        );
        let mut prefs = load(&path).unwrap();
        assert!(!prefs.discovery.enabled);
        assert!(prefs.discovery.cached.is_empty());
        prefs.discovery.roots = vec![
            "/".into(),
            "relative".into(),
            "/home/test/Projects/../Code".into(),
            "/home/test/Code".into(),
        ];
        prefs.discovery.excluded = vec!["/home/test/Code/build".into()];
        for i in 0..250 {
            let db = PathBuf::from(format!("/home/test/{i}.db"));
            prefs.discovery.record_open(db.clone(), i);
            prefs.discovery.remember(CachedDatabase {
                path: db,
                size: 100,
                modified: Some(i),
            });
        }
        prefs.discovery.record_open("/home/test/249.db".into(), 999);
        prefs.discovery.record_open("relative.db".into(), 1000);
        prefs.normalize();
        assert_eq!(prefs.discovery.recent.len(), 50);
        assert_eq!(prefs.discovery.cached.len(), 200);
        assert_eq!(
            prefs.discovery.roots,
            vec![PathBuf::from("/home/test/Code")]
        );
        assert_eq!(prefs.discovery.recent[0].last_opened, 999);
        save(&path, &prefs).unwrap();
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.discovery.cached, prefs.discovery.cached);
        assert_eq!(loaded.discovery.recent, prefs.discovery.recent);
        assert_eq!(loaded.discovery.excluded, prefs.discovery.excluded);
        fs_write(&path, r#"{"dark_mode":true}"#);
        assert!(load(&path).unwrap().discovery.enabled);
    }
    fn fs_write(path: &Path, text: &str) {
        std::fs::write(path, text).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn discovery_does_not_persist_non_utf8_paths_or_escape_exclusions_with_parent_components() {
        use std::os::unix::ffi::OsStringExt;
        let mut prefs = Preferences::default();
        prefs.discovery.record_open(
            std::ffi::OsString::from_vec(b"/home/test/odd-\xff.db".to_vec()).into(),
            1,
        );
        prefs
            .discovery
            .roots
            .push("/home/test/Code/../Private".into());
        prefs.normalize();
        assert!(prefs.discovery.recent.is_empty());
        assert_eq!(
            prefs.discovery.roots,
            vec![PathBuf::from("/home/test/Private")]
        );
        assert!(serde_json::to_vec(&prefs).is_ok());
    }
    #[test]
    fn bounded_atomic_application_storage() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut prefs = Preferences::default();
        for i in 0..150 {
            prefs.record_query("db-a", &format!("SELECT {i}"), Some(i), false);
        }
        prefs.record_query("db-a", "SELECT 149", Some(1000), true);
        assert_eq!(prefs.query_history.len(), 100);
        prefs.bookmark("db-a", "Quoted", "SELECT ';'");
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().saved_queries[0].sql, "SELECT ';'");
        std::fs::write(
            &path,
            r#"{"history":["SELECT 1"],"bookmarks":["SELECT 2"]}"#,
        )
        .unwrap();
        let legacy = load(&path).unwrap();
        assert_eq!(legacy.query_history[0].database, "");
        assert_eq!(legacy.saved_queries[0].sql, "SELECT 2");
        std::fs::write(&path, "bad JSON").unwrap();
        assert!(load(&path).is_err());
    }
}
