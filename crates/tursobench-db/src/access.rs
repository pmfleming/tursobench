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
    _main: File,
    _lifetime: Option<File>,
    pub(crate) reservation: Option<File>,
}

fn io_failure(e: std::io::Error) -> Failure {
    Failure {
        kind: ErrorKind::Io,
        message: e.to_string(),
    }
}

#[cfg(target_os = "linux")]
pub(crate) fn lock(file: &File, exclusive: bool) -> Result<bool, Failure> {
    lock_region(file, exclusive, 0)
}

#[cfg(target_os = "linux")]
fn lock_region(file: &File, exclusive: bool, length: libc::off_t) -> Result<bool, Failure> {
    use std::os::fd::AsRawFd;
    let mut region: libc::flock = unsafe { std::mem::zeroed() };
    region.l_type = if exclusive {
        libc::F_WRLCK
    } else {
        libc::F_RDLCK
    } as _;
    region.l_whence = libc::SEEK_SET as _;
    region.l_len = length;
    // l_start=0, l_len=0: the entire file, including future growth.
    // SAFETY: valid descriptor and initialized flock; F_OFD_SETLK does not retain
    // the pointer. Closing this owned descriptor releases only this OFD lock.
    if unsafe { libc::fcntl(file.as_raw_fd(), libc::F_OFD_SETLK, &mut region) } == 0 {
        return Ok(true);
    }
    let error = std::io::Error::last_os_error();
    if matches!(error.raw_os_error(), Some(libc::EAGAIN | libc::EACCES)) {
        Ok(false)
    } else {
        Err(io_failure(error))
    }
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn lock(_: &File, _: bool) -> Result<bool, Failure> {
    Err(Failure::input(
        "Coordinated database access is currently supported on Linux only",
    ))
}

pub(crate) fn reserve(path: &Path) -> Result<Option<File>, Failure> {
    let mut name = path.as_os_str().to_owned();
    name.push("-tshm");
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(std::path::PathBuf::from(name))
        .map_err(io_failure)?;
    Ok(lock(&file, true)?.then_some(file))
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
        if !lock(&main, false)? {
            return Err(Failure { kind: ErrorKind::Busy, message:
                "Database is locked by another application. Close its legacy connection or reopen it with compatible multiprocess WAL support.".into() });
        }
        let reservation = reserve(path)?;
        let lifetime = if reservation.is_none() {
            let mut name = path.as_os_str().to_owned();
            name.push("-tshm");
            let file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(std::path::PathBuf::from(name))
                .map_err(io_failure)?;
            #[cfg(target_os = "linux")]
            if !lock_region(&file, false, 1)? {
                return Err(Failure { kind: ErrorKind::Busy, message:
                    "Another read-only session is protecting this database. Close it before starting shared access.".into() });
            }
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
                    let name = std::fs::read_to_string(process.path().join("comm"))
                        .unwrap_or_else(|_| "Application".into());
                    let name: String = name
                        .trim()
                        .chars()
                        .filter(|c| !c.is_control())
                        .take(64)
                        .collect();
                    report.applications.push(format!("{name} ({pid})"));
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
