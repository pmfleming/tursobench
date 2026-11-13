use super::{App, tiling::Kind};
use crate::recovery::Document;
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct State {
    observed: Vec<Document>,
    revisions: Vec<(u64, u64)>,
    changed: Option<Instant>,
    cleared: bool,
}
impl State {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self {
            observed: vec![],
            revisions: vec![],
            changed: None,
            cleared: false,
        }
    }
}
impl App {
    fn recovery_views(&self) -> Vec<&super::document::Document> {
        let mut documents: Vec<_> = self
            .database
            .documents
            .values()
            .chain(std::iter::once(&self.database.document))
            .filter(|d| d.kind == Kind::Sql && d.migration.is_none())
            .collect();
        // Focus changes move documents between owners, not their recovery order.
        documents.sort_by_key(|d| d.id);
        documents
    }
    fn recovery_revisions(&self) -> Vec<(u64, u64)> {
        self.recovery_views()
            .into_iter()
            .map(|d| (d.id, d.sql_revision))
            .collect()
    }
    fn recovery_unchanged(&self) -> bool {
        let documents = self.recovery_views();
        documents.len() == self.database.recovery.observed.len()
            && documents
                .iter()
                .zip(&self.database.recovery.observed)
                .all(|(current, saved)| {
                    current.id == saved.id
                        && current.sql == saved.sql
                        && (self.database.recovery.cleared
                            || current.tools.selection == saved.selection)
                })
    }
    fn recovery_documents(&self) -> Vec<Document> {
        self.recovery_views()
            .into_iter()
            .map(|d| Document {
                id: d.id,
                sql: d.sql.clone(),
                selection: {
                    let n = d.sql.chars().count();
                    d.tools.selection.start.min(n)..d.tools.selection.end.min(n)
                },
            })
            .collect()
    }
    pub(super) fn poll_recovery(&mut self, ctx: &egui::Context) {
        self.recovery_store.poll();
        if !self.database.ready || !self.preferences.sql_recovery {
            return;
        }
        let revisions = self.recovery_revisions();
        if revisions != self.database.recovery.revisions {
            self.database.recovery.revisions = revisions;
            self.database.recovery.changed = Some(Instant::now());
        }
        if let Some(changed) = self.database.recovery.changed {
            let remaining = Duration::from_secs(1).saturating_sub(changed.elapsed());
            if remaining.is_zero() {
                self.save_recovery();
            } else {
                ctx.request_repaint_after(remaining);
            }
        }
    }
    pub(super) fn save_recovery(&mut self) {
        if !self.database.ready
            || !self.preferences.sql_recovery
            || self.database.tools.database_scope.is_empty()
        {
            return;
        }
        self.database.recovery.revisions = self.recovery_revisions();
        self.database.recovery.changed = None;
        if self.recovery_unchanged()
            && (self.database.recovery.cleared || self.recovery_store.error.is_none())
        {
            return;
        }
        let docs = self.recovery_documents();
        self.database.recovery.cleared = false;
        self.database.recovery.observed.clone_from(&docs);
        self.recovery_store
            .save(self.database.tools.database_scope.clone(), docs);
    }
    pub(super) fn restore_sql(&mut self) {
        if self.preferences.sql_recovery {
            let focus = self.database.workspace.focused;
            let active = self.database.workspace.active;
            let expanded = self.database.workspace.expanded;
            let minimized = self.database.workspace.minimized.clone();
            let visible = self.database.workspace.visible.clone();
            let docs = self
                .recovery_store
                .documents(&self.database.tools.database_scope);
            let mut sql_ids: Vec<_> = self
                .window_ids()
                .into_iter()
                .filter(|id| self.document_ref(*id).is_some_and(|d| d.kind == Kind::Sql))
                .collect();
            sql_ids.sort_unstable();
            for (index, saved) in docs.into_iter().enumerate() {
                let id = sql_ids
                    .get(index)
                    .copied()
                    .or_else(|| self.create_window(Kind::Sql, None, None, true));
                if let Some(id) = id {
                    self.switch_document(id);
                    self.database.document.set_sql(saved.sql);
                    self.database.document.tools.selection = saved.selection.clone();
                    self.database.document.restore_cursor = Some(saved.selection);
                }
            }
            self.switch_document(focus);
            self.database.workspace.focused = focus;
            self.database.workspace.active = active;
            self.database.workspace.expanded = expanded;
            self.database.workspace.minimized = minimized;
            self.database.workspace.visible.extend(visible);
        }
        self.database.recovery.observed = self.recovery_documents();
        self.database.recovery.revisions = self.recovery_revisions();
        self.database.recovery.changed = None;
    }
    pub(super) fn recovery_settings(&mut self, ui: &mut egui::Ui) {
        if crate::app::presentation::checkbox(
            ui,
            &mut self.preferences.sql_recovery,
            "Recover SQL documents",
        )
        .changed()
        {
            self.store.save(&self.preferences);
            self.database.recovery.observed.clear();
            self.database.recovery.revisions = self.recovery_revisions();
            self.database.recovery.changed = self.preferences.sql_recovery.then(Instant::now);
            self.database.recovery.cleared = false;
        }
        ui.small("SQL text may contain sensitive literals. Bindings and results are not saved.");
        if ui.button("Clear saved SQL recovery").clicked() {
            self.recovery_store.clear();
            self.database.recovery.cleared = true;
            self.database.recovery.observed = self.recovery_documents();
            self.database.recovery.revisions = self.recovery_revisions();
            self.database.recovery.changed = None;
            for database in self.parked.values_mut() {
                database.recovery.cleared = true;
                database.recovery.changed = None;
                database.recovery.observed = database
                    .documents
                    .values()
                    .chain(std::iter::once(&database.document))
                    .filter(|d| d.kind == Kind::Sql && d.migration.is_none())
                    .map(|d| Document {
                        id: d.id,
                        sql: d.sql.clone(),
                        selection: d.tools.selection.clone(),
                    })
                    .collect();
                database.recovery.observed.sort_by_key(|d| d.id);
                database.recovery.revisions = database
                    .documents
                    .values()
                    .chain(std::iter::once(&database.document))
                    .filter(|d| d.kind == Kind::Sql && d.migration.is_none())
                    .map(|d| (d.id, d.sql_revision))
                    .collect();
                database.recovery.revisions.sort_unstable();
            }
        }
        if let Some(error) = &self.recovery_store.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn idle_cursor_and_focus_changes_do_not_schedule_sql_snapshots() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.tools.database_scope = "/tmp/recovery-revisions.db".into();
        app.initialize_workspace();
        app.database.ready = true;
        app.open_sql_window(false);
        let editor = app.database.document.id;
        app.database.document.set_sql("SELECT 'λ'".into());
        app.save_recovery();
        for _ in 0..3 {
            app.poll_recovery(&ctx);
            assert!(app.database.recovery.changed.is_none());
        }
        app.database.document.tools.selection = 2..4;
        app.poll_recovery(&ctx);
        assert!(app.database.recovery.changed.is_none());
        // Cursor recovery is still flushed on explicit save (switch/close).
        assert!(!app.recovery_unchanged());
        app.save_recovery();
        assert!(app.recovery_unchanged());
        let before = app.recovery_revisions();
        app.focus_pane(Kind::Map);
        app.poll_recovery(&ctx);
        assert_eq!(app.recovery_revisions(), before);
        assert!(app.database.recovery.changed.is_none());
        app.switch_document(editor);
        app.database.recovery.cleared = true;
        app.database.document.tools.selection = 0..0;
        assert!(
            app.recovery_unchanged(),
            "cursor changes must not undo Clear recovery"
        );
        app.database.document.set_sql("SELECT 'changed'".into());
        app.poll_recovery(&ctx);
        assert!(app.database.recovery.changed.is_some());
        app.database.recovery.changed = Some(Instant::now() - Duration::from_secs(2));
        app.poll_recovery(&ctx);
        assert!(app.database.recovery.changed.is_none());
        assert!(!app.database.recovery.cleared);
        assert_eq!(app.database.recovery.observed[0].sql, "SELECT 'changed'");
        app.open_sql_window(true);
        app.poll_recovery(&ctx);
        assert!(
            app.database.recovery.changed.is_some(),
            "new editors must be recovered"
        );
    }
    #[test]
    fn restoring_multiple_editors_preserves_order_when_the_last_editor_is_focused() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.tools.database_scope = "/tmp/recovery-order.db".into();
        app.initialize_workspace();
        app.database.ready = true;
        app.open_sql_window(false);
        let first = app.database.document.id;
        app.database.document.set_sql("SELECT 'first'".into());
        app.open_sql_window(true);
        let second = app.database.document.id;
        app.database.document.set_sql("SELECT 'second'".into());
        app.save_recovery();
        let until = Instant::now() + Duration::from_secs(5);
        while app
            .recovery_store
            .documents(&app.database.tools.database_scope)
            .len()
            < 2
        {
            assert!(Instant::now() < until);
            app.recovery_store.poll();
            std::thread::yield_now();
        }
        app.database.document.set_sql(String::new());
        app.database
            .documents
            .get_mut(&first)
            .unwrap()
            .set_sql(String::new());
        app.restore_sql();
        assert_eq!(app.document_ref(first).unwrap().sql, "SELECT 'first'");
        assert_eq!(app.document_ref(second).unwrap().sql, "SELECT 'second'");
        app.poll_recovery(&ctx);
        assert!(app.database.recovery.changed.is_none());
    }
    #[test]
    fn recovery_restores_only_sql_and_cursor_without_execution() {
        let (ctx, mut app) = crate::app::test_ui::app();
        app.database.tools.database_scope = "/tmp/recovery-test.db".into();
        app.initialize_workspace();
        app.database.ready = true;
        app.open_sql_window(false);
        app.database.document.sql = "SELECT :secret, 'λ'".into();
        app.database.document.params = "{\":secret\":123}".into();
        app.database.document.tools.selection = 2..5;
        app.database.document.migration =
            Some(std::sync::Arc::new(tursobench_core::SchemaSnapshot {
                version: 0,
                objects: vec![],
            }));
        assert!(
            app.recovery_documents()
                .iter()
                .all(|d| d.sql != app.database.document.sql)
        );
        app.database.document.migration = None;
        app.save_recovery();
        // Wait for the worker acknowledgement, not a fixed scheduling delay.
        let until = Instant::now() + Duration::from_secs(5);
        while app
            .recovery_store
            .documents(&app.database.tools.database_scope)
            .is_empty()
        {
            assert!(Instant::now() < until);
            app.recovery_store.poll();
            std::thread::yield_now();
        }
        app.database.documents.clear();
        app.database.document = Default::default();
        app.database.workspace = Default::default();
        app.initialize_workspace();
        app.restore_sql();
        app.open_sql_window(false);
        assert_eq!(app.database.document.sql, "SELECT :secret, 'λ'");
        assert_eq!(app.database.document.restore_cursor, Some(2..5));
        assert_eq!(app.database.document.params, "[]");
        assert!(app.database.document.active.is_none());
        assert!(app.database.session.is_none());
        assert!(!app.database.writable);
        app.recovery_store.clear();
        app.database.recovery.cleared = true;
        app.database.recovery.observed = app.recovery_documents();
        app.save_recovery();
        app.poll_recovery(&ctx);
    }
}
