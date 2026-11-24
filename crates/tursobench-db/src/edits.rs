use crate::{
    browse::describe,
    connection::{configure, failure, rollback, to_engine},
};
use turso::params_from_iter;
use tursobench_core::{Access, CommitOutcome, ConnectionSettings, DraftBatch, ErrorKind, Failure};

pub(crate) async fn commit(
    db: &turso::Database,
    access: Access,
    settings: ConnectionSettings,
    batch: DraftBatch,
) -> CommitOutcome {
    if access != Access::ReadWrite {
        return CommitOutcome::Rejected(Failure {
            kind: ErrorKind::ReadOnly,
            message: "Read/write access is required for edit commits".into(),
        });
    }
    let statements = match batch.preview() {
        Ok(s) => s,
        Err(e) => return CommitOutcome::Rejected(e),
    };
    let c = match db.connect() {
        Ok(c) => c,
        Err(e) => return CommitOutcome::Rejected(failure(e)),
    };
    if let Err(e) = configure(&c, settings).await {
        return CommitOutcome::Rejected(e);
    }
    if let Err(e) = c.execute("BEGIN IMMEDIATE", ()).await {
        return CommitOutcome::Rejected(failure(e));
    }
    let result = async {
        let current = describe(&c, &batch.table.name).await?;
        if current != batch.table { return Err(Failure { kind: ErrorKind::Conflict, message: "Table schema changed; reload before editing".into() }); }
        for (index, statement) in statements.into_iter().enumerate() {
            let count = c.execute(&statement.sql, params_from_iter(statement.parameters.into_iter().map(to_engine).collect::<Result<Vec<_>,_>>()?)).await.map_err(failure)?;
            if count != 1 { return Err(Failure { kind: ErrorKind::Conflict, message: format!("Operation {} affected {count} rows instead of 1; original row changed or identity is ambiguous", index + 1) }); }
        }
        Ok::<(), Failure>(())
    }.await;
    finish(
        result,
        batch.edits.len(),
        async { c.execute("COMMIT", ()).await.map(|_| ()).map_err(failure) },
        rollback(&c),
    )
    .await
}

/// Resolve only the transaction boundary. Futures make failures deterministic in
/// tests without adding fault flags or replacing the database engine.
async fn finish(
    result: Result<(), Failure>,
    operations: usize,
    commit: impl Future<Output = Result<(), Failure>>,
    rollback: impl Future<Output = Result<(), Failure>>,
) -> CommitOutcome {
    if let Err(e) = result {
        return match rollback.await {
            Ok(()) => CommitOutcome::Rejected(e),
            Err(cleanup) => CommitOutcome::Unknown(Failure {
                kind: ErrorKind::Unknown,
                message: format!(
                    "{e}; rollback also failed: {cleanup}. Do not retry without inspecting the database."
                ),
            }),
        };
    }
    match commit.await {
        Ok(()) => CommitOutcome::Committed { operations },
        Err(e) => {
            let cleanup = rollback.await;
            // A failed acknowledgement may follow a durable commit.
            CommitOutcome::Unknown(Failure {
                kind: ErrorKind::Unknown,
                message: format!(
                    "Commit failed: {e}; cleanup: {cleanup:?}. Inspect before retrying."
                ),
            })
        }
    }
}

/// Runs `commit` only while every earlier outcome is confirmed; an unknown
/// outcome sets `uncertain`, refusing later commits until reopen.
pub(crate) async fn guarded(
    uncertain: &mut bool,
    commit: impl Future<Output = CommitOutcome>,
) -> CommitOutcome {
    if *uncertain {
        return CommitOutcome::Unknown(Failure {
            kind: ErrorKind::Unknown,
            message: "Prior outcome UNKNOWN; reopen before committing".into(),
        });
    }
    let outcome = commit.await;
    *uncertain |= matches!(outcome, CommitOutcome::Unknown(_));
    outcome
}

#[cfg(test)]
mod tests {
    use super::{finish, guarded};
    use crate::connection::{failure, rollback};
    use std::cell::Cell;
    use tursobench_core::{CommitOutcome, ErrorKind, Failure};

