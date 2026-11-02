//! Best-effort discovery: bounded directory traversal and 100-byte reads only.
//! Never instantiate an engine, follow directory links, or inspect database pages.
use std::{
    collections::{HashMap, HashSet, VecDeque},
    fs::{self, Metadata, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant, UNIX_EPOCH},
};
mod classification;
pub use classification::DatabaseKind;

#[derive(Clone, Debug)]
pub struct Root {
    pub path: PathBuf,
    /// Number of directory levels below this root to visit; zero is files only.
    pub depth: usize,
    pub extensionless: bool,
    /// Only user-added roots may opt into otherwise excluded locations.
    pub explicit: bool,
}

#[derive(Clone, Debug)]
pub struct Budget {
    pub duration: Duration,
    pub entries: usize,
    pub directories: usize,
    pub probes: usize,
}
impl Default for Budget {
    fn default() -> Self {
        Self {
            duration: Duration::from_secs(2),
            entries: 20_000,
            directories: 1_000,
            probes: 500,
        }
    }
}

#[derive(Default)]
pub struct Request {
    pub roots: Vec<Root>,
    pub known: Vec<PathBuf>,
    pub excluded: Vec<PathBuf>,
    pub budget: Budget,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Candidate {
    pub path: PathBuf,
    pub size: u64,
    pub modified: Option<u64>,
    pub kind: DatabaseKind,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Availability {
    Missing,
    Unavailable,
    NotDatabase,
    Skipped,
    ExcludedApplication,
}

#[derive(Debug)]
pub enum Event {
    Found(Candidate),
    Checked(PathBuf, Availability),
    Alias { path: PathBuf, original: PathBuf },
    Finished(Summary),
}

#[derive(Default, Debug)]
pub struct Summary {
    pub entries: usize,
    pub directories: usize,
    pub probes: usize,
    pub found: usize,
    pub unavailable: usize,
    pub skipped: usize,
    pub limited: bool,
    pub cancelled: bool,
    pub error: Option<String>,
}

pub struct Job {
    pub events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn start(request: Request, wake: impl Fn() + Send + 'static) -> io::Result<Self> {
        let (tx, events) = mpsc::sync_channel(32);
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("database-discovery".into())
            .spawn(move || {
                // A failed worker must leave the chooser usable and refreshable.
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    scan(request, &Mounts::load(), &flag, |event| {
                        let sent = tx.send(event).is_ok();
                        wake();
                        sent
                    })
                }));
                let summary = result.unwrap_or_else(|_| Summary {
                    error: Some("Database search stopped unexpectedly".into()),
                    ..Default::default()
                });
                let _ = tx.send(Event::Finished(summary));
                wake();
            })?;
        Ok(Self { events, cancel })
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
        // Dropping the receiver also unblocks a worker waiting to deliver results.
        // Do not join on the UI thread: filesystem operations may be slow.
    }
}

/// Build roots without touching the filesystem. Existence and mount checks run
/// on the worker. A broad launch directory never becomes a recursive root.
pub fn default_roots(
    home: Option<&Path>,
    cwd: Option<&Path>,
    data: Option<&Path>,
    state: Option<&Path>,
) -> Vec<Root> {
    let mut roots = Vec::new();
    // Visit application storage before broad development trees consume the budget.
    for (configured, fallback) in [(data, ".local/share"), (state, ".local/state")] {
        if let Some(path) = configured
            .filter(|p| p.is_absolute() && p.parent().is_some())
            .map(Path::to_path_buf)
            .or_else(|| {
                home.filter(|p| p.is_absolute() && p.parent().is_some())
                    .map(|p| p.join(fallback))
            })
        {
            roots.push(Root {
                path,
                depth: 3,
                extensionless: true,
                explicit: false,
            });
        }
    }
    if let Some(cwd) = cwd.filter(|p| p.is_absolute() && p.parent().is_some()) {
        roots.push(Root {
            path: cwd.into(),
            depth: if Some(cwd) == home || system_path(cwd) {
                0
            } else {
                5
            },
            extensionless: false,
            explicit: false,
        });
    }
    if let Some(home) = home.filter(|p| p.is_absolute() && p.parent().is_some()) {
        roots.push(Root {
            path: home.into(),
            depth: 0,
            extensionless: false,
            explicit: false,
        });
        for name in [
            "Projects",
            "projects",
            "Code",
            "code",
            "Developer",
            "Development",
            "src",
        ] {
            roots.push(Root {
                path: home.join(name),
                depth: 5,
                extensionless: false,
                explicit: false,
            });
        }
    }
    roots
}

