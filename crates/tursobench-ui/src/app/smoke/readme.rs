//! Opt-in documentation captures through the real application and worker.
use super::{App, Smoke, egui, shell};
use crate::app::tiling::Kind;

const SCENES: &[&str] = &[
    "workspace-dark.png",
    "table-data.png",
    "relationships.png",
    "structure.png",
    "sql-results.png",
    "edit-review.png",
    "database-chooser.png",
];

impl App {
    pub(super) fn start_readme_capture(&mut self) {
        self.create_window(Kind::Sql, None, None, false).unwrap();
        self.database.document.set_sql("SELECT d.edit_seq, d.now_id, d.tip_id,\n       s.bytes, length(s.digest) AS digest_bytes\nFROM document_history AS d\nJOIN snapshots AS s ON s.id = d.temp_id\nORDER BY s.bytes DESC\nLIMIT 12;".into());
        self.database.document.params = "[]".into();
        self.run_sql();
    }

    pub(super) fn smoke_readme(&mut self, ctx: &egui::Context, smoke: &mut Smoke) -> bool {
        match smoke.stage {
            50 if self.database.document.active.is_none()
                && self.database.documents.values().all(|d| d.active.is_none()) =>
            {
                self.retile();
                self.focus_pane(Kind::Data);
                smoke.stage = 51;
            }
            51 => {
                smoke.capture_frames += 1;
                if smoke.capture_frames >= 10 {
                    smoke.capture_pending = Some(SCENES[smoke.scene].into());
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    let wake = ctx.clone();
                    ctx.request_screenshot(move |image| {
                        let _ = tx.try_send(image);
                        wake.request_repaint();
                    });
                    smoke.capture_result = Some(rx);
                    smoke.capture_frames = 0;
                    smoke.stage = 52;
                }
            }
            52 if smoke.capture_pending.is_none() => {
                smoke.scene += 1;
                self.retile();
                match smoke.scene {
                    1 => self.expand_pane(Kind::Data),
                    2 => self.expand_pane(Kind::Map),
                    3 => self.expand_pane(Kind::Structure),
                    4 => self.expand_pane(Kind::Sql),
                    5 => {
                        self.close();
                        smoke.stage = 53;
                        return false;
                    }
                    6 => {
                        self.close();
                        smoke.stage = 56;
                        return false;
                    }
                    _ => {
                        assert!(self.database.session.is_none());
                        eprintln!(
                            "README CAPTURE PASSED: {} native Wayland scenes",
                            SCENES.len()
                        );
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        return true;
                    }
                }
                smoke.stage = 51;
            }
            53 if self.database.session.is_none() => {
                self.database.writable = true;
                self.open(ctx);
                smoke.stage = 54;
            }
            54 if self.database.ready
                && self.database.catalog_ticket.is_none()
                && self.database.ui.diagram.ticket.is_none() =>
            {
                self.browse_table(smoke.table.clone());
                self.load_diagram();
                smoke.stage = 55;
            }
            55 if self.database.document.active.is_none()
                && self.database.document.tools.inspection_ticket.is_none()
                && self.database.ui.diagram.ticket.is_none() =>
            {
                self.focus_pane(Kind::Data);
                assert!(!self.database.document.result.rows.is_empty());
                let column = self
                    .database
                    .document
                    .result
                    .columns
                    .iter()
                    .position(|c| c.name == "edit_seq")
                    .unwrap();
                let tursobench_core::Value::Integer(value) =
                    self.database.document.result.rows[0][column]
                else {
                    panic!("The session capture requires integer edit_seq metadata");
                };
                self.stage_cell(
                    0,
                    column,
                    tursobench_core::Value::Integer(value.saturating_add(1)),
                );
                assert!(self.has_drafts());
                self.database.document.show_preview = true;
                smoke.stage = 51;
            }
            56 if self.database.session.is_none() => {
                self.database.writable = false;
                self.database.document.ui.nav = shell::Nav::Databases;
                smoke.stage = 51;
            }
            _ => {}
        }
        false
    }
}
