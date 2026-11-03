use super::presentation::{self, Intent};
use super::{App, View, shell};
use eframe::egui;
use tursobench_core::Delivery;

impl App {
    pub(super) fn poll_storage(&mut self, ctx: &egui::Context) {
        self.store.flush();
        for result in self.store.results.try_iter() {
            if let Err(e) = result {
                self.status = format!("Could not save application preferences: {e}");
            }
        }
        if let Some(rx) = &self.export_job {
            let result = match rx.try_recv() {
                Ok(r) => Some(r),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("Export worker stopped unexpectedly".into()))
                }
                Err(_) => None,
            };
            if let Some(result) = result {
                self.export_job = None;
                self.status = result.unwrap_or_else(|e| format!("Export failed: {e}"));
                if self.quit {
                    if self.session.is_some() {
                        self.request_close();
                    } else {
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
        }
    }
    pub(super) fn history_controls(&mut self, ui: &mut egui::Ui) {
        match self.ui.sidebar {
            shell::ToolPanel::Saved => self.saved_queries_ui(ui),
            shell::ToolPanel::History => self.query_history_ui(ui),
            shell::ToolPanel::Settings => self.settings_ui(ui),
            _ => {}
        }
    }
    pub(super) fn load_sql_window(&mut self, sql: String) {
        if self.workspace.enabled
            && self
                .create_window(super::tiling::Kind::Sql, None, None, true)
                .is_none()
        {
            return;
        }
        self.document.sql = sql;
        self.document.params = "[]".into();
        self.document.ui.sql_open = true;
    }
    pub(super) fn export_scope(&self) -> &'static str {
        if self.document.kind == super::tiling::Kind::Record {
            return "pinned-record";
        }
        match self.document.result.completion.as_ref() {
            Some(c) if c.error.is_some() => "failed-query-partial-result",
            Some(c) if c.delivery == Delivery::Discarded => "discarded-query-partial-result",
            Some(c) if c.delivery == Delivery::Limited => "limited-query-result",
            Some(_)
                if self.document.view == View::Browse && self.document.tools.related.is_some() =>
            {
                "related-row-lookup"
            }
            Some(_) if self.document.view == View::Browse => "table-window",
            Some(_) => "complete-query-result",
            None => "unfinished",
        }
    }
    pub(super) fn usability_controls(&mut self, ui: &mut egui::Ui) {
        let scope = self.export_scope();
        let enabled = self.document.result.completion.is_some() && self.document.active.is_none();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(self.document.tools.format.label())
                .show_ui(ui, |ui| {
                    for format in crate::export::Format::ALL {
                        ui.selectable_value(
                            &mut self.document.tools.format,
                            format,
                            format.label(),
                        );
                    }
                });
            if self.document.tools.format == crate::export::Format::Sql {
                ui.label("Target table:");
                ui.text_edit_singleline(&mut self.document.tools.export_table);
            }
        });
        presentation::notice(
            ui,
            super::icons::Icon::Export,
            Intent::Neutral,
            &format!(
                "Scope: {scope} · retained engine values only, not staged overlays · never re-executes SQL"
            ),
        );
        presentation::footer(ui, |ui| {
            if ui
                .add_enabled(
                    enabled && self.export_job.is_none(),
                    presentation::action(ui, "Export retained result…", Intent::Primary),
                )
                .clicked()
            {
                let columns = self.document.result.columns.clone();
                let rows = self.document.result.rows.clone();
                let format = self.document.tools.format;
                let table = self.document.tools.export_table.clone();
                let wake = ui.ctx().clone();
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                self.export_job = Some(rx);
                std::thread::spawn(move || {
                    let result = if let Some(path) = rfd::FileDialog::new()
                        .set_title("Export to a NEW file — never overwrite")
                        .set_file_name(format!("tursobench-{scope}.{}", format.extension()))
                        .save_file()
                    {
                        let written = crate::export::write_result_as(
                            &path, format, scope, &columns, &rows, &table,
                        );
                        written.map(|_| {
                            format!(
                                "Exported {} rows ({scope}) to {}",
                                rows.len(),
                                path.display()
                            )
                        })
                    } else {
                        Ok("Export cancelled; no file written".into())
                    };
                    let _ = tx.send(result);
                    wake.request_repaint();
                });
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Copy retained TSV"))
                .clicked()
            {
                self.copy_results(ui.ctx(), crate::export::Format::Tsv);
            }
            if ui
                .add_enabled(enabled, egui::Button::new("Copy typed JSON"))
                .clicked()
            {
                self.copy_results(ui.ctx(), crate::export::Format::Json);
            }
        });
    }
}