/// Also used for explicit selections: bounded reads, no connection or sidecars.
pub fn inspect_kind(path: &Path) -> io::Result<DatabaseKind> {
    if !no_symlink_components(path)? {
        return Err(io::Error::other("Symlink paths are not inspected"));
    }
    read_candidate(path)?
        .map(|(_, candidate)| candidate.kind)
        .ok_or_else(|| io::Error::other("No recognized database header"))
}

fn read_candidate(path: &Path) -> io::Result<Option<(Metadata, Candidate)>> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NONBLOCK | libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() || metadata.len() < 100 {
        return Ok(None);
    }
    let mut header = [0; 100];
    file.read_exact(&mut header)?;
    if &header[..16] != b"SQLite format 3\0" {
        return Ok(None);
    }
    let candidate = Candidate {
        path: path.into(),
        size: metadata.len(),
        modified: metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
            .map(|t| t.as_secs()),
        kind: DatabaseKind::from_header(&header),
    };
    Ok(Some((metadata, candidate)))
}

fn system_path(path: &Path) -> bool {
    [
        "/proc",
        "/sys",
        "/dev",
        "/run",
        "/media",
        "/mnt",
        "/net",
        "/nix",
        "/usr",
        "/bin",
        "/sbin",
        "/lib",
        "/lib64",
        "/etc",
        "/var",
        "/boot",
        "/root",
        "/lost+found",
    ]
    .iter()
    .any(|root| path.starts_with(root))
        || path == Path::new("/home")
}

fn skip_directory(path: &Path) -> bool {
    let Some(name) = path.file_name().and_then(|s| s.to_str()) else {
        return true;
    };
    if name.starts_with('.') && !matches!(name, ".data" | ".turso" | ".libsql" | ".local") {
        return true;
    }
    if application_directory_name(name) {
        return true;
    }
    matches!(
        name.to_ascii_lowercase().as_str(),
        "node_modules"
            | "target"
            | "vendor"
            | "build"
            | "dist"
            | "result"
            | "__pycache__"
            | "venv"
            | "env"
            | "site-packages"
            | "cache"
            | "caches"
            | "trash"
            | "trash-1000"
            | "mozilla"
            | "firefox"
            | "chromium"
            | "google-chrome"
            | "brave"
            | "bravesoftware"
            | "vivaldi"
            | "microsoft-edge"
            | "webkitgtk"
            | "webkit"
            | "electron"
            | "steam"
            | "flatpak"
            | "containers"
            | "docker"
            | "nix"
            | "lost+found"
    )
}

fn application_directory_name(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "pki" | "nssdb" | "keyrings" | "evolution" | "nautilus"
    ) || name.split(['-', '_', '.']).any(|part| {
        matches!(
            part,
            "chrome"
                | "chromium"
                | "firefox"
                | "mozilla"
                | "brave"
                | "bravesoftware"
                | "vivaldi"
                | "webkit"
                | "webkitgtk"
        )
    })
}

fn candidate_name(path: &Path, extensionless: bool) -> bool {
    let Some(name) = path.file_name() else {
        return false;
    };
    // Lossy text is used only for matching, never to construct or open a path.
    let name = name.to_string_lossy().to_ascii_lowercase();
    if [
        "-wal", "-shm", "-journal", "-log", ".wal", ".shm", ".journal",
    ]
    .iter()
    .any(|suffix| name.ends_with(suffix))
    {
        return false;
    }
    matches!(
        path.extension()
            .and_then(|s| s.to_str())
            .map(str::to_ascii_lowercase)
            .as_deref(),
        Some("db" | "sqlite" | "sqlite3" | "db3" | "s3db" | "turso")
    ) || (extensionless && path.extension().is_none())
}

