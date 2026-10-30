use super::*;
use std::time::{Duration, Instant};
pub(super) struct Smoke {
    stage: u8,
    start: Instant,
    table: String,
    project_fixture: bool,
}
impl Smoke {
    pub(super) fn new() -> Self {
        let table = std::env::args().nth(3);
        Self {
            stage: 0,
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
                self.open(ctx);
                smoke.stage = 1;
            }
            1 if self.ready && self.active.is_none() => {
                self.browse_table(smoke.table.clone());
                smoke.stage = 2;
            }
            2 if self.ready && self.active.is_none() => {
                assert!(
                    self.result
                        .completion
                        .as_ref()
                        .is_some_and(|c| c.error.is_none()),
                    "{}",
                    self.status
                );
                if smoke.project_fixture {
                    assert!(
                        !self.result.rows.is_empty(),
                        "project fixture table must contain rows"
                    );
                    assert!(
                        self.table
                            .as_ref()
                            .is_some_and(|t| t.columns.len() == self.result.columns.len())
                    );
                } else {
                    assert_eq!(
                        self.result.rows.len(),
                        100,
                        "smoke fixture needs at least 100 rows"
                    );
                    assert_eq!(self.result.rows[0][0], Value::Integer(0));
                }
                smoke.stage = 9;
            }
            9 => {
                self.ui.workspace = shell::Workspace::Diagram;
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
                smoke.stage = 8;
            }
            8 => {
                self.sql = "SELECT ?1 AS typed_blob".into();
                self.params = "[{\"type\":\"Blob\",\"value\":[0,255]}]".into();
                self.run_sql();
                smoke.stage = 3;
            }
            3 if self.active.is_none() => {
                assert_eq!(self.result.rows, vec![vec![Value::Blob(vec![0, 255])]]);
                if smoke.table == "search_content" {
                    self.sql =
                        "SELECT entry_id FROM search_content WHERE fts_match(indexed_content,?1)"
                            .into();
                    self.params = "[{\"type\":\"Text\",\"value\":\"needle\"}]".into();
                    self.run_sql();
                    smoke.stage = 6;
                } else {
                    self.smoke_readonly_write(&smoke.table);
                    smoke.stage = 4;
                }
            }
            6 if self.active.is_none() => {
                assert_eq!(
                    self.result.rows,
                    vec![vec![Value::Text("entry-0000".into())]]
                );
                self.smoke_readonly_write(&smoke.table);
                smoke.stage = 4;
            }
            4 if self.active.is_none() => {
                assert_eq!(
                    self.result
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
        self.sql = format!("DELETE FROM {}", quote_identifier(table));
        self.params = "[]".into();
        self.run_sql();
    }
}
