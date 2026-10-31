use super::*;
use crate::{
    discovery::{self, Availability, Event, Job, Request, Root, Summary},
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
}
impl Entry {
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
    pub job: Option<Job>,
    entries: Vec<Entry>,
    filter: String,
    folder_input: String,
    exclude_input: String,
    pub focus_filter: bool,
    pub picking_folder: bool,
    pub message: Option<String>,
    summary: Option<Summary>,
    cancelling: bool,
    pending_folder: Option<PathBuf>,
    forgotten: HashSet<PathBuf>,
    #[cfg(test)]
    pub row_rects: Vec<(PathBuf, egui::Rect)>,
    #[cfg(test)]
    pub browse_rect: Option<egui::Rect>,
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
    let mut roots = Vec::new();
    let known: Vec<_> = prefs
        .recent
        .iter()
        .map(|e| e.path.clone())
        .chain(prefs.cached.iter().map(|e| e.path.clone()))
        .filter(|p| folder.as_ref().is_none_or(|root| p.starts_with(root)))
        .collect();
    if let Some(path) = folder {
        roots.push(Root {
            path,
            depth: 5,
            extensionless: true,
            explicit: true,
        });
    } else {
        // Explicit folders and recent parents get the first slice of the budget.
        for path in &prefs.roots {
            roots.push(Root {
                path: path.clone(),
                depth: 5,
                extensionless: true,
                explicit: true,
            });
        }
        for recent in &prefs.recent {
            if let Some(parent) = recent.path.parent().filter(|p| p.parent().is_some()) {
                roots.push(Root {
                    path: parent.into(),
                    depth: 1,
                    extensionless: true,
                    explicit: false,
                });
            }
        }
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let cwd = std::env::current_dir().ok();
        let data = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from);
        roots.extend(discovery::default_roots(
            home.as_deref(),
            cwd.as_deref(),
            data.as_deref(),
        ));
    }
    Request {
        roots,
        known,
        excluded: prefs.excluded.clone(),
        ..Default::default()
    }
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
                    entry.size = Some(candidate.size);
                } else {
                    // Append during a scan: preserve keyboard focus and scroll.
                    self.databases.entries.push(Entry {
                        path: candidate.path,
                        state: State::Found,
                        size: Some(candidate.size),
                        last_opened: None,
                    });
                }
            }
            Event::Checked(path, state) => {
                if let Some(entry) = self.databases.entries.iter_mut().find(|e| e.path == path) {
                    entry.state = State::Checked(state);
                }
                if matches!(state, Availability::Missing | Availability::NotDatabase) {
                    self.preferences.discovery.cached.retain(|c| c.path != path);
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
            self.status =
                "Database selected. Review access and exclusive use before opening.".into();
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
        if !self.preferences.discovery.roots.contains(&path) {
            self.preferences.discovery.roots.insert(0, path.clone());
        }
        self.preferences.discovery.normalize();
        self.store.save(&self.preferences);
        self.databases.cancel();
        self.databases.pending_folder = Some(path);
        self.databases.folder_input.clear();
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
        self.databases.message =
            Some("Folder excluded. Refresh to search with the updated settings.".into());
    }

    fn forget_database(&mut self, path: &Path) {
        self.databases.forgotten.insert(path.into());
        self.databases.entries.retain(|e| e.path != path);
        self.preferences.discovery.cached.retain(|e| e.path != path);
        self.preferences.discovery.recent.retain(|e| e.path != path);
        self.store.save(&self.preferences);
    }

    pub(super) fn discovery_settings_ui(&mut self, ui: &mut egui::Ui) {
        if ui
            .checkbox(
                &mut self.preferences.discovery.enabled,
                "Search for databases on startup",
            )
            .changed()
        {
            if !self.preferences.discovery.enabled {
                self.databases.cancel();
            }
            self.store.save(&self.preferences);
        }
        ui.small("Searches likely folders with a 2-second budget. Reads headers only. Added folders also allow locations skipped by automatic discovery; symlink directories and nested mounts are never followed.");
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(
                    self.picker.is_none() && self.databases.job.is_none(),
                    egui::Button::new("Add search folder…"),
                )
                .clicked()
            {
                self.browse_search_folder(ui.ctx());
            }
        });
        ui.add(
            egui::TextEdit::singleline(&mut self.databases.folder_input)
                .hint_text("/absolute/search/folder")
                .char_limit(4096)
                .desired_width(ui.available_width()),
        );
        if ui
            .add_enabled(
                !self.databases.folder_input.is_empty(),
                egui::Button::new("Save and search this folder"),
            )
            .clicked()
        {
            self.add_search_folder(PathBuf::from(&self.databases.folder_input));
        }
        let mut remove = None;
        for (index, path) in self.preferences.discovery.roots.iter().enumerate() {
            ui.push_id(("search-root", index), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                    ui.add(egui::Label::new(path.to_string_lossy()).truncate())
                        .on_hover_text(path.to_string_lossy());
                })
            });
        }
        if let Some(index) = remove {
            self.preferences.discovery.roots.remove(index);
            self.databases.cancel();
            self.databases.pending_folder = None;
            self.store.save(&self.preferences);
        }
        ui.separator();
        ui.label("Excluded folders");
        ui.add(
            egui::TextEdit::singleline(&mut self.databases.exclude_input)
                .hint_text("/absolute/folder/to/skip")
                .char_limit(4096)
                .desired_width(ui.available_width()),
        );
        if ui
            .add_enabled(
                !self.databases.exclude_input.is_empty(),
                egui::Button::new("Exclude folder"),
            )
            .clicked()
        {
            self.add_exclusion(PathBuf::from(&self.databases.exclude_input));
        }
        let mut remove = None;
        for (index, path) in self.preferences.discovery.excluded.iter().enumerate() {
            ui.push_id(("excluded-root", index), |ui| {
                ui.horizontal_wrapped(|ui| {
                    if ui.small_button("Remove").clicked() {
                        remove = Some(index);
                    }
                    ui.add(egui::Label::new(path.to_string_lossy()).truncate())
                        .on_hover_text(path.to_string_lossy());
                })
            });
        }
        if let Some(index) = remove {
            self.preferences.discovery.excluded.remove(index);
            self.store.save(&self.preferences);
            self.databases.message =
                Some("Exclusion removed. Refresh to search that location.".into());
        }
    }

    pub(super) fn database_chooser_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.strong("Local databases");
            if self.databases.job.is_some() {
                ui.spinner();
                ui.label(if self.databases.cancelling {
                    "Stopping search…"
                } else {
                    "Searching…"
                });
                if ui
                    .add_enabled(!self.databases.cancelling, egui::Button::new("Stop"))
                    .clicked()
                {
                    self.databases.cancel();
                }
            } else if ui
                .add_enabled(self.picker.is_none(), egui::Button::new("Refresh"))
                .clicked()
            {
                self.start_discovery(ui.ctx(), None);
            }
        });
        let filter = ui.add(
            egui::TextEdit::singleline(&mut self.databases.filter)
                .hint_text("Filter databases by name or path… · Ctrl+O")
                .char_limit(256)
                .desired_width(ui.available_width().min(700.0)),
        );
        if std::mem::take(&mut self.databases.focus_filter) {
            filter.request_focus();
        }
        if let Some(summary) = &self.databases.summary {
            if let Some(error) = &summary.error {
                ui.colored_label(ui.visuals().warn_fg_color, error);
            }
            ui.small(format!(
                "{} · {} candidates · {} headers checked · {} entries · {} folders{}{}",
                if summary.cancelled {
                    "Search stopped"
                } else if summary.limited {
                    "Search limit reached"
                } else {
                    "Search finished"
                },
                summary.found,
                summary.probes,
                summary.entries,
                summary.directories,
                if summary.unavailable > 0 {
                    format!(" · {} locations unavailable", summary.unavailable)
                } else {
                    String::new()
                },
                if summary.skipped > 0 {
                    format!(" · {} locations skipped", summary.skipped)
                } else {
                    String::new()
                }
            ));
        }
        if let Some(message) = &self.databases.message {
            ui.label(message);
        }
        let search = self.databases.filter.to_lowercase();
        let filtered: Vec<_> = self
            .databases
            .entries
            .iter()
            .filter(|e| e.path.to_string_lossy().to_lowercase().contains(&search))
            .collect();
        let mut selected = None;
        let mut forget = None;
        #[cfg(test)]
        {
            self.databases.row_rects.clear();
        }
        if filtered.is_empty() {
            ui.label(if self.databases.entries.is_empty() {
                "No database candidates yet. Browse for a file or add a search folder."
            } else {
                "No databases match this filter."
            });
        } else {
            egui::ScrollArea::vertical()
                .id_salt("database-candidates")
                .max_height(240.0)
                .auto_shrink([false, true])
                .show_rows(ui, 80.0, filtered.len(), |ui, range| {
                    for entry in &filtered[range] {
                        ui.push_id(&entry.path, |ui| {
                            ui.set_min_height(80.0);
                            ui.horizontal(|ui| {
                                let name = entry
                                    .path
                                    .file_name()
                                    .unwrap_or(entry.path.as_os_str())
                                    .to_string_lossy();
                                let response = ui
                                    .add_enabled(
                                        self.picker.is_none() && entry.selectable(),
                                        egui::Button::new(name)
                                            .selected(Path::new(&self.path) == entry.path)
                                            .truncate()
                                            .min_size(egui::vec2(
                                                (ui.available_width() - 75.0).max(40.0),
                                                26.0,
                                            )),
                                    )
                                    .on_hover_text(entry.path.to_string_lossy());
                                #[cfg(test)]
                                {
                                    self.databases
                                        .row_rects
                                        .push((entry.path.clone(), response.rect));
                                }
                                if response.clicked() {
                                    selected = Some(entry.path.clone());
                                }
                                if ui
                                    .small_button("Forget")
                                    .on_hover_text(
                                        "Remove saved path. A later search may find it again.",
                                    )
                                    .clicked()
                                {
                                    forget = Some(entry.path.clone());
                                }
                            });
                            ui.add(
                                egui::Label::new(
                                    entry.path.parent().unwrap_or(&entry.path).to_string_lossy(),
                                )
                                .truncate(),
                            )
                            .on_hover_text(entry.path.to_string_lossy());
                            ui.small(format!(
                                "{}{}{}",
                                entry.label(),
                                entry
                                    .size
                                    .map(|bytes| format!(" · {} KiB", bytes.div_ceil(1024)))
                                    .unwrap_or_default(),
                                if entry.path.to_str().is_none() {
                                    " · path unsupported by engine"
                                } else {
                                    ""
                                }
                            ));
                            ui.add_space(4.0);
                        });
                    }
                });
        }
        if let Some(path) = selected {
            self.select_database(&path);
        }
        if let Some(path) = forget {
            self.forget_database(&path);
        }
        ui.small("Candidates share the SQLite file format; the creating engine cannot be identified reliably. Compatibility is checked when opening. Search covers likely folders, not the whole disk.");
        let browse = ui.add_enabled(
            self.picker.is_none(),
            egui::Button::new("Browse for another file…"),
        );
        #[cfg(test)]
        {
            self.databases.browse_rect = Some(browse.rect);
        }
        if browse.clicked() {
            self.browse_file(ui.ctx());
        }
        egui::CollapsingHeader::new("Search folders and exclusions")
            .id_salt("discovery-settings")
            .show(ui, |ui| self.discovery_settings_ui(ui));
        ui.separator();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{discovery::Candidate, storage::RecentDatabase};
    use std::{
        sync::mpsc,
        time::{Duration, Instant},
    };

    fn found(path: &str) -> Event {
        Event::Found(Candidate {
            path: path.into(),
            size: 4096,
            modified: Some(1),
        })
    }
    fn frame(app: &mut App, ctx: &egui::Context, events: Vec<egui::Event>, width: f32) {
        let mut output = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(width, 900.0),
                )),
                events,
                ..Default::default()
            },
            |ui| app.database_chooser_ui(ui),
        );
        output.textures_delta.clear();
        assert!(!output.shapes.is_empty());
    }
    fn click(app: &mut App, ctx: &egui::Context, position: egui::Pos2) {
        for pressed in [true, false] {
            frame(
                app,
                ctx,
                vec![
                    egui::Event::PointerMoved(position),
                    egui::Event::PointerButton {
                        pos: position,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: egui::Modifiers::NONE,
                    },
                ],
                900.0,
            );
        }
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
        app.exclusive = true;
        app.writable = true;
        app.discovery_event(found("/work/z-last.db"));
        app.discovery_event(found("/work/a-first.db"));
        app.discovery_event(found("/work/z-last.db"));
        assert_eq!(app.path, "/explicit/command-line.db");
        assert!(app.exclusive && app.writable);
        assert_eq!(app.databases.entries.len(), 2);
        assert_eq!(app.databases.entries[0].path, Path::new("/work/z-last.db"));
        app.select_database(Path::new("/work/a-first.db"));
        assert_eq!(app.path, "/work/a-first.db");
        assert!(!app.exclusive && !app.writable);
        assert!(app.session.is_none());
        app.exclusive = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        app.select_database(Path::new("/work/z-last.db"));
        assert_eq!(app.path, "/work/a-first.db");
        assert!(app.exclusive);
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
    fn chooser_filters_clicks_and_keyboard_focus_without_opening_a_native_dialog() {
        let ctx = egui::Context::default();
        let mut app = App::new(&ctx);
        app.discovery_event(found("/work/agenda.db"));
        app.discovery_event(found("/other/clipboard.db"));
        app.ui.workspace = shell::Workspace::Query;
        app.choose_file(&ctx);
        assert_eq!(app.ui.workspace, shell::Workspace::Connect);
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
        assert!(browse.top() > row.bottom());
        click(&mut app, &ctx, row.center());
        assert_eq!(app.path, "/work/agenda.db");
        assert!(!app.exclusive && !app.writable);
        assert!(app.session.is_none() && app.picker.is_none());
        // Large candidate lists render only the visible rows.
        app.databases.filter.clear();
        for i in 0..500 {
            app.discovery_event(found(&format!("/bulk/{i}.db")));
        }
        for width in [720.0, 900.0, 1920.0] {
            frame(&mut app, &ctx, vec![], width);
            assert!(app.databases.row_rects.len() < 10);
            assert!(app.databases.browse_rect.unwrap().right() <= width);
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
        app.exclusive = true;
        app.writable = true;
        app.databases.picking_folder = true;
        let (tx, rx) = mpsc::sync_channel(1);
        app.picker = Some(rx);
        tx.send(Ok(Some(temp.path().into()))).unwrap();
        app.poll_picker();
        assert_eq!(app.path, "selected.db");
        assert!(app.exclusive && app.writable);
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
