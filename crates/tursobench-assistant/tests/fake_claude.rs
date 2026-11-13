//! Drive a scripted stand-in for `claude -p` through a whole conversation.
use std::{sync::Arc, time::Duration};
use tursobench_assistant::agent::{Agent, Protocol, Spec, Update};

const SCRIPT: &str = r#"
read -r first
case "$first" in *'"type":"user"'*) ;; *) echo "bad prompt: $first" >&2; exit 3;; esac
echo '{"type":"system","subtype":"init","model":"fake-model"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}},"parent_tool_use_id":null}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__tursobench__list_tables","input":{}}]},"parent_tool_use_id":null}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"[]"}]},"parent_tool_use_id":null}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Hello"}'
read -r second
read -r interrupt
case "$interrupt" in *'"subtype":"interrupt"'*) ;; *) exit 4;; esac
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"interrupted"}'
echo 'farewell' >&2
exit 0
"#;

fn next(agent: &Agent) -> Update {
    agent
        .updates
        .recv_timeout(Duration::from_secs(10))
        .expect("an update before the timeout")
}

#[test]
fn a_scripted_agent_streams_a_turn_accepts_an_interrupt_and_exits() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("claude.sh");
    std::fs::write(&script, SCRIPT).unwrap();
    let agent = Agent::spawn(
        Spec {
            program: "bash".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: vec![],
            cwd: dir.path().into(),
            protocol: Protocol::ClaudeStream,
            system_prompt: String::new(),
            mcp: None,
            model: None,
        },
        Arc::new(|| {}),
    )
    .unwrap();
    agent.prompt("What tables are there?");
    assert_eq!(
        next(&agent),
        Update::Ready {
            model: Some("fake-model".into())
        }
    );
    assert_eq!(next(&agent), Update::Text("Hel".into()));
    assert_eq!(next(&agent), Update::Text("lo".into()));
    assert!(matches!(next(&agent), Update::ToolCall { name, .. } if name == "list_tables"));
    assert!(matches!(next(&agent), Update::ToolResult { ok: true, text, .. } if text == "[]"));
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
    agent.prompt("Count to a million");
    agent.cancel();
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
    assert_eq!(
        next(&agent),
        Update::Exited {
            message: "farewell".into()
        }
    );
}

#[test]
fn a_missing_program_is_reported_at_spawn() {
    let error = Agent::spawn(
        Spec {
            program: "/nonexistent/claude".into(),
            args: vec![],
            env: vec![],
            cwd: std::env::temp_dir(),
            protocol: Protocol::ClaudeStream,
            system_prompt: String::new(),
            mcp: None,
            model: None,
        },
        Arc::new(|| {}),
    )
    .err()
    .unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}
