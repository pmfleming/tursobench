use crate::{
    browse::describe,
    worker::{configure, failure, rollback, to_engine},
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
    if let Err(e) = result {
        return match rollback(&c).await {
            Ok(()) => CommitOutcome::Rejected(e),
            Err(cleanup) => CommitOutcome::Unknown(Failure {
                kind: ErrorKind::Unknown,
                message: format!(
                    "{e}; rollback also failed: {cleanup}. Do not retry without inspecting the database."
                ),
            }),
        };
    }
    match c.execute("COMMIT", ()).await {
        Ok(_) => CommitOutcome::Committed {
            operations: batch.edits.len(),
        },
        Err(e) => {
            let cleanup = rollback(&c).await;
            // Conservatively retain drafts even if a commit I/O failure leaves autocommit true.
            CommitOutcome::Unknown(Failure {
                kind: ErrorKind::Unknown,
                message: format!(
                    "Commit failed: {e}; cleanup: {cleanup:?}. Inspect before retrying."
                ),
            })
        }
    }
}
