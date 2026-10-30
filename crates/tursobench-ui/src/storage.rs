use serde::{Deserialize, Serialize};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::mpsc::{self, Receiver, SyncSender, TrySendError},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub history: Vec<String>,
    pub bookmarks: Vec<String>,
    pub row_limit: usize,
    pub busy_timeout_ms: u64,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            history: vec![],
            bookmarks: vec![],
            row_limit: 1000,
            busy_timeout_ms: 250,
        }
    }
}
impl Preferences {
    pub fn record(&mut self, sql: &str) {
        if sql.len() > 64 * 1024 {
            return;
        }
        self.history.retain(|s| s != sql);
        self.history.insert(0, sql.into());
        self.normalize();
    }
    pub fn normalize(&mut self) {
        self.row_limit = self.row_limit.clamp(1, 10_000);
        self.busy_timeout_ms = self.busy_timeout_ms.min(5000);
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
            prefs.record(&format!("SELECT {i}"));
        }
        prefs.record("SELECT 149");
        assert_eq!(prefs.history.len(), 100);
        prefs.bookmarks.push("SELECT ';'".into());
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().bookmarks, prefs.bookmarks);
        std::fs::write(&path, "bad JSON").unwrap();
        assert!(load(&path).is_err());
    }
}
