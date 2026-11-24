use super::{App, control, egui, fixture, frame_at, wait_until};
use crate::app::assistant::Entry;
use crate::app::test_ui::{nodes, pointer};
use crate::assistant_tools::{PROPOSE_SQL, RUN_QUERY, Sharing};
use crate::storage::Provider;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tursobench_assistant::account::{Model, Operation, SignIn, Status};
use tursobench_assistant::mcp::ToolHost;
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
const TABLE: &[&str] = &["CREATE TABLE t(id INTEGER PRIMARY KEY)"];

/// A stand-in for `claude -p` that records how it was started in `$0.log` and
/// what it is sent in `$0.sent`. It answers each prompt with a short streamed
/// turn, except "Wait", which runs until it is interrupted.
const SCRIPT: &str = r#"#!/usr/bin/env bash
case " $* " in *" --tools  "*) ;; *) echo "built-in tools were not removed: $*" >&2; exit 5;; esac
printf 'ARGS %s\nQUIET %s\n' "$*" "${CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC:-unset}" > "$0.log"
echo '{"type":"system","subtype":"init","model":"fake-model"}'
while read -r line; do
  printf '%s\n' "$line" >> "$0.sent"
  case "$line" in
    *'"subtype":"interrupt"'*)
      echo '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"interrupted"}';;
    *'Wait"'*) ;;
    *'"type":"user"'*)
      echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__tursobench__run_query","input":{"sql":"SELECT count(*)\nFROM t"}}]},"parent_tool_use_id":null}'
      echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"[[2]]"}]},"parent_tool_use_id":null}'
      echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Use:\n```sql\nSELECT * FROM t;\n```"}},"parent_tool_use_id":null}'
      echo '{"type":"result","subtype":"success","is_error":false,"result":"done"}'
      ;;
  esac
done
"#;

/// A CLI that reads its input and never answers.
const WAITING: &str = "#!/usr/bin/env bash\nwhile read -r line; do :; done\n";

/// An executable stand-in for an agent CLI.
fn install(dir: &tempfile::TempDir, name: &str, script: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// Start the scripted Claude Code in place of `claude`, at `dir/claude`.
fn scripted(app: &mut App, dir: &tempfile::TempDir) {
    app.database.assistant.program = Some(install(dir, "claude", SCRIPT));
}

/// Send `text` and wait for the agent's turn to end.
fn converse(app: &mut App, ctx: &egui::Context, text: &str) {
    app.database.assistant.draft = text.into();
    app.send_assistant(ctx);
    wait_until(app, ctx, |a| !a.database.assistant.busy);
}

/// The scripted Claude Code answers one turn, sharing only the schema.
fn scripted_turn(app: &mut App, ctx: &egui::Context, dir: &tempfile::TempDir) {
    scripted(app, dir);
    app.set_assistant_sharing(Sharing::Schema);
    converse(app, ctx, "Hello");
}

fn last_notice(conversation: &crate::app::assistant::Conversation) -> Option<&str> {
    match conversation.entries.last() {
        Some(Entry::Notice(_, text)) => Some(text),
        _ => None,
    }
}

fn wait_for_account(app: &mut App, ctx: &egui::Context, provider: Provider) {
    wait_until(app, ctx, |a| a.accounts.rows[&provider].job.is_none());
}

fn status(app: &App, provider: Provider) -> &Status {
    app.accounts.rows[&provider].status.as_ref().unwrap()
}

/// A model as an agent's check lists it.
fn model(id: &str, name: &str) -> Model {
    Model {
        id: id.into(),
        name: name.into(),
        ..Default::default()
    }
}

/// Codex as a check would leave it: signed in, listing `models`.
fn codex_signed_in(app: &mut App, models: Vec<Model>) {
    app.accounts.rows.entry(Provider::Codex).or_default().status = Some(Status {
        state: SignIn::SignedIn,
        models,
        ..Default::default()
    });
}

/// One private runtime directory for every test in this process: tests run in
/// parallel, and a per-test directory could vanish under another test.
fn runtime_dir() {
    static RUNTIME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        tursobench_assistant::relay::set_runtime_root(runtime.path().into());
        runtime
    });
}

