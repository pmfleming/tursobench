use super::{
    App,
    icons::{self, Icon},
    sessions::DatabaseWorkspace,
};
use eframe::egui;
use std::path::Path;
use tursobench_core::TransactionState;

/// A small presentation snapshot; rendering never swaps or copies result buffers.
struct DatabaseRow {
    id: u64,
    path: String,
    writable: bool,
    transaction: TransactionState,
    drafts: usize,
    closing: bool,
}

impl From<&DatabaseWorkspace> for DatabaseRow {
    fn from(database: &DatabaseWorkspace) -> Self {
        Self {
            id: database.id,
            path: database.path.clone(),
            writable: database.writable,
            transaction: database.transaction,
            drafts: database
                .documents
                .values()
                .chain(std::iter::once(&database.document))
                .filter(|document| document.has_drafts())
                .count(),
            closing: database.closing,
        }
    }
}

impl App {
    pub(super) fn open_databases_ui(&mut self, ui: &mut egui::Ui) {
        let mut rows: Vec<_> = std::iter::once(&self.database)
            .chain(self.parked.values())
            .filter(|database| database.session.is_some())
            .map(DatabaseRow::from)
            .collect();
        rows.sort_by_key(|row| row.id);
        ui.horizontal(|ui| {
            ui.small("OPEN DATABASES");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.small(format!("{}/{}", rows.len(), super::sessions::MAX_DATABASES))
                    .on_hover_text(
                        "Up to four database workspaces; access review reserves a slot.",
                    );
            });
        });
        let enabled = self.picker.is_none() && !self.database.confirm_close && !self.quit;
        let mut select = None;
        let mut close = None;
        for row in rows {
            ui.push_id(("database-row", row.id), |ui| {
                ui.horizontal(|ui| {
                    let width =
                        (ui.available_width() - 28.0 - ui.spacing().item_spacing.x).max(40.0);
                    let name = Path::new(&row.path)
                        .file_name()
                        .unwrap_or_default()
                        .to_string_lossy();
                    let detail = if row.closing {
                        "Closing…".into()
                    } else if row.transaction != TransactionState::Autocommit || row.drafts > 0 {
                        format!(
                            "{}{}{}",
                            if row.transaction == TransactionState::Unknown {
                                "Unknown transaction"
                            } else if row.transaction == TransactionState::Active {
                                "Transaction"
                            } else {
                                ""
                            },
                            if row.transaction != TransactionState::Autocommit && row.drafts > 0 {
                                " · "
                            } else {
                                ""
                            },
                            if row.drafts > 0 {
                                format!("{} staged documents", row.drafts)
                            } else {
                                String::new()
                            }
                        )
                    } else {
                        Path::new(&row.path)
                            .parent()
                            .unwrap_or(Path::new("."))
                            .display()
                            .to_string()
                    };
                    let response = ui
                        .add_enabled_ui(enabled, |ui| {
                            ui.add_sized(
                                [width, 44.0],
                                egui::Button::new(
                                    egui::RichText::new(format!(
                                        "{}  {}\n{}",
                                        name,
                                        if row.writable { "RW" } else { "RO" },
                                        detail
                                    ))
                                    .size(12.0),
                                )
                                .selected(row.id == self.database.id)
                                .truncate(),
                            )
                        })
                        .inner;
                    response.widget_info(|| {
                        egui::WidgetInfo::selected(
                            egui::Role::Button,
                            response.enabled(),
                            row.id == self.database.id,
                            format!("Select database: {}", row.path),
                        )
                    });
                    if response.on_hover_text(&row.path).clicked() {
                        select = Some(row.id);
                    }
                    ui.add_enabled_ui(enabled && !row.closing, |ui| {
                        if icons::button(
                            ui,
                            Icon::Close,
                            &format!("Close database: {}", row.path),
                            "",
                        )
                        .clicked()
                        {
                            close = Some(row.id);
                        }
                    });
                });
            });
        }
        if let Some(id) = select {
            self.select_workspace(id);
        }
        if let Some(id) = close {
            self.select_workspace(id);
            if self.database.id == id {
                self.request_close();
            }
        }
    }
}
