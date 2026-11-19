use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
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
    pub removed_roots: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
    pub recent: Vec<RecentDatabase>,
    pub cached: Vec<CachedDatabase>,
}
impl Default for DiscoveryPreferences {
    fn default() -> Self {
        Self {
            enabled: true,
            roots: vec![],
            removed_roots: vec![],
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
        normalize_paths(&mut self.removed_roots, 128, 512 * 1024, false);
        normalize_paths(&mut self.excluded, 64, 32 * 1024, true);
        normalize_entries(&mut self.recent, |e| &mut e.path, 50, 64 * 1024);
        normalize_entries(&mut self.cached, |e| &mut e.path, 200, 256 * 1024);
    }
}

/// Keep the first `count` entries with distinct persistable paths, within
/// `bytes` of path text.
fn normalize_entries<T>(
    entries: &mut Vec<T>,
    path: fn(&mut T) -> &mut PathBuf,
    count: usize,
    bytes: usize,
) {
    let mut seen = HashSet::new();
    let mut total = 0;
    entries.retain_mut(|entry| {
        let path = path(entry);
        let Some(persisted) = persistable_path(path) else {
            return false;
        };
        total += persisted.as_os_str().len();
        *path = persisted;
        total <= bytes && seen.insert(path.clone())
    });
    entries.truncate(count);
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
    pub sql_recovery: bool,
    pub database_limit: Option<usize>,
    pub memory_budget_mib: Option<u64>,
    pub row_limit: usize,
    pub busy_timeout_ms: u64,
    pub query_timeout_ms: u64,
    pub query_history: Vec<HistoryEntry>,
    pub saved_queries: Vec<Bookmark>,
    pub font_size: u16,
    pub appearance: Option<crate::appearance::Mode>,
    pub slow_query_ms: u64,
    pub null_display: String,
    pub discovery: DiscoveryPreferences,
    pub map_layouts: Vec<MapLayout>,
    pub(crate) workspace_layouts: Vec<crate::app::workspace::Snapshot>,
    pub inspector_visibility: std::collections::BTreeMap<String, bool>,
    pub panels: Panels,
    pub assistant: AssistantPreferences,
}
/// The agent CLI the assistant drives and what each database shares with it.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct AssistantPreferences {
    pub provider: Provider,
    /// A full command line for any other Agent Client Protocol agent.
    pub custom_command: String,
    /// Passed to the agent as its model; empty keeps the agent's own default.
    #[serde(skip_serializing)]
    pub model: String,
    pub models: std::collections::BTreeMap<Provider, String>,
    /// Reasoning level per provider; a missing one keeps the agent's own.
    pub efforts: std::collections::BTreeMap<Provider, String>,
    /// Models starred in the chat's model picker, across agents, in the order starred.
    pub favourite_models: Vec<FavouriteModel>,
    pub executables: std::collections::BTreeMap<Provider, PathBuf>,
    /// Legacy per-database choices, migrated into `grants`.
    #[serde(skip_serializing)]
    pub sharing: std::collections::BTreeMap<String, crate::assistant_tools::Sharing>,
    /// What a database shares with an agent until it is changed for that database.
    pub default_sharing: crate::assistant_tools::Sharing,
    /// Choices made for one database and provider; they override the default.
    pub grants: std::collections::BTreeMap<
        String,
        std::collections::BTreeMap<Provider, crate::assistant_tools::Sharing>,
    >,
    /// What each agent CLI may keep or report, chosen separately per provider.
    pub privacy: Privacy,
    /// Sessions left before each agent's notice of who receives the data shows
    /// again; a missing provider has never seen it.
    pub intro_skips: std::collections::BTreeMap<Provider, u8>,
    /// How the chat's copy buttons format a reply or the conversation.
    pub copy_format: CopyFormat,
}
/// One starred model: an agent and its model id; empty is the agent's default.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavouriteModel {
    pub provider: Provider,
    pub model: String,
}
/// The text format the assistant's copy buttons write to the clipboard.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub enum CopyFormat {
    #[default]
    Markdown,
    Html,
}
impl AssistantPreferences {
    /// Sessions that use an agent without repeating its notice, after it shows.
    pub const INTRO_SKIPS: u8 = 4;
    /// Whether this session shows the selected agent's notice; counts the session.
    pub fn take_intro(&mut self) -> bool {
        let skips = self.intro_skips.entry(self.provider).or_insert(0);
        let show = *skips == 0;
        *skips = if show { Self::INTRO_SKIPS } else { *skips - 1 };
        show
    }
    pub fn selected_model(&self) -> &str {
        self.models.get(&self.provider).map_or("", String::as_str)
    }
    pub fn selected_effort(&self) -> &str {
        self.efforts.get(&self.provider).map_or("", String::as_str)
    }
    /// This database's own choice for the selected provider, if it has one.
    pub fn grant_for(&self, database: &str) -> Option<crate::assistant_tools::Sharing> {
        self.grants.get(database)?.get(&self.provider).copied()
    }
    pub fn sharing_for(&self, database: &str) -> crate::assistant_tools::Sharing {
        self.grant_for(database).unwrap_or(self.default_sharing)
    }
    pub fn clear_custom_grants(&mut self) {
        self.intro_skips.remove(&Provider::Custom);
        for grants in self.grants.values_mut() {
            grants.remove(&Provider::Custom);
        }
        self.grants.retain(|_, grants| !grants.is_empty());
    }
}
/// Per-provider privacy; aliases on Provider preserve existing preference keys.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(transparent)]
pub struct Privacy(std::collections::BTreeMap<Provider, ProviderPrivacy>);
impl Privacy {
    pub fn of(&self, provider: Provider) -> ProviderPrivacy {
        self.0.get(&provider).copied().unwrap_or_default()
    }
    pub fn of_mut(&mut self, provider: Provider) -> &mut ProviderPrivacy {
        self.0.entry(provider).or_default()
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct ProviderPrivacy {
    /// Ask the CLI to send no telemetry, metrics or error reports.
    pub telemetry_off: bool,
    /// Let the CLI save conversations in its own history.
    pub keep_history: bool,
    /// Tell the agent the database's file name; its folder is never sent.
    pub share_file_name: bool,
}
impl Default for ProviderPrivacy {
    fn default() -> Self {
        Self {
            telemetry_off: true,
            keep_history: false,
            share_file_name: true,
        }
    }
}
impl ProviderPrivacy {
    pub fn agent(self) -> tursobench_assistant::agent::Privacy {
        tursobench_assistant::agent::Privacy {
            telemetry_off: self.telemetry_off,
            keep_history: self.keep_history,
        }
    }
}
pub use tursobench_assistant::vendor::Provider;
/// The navigation sidebar and the assistant panel are shown or hidden independently.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct Panels {
    pub sidebar: bool,
    pub assistant: bool,
}
impl Default for Panels {
    fn default() -> Self {
        Self {
            sidebar: true,
            assistant: false,
        }
    }
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
            sql_recovery: true,
            database_limit: None,
            memory_budget_mib: None,
            row_limit: 1000,
            busy_timeout_ms: 250,
            query_timeout_ms: 30_000,
            query_history: vec![],
            saved_queries: vec![],
            font_size: 14,
            appearance: Some(crate::appearance::Mode::System),
            slow_query_ms: 500,
            null_display: "NULL".into(),
            discovery: DiscoveryPreferences::default(),
            map_layouts: vec![],
            workspace_layouts: vec![],
            inspector_visibility: Default::default(),
            panels: Panels::default(),
            assistant: AssistantPreferences::default(),
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
    pub fn normalize(&mut self) {
        self.discovery.normalize();
        self.inspector_visibility
            .retain(|path, _| path.len() <= 4096 && Path::new(path).is_absolute());
        while self.inspector_visibility.len() > 20 {
            self.inspector_visibility.pop_first();
        }
        // Legacy sharing and model apply only to the previously selected vendor.
        for (path, sharing) in std::mem::take(&mut self.assistant.sharing) {
            self.assistant
                .grants
                .entry(path)
                .or_default()
                .entry(self.assistant.provider)
                .or_insert(sharing);
        }
        if !self.assistant.model.is_empty() {
            self.assistant
                .models
                .entry(self.assistant.provider)
                .or_insert(std::mem::take(&mut self.assistant.model));
        }
        self.assistant.grants.retain(|path, grants| {
            !grants.is_empty() && path.len() <= 4096 && Path::new(path).is_absolute()
        });
        while self.assistant.grants.len() > 50 {
            self.assistant.grants.pop_first();
        }
        self.assistant.models.retain(|_, model| model.len() <= 200);
        let mut seen = std::collections::BTreeSet::new();
        self.assistant
            .favourite_models
            .retain(|f| f.model.len() <= 200 && seen.insert((f.provider, f.model.clone())));
        self.assistant.favourite_models.truncate(24);
        self.assistant
            .efforts
            .retain(|_, effort| !effort.is_empty() && effort.len() <= 40);
        self.assistant
            .executables
            .retain(|_, path| path.is_absolute() && path.as_os_str().len() <= 4096);
        self.assistant
            .intro_skips
            .retain(|_, skips| *skips <= AssistantPreferences::INTRO_SKIPS);
        if self.assistant.custom_command.len() > 4096 {
            self.assistant.custom_command.clear();
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
        self.database_limit = self.database_limit.map(|limit| limit.max(1));
        self.memory_budget_mib = self
            .memory_budget_mib
            .map(|budget| budget.clamp(1, u64::MAX / crate::resources::MIB));
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
    tx: Option<SyncSender<Preferences>>,
    pub results: Receiver<Result<(), String>>,
    pending: Option<Preferences>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Store {
    pub fn start(wake: impl Fn() + Send + 'static) -> (Self, Preferences, Option<String>) {
        let path = if cfg!(test) {
            Err("Preference persistence disabled in UI tests".into())
        } else {
            config_path()
        };
        Self::start_path(path, wake)
    }
    fn start_path(
        path: Result<PathBuf, String>,
        wake: impl Fn() + Send + 'static,
    ) -> (Self, Preferences, Option<String>) {
        let (tx, rx) = mpsc::sync_channel::<Preferences>(1);
        let (done, results) = mpsc::channel();
        let (initial, loaded) = mpsc::sync_channel(1);
        let thread = std::thread::spawn(move || {
            let mut database = path.and_then(|p| {
                let parent = p.parent().ok_or("Missing preferences directory")?;
                std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    std::fs::set_permissions(parent, std::fs::Permissions::from_mode(0o700))
                        .map_err(|e| e.to_string())?;
                }
                tursobench_db::preferences::Store::open(&p)
            });
            let prefs = database
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|db| decode(db.payload().unwrap_or("{}")));
            let (prefs, warning) = match prefs {
                Ok(prefs) => (prefs, None),
                Err(e) => {
                    database = Err(format!("Preference saving disabled until recovery: {e}"));
                    (Preferences::default(), Some(e))
                }
            };
            if initial.send((prefs, warning)).is_err() {
                return;
            }
            while let Ok(prefs) = rx.recv() {
                let result = database.as_mut().map_err(|e| e.clone()).and_then(|db| {
                    let payload = serde_json::to_string(&prefs).map_err(|e| e.to_string())?;
                    db.save(&payload)
                });
                if done.send(result).is_err() {
                    break;
                }
                wake();
            }
        });
        // Startup waits for the initial settings; engine handles stay on the
        // storage thread and all subsequent saves are asynchronous/coalesced.
        let (prefs, warning) = loaded.recv().unwrap_or_else(|_| {
            (
                Preferences::default(),
                Some("Preference worker stopped during startup".into()),
            )
        });
        (
            Self {
                tx: Some(tx),
                results,
                pending: None,
                thread: Some(thread),
            },
            prefs,
            warning,
        )
    }
    pub fn save(&mut self, prefs: &Preferences) {
        self.pending = Some(prefs.clone());
        self.flush();
    }
    pub fn flush(&mut self) {
        if let (Some(tx), Some(prefs)) = (&self.tx, self.pending.take()) {
            match tx.try_send(prefs) {
                Ok(()) => {}
                Err(TrySendError::Full(p)) => self.pending = Some(p),
                Err(TrySendError::Disconnected(_)) => {}
            }
        }
    }
}
impl Drop for Store {
    fn drop(&mut self) {
        // Drain the last coalesced snapshot and close the database before exit.
        if let Some(tx) = self.tx.take()
            && let Some(prefs) = self.pending.take()
        {
            let _ = tx.send(prefs);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}
pub(crate) fn config_path() -> Result<PathBuf, String> {
    let root = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".config")))
        .ok_or("No application configuration directory available")?;
    Ok(root.join("tursobench/preferences.db"))
}
fn decode(payload: &str) -> Result<Preferences, String> {
    let mut prefs: Preferences = serde_json::from_str(payload).map_err(|e| e.to_string())?;
    prefs.normalize();
    Ok(prefs)
}

#[cfg(test)]
mod tests {
    use super::{CachedDatabase, MapLayout, Path, PathBuf, Preferences, Store, decode};
    fn read(path: &Path) -> Preferences {
        let db = tursobench_db::preferences::Store::open(path).unwrap();
        decode(db.payload().unwrap_or("{}")).unwrap()
    }
    fn save_and_wait(store: &mut Store, prefs: &Preferences) -> Result<(), String> {
        store.save(prefs);
        store
            .results
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap()
    }
    #[test]
    fn assistant_migration_scopes_legacy_consent_and_models_to_the_selected_vendor() {
        use super::Provider;
        let mut prefs = decode(r#"{"assistant":{"provider":"Codex","model":"my-model","sharing":{"/tmp/a.db":"Rows"},"privacy":{"claude":{"keep_history":true}}}}"#).unwrap();
        assert_eq!(prefs.assistant.selected_model(), "my-model");
        assert_eq!(
            prefs.assistant.grant_for("/tmp/a.db"),
            Some(crate::assistant_tools::Sharing::Rows)
        );
        assert!(prefs.assistant.privacy.of(Provider::Claude).keep_history);
        prefs.assistant.provider = Provider::Claude;
        assert_eq!(prefs.assistant.grant_for("/tmp/a.db"), None);
        assert_eq!(
            prefs.assistant.sharing_for("/tmp/a.db"),
            crate::assistant_tools::Sharing::Rows
        );
        prefs.assistant.default_sharing = crate::assistant_tools::Sharing::Schema;
        assert_eq!(
            prefs.assistant.sharing_for("/tmp/a.db"),
            crate::assistant_tools::Sharing::Schema
        );
        assert_eq!(prefs.assistant.selected_model(), "");
        let encoded = serde_json::to_string(&prefs).unwrap();
        let restored = decode(&encoded).unwrap();
        assert_eq!(restored.assistant, prefs.assistant);
        assert!(!encoded.contains("\"sharing\":"));
    }

    #[test]
    fn shutdown_drains_the_latest_coalesced_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.db");
        let (mut store, mut prefs, warning) = Store::start_path(Ok(path.clone()), || {});
        assert!(warning.is_none());
        for i in 1..=50 {
            prefs.row_limit = i;
            store.save(&prefs);
        }
        drop(store);
        assert_eq!(read(&path).row_limit, 50);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(dir.path()).unwrap().permissions().mode() & 0o777,
                0o700
            );
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    #[test]
    fn turso_storage_roundtrips_settings_history_and_layouts_without_json_import() {
        let dir = tempfile::tempdir().unwrap();
        let legacy = dir.path().join("preferences.json");
        std::fs::write(&legacy, r#"{"font_size":20,"saved_queries":[{"database":"old","name":"old","sql":"SELECT 99"}]}"#).unwrap();
        let original = std::fs::read(&legacy).unwrap();
        let path = dir.path().join("preferences.db");
        let (mut store, mut prefs, error) = Store::start_path(Ok(path.clone()), || {});
        assert!(error.is_none());
        assert_eq!(prefs.font_size, 14);
        assert!(prefs.saved_queries.is_empty());
        prefs.font_size = 18;
        prefs.memory_budget_mib = Some(12 * 1024);
        prefs.database_limit = Some(17);
        prefs.appearance = Some(crate::appearance::Mode::Light);
        for i in 0..150 {
            prefs.record_query("db-a", &format!("SELECT {i}"), Some(i), false);
        }
        prefs.bookmark("db-a", "Quoted", "SELECT ';'");
        prefs
            .inspector_visibility
            .insert("/home/test/a.db".into(), false);
        prefs.map_layouts.push(MapLayout {
            database: "/home/test/a.db".into(),
            positions: [("t".into(), [32.0, 64.0])].into(),
            pan: [10.0, 20.0],
            zoom: 0.75,
            pack_width: 1200.0,
            all_columns: true,
            selected: Some("t".into()),
        });
        save_and_wait(&mut store, &prefs).unwrap();
        drop(store);
        // Drop drains the writer and releases its database handles before reopen.
        let (reader, loaded, error) = Store::start_path(Ok(path.clone()), || {});
        assert!(error.is_none(), "{error:?}");
        assert_eq!(loaded.font_size, 18);
        assert_eq!(loaded.appearance, Some(crate::appearance::Mode::Light));
        assert_eq!(loaded.query_history.len(), 100);
        assert_eq!(loaded.saved_queries[0].sql, "SELECT ';'");
        assert_eq!(loaded.map_layouts, prefs.map_layouts);
        assert_eq!(loaded.inspector_visibility, prefs.inspector_visibility);
        assert_eq!(loaded.memory_budget_mib, Some(12 * 1024));
        assert_eq!(loaded.database_limit, Some(17));
        drop(reader);
        assert_eq!(std::fs::read(&legacy).unwrap(), original);
    }
    #[test]
    fn unsupported_payload_disables_saves_without_losing_stored_queries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.db");
        {
            let mut db = tursobench_db::preferences::Store::open(&path).unwrap();
            db.save(r#"{"font_size":"invalid","saved_queries":[{"database":"db","name":"q","sql":"SELECT 1"}]}"#).unwrap();
        }
        let (mut store, prefs, warning) = Store::start_path(Ok(path.clone()), || {});
        assert!(warning.is_some());
        assert_eq!(prefs.font_size, 14);
        assert!(save_and_wait(&mut store, &prefs).is_err());
        let db = tursobench_db::preferences::Store::open(&path).unwrap();
        assert!(db.payload().unwrap().contains("SELECT 1"));
    }
    #[test]
    fn resource_preferences_default_to_auto_and_normalize_without_overflow() {
        let old = decode("{}").unwrap();
        assert_eq!(old.database_limit, None);
        assert_eq!(old.memory_budget_mib, None);
        let prefs =
            decode(r#"{"database_limit":0,"memory_budget_mib":18446744073709551615}"#).unwrap();
        assert_eq!(prefs.database_limit, Some(1));
        assert_eq!(
            prefs.memory_budget_mib,
            Some(u64::MAX / crate::resources::MIB)
        );
    }

    #[test]
    fn font_and_discovery_preferences_normalize_without_legacy_migration() {
        let mut prefs = decode(r#"{"font_size":100,"discovery":{"enabled":false}}"#).unwrap();
        assert_eq!(prefs.font_size, 20);
        assert!(!prefs.discovery.enabled);
        assert_eq!(prefs.appearance, Some(crate::appearance::Mode::System));
        prefs.discovery.roots = vec![
            "/".into(),
            "relative".into(),
            "/home/test/Projects/../Code".into(),
            "/home/test/Code".into(),
        ];
        for i in 0..250 {
            prefs
                .discovery
                .record_open(format!("/home/test/{i}.db").into(), i);
            prefs.discovery.remember(CachedDatabase {
                path: format!("/home/test/{i}.db").into(),
                size: 100,
                modified: Some(i),
            });
        }
        prefs.normalize();
        assert_eq!(
            prefs.discovery.roots,
            vec![PathBuf::from("/home/test/Code")]
        );
        assert_eq!(prefs.discovery.recent.len(), 50);
        assert_eq!(prefs.discovery.cached.len(), 200);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("preferences.db");
        {
            let mut db = tursobench_db::preferences::Store::open(&path).unwrap();
            db.save(&serde_json::to_string(&prefs).unwrap()).unwrap();
        }
        assert_eq!(read(&path).discovery.cached, prefs.discovery.cached);
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
}