fn labelled(output: &egui::FullOutput, name: &str) -> bool {
    nodes(output).iter().any(|(_, node)| {
        node.label()
            .or(node.value())
            .is_some_and(|text| text == name)
    })
}

/// Click at the size the test measured controls at.
fn tap(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    for pressed in [true, false] {
        frame_at(app, ctx, SIZE, pointer(pos, pressed));
    }
}

/// Lay out a frame and click the control called `name` in it.
fn press(app: &mut App, ctx: &egui::Context, name: &str) {
    let output = frame_at(app, ctx, SIZE, vec![]);
    tap(app, ctx, control(&output, name).center());
}

/// The current value a named control reports.
fn value_of(output: &egui::FullOutput, name: &str) -> Option<String> {
    nodes(output)
        .iter()
        .find(|(_, node)| node.label().is_some_and(|l| l == name))
        .and_then(|(_, node)| node.value().map(str::to_owned))
}

/// What the header's model picker shows.
fn picked(output: &egui::FullOutput) -> Option<String> {
    value_of(output, "Assistant model picker")
}

/// Settings → Assistant, laid out.
fn assistant_settings(app: &mut App, ctx: &egui::Context) -> egui::FullOutput {
    app.open_assistant_settings();
    frame_at(app, ctx, SIZE, vec![]);
    frame_at(app, ctx, SIZE, vec![])
}

/// Scroll the open settings to their end, where each database's choices are.
fn scroll_settings_to_end(app: &mut App, ctx: &egui::Context) {
    let output = frame_at(app, ctx, SIZE, vec![]);
    let over = control(&output, "Copilot CLI").center();
    let wheel = egui::Event::MouseWheel {
        unit: egui::MouseWheelUnit::Point,
        delta: egui::vec2(0.0, -2000.0),
        phase: egui::TouchPhase::Move,
        source: egui::MouseWheelSource::Unknown,
        modifiers: egui::Modifiers::NONE,
    };
    frame_at(app, ctx, SIZE, vec![egui::Event::PointerMoved(over), wheel]);
    frame_at(app, ctx, SIZE, vec![]);
}

#[test]
fn a_conversation_shares_results_by_default_streams_tools_and_proposes_sql_without_running_it() {
    let (dir, ctx, mut app) = fixture(
        &[
            "CREATE TABLE t(id INTEGER PRIMARY KEY, v TEXT)",
            "INSERT INTO t VALUES (1, 'a'), (2, 'b')",
        ],
        true,
    );
    runtime_dir();
    app.set_assistant_shown(true);
    let database = app.database.tools.database_scope.clone();
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.preferences.assistant.grant_for(&database), None);
    assert_eq!(
        app.preferences.assistant.sharing_for(&database),
        Sharing::Rows
    );

    scripted(&mut app, &dir);
    app.database.assistant.draft = "What is in t?".into();
    app.send_assistant(&ctx);
    assert!(app.database.assistant.busy);
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let entries = &app.database.assistant.entries;
    assert!(matches!(&entries[0], Entry::User(text) if text == "What is in t?"));
    assert!(
        matches!(&entries[1], Entry::Tool { name, result: Some((true, _)), .. } if name == RUN_QUERY)
    );
    assert!(matches!(&entries[2], Entry::Reply(text) if text.contains("SELECT * FROM t;")));
    // A tool call folds out to show its result.
    press(&mut app, &ctx, "SELECT count(*)…");
    assert!(labelled(&frame_at(&mut app, &ctx, SIZE, vec![]), "[[2]]"));

    // The reply's SQL block opens in an editor tile without running.
    press(&mut app, &ctx, "Open in a new SQL editor tile (not run)");
    assert_eq!(app.database.document.sql, "SELECT * FROM t;");
    assert!(app.database.document.active.is_none());

    // Tools reach the real session: reads work, writes only become proposals.
    let host = app.database.assistant.host().unwrap();
    let query = |sql: &str| host.call(RUN_QUERY, &serde_json::json!({ "sql": sql }));
    let read = query("SELECT count(*) AS n FROM t");
    assert!(!read.is_error, "{}", read.text);
    assert!(read.text.contains("[[2]]"), "{}", read.text);
    assert!(query("DELETE FROM t").is_error);
    let proposed = host.call(
        PROPOSE_SQL,
        &serde_json::json!({"sql": "DELETE FROM t WHERE id = 2"}),
    );
    assert!(!proposed.is_error);
    wait_until(&mut app, &ctx, |a| {
        a.database.document.sql == "DELETE FROM t WHERE id = 2"
    });
    assert!(app.database.document.active.is_none());
    let still = query("SELECT count(*) FROM t");
    assert!(still.text.contains("[[2]]"), "{}", still.text);

    // The ribbon switches to schema only at once for the running agent.
    press(&mut app, &ctx, "Share the schema only");
    assert_eq!(
        app.preferences.assistant.grant_for(&database),
        Some(Sharing::Schema)
    );
    assert!(query("SELECT 1").is_error);
    assert!(last_notice(&app.database.assistant).is_some());
    app.close();
}