#[derive(Default)]
struct Mounts {
    points: HashSet<PathBuf>,
    blocked: Vec<PathBuf>,
    reliable: bool,
}
impl Mounts {
    fn load() -> Self {
        #[cfg(target_os = "linux")]
        {
            // Read proc metadata, never enumerate mountpoints themselves. In
            // particular, stat/read_dir on an autofs path could trigger a mount.
            let Ok(text) = fs::read_to_string("/proc/self/mountinfo") else {
                return Self::default();
            };
            let mut mounts = Self::parse(&text);
            let mut devices = HashMap::new();
            for line in text.lines() {
                let fields: Vec<_> = line.split_whitespace().collect();
                if fields.len() < 6 {
                    continue;
                }
                let device = fields[2];
                let removable = *devices.entry(device).or_insert_with(|| {
                    let mut path = fs::canonicalize(format!("/sys/dev/block/{device}")).ok();
                    while let Some(current) = path {
                        if fs::read_to_string(current.join("removable"))
                            .is_ok_and(|s| s.trim() == "1")
                        {
                            return true;
                        }
                        if current == Path::new("/sys/devices") {
                            break;
                        }
                        path = current.parent().map(Path::to_path_buf);
                    }
                    false
                });
                if removable {
                    mounts.blocked.push(unescape_mount(fields[4]));
                }
            }
            mounts
        }
        #[cfg(not(target_os = "linux"))]
        {
            Self::default()
        }
    }
    fn parse(text: &str) -> Self {
        let mut result = Self::default();
        for line in text.lines() {
            let Some((left, right)) = line.split_once(" - ") else {
                continue;
            };
            let Some(point) = left.split_whitespace().nth(4) else {
                continue;
            };
            let Some(kind) = right.split_whitespace().next() else {
                continue;
            };
            let path = unescape_mount(point);
            // Unknown/FUSE filesystems are conservatively excluded. User-added
            // roots can opt in; traversal still will not cross another mount.
            if !matches!(
                kind,
                "ext2"
                    | "ext3"
                    | "ext4"
                    | "btrfs"
                    | "xfs"
                    | "zfs"
                    | "f2fs"
                    | "tmpfs"
                    | "overlay"
                    | "ramfs"
                    | "bcachefs"
            ) {
                result.blocked.push(path.clone());
            }
            result.points.insert(path);
        }
        result.reliable = result.points.contains(Path::new("/"));
        result
    }
    fn automatic_allowed(&self, path: &Path) -> bool {
        self.reliable && !system_path(path) && !self.blocked.iter().any(|p| path.starts_with(p))
    }
    fn boundary(&self, path: &Path) -> bool {
        self.points.contains(path)
    }
    fn explicit_allowed(&self, root: &Path, path: &Path) -> bool {
        path.starts_with(root)
            && !self
                .points
                .iter()
                .any(|p| p != root && p.starts_with(root) && path.starts_with(p))
    }
}

