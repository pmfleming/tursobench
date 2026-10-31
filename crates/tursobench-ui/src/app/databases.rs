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
            (State::Remembered, true) => "Opened before · not checked this launch",
            (State::Remembered, false) => "Remembered candidate · not checked this launch",
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
                .show(ui, |ui| {
                    for entry in filtered {
                        ui.push_id(&entry.path, |ui| {
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