#[test]
fn a_missing_agent_is_explained_and_stop_ends_a_running_turn() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    app.set_assistant_shown(true);
    app.database.assistant.draft = "Wait".into();
    app.database.assistant.program = Some(dir.path().join("no-such-claude"));
    app.send_assistant(&ctx);
    let missing = last_notice(&app.database.assistant);
    assert!(missing.is_some_and(|text| text.contains("was not found")));
    assert!(!app.database.assistant.busy);
    // The draft waits to be sent again; stopping is not a failure to report.
    scripted(&mut app, &dir);
    app.send_assistant(&ctx);
    assert!(app.database.assistant.busy);
    press(&mut app, &ctx, "Stop");
    wait_until(&mut app, &ctx, |a| !a.database.assistant.busy);
    let last = app.database.assistant.entries.last();
    assert!(matches!(last, Some(Entry::User(text)) if text == "Wait"));
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

/// The agent asked to `change` and was declined, and replied with `reply`.
fn declined_then_replied(app: &App, change: &str, reply: &str) {
    let entries = &app.database.assistant.entries;
    assert!(entries.iter().any(
        |e| matches!(e, Entry::Notice(_, text) if text.contains("Declined") && text.contains(change))
    ));
    assert!(
        entries
            .iter()
            .any(|e| matches!(e, Entry::Reply(text) if text == reply))
    );
}

