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

/// One private runtime directory for every test in this process: tests run in
/// parallel, and a per-test directory could vanish under another test.
fn runtime_dir(_: &tempfile::TempDir) {
    static RUNTIME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        // SAFETY: set once, before any tool endpoint (the only reader) starts.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", runtime.path()) };
        runtime
    });
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

/// A stand-in ACP agent that calls back for one tool permission.
const ACP_SCRIPT: &str = r#"#!/usr/bin/env bash
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":1,"agentCapabilities":{}}}';;
    *'"method":"session/new"'*)
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s1"}}';;
    *'"method":"session/prompt"'*)
      echo '{"jsonrpc":"2.0","id":77,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"z","title":"Edit file /etc/hosts"},"options":[{"optionId":"ok","kind":"allow_once"},{"optionId":"no","kind":"reject_once"}]}}'
      read -r answer
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"ACP reply"}}}}'
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}';;
  esac
done
"#;

#[test]
fn an_acp_agent_can_be_chosen_in_settings_and_its_own_edits_are_declined() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    runtime_dir(&dir);
    app.settings.open = true;
    app.settings.page = crate::app::settings::Page::Assistant;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(&mut app, &ctx, control(&output, "Copilot CLI").center());
    assert_eq!(
        app.preferences.assistant.provider,
        crate::storage::Provider::Copilot
    );
    app.settings.open = false;
    let script = dir.path().join("copilot");
    std::fs::write(&script, ACP_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.database.assistant.program = Some(script);
    app.set_assistant_sharing(Sharing::Schema);
    app.database.assistant.draft = "Hi".into();
    app.send_assistant(&ctx);
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let entries = &app.database.assistant.entries;
    assert!(entries.iter().any(
        |e| matches!(e, Entry::Notice(_, text) if text.contains("Declined") && text.contains("/etc/hosts"))
    ));
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Reply(text) if text == "ACP reply"))
    );
    // The settings list the database's choice, below the privacy section, and
    // can forget it.
    app.settings.open = true;
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let over = control(&output, "Copilot CLI").center();
    frame_at(
        &mut app,
        &ctx,
        SIZE,
        vec![
            egui::Event::PointerMoved(over),
            egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -2000.0),
                phase: egui::TouchPhase::Move,
                source: egui::MouseWheelSource::Unknown,
                modifiers: egui::Modifiers::NONE,
            },
        ],
    );
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Forget; ask again").center(),
    );
    assert!(app.preferences.assistant.sharing.is_empty());
    assert!(app.database.assistant.entries.is_empty());
    app.close();
}

/// A stand-in for `codex app-server`.
const CODEX_SCRIPT: &str = r#"#!/usr/bin/env bash
case " $* " in *" app-server "*"--disable shell_tool"*) ;; *) echo "shell not disabled: $*" >&2; exit 5;; esac
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":'"$id"',"result":{}}';;
    *'"method":"thread/start"'*)
      case "$line" in *'"sandbox":"read-only"'*) ;; *) exit 6;; esac
      case "$line" in *'--mcp-relay'*) ;; *) exit 7;; esac
      echo '{"id":'"$id"',"result":{"thread":{"id":"th"},"model":"gpt-fake"}}';;
    *'"method":"turn/start"'*)
      echo '{"id":'"$id"',"result":{"turn":{"id":"tu"}}}'
      echo '{"id":77,"method":"item/fileChange/requestApproval","params":{"threadId":"th","turnId":"tu","itemId":"f"}}'
      read -r answer
      case "$answer" in *'"decline"'*) ;; *) exit 8;; esac
      echo '{"method":"item/agentMessage/delta","params":{"itemId":"m","delta":"Codex reply"}}'
      echo '{"method":"turn/completed","params":{"turn":{"id":"tu","status":"completed","error":null}}}';;
  esac
done
"#;

