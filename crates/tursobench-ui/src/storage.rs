use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
};

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