#[test]
fn agents_decline_their_own_edits_and_forgetting_sharing_reaches_every_conversation() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    // An ACP agent chosen in Settings.
    assistant_settings(&mut app, &ctx);
    press(&mut app, &ctx, "Copilot CLI");
    assert_eq!(app.preferences.assistant.provider, Provider::Copilot);
    app.settings.open = false;
    app.database.assistant.program = Some(install(&dir, "agent", ACP_SCRIPT));
    app.set_assistant_sharing(Sharing::Schema);
    converse(&mut app, &ctx, "Hi");
    declined_then_replied(&app, "/etc/hosts", "ACP reply");
    // In the background, the conversation still hears of sharing changes.
    let id = app.database.id;
    assert!(app.new_workspace(&ctx));
    // The settings list the database's choice, below the privacy section, and
    // can forget it.
    app.settings.open = true;
    scroll_settings_to_end(&mut app, &ctx);
    press(&mut app, &ctx, "Forget; use the default");
    assert!(app.preferences.assistant.grants.is_empty());
    // The conversation goes on with the default, and is told so.
    let background = &app.parked[&id].assistant;
    assert!(background.host().is_some());
    assert_eq!(
        last_notice(background),
        Some("Now sharing the schema and query results")
    );
    // Changing the default reaches a database without its own choice.
    app.select_workspace(id);
    press(&mut app, &ctx, "Default: share the schema only");
    assert_eq!(app.preferences.assistant.default_sharing, Sharing::Schema);
    assert_eq!(
        last_notice(&app.database.assistant),
        Some("Now sharing only the schema")
    );
    // Codex's own edits are declined too.
    app.select_assistant_provider(Provider::Codex);
    app.database.assistant.program = Some(install(&dir, "agent", CODEX_SCRIPT));
    converse(&mut app, &ctx, "Hi");
    assert_eq!(app.database.assistant.model.as_deref(), Some("gpt-fake"));
    declined_then_replied(&app, "change files", "Codex reply");
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
fn privacy_is_chosen_per_provider_and_reaches_the_agent() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    let output = assistant_settings(&mut app, &ctx);
    assert!(labelled(&output, "AGENT SETTINGS") && labelled(&output, "Privacy"));
    // Fixed promises live in the docs, not the settings area.
    assert!(!labelled(&output, "Credentials stay with the CLI"));
    let history = "Save conversations in Claude Code's history";
    press(&mut app, &ctx, history);
    press(&mut app, &ctx, "Tell the agent the database's file name");
    let privacy = &app.preferences.assistant.privacy;
    let claude = privacy.of(Provider::Claude);
    assert!(claude.keep_history && !claude.share_file_name);
    assert!(claude.telemetry_off);
    // Other providers keep their private defaults.
    assert_eq!(
        privacy.of(Provider::Codex),
        crate::storage::ProviderPrivacy::default()
    );

    // Choosing the chat's agent leaves the settings area where it was.
    press(&mut app, &ctx, "Codex");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.preferences.assistant.provider, Provider::Codex);
    assert!(labelled(&output, history));
    // The "Settings for" choice switches it, and leaves the chat's agent alone.
    app.accounts.configuring = Some(Provider::Copilot);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.preferences.assistant.provider, Provider::Codex);
    let note = "Telemetry and history follow Copilot CLI's own settings";
    assert!(labelled(&output, note));
    assert!(!labelled(&output, "Block analytics and telemetry"));
    press(&mut app, &ctx, "Claude Code");
    app.settings.open = false;

    scripted_turn(&mut app, &ctx, &dir);
    let log = std::fs::read_to_string(dir.path().join("claude.log")).unwrap();
    assert!(!log.contains("--no-session-persistence"), "{log}");
    assert!(log.contains("QUIET 1"), "{log}");
    assert!(
        !log.contains("fixture.db"),
        "the file name stays private: {log}"
    );
    assert!(log.contains("a database open"), "{log}");
    app.close();
}

