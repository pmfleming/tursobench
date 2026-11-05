use super::{App, View};
use eframe::egui;
use tursobench_db::{Operation, profile::Request};
pub(super) struct State {
    pub open: bool,
    table: String,
    column: String,
    exact: bool,
    sample: usize,
}
impl Default for State {
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
        let mut open = self.tools.profile.open;
        if !open {
            return;
        }
        let mut run = None;
        egui::Window::new("Data profiling").open(&mut open).resizable(true).show(ctx, |ui| {
            let old = self.tools.profile.table.clone();
            egui::ComboBox::from_label("Table").selected_text(&self.tools.profile.table).show_ui(ui, |ui| {
                for table in self.schema.iter().filter(|s|s.kind == "table") { ui.selectable_value(&mut self.tools.profile.table,table.name.clone(),&table.name); }
            });
            if old != self.tools.profile.table { self.tools.profile.column.clear(); }
            ui.horizontal(|ui| { ui.label("Column (blank = all, up to 24)"); ui.text_edit_singleline(&mut self.tools.profile.column); });
            ui.checkbox(&mut self.tools.profile.exact,"Exact scan of the complete table");
            if !self.tools.profile.exact {
                ui.add(egui::DragValue::new(&mut self.tools.profile.sample).range(1..=100_000).prefix("First ").suffix(" rows"));
                ui.small("Prefix sample in engine scan order, not random. Statistics describe only those rows; distinctness does not establish table uniqueness.");
            } else { ui.small("Exact statistics may scan the table repeatedly. Query deadlines and discard apply."); }
            if ui.add_enabled(self.ready && !self.closing && !self.tools.profile.table.is_empty(),egui::Button::new("Generate profile")).clicked() {
                run = Some(Request { table:self.tools.profile.table.clone(),column:(!self.tools.profile.column.is_empty()).then(||self.tools.profile.column.clone()),sample_rows:(!self.tools.profile.exact).then_some(self.tools.profile.sample) });
            }
        });
        self.tools.profile.open = open;
        if let Some(request) = run {
            self.run_profile(request);
        }
    }
    pub(super) fn run_profile(&mut self, request: Request) {
        if self
            .submit(Operation::Profile(request.clone()), View::Diagnostics)
            .is_some()
        {
            self.document.profile = Some(request);
            self.tools.profile.open = false;
        }
    }
    pub(super) fn profile_header(&mut self, ui: &mut egui::Ui) {
        let Some(request) = self.document.profile.clone() else {
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
            if self.document.stale {
                ui.colored_label(ui.visuals().warn_fg_color, "Stale — data changed");
            }
            if ui
                .add_enabled(
                    self.document.active.is_none() && self.ready,
                    egui::Button::new("Refresh profile"),
                )
                .clicked()
            {
                self.run_profile(request);
            }
        });
        ui.collapsing("Value frequencies (top five per column)", |ui| {
            let rows: Vec<_> = self.document.result.rows.iter().filter(|r| matches!(r.get(1),Some(tursobench_core::Value::Text(s)) if s == "top value")).collect();
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
