//! Linux admission locks complement the SDK's shared WAL protocol.
//!
//! Main-file OFD read locks conflict with legacy whole-file writer locks and
//! survive the engine closing other descriptors. An exclusive `.tshm` lock
//! reserves the legacy read-only path when no native coordinator is active.
//! This also stops a future SDK writer from initializing coordination behind
//! a legacy reader. These are advisory locks, not protection against raw writes.
use std::{fs::File, path::Path};
use tursobench_core::{ErrorKind, Failure};

pub(crate) struct Admission {
    _main: HeldLock,
    _lifetime: Option<HeldLock>,
    pub(crate) reservation: Option<HeldLock>,
}

pub(crate) struct HeldLock(File);

impl Drop for HeldLock {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        {
            // Unlock before close: a fork/dup may still own the same description.
            let _ = flock(&self.0, libc::F_OFD_SETLK, libc::F_UNLCK, 0);
        }
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;

    #[test]
    fn admission_releases_locks_even_with_inherited_descriptors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("admission.db");
        std::fs::write(&path, b"fixture").unwrap();
        let admission = Admission::acquire(&path).unwrap();
        // dup and fork share the same open file description. A concurrent
        // process spawn can retain these descriptors until its exec completes.
        let inherited_main = admission._main.0.try_clone().unwrap();
        let inherited_reservation = admission
            .reservation
            .as_ref()
            .unwrap()
            .0
            .try_clone()
            .unwrap();
        drop(admission);
        let main = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&path)
            .unwrap();
        assert!(
            lock(main, true).unwrap().is_some(),
            "main admission lock survived close"
        );
        assert!(
            reserve(&path).unwrap().is_some(),
            "reservation survived close"
        );
        drop((inherited_main, inherited_reservation));
    }
}

fn io_failure(e: std::io::Error) -> Failure {
    Failure {
        kind: ErrorKind::Io,
        message: e.to_string(),
    }
}

/// Advisory, non-mutating preflight. Queries locks without acquiring them or
/// creating sidecars; Session::open still performs authoritative admission.
pub fn check_access(path: &Path) -> Result<(), Failure> {
    use std::io::Read;
    let path = std::fs::canonicalize(path).map_err(|e| Failure {
        kind: ErrorKind::Io,
        message: match e.kind() {
            std::io::ErrorKind::NotFound => "File not found.".into(),
            std::io::ErrorKind::PermissionDenied => {
                "Permission denied. Check file and folder access.".into()
            }
            _ => e.to_string(),
        },
    })?;
    let metadata = std::fs::metadata(&path).map_err(io_failure)?;
    if !metadata.is_file() {
        return Err(Failure::input("Choose a database file, not a folder."));
    }
    if path.to_str().is_none() {
        return Err(Failure::input("The database path must use UTF-8."));
    }
    #[cfg(target_os = "linux")]
    if owns_posix_lock(&metadata) {
        return Err(Failure {
            kind: ErrorKind::Busy,
            message: "Already in use by this application.".into(),
        });
    }
    let mut file = File::open(&path).map_err(io_failure)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if file.metadata().map_err(io_failure)?.nlink() != 1 {
            return Err(Failure::input(
                "Hard-linked database files are unsupported.",
            ));
        }
    }
    let mut header = [0; 16];
    if metadata.len() < 100
        || file.read_exact(&mut header).is_err()
        || &header != b"SQLite format 3\0"
    {
        return Err(Failure::input(
            "No database header found. Choose a different file.",
        ));
    }
    #[cfg(target_os = "linux")]
    {
        if let Some(pid) = conflicting_reader(&file, 0)? {
            let owner = if pid > 0 {
                process_name(pid as u32)
            } else {
                "Another connection".into()
            };
            return Err(Failure {
                kind: ErrorKind::Busy,
                message: format!(
                    "{owner} holds a write lock. Close that connection to open this file."
                ),
            });
        }
        let mut sidecar = path.as_os_str().to_owned();
        sidecar.push("-tshm");
        match File::open(std::path::PathBuf::from(sidecar)) {
            Ok(file) if conflicting_reader(&file, 1)?.is_some() => {
                return Err(Failure {
                    kind: ErrorKind::Busy,
                    message:
                        "Another read-only session has reserved this file. Close it before opening."
                            .into(),
                });
            }
            Ok(_) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_failure(e)),
        }
        Ok(())
    }
    #[cfg(not(target_os = "linux"))]
    Err(Failure::input(
        "Database access is currently supported on Linux only.",
    ))
}