fn unescape_mount(value: &str) -> PathBuf {
    let bytes = value.as_bytes();
    let mut result = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\'
            && index + 3 < bytes.len()
            && bytes[index + 1..=index + 3]
                .iter()
                .all(|b| (b'0'..=b'7').contains(b))
        {
            result.push(
                (bytes[index + 1] - b'0') * 64 + (bytes[index + 2] - b'0') * 8 + bytes[index + 3]
                    - b'0',
            );
            index += 4;
        } else {
            result.push(bytes[index]);
            index += 1;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(result).into()
    }
    #[cfg(not(unix))]
    {
        String::from_utf8_lossy(&result).into_owned().into()
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
enum Identity {
    #[cfg(unix)]
    Inode(u64, u64),
    #[cfg(not(unix))]
    Path(PathBuf),
}
fn identity(metadata: &Metadata, _path: &Path) -> Identity {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Identity::Inode(metadata.dev(), metadata.ino())
    }
    #[cfg(not(unix))]
    {
        let _ = metadata;
        Identity::Path(_path.into())
    }
}

/// Reject links in ancestors too: an innocent-looking project root can live
/// below a symlink into a network mount. Called only after mount policy checks.
fn no_symlink_components(path: &Path) -> io::Result<bool> {
    let mut prefix = PathBuf::new();
    for part in path.components() {
        prefix.push(part);
        if fs::symlink_metadata(&prefix)?.file_type().is_symlink() {
            return Ok(false);
        }
    }
    Ok(true)
}

struct Scanner<'a, F> {
    request: Request,
    mounts: &'a Mounts,
    cancel: &'a AtomicBool,
    emit: F,
    start: Instant,
    summary: Summary,
    files: HashMap<Identity, PathBuf>,
    probed_paths: HashSet<PathBuf>,
    application_dirs: HashMap<PathBuf, bool>,
}
impl<F: FnMut(Event) -> bool> Scanner<'_, F> {
    fn stopped(&mut self) -> bool {
        if self.cancel.load(Ordering::Relaxed) {
            self.summary.cancelled = true;
            return true;
        }
        if self.start.elapsed() >= self.request.budget.duration {
            self.summary.limited = true;
            return true;
        }
        false
    }
    fn excluded(&self, path: &Path) -> bool {
        self.request.excluded.iter().any(|p| path.starts_with(p))
    }
    fn application_path(&mut self, path: &Path) -> bool {
        // Called only after mount and symlink checks. Cache marker checks across
        // siblings and remembered files; never read browser settings or content.
        for directory in path.ancestors() {
            if directory
                .file_name()
                .and_then(|s| s.to_str())
                .is_some_and(application_directory_name)
            {
                return true;
            }
            let ignored = *self
                .application_dirs
                .entry(directory.into())
                .or_insert_with(|| {
                    let regular = |name: &str| {
                        let marker = directory.join(name);
                        !self.mounts.boundary(&marker)
                            && fs::symlink_metadata(marker).is_ok_and(|m| m.is_file())
                    };
                    // Chromium profiles can be inside arbitrarily named applications.
                    // Firefox also permits arbitrary profile and installation names.
                    regular("Local State")
                        || regular("profiles.ini")
                        || (regular("Preferences") && (regular("History") || regular("Login Data")))
                        || (regular("prefs.js") && regular("places.sqlite"))
                });
            if ignored {
                return true;
            }
        }
        false
    }
    fn send(&mut self, event: Event) {
        if !(self.emit)(event) {
            self.summary.cancelled = true;
            self.cancel.store(true, Ordering::Relaxed);
        }
    }
    fn checked(&mut self, path: &Path, availability: Availability, known: bool) {
        if availability == Availability::Unavailable {
            self.summary.unavailable += 1;
        }
        if known {
            self.send(Event::Checked(path.into(), availability));
        }
    }
    fn probe(&mut self, path: &Path, known: bool) {
        if self.stopped() {
            return;
        }
        if !self.probed_paths.insert(path.into()) {
            return;
        }
        if self.summary.probes >= self.request.budget.probes {
            self.summary.limited = true;
            return;
        }
        self.summary.probes += 1;
        // O_NONBLOCK and O_NOFOLLOW keep replaced FIFOs and symlinks from
        // blocking the worker or redirecting its header reads.
        let result = read_candidate(path);
        match result {
            Ok(Some((metadata, candidate))) => {
                let id = identity(&metadata, path);
                if let Some(original) = self.files.get(&id) {
                    if known && original != path {
                        self.send(Event::Alias {
                            path: path.into(),
                            original: original.clone(),
                        });
                    }
                } else {
                    self.files.insert(id, path.into());
                    self.summary.found += 1;
                    self.send(Event::Found(candidate));
                }
            }
            Ok(None) => self.checked(path, Availability::NotDatabase, known),
            Err(e) => self.checked(
                path,
                if e.kind() == io::ErrorKind::NotFound {
                    Availability::Missing
                } else {
                    Availability::Unavailable
                },
                known,
            ),
        }
    }
    fn check_known(&mut self, path: &Path) {
        let explicit = self
            .request
            .roots
            .iter()
            .any(|r| r.explicit && self.mounts.explicit_allowed(&r.path, path));
        let beneath_explicit = self
            .request
            .roots
            .iter()
            .any(|r| r.explicit && path.starts_with(&r.path));
        if self.excluded(path)
            || (beneath_explicit && !explicit)
            || (!explicit && !self.mounts.automatic_allowed(path))
        {
            self.checked(path, Availability::Skipped, true);
        } else {
            match no_symlink_components(path) {
                Ok(true) => {
                    if !explicit && path.parent().is_some_and(|p| self.application_path(p)) {
                        self.checked(path, Availability::ExcludedApplication, true);
                    } else {
                        self.probe(path, true);
                    }
                }
                Ok(false) => self.checked(path, Availability::Skipped, true),
                Err(e) => self.checked(
                    path,
                    if e.kind() == io::ErrorKind::NotFound {
                        Availability::Missing
                    } else {
                        Availability::Unavailable
                    },
                    true,
                ),
            }
        }
    }
    fn directory(&mut self, root: &Root, device: Option<u64>) -> Option<(Metadata, u64)> {
        if self.excluded(&root.path)
            || (!root.explicit && !self.mounts.automatic_allowed(&root.path))
        {
            self.summary.skipped += 1;
            return None;
        }
        // Check ancestry before opening a directory, including a parent
        // replaced with a symlink since it was queued.
        match no_symlink_components(&root.path) {
            Ok(true) => {}
            Ok(false) => {
                self.summary.skipped += 1;
                return None;
            }
            Err(e) => {
                if root.explicit || e.kind() != io::ErrorKind::NotFound {
                    self.summary.unavailable += 1;
                }
                return None;
            }
        }
        if !root.explicit && self.application_path(&root.path) {
            self.summary.skipped += 1;
            return None;
        }
        let metadata = match fs::symlink_metadata(&root.path) {
            Ok(m) if m.is_dir() && !m.file_type().is_symlink() => m,
            Err(e) if e.kind() != io::ErrorKind::NotFound => {
                self.summary.unavailable += 1;
                return None;
            }
            _ => {
                if root.explicit {
                    self.summary.unavailable += 1;
                }
                return None;
            }
        };
        #[cfg(unix)]
        let current_device = {
            use std::os::unix::fs::MetadataExt;
            metadata.dev()
        };
        #[cfg(not(unix))]
        let current_device = 0u64;
        if device.is_some_and(|d| d != current_device) {
            self.summary.skipped += 1;
            return None;
        }
        Some((metadata, current_device))
    }
    fn entry(
        &mut self,
        entry: io::Result<fs::DirEntry>,
        root: &Root,
        current_device: u64,
        queue: &mut VecDeque<(Root, Option<u64>)>,
    ) {
        let entry = match entry {
            Ok(e) => e,
            Err(_) => {
                self.summary.unavailable += 1;
                return;
            }
        };
        let path = entry.path();
        if self.excluded(&path) {
            return;
        }
        // Never touch an automount entry before testing mount policy.
        if self.mounts.boundary(&path) {
            self.summary.skipped += 1;
            return;
        }
        let kind = match entry.file_type() {
            Ok(t) => t,
            Err(_) => {
                self.summary.unavailable += 1;
                return;
            }
        };
        if kind.is_dir() && root.depth > 0 && !skip_directory(&path) {
            queue.push_back((
                Root {
                    path,
                    depth: root.depth - 1,
                    extensionless: root.extensionless,
                    explicit: root.explicit,
                },
                Some(current_device),
            ));
        } else if kind.is_file() && candidate_name(&path, root.extensionless) {
            self.probe(&path, false);
        }
    }
    fn entry_limit(&mut self) -> bool {
        let limited = self.summary.entries >= self.request.budget.entries
            || self.summary.probes >= self.request.budget.probes;
        self.summary.limited |= limited;
        limited
    }
    fn run(mut self) -> Summary {
        // Revalidate remembered files before traversing directories. Explicit
        // search roots can authorize network locations, never user exclusions.
        for path in std::mem::take(&mut self.request.known) {
            if self.stopped() {
                return self.summary;
            }
            self.check_known(&path);
        }
        let mut queue: VecDeque<_> = std::mem::take(&mut self.request.roots)
            .into_iter()
            .map(|root| (root, None))
            .collect();
        let mut visited: HashMap<(Identity, bool), usize> = HashMap::new();
        while let Some((root, device)) = queue.pop_front() {
            if self.stopped() || self.entry_limit() {
                break;
            }
            if self.summary.directories >= self.request.budget.directories {
                self.summary.limited = true;
                break;
            }
            let Some((metadata, current_device)) = self.directory(&root, device) else {
                continue;
            };
            let key = (identity(&metadata, &root.path), root.extensionless);
            if visited.get(&key).is_some_and(|depth| *depth >= root.depth) {
                continue;
            }
            visited.insert(key, root.depth);
            self.summary.directories += 1;
            let entries = match fs::read_dir(&root.path) {
                Ok(entries) => entries,
                Err(_) => {
                    self.summary.unavailable += 1;
                    continue;
                }
            };
            for entry in entries {
                if self.stopped() || self.entry_limit() {
                    break;
                }
                self.summary.entries += 1;
                self.entry(entry, &root, current_device, &mut queue);
            }
        }
        self.summary.cancelled |= self.cancel.load(Ordering::Relaxed);
        self.summary
    }
}