    /// A database whose only connection holds one uncommitted insert.
    async fn pending_insert(path: &std::path::Path) -> (turso::Database, turso::Connection) {
        let db = turso::Builder::new_local(path.to_str().unwrap())
            .build()
            .await
            .unwrap();
        let c = db.connect().unwrap();
        for sql in [
            "CREATE TABLE items(id INTEGER PRIMARY KEY)",
            "BEGIN IMMEDIATE",
            "INSERT INTO items VALUES(1)",
        ] {
            c.execute(sql, ()).await.unwrap();
        }
        (db, c)
    }
    fn io() -> Failure {
        Failure {
            kind: ErrorKind::Io,
            message: "injected I/O failure".into(),
        }
    }
    /// COMMIT and ROLLBACK on `c` that record their calls in `called` and fail
    /// before COMMIT (point 1), after a durable COMMIT (point 2) or on rollback.
    fn boundary<'a>(
        c: &'a turso::Connection,
        commit_point: u8,
        rollback_failed: bool,
        called: &'a (Cell<bool>, Cell<bool>),
    ) -> (
        impl Future<Output = Result<(), Failure>> + 'a,
        impl Future<Output = Result<(), Failure>> + 'a,
    ) {
        let commit = async move {
            called.0.set(true);
            if commit_point == 1 {
                return Err(io());
            }
            c.execute("COMMIT", ()).await.map_err(failure)?;
            if commit_point == 2 { Err(io()) } else { Ok(()) }
        };
        let cleanup = async move {
            called.1.set(true);
            if rollback_failed {
                Err(io())
            } else {
                rollback(c).await
            }
        };
        (commit, cleanup)
    }
    /// The durable row count, read through a second connection.
    async fn durable_rows(db: &turso::Database) -> turso::Value {
        let mut rows = db
            .connect()
            .unwrap()
            .query("SELECT count(*) FROM items", ())
            .await
            .unwrap();
        rows.next().await.unwrap().unwrap().get_value(0).unwrap()
    }

    #[tokio::test]
    async fn boundary_failures_preserve_uncertainty_and_block_retries() {
        // commit point: 0 = success, 1 = before commit, 2 = after durable commit.
        for (statement_failed, commit_point, rollback_failed) in [
            (false, 0, false),
            (true, 0, false),
            (true, 0, true),
            (false, 1, false),
            (false, 1, true),
            (false, 2, false),
            (false, 2, true),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let (db, c) = pending_insert(&dir.path().join("boundary.db")).await;
            let called = (Cell::new(false), Cell::new(false));
            let (commit, cleanup) = boundary(&c, commit_point, rollback_failed, &called);
            let statement = match statement_failed {
                true => Err(Failure::input("statement failed")),
                false => Ok(()),
            };
            let mut unconfirmed = false;
            let outcome = guarded(&mut unconfirmed, finish(statement, 1, commit, cleanup)).await;
            let uncertain = rollback_failed || commit_point != 0;
            assert_eq!(unconfirmed, uncertain);
            assert_eq!(called.0.get(), !statement_failed);
            assert_eq!(called.1.get(), statement_failed || commit_point != 0);
            match outcome {
                CommitOutcome::Committed { operations: 1 } => {
                    assert!(!statement_failed && !uncertain)
                }
                CommitOutcome::Rejected(e) => {
                    assert_eq!(e.message, "statement failed");
                    assert!(statement_failed && !uncertain);
                }
                CommitOutcome::Unknown(e) => {
                    assert_eq!(e.kind, ErrorKind::Unknown);
                    assert!(uncertain);
                    assert!(e.message.contains("injected I/O failure"));
                }
                other => panic!("{other:?}"),
            }
            let retry_called = Cell::new(false);
            let retry = guarded(&mut unconfirmed, async {
                retry_called.set(true);
                CommitOutcome::Committed { operations: 0 }
            })
            .await;
            assert_eq!(retry_called.get(), !uncertain);
            assert_eq!(matches!(retry, CommitOutcome::Unknown(_)), uncertain);
            rollback(&c).await.unwrap();
            // Durable state includes an ambiguous COMMIT whose acknowledgement failed.
            let committed = !statement_failed && commit_point != 1;
            assert_eq!(
                durable_rows(&db).await,
                turso::Value::Integer(committed.into())
            );
        }
    }
}
