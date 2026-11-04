use super::App;
use eframe::egui;

mod chooser;
mod connection;
mod inspection;
mod presentation;
mod settings;
mod switcher;
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
    filter: String,
    switcher_filter: String,
    highlighted: Option<PathBuf>,
    navigating: bool,
    reveal_highlight: bool,
    list_offset: f32,
    folder_input: String,
    exclude_input: String,
    pub focus_filter: bool,
    pub picking_folder: bool,
    pub editing_path: bool,
    pub(super) settings_open: bool,
    pub message: Option<String>,
    summary: Option<Summary>,
    cancelling: bool,
    pending_folder: Option<PathBuf>,
    forgotten: HashSet<PathBuf>,
    #[cfg(test)]
    pub row_rects: Vec<(PathBuf, egui::Rect)>,
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
    pub settings_rect: Option<egui::Rect>,
    #[cfg(test)]
    pub override_rect: Option<egui::Rect>,
}
impl Chooser {
    pub(super) fn new(prefs: &DiscoveryPreferences) -> Self {
        let mut chooser = Self {
            focus_filter: true,
            ..Default::default()
        };
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
        let Some(path) = normalized_absolute(Path::new(&self.tools.database_scope)) else {
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

    fn select_database(&mut self, path: &Path) {
        if self.picker.is_some() || self.session.is_some() {
            return;
        }
        if let Some(path) = path.to_str() {
            self.set_database_path(path.into());
            self.status = "Database selected. Review access before opening.".into();
        } else {
            self.status = "Only UTF-8 database paths are supported by the Turso engine".into();
        }
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
        self.databases.forgotten.insert(path.into());
        self.databases.entries.retain(|e| e.path != path);
        self.preferences.discovery.cached.retain(|e| e.path != path);
        self.preferences.discovery.recent.retain(|e| e.path != path);
        self.store.save(&self.preferences);
    }
}

#[cfg(test)]
mod tests {
    use super::{
        App, Availability, CachedDatabase, Chooser, DatabaseKind, DiscoveryPreferences, Event,
        Path, State, egui, request,
    };
    use crate::app::shell;
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
        })
    }
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>, width: f32) {
        let output = crate::app::test_ui::render(ctx, egui::vec2(width, 900.0), events, |ui| {
            app.database_chooser_ui(ui)
        });
        assert!(!output.shapes.is_empty());
    }
    fn click(app: &mut App, ctx: &egui::Context, position: egui::Pos2) {
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                crate::app::test_ui::pointer(position, pressed),
                900.0,
            );
        }
    }
    fn key(app: &mut App, ctx: &egui::Context, key: egui::Key) {
        frame(
            app,
            ctx,
            [true, false]
                .into_iter()
                .map(|pressed| egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                })
                .collect(),
            900.0,
        );
    }

    #[test]
    fn search_and_results_support_shelllist_arrow_keys_and_explicit_selection() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.path = "/explicit/database.db".into();

        app.writable = true;
        for path in ["/work/a.db", "/work/missing.db", "/work/b.db"] {
            app.discovery_event(found(path));
        }
        app.discovery_event(Event::Checked(
            "/work/missing.db".into(),
            Availability::Missing,
        ));
        frame(&mut app, &ctx, vec![], 900.0);
        key(&mut app, &ctx, egui::Key::ArrowUp);
        assert!(!app.databases.navigating);
        key(&mut app, &ctx, egui::Key::ArrowDown);
        assert!(app.databases.navigating);
        assert_eq!(
            app.databases.highlighted.as_deref(),
            Some(Path::new("/work/a.db"))
        );
        key(&mut app, &ctx, egui::Key::ArrowDown);
        assert_eq!(
            app.databases.highlighted.as_deref(),
            Some(Path::new("/work/b.db"))
        );
        assert_eq!(app.path, "/explicit/database.db");
        assert!(app.writable);
        key(&mut app, &ctx, egui::Key::Enter);
        assert_eq!(app.path, "/work/b.db");
        assert!(!app.writable);
        assert!(app.session.is_none() && app.picker.is_none());
        key(&mut app, &ctx, egui::Key::ArrowUp);
        assert!(app.databases.navigating);
        key(&mut app, &ctx, egui::Key::ArrowUp);
        assert!(!app.databases.navigating);
        assert!(ctx.memory(|m| m.has_focus(egui::Id::unique("database-search"))));
    }

    #[test]
    fn typing_from_results_preserves_the_native_search_cursor() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.discovery_event(found("/work/agenda.db"));
        frame(&mut app, &ctx, vec![], 900.0);
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("agenda".into())],
            900.0,
        );
        key(&mut app, &ctx, egui::Key::Home);
        key(&mut app, &ctx, egui::Key::ArrowRight);
        key(&mut app, &ctx, egui::Key::ArrowDown);
        assert!(app.databases.navigating);
        frame(&mut app, &ctx, vec![egui::Event::Text("g".into())], 900.0);
        assert_eq!(app.databases.filter, "aggenda");
        assert!(!app.databases.navigating);
        assert!(app.databases.row_rects.is_empty());
        key(&mut app, &ctx, egui::Key::ArrowDown);
        key(&mut app, &ctx, egui::Key::Enter);
        assert!(app.path.is_empty());
        assert!(app.session.is_none() && app.picker.is_none());
    }

    #[test]
    fn keyboard_navigation_reveals_virtualized_results_and_survives_async_updates() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        for i in 0..100 {
            app.discovery_event(found(&format!("/work/{i:03}.db")));
        }
        frame(&mut app, &ctx, vec![], 900.0);
        key(&mut app, &ctx, egui::Key::ArrowDown);
        for _ in 0..6 {
            key(&mut app, &ctx, egui::Key::PageDown);
        }
        assert_eq!(
            app.databases.highlighted.as_deref(),
            Some(Path::new("/work/030.db"))
        );
        assert!(app.databases.list_offset > 0.0);
        assert!(
            app.databases
                .row_rects
                .iter()
                .any(|(p, _)| Some(p) == app.databases.highlighted.as_ref())
        );
        let offset = app.databases.list_offset;
        app.discovery_event(found("/work/arriving.db"));
        frame(&mut app, &ctx, vec![], 900.0);
        assert_eq!(
            app.databases.highlighted.as_deref(),
            Some(Path::new("/work/030.db"))
        );
        assert_eq!(app.databases.list_offset, offset);
        assert!(app.path.is_empty());
    }
    #[test]
    fn typing_and_enter_in_one_frame_selects_the_new_filtered_result() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.discovery_event(found("/work/agenda.db"));
        app.discovery_event(found("/work/clipboard.db"));
        frame(&mut app, &ctx, vec![], 900.0);
        frame(
            &mut app,
            &ctx,
            vec![
                egui::Event::Text("clipboard".into()),
                egui::Event::Key {
                    key: egui::Key::Enter,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::NONE,
                },
            ],
            900.0,
        );
        assert_eq!(app.path, "/work/clipboard.db");
        assert!(app.session.is_none());
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        assert!(app.databases.job.is_none());
        app.path = "/explicit/command-line.db".into();

        app.writable = true;
        app.discovery_event(found("/work/z-last.db"));
        app.discovery_event(found("/work/a-first.db"));
        app.discovery_event(found("/work/z-last.db"));
        assert_eq!(app.path, "/explicit/command-line.db");
        assert!(app.writable);
        assert_eq!(app.databases.entries.len(), 2);
        assert_eq!(app.databases.entries[0].path, Path::new("/work/z-last.db"));
        app.select_database(Path::new("/work/a-first.db"));
        assert_eq!(app.path, "/work/a-first.db");
        assert!(!app.writable);
        assert!(app.session.is_none());
        app.writable = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        app.select_database(Path::new("/work/z-last.db"));
        assert_eq!(app.path, "/work/a-first.db");
        assert!(app.writable);
        drop(tx);
    }

    #[test]
    fn stale_entries_aliases_and_forgetting_do_not_reappear_from_pending_events() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.discovery_event(found("/data/custom-browser/History"));
        app.discovery_event(found("/data/custom-browser/Cookies"));
        app.tools.database_scope = "/data/custom-browser/History".into();
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
    fn chooser_filters_clicks_and_keyboard_focus_without_opening_a_native_dialog() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.discovery_event(found("/work/agenda.db"));
        app.discovery_event(found("/other/clipboard.db"));
        app.document.ui.nav = shell::Nav::Result;
        app.choose_file(&ctx);
        assert_eq!(app.document.ui.nav, shell::Nav::Databases);
        assert!(app.picker.is_none());
        frame(&mut app, &ctx, vec![], 900.0);
        assert!(ctx.memory(|m| m.focused().is_some()));
        frame(
            &mut app,
            &ctx,
            vec![egui::Event::Text("agenda".into())],
            900.0,
        );
        assert_eq!(app.databases.filter, "agenda");
        assert_eq!(app.databases.row_rects.len(), 1);
        let row = app.databases.row_rects[0].1;
        let browse = app.databases.browse_rect.unwrap();
        let search = app.databases.search_rect.unwrap();
        assert!(search.contains_rect(browse));
        assert_eq!(browse.center().y, search.center().y);
        assert!(row.top() > search.bottom());
        click(&mut app, &ctx, row.center());
        assert_eq!(app.path, "/work/agenda.db");
        assert!(!app.writable);
        assert!(app.session.is_none() && app.picker.is_none());
        // Large candidate lists render only the visible rows.
        app.databases.filter.clear();
        for i in 0..500 {
            app.discovery_event(found(&format!("/bulk/{i}.db")));
        }
        for width in [180.0, 300.0, 720.0, 900.0, 1920.0] {
            frame(&mut app, &ctx, vec![], width);
            assert!(app.databases.row_rects.len() < 10);
            assert!(app.databases.browse_rect.unwrap().right() <= width);
            assert!(
                app.databases
                    .search_rect
                    .unwrap()
                    .contains_rect(app.databases.browse_rect.unwrap())
            );
        }
    }

    #[test]
    fn folder_selection_keeps_database_selection_and_can_find_extensionless_files() {
        let temp = tempfile::tempdir().unwrap();
        let database = temp.path().join("extensionless");
        let mut header = [0; 100];
        header[..16].copy_from_slice(b"SQLite format 3\0");
        std::fs::write(&database, header).unwrap();
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.path = "selected.db".into();

        app.writable = true;
        app.databases.picking_folder = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        tx.send(Ok(Some(temp.path().into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.path, "selected.db");
        assert!(app.writable);
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.add_search_folder("relative".into());
        app.add_search_folder("/".into());
        assert!(app.preferences.discovery.roots.is_empty());
        app.add_exclusion("/work/private".into());
        app.add_search_folder("/work/project/../private".into());
        assert!(app.preferences.discovery.roots.is_empty());
        app.set_database_path("/work/selected.db".into());
        assert!(app.preferences.discovery.recent.is_empty());
        app.tools.database_scope = "/work/selected.db".into();
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
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
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
