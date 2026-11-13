use super::{App, click, control, egui, fixture, frame_at, wait_until};
use crate::app::assistant::Entry;
use crate::assistant_tools::{PROPOSE_SQL, RUN_QUERY, Sharing};
use std::os::unix::fs::PermissionsExt;
use tursobench_assistant::mcp::ToolHost;
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);

/// A stand-in for `claude -p`: answer each prompt with a short streamed turn.
const SCRIPT: &str = r#"#!/usr/bin/env bash
case " $* " in *" --tools  "*) ;; *) echo "built-in tools were not removed: $*" >&2; exit 5;; esac
echo '{"type":"system","subtype":"init","model":"fake-model"}'
while read -r line; do
  case "$line" in
    *'"type":"user"'*)
      echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__tursobench__list_tables","input":{}}]},"parent_tool_use_id":null}'
      echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"{\"objects\":[]}"}]},"parent_tool_use_id":null}'
      echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Use:\n```sql\nSELECT * FROM t;\n```"}},"parent_tool_use_id":null}'
      echo '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
      ;;
  esac
done
"#;

fn scripted(app: &mut App, dir: &tempfile::TempDir) {
    let script = dir.path().join("claude");
    std::fs::write(&script, SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.database.assistant.program = Some(script);
}

fn runtime_dir(dir: &tempfile::TempDir) {
    let runtime = dir.path().join("runtime");
    std::fs::create_dir_all(&runtime).unwrap();
    std::fs::set_permissions(&runtime, std::fs::Permissions::from_mode(0o700)).unwrap();
    // SAFETY: set before the tool endpoint (the only reader) starts.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", &runtime) };
}

#[test]
fn a_conversation_asks_for_consent_streams_tools_and_proposes_sql_without_running_it() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES (1, 'a'), (2, 'b')",
        ],
        true,
    );
    runtime_dir(&dir);
    scripted(&mut app, &dir);
    app.preferences.panels.assistant = true;
    let database = app.database.tools.database_scope.clone();
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Schema and query results").center(),
    );
    assert_eq!(
        app.preferences.assistant.sharing.get(&database),
        Some(&Sharing::Rows)
    );

    app.database.assistant.draft = "What is in t?".into();
    app.send_assistant(&ctx);
    assert!(app.database.assistant.busy);
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let entries = &app.database.assistant.entries;
    assert!(matches!(&entries[0], Entry::User(text) if text == "What is in t?"));
    assert!(
        matches!(&entries[1], Entry::Tool { name, result: Some((true, _)), .. } if name == "list_tables")
    );
    assert!(matches!(&entries[2], Entry::Reply(text) if text.contains("SELECT * FROM t;")));
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);

    // The reply's SQL block opens in an editor tile without running.
    click(
        &mut app,
        &ctx,
        control(&output, "Open in a new SQL editor tile (not run)").center(),
    );
    assert_eq!(app.database.document.sql, "SELECT * FROM t;");
    assert!(app.database.document.active.is_none());

    // Tools reach the real session: reads work, writes only become proposals.
    let host = app.database.assistant.host().unwrap();
    let read = host.call(
        RUN_QUERY,
        &serde_json::json!({"sql": "SELECT count(*) AS n FROM t"}),
    );
    assert!(!read.is_error, "{}", read.text);
    assert!(read.text.contains("[[2]]"), "{}", read.text);
    let write = host.call(RUN_QUERY, &serde_json::json!({"sql": "DELETE FROM t"}));
    assert!(write.is_error);
    let proposed = host.call(
        PROPOSE_SQL,
        &serde_json::json!({"sql": "DELETE FROM t WHERE id = 2"}),
    );
    assert!(!proposed.is_error);
    wait_until(&mut app, &ctx, |a| {
        a.database.document.sql == "DELETE FROM t WHERE id = 2"
    });
    assert!(app.database.document.active.is_none());
    let still = host.call(
        RUN_QUERY,
        &serde_json::json!({"sql": "SELECT count(*) FROM t"}),
    );
    assert!(still.text.contains("[[2]]"), "{}", still.text);

    // Schema-only sharing takes effect at once for the running agent.
    app.set_assistant_sharing(Sharing::Schema);
    let refused = host.call(RUN_QUERY, &serde_json::json!({"sql": "SELECT 1"}));
    assert!(refused.is_error);
    assert!(matches!(
        app.database.assistant.entries.last(),
        Some(Entry::Notice(..))
    ));
    app.close();
}

#[test]
fn a_missing_agent_is_explained_and_no_conversation_starts_without_consent() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    runtime_dir(&dir);
    app.database.assistant.draft = "hello".into();
    app.send_assistant(&ctx);
    assert!(app.database.assistant.entries.is_empty() && !app.database.assistant.busy);
    app.set_assistant_sharing(Sharing::Schema);
    app.database.assistant.program = Some(dir.path().join("no-such-claude"));
    app.send_assistant(&ctx);
    assert!(matches!(
        app.database.assistant.entries.last(),
        Some(Entry::Notice(_, text)) if text.contains("not found on PATH")
    ));
    assert!(!app.database.assistant.busy);
    app.close();
}