#[test]
fn each_agents_notice_shows_on_first_use_then_every_few_sessions_and_links_to_its_privacy() {
    use crate::storage::AssistantPreferences;
    let (_dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    app.set_assistant_shown(true);
    let notice = "Privacy settings";
    let session = |app: &mut App| {
        // A new session forgets what this one decided.
        ctx.memory_mut(|m| m.data.clear());
        frame_at(app, &ctx, SIZE, vec![]);
        labelled(&frame_at(app, &ctx, SIZE, vec![]), notice)
    };
    assert!(session(&mut app), "first use shows the notice");
    // Repainting within a session keeps the decision.
    assert!(labelled(&frame_at(&mut app, &ctx, SIZE, vec![]), notice));
    for _ in 0..AssistantPreferences::INTRO_SKIPS {
        assert!(!session(&mut app));
    }
    assert!(session(&mut app), "shows again after a few sessions");
    assert!(!session(&mut app));
    // Another agent has its own first use.
    app.select_assistant_provider(Provider::Codex);
    assert!(session(&mut app));

    // The notice links to the agent's privacy settings.
    press(&mut app, &ctx, notice);
    assert!(app.settings.open);
    assert_eq!(app.settings.page, crate::app::settings::Page::Assistant);
    assert_eq!(app.accounts.configuring, Some(Provider::Codex));
    for _ in 0..3 {
        frame_at(&mut app, &ctx, SIZE, vec![]);
    }
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    // Scrolled so the privacy switches are in view.
    let switch = control(&output, "Tell the agent the database's file name");
    assert!(switch.top() > 0.0 && switch.bottom() < SIZE.y);
    app.close();
}

#[test]
fn one_picker_changes_agent_model_and_favourites_with_each_agents_sharing() {
    use crate::storage::FavouriteModel;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    app.set_assistant_shown(true);
    scripted_turn(&mut app, &ctx, &dir);
    let sharing = |app: &App| {
        let database = &app.database.tools.database_scope;
        app.preferences.assistant.sharing_for(database)
    };
    // Another agent starts afresh, with its own sharing.
    app.select_assistant_provider(Provider::Codex);
    assert!(app.database.assistant.host().is_none());
    assert!(app.database.assistant.entries.is_empty());
    assert_eq!(sharing(&app), Sharing::Rows);
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(sharing(&app), Sharing::Schema);
    converse(&mut app, &ctx, "Hello");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(picked(&output).as_deref(), Some("Claude Code"));
    press(&mut app, &ctx, "Assistant model picker");
    press(&mut app, &ctx, "Sonnet");
    assert_eq!(app.preferences.assistant.selected_model(), "sonnet");
    assert!(app.database.assistant.host().is_none());
    // The picker stays open so the reasoning level can follow the model.
    assert!(labelled(
        &frame_at(&mut app, &ctx, SIZE, vec![]),
        "Favourites"
    ));
    let ctrl_3 = super::key_event(egui::Key::Num3, true, egui::Modifiers::CTRL);
    frame_at(&mut app, &ctx, SIZE, vec![ctrl_3]);
    assert_eq!(app.preferences.assistant.selected_model(), "opus");

    codex_signed_in(&mut app, vec![model("gpt-test", "Test model")]);
    // Browsing another agent's models changes nothing until one is chosen.
    press(&mut app, &ctx, "Codex · Signed in");
    assert_eq!(app.preferences.assistant.provider, Provider::Claude);
    press(&mut app, &ctx, "Favourite Test model");
    assert_eq!(
        app.preferences.assistant.favourite_models,
        [FavouriteModel {
            provider: Provider::Codex,
            model: "gpt-test".into()
        }]
    );
    press(&mut app, &ctx, "Favourites");
    press(&mut app, &ctx, "Test model");
    assert_eq!(app.preferences.assistant.provider, Provider::Codex);
    assert_eq!(app.preferences.assistant.selected_model(), "gpt-test");
    assert_eq!(sharing(&app), Sharing::Rows);
    let escape = super::key_event(egui::Key::Escape, true, egui::Modifiers::NONE);
    frame_at(&mut app, &ctx, SIZE, vec![escape]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(!labelled(&output, "Favourites"));
    assert!(labelled(&output, "Share the schema and query results"));
    assert_eq!(picked(&output).as_deref(), Some("Test model"));
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(app.preferences.assistant.selected_model(), "opus");
    assert_eq!(sharing(&app), Sharing::Schema);
    app.close();
}

/// Start signing in to `provider` with `program` in place of its CLI.
fn sign_in(app: &mut App, ctx: &egui::Context, provider: Provider, program: PathBuf) {
    app.preferences
        .assistant
        .executables
        .insert(provider, program);
    app.agent_account_job(ctx, provider, Operation::Login { method: None });
}

fn cancel(app: &App, provider: Provider) {
    app.accounts.rows[&provider].job.as_ref().unwrap().cancel();
}

#[test]
fn only_the_selected_vendors_account_change_stops_chat_and_blocks_new_turns() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    scripted_turn(&mut app, &ctx, &dir);
    let helper = install(&dir, "waiting-helper", WAITING);

    // Signing in to another vendor leaves the Claude Code chat running.
    sign_in(&mut app, &ctx, Provider::Codex, helper.clone());
    assert!(!app.assistant_account_pending());
    assert!(app.database.assistant.host().is_some());
    let entries = app.database.assistant.entries.len();
    converse(&mut app, &ctx, "Still here");
    assert!(app.database.assistant.entries.len() > entries);
    cancel(&app, Provider::Codex);
    wait_for_account(&mut app, &ctx, Provider::Codex);
    assert!(app.database.assistant.host().is_some());

    // Signing out of the selected vendor ends its chat and holds new turns.
    let executables = &mut app.preferences.assistant.executables;
    executables.insert(Provider::Claude, helper);
    app.agent_account_job(&ctx, Provider::Claude, Operation::Logout);
    assert!(app.assistant_account_pending());
    assert!(app.database.assistant.host().is_none());
    app.database.assistant.draft = "Wait for account change".into();
    app.send_assistant(&ctx);
    assert!(app.database.assistant.host().is_none());
    assert!(app.database.assistant.entries.is_empty());
    cancel(&app, Provider::Claude);
    wait_until(&mut app, &ctx, |a| !a.assistant_account_pending());
    assert_eq!(app.database.assistant.draft, "Wait for account change");
    app.close();
}

#[test]
fn cancelling_or_failing_a_sign_in_keeps_the_last_status_and_models() {
    use crate::app::presentation::Intent;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    app.accounts.rows.entry(Provider::Codex).or_default().status = Some(Status {
        state: SignIn::SignedOut,
        models: vec![model("gpt-test", "Test model")],
        ..Default::default()
    });
    let still_known = |app: &App, described| {
        let status = status(app, Provider::Codex);
        assert_eq!(status.state, SignIn::SignedOut);
        assert_eq!(status.models[0].id, "gpt-test");
        let row = &app.accounts.rows[&Provider::Codex];
        assert_eq!(row.describe(), Some(described));
    };
    sign_in(
        &mut app,
        &ctx,
        Provider::Codex,
        install(&dir, "waiting", WAITING),
    );
    cancel(&app, Provider::Codex);
    wait_for_account(&mut app, &ctx, Provider::Codex);
    still_known(&app, ("Signed out", Intent::Pending));
    let failing = install(&dir, "failing", "#!/bin/sh\nexit 3\n");
    sign_in(&mut app, &ctx, Provider::Codex, failing);
    wait_for_account(&mut app, &ctx, Provider::Codex);
    still_known(&app, ("Sign-in failed", Intent::Danger));
    app.close();
}

#[test]
fn the_panel_checks_the_selected_vendor_once_without_opening_settings() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    // A stand-in for `codex app-server` that answers only status calls.
    let codex = install(
        &dir,
        "codex",
        r#"#!/usr/bin/env bash
[[ $1 == --version ]] && { echo 'codex test'; exit 0; }
while read -r line; do
  case "$line" in
    *'"method":"initialize"'*) echo '{"id":1,"result":{}}';;
    *'"method":"account/read"'*) echo '{"id":4,"result":{"account":{"type":"chatgpt","planType":"pro"},"requiresOpenaiAuth":true}}';;
    *'"method":"model/list"'*) echo '{"id":10,"result":{"data":[{"id":"gpt-test","displayName":"Test model"}],"nextCursor":null}}';;
  esac
