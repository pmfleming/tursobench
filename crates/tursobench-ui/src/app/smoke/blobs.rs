//! Opt-in captures of the record tile reading blobs, through the real
//! application and worker, from the `blob_fields` and `blobs` tables
//! `tests/fixtures/generate.py` writes.
use super::{App, Smoke, egui};
use crate::app::tiling::Kind;

/// A capture: its file, the window size, the table browsed and the record
/// field selected, if any.
struct Scene {
    name: &'static str,
    size: [f32; 2],
    table: &'static str,
    field: Option<usize>,
}
const WIDE: [f32; 2] = [1800.0, 1000.0];
const LAPTOP: [f32; 2] = [1366.0, 768.0];
const SCENES: &[Scene] = &[
    Scene {
        name: "blob-fields.png",
        size: WIDE,
        table: "blob_fields",
        field: None,
    },
    Scene {
        name: "blob-fields-laptop.png",
        size: LAPTOP,
        table: "blob_fields",
        field: None,
    },
    Scene {
        name: "blob-grid.png",
        size: WIDE,
        table: "blobs",
        field: None,
    },
];

impl App {
    /// Whether every document has finished loading.
    fn smoke_idle(&self) -> bool {
        self.database.document.active.is_none()
            && self.database.documents.values().all(|d| d.active.is_none())
    }
    /// Size the window and browse the scene's table.
    fn blob_scene(&mut self, ctx: &egui::Context, index: usize) {
        let scene = &SCENES[index];
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(scene.size.into()));
        self.retile();
        self.focus_pane(Kind::Data);
        self.browse_table(scene.table.into());
    }
    pub(super) fn smoke_blobs(&mut self, ctx: &egui::Context, smoke: &mut Smoke) -> bool {
        match smoke.stage {
            80 if self.smoke_idle() => {
                self.blob_scene(ctx, 0);
                smoke.stage = 81;
            }
            // Select the field once its table has loaded, then capture.
            81 if self.smoke_idle() => {
                let field = SCENES[smoke.scene].field.unwrap_or(0);
                self.select_cell(0, field, false);
                smoke.stage = 82;
            }
            82 => smoke.capture_after(ctx, 15, SCENES[smoke.scene].name, 83),
            83 if smoke.capture_pending.is_none() => {
                smoke.scene += 1;
                if smoke.scene == SCENES.len() {
                    eprintln!(
                        "BLOB CAPTURE PASSED: {} native Wayland scenes",
                        SCENES.len()
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return true;
                }
                self.blob_scene(ctx, smoke.scene);
                smoke.stage = 81;
            }
            _ => {}
        }
        false
    }
}