#[cfg(target_os = "linux")]
fn owns_posix_lock(target: &std::fs::Metadata) -> bool {
    use std::os::unix::fs::MetadataExt;
    // Closing any newly opened descriptor would release this process's POSIX
    // locks on the inode. Inspect existing descriptors before opening the file.
    let Ok(fds) = std::fs::read_dir("/proc/self/fd") else {
        return false;
    };
    fds.flatten().any(|fd| {
        std::fs::metadata(fd.path())
            .is_ok_and(|m| m.dev() == target.dev() && m.ino() == target.ino())
            && std::fs::read_to_string(
                std::path::Path::new("/proc/self/fdinfo").join(fd.file_name()),
            )
            .is_ok_and(|s| {
                s.lines()
                    .any(|line| line.starts_with("lock:") && line.contains("POSIX"))
            })
    })
}

#[cfg(target_os = "linux")]
fn flock(
    file: &File,
    command: i32,
    mode: i32,
    length: libc::off_t,
) -> std::io::Result<libc::flock> {
    use std::os::fd::AsRawFd;
    let mut region = libc::flock {
        l_type: mode as _,
        l_whence: libc::SEEK_SET as _,
        l_start: 0,
        l_len: length,
        l_pid: 0,
    };
    loop {
        // SAFETY: file owns a valid descriptor, region is initialized, and
        // F_OFD_GETLK/F_OFD_SETLK do not retain its pointer after this call.
        if unsafe { libc::fcntl(file.as_raw_fd(), command, &mut region) } != -1 {
            return Ok(region);
        }
        let error = std::io::Error::last_os_error();
        if error.kind() != std::io::ErrorKind::Interrupted {
            return Err(error);
        }
    }
}
#[cfg(target_os = "linux")]
fn conflicting_reader(file: &File, length: libc::off_t) -> Result<Option<i32>, Failure> {
    let region = flock(file, libc::F_OFD_GETLK, libc::F_RDLCK, length).map_err(io_failure)?;
    Ok((region.l_type != libc::F_UNLCK as libc::c_short).then_some(region.l_pid))
}