done
"#,
    );
    let broken = install(&dir, "broken-claude", "#!/bin/sh\nexit 0\n");
    let executables = &mut app.preferences.assistant.executables;
    executables.insert(Provider::Codex, codex);
    executables.insert(Provider::Claude, broken);
    app.preferences.assistant.provider = Provider::Codex;
    app.set_assistant_shown(true);
    app.accounts.automatic = true;

    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(app.accounts.rows[&Provider::Codex].job.is_some());
    wait_for_account(&mut app, &ctx, Provider::Codex);
    assert_eq!(status(&app, Provider::Codex).models[0].id, "gpt-test");
    assert!(!app.settings.open);
    // Only the selected vendor is asked.
    assert!(!app.accounts.rows.contains_key(&Provider::Antigravity));
    press(&mut app, &ctx, "Assistant model picker");
    press(&mut app, &ctx, "Test model");
    assert_eq!(app.preferences.assistant.selected_model(), "gpt-test");

    // Choosing another vendor checks it; a failed check is not retried each frame.
    app.select_assistant_provider(Provider::Claude);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    wait_for_account(&mut app, &ctx, Provider::Claude);
    assert!(app.accounts.rows[&Provider::Claude].error.is_some());
    for _ in 0..3 {
        frame_at(&mut app, &ctx, SIZE, vec![]);
    }
    assert!(app.accounts.rows[&Provider::Claude].job.is_none());
    app.close();
}

