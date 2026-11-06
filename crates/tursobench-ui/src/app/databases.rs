use super::App;
use eframe::egui;

mod chooser;
mod connection;
mod inspection;
mod presentation;
pub(super) use presentation::{folder_label, opened_database_icon};
mod settings;
use crate::{
    discovery::{self, Availability, DatabaseKind, Event, Job, Request, Root, Summary},
    storage::{CachedDatabase, DiscoveryPreferences, normalized_absolute},
};
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::mpsc::TryRecvError,
    time::{SystemTime, UNIX_EPOCH},
};

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    Remembered,
    Found,
    Checked(Availability),
}

#[derive(Clone)]
struct Entry {
    path: PathBuf,
    state: State,
    size: Option<u64>,
    last_opened: Option<u64>,
    kind: DatabaseKind,
}
impl Entry {
    fn badge(&self) -> Option<&'static str> {
        match self.state {
            State::Checked(Availability::Missing) => Some("Missing"),
            State::Checked(Availability::Unavailable) => Some("Unreadable"),
            State::Checked(Availability::NotDatabase) => Some("Not a database"),
            State::Checked(Availability::Skipped) => Some("Outside search scope"),
            State::Checked(Availability::ExcludedApplication) => Some("Use Browse"),
            _ if self.path.to_str().is_none() => Some("Unsupported path"),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match (self.state, self.last_opened.is_some()) {
            (State::Remembered, true) => "Opened before · awaiting header check",
            (State::Remembered, false) => "Remembered candidate · awaiting header check",
            (State::Found, true) => "Opened before · header checked",
            (State::Found, false) => "Database candidate · header checked",
            (State::Checked(Availability::Missing), _) => "File no longer found",
            (State::Checked(Availability::Unavailable), _) => "Could not read file",
            (State::Checked(Availability::NotDatabase), _) => "No recognized database header",
            (State::Checked(Availability::Skipped), _) => {
                "Not searched · outside automatic search policy"
            }
            (State::Checked(Availability::ExcludedApplication), _) => {
                "Excluded application data · use Browse to select explicitly"
            }
        }
    }
    fn selectable(&self) -> bool {
        !matches!(
            self.state,
            State::Checked(
                Availability::Missing | Availability::NotDatabase | Availability::Unavailable
            )
        ) && self.path.to_str().is_some()
    }
}

#[derive(Default)]
pub(super) struct Chooser {
    pub inspection: inspection::Inspection,
    pub open_failure: Option<tursobench_core::Failure>,
    pub job: Option<Job>,
    entries: Vec<Entry>,
    folder_input: String,
    exclude_input: String,
    pub picking_folder: bool,
    pub editing_path: bool,
    pub message: Option<String>,
    summary: Option<Summary>,
    cancelling: bool,
    pending_folder: Option<PathBuf>,
    forgotten: HashSet<PathBuf>,
    identities: std::collections::HashMap<PathBuf, crate::discovery::Identity>,
    #[cfg(test)]
    pub row_rects: Vec<(PathBuf, egui::Rect)>,
    #[cfg(test)]
    pub rendered_candidates: usize,
    #[cfg(test)]
    pub browse_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub search_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub edit_path_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub open_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub open_enabled: bool,
    #[cfg(test)]
    pub refresh_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub override_rect: Option<egui::Rect>,
}
impl Chooser {
    pub(super) fn new(prefs: &DiscoveryPreferences) -> Self {
        let mut chooser = Self::default();
        for recent in &prefs.recent {
            chooser.entries.push(Entry {
                path: recent.path.clone(),
                state: State::Remembered,
                size: None,
                last_opened: Some(recent.last_opened),
                kind: DatabaseKind::Unknown,
            });
        }
        for cached in &prefs.cached {
            if let Some(entry) = chooser.entries.iter_mut().find(|e| e.path == cached.path) {
                entry.size = Some(cached.size);
            } else {
                chooser.entries.push(Entry {
                    path: cached.path.clone(),
                    state: State::Remembered,
                    size: Some(cached.size),
                    last_opened: None,
                    kind: DatabaseKind::Unknown,
                });
            }
        }
        chooser
            .entries
            .retain(|e| !prefs.excluded.iter().any(|p| e.path.starts_with(p)));
        chooser.entries.sort_by(|a, b| {
            b.last_opened
                .cmp(&a.last_opened)
                .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
                .then_with(|| a.path.cmp(&b.path))
        });
        chooser
    }
    fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
            self.cancelling = true;
        }
    }
}

