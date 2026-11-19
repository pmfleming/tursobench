//! Turso conversions and connection invariants shared by all worker operations.
use crate::access::Admission;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use turso::{Builder, Connection};
use tursobench_core::{
    Access, Bindings, Column, ConnectionSettings, Coordination, ErrorKind, Failure,
    TransactionState, Value,
};

pub(crate) fn failure(e: turso::Error) -> Failure {
    let kind = match &e {
        turso::Error::Busy(_) | turso::Error::BusySnapshot(_) => ErrorKind::Busy,
        turso::Error::Readonly(_) => ErrorKind::ReadOnly,
        turso::Error::Constraint(_) => ErrorKind::Constraint,
        turso::Error::Interrupt(_) => ErrorKind::Interrupted,
        turso::Error::IoError(..) => ErrorKind::Io,
        _ => ErrorKind::Engine,
    };
    Failure {
        kind,
        message: match e {
            turso::Error::BusySnapshot(ref detail) => format!(
                "Read snapshot changed; roll back the transaction before retrying. {detail}"
            ),
            turso::Error::Busy(ref detail) => {
                format!("Database is busy; no automatic retry was made. {detail}")
            }
            _ => e.to_string(),
        },
    }
}
pub(crate) fn state(c: &Connection) -> TransactionState {
    match c.is_autocommit() {
        Ok(true) => TransactionState::Autocommit,
        Ok(false) => TransactionState::Active,
        Err(_) => TransactionState::Unknown,
    }
}
pub(crate) fn to_engine(v: Value) -> Result<turso::Value, Failure> {
    Ok(match v {
        Value::Null => turso::Value::Null,
        Value::Integer(n) => turso::Value::Integer(n),
        Value::Real(n) => turso::Value::Real(n),
        Value::Text(s) => turso::Value::Text(s),
        Value::Blob(b) => turso::Value::Blob(b),
        Value::Deferred(_) => {
            return Err(Failure::input(
                "Deferred values cannot be used as SQL bindings",
            ));
        }
    })
}
pub(crate) fn from_engine(v: turso::Value) -> Value {
    match v {
        turso::Value::Null => Value::Null,
        turso::Value::Integer(n) => Value::Integer(n),
        turso::Value::Real(n) => Value::Real(n),
        turso::Value::Text(s) => Value::Text(s),
        turso::Value::Blob(b) => Value::Blob(b),
    }
}
/// The first value a PRAGMA reports, draining any further rows.
pub(crate) async fn pragma(c: &Connection, sql: &str) -> Result<Option<turso::Value>, Failure> {
    let mut rows = c.query(sql, ()).await.map_err(failure)?;
    let value = rows
        .next()
        .await
        .map_err(failure)?
        .map(|row| row.get_value(0))
        .transpose()
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
    Ok(value)
}
pub(crate) async fn configure(c: &Connection, settings: ConnectionSettings) -> Result<(), Failure> {
    c.query_timeout(Duration::from_millis(settings.query_timeout_ms))
        .map_err(failure)?;
    c.busy_timeout(Duration::from_millis(settings.busy_timeout_ms))
        .map_err(failure)?;
    c.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(failure)?;
    let enabled = pragma(c, "PRAGMA foreign_keys")
        .await?
        .ok_or_else(|| Failure::input("Foreign key status unavailable"))?;
    if enabled != turso::Value::Integer(1) {
        return Err(Failure::input(
            "Foreign key enforcement could not be enabled",
        ));
    }
    Ok(())
}
pub(crate) async fn rollback(c: &Connection) -> Result<(), Failure> {
    match state(c) {
        TransactionState::Autocommit => Ok(()),
        _ => c.execute("ROLLBACK", ()).await.map(|_| ()).map_err(failure),
    }
}
pub(crate) fn params(bindings: Bindings) -> Result<turso::params::Params, Failure> {
    Ok(match bindings {
        Bindings::Positional(values) => turso::params::Params::Positional(
            values
                .into_iter()
                .map(to_engine)
                .collect::<Result<_, _>>()?,
        ),
        Bindings::Named(values) => turso::params::Params::Named(
            values
                .into_iter()
                .map(|(name, value)| to_engine(value).map(|value| (name.into(), value)))
                .collect::<Result<_, _>>()?,
        ),
    })
}
pub(crate) fn metadata_bytes(columns: &[Column]) -> usize {
    columns
        .iter()
        .map(|c| c.name.len() + c.declared_type.as_ref().map_or(0, String::len))
        .sum()
}
/// Result metadata, or `None` beyond the 512-column / 1 MiB display budget.
pub(crate) fn columns(statement: &turso::Statement) -> Option<Vec<Column>> {
    let columns: Vec<_> = statement
        .columns()
        .into_iter()
        .map(|c| Column {
            name: c.name().into(),
            declared_type: c.decl_type().map(str::to_owned),
        })
        .collect();
    (columns.len() <= 512 && metadata_bytes(&columns) <= 1024 * 1024).then_some(columns)
}