#[test]
fn agent_scan_reports_status_and_missing_executables_without_starting_a_conversation() {
    use tursobench_assistant::vendor::VENDORS;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    let script = install(
        &dir,
        "my claude",
        "#!/usr/bin/env bash\nif [[ $1 == --version ]]; then echo 'Claude test'; else echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"email\":\"private@example.com\"}'; fi\n",
    );
    let executables = &mut app.preferences.assistant.executables;
    for vendor in VENDORS {
        executables.insert(vendor.id, dir.path().join("not-installed"));
    }
    executables.insert(Provider::Claude, script);
    app.scan_assistant(&ctx);
    wait_until(&mut app, &ctx, |a| {
        a.accounts.rows.values().all(|row| row.job.is_none())
    });
    let claude = status(&app, Provider::Claude);
    assert_eq!(claude.state, SignIn::SignedIn);
    assert_eq!(claude.version.as_deref(), Some("Claude test"));
    assert_eq!(status(&app, Provider::Codex).state, SignIn::Missing);
    assert!(app.database.assistant.host().is_none());
    assert!(app.preferences.assistant.grants.is_empty());
    let saved = serde_json::to_string(&app.preferences).unwrap();
    assert!(!saved.contains("private@example.com"));
    app.close();
}

/// Click the control called `name` and return the text it copied.
fn copied(app: &mut App, ctx: &egui::Context, name: &str) -> Vec<String> {
    frame_at(app, ctx, SIZE, vec![]);
    let output = frame_at(app, ctx, SIZE, vec![]);
    let pos = control(&output, name).center();
    frame_at(app, ctx, SIZE, pointer(pos, true));
    let output = frame_at(app, ctx, SIZE, pointer(pos, false));
    let commands = output.platform_output.commands.into_iter();
    commands
        .filter_map(|c| match c {
            egui::OutputCommand::CopyText(text) => Some(text),
            _ => None,
        })
        .collect()
}

#[test]
fn copy_buttons_write_the_reply_and_conversation_in_the_chosen_format() {
    use crate::storage::CopyFormat;
    let (_dir, ctx, mut app) = fixture(TABLE, false);
    app.set_assistant_shown(true);
    app.database.assistant.entries =
        vec![Entry::User("Hi".into()), Entry::Reply("**Done**".into())];
    assert_eq!(copied(&mut app, &ctx, "Copy last reply"), ["**Done**"]);
    assert_eq!(
        copied(&mut app, &ctx, "Copy conversation"),
        ["## You\n\nHi\n\n## Claude Code\n\n**Done**"]
    );
    app.preferences.assistant.copy_format = CopyFormat::Html;
    assert_eq!(
        copied(&mut app, &ctx, "Copy last reply"),
        ["<p><strong>Done</strong></p>"]
    );
    assert_eq!(
        copied(&mut app, &ctx, "Copy conversation"),
        ["<h2>You</h2>\n<p>Hi</p>\n<h2>Claude Code</h2>\n<p><strong>Done</strong></p>"]
    );
    app.close();
}

