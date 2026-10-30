use super::*;
use std::time::{Duration, Instant};
pub(super) struct Smoke {
    stage: u8,
    start: Instant,
}
impl Smoke {
    pub(super) fn new() -> Self {
        Self {
            stage: 0,
            start: Instant::now(),
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
                self.browse_table("items".into());
                smoke.stage = 2;
            }
            2 if self.ready && self.active.is_none() => {
                assert_eq!(
                    self.result.rows.len(),
                    100,
                    "smoke fixture needs at least 100 rows"
                );
                assert_eq!(self.result.rows[0][0], Value::Integer(0));
                self.sql = "SELECT ?1 AS typed_blob".into();
                self.params = "[{\"type\":\"Blob\",\"value\":[0,255]}]".into();
                self.run_sql();
                smoke.stage = 3;
            }
            3 if self.active.is_none() => {
                assert_eq!(self.result.rows, vec![vec![Value::Blob(vec![0, 255])]]);
                self.sql = "DELETE FROM items".into();
                self.params = "[]".into();
                self.run_sql();
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
                    "DESKTOP SMOKE PASSED: open, schema, page, typed query, read-only rejection, close; elapsed {:?}",
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
}
