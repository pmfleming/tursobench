use super::{App, shell};
use eframe::egui;
use tursobench_core::{ErrorKind, Value, quote_identifier};

use std::time::{Duration, Instant};
pub(super) struct Smoke {
    stage: u8,
    capture_dir: Option<std::path::PathBuf>,
    capture_pending: Option<String>,
    capture_frames: u8,
    start: Instant,
    table: String,
    project_fixture: bool,
}
impl Smoke {
    pub(super) fn new() -> Self {
        let table = std::env::args().nth(3);
        Self {
            stage: 0,
            capture_dir: std::env::var_os("TURSOBENCH_CAPTURE_DIR").map(Into::into),
            capture_pending: None,
            capture_frames: 0,
            start: Instant::now(),
            project_fixture: table.is_some(),
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
            let screenshot = ctx.input(|i| {
                i.events.iter().find_map(|e| match e {
                    egui::Event::Screenshot { image, .. } => Some(image.clone()),
                    _ => None,
                })
            });
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
            }
        }
        if smoke.start.elapsed() > Duration::from_secs(20) {
            eprintln!(
                "DESKTOP SMOKE FAILED at stage {}: {}",
                smoke.stage, self.status
            );
            std::process::exit(1);
        }
        match smoke.stage {
            0 => {
                self.exclusive = true;
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
                    smoke.stage = 20;
                } else {
                    smoke.stage = 8;
                }
            }
            26 if self.document.active.is_some()
                || self.document.tools.inspection_ticket.is_some() => {}
            20 | 22 | 24 | 26 => {
                if smoke.capture_frames == 0 {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CursorPosition(egui::pos2(
                        1795.0, 995.0,
                    )));
                }
                smoke.capture_frames += 1;
                if smoke.capture_frames >= 15 {
                    smoke.capture_pending = Some(
                        match smoke.stage {
                            20 => "tiled-system.png",
                            22 => "expanded-map.png",
                            24 => "tiled-light.png",
                            _ => "wide-table-below.png",
                        }
                        .into(),
                    );
                    ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
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
                smoke.stage = 24;
            }
            25 if smoke.capture_pending.is_none() => {
                self.preferences.appearance = Some(crate::appearance::Mode::System);
                if self.schema.iter().any(|o| o.name == "wide_records") {
                    self.browse_table("wide_records".into());
                    smoke.stage = 26;
                } else {
                    smoke.stage = 8;
                }
            }
            27 if smoke.capture_pending.is_none() => {
                smoke.stage = 8;
            }
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
                self.close();
                smoke.stage = 5;
            }
            5 if self.session.is_none() => {
                assert!(self.status.contains("cleanup confirmed"), "{}", self.status);
                eprintln!(
                    "DESKTOP SMOKE PASSED: open, schema, page, diagram, typed query, read-only rejection, close; elapsed {:?}",
                    smoke.start.elapsed()
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return;
            }
            _ => {}
        }
        self.smoke = Some(smoke);
        // Automated smoke mode only; normal rendering is event-driven.
        ctx.request_repaint_after(Duration::from_millis(25));
    }
    fn smoke_readonly_write(&mut self, table: &str) {
        self.document.sql = format!("DELETE FROM {}", quote_identifier(table));
        self.document.params = "[]".into();
        self.run_sql();
    }
}