#[test]
fn codex_can_be_chosen_in_settings_and_its_own_edits_are_declined() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    runtime_dir(&dir);
    app.settings.open = true;
    app.settings.page = crate::app::settings::Page::Assistant;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(&mut app, &ctx, control(&output, "Codex").center());
    assert_eq!(
        app.preferences.assistant.provider,
        crate::storage::Provider::Codex
    );
    app.settings.open = false;
    let script = dir.path().join("codex");
    std::fs::write(&script, CODEX_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.database.assistant.program = Some(script);
    app.set_assistant_sharing(Sharing::Schema);
    app.database.assistant.draft = "Hi".into();
    app.send_assistant(&ctx);
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let conversation = &app.database.assistant;
    assert_eq!(conversation.model.as_deref(), Some("gpt-fake"));
    assert!(conversation.entries.iter().any(
        |e| matches!(e, Entry::Notice(_, text) if text.contains("Declined") && text.contains("change files"))
    ));
    assert!(
        conversation
            .entries
            .iter()
            .any(|e| matches!(e, Entry::Reply(text) if text == "Codex reply"))
    );
    app.close();
}

fn labelled(output: &egui::FullOutput, name: &str) -> bool {
    crate::app::test_ui::nodes(output).iter().any(|(_, node)| {
        node.label()
            .or(node.value())
            .is_some_and(|text| text == name)
    })
}

/// A stand-in for `claude -p` that records how it was started.
const RECORDING_SCRIPT: &str = r#"#!/usr/bin/env bash
{ printf 'ARGS %s\n' "$*"; printf 'QUIET %s\n' "${CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC:-unset}"; } > "$0.log"
echo '{"type":"system","subtype":"init","model":"fake-model"}'
while read -r line; do
  echo '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
done
"#;

#[test]
fn privacy_is_chosen_per_provider_and_reaches_the_agent() {
    let (dir, ctx, mut app) = fixture(&["CREATE TABLE t(id INTEGER PRIMARY KEY)"], false);
    runtime_dir(&dir);
    app.settings.open = true;
    app.settings.page = crate::app::settings::Page::Assistant;
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(labelled(&output, "PRIVACY · CLAUDE CODE"));
    assert!(labelled(
        &output,
        "Sign-in and account details stay with the CLI"
    ));
    click(
        &mut app,
        &ctx,
        control(&output, "Save conversations in Claude Code's history").center(),
    );
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(
        &mut app,
        &ctx,
        control(&output, "Tell the agent the database's file name").center(),
    );
    let privacy = app.preferences.assistant.privacy;
    assert!(privacy.claude.keep_history && !privacy.claude.share_file_name);
    assert!(privacy.claude.telemetry_off);
    // Other providers keep their private defaults.
    assert_eq!(privacy.codex, crate::storage::ProviderPrivacy::default());

    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    click(&mut app, &ctx, control(&output, "Codex").center());
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(labelled(&output, "Block analytics and telemetry"));
    assert!(!labelled(
        &output,
        "Save conversations in Claude Code's history"
    ));
    click(&mut app, &ctx, control(&output, "Copilot CLI").center());
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(labelled(
        &output,
        "Telemetry and history follow Copilot CLI's own settings"
    ));
    assert!(!labelled(&output, "Block analytics and telemetry"));
    click(&mut app, &ctx, control(&output, "Claude Code").center());
    app.settings.open = false;

    let script = dir.path().join("claude");
    std::fs::write(&script, RECORDING_SCRIPT).unwrap();
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
    app.database.assistant.program = Some(script.clone());
    app.set_assistant_sharing(Sharing::Schema);
    app.database.assistant.draft = "Hi".into();
    app.send_assistant(&ctx);
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let log = std::fs::read_to_string(script.with_extension("log")).unwrap();
    assert!(!log.contains("--no-session-persistence"), "{log}");
    assert!(log.contains("QUIET 1"), "{log}");
    assert!(
        !log.contains("fixture.db"),
        "the file name stays private: {log}"
    );
    assert!(log.contains("a database open"), "{log}");
    app.close();
}