/// The engine and connections a session owns until close.
pub(crate) struct Engine {
    pub(crate) db: turso::Database,
    pub(crate) editor: Connection,
    pub(crate) browse: Connection,
    pub(crate) admission: Admission,
    pub(crate) coordination: Coordination,
}
fn local(path: &str, shared: bool) -> Builder {
    Builder::new_local(path)
        // Turso's native FTS indexes require both the Cargo `fts` feature and
        // index-method capability when opening existing files or creating indexes.
        .experimental_index_method(true)
        .experimental_multiprocess_wal(shared)
}
/// Forces schema validation by reading the catalog.
async fn validate_schema(c: &Connection) -> Result<(), Failure> {
    let mut rows = c
        .query("SELECT name FROM sqlite_schema LIMIT 1", ())
        .await
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
    Ok(())
}
fn database_file(path: &Path) -> Result<PathBuf, Failure> {
    let path = std::fs::canonicalize(path).map_err(|e| Failure {
        kind: ErrorKind::Io,
        message: e.to_string(),
    })?;
    if !path.is_file()
        || std::fs::metadata(&path)
            .map_err(|e| Failure::input(e.to_string()))?
            .len()
            == 0
    {
        return Err(Failure::input(
            "Select an existing non-empty regular database file",
        ));
    }
    Ok(path)
}
/// Opens the session engine; a failure also names applications holding the file.
pub(crate) async fn open(
    path: &Path,
    access: Access,
    settings: ConnectionSettings,
) -> Result<Engine, Failure> {
    engine(path, access, settings).await.map_err(|mut failure| {
        let usage = crate::access::processes_using(path);
        if !usage.applications.is_empty() {
            let names = usage.applications.join(", ");
            failure.message.push_str(&format!(" Also open in {names}."));
        }
        failure
    })
}
async fn engine(
    path: &Path,
    access: Access,
    settings: ConnectionSettings,
) -> Result<Engine, Failure> {
    let path = database_file(path)?;
    let text = path
        .to_str()
        .ok_or_else(|| Failure::input("Non-UTF8 paths are not supported by Turso's builder"))?;
    let mut admission = Admission::acquire(&path)?;
    if access == Access::ReadWrite {
        // Validate compatibility before permitting the engine's writable-open side effects.
        let preflight = local(text, admission.reservation.is_none())
            .read_only(true)
            .build()
            .await
            .map_err(failure)?;
        validate_schema(&preflight.connect().map_err(failure)?).await?;
        drop(preflight);
        // A writable open initializes/joins native coordination. The held
        // main-file lock excludes legacy writers across this transition.
        admission.reservation = None;
    }
    let shared = admission.reservation.is_none();
    let db = local(text, shared)
        .read_only(access == Access::ReadOnly)
        .build()
        .await
        .map_err(failure)?;
    let coordination = if !shared {
        Coordination::ProtectedReadOnly
    } else if db.is_multiprocess_wal_coordinated().map_err(failure)? {
        Coordination::SharedWal
    } else {
        return Err(Failure::input(
            "Shared WAL coordination is unavailable; no uncoordinated connection was admitted",
        ));
    };
    // The engine now owns its native lifetime lock. Release the temporary
    // admission lock so maintenance can later reserve a fully closed mapping.
    admission.joined();
    let editor = db.connect().map_err(failure)?;
    let browse = db.connect().map_err(failure)?;
    for c in [&editor, &browse] {
        configure(c, settings).await?;
    }
    validate_schema(&browse).await?;
    Ok(Engine {
        db,
        editor,
        browse,
        admission,
        coordination,
    })
}
