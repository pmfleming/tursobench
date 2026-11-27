use super::presentation::{self, Intent};
use super::{App, View};
use crate::export::Format;
use eframe::egui;
use std::sync::mpsc::TryRecvError;
use tursobench_core::Delivery;

impl App {
    pub(super) fn poll_storage(&mut self, ctx: &egui::Context) {
        self.store.flush();
        for result in self.store.results.try_iter() {
            if let Err(e) = result {
                self.database.status = format!("Could not save application preferences: {e}");
            }
        }
        let Some(rx) = &self.database.export_job else {
            return;
        };
        let result = match rx.try_recv() {
            Ok(result) => result,
            Err(TryRecvError::Disconnected) => Err("Export worker stopped unexpectedly".into()),
            Err(TryRecvError::Empty) => return,
        };
        self.database.export_job = None;
        self.database.status = result.unwrap_or_else(|e| format!("Export failed: {e}"));
        if self.quit && self.database.session.is_some() {
            self.request_close();
        } else if self.quit {
            self.continue_quit(ctx);
        }
    }
    pub(super) fn load_sql_window(&mut self, sql: String) {
        if self.database.workspace.enabled
            && self
                .create_window(super::tiling::Kind::Sql, None, None, true)
                .is_none()
        {
            return;
        }
        self.database.document.set_sql(sql);
        self.database.document.params = "[]".into();
        self.database.document.ui.sql_open = true;
    }
    pub(super) fn export_scope(&self) -> &'static str {
        if self.database.document.kind == super::tiling::Kind::Record {
            return "pinned-record";
        }
        match self.database.document.result.completion.as_ref() {
            Some(c) if c.error.is_some() => "failed-query-partial-result",
            Some(c) if c.delivery == Delivery::Discarded => "discarded-query-partial-result",
            Some(c) if c.delivery == Delivery::Limited => "limited-query-result",
            Some(_)
                if self.database.document.view == View::Browse
                    && self.database.document.tools.related.is_some() =>
            {
                "related-row-lookup"
            }
            Some(_) if self.database.document.view == View::Browse => "table-window",
            Some(_) => "complete-query-result",
            None => "unfinished",
        }
    }
    pub(super) fn retained_export_dialog(&mut self, ctx: &egui::Context) {
        if !self.database.ui.export_open {
            return;
        }
        let mut open = true;
        presentation::dialog(ctx, "Export retained result", &mut open)
            .default_width(600.0)
            .default_height(200.0)
            .max_width((ctx.content_rect().width() - 40.0).max(240.0))
            .max_height(ctx.content_rect().height() * 0.8)
            .vscroll(true)
            .show(ctx, |ui| self.usability_controls(ui));
        self.database.ui.export_open = open;
    }
    fn usability_controls(&mut self, ui: &mut egui::Ui) {
        if ui
            .add_enabled(
                self.database.ready,
                egui::Button::new("Complete export / import / maintenance…"),
            )
            .clicked()
        {
            self.open_transfers();
        }
        let scope = self.export_scope();
        let enabled = self.database.document.result.completion.is_some()
            && self.database.document.active.is_none();
        ui.horizontal_wrapped(|ui| {
            egui::ComboBox::from_id_salt("export-format")
                .selected_text(self.database.document.tools.format.label())
                .show_ui(ui, |ui| {
                    for format in Format::ALL {
                        ui.selectable_value(
                            &mut self.database.document.tools.format,
                            format,
                            format.label(),
                        );
                    }
                });
            if self.database.document.tools.format == Format::Sql {
                ui.label("Target table:");
                ui.text_edit_singleline(&mut self.database.document.tools.export_table);
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
                    enabled && self.database.export_job.is_none(),
                    presentation::action(ui, "Export retained result…", Intent::Primary),
                )
                .clicked()
            {
                self.start_export(ui.ctx(), scope);
            }
            for (label, format) in [
                ("Copy retained TSV", Format::Tsv),
                ("Copy typed JSON", Format::Json),
            ] {
                if ui.add_enabled(enabled, egui::Button::new(label)).clicked() {
                    self.copy_results(ui.ctx(), format);
                }
            }
        });
    }
    /// Choose a new file on a worker thread and write the retained result to it.
    fn start_export(&mut self, ctx: &egui::Context, scope: &'static str) {
        let result = &self.database.document.result;
        let (columns, rows) = (result.columns.clone(), result.rows.clone());
        let format = self.database.document.tools.format;
        let table = self.database.document.tools.export_table.clone();
        let wake = ctx.clone();
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        self.database.export_job = Some(rx);
        std::thread::spawn(move || {
            let path = rfd::FileDialog::new()
                .set_title("Export to a NEW file — never overwrite")
                .set_file_name(format!("tursobench-{scope}.{}", format.extension()))
                .save_file();
            let result = match path {
                Some(path) => {
                    crate::export::write_result_as(&path, format, scope, &columns, &rows, &table)
                        .map(|()| {
                            format!(
                                "Exported {} rows ({scope}) to {}",
                                rows.len(),
                                path.display()
                            )
                        })
                }
                None => Ok("Export cancelled; no file written".into()),
            };
            let _ = tx.send(result);
            wake.request_repaint();
        });
    }
}
