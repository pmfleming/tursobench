//! Conservative external-change notifications; no uncommitted content is read.
use std::{
    path::{Path, PathBuf},
    time::SystemTime,
};
use tursobench_core::{ErrorKind, Failure};

#[derive(PartialEq, Eq)]
struct Stamp {
    length: u64,
    modified: SystemTime,
    identity: (u64, u64),
    changed: (i64, i64),
}
fn stamp(path: &Path) -> Result<Option<Stamp>, Failure> {
    let meta = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => {
            return Err(Failure {
                kind: ErrorKind::Io,
                message: e.to_string(),
            });
        }
    };
    #[cfg(unix)]
    let (identity, changed) = {
        use std::os::unix::fs::MetadataExt;
        ((meta.dev(), meta.ino()), (meta.ctime(), meta.ctime_nsec()))
    };
    #[cfg(not(unix))]
    let (identity, changed) = ((0, 0), (0, 0));
    Ok(Some(Stamp {
        length: meta.len(),
        modified: meta.modified().map_err(|e| Failure::input(e.to_string()))?,
        identity,
        changed,
    }))
}
async fn schema_version(c: &turso::Connection) -> Result<i64, Failure> {
    let mut rows = c
        .query("PRAGMA schema_version", ())
        .await
        .map_err(crate::connection::failure)?;
    let row = rows
        .next()
        .await
        .map_err(crate::connection::failure)?
        .ok_or_else(|| Failure::input("Schema version is unavailable"))?;
    match row.get_value(0).map_err(crate::connection::failure)? {
        turso::Value::Integer(version) => Ok(version),
        _ => Err(Failure::input("Invalid schema version")),
    }
}

pub(crate) struct Changes {
    path: PathBuf,
    wal: PathBuf,
    prior: (Option<Stamp>, Option<Stamp>),
    schema: i64,
}
impl Changes {
    pub(crate) async fn new(path: &Path, c: &turso::Connection) -> Result<Self, Failure> {
        let mut wal = path.as_os_str().to_owned();
        wal.push("-wal");
        let wal = PathBuf::from(wal);
        let schema = schema_version(c).await?;
        Ok(Self {
            path: path.into(),
            prior: (stamp(path)?, stamp(&wal)?),
            wal,
            schema,
        })
    }
    pub(crate) async fn poll(&mut self, c: &turso::Connection) -> Result<Option<bool>, Failure> {
        let next = (stamp(&self.path)?, stamp(&self.wal)?);
        if next.0.as_ref().map(|s| s.identity) != self.prior.0.as_ref().map(|s| s.identity) {
            return Err(Failure {
                kind: ErrorKind::Conflict,
                message: "Database file was removed or replaced. Reopen it before continuing."
                    .into(),
            });
        }
        if next == self.prior {
            return Ok(None);
        }
        // Browse stays in autocommit, preserving the editor's manual snapshot.
        let schema = schema_version(c).await?;
        let changed = schema != self.schema;
        self.schema = schema;
        self.prior = next;
        Ok(Some(changed))
    }
}
