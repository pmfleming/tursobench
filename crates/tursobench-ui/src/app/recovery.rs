use super::{App, tiling::Kind};
use crate::recovery::Document;
use eframe::egui;
use std::time::{Duration, Instant};

pub(super) struct State {
    pub store: crate::recovery::Store,
    observed: Vec<Document>,
    changed: Option<Instant>,
    checked: Instant,
    cleared: bool,
}
impl State {
    pub fn new(ctx: &egui::Context) -> Self {
        let wake = ctx.clone();
        Self {
            store: crate::recovery::Store::start(move || wake.request_repaint()),
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
                (d.kind == Kind::Sql).then(|| Document {
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
        self.recovery.store.poll();
        if !self.ready || !self.preferences.sql_recovery {
            return;
        }
        ctx.request_repaint_after(Duration::from_millis(500));
        if self.recovery.checked.elapsed() < Duration::from_millis(500) {
            return;
        }
        self.recovery.checked = Instant::now();
        let docs = self.recovery_documents();
        if docs != self.recovery.observed {
            self.recovery.cleared = false;
            self.recovery.observed = docs;
            self.recovery.changed = Some(Instant::now());
        }
        if self
            .recovery
            .changed
            .is_some_and(|t| t.elapsed() >= Duration::from_secs(1))
        {
            self.save_recovery();
        }
    }
    pub(super) fn save_recovery(&mut self) {
        if !self.ready || !self.preferences.sql_recovery || self.tools.database_scope.is_empty() {
            return;
        }
        let docs = self.recovery_documents();
        if self.recovery.cleared && docs == self.recovery.observed {
            return;
        }
        self.recovery.observed.clone_from(&docs);
        self.recovery.changed = None;
        self.recovery
            .store
            .save(self.tools.database_scope.clone(), docs);
    }
    pub(super) fn restore_sql(&mut self) {
        if self.preferences.sql_recovery {
            let focus = self.workspace.focused;
            let active = self.workspace.active;
            let expanded = self.workspace.expanded;
            let minimized = self.workspace.minimized.clone();
            let visible = self.workspace.visible.clone();
            let docs = self.recovery.store.documents(&self.tools.database_scope);
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
                    self.document.sql = saved.sql;
                    self.document.tools.selection = saved.selection.clone();
                    self.document.restore_cursor = Some(saved.selection);
                }
            }
            self.switch_document(focus);
            self.workspace.focused = focus;
            self.workspace.active = active;
            self.workspace.expanded = expanded;
            self.workspace.minimized = minimized;
            self.workspace.visible.extend(visible);
        }
        self.recovery.observed = self.recovery_documents();
        self.recovery.changed = None;
    }
    pub(super) fn recovery_settings(&mut self, ui: &mut egui::Ui) {
        ui.separator();
        if ui
            .checkbox(&mut self.preferences.sql_recovery, "Recover SQL documents")
            .changed()
        {
            self.store.save(&self.preferences);
            self.recovery.observed = self.recovery_documents();
            self.recovery.changed = self.preferences.sql_recovery.then(Instant::now);
            self.recovery.cleared = false;
        }
        ui.small("SQL text may contain sensitive literals. Bindings and results are not saved.");
        if ui.button("Clear saved SQL recovery").clicked() {
            self.recovery.store.clear();
            self.recovery.cleared = true;
            self.recovery.observed = self.recovery_documents();
            self.recovery.changed = None;
        }
        if let Some(error) = &self.recovery.store.error {
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
        app.tools.database_scope = "/tmp/recovery-test.db".into();
        app.initialize_workspace();
        app.ready = true;
        app.open_sql_window(false);
        app.document.sql = "SELECT :secret, 'λ'".into();
        app.document.params = "{\":secret\":123}".into();
        app.document.tools.selection = 2..5;
        app.save_recovery();
        // Wait for the worker acknowledgement, not a fixed scheduling delay.
        let until = Instant::now() + Duration::from_secs(5);
        while app
            .recovery
            .store
            .documents(&app.tools.database_scope)
            .is_empty()
        {
            assert!(Instant::now() < until);
            app.recovery.store.poll();
            std::thread::yield_now();
        }
        app.documents.clear();
        app.document = Default::default();
        app.workspace = Default::default();
        app.initialize_workspace();
        app.restore_sql();
        app.open_sql_window(false);
        assert_eq!(app.document.sql, "SELECT :secret, 'λ'");
        assert_eq!(app.document.restore_cursor, Some(2..5));
        assert_eq!(app.document.params, "[]");
        assert!(app.document.active.is_none());
        assert!(app.session.is_none());
        assert!(!app.writable);
        app.recovery.store.clear();
        app.recovery.cleared = true;
        app.recovery.observed = app.recovery_documents();
        app.save_recovery();
        app.poll_recovery(&ctx);
    }
}