#[test]
fn a_custom_agents_command_executable_and_sign_in_method_are_set_in_settings() {
    use tursobench_assistant::account::AuthMethod;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    let custom = Provider::Custom;
    let grants = app.preferences.assistant.grants.entry("a.db".into());
    grants.or_default().insert(custom, Sharing::Schema);
    app.accounts.configuring = Some(custom);
    assistant_settings(&mut app, &ctx);
    let typed = |app: &mut App, name: &str, text: &str| {
        press(app, &ctx, name);
        frame_at(app, &ctx, SIZE, vec![egui::Event::Text(text.into())]);
    };
    // A new command forgets the custom agent's grants.
    typed(&mut app, "Agent command", "my-agent acp");
    assert_eq!(app.preferences.assistant.custom_command, "my-agent acp");
    assert!(app.preferences.assistant.grants.is_empty());
    assert!(!app.agent_state(custom).2, "not on PATH");

    press(&mut app, &ctx, "Choose executable");
    typed(&mut app, "Executable path", "relative/agent");
    press(&mut app, &ctx, "Apply");
    let error = app.accounts.rows[&custom].error.as_deref();
    assert_eq!(error, Some("Use an absolute executable path"));
    let program = install(&dir, "it's agent", "#!/bin/sh\nexit 0\n");
    app.accounts.executable = program.to_string_lossy().into();
    press(&mut app, &ctx, "Apply");
    assert_eq!(app.preferences.assistant.executables[&custom], program);
    // The path decides, not the command's first word.
    assert!(app.agent_state(custom).2);
    wait_for_account(&mut app, &ctx, custom);
    let quoted = format!("'{}'", program.display()).replace("it's", r"it'\''s");
    assert_eq!(copied(&mut app, &ctx, "Copy sign-in command"), [quoted]);

    let method = |id: &str| AuthMethod {
        id: id.into(),
        name: id.to_uppercase(),
    };
    app.accounts.rows.get_mut(&custom).unwrap().status = Some(Status {
        state: SignIn::SignedOut,
        auth_methods: vec![method("a"), method("b")],
        ..Default::default()
    });
    assert!(labelled(
        &frame_at(&mut app, &ctx, SIZE, vec![]),
        "Sign in with"
    ));
    assert_eq!(app.accounts.rows[&custom].method.as_deref(), Some("a"));
    app.close();
}

#[test]
fn the_reasoning_level_changes_mid_conversation_and_follows_each_codex_model() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir();
    scripted(&mut app, &dir);
    let log = dir.path().join("claude.sent");
    app.set_assistant_shown(true);
    converse(&mut app, &ctx, "Hello");
    press(&mut app, &ctx, "Assistant model picker");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let level = |output: &egui::FullOutput| value_of(output, "Assistant reasoning level");
    assert_eq!(level(&output).as_deref(), Some("Default"));
    press(&mut app, &ctx, "Reasoning Extra high");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(picked(&output).as_deref(), Some("Claude Code · Extra high"));
    assert_eq!(app.preferences.assistant.selected_effort(), "xhigh");
    // Unlike a model change, the conversation and its agent carry on.
    assert!(app.database.assistant.host().is_some());
    assert!(!app.database.assistant.entries.is_empty());
    let sent = || std::fs::read_to_string(&log).unwrap_or_default();
    wait_until(&mut app, &ctx, |_| {
        sent().contains(r#""effortLevel":"xhigh""#)
    });
    assert!(sent().contains("get_settings"));
    // Back to the default: the level read at start is restored (unknown here).
    app.select_effort_for(Provider::Claude, String::new());
    wait_until(&mut app, &ctx, |_| sent().contains(r#""effortLevel":null"#));

    // Codex lists levels per model; the default model stands in for none.
    let leveled = |id: &str, efforts: &[&str], default: bool| Model {
        efforts: efforts.iter().map(|e| (*e).into()).collect(),
        default_effort: Some("low".into()),
        default,
        ..model(id, id)
    };
    codex_signed_in(
        &mut app,
        vec![
            leveled("deep", &["low", "high", "ultra"], true),
            leveled("quick", &["low"], false),
        ],
    );
    // Browsing Codex in the open picker shows its levels; choosing one
    // is remembered for Codex without leaving Claude Code.
    press(&mut app, &ctx, "Codex · Signed in");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(level(&output).as_deref(), Some("Default · Low"));
    press(&mut app, &ctx, "Reasoning Ultra");
    assert_eq!(app.preferences.assistant.provider, Provider::Claude);
    assert_eq!(app.preferences.assistant.efforts[&Provider::Codex], "ultra");
    app.select_assistant_provider(Provider::Codex);
    assert_eq!(app.assistant_effort().as_deref(), Some("ultra"));
    // A model without that level falls back to the agent's own.
    app.choose_assistant_model(Provider::Codex, "quick".into());
    assert_eq!(app.preferences.assistant.selected_effort(), "ultra");
    assert_eq!(app.assistant_effort(), None);
    // Claude keeps its own choice.
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(app.preferences.assistant.selected_effort(), "");
    app.close();
}