fn request(prefs: &DiscoveryPreferences, folder: Option<PathBuf>) -> Request {
    let known: Vec<_> = prefs
        .recent
        .iter()
        .map(|e| e.path.clone())
        .chain(prefs.cached.iter().map(|e| e.path.clone()))
        .filter(|p| folder.as_ref().is_none_or(|root| p.starts_with(root)))
        .collect();
    let roots = if let Some(path) = folder {
        vec![Root {
            path,
            depth: 5,
            extensionless: true,
            explicit: true,
        }]
    } else {
        search_roots(prefs)
    };
    Request {
        roots,
        known,
        excluded: prefs.excluded.clone(),
        ..Default::default()
    }
}

fn search_roots(prefs: &DiscoveryPreferences) -> Vec<Root> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let cwd = std::env::current_dir().ok();
    let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
    let state = std::env::var_os("XDG_STATE_HOME").map(PathBuf::from);
    configured_roots(
        prefs,
        discovery::default_roots(
            home.as_deref(),
            cwd.as_deref(),
            data.as_deref(),
            state.as_deref(),
        ),
    )
}

/// The settings list and the worker use exactly the same roots and policies.
fn configured_roots(prefs: &DiscoveryPreferences, defaults: Vec<Root>) -> Vec<Root> {
    let added = prefs.roots.iter().map(|path| Root {
        path: path.clone(),
        depth: 5,
        extensionless: true,
        explicit: true,
    });
    let recent = prefs.recent.iter().filter_map(|entry| {
        entry
            .path
            .parent()
            .filter(|p| p.parent().is_some())
            .map(|path| Root {
                path: path.into(),
                depth: 1,
                extensionless: true,
                explicit: false,
            })
    });
    let mut roots = Vec::<Root>::new();
    let mut positions = std::collections::BTreeMap::<PathBuf, usize>::new();
    for mut root in added.chain(recent).chain(defaults) {
        let Some(path) = normalized_absolute(&root.path) else {
            continue;
        };
        if prefs.removed_roots.contains(&path) || prefs.excluded.iter().any(|p| path.starts_with(p))
        {
            continue;
        }
        root.path = path;
        if let Some(&index) = positions.get(&root.path) {
            let previous = &mut roots[index];
            previous.depth = previous.depth.max(root.depth);
            previous.extensionless |= root.extensionless;
            previous.explicit |= root.explicit;
        } else {
            positions.insert(root.path.clone(), roots.len());
            roots.push(root);
        }
    }
    roots
}

impl App {
    pub(super) fn start_discovery(&mut self, ctx: &egui::Context, folder: Option<PathBuf>) {
        // Never replace a worker that may still be inside a filesystem call.
        if self.databases.job.is_some() {
            return;
        }
        self.databases.summary = None;
        self.databases.message = None;
        self.databases.cancelling = false;
        self.databases.forgotten.clear();
        for entry in &mut self.databases.entries {
            if folder
                .as_ref()
                .is_none_or(|root| entry.path.starts_with(root))
            {
                entry.state = State::Remembered;
                entry.kind = DatabaseKind::Unknown;
            }
        }
        let wake = ctx.clone();
        match Job::start(request(&self.preferences.discovery, folder), move || {
            wake.request_repaint()
        }) {
            Ok(job) => self.databases.job = Some(job),
            Err(error) => {
                self.databases.message = Some(format!(
                    "Could not start database search: {error}. Browse or enter a database path."
                ))
            }
        }
    }

