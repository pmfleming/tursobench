use super::{App, tiling::Kind};
use crate::recovery::Document;
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct State {
    observed: Vec<Document>,
    changed: Option<Instant>,
    checked: Instant,
    cleared: bool,
}
impl State {
    pub fn new(_ctx: &egui::Context) -> Self {
        Self {
            observed: vec![],
            changed: None,
            checked: Instant::now(),
            cleared: false,
        }
    }
}
impl App {
    fn recovery_documents(&self) -> Vec<Document> {
        self.window_ids()
            .into_iter()
            .filter_map(|id| {
                let d = self.document_ref(id)?;
                (d.kind == Kind::Sql && d.migration.is_none()).then(|| Document {
                    id,
                    sql: d.sql.clone(),
                    selection: {
                        let n = d.sql.chars().count();
                        d.tools.selection.start.min(n)..d.tools.selection.end.min(n)
                    },
                })
            })
            .collect()
    }
    pub(super) fn poll_recovery(&mut self, ctx: &egui::Context) {
        self.recovery_store.poll();
        if !self.database.ready || !self.preferences.sql_recovery {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        if self.database.recovery.checked.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.database.recovery.checked = Instant::now();
        let docs = self.recovery_documents();
        if docs != self.database.recovery.observed {
            self.database.recovery.cleared = false;
            self.database.recovery.observed = docs;
            self.database.recovery.changed = Some(Instant::now());
        }
        if self
            .database
            .recovery
            .changed
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.save_recovery();
        }
    }
    pub(super) fn save_recovery(&mut self) {
        if !self.database.ready
            || !self.preferences.sql_recovery
            || self.database.tools.database_scope.is_empty()
        {
            return;
        }
        let docs = self.recovery_documents();
        if self.database.recovery.cleared && docs == self.database.recovery.observed {
            return;
        }
        self.database.recovery.observed.clone_from(&docs);
        self.database.recovery.changed = None;
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
            let sql_ids: Vec<_> = self
                .window_ids()
                .into_iter()
                .filter(|id| self.document_ref(*id).is_some_and(|d| d.kind == Kind::Sql))
                .collect();
            for (index, saved) in docs.into_iter().enumerate() {
                let id = sql_ids
                    .get(index)
                    .copied()
                    .or_else(|| self.create_window(Kind::Sql, None, None, true));
                if let Some(id) = id {
                    self.switch_document(id);
                    self.database.document.sql = saved.sql;
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
        self.database.recovery.changed = None;
    }
    pub(super) fn recovery_settings(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        if ui
            .checkbox(&mut self.preferences.sql_recovery, "Recover SQL documents")
            .changed()
        {
            self.store.save(&self.preferences);
            self.database.recovery.observed = self.recovery_documents();
            self.database.recovery.changed = self.preferences.sql_recovery.then(Instant::now);
            self.database.recovery.cleared = false;
        }
        ui.small("SQL text may contain sensitive literals. Bindings and results are not saved.");
        if ui.button("Clear saved SQL recovery").clicked() {
            self.recovery_store.clear();
            self.database.recovery.cleared = true;
            self.database.recovery.observed = self.recovery_documents();
            self.database.recovery.changed = None;
            for database in self.parked.values_mut() {
                database.recovery.cleared = true;
                database.recovery.changed = None;
                database.recovery.observed = database
                    .documents
                    .values()
                    .chain(std::iter::once(&database.document))
                    .filter(|d| d.kind == Kind::Sql)
                    .map(|d| Document {
                        id: d.id,
                        sql: d.sql.clone(),
                        selection: d.tools.selection.clone(),
                    })
                    .collect();
                database.recovery.observed.sort_by_key(|d| d.id);
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