#[cfg(target_os = "linux")]
fn process_name(pid: u32) -> String {
    let name = std::fs::read_link(format!("/proc/{pid}/exe"))
        .ok()
        .and_then(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
        .or_else(|| std::fs::read_to_string(format!("/proc/{pid}/comm")).ok())
        .unwrap_or_else(|| "Application".into());
    let name = name.trim().trim_start_matches('.');
    let name = name.strip_suffix("-wrapped").unwrap_or(name);
    let name: String = name.chars().filter(|c| !c.is_control()).take(64).collect();
    format!("{name} ({pid})")
}

fn lock(file: File, exclusive: bool) -> Result<Option<HeldLock>, Failure> {
    lock_region(file, exclusive, 0)
}

#[cfg(target_os = "linux")]
fn lock_region(
    file: File,
    exclusive: bool,
    length: libc::off_t,
) -> Result<Option<HeldLock>, Failure> {
    let mode = if exclusive {
        libc::F_WRLCK
    } else {
        libc::F_RDLCK
    };
    match flock(&file, libc::F_OFD_SETLK, mode, length) {
        Ok(_) => Ok(Some(HeldLock(file))),
        Err(e) if matches!(e.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) => Ok(None),
        Err(e) => Err(io_failure(e)),
    }
}

#[cfg(not(target_os = "linux"))]
fn lock_region(_: File, _: bool, _: i64) -> Result<Option<HeldLock>, Failure> {
    Err(Failure::input(
        "Coordinated database access is currently supported on Linux only",
    ))
}

pub(crate) fn reserve(path: &Path) -> Result<Option<HeldLock>, Failure> {
    let mut name = path.as_os_str().to_owned();
    name.push("-tshm");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(std::path::PathBuf::from(name))
        .map_err(io_failure)?;
    lock(file, true)
}

impl Admission {
    pub(crate) fn joined(&mut self) {
        self._lifetime = None;
    }
    pub(crate) fn acquire(path: &Path) -> Result<Self, Failure> {
        if std::env::var_os("TURSO_DISABLE_FILE_LOCK").is_some()
            || std::env::var_os("LIMBO_DISABLE_FILE_LOCK").is_some()
        {
            return Err(Failure::input("Database file locking must be enabled"));
        }
        let main = File::open(path).map_err(io_failure)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if main.metadata().map_err(io_failure)?.nlink() != 1 {
                return Err(Failure::input(
                    "Hard-linked databases cannot be shared safely because aliases use different WAL/coordination paths. Open a database with one file name.",
                ));
            }
        }
        let Some(main) = lock(main, false)? else {
            return Err(Failure { kind: ErrorKind::Busy, message:
                "Database is locked by another application. Close its legacy connection or reopen it with compatible multiprocess WAL support.".into() });
        };
        let reservation = reserve(path)?;
        let lifetime = if reservation.is_none() {
            let mut name = path.as_os_str().to_owned();
            name.push("-tshm");
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(std::path::PathBuf::from(name))
                .map_err(io_failure)?;
            let Some(file) = lock_region(file, false, 1)? else {
                return Err(Failure { kind: ErrorKind::Busy, message:
                    "Another read-only session is protecting this database. Close it before starting shared access.".into() });
            };
            Some(file)
        } else {
            None
        };
        Ok(Self {
            _main: main,
            _lifetime: lifetime,
            reservation,
        })
    }
}

#[derive(Debug, Clone, Default)]
pub struct ProcessUsage {
    pub applications: Vec<String>,
    pub incomplete: bool,
}

/// Bounded, best-effort diagnostics. No matches must never imply exclusive use.
pub fn processes_using(path: &Path) -> ProcessUsage {
    #[cfg(target_os = "linux")]
    {
        use std::{
            collections::HashSet,
            os::unix::fs::MetadataExt,
            time::{Duration, Instant},
        };
        let mut report = ProcessUsage::default();
        let targets: HashSet<_> = ["", "-wal", "-tshm", "-log"]
            .into_iter()
            .filter_map(|suffix| {
                let mut p = path.as_os_str().to_owned();
                p.push(suffix);
                std::fs::metadata(std::path::PathBuf::from(p))
                    .ok()
                    .map(|m| (m.dev(), m.ino()))
            })
            .collect();
        if targets.is_empty() {
            report.incomplete = true;
            return report;
        }
        let Ok(processes) = std::fs::read_dir("/proc") else {
            report.incomplete = true;
            return report;
        };
        let started = Instant::now();
        for process in processes.take(8192).flatten() {
            if started.elapsed() > Duration::from_millis(200) || report.applications.len() >= 16 {
                report.incomplete = true;
                break;
            }
            let Some(pid) = process
                .file_name()
                .to_str()
                .and_then(|s| s.parse::<u32>().ok())
            else {
                continue;
            };
            if pid == std::process::id() {
                continue;
            }
            let Ok(fds) = std::fs::read_dir(process.path().join("fd")) else {
                report.incomplete = true;
                continue;
            };
            for (i, fd) in fds.flatten().enumerate() {
                if i >= 4096 || started.elapsed() > Duration::from_millis(200) {
                    report.incomplete = true;
                    break;
                }
                if std::fs::metadata(fd.path())
                    .ok()
                    .is_some_and(|m| targets.contains(&(m.dev(), m.ino())))
                {
                    report.applications.push(process_name(pid));
                    break;
                }
            }
        }
        report.applications.sort();
        report
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = path;
        ProcessUsage {
            applications: vec![],
            incomplete: true,
        }
    }
}
