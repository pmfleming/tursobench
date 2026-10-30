use super::*;
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
        ui.separator();
        ui.collapsing("Bookmarks", |ui| {
            if ui.button("Bookmark editor SQL").clicked()
                && self.sql.len() <= 64 * 1024
                && !self.preferences.bookmarks.contains(&self.sql)
            {
                self.preferences.bookmarks.insert(0, self.sql.clone());
                self.preferences.normalize();
                self.store.save(&self.preferences);
            }
            for sql in self.preferences.bookmarks.clone() {
                if ui.button(Value::Text(sql.clone()).preview()).clicked() {
                    self.sql = sql;
                }
            }
            if ui.button("Clear bookmarks").clicked() {
                self.preferences.bookmarks.clear();
                self.store.save(&self.preferences);
            }
        });
        ui.collapsing("Query history", |ui| {
            ui.small("Stored locally. SQL literals may contain sensitive data. Parameter values are not saved.");
            egui::ScrollArea::vertical().id_salt("history-list").max_height(200.0).show(ui, |ui| {
                for sql in self.preferences.history.clone() { if ui.button(Value::Text(sql.clone()).preview()).clicked() { self.sql = sql; } }
            });
            if ui.button("Clear history").clicked() { self.preferences.history.clear(); self.store.save(&self.preferences); }
        });
        ui.collapsing("Settings", |ui| {
            ui.label("Query row retention limit");
            ui.add(egui::DragValue::new(&mut self.preferences.row_limit).range(1..=10_000));
            ui.label("Busy timeout (ms), applies after reopen");
            ui.add(egui::DragValue::new(&mut self.preferences.busy_timeout_ms).range(0..=5000));
            if ui.button("Save settings").clicked() {
                self.preferences.normalize();
                self.store.save(&self.preferences);
                self.status = "Saving application-owned preferences…".into();
            }
        });
    }
    pub(super) fn usability_controls(&mut self, ui: &mut egui::Ui) {
        let finished = self.result.completion.as_ref();
        let scope = if let Some(c) = finished {
            if c.error.is_some() {
                "failed-query-partial-result"
            } else if c.delivery == Delivery::Discarded {
                "discarded-query-partial-result"
            } else if c.delivery == Delivery::Limited {
                "limited-query-result"
            } else if self.view == View::Browse {
                "table-page"
            } else {
                "complete-query-result"
            }
        } else {
            "unfinished"
        };
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    finished.is_some() && self.active.is_none() && self.export_job.is_none(),
                    egui::Button::new("Export displayed result…"),
                )
                .clicked()
            {
                let columns = self.result.columns.clone();
                let rows = self.result.rows.clone();
                let scope = scope.to_string();
                let wake = ui.ctx().clone();
                let (tx, rx) = std::sync::mpsc::sync_channel(1);
                self.export_job = Some(rx);
                std::thread::spawn(move || {
                    let result = if let Some(path) = rfd::FileDialog::new()
                        .set_title("Export to a NEW typed JSONL file")
                        .set_file_name("tursobench-result.jsonl")
                        .save_file()
                    {
                        crate::export::write_result(&path, &scope, &columns, &rows).map(|_| {
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
            ui.small(format!("Export scope: {scope}; never re-executes SQL"));
        });
    }
}
