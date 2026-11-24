use super::App;
use eframe::egui;

mod chooser;
mod connection;
mod inspection;
mod presentation;
pub(super) use presentation::{folder_label, opened_database_icon};
mod settings;
use crate::{
    discovery::{self, Availability, Candidate, DatabaseKind, Event, Job, Request, Root, Summary},
    storage::{CachedDatabase, DiscoveryPreferences, normalized_absolute},
};
pub(super) use settings::FolderList;
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

/// The row badge and the full description of a path the search checked.
fn checked_status(availability: Availability) -> (&'static str, &'static str) {
    match availability {
        Availability::Missing => ("Missing", "File no longer found"),
        Availability::Unavailable => ("Unreadable", "Could not read file"),
        Availability::NotDatabase => ("Not a database", "No recognized database header"),
        Availability::Skipped => (
            "Outside search scope",
            "Not searched · outside automatic search policy",
        ),
        Availability::ExcludedApplication => (
            "Use Browse",
            "Excluded application data · use Browse to select explicitly",
        ),
    }
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
    /// A path known from history or the search cache, not yet checked.
    fn remembered(path: PathBuf, last_opened: Option<u64>) -> Self {
        Self {
            path,
            state: State::Remembered,
            size: None,
            last_opened,
            kind: DatabaseKind::Unknown,
        }
    }

    fn badge(&self) -> Option<&'static str> {
        match self.state {
            State::Checked(availability) => Some(checked_status(availability).0),
            _ if self.path.to_str().is_none() => Some("Unsupported path"),
            _ => None,
        }
    }

    fn label(&self) -> &'static str {
        match (self.state, self.last_opened.is_some()) {
            (State::Checked(availability), _) => checked_status(availability).1,
            (State::Remembered, true) => "Opened before · awaiting header check",
            (State::Remembered, false) => "Remembered candidate · awaiting header check",
            (State::Found, true) => "Opened before · header checked",
            (State::Found, false) => "Database candidate · header checked",
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
    pub folder_input: String,
    pub folder_list: FolderList,
    pub picking_folder: Option<FolderList>,
    pub editing_path: bool,
    /// Height the open review took under its row last frame, with spacing.
    review_height: f32,
    /// The review already scrolled into view; a new selection reveals again.
    review_revealed: Option<PathBuf>,
    pub message: Option<String>,
    pub summary: Option<Summary>,
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
            let entry = Entry::remembered(recent.path.clone(), Some(recent.last_opened));
            chooser.entries.push(entry);
        }
        for cached in &prefs.cached {
            let size = Some(cached.size);
            match chooser.entry_mut(&cached.path) {
                Some(entry) => entry.size = size,
                None => chooser.entries.push(Entry {
                    size,
                    ..Entry::remembered(cached.path.clone(), None)
                }),
            }
        }
        chooser.entries.retain(|e| !prefs.excludes(&e.path));
        chooser.entries.sort_by(|a, b| {
            b.last_opened
                .cmp(&a.last_opened)
                .then_with(|| a.path.file_name().cmp(&b.path.file_name()))
                .then_with(|| a.path.cmp(&b.path))
        });
        chooser
    }
    fn entry_mut(&mut self, path: &Path) -> Option<&mut Entry> {
        self.entries.iter_mut().find(|e| e.path == path)
    }
    fn cancel(&mut self) {
        if let Some(job) = &self.job {
            job.cancel();
            self.cancelling = true;
        }
    }

    /// List and remember a found candidate unless it was forgotten or excluded.
    fn found(&mut self, prefs: &mut DiscoveryPreferences, candidate: Candidate) {
        let path = candidate.path;
        if self.forgotten.contains(&path) || prefs.excludes(&path) {
            return;
        }
        if let Some(identity) = candidate.identity {
            self.identities.insert(path.clone(), identity);
        }
        let size = candidate.size;
        prefs.remember(CachedDatabase {
            path: path.clone(),
            size,
            modified: candidate.modified,
        });
        let (state, kind, size) = (State::Found, candidate.kind, Some(size));
        if let Some(entry) = self.entry_mut(&path) {
            (entry.state, entry.kind, entry.size) = (state, kind, size);
        } else {
            // Append during a scan: preserve keyboard focus and scroll.
            self.entries.push(Entry {
                path,
                state,
                size,
                last_opened: None,
                kind,
            });
        }
    }

    /// A remembered path that is no longer a usable candidate.
    fn checked(&mut self, prefs: &mut DiscoveryPreferences, path: PathBuf, state: Availability) {
        self.identities.remove(&path);
        if let Some(entry) = self.entry_mut(&path) {
            entry.state = State::Checked(state);
            entry.kind = DatabaseKind::Unknown;
        }
        use Availability::{ExcludedApplication, Missing, NotDatabase};
        if matches!(state, Missing | NotDatabase | ExcludedApplication) {
            prefs.cached.retain(|c| c.path != path);
        }
        if state == ExcludedApplication {
            self.entries
                .retain(|e| e.path != path || e.last_opened.is_some());
        }
    }

    /// `path` is another name for `original`: keep one row, with the later open.
    fn alias(&mut self, prefs: &mut DiscoveryPreferences, path: PathBuf, original: PathBuf) {
        if let Some(identity) = self.identities.get(&original).cloned() {
            self.identities.insert(path.clone(), identity);
        }
        let recent = self.entry_mut(&path).and_then(|e| e.last_opened);
        self.entries.retain(|e| e.path != path);
        prefs.cached.retain(|e| e.path != path);
        if let Some(time) = recent
            && let Some(entry) = self.entry_mut(&original)
        {
            entry.last_opened = Some(entry.last_opened.unwrap_or(0).max(time));
        }
    }
}

