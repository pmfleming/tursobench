use super::{App, shell};
use eframe::egui;
use tursobench_core::{ErrorKind, Value, quote_identifier};

use std::time::{Duration, Instant};
mod readme;

pub(super) struct Smoke {
    stage: u8,
    capture_dir: Option<std::path::PathBuf>,
    capture_pending: Option<String>,
    capture_result: Option<std::sync::mpsc::Receiver<std::sync::Arc<egui::ColorImage>>>,
    capture_frames: u8,
    start: Instant,
    table: String,
    project_fixture: bool,
    scene: usize,
    readme: bool,
}
impl Smoke {
    pub(super) fn new() -> Self {
        let table = std::env::args().nth(3);
        Self {
            stage: 0,
            capture_dir: std::env::var_os("TURSOBENCH_CAPTURE_DIR").map(Into::into),
            capture_pending: None,
            capture_result: None,
            capture_frames: 0,
            start: Instant::now(),
            project_fixture: table.is_some(),
            scene: 0,
            readme: std::env::var_os("TURSOBENCH_README_CAPTURE").is_some(),
            table: table.unwrap_or_else(|| "items".into()),
        }
    }
}
impl App {
    pub(super) fn smoke_tick(&mut self, ctx: &egui::Context) {
        let Some(mut smoke) = self.smoke.take() else {
            return;
        };
        if let (Some(dir), Some(name)) = (&smoke.capture_dir, &smoke.capture_pending) {
            let screenshot = smoke
                .capture_result
                .as_ref()
                .and_then(|rx| rx.try_recv().ok());
            if let Some(image) = screenshot {
                use eframe::icon_data::IconDataExt;
                let icon = egui::IconData {
                    rgba: image.pixels.iter().flat_map(|c| c.to_array()).collect(),
                    width: image.size[0] as u32,
                    height: image.size[1] as u32,
                };
                std::fs::create_dir_all(dir).expect("capture directory");
                std::fs::write(
                    dir.join(name),
                    icon.to_png_bytes().expect("encode screenshot"),
                )
                .expect("write screenshot");
                smoke.capture_pending = None;
                smoke.capture_result = None;
            }
        }
        if smoke.start.elapsed()
            > Duration::from_secs(if smoke.capture_dir.is_some() { 60 } else { 20 })
        {
            eprintln!(
                "DESKTOP SMOKE FAILED at stage {}: {}",
                smoke.stage, self.status
            );
            std::process::exit(1);
        }
        if matches!(smoke.stage, 50..=56) && self.smoke_readme(ctx, &mut smoke) {
            return;
        }
        match smoke.stage {
            0 => {
                self.writable = false;
                // The desktop smoke runner intentionally uses a closed SQLite
                // compatibility fixture and opts into opening it with Turso.
                if self.databases.inspection.kind(&self.path)
                    == crate::discovery::DatabaseKind::LikelySqlite
                {
                    self.databases.inspection.override_sqlite = true;
                }
                self.open(ctx);
                if self.session.is_some() {
                    smoke.stage = 1;
                }
            }
            1 if self.ready
                && self.document.active.is_none()
                && self.catalog_ticket.is_none()
                && self.ui.diagram.ticket.is_none() =>
            {
                assert_eq!(self.document.ui.nav, shell::Nav::Overview);
                assert!(self.ui.diagram.data.is_some());
                self.browse_table(smoke.table.clone());
                smoke.stage = 2;
            }
            2 if self.ready && self.document.active.is_none() => {
                assert!(
                    self.document
                        .result
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.error.is_none()),
                    "{}",
                    self.status
                );
                if smoke.project_fixture {
                    assert!(
                        !self.document.result.rows.is_empty(),
                        "project fixture table must contain rows"
                    );
                    assert!(
                        self.document
                            .table
                            .as_ref()
                            .is_some_and(|t| t.columns.len() == self.document.result.columns.len())
                    );
                } else {
                    assert_eq!(
                        self.document.result.rows.len(),
                        100,
                        "smoke fixture needs at least 100 rows"
                    );
                    assert_eq!(self.document.result.rows[0][0], Value::Integer(0));
                }
                smoke.stage = 9;
            }
            9 => {
                assert_eq!(self.document.ui.nav, shell::Nav::Table);
                self.document.ui.table_view = shell::TableView::Structure;
                smoke.stage = 10;
            }
            10 => {
                self.document.ui.table_view = shell::TableView::Data;
                self.document.ui.nav = shell::Nav::Overview;
                self.show_structure();
                self.load_diagram();
                smoke.stage = 7;
            }
            7 if self.ui.diagram.ticket.is_none() => {
                let graph = self.ui.diagram.data.as_ref().expect("diagram metadata");
                assert!(!graph.limited);
                assert!(graph.tables.iter().any(|t| t.name == smoke.table));
                assert!(graph.tables.iter().all(|t| t.error.is_none()));
                self.ui.diagram.selected = Some(smoke.table.clone());
                if smoke.capture_dir.is_some() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        1800.0, 1000.0,
                    )));
                    self.browse_table(smoke.table.clone());
                    self.retile();
                    self.preferences.appearance = Some(crate::appearance::Mode::Dark);
                    if smoke.readme {
                        self.start_readme_capture();
                        smoke.stage = 50;
                    } else {
                        smoke.stage = 20;
                    }
                } else {
                    smoke.stage = 8;
                }
            }
            20..=31 => self.smoke_capture_layout(ctx, &mut smoke),
            8 => {
                self.document.sql = "SELECT ?1 AS typed_blob".into();
                self.document.params = "[{\"type\":\"Blob\",\"value\":[0,255]}]".into();
                self.run_sql();
                smoke.stage = 3;
            }
            3 if self.document.active.is_none() => {
                assert_eq!(
                    self.document.result.rows,
                    vec![vec![Value::Blob(vec![0, 255])]]
                );
                if smoke.table == "search_content" {
                    self.document.sql =
                        "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)"
                            .into();
                    self.document.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
                    self.run_sql();
                    smoke.stage = 6;
                } else {
                    self.smoke_readonly_write(&smoke.table);
                    smoke.stage = 4;
                }
            }
            6 if self.document.active.is_none() => {
                assert_eq!(
                    self.document.result.rows,
                    vec![vec![Value::Text("entry-0000".into())]]
                );
                self.smoke_readonly_write(&smoke.table);
                smoke.stage = 4;
            }
            4 if self.document.active.is_none() => {
                assert_eq!(
                    self.document
                        .result
                        .completion
                        .as_ref()
                        .unwrap()
                        .error
                        .as_ref()
                        .unwrap()
                        .kind,
                    ErrorKind::ReadOnly
                );
                let column = self
                    .ui
                    .diagram
                    .data
                    .as_ref()
                    .unwrap()
                    .tables
                    .iter()
                    .find(|t| t.name == smoke.table)
                    .unwrap()
                    .inspection
                    .as_ref()
                    .unwrap()
                    .table
                    .columns[0]
                    .name
                    .clone();
                self.run_profile(tursobench_db::profile::Request {
                    table: smoke.table.clone(),
                    column: Some(column),
                    sample_rows: Some(100),
                });
                smoke.stage = 60;
            }
            60 if self.document.active.is_none() => {
                assert!(
                    self.document
                        .result
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.error.is_none()),
                    "{}",
                    self.status
                );
                assert!(self.document.profile.is_some());
                assert!(!self.document.result.rows.is_empty());
                // Render the profile before closing on the following frame.
                smoke.stage = 61;
            }
            61 => {
                self.submit(
                    tursobench_db::Operation::Admin(None),
                    super::View::Diagnostics,
                );
                smoke.stage = 62;
            }
            62 if self.document.active.is_none() => {
                assert!(
                    self.document
                        .result
                        .rows
                        .iter()
                        .any(|r| r[0] == Value::Text("Main file bytes".into()))
                );
                smoke.stage = 63;
            }
            63 => {
                self.close();
                smoke.stage = 5;
            }
            5 if self.session.is_none() => {
                assert!(self.status.contains("cleanup confirmed"), "{}", self.status);
                if smoke.capture_dir.is_some() {
                    // Capture-only pass uses the disposable fixture with real writable
                    // access. Stage locally to exercise edit surfaces; never commit.
                    self.writable = true;

                    self.open(ctx);
                    smoke.stage = 40;
                } else {
                    eprintln!(
                        "DESKTOP SMOKE PASSED: open, schema, page, diagram, typed query, read-only rejection, profile, admin, close; elapsed {:?}",
                        smoke.start.elapsed()
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
            }
            40 if self.ready
                && self.catalog_ticket.is_none()
                && self.ui.diagram.ticket.is_none() =>
            {
                self.browse_table(smoke.table.clone());
                smoke.stage = 41;
            }
            41 if self.document.active.is_none()
                && self.document.tools.inspection_ticket.is_none() =>
            {
                self.retile();
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1366.0, 768.0)));
                if self.can_edit() && !self.document.result.rows.is_empty() {
                    let col = usize::from(self.document.result.columns.len() > 1);
                    self.stage_cell(
                        0,
                        col,
                        Value::Text("Staged locally for visual review".into()),
                    );
                }
                let sql = format!("SELECT * FROM {} LIMIT 100", quote_identifier(&smoke.table));
                self.preferences
                    .record_query(&self.tools.database_scope, &sql, Some(12), false);
                self.preferences
                    .bookmark(&self.tools.database_scope, "Sample table", &sql);
                self.prepare_visual_scene(0);
                smoke.stage = 42;
            }
            42 => {
                smoke.capture_frames += 1;
                if smoke.capture_frames >= 8 {
                    smoke.capture_pending = Some(VISUAL_SCENES[smoke.scene].into());
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    let wake = ctx.clone();
                    ctx.request_screenshot(move |image| {
                        let _ = tx.try_send(image);
                        wake.request_repaint();
                    });
                    smoke.capture_result = Some(rx);
                    smoke.capture_frames = 0;
                    smoke.stage = 43;
                }
            }
            43 if smoke.capture_pending.is_none() => {
                smoke.scene += 1;
                if smoke.scene == 14 {
                    self.prepare_visual_scene(14);
                    self.close();
                    smoke.stage = 44;
                } else if smoke.scene == VISUAL_SCENES.len() {
                    assert!(self.session.is_none());
                    eprintln!(
                        "DESKTOP VISUAL SMOKE PASSED: six workspace sizes/themes, navigator tools, staged edits, dialogs, chooser and search settings; elapsed {:?}",
                        smoke.start.elapsed()
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                } else {
                    self.prepare_visual_scene(smoke.scene);
                    smoke.stage = 42;
                }
            }
            44 if self.session.is_none() => {
                self.prepare_visual_scene(14);
                smoke.stage = 42;
            }
            _ => {}
        }
        self.smoke = Some(smoke);
        // Automated smoke mode only; normal rendering is event-driven.
        ctx.request_repaint_after(Duration::from_millis(25));
    }
    fn prepare_visual_scene(&mut self, scene: usize) {
        self.preferences.appearance = Some(crate::appearance::Mode::Dark);
        self.ui.sidebar = shell::ToolPanel::Schema;
        self.ui.palette_open = false;
        self.ui.export_open = false;
        self.ui.outcomes = false;
        self.ui.access_dialog = false;
        self.confirm_close = false;
        self.tools.help = false;
        self.document.show_preview = false;
        self.document.show_insert = false;
        self.document.ui.inspector_detached = false;
        self.document.ui.inspector_open = false;
        self.databases.settings_open = false;
        match scene {
            1 => self.document.show_preview = true,
            2 => self.begin_insert(),
            3 => self.ui.sidebar = shell::ToolPanel::History,
            4 => self.ui.sidebar = shell::ToolPanel::Saved,
            5 => self.ui.sidebar = shell::ToolPanel::Diagnostics,
            6 => self.ui.sidebar = shell::ToolPanel::Settings,
            7 => {
                self.ui.palette_open = true;
                self.ui.palette_query.clear();
            }
            8 => self.ui.export_open = true,
            9 => self.ui.outcomes = true,
            10 => self.tools.help = true,
            11 => {
                self.ui.access_dialog = true;
            }
            12 => {
                self.document.inspect = Some((0, 0));
                self.document.ui.inspector_detached = true;
                self.document.ui.inspector_open = true;
                self.document.ui.inspector = shell::DetailMode::Cell;
            }
            13 => self.confirm_close = true,
            14 => self.document.ui.nav = shell::Nav::Databases,
            15 => self.databases.settings_open = true,
            _ => {}
        }
    }
    fn smoke_capture_layout(&mut self, ctx: &egui::Context, smoke: &mut Smoke) {
        match smoke.stage {
            26 if self.document.active.is_some()
                || self.document.tools.inspection_ticket.is_some() => {}
            20 | 22 | 24 | 26 | 28 | 30 => {
                if smoke.capture_frames == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(egui::pos2(
                        1795.0, 995.0,
                    )));
                }
                smoke.capture_frames += 1;
                if smoke.capture_frames >= 15 {
                    smoke.capture_pending = Some(
                        match smoke.stage {
                            20 => "workspace-dark.png",
                            22 => "expanded-map.png",
                            24 => "laptop-light.png",
                            26 => "laptop-dark.png",
                            28 => "minimized-tiles.png",
                            _ => "widescreen.png",
                        }
                        .into(),
                    );
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    let wake = ctx.clone();
                    ctx.request_screenshot(move |image| {
                        let _ = tx.try_send(image);
                        wake.request_repaint();
                    });
                    smoke.capture_result = Some(rx);
                    smoke.capture_frames = 0;
                    smoke.stage += 1;
                }
            }
            21 if smoke.capture_pending.is_none() => {
                self.overview();
                self.expand_window(self.document.id);
                smoke.stage = 22;
            }
            23 if smoke.capture_pending.is_none() => {
                self.expand_window(self.document.id);
                self.browse_table(smoke.table.clone());
                self.preferences.appearance = Some(crate::appearance::Mode::Light);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1366.0, 768.0)));
                smoke.stage = 24;
            }
            25 if smoke.capture_pending.is_none() => {
                self.preferences.appearance = Some(crate::appearance::Mode::Dark);
                smoke.stage = 26;
            }
            27 if smoke.capture_pending.is_none() => {
                for kind in [
                    super::tiling::Kind::Record,
                    super::tiling::Kind::Structure,
                    super::tiling::Kind::Sql,
                ] {
                    self.minimize_pane(kind);
                }
                smoke.stage = 28;
            }
            29 if smoke.capture_pending.is_none() => {
                self.retile();
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(3440.0, 1440.0)));
                smoke.stage = 30;
            }
            31 if smoke.capture_pending.is_none() => {
                smoke.stage = 8;
            }
            _ => {}
        }
    }
    fn smoke_readonly_write(&mut self, table: &str) {
        self.document.sql = format!("DELETE FROM {}", quote_identifier(table));
        self.document.params = "[]".into();
        self.run_sql();
    }
}

// Additional captures are opt-in and operate only on the smoke fixture.
const VISUAL_SCENES: &[&str] = &[
    "staged-workspace.png",
    "review-dialog.png",
    "insert-dialog.png",
    "navigator-history.png",
    "navigator-saved.png",
    "navigator-checks.png",
    "navigator-settings.png",
    "command-palette.png",
    "export-dialog.png",
    "execution-outcomes.png",
    "keyboard-reference.png",
    "access-dialog.png",
    "cell-detail.png",
    "close-dialog.png",
    "database-chooser.png",
    "search-settings.png",
];