fn scan(
    request: Request,
    mounts: &Mounts,
    cancel: &AtomicBool,
    emit: impl FnMut(Event) -> bool,
) -> Summary {
    Scanner {
        request,
        mounts,
        cancel,
        emit,
        start: Instant::now(),
        summary: Summary::default(),
        files: HashMap::new(),
        probed_paths: HashSet::new(),
        application_dirs: HashMap::new(),
    }
    .run()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn database(path: &Path) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut bytes = [0; 100];
        bytes[..16].copy_from_slice(b"SQLite format 3\0");
        fs::write(path, bytes).unwrap();
    }
    fn request(path: &Path) -> Request {
        Request {
            roots: vec![Root {
                path: path.into(),
                depth: 5,
                extensionless: true,
                explicit: true,
            }],
            ..Default::default()
        }
    }
    fn run(request: Request) -> (Vec<Event>, Summary) {
        let mut events = vec![];
        let summary = scan(request, &Mounts::default(), &AtomicBool::new(false), |e| {
            events.push(e);
            true
        });
        (events, summary)
    }
    fn paths(events: &[Event]) -> Vec<PathBuf> {
        events
            .iter()
            .filter_map(|e| {
                if let Event::Found(c) = e {
                    Some(c.path.clone())
                } else {
                    None
                }
            })
            .collect()
    }
    #[test]
    fn finds_headers_in_likely_files_without_following_gitignore() {
        let temp = tempfile::tempdir().unwrap();
        for name in [
            "app.db",
            "nested/APP.SQLITE3",
            ".data/store",
            "native.turso",
        ] {
            database(&temp.path().join(name));
        }
        for name in [
            "target/build.db",
            "node_modules/dependency.db",
            ".git/objects/fake.db",
            "chromium/History",
            "cache/old.db",
            "main.db-wal",
            "main.db-shm",
            "main.db-journal",
            "main.db-log",
            "wrong.txt",
        ] {
            database(&temp.path().join(name));
        }
        fs::write(temp.path().join(".gitignore"), "*.db\n.data\n").unwrap();
        fs::write(temp.path().join("fake.db"), [0; 100]).unwrap();
        fs::write(temp.path().join("empty.db"), []).unwrap();
        let (events, summary) = run(request(temp.path()));
        assert_eq!(paths(&events).len(), 4);
        assert_eq!(summary.found, 4);
        assert!(!summary.limited);
    }
    #[test]
    fn custom_browser_profiles_and_platform_stores_are_excluded_including_old_cache_entries() {
        let temp = tempfile::tempdir().unwrap();
        let browser = temp.path().join("arbitrarily-named-app");
        let history = browser.join("Default/History");
        database(&history);
        fs::write(browser.join("Local State"), "{}").unwrap();
        for name in [
            "captive-portal-chrome/first_party_sets.db",
            "chrome-web-apps/app/Default/History",
            "pki/nssdb/key4.db",
            "evolution/addressbook/contacts.db",
            "nautilus/tags/meta.db",
        ] {
            database(&temp.path().join(name));
        }
        database(&temp.path().join("clip-daemon/history/history.db"));
        let mounts = Mounts::parse("1 0 8:1 / / rw - ext4 /dev/a rw\n");
        let mut req = request(temp.path());
        req.roots[0].explicit = false;
        req.known = vec![history.clone(), temp.path().join("pki/nssdb/key4.db")];
        let mut events = vec![];
        let summary = scan(req, &mounts, &AtomicBool::new(false), |event| {
            events.push(event);
            true
        });
        assert_eq!(summary.found, 1);
        assert_eq!(
            paths(&events),
            vec![temp.path().join("clip-daemon/history/history.db")]
        );
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(
                    event,
                    Event::Checked(_, Availability::ExcludedApplication)
                ))
                .count(),
            2
        );
        // A deliberate search of the profile itself still allows an override.
        let mut req = request(&browser);
        req.known.push(history.clone());
        assert_eq!(paths(&run(req).0), vec![history]);
    }
    #[test]
    fn budgets_cancellation_depth_and_exclusions_are_enforced() {
        let temp = tempfile::tempdir().unwrap();
        for i in 0..10 {
            database(&temp.path().join(format!("{i}.db")));
        }
        database(&temp.path().join("nested/deep.db"));
        let mut req = request(temp.path());
        req.budget.entries = 3;
        let (_, summary) = run(req);
        assert!(summary.limited && summary.entries == 3 && summary.probes <= 3);
        let mut req = request(temp.path());
        req.budget.probes = 2;
        let (_, summary) = run(req);
        assert!(summary.limited && summary.probes == 2);
        let mut req = request(temp.path());
        req.budget.directories = 1;
        assert!(run(req).1.limited);
        let mut req = request(temp.path());
        req.roots[0].depth = 0;
        assert_eq!(run(req).1.found, 10);
        let mut req = request(temp.path());
        req.excluded.push(temp.path().join("nested"));
        assert_eq!(run(req).1.found, 10);
        let summary = scan(
            request(temp.path()),
            &Mounts::default(),
            &AtomicBool::new(true),
            |_| panic!("cancelled"),
        );
        assert!(summary.cancelled && summary.entries == 0);
        let mut req = request(temp.path());
        req.budget.duration = Duration::ZERO;
        assert!(run(req).1.limited);
    }
    #[test]
    fn known_files_are_checked_even_without_recognized_extensions() {
        let temp = tempfile::tempdir().unwrap();
        let good = temp.path().join("custom.bin");
        database(&good);
        let bad = temp.path().join("bad.db");
        fs::write(&bad, "not a database").unwrap();
        let missing = temp.path().join("missing.db");
        let mut req = request(temp.path());
        req.known = vec![good.clone(), bad.clone(), missing.clone()];
        let (events, summary) = run(req);
        assert_eq!(paths(&events), vec![good]);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Checked(p, Availability::Missing) if p == &missing))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, Event::Checked(p, Availability::NotDatabase) if p == &bad))
        );
        assert_eq!(summary.found, 1);
    }
    #[cfg(unix)]
    #[test]
    fn ignores_symlinks_special_files_and_deduplicates_inodes() {
        use std::os::unix::{ffi::OsStringExt, fs::symlink};
        let temp = tempfile::tempdir().unwrap();
        database(&temp.path().join("original.db"));
        fs::hard_link(temp.path().join("original.db"), temp.path().join("hard.db")).unwrap();
        symlink(temp.path(), temp.path().join("loop")).unwrap();
        symlink(temp.path().join("original.db"), temp.path().join("link.db")).unwrap();
        let odd = temp
            .path()
            .join(std::ffi::OsString::from_vec(b"odd-\xff.db".to_vec()));
        database(&odd);
        let fifo =
            std::ffi::CString::new(temp.path().join("pipe.db").as_os_str().as_encoded_bytes())
                .unwrap();
        assert_eq!(unsafe { libc::mkfifo(fifo.as_ptr(), 0o600) }, 0);
        let (events, summary) = run(request(temp.path()));
        assert_eq!(summary.found, 2);
        assert!(paths(&events).contains(&odd));
        let mut req = request(&temp.path().join("loop"));
        req.known.push(temp.path().join("pipe.db"));
        assert_eq!(run(req).1.found, 0);
    }
    #[test]
    fn mounts_are_checked_before_io_and_explicit_roots_can_opt_in() {
        let temp = tempfile::tempdir().unwrap();
        database(&temp.path().join("app.db"));
        let mounts = Mounts::parse(&format!(
            "1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 0:1 / {} rw - autofs none rw\n",
            temp.path().display()
        ));
        assert!(!mounts.automatic_allowed(&temp.path().join("sub")));
        let mut req = request(temp.path());
        req.roots[0].explicit = false;
        let summary = scan(req, &mounts, &AtomicBool::new(false), |_| true);
        assert_eq!(summary.entries, 0);
        let summary = scan(
            request(temp.path()),
            &mounts,
            &AtomicBool::new(false),
            |_| true,
        );
        assert_eq!(summary.found, 1);
        assert_eq!(unescape_mount("/home/a\\040b"), Path::new("/home/a b"));
        assert!(!Mounts::default().automatic_allowed(temp.path()));
    }
    #[test]
    fn overlapping_roots_do_not_hide_deeper_results() {
        let temp = tempfile::tempdir().unwrap();
        database(&temp.path().join("nested/deep/app.db"));
        let mut req = request(temp.path());
        let mut shallow = req.roots[0].clone();
        shallow.depth = 0;
        req.roots.insert(0, shallow);
        assert_eq!(run(req).1.found, 1);
    }
    #[test]
    fn explicit_folders_do_not_authorize_nested_mounts_or_cached_files_inside_them() {
        let temp = tempfile::tempdir().unwrap();
        let nested = temp.path().join("mount");
        database(&nested.join("remote.db"));
        database(&temp.path().join("local.db"));
        for kind in ["nfs", "ext4", "autofs", "fuse.sshfs"] {
            let mounts = Mounts::parse(&format!(
                "1 0 8:1 / / rw - ext4 /dev/a rw\n2 1 0:1 / {} rw - {kind} source rw\n",
                nested.display()
            ));
            let mut req = request(temp.path());
            req.known.push(nested.join("remote.db"));
            let mut events = vec![];
            let summary = scan(req, &mounts, &AtomicBool::new(false), |event| {
                events.push(event);
                true
            });
            assert_eq!(summary.found, 1, "{kind}");
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, Event::Checked(_, Availability::Skipped)))
            );
        }
    }
    #[test]
    fn invalid_explicit_roots_and_symlink_ancestors_are_reported() {
        let temp = tempfile::tempdir().unwrap();
        let (_, summary) = run(request(&temp.path().join("missing")));
        assert_eq!(summary.unavailable, 1);
        let file = temp.path().join("file.db");
        database(&file);
        assert_eq!(run(request(&file)).1.unavailable, 1);
        #[cfg(unix)]
        {
            std::os::unix::fs::symlink(temp.path(), temp.path().join("alias")).unwrap();
            assert_eq!(run(request(&temp.path().join("alias/nested"))).1.skipped, 1);
        }
    }
    #[cfg(unix)]
    #[test]
    fn permission_failures_do_not_abort_other_roots() {
        use std::os::unix::fs::PermissionsExt;
        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let temp = tempfile::tempdir().unwrap();
        let denied = temp.path().join("denied");
        database(&denied.join("private.db"));
        database(&temp.path().join("public.db"));
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o0)).unwrap();
        let (_, summary) = run(request(temp.path()));
        fs::set_permissions(&denied, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(summary.found, 1);
        assert!(summary.unavailable > 0);
    }
    #[test]
    fn root_defaults_are_shallow_for_home_and_never_scan_the_filesystem_root() {
        let home = Path::new("/home/test");
        let roots = default_roots(Some(home), Some(home), None, None);
        assert!(
            roots
                .iter()
                .filter(|r| r.path == home)
                .all(|r| r.depth == 0)
        );
        assert!(
            roots
                .iter()
                .any(|r| r.path == home.join(".local/share") && r.extensionless)
        );
        assert!(
            roots
                .iter()
                .any(|r| r.path == home.join(".local/state") && r.extensionless)
        );
        assert_eq!(roots[0].path, home.join(".local/share"));
        assert_eq!(roots[1].path, home.join(".local/state"));
        assert!(default_roots(None, Some(Path::new("/")), None, None).is_empty());
    }
    #[test]
    fn application_roots_respect_xdg_overrides_without_home() {
        let roots = default_roots(
            None,
            None,
            Some(Path::new("/data")),
            Some(Path::new("/state")),
        );
        assert_eq!(
            roots.iter().map(|r| r.path.as_path()).collect::<Vec<_>>(),
            vec![Path::new("/data"), Path::new("/state")]
        );
        assert!(
            roots
                .iter()
                .all(|r| r.extensionless && r.depth == 3 && !r.explicit)
        );
        let home = Path::new("/home/test");
        let fallback = default_roots(
            Some(home),
            None,
            Some(Path::new("relative")),
            Some(Path::new("/")),
        );
        assert_eq!(fallback[0].path, home.join(".local/share"));
        assert_eq!(fallback[1].path, home.join(".local/state"));
    }
    #[test]
    fn real_turso_database_and_sidecars_are_unchanged_by_discovery() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("native.db");
        crate::test_support::create(
            &path,
            &[
                "CREATE TABLE example(id INTEGER PRIMARY KEY, value TEXT)",
                "INSERT INTO example VALUES (1, 'discovery fixture')",
            ],
        );
        let snapshot = || crate::test_support::snapshot(temp.path());
        let before = snapshot();
        let (events, summary) = run(request(temp.path()));
        assert_eq!(summary.found, 1);
        assert_eq!(paths(&events), vec![path]);
        assert!(
            events.iter().any(
                |event| matches!(event, Event::Found(c) if c.kind == DatabaseKind::LikelyTurso)
            )
        );
        assert_eq!(snapshot(), before);
    }
    #[test]
    fn worker_finishes_and_dropping_a_full_receiver_unblocks_it() {
        let temp = tempfile::tempdir().unwrap();
        database(&temp.path().join("app.db"));
        let job = Job::start(request(temp.path()), || {}).unwrap();
        assert!(matches!(
            job.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Found(_)
        ));
        assert!(matches!(
            job.events.recv_timeout(Duration::from_secs(5)).unwrap(),
            Event::Finished(_)
        ));
        // A receiver that disappears is treated as cancellation, not an error loop.
        let summary = scan(
            request(temp.path()),
            &Mounts::default(),
            &AtomicBool::new(false),
            |_| false,
        );
        assert!(summary.cancelled);

        // Fill the actual result channel before dropping the receiver. The wake
        // sender disappearing proves that the worker exited without a UI join.
        for i in 0..100 {
            database(&temp.path().join(format!("{i}.db")));
        }
        let (wake_tx, wake_rx) = mpsc::channel();
        let job = Job::start(request(temp.path()), move || {
            let _ = wake_tx.send(());
        })
        .unwrap();
        for _ in 0..32 {
            wake_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        }
        drop(job);
        loop {
            match wake_rx.recv_timeout(Duration::from_secs(5)) {
                Ok(()) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("discovery worker remained blocked after drop: {e}"),
            }
        }
    }
}
