//! Turso conversions and connection invariants shared by all worker operations.
use std::time::Duration;
use turso::Connection;
use tursobench_core::{ConnectionSettings, ErrorKind, Failure, TransactionState, Value};

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
        message: e.to_string(),
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
pub(crate) async fn configure(c: &Connection, settings: ConnectionSettings) -> Result<(), Failure> {
    c.query_timeout(Duration::from_millis(settings.query_timeout_ms))
        .map_err(failure)?;
    c.busy_timeout(Duration::from_millis(settings.busy_timeout_ms))
        .map_err(failure)?;
    c.execute("PRAGMA foreign_keys = ON", ())
        .await
        .map_err(failure)?;
    let mut rows = c.query("PRAGMA foreign_keys", ()).await.map_err(failure)?;
    let enabled = rows
        .next()
        .await
        .map_err(failure)?
        .ok_or_else(|| Failure::input("Foreign key status unavailable"))?
        .get_value(0)
        .map_err(failure)?;
    while rows.next().await.map_err(failure)?.is_some() {}
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