fn request(prefs: &DiscoveryPreferences, folder: Option<PathBuf>) -> Request {
    let known = prefs.recent.iter().map(|e| &e.path);
    let known = known
        .chain(prefs.cached.iter().map(|e| &e.path))
        .filter(|p| folder.as_ref().is_none_or(|root| p.starts_with(root)))
        .cloned()
        .collect();
    let roots = match folder {
        Some(path) => vec![Root::explicit(path)],
        None => search_roots(prefs),
    };
    Request {
        roots,
        known,
        excluded: prefs.excluded.clone(),
        ..Default::default()
    }
}

fn search_roots(prefs: &DiscoveryPreferences) -> Vec<Root> {
    let var = |name| std::env::var_os(name).map(PathBuf::from);
    let (home, data, state) = (var("HOME"), var("XDG_DATA_HOME"), var("XDG_STATE_HOME"));
    let cwd = std::env::current_dir().ok();
    let (home, cwd, data, state) = (
        home.as_deref(),
        cwd.as_deref(),
        data.as_deref(),
        state.as_deref(),
    );
    configured_roots(prefs, discovery::default_roots(home, cwd, data, state))
}

/// The settings list and the worker use exactly the same roots and policies.
fn configured_roots(prefs: &DiscoveryPreferences, defaults: Vec<Root>) -> Vec<Root> {
    let added = prefs.roots.iter().cloned().map(Root::explicit);
    let recent = prefs.recent.iter().filter_map(|entry| {
        let folder = entry.path.parent().filter(|p| p.parent().is_some())?;
        Some(Root::automatic(folder.into(), 1, true))
    });
    let mut roots = Vec::<Root>::new();
    for mut root in added.chain(recent).chain(defaults) {
        let Some(path) = normalized_absolute(&root.path)
            .filter(|p| !prefs.removed_roots.contains(p) && !prefs.excludes(p))
        else {
            continue;
        };
        if let Some(previous) = roots.iter_mut().find(|r| r.path == path) {
            previous.depth = previous.depth.max(root.depth);
            previous.extensionless |= root.extensionless;
            previous.explicit |= root.explicit;
        } else {
            root.path = path;
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
        let prefs = &mut self.preferences.discovery;
        match event {
            Event::Found(candidate) => self.databases.found(prefs, candidate),
            Event::Checked(path, state) => self.databases.checked(prefs, path, state),
            Event::Alias { path, original } => self.databases.alias(prefs, path, original),
            Event::Finished(summary) => {
                self.databases.job = None;
                self.databases.cancelling = false;
                self.databases.summary = Some(summary);
                prefs.normalize();
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
        match self.databases.entry_mut(&path) {
            Some(entry) => entry.last_opened = Some(time),
            None => self
                .databases
                .entries
                .insert(0, Entry::remembered(path, Some(time))),
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
        if self.preferences.discovery.excludes(&path) {
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
        self.databases.folder_input.clear();
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
}

#[cfg(test)]
mod tests {
    use super::{
        Availability, CachedDatabase, Candidate, Chooser, DatabaseKind, DiscoveryPreferences,
        Event, Path, PathBuf, Root, State, presentation::EngineMark, request,
    };
    use crate::app::{
        App,
        test_ui::{self, key_event, pointer, render},
    };
    use crate::storage::RecentDatabase;
    use eframe::egui;
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    /// Run `step` until `done`, failing after five seconds.
    pub(super) fn wait(app: &mut App, mut step: impl FnMut(&mut App), done: fn(&App) -> bool) {
        let start = Instant::now();
        while !done(app) {
            step(app);
            assert!(
                start.elapsed() < Duration::from_secs(5),
                "{}",
                app.database.status
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    /// A real Turso database with one `example` table, in its own folder.
    pub(super) fn turso_file(name: &str) -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join(name);
        crate::test_support::create(&path, &["CREATE TABLE example(id INTEGER PRIMARY KEY)"]);
        (dir, path)
    }
    fn finish_search(app: &mut App, ctx: &egui::Context) {
        wait(
            app,
            |app| app.poll_discovery(ctx),
            |app| app.databases.job.is_none(),
        );
    }

    /// The navigation sidebar at `width` by `height`.
    fn sidebar(
        app: &mut App,
        ctx: &egui::Context,
        (width, height): (f32, f32),
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        render(ctx, egui::vec2(width, height), events, |ui| {
            app.navigation_sidebar(ui)
        })
    }

    /// A key press.
    fn key(key: egui::Key) -> Vec<egui::Event> {
        vec![key_event(key, true, egui::Modifiers::NONE)]
    }

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
        .map(|path| Root::automatic(path.into(), 3, false))
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
        let (ctx, mut app) = test_ui::app();
        app.discovery_event(found("/work/agenda.db"));
        app.discovery_event(found("/work/clipboard.db"));
        let frame = |app: &mut App, events| sidebar(app, &ctx, (300.0, 600.0), events);
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        frame(&mut app, vec![egui::Event::Text("agenda".into())]);
        for k in [egui::Key::Home, egui::Key::ArrowRight, egui::Key::ArrowDown] {
            frame(&mut app, key(k));
        }
        frame(&mut app, vec![egui::Event::Text("g".into())]);
        assert_eq!(app.sidebar_search.query, "aggenda");
        assert!(app.database.path.is_empty());
        app.sidebar_search.query.clear();
        app.sidebar_search.focus = true;
        frame(&mut app, vec![]);
        frame(
            &mut app,
            vec![
                egui::Event::Text("clipboard".into()),
                key_event(egui::Key::Enter, true, egui::Modifiers::NONE),
            ],
        );
        assert_eq!(app.database.path, "/work/clipboard.db");
        assert!(!app.database.writable);
        assert!(app.database.session.is_none());
    }

    #[test]
    fn available_rows_select_without_opening_and_skip_unavailable_candidates() {
        let (ctx, mut app) = test_ui::app();
        app.discovery_event(found("/work/a.db"));
        app.discovery_event(found("/work/missing.db"));
        app.discovery_event(Event::Checked(
            "/work/missing.db".into(),
            Availability::Missing,
        ));
        let frame = |app: &mut App, events| sidebar(app, &ctx, (272.0, 600.0), events);
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
        for k in [egui::Key::ArrowDown, egui::Key::ArrowDown, egui::Key::Enter] {
            frame(&mut app, key(k));
        }
        assert_eq!(app.database.path, "/work/a.db");
        assert!(app.database.session.is_none() && app.picker.is_none());
    }

    #[test]
    #[cfg(unix)]
    fn cached_identity_and_virtual_rows_bound_database_list_repaints() {
        let (dir, source) = turso_file("source.db");
        let alias = dir.path().join("alias.db");
        let (ctx, mut app) = test_ui::app();
        app.set_database_path(source.to_str().unwrap().into());
        test_ui::inspect_selected(&mut app, &ctx);
        app.open(&ctx);
        wait(
            &mut app,
            |app| app.poll(&ctx),
            |app| app.database.ready && app.database.catalog_ticket.is_none(),
        );
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
        let listed = app.available_search_results("");
        assert!(
            listed
                .iter()
                .all(|(i, _)| app.databases.entries[*i].path != alias)
        );
        let frame = |app: &mut App, events| sidebar(app, &ctx, (272.0, 600.0), events);
        crate::app::sessions::IDENTITY_PROBES.with(|count| count.set(0));
        frame(&mut app, vec![]);
        frame(&mut app, vec![]);
        assert!(app.databases.rendered_candidates < 20);
        crate::app::sessions::IDENTITY_PROBES
            .with(|count| assert_eq!(count.get(), 0, "painting must not stat paths"));
        frame(&mut app, key(egui::Key::ArrowDown));
        for _ in 0..6 {
            frame(&mut app, key(egui::Key::PageDown));
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
    fn access_review_opens_under_the_clicked_row_without_reordering_the_list() {
        let (ctx, mut app) = test_ui::app();
        let paths: Vec<_> = (0..40).map(|i| format!("/work/{i:03}.db")).collect();
        for path in &paths {
            app.discovery_event(found(path));
        }
        let frame = |app: &mut App, height, events| sidebar(app, &ctx, (272.0, height), events);
        frame(&mut app, 1000.0, vec![]);
        frame(&mut app, 1000.0, vec![]);
        let before = app.databases.row_rects.clone();
        assert_eq!(before[3].0, Path::new(&paths[3]));
        for pressed in [true, false] {
            frame(&mut app, 1000.0, pointer(before[3].1.center(), pressed));
        }
        for _ in 0..3 {
            frame(&mut app, 1000.0, vec![]);
        }
        assert_eq!(app.database.path, paths[3]);
        let after = app.databases.row_rects.clone();
        // Rows up to and including the clicked one have not moved.
        assert_eq!(before[..4], after[..4]);
        // The review sits between the clicked row and the next one.
        let open = app.databases.open_rect.unwrap();
        assert!(open.top() >= after[3].1.bottom());
        assert_eq!(after[4].0, Path::new(&paths[4]));
        assert!(after[4].1.top() >= open.bottom());
        // Everything below is displaced by one and the same amount.
        let shift = after[4].1.top() - before[4].1.top();
        assert!(shift > 34.0);
        assert_eq!(after[5].1.top() - before[5].1.top(), shift);
        // Cancelling closes the gap again.
        app.cancel_database_review();
        for _ in 0..2 {
            frame(&mut app, 1000.0, vec![]);
        }
        assert_eq!(app.databases.row_rects[..10], before[..10]);

        // A review that would open below the fold scrolls into view, and the
        // rows that scroll past it stay virtual.
        frame(&mut app, 420.0, vec![]);
        frame(&mut app, 420.0, vec![]);
        let (path, row) = app.databases.row_rects.last().unwrap().clone();
        for pressed in [true, false] {
            frame(&mut app, 420.0, pointer(row.center(), pressed));
        }
        for _ in 0..40 {
            frame(&mut app, 420.0, vec![]);
        }
        assert_eq!(Path::new(&app.database.path), path);
        let open = app.databases.open_rect.unwrap();
        assert!(open.bottom() <= 420.0, "{open:?}");
        assert!(app.databases.rendered_candidates <= 14);
    }

    #[test]
    fn available_search_keeps_filenames_visible_and_selection_opens_access_review() {
        let (ctx, mut app) = test_ui::app();
        let path = format!(
            "/home/demo/{}/notifications-v2.db",
            "long-folder/".repeat(12)
        );
        app.discovery_event(found(&path));
        app.databases.entries[0].kind = DatabaseKind::LikelySqlite;
        app.sidebar_search.query = "notifications".into();
        let frame = |app: &mut App, events| sidebar(app, &ctx, (272.0, 600.0), events);
        frame(&mut app, vec![]);
        let output = frame(&mut app, vec![]);
        assert!(output.shapes.iter().any(|s|matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == "notifications-v2.db")));
        assert!(
            output
                .shapes
                .iter()
                .any(|s| matches!(&s.shape,egui::Shape::Text(t) if t.galley.job.text == "SQLite"))
        );
        let row = test_ui::control(&output, &format!("{path} · Likely SQLite"));
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
        // Only a successful open, never a header guess, earns the checked mark.
        assert_eq!(chooser.entries[0].engine_mark(), EngineMark::OpenedTurso);
        let mut entry = chooser.entries[1].clone();
        for (kind, mark) in [
            (DatabaseKind::Unknown, EngineMark::Other),
            (DatabaseKind::LikelyTurso, EngineMark::LikelyTurso),
            (DatabaseKind::LikelySqlite, EngineMark::Other),
        ] {
            entry.kind = kind;
            assert_eq!(entry.engine_mark(), mark);
        }
        entry.last_opened = Some(1);
        assert_eq!(entry.engine_mark(), EngineMark::OpenedTurso);
        entry.state = State::Checked(Availability::NotDatabase);
        assert_eq!(entry.engine_mark(), EngineMark::Other);
    }

    #[test]
    fn incremental_discoveries_never_replace_paths_consent_or_row_order() {
        let (ctx, mut app) = test_ui::app();
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
        let (_ctx, mut app) = test_ui::app();
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
        let (_ctx, mut app) = test_ui::app();
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
        let (ctx, mut app) = test_ui::app();
        app.database.path = "selected.db".into();

        app.database.writable = true;
        app.databases.picking_folder = Some(super::FolderList::Search);
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        tx.send(Ok(Some(temp.path().into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.database.path, "selected.db");
        assert!(app.database.writable);
        assert_eq!(app.databases.folder_input, temp.path().to_str().unwrap());
        assert!(
            !app.preferences
                .discovery
                .roots
                .contains(&temp.path().to_path_buf())
        );
        app.add_search_folder(app.databases.folder_input.clone().into());
        assert!(
            app.preferences
                .discovery
                .roots
                .contains(&temp.path().to_path_buf())
        );
        app.poll_discovery(&ctx);
        assert!(app.databases.job.is_some());
        finish_search(&mut app, &ctx);
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
        let (_ctx, mut app) = test_ui::app();
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
        let (ctx, mut app) = test_ui::app();
        app.start_discovery(&ctx, Some(temp.path().into()));
        app.databases.cancel();
        app.start_discovery(&ctx, Some(temp.path().join("replacement")));
        assert!(app.databases.cancelling);
        assert!(app.databases.job.is_some());
        finish_search(&mut app, &ctx);
        assert!(app.databases.summary.is_some());
        assert!(!app.databases.cancelling);
    }
}
