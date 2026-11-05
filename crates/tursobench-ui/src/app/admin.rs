use super::{App, View};
use eframe::egui;
use tursobench_db::{Operation, admin::Change};
pub(super) struct State {
    pub open: bool,
    busy: u64,
    cache: u32,
}
impl Default for State {
    fn default() -> Self {
        Self {
            open: false,
            busy: 250,
            cache: 2048,
        }
    }
}
impl App {
    pub(super) fn admin_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.tools.admin.open;
        if !open {
            return;
        }
        let mut request = None;
        egui::Window::new("Database administration").open(&mut open).show(ctx,|ui| {
            ui.label("Read effective PRAGMAs and file sizes; unsupported values are reported individually.");
            if ui.add_enabled(self.ready && !self.closing,egui::Button::new("Refresh dashboard")).clicked() { request = Some(None); }
            ui.separator();
            ui.label("Editor connection settings (reset on reopen). Finish transactions before applying.");
            ui.small("Cache capacity affects the shared pager. Browse/edit-worker busy timeouts are unchanged. Foreign-key enforcement remains enabled.");
            let enabled = self.ready && self.writable && !self.closing && !self.uncertain_edit && self.transaction == tursobench_core::TransactionState::Autocommit;
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.tools.admin.busy).range(0..=5000).prefix("Busy timeout ").suffix(" ms"));
                if ui.add_enabled(enabled,egui::Button::new("Apply timeout")).clicked() { request = Some(Some(Change::EditorBusyTimeout(self.tools.admin.busy))); }
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.tools.admin.cache).range(64..=65536).prefix("Cache ").suffix(" KiB"));
                if ui.add_enabled(enabled,egui::Button::new("Apply cache size")).clicked() { request = Some(Some(Change::EditorCacheKiB(self.tools.admin.cache))); }
            });
            if !self.writable { ui.weak("Open explicitly for writes to enable changes."); }
            if ui.button("Checkpoint / verified backup…").clicked() { self.open_transfers(); }
        });
        self.tools.admin.open = open;
        if let Some(change) = request {
            self.submit(Operation::Admin(change), View::Diagnostics);
        }
    }
}
