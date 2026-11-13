//! Opt-in assistant captures. The conversation is a seeded visual fixture: no
//! agent runs and nothing leaves the machine.
use super::{App, Smoke};
use crate::app::assistant::Entry;
use crate::app::presentation::Intent;
use crate::assistant_tools::Sharing;
use eframe::egui;
use serde_json::json;

const SCENES: &[&str] = &[
    "assistant-consent.png",
    "assistant-conversation.png",
    "assistant-without-sidebar.png",
    "assistant-light.png",
    "panels-hidden.png",
    "assistant-settings.png",
    "assistant-privacy.png",
];

impl App {
    pub(super) fn smoke_assistant(&mut self, ctx: &egui::Context, smoke: &mut Smoke) -> bool {
        match smoke.stage {
            90 => {
                self.prepare_assistant_scene(ctx, smoke.scene);
                smoke.capture_frames = 0;
                smoke.stage = 91;
            }
            91 => {
                smoke.capture_frames += 1;
                if smoke.capture_frames >= 20 {
                    smoke.capture_pending = Some(SCENES[smoke.scene].into());
                    let (tx, rx) = std::sync::mpsc::sync_channel(1);
                    let wake = ctx.clone();
                    ctx.request_screenshot(move |image| {
                        let _ = tx.try_send(image);
                        wake.request_repaint();
                    });
                    smoke.capture_result = Some(rx);
                    smoke.stage = 92;
                }
            }
            92 if smoke.capture_pending.is_none() => {
                smoke.scene += 1;
                if smoke.scene == SCENES.len() {
                    self.settings.open = false;
                    self.close();
                    smoke.stage = 93;
                } else {
                    smoke.stage = 90;
                }
            }
            93 if self.database.session.is_none() => {
                eprintln!(
                    "ASSISTANT VISUAL SMOKE PASSED: {} scenes; elapsed {:?}",
                    SCENES.len(),
                    smoke.start.elapsed()
                );
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                return true;
            }
            _ => {}
        }
        false
    }

    fn prepare_assistant_scene(&mut self, ctx: &egui::Context, scene: usize) {
        ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(1366.0, 768.0)));
        self.preferences.appearance = Some(crate::appearance::Mode::Dark);
        self.preferences.panels.sidebar = true;
        self.preferences.panels.assistant = true;
        match scene {
            0 => {}
            1 => {
                self.set_assistant_sharing(Sharing::Rows);
                self.seed_assistant_conversation();
            }
            2 => self.preferences.panels.sidebar = false,
            3 => {
                self.preferences.panels.sidebar = false;
                self.preferences.appearance = Some(crate::appearance::Mode::Light);
            }
            4 => {
                self.preferences.panels.sidebar = false;
                self.preferences.panels.assistant = false;
            }
            5 => {
                self.settings.open = true;
                self.settings.page = crate::app::settings::Page::Assistant;
            }
            _ => {
                self.preferences.assistant.provider = crate::storage::Provider::Codex;
                self.open_assistant_privacy();
            }
        }
    }

    fn seed_assistant_conversation(&mut self) {
        let conversation = &mut self.database.assistant;
        let tool = |id: &str, name: &str, input, result: &str| Entry::Tool {
            id: id.into(),
            name: name.into(),
            input,
            result: Some((true, result.into())),
        };
        conversation.entries = vec![
            Entry::User("Which projects have the most blocked items?".into()),
            tool("1", "list_tables", json!({}), r#"{"objects":[...]}"#),
            tool("2", "describe_table", json!({"name": "items"}), "{}"),
            tool(
                "3",
                "run_query",
                json!({"sql": "SELECT p.title, count(*) AS blocked\nFROM items i JOIN projects p ON p.id = i.project_id\nWHERE i.status = 'blocked' GROUP BY p.title ORDER BY blocked DESC"}),
                r#"{"columns":["title","blocked"],"rows":[["Launch",84],["Search",83],["Workbench",83]]}"#,
            ),
            Entry::Reply(
                "**Launch** has the most blocked items (84), then Search and Workbench \
                 (83 each).\n\n```sql\nSELECT p.title, count(*) AS blocked\nFROM items i\n\
                 JOIN projects p ON p.id = i.project_id\nWHERE i.status = 'blocked'\n\
                 GROUP BY p.title\nORDER BY blocked DESC;\n```\n\nThe \
                 `items_project_status` index covers this filter."
                    .into(),
            ),
            Entry::User("Mark Launch's blocked items as pending".into()),
            tool(
                "4",
                "propose_sql",
                json!({"sql": "UPDATE items SET status = 'pending'\nWHERE status = 'blocked' AND project_id = 1;"}),
                "Opened in a new SQL editor tile.",
            ),
            Entry::Notice(
                Intent::Primary,
                "Opened proposed SQL in an editor tile. Review it before running.".into(),
            ),
            Entry::Reply(
                "I opened an `UPDATE` in a new SQL editor tile. It has not been run; \
                 review it and run it if it looks right."
                    .into(),
            ),
        ];
        self.load_sql_window(
            "UPDATE items SET status = 'pending'\nWHERE status = 'blocked' AND project_id = 1;"
                .into(),
        );
    }
}