    pub(super) fn poll_discovery(&mut self, ctx: &egui::Context) {
        // Bound UI work as well as worker work; results never select a file.
        for _ in 0..64 {
            let Some(job) = &self.databases.job else {
                break;
            };
            match job.events.try_recv() {
                Ok(event) => self.discovery_event(event),
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.discovery_event(Event::Finished(Summary {
                        error: Some("Database search worker disconnected unexpectedly".into()),
                        ..Default::default()
                    }));
                    break;
                }
            }
        }
        if self.databases.job.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        if self.databases.job.is_none()
            && let Some(folder) = self.databases.pending_folder.take()
        {
            self.start_discovery(ctx, Some(folder));
        }
    }

    fn discovery_event(&mut self, event: Event) {
        match event {
            Event::Found(candidate) => {
                if self.databases.forgotten.contains(&candidate.path)
                    || self
                        .preferences
                        .discovery
                        .excluded
                        .iter()
                        .any(|p| candidate.path.starts_with(p))
                {
                    return;
                }
                if let Some(identity) = candidate.identity {
                    self.databases
                        .identities
                        .insert(candidate.path.clone(), identity);
                }
                self.preferences.discovery.remember(CachedDatabase {
                    path: candidate.path.clone(),
                    size: candidate.size,
                    modified: candidate.modified,
                });
                if let Some(entry) = self
                    .databases
                    .entries
                    .iter_mut()
                    .find(|e| e.path == candidate.path)
                {
                    entry.state = State::Found;
                    entry.kind = candidate.kind;
                    entry.size = Some(candidate.size);
                } else {
                    // Append during a scan: preserve keyboard focus and scroll.
                    self.databases.entries.push(Entry {
                        path: candidate.path,
                        state: State::Found,
                        size: Some(candidate.size),
                        last_opened: None,
                        kind: candidate.kind,
                    });
                }
            }
            Event::Checked(path, state) => {
                self.databases.identities.remove(&path);
                if let Some(entry) = self.databases.entries.iter_mut().find(|e| e.path == path) {
                    entry.state = State::Checked(state);
                    entry.kind = DatabaseKind::Unknown;
                }
                if matches!(
                    state,
                    Availability::Missing
                        | Availability::NotDatabase
                        | Availability::ExcludedApplication
                ) {
                    self.preferences.discovery.cached.retain(|c| c.path != path);
                }
                if state == Availability::ExcludedApplication {
                    self.databases
                        .entries
                        .retain(|e| e.path != path || e.last_opened.is_some());
                }
            }
            Event::Alias { path, original } => {
                if let Some(identity) = self.databases.identities.get(&original).cloned() {
                    self.databases.identities.insert(path.clone(), identity);
                }
                let recent = self
                    .databases
                    .entries
                    .iter()
                    .find(|e| e.path == path)
                    .and_then(|e| e.last_opened);
                self.databases.entries.retain(|e| e.path != path);
                self.preferences.discovery.cached.retain(|e| e.path != path);
                if let Some(time) = recent
                    && let Some(entry) = self
                        .databases
                        .entries
                        .iter_mut()
                        .find(|e| e.path == original)
                {
                    entry.last_opened = Some(entry.last_opened.unwrap_or(0).max(time));
                }
            }
            Event::Finished(summary) => {
                self.databases.job = None;
                self.databases.cancelling = false;
                self.databases.summary = Some(summary);
                self.preferences.discovery.normalize();
                self.store.save(&self.preferences);
            }
        }
    }

    pub(super) fn record_database_open(&mut self) {
        // Resolve file identity once after opening; paint paths use this snapshot.
        self.database.identity = std::fs::metadata(&self.database.path)
            .ok()
            .map(|metadata| crate::discovery::identity(&metadata, Path::new(&self.database.path)));
        if let Some(identity) = self.database.identity.clone() {
            self.databases
                .identities
                .insert(self.database.path.clone().into(), identity);
        }
        let Some(path) = normalized_absolute(Path::new(&self.database.tools.database_scope)) else {
            return;
        };
        let time = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        self.preferences.discovery.record_open(path.clone(), time);
        if let Some(entry) = self.databases.entries.iter_mut().find(|e| e.path == path) {
            entry.last_opened = Some(time);
        } else {
            self.databases.entries.insert(
                0,
                Entry {
                    path,
                    state: State::Remembered,
                    size: None,
                    last_opened: Some(time),
                    kind: DatabaseKind::Unknown,
                },
            );
        }
        self.store.save(&self.preferences);
    }

    pub(super) fn add_search_folder(&mut self, path: PathBuf) {
        let Some(path) = normalized_absolute(&path)
            .filter(|p| p.parent().is_some() && p.to_str().is_some_and(|s| s.len() <= 4096))
        else {
            self.databases.message = Some("Enter an absolute folder path narrower than the filesystem root. Only UTF-8 paths can be saved.".into());
            return;
        };
        if self
            .preferences
            .discovery
            .excluded
            .iter()
            .any(|p| path.starts_with(p))
        {
            self.databases.message =
                Some("That folder is excluded. Remove its exclusion before searching it.".into());
            return;
        }
        self.preferences
            .discovery
            .removed_roots
            .retain(|p| p != &path);
        if !self.preferences.discovery.roots.contains(&path) {
            self.preferences.discovery.roots.insert(0, path.clone());
        }
        self.preferences.discovery.normalize();
        self.store.save(&self.preferences);
        self.databases.cancel();
        self.databases.pending_folder = Some(path);
        self.databases.folder_input.clear();
        self.databases.message = None;
    }

    fn remove_search_folder(&mut self, path: PathBuf) {
        let prefs = &mut self.preferences.discovery;
        if !prefs.removed_roots.contains(&path) {
            if prefs.removed_roots.len() >= 128 || path.as_os_str().len() > 4096 {
                self.databases.message =
                    Some("Search-folder removal limit reached; use an exclusion instead.".into());
                return;
            }
            prefs.removed_roots.insert(0, path.clone());
        }
        prefs.roots.retain(|p| p != &path);
        self.databases.cancel();
        self.databases.pending_folder = None;
        self.store.save(&self.preferences);
    }

    fn add_exclusion(&mut self, path: PathBuf) {
        let Some(path) =
            normalized_absolute(&path).filter(|p| p.to_str().is_some_and(|s| s.len() <= 4096))
        else {
            self.databases.message = Some("Enter an absolute folder path to exclude.".into());
            return;
        };
        self.preferences.discovery.excluded.insert(0, path.clone());
        self.preferences.discovery.normalize();
        self.preferences
            .discovery
            .cached
            .retain(|e| !e.path.starts_with(&path));
        self.databases
            .entries
            .retain(|e| !e.path.starts_with(&path));
        self.databases.cancel();
        self.databases.pending_folder = None;
        self.databases.exclude_input.clear();
        self.store.save(&self.preferences);
        self.databases.message = Some("Folder excluded.".into());
    }

    fn forget_database(&mut self, path: &Path) {
        self.databases.identities.remove(path);
        self.databases.forgotten.insert(path.into());
        self.databases.entries.retain(|e| e.path != path);
        self.preferences.discovery.cached.retain(|e| e.path != path);
        self.preferences.discovery.recent.retain(|e| e.path != path);
        self.store.save(&self.preferences);
    }
}

