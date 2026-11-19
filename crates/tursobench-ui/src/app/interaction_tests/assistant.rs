use super::{App, control, egui, fixture, frame_at, wait_until};
use crate::app::assistant::Entry;
use crate::assistant_tools::{PROPOSE_SQL, RUN_QUERY, Sharing};
use crate::storage::Provider;
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use tursobench_assistant::account::{Model, Operation, SignIn, Status};
use tursobench_assistant::mcp::ToolHost;
const SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
const TABLE: &[&str] = &["CREATE TABLE t(id INTEGER PRIMARY KEY)"];

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

/// A CLI that reads its input and never answers.
const WAITING: &str = "#!/usr/bin/env bash\nwhile read -r line; do :; done\n";

/// An executable stand-in for an agent CLI.
fn install(dir: &tempfile::TempDir, name: &str, script: &str) -> PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, script).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

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
fn runtime_dir(_: &tempfile::TempDir) {
    static RUNTIME: std::sync::OnceLock<tempfile::TempDir> = std::sync::OnceLock::new();
    RUNTIME.get_or_init(|| {
        let runtime = tempfile::tempdir().unwrap();
        std::fs::set_permissions(runtime.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        tursobench_assistant::relay::set_runtime_root(runtime.path().into());
        runtime
    });
}

fn labelled(output: &egui::FullOutput, name: &str) -> bool {
    crate::app::test_ui::nodes(output).iter().any(|(_, node)| {
        node.label()
            .or(node.value())
            .is_some_and(|text| text == name)
    })
}

/// Click at the size the test measured controls at.
fn tap(app: &mut App, ctx: &egui::Context, pos: egui::Pos2) {
    for pressed in [true, false] {
        let events = vec![
            egui::Event::PointerMoved(pos),
            super::pointer_button(pos, pressed),
        ];
        frame_at(app, ctx, SIZE, events);
    }
}

/// Lay out a frame and click the control called `name` in it.
fn press(app: &mut App, ctx: &egui::Context, name: &str) {
    let output = frame_at(app, ctx, SIZE, vec![]);
    tap(app, ctx, control(&output, name).center());
}

/// The current value a named control reports.
fn value_of(output: &egui::FullOutput, name: &str) -> Option<String> {
    crate::app::test_ui::nodes(output)
        .iter()
        .find(|(_, node)| node.label().is_some_and(|l| l == name))
        .and_then(|(_, node)| node.value().map(str::to_owned))
}

/// Settings → Assistant, laid out.
fn assistant_settings(app: &mut App, ctx: &egui::Context) -> egui::FullOutput {
    app.open_assistant_settings();
    frame_at(app, ctx, SIZE, vec![]);
    frame_at(app, ctx, SIZE, vec![])
}

/// Choose an agent in Settings, then have a stand-in for it answer one
/// schema-only turn.
fn choose_and_converse(
    app: &mut App,
    ctx: &egui::Context,
    dir: &tempfile::TempDir,
    (name, provider): (&str, Provider),
    script: &str,
) {
    assistant_settings(app, ctx);
    press(app, ctx, name);
    assert_eq!(app.preferences.assistant.provider, provider);
    app.settings.open = false;
    app.database.assistant.program = Some(install(dir, "agent", script));
    app.set_assistant_sharing(Sharing::Schema);
    converse(app, ctx, "Hi");
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
    runtime_dir(&dir);
    scripted(&mut app, &dir);
    app.set_assistant_shown(true);
    let database = app.database.tools.database_scope.clone();
    frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.preferences.assistant.grant_for(&database), None);
    assert_eq!(
        app.preferences.assistant.sharing_for(&database),
        Sharing::Rows
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
fn a_missing_agent_is_explained() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    app.database.assistant.draft = "hello".into();
    app.database.assistant.program = Some(dir.path().join("no-such-claude"));
    app.send_assistant(&ctx);
    assert!(
        last_notice(&app.database.assistant).is_some_and(|text| text.contains("was not found"))
    );
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
fn an_acp_agent_declines_its_own_edits_and_forgetting_its_sharing_reaches_every_conversation() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    let copilot = ("Copilot CLI", Provider::Copilot);
    choose_and_converse(&mut app, &ctx, &dir, copilot, ACP_SCRIPT);
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
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    let codex = ("Codex", Provider::Codex);
    choose_and_converse(&mut app, &ctx, &dir, codex, CODEX_SCRIPT);
    assert_eq!(app.database.assistant.model.as_deref(), Some("gpt-fake"));
    declined_then_replied(&app, "change files", "Codex reply");
    app.close();
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
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    let output = assistant_settings(&mut app, &ctx);
    assert!(labelled(&output, "AGENT SETTINGS") && labelled(&output, "Privacy"));
    // Fixed promises live in the docs, not the settings area.
    assert!(!labelled(&output, "Credentials stay with the CLI"));
    press(
        &mut app,
        &ctx,
        "Save conversations in Claude Code's history",
    );
    press(&mut app, &ctx, "Tell the agent the database's file name");
    let privacy = app.preferences.assistant.privacy.clone();
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
    assert!(labelled(
        &output,
        "Save conversations in Claude Code's history"
    ));
    // The "Settings for" choice switches it, and leaves the chat's agent alone.
    app.accounts.configuring = Some(Provider::Copilot);
    frame_at(&mut app, &ctx, SIZE, vec![]);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(app.preferences.assistant.provider, Provider::Codex);
    assert!(labelled(
        &output,
        "Telemetry and history follow Copilot CLI's own settings"
    ));
    assert!(!labelled(&output, "Block analytics and telemetry"));
    tap(&mut app, &ctx, control(&output, "Claude Code").center());
    app.settings.open = false;

    let script = install(&dir, "claude", RECORDING_SCRIPT);
    app.database.assistant.program = Some(script.clone());
    app.set_assistant_sharing(Sharing::Schema);
    converse(&mut app, &ctx, "Hi");
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

#[test]
fn each_agents_notice_shows_on_first_use_then_every_few_sessions_and_links_to_its_privacy() {
    use crate::storage::AssistantPreferences;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
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
fn switching_vendor_keeps_each_vendors_sharing_and_model() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    app.preferences
        .assistant
        .models
        .insert(Provider::Claude, "sonnet".into());
    scripted_turn(&mut app, &ctx, &dir);
    let sharing = |app: &App| {
        app.preferences
            .assistant
            .sharing_for(&app.database.tools.database_scope)
    };
    assert!(app.database.assistant.host().is_some());
    app.select_assistant_provider(Provider::Codex);
    assert!(app.database.assistant.host().is_none());
    assert!(app.database.assistant.entries.is_empty());
    assert_eq!(sharing(&app), Sharing::Rows);
    assert_eq!(app.preferences.assistant.selected_model(), "");
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(app.preferences.assistant.selected_model(), "sonnet");
    assert_eq!(sharing(&app), Sharing::Schema);
    app.close();
}

#[test]
fn agent_scan_reports_status_and_missing_executables_without_starting_a_conversation() {
    use tursobench_assistant::vendor::VENDORS;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    let executables = &mut app.preferences.assistant.executables;
    for vendor in VENDORS {
        executables.insert(vendor.id, dir.path().join("not-installed"));
    }
    let script = install(
        &dir,
        "my claude",
        "#!/usr/bin/env bash\nif [[ $1 == --version ]]; then echo 'Claude test'; else echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\",\"email\":\"private@example.com\"}'; fi\n",
    );
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
    assert!(
        !serde_json::to_string(&app.preferences)
            .unwrap()
            .contains("private@example.com")
    );
    app.close();
}

#[test]
fn one_picker_changes_agent_model_and_favourites_with_each_agents_sharing() {
    use crate::app::assistant_picker::menu_id;
    use crate::storage::FavouriteModel;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    app.set_assistant_shown(true);
    scripted_turn(&mut app, &ctx, &dir);
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(
        value_of(&output, "Assistant model picker").as_deref(),
        Some("Claude Code")
    );
    tap(
        &mut app,
        &ctx,
        control(&output, "Assistant model picker").center(),
    );
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    tap(&mut app, &ctx, control(&output, "Sonnet").center());
    assert_eq!(app.preferences.assistant.selected_model(), "sonnet");
    assert!(app.database.assistant.host().is_none());
    // The picker stays open so the reasoning level can follow the model.
    assert!(egui::Popup::is_id_open(&ctx, menu_id()));
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
    assert_eq!(
        app.preferences
            .assistant
            .sharing_for(&app.database.tools.database_scope),
        Sharing::Rows
    );
    let escape = super::key_event(egui::Key::Escape, true, egui::Modifiers::NONE);
    frame_at(&mut app, &ctx, SIZE, vec![escape]);
    assert!(!egui::Popup::is_id_open(&ctx, menu_id()));
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert!(labelled(&output, "Share the schema and query results"));
    assert_eq!(
        value_of(&output, "Assistant model picker").as_deref(),
        Some("Test model")
    );
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(app.preferences.assistant.selected_model(), "opus");
    app.close();
}

#[test]
fn only_the_selected_vendors_account_change_stops_chat_and_blocks_new_turns() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    scripted_turn(&mut app, &ctx, &dir);
    let helper = install(&dir, "waiting-helper", WAITING);
    for provider in [Provider::Codex, Provider::Claude] {
        app.preferences
            .assistant
            .executables
            .insert(provider, helper.clone());
    }
    let cancel = |app: &mut App, provider| {
        app.accounts.rows[&provider].job.as_ref().unwrap().cancel();
    };

    // Signing in to another vendor leaves the Claude Code chat running.
    app.agent_account_job(&ctx, Provider::Codex, Operation::Login { method: None });
    assert!(!app.assistant_account_pending());
    assert!(app.database.assistant.host().is_some());
    let entries = app.database.assistant.entries.len();
    converse(&mut app, &ctx, "Still here");
    assert!(app.database.assistant.entries.len() > entries);
    cancel(&mut app, Provider::Codex);
    wait_for_account(&mut app, &ctx, Provider::Codex);
    assert!(app.database.assistant.host().is_some());

    // Signing out of the selected vendor ends its chat and holds new turns.
    app.agent_account_job(&ctx, Provider::Claude, Operation::Logout);
    assert!(app.assistant_account_pending());
    assert!(app.database.assistant.host().is_none());
    app.database.assistant.draft = "Wait for account change".into();
    app.send_assistant(&ctx);
    assert!(app.database.assistant.host().is_none());
    assert!(app.database.assistant.entries.is_empty());
    cancel(&mut app, Provider::Claude);
    wait_until(&mut app, &ctx, |a| !a.assistant_account_pending());
    assert_eq!(app.database.assistant.draft, "Wait for account change");
    app.close();
}

#[test]
fn the_panel_checks_the_selected_vendor_once_without_opening_settings() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
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
fn cancelling_or_failing_a_sign_in_keeps_the_last_status_and_models() {
    use crate::app::presentation::Intent;
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    let waiting = install(&dir, "waiting-codex", WAITING);
    let failing = install(&dir, "failing-codex", "#!/bin/sh\nexit 3\n");
    app.accounts.rows.entry(Provider::Codex).or_default().status = Some(Status {
        state: SignIn::SignedOut,
        models: vec![model("gpt-test", "Test model")],
        ..Default::default()
    });
    let sign_in = |app: &mut App, program: PathBuf| {
        let executables = &mut app.preferences.assistant.executables;
        executables.insert(Provider::Codex, program);
        app.agent_account_job(&ctx, Provider::Codex, Operation::Login { method: None });
    };
    let still_known = |app: &App, described| {
        let status = status(app, Provider::Codex);
        assert_eq!(status.state, SignIn::SignedOut);
        assert_eq!(status.models[0].id, "gpt-test");
        assert_eq!(
            app.accounts.rows[&Provider::Codex].describe(),
            Some(described)
        );
    };

    sign_in(&mut app, waiting);
    app.accounts.rows[&Provider::Codex]
        .job
        .as_ref()
        .unwrap()
        .cancel();
    wait_for_account(&mut app, &ctx, Provider::Codex);
    still_known(&app, ("Signed out", Intent::Pending));

    sign_in(&mut app, failing);
    wait_for_account(&mut app, &ctx, Provider::Codex);
    still_known(&app, ("Sign-in failed", Intent::Danger));
    app.close();
}

#[test]
fn copy_buttons_write_the_reply_and_conversation_in_the_chosen_format() {
    use crate::storage::CopyFormat;
    let (_dir, ctx, mut app) = fixture(TABLE, false);
    app.set_assistant_shown(true);
    app.database.assistant.entries =
        vec![Entry::User("Hi".into()), Entry::Reply("**Done**".into())];
    let copied = |app: &mut App, name: &str| -> Vec<String> {
        frame_at(app, &ctx, SIZE, vec![]);
        let output = frame_at(app, &ctx, SIZE, vec![]);
        let pos = control(&output, name).center();
        let press = |down| {
            vec![
                egui::Event::PointerMoved(pos),
                super::pointer_button(pos, down),
            ]
        };
        frame_at(app, &ctx, SIZE, press(true));
        frame_at(app, &ctx, SIZE, press(false))
            .platform_output
            .commands
            .into_iter()
            .filter_map(|c| match c {
                egui::OutputCommand::CopyText(text) => Some(text),
                _ => None,
            })
            .collect()
    };
    assert_eq!(copied(&mut app, "Copy last reply"), ["**Done**"]);
    assert_eq!(
        copied(&mut app, "Copy conversation"),
        ["## You\n\nHi\n\n## Claude Code\n\n**Done**"]
    );
    app.preferences.assistant.copy_format = CopyFormat::Html;
    assert_eq!(
        copied(&mut app, "Copy last reply"),
        ["<p><strong>Done</strong></p>"]
    );
    assert_eq!(
        copied(&mut app, "Copy conversation"),
        ["<h2>You</h2>\n<p>Hi</p>\n<h2>Claude Code</h2>\n<p><strong>Done</strong></p>"]
    );
    app.close();
}

#[test]
fn the_reasoning_level_changes_mid_conversation_and_follows_each_codex_model() {
    let (dir, ctx, mut app) = fixture(TABLE, false);
    runtime_dir(&dir);
    // A Claude stand-in that records what it is sent.
    let log = dir.path().join("sent.log");
    let script = format!(
        "#!/usr/bin/env bash\nwhile read -r line; do\n  printf '%s\\n' \"$line\" >> '{}'\n  case \"$line\" in *'\"type\":\"user\"'*) echo '{{\"type\":\"result\",\"subtype\":\"success\",\"is_error\":false,\"result\":\"ok\"}}';; esac\ndone\n",
        log.display()
    );
    app.database.assistant.program = Some(install(&dir, "claude", &script));
    app.set_assistant_shown(true);
    converse(&mut app, &ctx, "Hello");
    press(&mut app, &ctx, "Assistant model picker");
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    let level = |output: &egui::FullOutput| value_of(output, "Assistant reasoning level");
    assert_eq!(level(&output).as_deref(), Some("Default"));
    tap(
        &mut app,
        &ctx,
        control(&output, "Reasoning Extra high").center(),
    );
    let output = frame_at(&mut app, &ctx, SIZE, vec![]);
    assert_eq!(
        value_of(&output, "Assistant model picker").as_deref(),
        Some("Claude Code · Extra high")
    );
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
    app.select_assistant_effort(String::new());
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
    tap(&mut app, &ctx, control(&output, "Reasoning Ultra").center());
    assert_eq!(app.preferences.assistant.provider, Provider::Claude);
    assert_eq!(app.preferences.assistant.efforts[&Provider::Codex], "ultra");
    app.select_assistant_provider(Provider::Codex);
    assert_eq!(app.assistant_effort().as_deref(), Some("ultra"));
    // A model without that level falls back to the agent's own.
    app.select_assistant_model("quick".into());
    assert_eq!(app.preferences.assistant.selected_effort(), "ultra");
    assert_eq!(app.assistant_effort(), None);
    // Claude keeps its own choice.
    app.select_assistant_provider(Provider::Claude);
    assert_eq!(app.preferences.assistant.selected_effort(), "");
    app.close();
}
