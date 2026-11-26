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
    /// Open the field in the value view.
    open: bool,
}
// Fields of `blob_fields`, in table order.
const AVATAR: usize = 3;
const EMBEDDING: usize = 5;
const META: usize = 6;
const PAYLOAD: usize = 7;
const SESSION: usize = 8;
const NOTE: usize = 9;
const PHOTO: usize = 10;
const WIDE: [f32; 2] = [1800.0, 1000.0];
const LAPTOP: [f32; 2] = [1366.0, 768.0];
const SCENES: &[Scene] = &[
    Scene {
        name: "blob-fields.png",
        size: WIDE,
        table: "blob_fields",
        field: None,
        open: false,
    },
    Scene {
        name: "blob-fields-laptop.png",
        size: LAPTOP,
        table: "blob_fields",
        field: None,
        open: false,
    },
    Scene {
        name: "blob-hex-preview.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(SESSION),
        open: false,
    },
    Scene {
        name: "blob-hex-preview-laptop.png",
        size: LAPTOP,
        table: "blob_fields",
        field: Some(SESSION),
        open: false,
    },
    Scene {
        name: "blob-tree-preview.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(META),
        open: false,
    },
    Scene {
        name: "blob-text-preview.png",
        size: LAPTOP,
        table: "blob_fields",
        field: Some(NOTE),
        open: false,
    },
    Scene {
        name: "blob-value-hex.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(SESSION),
        open: true,
    },
    Scene {
        name: "blob-value-hex-laptop.png",
        size: LAPTOP,
        table: "blob_fields",
        field: Some(SESSION),
        open: true,
    },
    Scene {
        name: "blob-value-tree.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(META),
        open: true,
    },
    Scene {
        name: "blob-image-preview.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(AVATAR),
        open: false,
    },
    Scene {
        name: "blob-value-image.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(PHOTO),
        open: true,
    },
    Scene {
        name: "blob-value-contents.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(PAYLOAD),
        open: true,
    },
    Scene {
        name: "blob-vector-preview.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(EMBEDDING),
        open: false,
    },
    Scene {
        name: "blob-value-vector.png",
        size: WIDE,
        table: "blob_fields",
        field: Some(EMBEDDING),
        open: true,
    },
    Scene {
        name: "blob-grid.png",
        size: WIDE,
        table: "blobs",
        field: None,
        open: false,
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
                self.database.document.tools.record_scroll_to = Some(field);
                self.close_value();
                if SCENES[smoke.scene].open {
                    self.open_value(0, field);
                }
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
