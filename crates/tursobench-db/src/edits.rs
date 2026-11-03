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
            let count = c.execute(&statement.sql, params_from_iter(statement.parameters.into_iter().map(to_engine))).await.map_err(failure)?;
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

#[derive(Default)]
pub(super) struct CommitState {
    uncertain: bool,
}
impl CommitState {
    pub async fn run(&mut self, operation: impl Future<Output = CommitOutcome>) -> CommitOutcome {
        if self.uncertain {
            return CommitOutcome::Unknown(Failure { kind: ErrorKind::Unknown, message: "A previous edit commit has an uncertain outcome; close and inspect before retrying".into() });
        }
        let outcome = operation.await;
        self.uncertain = matches!(outcome, CommitOutcome::Unknown(_));
        outcome
    }
}

#[cfg(test)]
mod tests {
    use super::{CommitState, finish};
    use crate::connection::{failure, rollback};
    use std::cell::Cell;
    use tursobench_core::{CommitOutcome, ErrorKind, Failure};

    #[tokio::test]
    async fn boundary_failures_preserve_uncertainty_and_block_retries() {
        let io = || Failure {
            kind: ErrorKind::Io,
            message: "injected I/O failure".into(),
        };
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
            let db = turso::Builder::new_local(dir.path().join("boundary.db").to_str().unwrap())
                .build()
                .await
                .unwrap();
            let c = db.connect().unwrap();
            c.execute("CREATE TABLE items(id INTEGER PRIMARY KEY)", ())
                .await
                .unwrap();
            c.execute("BEGIN IMMEDIATE", ()).await.unwrap();
            c.execute("INSERT INTO items VALUES(1)", ()).await.unwrap();
            let commit_called = Cell::new(false);
            let rollback_called = Cell::new(false);
            let mut state = CommitState::default();
            let outcome = state
                .run(finish(
                    if statement_failed {
                        Err(Failure::input("statement failed"))
                    } else {
                        Ok(())
                    },
                    1,
                    async {
                        commit_called.set(true);
                        if commit_point == 1 {
                            return Err(io());
                        }
                        c.execute("COMMIT", ()).await.map_err(failure)?;
                        if commit_point == 2 { Err(io()) } else { Ok(()) }
                    },
                    async {
                        rollback_called.set(true);
                        if rollback_failed {
                            Err(io())
                        } else {
                            rollback(&c).await
                        }
                    },
                ))
                .await;
            let uncertain = rollback_failed || commit_point != 0;
            assert_eq!(commit_called.get(), !statement_failed);
            assert_eq!(rollback_called.get(), statement_failed || commit_point != 0);
            match outcome {
                CommitOutcome::Committed { operations } => {
                    assert_eq!(operations, 1);
                    assert!(!statement_failed && !uncertain);
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
            }
            let retry_called = Cell::new(false);
            let retry = state
                .run(async {
                    retry_called.set(true);
                    CommitOutcome::Committed { operations: 0 }
                })
                .await;
            assert_eq!(retry_called.get(), !uncertain);
            assert_eq!(matches!(retry, CommitOutcome::Unknown(_)), uncertain);
            rollback(&c).await.unwrap();
            // Read through a second connection to check durable state, including
            // the ambiguous successful COMMIT with a failed acknowledgement.
            let reader = db.connect().unwrap();
            let mut rows = reader
                .query("SELECT count(*) FROM items", ())
                .await
                .unwrap();
            let count = rows.next().await.unwrap().unwrap().get_value(0).unwrap();
            assert_eq!(
                count,
                turso::Value::Integer(i64::from(!statement_failed && commit_point != 1))
            );
            assert!(rows.next().await.unwrap().is_none());
        }
    }
}
