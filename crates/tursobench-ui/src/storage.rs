use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
    io::{Read, Write},
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
    pub query_timeout_ms: u64,
    pub query_history: Vec<HistoryEntry>,
    pub saved_queries: Vec<Bookmark>,
    pub dark_mode: bool,
    pub font_size: u16,
    #[serde(default)]
    pub appearance: Option<crate::appearance::Mode>,
    #[serde(skip)]
    pub legacy_appearance: bool,
    pub slow_query_ms: u64,
    pub null_display: String,
    pub discovery: DiscoveryPreferences,
    pub map_layouts: Vec<MapLayout>,
    pub(crate) workspace_layouts: Vec<crate::app::workspace::Snapshot>,
    pub inspector_visibility: std::collections::BTreeMap<String, bool>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MapLayout {
    pub database: String,
    pub positions: std::collections::BTreeMap<String, [f32; 2]>,
    pub pan: [f32; 2],
    pub zoom: f32,
    #[serde(default)]
    pub pack_width: f32,
    pub all_columns: bool,
    pub selected: Option<String>,
}
impl Default for Preferences {
    fn default() -> Self {
        Self {
            history: vec![],
            bookmarks: vec![],
            row_limit: 1000,
            busy_timeout_ms: 250,
            query_timeout_ms: 30_000,
            query_history: vec![],
            saved_queries: vec![],
            dark_mode: true,
            font_size: 14,
            appearance: Some(crate::appearance::Mode::System),
            legacy_appearance: false,
            slow_query_ms: 500,
            null_display: "NULL".into(),
            discovery: DiscoveryPreferences::default(),
            map_layouts: vec![],
            workspace_layouts: vec![],
            inspector_visibility: Default::default(),
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
        if self.appearance.is_none() {
            self.appearance = Some(crate::appearance::Mode::from_dark(self.dark_mode));
            self.legacy_appearance = true;
        }
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
        self.inspector_visibility
            .retain(|path, _| path.len() <= 4096 && Path::new(path).is_absolute());
        while self.inspector_visibility.len() > 20 {
            self.inspector_visibility.pop_first();
        }
        self.workspace_layouts.truncate(20);
        let mut workspace_names = HashSet::new();
        let mut workspace_bytes = 0;
        self.workspace_layouts.retain(|layout| {
            workspace_bytes += layout.database.len()
                + layout
                    .views
                    .iter()
                    .map(|v| v.object.as_ref().map_or(0, String::len) + 64)
                    .sum::<usize>();
            layout.valid()
                && workspace_bytes <= 128 * 1024
                && workspace_names.insert(layout.database.clone())
        });
        self.map_layouts.truncate(20);
        let mut layout_bytes = 0;
        self.map_layouts.retain_mut(|layout| {
            if layout.database.len() > 4096
                || !Path::new(&layout.database).is_absolute()
                || !layout.pack_width.is_finite()
                || !(0.0..=3600.0).contains(&layout.pack_width)
                || !layout.zoom.is_finite()
                || !(0.0001..=2.5).contains(&layout.zoom)
                || layout
                    .pan
                    .iter()
                    .any(|n| !n.is_finite() || n.abs() > 10_000_000.0)
            {
                return false;
            }
            let mut count = 0;
            layout.positions.retain(|name, pos| {
                count += 1;
                layout_bytes += name.len() + 16;
                count <= 512
                    && name.len() <= 4096
                    && layout_bytes <= 256 * 1024
                    && pos.iter().all(|n| n.is_finite() && n.abs() <= 10_000_000.0)
            });
            if layout.selected.as_ref().is_some_and(|s| s.len() > 4096) {
                layout.selected = None;
            }
            layout_bytes += layout.database.len() + layout.selected.as_ref().map_or(0, String::len);
            layout_bytes <= 256 * 1024
        });
        self.row_limit = self.row_limit.clamp(1, 10_000);
        self.query_timeout_ms = self.query_timeout_ms.min(3_600_000);
        self.busy_timeout_ms = self.busy_timeout_ms.min(5000);
        self.font_size = self.font_size.clamp(10, 20);
        self.slow_query_ms = self.slow_query_ms.clamp(1, 60_000);
        self.null_display = self.null_display.chars().take(32).collect();
        self.query_history
            .retain(|h| h.sql.len() <= 64 * 1024 && h.database.len() <= 4096);
        retain_prefix(&mut self.query_history, |h| h.sql.len() + h.database.len());
        self.saved_queries.retain(|b| {
            b.sql.len() <= 64 * 1024 && b.database.len() <= 4096 && b.name.len() <= 256
        });
        retain_prefix(&mut self.saved_queries, |b| {
            b.sql.len() + b.database.len() + b.name.len()
        });
        for list in [&mut self.history, &mut self.bookmarks] {
            list.retain(|s| s.len() <= 64 * 1024);
            retain_prefix(list, String::len);
        }
    }
}
// Keep the most recent prefix in one pass, bounded by count and bytes.
fn retain_prefix<T>(items: &mut Vec<T>, size: impl Fn(&T) -> usize) {
    let mut bytes = 0;
    let keep = items
        .iter()
        .take(100)
        .take_while(|item| {
            bytes += size(item);
            bytes <= 512 * 1024
        })
        .count();
    items.truncate(keep);
}
pub struct Store {
    tx: SyncSender<Preferences>,
    pub results: Receiver<Result<(), String>>,
    pending: Option<Preferences>,
}
impl Store {
    pub fn start(wake: impl Fn() + Send + 'static) -> (Self, Preferences, Option<String>) {
        let mut path = if cfg!(test) {
            Err("Preference persistence disabled in UI tests".into())
        } else {
            config_path()
        };
        let loaded = path
            .as_ref()
            .map_err(Clone::clone)
            .and_then(|p| load_report(p));
        let (prefs, error) = match loaded {
            Ok(p) => p,
            Err(e) => {
                // Never overwrite unreadable or newer-version preferences with defaults.
                path = Err(format!("Preference saving disabled until recovery: {e}"));
                (Preferences::default(), Some(e))
            }
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
#[cfg(test)]
fn load(path: &Path) -> Result<Preferences, String> {
    load_report(path).map(|(prefs, _)| prefs)
}
fn read_preferences(path: &Path) -> Result<Vec<u8>, String> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .map_err(|e| e.to_string())?
        .take(2 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 2 * 1024 * 1024 {
        return Err("Preferences exceed 2 MiB".into());
    }
    Ok(bytes)
}
fn load_report(path: &Path) -> Result<(Preferences, Option<String>), String> {
    if !path.exists() {
        return Ok((Preferences::default(), None));
    }
    if path.metadata().map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
        return Err("Preferences exceed 2 MiB".into());
    }
    let original = read_preferences(path)?;
    let mut data: serde_json::Value =
        serde_json::from_slice(&original).map_err(|e| e.to_string())?;
    let fields = data
        .as_object_mut()
        .ok_or("Preferences must be an object")?;
    if fields
        .get("format_version")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0)
        > 1
    {
        return Err("Preferences use a newer format version".into());
    }
    let mut invalid = Vec::new();
    for (key, value) in fields.clone() {
        let candidate = serde_json::json!({key.clone(): value.clone()});
        if serde_json::from_value::<Preferences>(candidate).is_err() {
            invalid.push(key.clone());
            if let Some(entries) = value.as_array() {
                let valid = entries
                    .iter()
                    .filter(|entry| {
                        serde_json::from_value::<Preferences>(
                            serde_json::json!({key.clone(): [entry]}),
                        )
                        .is_ok()
                    })
                    .cloned()
                    .collect::<Vec<_>>();
                fields.insert(key, valid.into());
            } else {
                fields.remove(&key);
            }
        }
    }
    let mut p: Preferences = serde_json::from_value(data).map_err(|e| e.to_string())?;
    let warning = if invalid.is_empty() {
        None
    } else {
        let mut backup =
            tempfile::NamedTempFile::new_in(path.parent().ok_or("Missing preferences directory")?)
                .map_err(|e| e.to_string())?;
        backup.write_all(&original).map_err(|e| e.to_string())?;
        backup.as_file().sync_all().map_err(|e| e.to_string())?;
        let (_, saved) = backup.keep().map_err(|e| e.to_string())?;
        Some(format!(
            "Recovered preferences; reset invalid fields {}. Original saved at {}",
            invalid.join(", "),
            saved.display()
        ))
    };
    p.migrate_legacy();
    p.normalize();
    Ok((p, warning))
}
fn save(path: &Path, prefs: &Preferences) -> Result<(), String> {
    let parent = path.parent().ok_or("Missing preferences directory")?;
    std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let mut temp = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
    let mut data = serde_json::to_value(prefs).map_err(|e| e.to_string())?;
    data["format_version"] = 1.into();
    serde_json::to_writer_pretty(&mut temp, &data).map_err(|e| e.to_string())?;
    temp.flush().map_err(|e| e.to_string())?;
    if temp.as_file().metadata().map_err(|e| e.to_string())?.len() > 2 * 1024 * 1024 {
        return Err("Preferences exceed 2 MiB; existing file preserved".into());
    }
    temp.as_file().sync_all().map_err(|e| e.to_string())?;
    if path.exists() {
        let original = read_preferences(path)?;
        let existing:serde_json::Value=serde_json::from_slice(&original).map_err(|e|format!("Preferences changed to unreadable JSON; original and previous backup preserved: {e}"))?;
        if !existing.is_object() || existing["format_version"].as_u64().is_some_and(|n| n > 1) {
            return Err("Preferences changed to an unsupported format; original preserved".into());
        }
        let mut backup = tempfile::NamedTempFile::new_in(parent).map_err(|e| e.to_string())?;
        backup.write_all(&original).map_err(|e| e.to_string())?;
        backup.as_file().sync_all().map_err(|e| e.to_string())?;
        backup
            .persist(path.with_extension("json.backup"))
            .map_err(|e| e.to_string())?;
    }
    temp.persist(path).map_err(|e| e.to_string())?;
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn runtime_corruption_does_not_overwrite_the_file_or_previous_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let prefs = Preferences::default();
        save(&path, &prefs).unwrap();
        save(&path, &prefs).unwrap();
        let backup = std::fs::read(path.with_extension("json.backup")).unwrap();
        fs_write(&path, "{broken");
        assert!(save(&path, &prefs).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "{broken");
        assert_eq!(
            std::fs::read(path.with_extension("json.backup")).unwrap(),
            backup
        );
        fs_write(&path, r#"{"format_version":999}"#);
        assert!(save(&path, &prefs).is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"format_version":999}"#
        );
        assert_eq!(
            std::fs::read(path.with_extension("json.backup")).unwrap(),
            backup
        );
    }
    #[test]
    fn oversized_preferences_never_replace_a_loadable_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let mut prefs = Preferences::default();
        save(&path, &prefs).unwrap();
        let original = std::fs::read(&path).unwrap();
        prefs.null_display = "x".repeat(2 * 1024 * 1024);
        assert!(save(&path, &prefs).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), original);
        assert!(load(&path).is_ok());
    }
    #[test]
    fn recovery_keeps_valid_entries_and_preserves_the_original() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        let original = r#"{"font_size":"bad","saved_queries":[{"database":"db","name":"q","sql":"SELECT 1"},{"unknown":"bad"}]}"#;
        fs_write(&path, original);
        let (prefs, warning) = load_report(&path).unwrap();
        assert!(warning.unwrap().contains("Original saved at"));
        assert_eq!(prefs.saved_queries.len(), 1);
        assert_eq!(prefs.font_size, 14);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), original);
        save(&path, &prefs).unwrap();
        assert_eq!(
            std::fs::read_to_string(path.with_extension("json.backup")).unwrap(),
            original
        );
        fs_write(&path, r#"{"format_version":999}"#);
        assert!(load_report(&path).is_err());
    }
    #[test]
    fn font_size_migrates_clamps_and_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        fs_write(&path, "{}");
        let mut prefs = load(&path).unwrap();
        assert_eq!(prefs.font_size, 14);
        prefs.font_size = 18;
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().font_size, 18);
        for (value, expected) in [(0, 10), (100, 20)] {
            fs_write(&path, &format!("{{\"font_size\":{value}}}"));
            assert_eq!(load(&path).unwrap().font_size, expected);
        }
    }

    #[test]
    fn appearance_defaults_to_system_and_migrates_explicit_legacy_choices() {
        use crate::appearance::Mode;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("prefs.json");
        assert_eq!(load(&path).unwrap().appearance, Some(Mode::System));
        fs_write(&path, r#"{"dark_mode":false}"#);
        let mut prefs = load(&path).unwrap();
        assert_eq!(prefs.appearance, Some(Mode::Light));
        assert!(prefs.legacy_appearance);
        prefs.appearance = Some(Mode::System);
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().appearance, Some(Mode::System));
    }
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
    #[test]
    fn map_layouts_round_trip_and_drop_invalid_geometry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let layout = MapLayout {
            database: "/home/test/map.db".into(),
            positions: [("table".into(), [32.0, 64.0])].into(),
            pan: [10.0, 20.0],
            zoom: 0.75,
            pack_width: 1200.0,
            all_columns: true,
            selected: Some("table".into()),
        };
        let mut prefs = Preferences::default();
        prefs.map_layouts.push(layout.clone());
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().map_layouts, vec![layout.clone()]);
        let mut invalid = layout.clone();
        invalid.zoom = f32::NAN;
        prefs.map_layouts.push(invalid);
        prefs.map_layouts[0]
            .positions
            .insert("invalid".into(), [f32::INFINITY, 0.0]);
        prefs.normalize();
        assert_eq!(prefs.map_layouts, vec![layout]);
        fs_write(&path, "{}");
        assert!(load(&path).unwrap().map_layouts.is_empty());
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
        prefs
            .inspector_visibility
            .insert("/home/test/a.db".into(), false);
        save(&path, &prefs).unwrap();
        assert_eq!(load(&path).unwrap().saved_queries[0].sql, "SELECT ';'");
        std::fs::write(
            &path,
            r#"{"history":["SELECT 1"],"bookmarks":["SELECT 2"]}"#,
        )
        .unwrap();
        let legacy = load(&path).unwrap();
        assert!(legacy.inspector_visibility.is_empty());
        assert_eq!(legacy.query_history[0].database, "");
        assert_eq!(legacy.saved_queries[0].sql, "SELECT 2");
        std::fs::write(&path, "bad JSON").unwrap();
        assert!(load(&path).is_err());
    }
}
