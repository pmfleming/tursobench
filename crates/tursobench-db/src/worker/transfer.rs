//! Imports, exports and maintenance: each refuses to run after an unconfirmed
//! outcome and reports the editor's transaction state afterwards.
use super::Worker;
use crate::{connection::state, sql, transfers};
use std::path::{Path, PathBuf};
use tursobench_core::{
    Access, Bindings, Browse, Completion, ErrorKind, EventKind, Failure, ImportSpec, Ticket,
    TransactionState, TransferOutcome,
};

impl Worker<'_> {
    pub(super) async fn export(
        &mut self,
        ticket: Ticket,
        path: PathBuf,
        text: String,
        parameters: Bindings,
        editor: bool,
    ) -> Result<EventKind, Failure> {
        sql::validate_coordinated(&text)?;
        let effect = sql::effect(&text);
        if effect.writes() {
            self.settled("reopen before executing writes")?;
        }
        if self.access == Access::ReadOnly {
            sql::validate_readonly(&text)?;
        }
        let (connection, scope) = if editor {
            (self.editor, "complete-query-result")
        } else {
            (self.browse, "complete-table")
        };
        let read = sql::stoppable_read(&text).then(|| {
            self.output
                .discard
                .interruptible(ticket.request, connection)
        });
        let result = transfers::export(connection, &path, &text, parameters, scope, || {
            self.output.discarded(ticket)
        })
        .await;
        drop(read);
        let mut outcome = result?;
        if editor {
            let completion = Completion {
                rows_seen: outcome.rows,
                changed: outcome.changed,
                error: outcome.error.clone().filter(|_| !outcome.executed),
                ..Completion::new(outcome.transaction)
            };
            self.note_completion(ticket, effect, &completion);
        }
        outcome.transaction = state(self.editor);
        Ok(EventKind::TransferFinished(outcome))
    }
    pub(super) async fn export_table(
        &mut self,
        ticket: Ticket,
        path: PathBuf,
        mut spec: Browse,
    ) -> Result<EventKind, Failure> {
        spec.offset = 0;
        spec.options.cursor = None;
        spec.options.reverse = false;
        let (_, mut text, parameters) = crate::browse::page(self.browse, spec).await?;
        let end = text
            .rfind(" LIMIT ")
            .ok_or_else(|| Failure::input("Invalid controlled export query"))?;
        text.truncate(end);
        self.export(ticket, path, text, parameters.into(), false)
            .await
    }
    pub(super) async fn import(
        &mut self,
        ticket: Ticket,
        spec: ImportSpec,
    ) -> Result<EventKind, Failure> {
        self.settled("reopen before importing")?;
        if self.access != Access::ReadWrite || state(self.editor) != TransactionState::Autocommit {
            return Err(Failure::input(
                "Import requires write access and editor autocommit",
            ));
        }
        let outcome = crate::imports::import(self.db, spec, self.settings, || {
            self.output.discarded(ticket)
        })
        .await?;
        let unknown = outcome.transaction == TransactionState::Unknown;
        self.uncertain |= unknown;
        if outcome.changed > 0 || unknown {
            self.output.invalidated(ticket, false);
        }
        Ok(EventKind::TransferFinished(outcome))
    }
    pub(super) async fn checkpoint(&self) -> Result<EventKind, Failure> {
        self.settled("reopen before maintenance")?;
        if self.access != Access::ReadWrite {
            return Err(Failure::input("Checkpoint requires write access"));
        }
        transfers::checkpoint(self.editor).await?;
        Ok(EventKind::TransferFinished(TransferOutcome {
            rows: 0,
            changed: 0,
            transaction: state(self.editor),
            error: None,
            message: "Checkpoint completed".into(),
            executed: false,
        }))
    }
    /// Checkpoints so the closed main file is complete; the session loop
    /// publishes the copy once every connection has closed.
    pub(super) async fn prepare_backup(&self, destination: &Path) -> Result<(), Failure> {
        if self.uncertain {
            return Err(Failure {
                kind: ErrorKind::Unknown,
                message: "Prior outcome UNKNOWN; reopen and inspect before maintenance".into(),
            });
        }
        if self.access != Access::ReadWrite {
            return Err(Failure {
                kind: ErrorKind::ReadOnly,
                message: "Backup-and-close requires write access for an explicit checkpoint".into(),
            });
        }
        if destination.exists() {
            return Err(Failure::input("Backup destination already exists"));
        }
        transfers::checkpoint(self.editor).await
    }
}