impl App {
    pub(super) fn listed_workspace(&self, path: &Path) -> Option<u64> {
        let identity = self.databases.identities.get(path);
        std::iter::once(&self.database)
            .chain(self.parked.values())
            .find(|d| {
                d.retained()
                    && (Path::new(&d.path) == path
                        || identity
                            .zip(d.identity.as_ref())
                            .is_some_and(|(a, b)| a == b))
            })
            .map(|d| d.id)
    }
    pub(super) fn available_search_results(&self, query: &str) -> Vec<(usize, String)> {
        let query = query.to_lowercase();
        self.databases
            .entries
            .iter()
            .enumerate()
            .filter(|(_, e)| e.selectable() && self.listed_workspace(&e.path).is_none())
            .filter(|(_, e)| {
                query
                    .split_whitespace()
                    .all(|word| e.path.to_string_lossy().to_lowercase().contains(word))
            })
            .map(|(i, e)| (i, e.path.display().to_string()))
            .collect()
    }
    pub(super) fn available_result_ui(
        &self,
        ui: &mut egui::Ui,
        index: usize,
        highlighted: bool,
        enabled: bool,
    ) -> (egui::Response, bool) {
        self.databases.entries[index].row_ui(ui, false, highlighted, enabled)
    }
    pub(super) fn choose_available_result(&mut self, ctx: &egui::Context, index: usize) {
        let path = self.databases.entries[index].path.clone();
        self.request_database(ctx, path);
    }
    pub(super) fn forget_available_result(&mut self, index: usize) {
        let path = self.databases.entries[index].path.clone();
        self.forget_database(&path);
    }
    #[cfg(test)]
    fn available_database_paths(&self, query: &str) -> Vec<PathBuf> {
        self.available_search_results(query)
            .into_iter()
            .map(|(i, _)| self.databases.entries[i].path.clone())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::{
        Availability, CachedDatabase, Chooser, DatabaseKind, DiscoveryPreferences, Event, Path,
        State, request,
    };
    use crate::{discovery::Candidate, storage::RecentDatabase};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    #[test]
    fn search_roots_merge_policies_and_respect_saved_removals_and_exclusions() {
        let mut prefs = DiscoveryPreferences::default();
        prefs.roots.push("/work/added".into());
        prefs.record_open("/work/projects/current.db".into(), 1);
        prefs.record_open("/work/projects/other.db".into(), 2);
        prefs.removed_roots.push("/work/removed".into());
        prefs.excluded.push("/work/private".into());
        let defaults = [
            "/work/projects",
            "/work/added",
            "/work/removed",
            "/work/private/nested",
        ]
        .into_iter()
        .map(|path| super::Root {
            path: path.into(),
            depth: 3,
            extensionless: false,
            explicit: false,
        })
        .collect();
        let roots = super::configured_roots(&prefs, defaults);
        assert_eq!(roots.len(), 2);
        assert_eq!(roots[0].path, Path::new("/work/added"));
        assert!(roots[0].explicit && roots[0].extensionless);
        assert_eq!(roots[0].depth, 5);
        assert_eq!(roots[1].path, Path::new("/work/projects"));
        assert_eq!(roots[1].depth, 3);
        assert!(roots[1].extensionless && !roots[1].explicit);
    }

    fn found(path: &str) -> Event {
        Event::Found(Candidate {
            path: path.into(),
            size: 4096,
            modified: Some(1),
            kind: DatabaseKind::Unknown,
            identity: None,
        })
    }
    #[test]
    fn shared_sidebar_search_preserves_cursor_and_selects_the_edited_query() {
        use crate::app::test_ui::{key_event, render};
        let (ctx, mut app) = crate::app::test_ui::app();
        app.discovery_event(found("/work/agenda.db"));
        app.discovery_event(found("/work/clipboard.db"));
        let frame = |app: &mut crate::app::App, events| {
            render(&ctx, eframe::egui::vec2(300.0, 600.0), events, |ui| {
                app.navigation_sidebar(ui)
            })
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![eframe::egui::Event::Text("agenda".into())]);
        for key in [
            eframe::egui::Key::Home,
            eframe::egui::Key::ArrowRight,
            eframe::egui::Key::ArrowDown,
        ] {
            frame(
                &mut app,
                vec![key_event(key, true, eframe::egui::Modifiers::NONE)],
            );
        }
        frame(&mut app, vec![eframe::egui::Event::Text("g".into())]);
        assert_eq!(app.sidebar_search.query, "aggenda");
        assert!(app.database.path.is_empty());
        app.sidebar_search.query.clear();
        app.sidebar_search.focus = true;
        frame(&mut app, vec![]);
        frame(
            &mut app,
            vec![
                eframe::egui::Event::Text("clipboard".into()),
                key_event(
                    eframe::egui::Key::Enter,
                    true,
                    eframe::egui::Modifiers::NONE,
                ),
            ],
        );
        assert_eq!(app.database.path, "/work/clipboard.db");
        assert!(!app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn available_rows_select_without_opening_and_skip_unavailable_candidates() {
        use crate::app::test_ui::{key_event, pointer, render};
        let (ctx, mut app) = crate::app::test_ui::app();
        app.discovery_event(found("/work/a.db"));
        app.discovery_event(found("/work/missing.db"));
        app.discovery_event(Event::Checked(
            "/work/missing.db".into(),
            Availability::Missing,
        ));
        let frame = |app: &mut crate::app::App, events| {
            render(&ctx, eframe::egui::vec2(272.0, 600.0), events, |ui| {
                app.navigation_sidebar(ui)
            })
        };
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        let row = app.databases.row_rects[0].1;
        assert_eq!(row.height(), 34.0);
        for pressed in [true, false] {
            frame(&mut app, pointer(row.center(), pressed));
        }
        assert_eq!(app.database.path, "/work/a.db");
        assert!(!app.database.writable);
        app.sidebar_search.focus = true;
        frame(&mut app, vec![]);
        for key in [
            eframe::egui::Key::ArrowDown,
            eframe::egui::Key::ArrowDown,
            eframe::egui::Key::Enter,
        ] {
            frame(
                &mut app,
                vec![key_event(key, true, eframe::egui::Modifiers::NONE)],
            );
        }
        assert_eq!(app.database.path, "/work/a.db");
        assert!(app.database.session.is_none() && app.picker.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn cached_identity_and_virtual_rows_bound_database_list_repaints() {
        use crate::app::test_ui::{inspect_selected, key_event, render};
        use eframe::egui;
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("source.db");
        let alias = dir.path().join("alias.db");
        crate::test_support::create(&source, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);

        let (ctx, mut app) = crate::app::test_ui::app();
        app.set_database_path(source.to_str().unwrap().into());
        inspect_selected(&mut app, &ctx);
        app.open(&ctx);
        let start = std::time::Instant::now();
        while !app.database.ready || app.database.catalog_ticket.is_some() {
            app.poll(&ctx);
            assert!(start.elapsed() < std::time::Duration::from_secs(5));
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        std::os::unix::fs::symlink(&source, &alias).unwrap();
        let identity = app.database.identity.clone().expect("opened identity");
        app.discovery_event(Event::Found(Candidate {
            path: alias.clone(),
            size: 4096,
            modified: None,
            kind: DatabaseKind::LikelyTurso,
            identity: Some(identity),
        }));
        for index in 0..500 {
            app.discovery_event(found(&format!("/work/{index:03}.db")));
        }
        assert!(!app.available_database_paths("").contains(&alias));
        let frame = |app: &mut crate::app::App, events| {
            render(&ctx, egui::vec2(272.0, 600.0), events, |ui| {
                app.navigation_sidebar(ui)
            })
        };
        crate::app::sessions::IDENTITY_PROBES.with(|count| count.set(0));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert!(app.databases.rendered_candidates < 20);
        crate::app::sessions::IDENTITY_PROBES
            .with(|count| assert_eq!(count.get(), 0, "painting must not stat paths"));
        frame(
            &mut app,
            vec![key_event(egui::Key::ArrowDown, true, egui::Modifiers::NONE)],
        );
        for _ in 0..6 {
            frame(
                &mut app,
                vec![key_event(egui::Key::PageDown, true, egui::Modifiers::NONE)],
            );
        }
        for _ in 0..15 {
            frame(&mut app, vec![]);
        }
        assert_eq!(app.sidebar_search.index, 30);
        assert!(
            app.databases
                .row_rects
                .iter()
                .any(|(p, _)| p == Path::new("/work/030.db"))
        );
        app.discovery_event(found("/work/new.db"));
        frame(&mut app, vec![]);
        assert_eq!(
            app.sidebar_search.available_highlight.as_deref(),
            Some(Path::new("/work/030.db"))
        );
        assert!(app.databases.rendered_candidates < 20);
        assert_eq!(app.duplicate_workspace(&alias), Some(app.database.id));
        crate::app::sessions::IDENTITY_PROBES
            .with(|count| assert!(count.get() > 0, "explicit selection checks identity"));
        app.close();
    }

    #[test]
    fn available_search_keeps_filenames_visible_and_selection_opens_access_review() {
        use crate::app::test_ui::{control, pointer, render};
        use eframe::egui;
        let (ctx, mut app) = crate::app::test_ui::app();
        let path = format!(
            "/home/demo/{}/notifications-v2.db",
            "long-folder/".repeat(12)
        );
        app.discovery_event(found(&path));
        app.databases.entries[0].kind = DatabaseKind::LikelySqlite;
        app.sidebar_search.query = "notifications".into();
        let frame = |app: &mut crate::app::App, events| {
            render(&ctx, egui::vec2(272.0, 600.0), events, |ui| {
                app.navigation_sidebar(ui)
            })
        };
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == "notifications-v2.db")));
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == "SQLite"))
        );
        let row = control(&output, &format!("{path} · Likely SQLite"));
        for pressed in [true, false] {
            frame(&mut app, pointer(row.center(), pressed));
        }
        assert_eq!(app.database.path, path);
        assert!(!app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn cached_and_recent_entries_are_immediate_unique_and_unverified() {
        let mut prefs = DiscoveryPreferences::default();
        prefs.record_open("/work/recent.db".into(), 10);
        prefs.remember(CachedDatabase {
            path: "/work/recent.db".into(),
            size: 100,
            modified: None,
        });
        prefs.remember(CachedDatabase {
            path: "/work/other.db".into(),
            size: 200,
            modified: None,
        });
        prefs.remember(CachedDatabase {
            path: "/private/hidden.db".into(),
            size: 200,
            modified: None,
        });
        prefs.excluded.push("/private".into());
        let chooser = Chooser::new(&prefs);
        assert!(chooser.job.is_none());
        assert_eq!(chooser.entries.len(), 2);
        assert_eq!(chooser.entries[0].path, Path::new("/work/recent.db"));
        assert!(chooser.entries.iter().all(|e| e.state == State::Remembered));
    }

    #[test]
    fn incremental_discoveries_never_replace_paths_consent_or_row_order() {
        let (ctx, mut app) = crate::app::test_ui::app();
        assert!(app.databases.job.is_none());
        app.database.path = "/explicit/command-line.db".into();

        app.database.writable = true;
        app.discovery_event(found("/work/z-last.db"));
        app.discovery_event(found("/work/a-first.db"));
        app.discovery_event(found("/work/z-last.db"));
        assert_eq!(app.database.path, "/explicit/command-line.db");
        assert!(app.database.writable);
        assert_eq!(app.databases.entries.len(), 2);
        assert_eq!(app.databases.entries[0].path, Path::new("/work/z-last.db"));
        app.request_database(&ctx, "/work/a-first.db".into());
        assert_eq!(app.database.path, "/work/a-first.db");
        assert!(!app.database.writable);
        assert!(app.database.session.is_none());
        app.database.writable = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        app.request_database(&ctx, "/work/z-last.db".into());
        assert_eq!(app.database.path, "/work/a-first.db");
        assert!(app.database.writable);
        drop(tx);
    }

    #[test]
    fn stale_entries_aliases_and_forgetting_do_not_reappear_from_pending_events() {
        let (_ctx, mut app) = crate::app::test_ui::app();
        app.discovery_event(found("/work/a.db"));
        app.discovery_event(found("/work/b.db"));
        app.discovery_event(Event::Checked("/work/a.db".into(), Availability::Missing));
        assert!(!app.databases.entries[0].selectable());
        assert!(
            app.preferences
                .discovery
                .cached
                .iter()
                .all(|e| e.path != Path::new("/work/a.db"))
        );
        app.discovery_event(Event::Alias {
            path: "/work/a.db".into(),
            original: "/work/b.db".into(),
        });
        assert_eq!(app.databases.entries.len(), 1);
        app.forget_database(Path::new("/work/b.db"));
        app.discovery_event(found("/work/b.db"));
        assert!(app.databases.entries.is_empty());
        assert!(app.preferences.discovery.cached.is_empty());
        app.add_exclusion("/work".into());
        app.discovery_event(found("/work/c.db"));
        assert!(app.databases.entries.is_empty());
    }
    #[test]
    fn application_exclusions_remove_old_discoveries_but_keep_deliberate_open_history() {
        let (_ctx, mut app) = crate::app::test_ui::app();
        app.discovery_event(found("/data/custom-browser/History"));
        app.discovery_event(found("/data/custom-browser/Cookies"));
        app.database.tools.database_scope = "/data/custom-browser/History".into();
        app.record_database_open();
        for path in [
            "/data/custom-browser/History",
            "/data/custom-browser/Cookies",
        ] {
            app.discovery_event(Event::Checked(
                path.into(),
                Availability::ExcludedApplication,
            ));
        }
        assert!(app.preferences.discovery.cached.is_empty());
        assert_eq!(app.databases.entries.len(), 1);
        assert_eq!(app.preferences.discovery.recent.len(), 1);
        assert!(app.databases.entries[0].last_opened.is_some());
    }

    #[test]
    fn folder_selection_keeps_database_selection_and_can_find_extensionless_files() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("extensionless");
        let mut header = [0; 100];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        std::fs::write(&database, header).unwrap();
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.path = "selected.db".into();

        app.database.writable = true;
        app.databases.picking_folder = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        tx.send(Ok(Some(temp.path().into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.database.path, "selected.db");
        assert!(app.database.writable);
        assert!(
            app.preferences
                .discovery
                .roots
                .contains(&temp.path().to_path_buf())
        );
        app.poll_discovery(&ctx);
        assert!(app.databases.job.is_some());
        let start = Instant::now();
        while app.databases.job.is_some() {
            app.poll_discovery(&ctx);
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(
            app.databases
                .entries
                .iter()
                .any(|e| e.path == database && e.state == State::Found)
        );
        assert_eq!(std::fs::read(&database).unwrap(), header);
        assert_eq!(std::fs::read_dir(temp.path()).unwrap().count(), 1);
    }

    #[test]
    fn roots_are_normalized_exclusions_win_and_only_successful_opens_are_recorded() {
        let (_ctx, mut app) = crate::app::test_ui::app();
        app.add_search_folder("relative".into());
        app.add_search_folder("/".into());
        assert!(app.preferences.discovery.roots.is_empty());
        app.add_exclusion("/work/private".into());
        app.add_search_folder("/work/project/../private".into());
        assert!(app.preferences.discovery.roots.is_empty());
        app.set_database_path("/work/selected.db".into());
        assert!(app.preferences.discovery.recent.is_empty());
        app.database.tools.database_scope = "/work/selected.db".into();
        app.record_database_open();
        assert_eq!(app.preferences.discovery.recent.len(), 1);
        assert_eq!(
            app.preferences.discovery.recent[0].path,
            Path::new("/work/selected.db")
        );
        app.record_database_open();
        assert_eq!(app.preferences.discovery.recent.len(), 1);
        app.preferences.discovery.recent.push(RecentDatabase {
            path: "/work/project.db".into(),
            last_opened: 0,
        });
        let req = request(&app.preferences.discovery, Some("/work/private".into()));
        assert!(req.known.is_empty());
        assert_eq!(req.roots.len(), 1);
        assert!(req.roots[0].explicit);
    }

    #[test]
    fn cancellation_retains_the_job_until_it_acknowledges_completion() {
        let temp = tempfile::tempdir().unwrap();
        let (ctx, mut app) = crate::app::test_ui::app();
        app.start_discovery(&ctx, Some(temp.path().into()));
        app.databases.cancel();
        app.start_discovery(&ctx, Some(temp.path().join("replacement")));
        assert!(app.databases.cancelling);
        assert!(app.databases.job.is_some());
        let start = Instant::now();
        while app.databases.job.is_some() {
            app.poll_discovery(&ctx);
            assert!(start.elapsed() < Duration::from_secs(5));
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(app.databases.summary.is_some());
        assert!(!app.databases.cancelling);
    }
}
