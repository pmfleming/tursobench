//! Profiling and PRAGMA tools exposed by Database checks.
use super::{App, View};
use eframe::egui;
use tursobench_db::{Operation, admin::Change, profile::Request};
pub(super) struct Admin {
    pub open: bool,
    busy: u64,
    cache: u32,
}
impl Default for Admin {
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
        let mut open = self.database.tools.admin.open;
        if !open {
            return;
        }
        let mut request = None;
        egui::Window::new("Database administration").open(&mut open).show(ctx,|ui| {
            ui.label("Read effective PRAGMAs and file sizes; unsupported values are reported individually.");
            if ui.add_enabled(self.database.ready && !self.database.closing,egui::Button::new("Refresh dashboard")).clicked() { request = Some(None); }
            ui.separator();
            ui.label("Editor connection settings (reset on reopen). Finish transactions before applying.");
            ui.small("Cache capacity affects the shared pager. Browse/edit-worker busy timeouts are unchanged. Foreign-key enforcement remains enabled.");
            let enabled = self.database.ready && self.database.writable && !self.database.closing && !self.database.uncertain_edit && self.database.transaction == tursobench_core::TransactionState::Autocommit;
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.database.tools.admin.busy).range(0..=5000).prefix("Busy timeout ").suffix(" ms"));
                if ui.add_enabled(enabled,egui::Button::new("Apply timeout")).clicked() { request = Some(Some(Change::EditorBusyTimeout(self.database.tools.admin.busy))); }
            });
            ui.horizontal(|ui| {
                ui.add(egui::DragValue::new(&mut self.database.tools.admin.cache).range(64..=65536).prefix("Cache ").suffix(" KiB"));
                if ui.add_enabled(enabled,egui::Button::new("Apply cache size")).clicked() { request = Some(Some(Change::EditorCacheKiB(self.database.tools.admin.cache))); }
            });
            if !self.database.writable { ui.weak("Open explicitly for writes to enable changes."); }
            if ui.button("Checkpoint / verified backup…").clicked() { self.open_transfers(); }
        });
        self.database.tools.admin.open = open;
        if let Some(change) = request {
            self.submit(Operation::Admin(change), View::Diagnostics);
        }
    }
}

pub(super) struct Profile {
    pub open: bool,
    table: String,
    column: String,
    exact: bool,
    sample: usize,
}
impl Default for Profile {
    fn default() -> Self {
        Self {
            open: false,
            table: String::new(),
            column: String::new(),
            exact: false,
            sample: 10_000,
        }
    }
}
impl App {
    pub(super) fn profile_ui(&mut self, ctx: &egui::Context) {
        let mut open = self.database.tools.profile.open;
        if !open {
            return;
        }
        let mut run = None;
        egui::Window::new("Data profiling").open(&mut open).resizable(true).show(ctx, |ui| {
            let old = self.database.tools.profile.table.clone();
            egui::ComboBox::from_label("Table").selected_text(&self.database.tools.profile.table).show_ui(ui, |ui| {
                for table in self.database.schema.iter().filter(|s|s.kind == "table") { ui.selectable_value(&mut self.database.tools.profile.table,table.name.clone(),&table.name); }
            });
            if old != self.database.tools.profile.table { self.database.tools.profile.column.clear(); }
            ui.horizontal(|ui| { ui.label("Column (blank = all, up to 24)"); ui.text_edit_singleline(&mut self.database.tools.profile.column); });
            crate::app::presentation::checkbox(ui, &mut self.database.tools.profile.exact,"Exact scan of the complete table");
            if !self.database.tools.profile.exact {
                ui.add(egui::DragValue::new(&mut self.database.tools.profile.sample).range(1..=100_000).prefix("First ").suffix(" rows"));
                ui.small("Prefix sample in engine scan order, not random. Statistics describe only those rows; distinctness does not establish table uniqueness.");
            } else { ui.small("Exact statistics may scan the table repeatedly. Query deadlines and discard apply."); }
            if ui.add_enabled(self.database.ready && !self.database.closing && !self.database.tools.profile.table.is_empty(),egui::Button::new("Generate profile")).clicked() {
                run = Some(Request { table:self.database.tools.profile.table.clone(),column:(!self.database.tools.profile.column.is_empty()).then(||self.database.tools.profile.column.clone()),sample_rows:(!self.database.tools.profile.exact).then_some(self.database.tools.profile.sample) });
            }
        });
        self.database.tools.profile.open = open;
        if let Some(request) = run {
            self.run_profile(request);
        }
    }
    pub(super) fn run_profile(&mut self, request: Request) {
        if self
            .submit(Operation::Profile(request.clone()), View::Diagnostics)
            .is_some()
        {
            self.database.document.profile = Some(request);
            self.database.tools.profile.open = false;
        }
    }
    pub(super) fn profile_header(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.database.document.profile.clone() else {
            return;
        };
        ui.horizontal_wrapped(|ui| {
            ui.strong(format!(
                "Profile: {}{}",
                request.table,
                request
                    .column
                    .as_ref()
                    .map_or(String::new(), |c| format!(".{c}"))
            ));
            ui.label(request.sample_rows.map_or("Exact scan".into(), |n| {
                format!("Prefix sample: at most {n} rows")
            }));
            if self.database.document.stale {
                ui.colored_label(ui.visuals().warn_fg_color, "Stale — data changed");
            }
            if ui
                .add_enabled(
                    self.database.document.active.is_none() && self.database.ready,
                    egui::Button::new("Refresh profile"),
                )
                .clicked()
            {
                self.run_profile(request);
            }
        });
        ui.collapsing("Value frequencies (top five per column)", |ui| {
            let rows: Vec<_> = self.database.document.result.rows.iter().filter(|r| matches!(r.get(1),Some(tursobench_core::Value::Text(s)) if s == "top value")).collect();
            let maximum = rows.iter().filter_map(|r| match r.get(3) { Some(tursobench_core::Value::Integer(n)) => Some(*n), _ => None }).max().unwrap_or(1).max(1);
            for row in rows {
                if let Some(tursobench_core::Value::Integer(n)) = row.get(3) {
                    ui.add(egui::ProgressBar::new(*n as f32 / maximum as f32).text(format!("{} · {} · {n}",row[0].preview(),row[2].preview())));
                }
            }
        });
        ui.small("Rows reports the actual population examined. Frequencies exclude NULL; numeric aggregates use numeric values only. Sum is floating point. Results are committed browse state.");
    }
}
