use super::{App, View, shell};
use eframe::egui;
use tursobench_core::{Delivery, Value};

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
    fn saved_queries_ui(&mut self, ui: &mut egui::Ui) {
        let mut load = None;
        ui.heading("Saved queries");
        ui.vertical(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut self.tools.bookmark_name)
                    .hint_text("Bookmark name")
                    .char_limit(64),
            );
            if ui.button("Bookmark editor SQL").clicked() {
                let name = if self.tools.bookmark_name.is_empty() {
                    self.document.sql.chars().take(60).collect()
                } else {
                    self.tools.bookmark_name.clone()
                };
                self.preferences
                    .bookmark(&self.tools.database_scope, &name, &self.document.sql);
                self.store.save(&self.preferences);
            }
            ui.add(
                egui::TextEdit::singleline(&mut self.tools.bookmark_search)
                    .hint_text("Search bookmarks…"),
            );
            ui.small("Rename inline. Load clears bindings without executing.");
            let search = self.tools.bookmark_search.to_lowercase();
            let mut remove = None;
            let mut renamed = false;
            for (index, bookmark) in self.preferences.saved_queries.iter_mut().enumerate() {
                if !(bookmark.database.is_empty() || bookmark.database == self.tools.database_scope)
                    || !format!("{} {}", bookmark.name, bookmark.sql)
                        .to_lowercase()
                        .contains(&search)
                {
                    continue;
                }
                ui.horizontal_wrapped(|ui| {
                    if ui
                        .small_button("Load")
                        .on_hover_text(&bookmark.sql)
                        .clicked()
                    {
                        load = Some(bookmark.sql.clone());
                    }
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut bookmark.name)
                                .char_limit(64)
                                .desired_width(130.0),
                        )
                        .changed()
                    {
                        renamed = true;
                    }
                    if ui
                        .small_button("Delete")
                        .on_hover_text("Delete bookmark")
                        .clicked()
                    {
                        remove = Some(index);
                    }
                });
                if bookmark.database.is_empty() {
                    ui.small("Unscoped / legacy bookmark");
                }
            }
            if renamed {
                self.store.save(&self.preferences);
            }
            if let Some(index) = remove {
                self.preferences.saved_queries.remove(index);
                self.store.save(&self.preferences);
            }
            if ui.button("Clear ALL bookmarks").clicked() {
                self.preferences.saved_queries.clear();
                self.preferences.bookmarks.clear();
                self.store.save(&self.preferences);
            }
        });
        if let Some(sql) = load {
            self.load_sql_window(sql);
        }
    }
    fn query_history_ui(&mut self, ui: &mut egui::Ui) {
        let mut load = None;
        ui.heading("Query history");
        ui.vertical(|ui|{
        ui.small("This database + legacy unscoped entries. SQL may contain secrets; bindings are never saved. Loading clears bindings.");
        ui.add(egui::TextEdit::singleline(&mut self.tools.history_search).hint_text("Search history…"));
        ui.checkbox(&mut self.tools.slow_only,"Slow only");ui.checkbox(&mut self.tools.sort_slow,"Slowest first");
        let search=self.tools.history_search.to_lowercase();let threshold=self.preferences.slow_query_ms;
        let mut entries:Vec<_>=self.preferences.query_history.iter().enumerate().filter(|(_,h)|(h.database.is_empty()||h.database==self.tools.database_scope)&&h.sql.to_lowercase().contains(&search)&&(!self.tools.slow_only||h.elapsed_ms.is_some_and(|ms|ms>=threshold))).collect();
        if self.tools.sort_slow{entries.sort_by_key(|(_,h)|std::cmp::Reverse(h.elapsed_ms));}
        let mut remove=None;
        egui::ScrollArea::vertical().id_salt("history-list").max_height(200.0).show(ui,|ui|{
            for(index,h)in entries{
                ui.horizontal(|ui| {
                    if ui.small_button("×").clicked() { remove=Some(index); }
                    if ui.add_sized([ui.available_width().max(40.0),28.0],egui::Button::new(Value::Text(h.sql.clone()).preview()).truncate()).on_hover_text(&h.sql).clicked() { load=Some(h.sql.clone()); }
                });
                let time=h.elapsed_ms.map_or("untimed".into(),|ms|format!("{ms} ms"));let label=format!("{time}{}{}{}",if h.failed{" · failed"}else{""},if h.elapsed_ms.is_some_and(|ms|ms>=threshold){" · slow"}else{""},if h.database.is_empty(){" · unscoped"}else{""});
                if h.failed||h.elapsed_ms.is_some_and(|ms|ms>=threshold){ui.colored_label(egui::Color32::from_rgb(190,120,50),label);}else{ui.small(label);}
            }
        });
        if let Some(index)=remove{self.preferences.query_history.remove(index);self.store.save(&self.preferences);}
        if ui.button("Clear ALL stored history").clicked(){self.preferences.query_history.clear();self.preferences.history.clear();self.store.save(&self.preferences);}
    });
        if let Some(sql) = load {
            self.load_sql_window(sql);
        }
    }
    fn load_sql_window(&mut self, sql: String) {
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
    pub(super) fn settings_ui(&mut self, ui: &mut egui::Ui) {
        ui.heading("Settings");
        ui.label(&self.ui.font_label);
        ui.vertical(|ui| {
            ui.label("Query row retention limit");
            ui.add(egui::DragValue::new(&mut self.preferences.row_limit).range(1..=10_000));
            ui.label("Busy timeout (ms), applies after reopen");
            ui.add(egui::DragValue::new(&mut self.preferences.busy_timeout_ms).range(0..=5000));
            ui.label("Slow query threshold (ms)");
            ui.add(egui::DragValue::new(&mut self.preferences.slow_query_ms).range(1..=60_000));
            ui.label("NULL display marker (types unchanged)");
            ui.add(egui::TextEdit::singleline(&mut self.preferences.null_display).char_limit(32));
            ui.label("Appearance");
            let before = self.preferences.appearance;
            for (mode, label) in [(crate::appearance::Mode::System, "Follow system"), (crate::appearance::Mode::Light, "Light"), (crate::appearance::Mode::Dark, "Dark")] {
                ui.radio_value(&mut self.preferences.appearance, Some(mode), label);
            }
            if self.preferences.legacy_appearance {
                ui.small("Your previous light/dark choice was retained. Choose Follow system to track desktop changes.");
            }
            if before != self.preferences.appearance {
                self.preferences.legacy_appearance = false;
                self.apply_theme(ui.ctx());
                self.store.save(&self.preferences);
            }
            if ui.button("Save settings").clicked() {
                self.preferences.normalize();
                self.store.save(&self.preferences);
                self.status = "Saving application-owned preferences…".into();
            }
            ui.separator();
            egui::CollapsingHeader::new("Database discovery").show(ui, |ui| {
                self.discovery_settings_ui(ui);
                if let Some(message) = &self.databases.message {
                    ui.label(message);
                }
            });
        });
    }
    pub(super) fn export_scope(&self) -> &'static str {
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
            if ui
                .add_enabled(
                    enabled && self.export_job.is_none(),
                    egui::Button::new("Export retained result…"),
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
        ui.small(format!("Scope: {scope} · retained engine values only, not staged overlays · never re-executes SQL"));
    }
}
