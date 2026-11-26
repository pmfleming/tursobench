//! Opt-in captures of the record tile reading blobs, through the real
//! application and worker, from the `blob_fields` and `blobs` tables
//! `tests/fixtures/generate.py` writes. `scripts/capture-blob-viewer.sh` runs them.
use super::{App, Smoke, egui};
use crate::app::{tiling::Kind, value::Lens};

/// A capture: its file, the window size, the table browsed, the record field
/// selected, whether it opens in the value view, through which lens, and
/// whether the record tile fills the workspace.
struct Scene {
    name: &'static str,
    size: [f32; 2],
    table: &'static str,
    field: usize,
    open: bool,
    lens: Option<Lens>,
    expand: bool,
}
const WIDE: [f32; 2] = [1800.0, 1000.0];
const LAPTOP: [f32; 2] = [1366.0, 768.0];
// Fields of `blob_fields`, in table order.
const AVATAR: usize = 3;
const EMBEDDING: usize = 5;
const META: usize = 6;
const PAYLOAD: usize = 7;
const SESSION: usize = 8;
const NOTE: usize = 9;
const PHOTO: usize = 10;
const PLACE: usize = 11;

const fn scene(name: &'static str, size: [f32; 2], field: usize, open: bool) -> Scene {
    Scene {
        name,
        size,
        table: "blob_fields",
        field,
        open,
        lens: None,
        expand: false,
    }
}
const SCENES: &[Scene] = &[
    scene("blob-fields.png", WIDE, 0, false),
    scene("blob-fields-laptop.png", LAPTOP, 0, false),
    scene("blob-hex-preview.png", WIDE, SESSION, false),
    scene("blob-hex-preview-laptop.png", LAPTOP, SESSION, false),
    scene("blob-tree-preview.png", WIDE, META, false),
    scene("blob-text-preview-laptop.png", LAPTOP, NOTE, false),
    scene("blob-image-preview.png", WIDE, AVATAR, false),
    scene("blob-vector-preview.png", WIDE, EMBEDDING, false),
    scene("blob-geometry-preview.png", WIDE, PLACE, false),
    Scene {
        lens: Some(Lens::Hex),
        ..scene("blob-value-hex.png", WIDE, PAYLOAD, true)
    },
    Scene {
        lens: Some(Lens::Hex),
        ..scene("blob-value-hex-laptop.png", LAPTOP, PAYLOAD, true)
    },
    Scene {
        lens: Some(Lens::Hex),
        expand: true,
        ..scene("blob-value-expanded.png", WIDE, PAYLOAD, true)
    },
    scene("blob-value-contents.png", WIDE, PAYLOAD, true),
    scene("blob-value-tree.png", WIDE, META, true),
    scene("blob-value-image.png", WIDE, PHOTO, true),
    scene("blob-value-vector.png", WIDE, EMBEDDING, true),
    Scene {
        table: "blobs",
        ..scene("blob-grid.png", WIDE, 0, false)
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
    /// Select the scene's field, through its lens, opened or previewed.
    fn blob_field(&mut self, scene: &Scene) {
        self.close_value();
        let lenses = &mut self.database.document.tools.columns.lenses;
        match scene.lens {
            Some(lens) => lenses.insert(scene.field, lens),
            None => lenses.remove(&scene.field),
        };
        self.select_cell(0, scene.field, false);
        self.database.document.tools.record_scroll_to = Some(scene.field);
        if scene.open {
            self.open_value(0, scene.field);
        }
        if scene.expand {
            self.expand_pane(Kind::Record);
        }
    }
    pub(super) fn smoke_blobs(&mut self, ctx: &egui::Context, smoke: &mut Smoke) -> bool {
        match smoke.stage {
            80 if self.smoke_idle() => {
                self.blob_scene(ctx, 0);
                smoke.stage = 81;
            }
            // Select the field once its table has loaded, then capture.
            81 if self.smoke_idle() => {
                self.blob_field(&SCENES[smoke.scene]);
                smoke.stage = 82;
            }
            82 => smoke.capture_after(ctx, 20, SCENES[smoke.scene].name, 83),
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
