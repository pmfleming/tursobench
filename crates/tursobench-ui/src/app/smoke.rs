use super::{App, shell};
use eframe::egui;
use tursobench_core::{ErrorKind, Value, quote_identifier};

use std::time::{Duration, Instant};
mod assistant;
mod readme;

pub(super) struct Smoke {
    stage: u8,
    first_workspace: Option<u64>,
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
            first_workspace: None,
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
impl Smoke {
    /// Count a frame; once `frames` have drawn, screenshot the window to `name`
    /// and go on to stage `next`, which waits for the screenshot to be written.
    pub(super) fn capture_after(&mut self, ctx: &egui::Context, frames: u8, name: &str, next: u8) {
        self.capture_frames += 1;
        if self.capture_frames < frames {
            return;
        }
        self.capture_pending = Some(name.into());
        let (tx, rx) = std::sync::mpsc::sync_channel(1);
        let wake = ctx.clone();
        ctx.request_screenshot(move |image| {
            let _ = tx.try_send(image);
            wake.request_repaint();
        });
        self.capture_result = Some(rx);
        self.capture_frames = 0;
        self.stage = next;
    }
    /// Write a requested screenshot once it arrives.
    fn save_capture(&mut self) {
        let (Some(dir), Some(name)) = (&self.capture_dir, &self.capture_pending) else {
            return;
        };
        let Some(image) = self
            .capture_result
            .as_ref()
            .and_then(|rx| rx.try_recv().ok())
        else {
            return;
        };
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
        self.capture_pending = None;
        self.capture_result = None;
    }
}
impl App {
    pub(super) fn smoke_tick(&mut self, ctx: &egui::Context) {
        let Some(mut smoke) = self.smoke.take() else {
            return;
        };
        smoke.save_capture();
        if smoke.start.elapsed()
            > Duration::from_secs(if smoke.capture_dir.is_some() { 60 } else { 20 })
        {
            eprintln!(
                "DESKTOP SMOKE FAILED at stage {}: {}",
                smoke.stage, self.database.status
            );
            std::process::exit(1);
        }
        if matches!(smoke.stage, 50..=56) && self.smoke_readme(ctx, &mut smoke) {
            return;
        }
        if matches!(smoke.stage, 90..=93) && self.smoke_assistant(ctx, &mut smoke) {
            return;
        }
        let stage = smoke.stage;
        match stage {
            0..=2 | 7 | 9 | 10 => self.smoke_startup(ctx, &mut smoke),
            3 | 4 | 6 | 8 => self.smoke_queries(&mut smoke),
            20..=31 => self.smoke_capture_workspace(ctx, &mut smoke),
            39..=45 if self.smoke_capture_scenes(ctx, &mut smoke) => return,
            60..=74 => self.smoke_workbench(ctx, &mut smoke),
            5 if self.database.session.is_none() => {
                assert!(
                    self.database.status.contains("cleanup confirmed"),
                    "{}",
                    self.database.status
                );
                if smoke.capture_dir.is_some() {
                    // Capture-only pass uses the disposable fixture with real writable
                    // access. Stage locally to exercise edit surfaces; never commit.
                    self.database.writable = true;
                    smoke.stage = 39;
                } else {
                    eprintln!(
                        "DESKTOP SMOKE PASSED: open, schema, page, diagram, typed query, read-only rejection, profile, admin, database cycling, close; elapsed {:?}",
                        smoke.start.elapsed()
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return;
                }
            }
            _ => {}
        }
        self.smoke = Some(smoke);
        // Automated smoke mode only; normal rendering is event-driven.
        ctx.request_repaint_after(Duration::from_millis(25));
    }
    /// Open the fixture, browse its first page, show Structure and the map.
    fn smoke_startup(&mut self, ctx: &egui::Context, smoke: &mut Smoke) {
        match smoke.stage {
            0 => {
                self.database.writable = false;
                // The desktop smoke runner intentionally uses a closed SQLite
                // compatibility fixture and opts into opening it with Turso.
                if self.smoke_open(ctx) {
                    smoke.stage = 1;
                }
            }
            1 if self.smoke_loaded() && self.database.document.active.is_none() => {
                assert_eq!(self.database.document.ui.nav, shell::Nav::Overview);
                assert!(self.database.ui.diagram.data.is_some());
                self.browse_table(smoke.table.clone());
                smoke.stage = 2;
            }
            2 if self.database.ready && self.database.document.active.is_none() => {
                self.assert_first_page(smoke.project_fixture);
                smoke.stage = 9;
            }
            9 => {
                assert_eq!(self.database.document.ui.nav, shell::Nav::Table);
                self.database.document.ui.table_view = shell::TableView::Structure;
                smoke.stage = 10;
            }
            10 => {
                self.database.document.ui.table_view = shell::TableView::Data;
                self.database.document.ui.nav = shell::Nav::Overview;
                self.show_structure();
                self.load_diagram();
                smoke.stage = 7;
            }
            7 if self.database.ui.diagram.ticket.is_none() => {
                self.select_diagram_table(&smoke.table);
                smoke.stage = if smoke.capture_dir.is_some() {
                    self.start_captures(ctx, smoke)
                } else {
                    8
                };
            }
            _ => {}
        }
    }
    /// A typed parameter, full-text search, a rejected read-only write and a profile.
    fn smoke_queries(&mut self, smoke: &mut Smoke) {
        match smoke.stage {
            8 => {
                self.database
                    .document
                    .set_sql("SELECT ?1 AS typed_blob".into());
                self.database.document.params = "[{\"type\":\"Blob\",\"value\":[0,255]}]".into();
                self.run_sql();
                smoke.stage = 3;
            }
            3 if self.database.document.active.is_none() => {
                assert_eq!(
                    self.database.document.result.rows,
                    vec![vec![Value::Blob(vec![0, 255])]]
                );
                if smoke.table == "search_content" {
                    self.database.document.set_sql(
                        "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)"
                            .into(),
                    );
                    self.database.document.params =
                        "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
                    self.run_sql();
                    smoke.stage = 6;
                } else {
                    self.smoke_readonly_write(&smoke.table);
                    smoke.stage = 4;
                }
            }
            6 if self.database.document.active.is_none() => {
                assert_eq!(
                    self.database.document.result.rows,
                    vec![vec![Value::Text("entry-0000".into())]]
                );
                self.smoke_readonly_write(&smoke.table);
                smoke.stage = 4;
            }
            4 if self.database.document.active.is_none() => {
                let completion = self.database.document.result.completion.as_ref();
                let error = completion.and_then(|c| c.error.as_ref());
                assert_eq!(error.map(|e| &e.kind), Some(&ErrorKind::ReadOnly));
                let column = self.diagram_columns(&smoke.table)[0].name.clone();
                self.run_profile(tursobench_db::profile::Request {
                    table: smoke.table.clone(),
                    column: Some(column),
                    sample_rows: Some(100),
                });
                smoke.stage = 60;
            }
            _ => {}
        }
    }
    fn smoke_workbench(&mut self, ctx: &egui::Context, smoke: &mut Smoke) {
        match smoke.stage {
            60 if self.database.document.active.is_none() => {
                self.assert_query_succeeded();
                assert!(self.database.document.profile.is_some());
                assert!(!self.database.document.result.rows.is_empty());
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
            62 if self.database.document.active.is_none() => {
                assert!(
                    self.database
                        .document
                        .result
                        .rows
                        .iter()
                        .any(|r| r[0] == Value::Text("Main file bytes".into()))
                );
                smoke.stage = 63;
            }
            63 => {
                if let Some(path) = std::env::var_os("TURSOBENCH_SECOND_DB") {
                    smoke.first_workspace = Some(self.database.id);
                    self.request_database(ctx, path.into());
                    smoke.stage = 64;
                } else {
                    self.close();
                    smoke.stage = 5;
                }
            }
            64 if self.smoke_open(ctx) => smoke.stage = 65,
            65 if self.smoke_loaded() => {
                self.run_text("SELECT count(*) FROM items".into(), None);
                smoke.stage = 66;
            }
            66 if self.database.document.active.is_none() => {
                assert_eq!(
                    self.database.document.result.rows,
                    vec![vec![Value::Integer(100)]]
                );
                let second = self.database.id;
                self.cycle_database(-1);
                assert_eq!(self.database.id, smoke.first_workspace.unwrap());
                assert!(self.database.ready);
                self.cycle_database(1);
                assert_eq!(self.database.id, second);
                self.comparison.open = true;
                self.comparison.source = smoke.first_workspace.unwrap();
                self.comparison.target = second;
                self.capture_schema(self.comparison.source);
                self.capture_schema(second);
                smoke.stage = 69;
            }
            69 if self.comparison_ready() => {
                if smoke.capture_dir.is_some() && !smoke.readme {
                    self.comparison.open = false;
                    self.sidebar_search.query = "name".into();
                    self.sidebar_search.scope = super::database_navigation::Scope::AllOpen;
                    smoke.capture_frames = 0;
                    smoke.stage = 71;
                } else {
                    smoke.stage = 70;
                }
            }
            71 | 73 => {
                let name = if smoke.stage == 71 {
                    "sidebar-search.png"
                } else {
                    "sidebar-open-databases.png"
                };
                smoke.capture_after(ctx, 8, name, smoke.stage + 1);
            }
            72 if smoke.capture_pending.is_none() => {
                self.sidebar_search.query.clear();
                smoke.stage = 73;
            }
            74 if smoke.capture_pending.is_none() => smoke.stage = 67,
            70 => {
                self.comparison.open = false;
                smoke.stage = 67;
            }
            67 => {
                self.close();
                smoke.stage = 68;
            }
            68 if self.database.session.is_none() => {
                self.select_workspace(smoke.first_workspace.unwrap());
                self.close();
                smoke.stage = 5;
            }
            _ => {}
        }
    }
    fn prepare_visual_scene(&mut self, ctx: &egui::Context, scene: usize) {
        self.sidebar_search.query.clear();
        self.set_dark(ctx, true);
        self.preferences.font_size = 14;
        self.database.ui.sidebar = shell::ToolPanel::Schema;
        self.database.ui.palette_open = false;
        self.database.ui.export_open = false;
        self.database.ui.outcomes = false;
        self.database.ui.access_dialog = false;
        self.database.confirm_close = false;
        self.database.tools.help = false;
        self.database.document.show_preview = false;
        self.database.document.show_insert = false;
        self.database.document.tools.spreadsheet.dialog = None;
        self.database.document.ui.inspector_detached = false;
        self.database.document.ui.inspector_open = false;
        self.settings.open = false;
        match scene {
            1 if std::env::var_os("TURSOBENCH_SPREADSHEET_CAPTURE").is_some() => {
                self.select_cell(0, 1, false);
                self.select_cell(2, 1, true);
                self.open_fill();
            }
            2 if std::env::var_os("TURSOBENCH_SPREADSHEET_CAPTURE").is_some() => {
                self.open_paste(
                    true,
                    "1001\tPasted task\tin progress\n1002\tAnother task\tdone".into(),
                );
                if let Some(super::spreadsheet::Dialog::Paste(mut paste)) =
                    self.database.document.tools.spreadsheet.dialog.take()
                {
                    self.preview_clipboard(&mut paste);
                    self.database.document.tools.spreadsheet.dialog =
                        Some(super::spreadsheet::Dialog::Paste(paste));
                }
            }
            1 => self.database.document.show_preview = true,
            2 => self.begin_insert(),
            3 => self.database.ui.sidebar = shell::ToolPanel::History,
            4 => self.database.ui.sidebar = shell::ToolPanel::Saved,
            5 => self.database.ui.sidebar = shell::ToolPanel::Diagnostics,
            6 => {
                self.settings.page = super::settings::Page::Appearance;
                self.settings.open = true;
            }
            7 => {
                self.database.ui.palette_open = true;
                self.database.ui.palette_query.clear();
            }
            8 => self.database.ui.export_open = true,
            9 => self.database.ui.outcomes = true,
            10 => self.database.tools.help = true,
            11 => {
                self.database.ui.access_dialog = true;
            }
            12 => {
                self.database.document.inspect = Some((0, 0));
                self.database.document.ui.inspector_detached = true;
                self.database.document.ui.inspector_open = true;
                self.database.document.ui.inspector = shell::DetailMode::Cell;
            }
            13 => self.database.confirm_close = true,
            14 => {
                self.database.document.ui.nav = shell::Nav::Databases;
                if self.database.session.is_none() {
                    self.database.writable = false;
                    self.databases.inspection.override_sqlite = false;
                }
            }
            15..=20 => {
                if scene == 15 {
                    // Display search feedback without traversing the user's folders.
                    self.preferences.discovery.roots =
                        ["/work/projects", "/srv/analytics", "/mnt/scratch/databases"]
                            .into_iter()
                            .map(Into::into)
                            .collect();
                    self.preferences.discovery.excluded = vec!["/work/projects/private".into()];
                    self.databases.summary = Some(crate::discovery::Summary {
                        probes: 300,
                        directories: 1000,
                        unavailable: 1,
                        skipped: 1,
                        limited: true,
                        ..Default::default()
                    });
                }
                self.settings.open = true;
                self.settings.page = match scene {
                    16 => super::settings::Page::Resources,
                    17 | 19 => super::settings::Page::Queries,
                    _ => super::settings::Page::Discovery,
                };
                self.databases.folder_list = if scene == 18 {
                    super::databases::FolderList::Excluded
                } else {
                    super::databases::FolderList::Search
                };
                if scene == 19 {
                    self.set_dark(ctx, false);
                }
                if scene == 20 {
                    self.preferences.font_size = 20;
                    ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                        1024.0, 600.0,
                    )));
                }
            }
            _ => {}
        }
    }
    /// Workspace captures at several sizes and themes: even stages capture,
    /// odd ones set up the next arrangement once the capture is written.
    fn smoke_capture_workspace(&mut self, ctx: &egui::Context, smoke: &mut Smoke) {
        match smoke.stage {
            26 if self.database.document.active.is_some()
                || self.database.document.tools.inspection_ticket.is_some() => {}
            20 | 22 | 24 | 26 | 28 | 30 => {
                if smoke.capture_frames == 0 {
                    if std::env::var("TURSOBENCH_CAPTURE_STRUCTURE_SECTION").as_deref()
                        == Ok("foreign-keys")
                    {
                        self.database.document.ui.structure_section =
                            shell::StructureSection::ForeignKeys;
                    }
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(egui::pos2(
                        1795.0, 995.0,
                    )));
                }
                let name = WORKSPACE_CAPTURES[usize::from(smoke.stage - 20) / 2];
                smoke.capture_after(ctx, 15, name, smoke.stage + 1);
            }
            21 if smoke.capture_pending.is_none() => {
                self.overview();
                self.expand_window(self.database.document.id);
                smoke.stage = 22;
            }
            23 if smoke.capture_pending.is_none() => {
                self.expand_window(self.database.document.id);
                self.browse_table(smoke.table.clone());
                self.set_dark(ctx, false);
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1366.0, 768.0)));
                smoke.stage = 24;
            }
            25 if smoke.capture_pending.is_none() => {
                self.set_dark(ctx, true);
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
    /// The visual scenes on a writable copy of the fixture: dialogs, navigator
    /// tools and settings pages, one capture each. Returns whether the smoke ended.
    fn smoke_capture_scenes(&mut self, ctx: &egui::Context, smoke: &mut Smoke) -> bool {
        let spreadsheet = std::env::var_os("TURSOBENCH_SPREADSHEET_CAPTURE").is_some();
        match smoke.stage {
            39 if self.smoke_open(ctx) => smoke.stage = 40,
            40 if self.smoke_loaded() => {
                self.browse_table(smoke.table.clone());
                smoke.stage = 41;
            }
            41 if self.database.document.active.is_none()
                && self.database.document.tools.inspection_ticket.is_none() =>
            {
                self.retile();
                ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1366.0, 768.0)));
                if self.can_edit() && !self.database.document.result.rows.is_empty() {
                    let col = usize::from(self.database.document.result.columns.len() > 1);
                    self.stage_cell(
                        0,
                        col,
                        Value::Text("Staged locally for visual review".into()),
                    );
                }
                let sql = format!("SELECT * FROM {} LIMIT 100", quote_identifier(&smoke.table));
                self.preferences.record_query(
                    &self.database.tools.database_scope,
                    &sql,
                    Some(12),
                    false,
                );
                self.preferences.bookmark(
                    &self.database.tools.database_scope,
                    "Sample table",
                    &sql,
                );
                self.prepare_visual_scene(ctx, 0);
                smoke.stage = 42;
            }
            42 => {
                let name = match smoke.scene {
                    1 if spreadsheet => "fill-selection.png",
                    2 if spreadsheet => "paste-new-rows.png",
                    scene => VISUAL_SCENES[scene],
                };
                smoke.capture_after(ctx, 8, name, 43);
            }
            43 if smoke.capture_pending.is_none() => {
                if smoke.scene == 2 && spreadsheet {
                    self.database.document.tools.spreadsheet.dialog = None;
                    self.database.document.drafts = None;
                    self.close();
                    smoke.stage = 45;
                    return false;
                }
                smoke.scene += 1;
                if smoke.scene == 14 {
                    self.prepare_visual_scene(ctx, 14);
                    self.close();
                    smoke.stage = 44;
                } else if smoke.scene == VISUAL_SCENES.len() {
                    assert!(self.database.session.is_none());
                    eprintln!(
                        "DESKTOP VISUAL SMOKE PASSED: six workspace sizes/themes, navigator tools, staged edits, dialogs, sidebar access and all Settings pages; elapsed {:?}",
                        smoke.start.elapsed()
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    return true;
                } else {
                    self.prepare_visual_scene(ctx, smoke.scene);
                    smoke.stage = 42;
                }
            }
            44 if self.database.session.is_none() => {
                self.prepare_visual_scene(ctx, 14);
                smoke.stage = 42;
            }
            45 if self.database.session.is_none() => {
                eprintln!("SPREADSHEET VISUAL SMOKE PASSED");
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return true;
            }
            _ => {}
        }
        false
    }
    fn assert_query_succeeded(&self) {
        let completion = self.database.document.result.completion.as_ref();
        assert!(
            completion.is_some_and(|c| c.error.is_none()),
            "{}",
            self.database.status
        );
    }
    /// The fixture's first page: any rows of a project table with every column,
    /// or exactly the first hundred rows of the smoke fixture.
    fn assert_first_page(&self, project_fixture: bool) {
        self.assert_query_succeeded();
        let document = &self.database.document;
        let rows = &document.result.rows;
        if project_fixture {
            assert!(!rows.is_empty(), "project fixture table must contain rows");
            assert!(
                document
                    .table
                    .as_ref()
                    .is_some_and(|t| t.columns.len() == document.result.columns.len())
            );
        } else {
            assert_eq!(rows.len(), 100, "smoke fixture needs at least 100 rows");
            assert_eq!(rows[0][0], Value::Integer(0));
        }
    }
    /// The relationship map loaded completely, with the fixture table selected.
    fn select_diagram_table(&mut self, table: &str) {
        let graph = self
            .database
            .ui
            .diagram
            .data
            .as_ref()
            .expect("diagram metadata");
        assert!(!graph.limited);
        assert!(graph.tables.iter().any(|t| t.name == table));
        assert!(graph.tables.iter().all(|t| t.error.is_none()));
        self.database.ui.diagram.selected = Some(table.into());
    }
    fn diagram_columns(&self, table: &str) -> &[tursobench_core::TableColumn] {
        let graph = self.database.ui.diagram.data.as_ref().unwrap();
        let table = graph.tables.iter().find(|t| t.name == table).unwrap();
        &table.inspection.as_ref().unwrap().table.columns
    }
    /// Set up the large dark workspace the captures start from and return the
    /// first stage of the captures asked for.
    fn start_captures(&mut self, ctx: &egui::Context, smoke: &Smoke) -> u8 {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1800.0, 1000.0)));
        self.browse_table(smoke.table.clone());
        self.retile();
        self.set_dark(ctx, true);
        if std::env::var_os("TURSOBENCH_ASSISTANT_CAPTURE").is_some() {
            90
        } else if smoke.readme {
            self.start_readme_capture();
            50
        } else {
            20
        }
    }
    /// Open the fixture, opting into Turso for a closed SQLite file as the
    /// desktop smoke intends. Returns whether a session started.
    /// The opened database has its catalog and relationships.
    fn smoke_loaded(&self) -> bool {
        let database = &self.database;
        database.ready && database.catalog_ticket.is_none() && database.ui.diagram.ticket.is_none()
    }
    fn smoke_open(&mut self, ctx: &egui::Context) -> bool {
        if self.databases.inspection.kind(&self.database.path)
            == crate::discovery::DatabaseKind::LikelySqlite
        {
            self.databases.inspection.override_sqlite = true;
        }
        self.open(ctx);
        self.database.session.is_some()
    }
    fn smoke_readonly_write(&mut self, table: &str) {
        self.database
            .document
            .set_sql(format!("DELETE FROM {}", quote_identifier(table)));
        self.database.document.params = "[]".into();
        self.run_sql();
    }
}

/// The workspace captures, one per even stage from 20.
const WORKSPACE_CAPTURES: [&str; 6] = [
    "workspace-dark.png",
    "expanded-map.png",
    "laptop-light.png",
    "laptop-dark.png",
    "minimized-tiles.png",
    "widescreen.png",
];
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
    "close-confirmation.png",
    "database-access.png",
    "search-settings.png",
    "settings-resources.png",
    "settings-queries.png",
    "settings-excluded.png",
    "settings-queries-light.png",
    "settings-discovery-large-text.png",
];
