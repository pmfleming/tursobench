//! Drive a scripted ACP agent: handshake, MCP setup, streaming, a declined
//! permission request and a refused file-system request.
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update},
    relay::Launch,
};

const SCRIPT: &str = r#"
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      case "$line" in *'"terminal":false'*) ;; *) echo "terminal advertised" >&2; exit 5;; esac
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"protocolVersion":1,"agentCapabilities":{},"authMethods":[]}}';;
    *'"method":"session/new"'*)
      case "$line" in *'--mcp-relay'*'"name":"tursobench"'*) ;; *) echo "no tool server: $line" >&2; exit 6;; esac
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"sessionId":"s1","models":{"currentModelId":"fake-small","availableModels":[{"modelId":"fake-small"},{"modelId":"fake-large"}]}}}';;
    *'"method":"session/set_model"'*)
      case "$line" in *'"modelId":"fake-large"'*) ;; *) exit 9;; esac
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{}}';;
    *'"method":"session/prompt"'*)
      case "$line" in *tursobench-instructions*'Be helpful'*) ;; *) echo "no instructions" >&2; exit 10;; esac
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"Hi"}}}}'
      echo '{"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"x","title":"Run shell: ls"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
      read -r answer
      case "$answer" in *'"optionId":"no"'*) ;; *) echo "permission not declined: $answer" >&2; exit 7;; esac
      echo '{"jsonrpc":"2.0","id":901,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"y","title":"tursobench/list_tables"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
      read -r answer
      case "$answer" in *'"optionId":"yes"'*) ;; *) echo "own tool not allowed: $answer" >&2; exit 11;; esac
      echo '{"jsonrpc":"2.0","method":"session/update","params":{"sessionId":"s1","update":{"sessionUpdate":"tool_call","toolCallId":"y","title":"tursobench/list_tables","status":"completed","content":[{"type":"content","content":{"type":"text","text":"[]"}}]}}}'
      echo '{"jsonrpc":"2.0","id":902,"method":"fs/read_text_file","params":{"sessionId":"s1","path":"/etc/hostname"}}'
      read -r answer
      case "$answer" in *'-32601'*) ;; *) echo "fs allowed: $answer" >&2; exit 8;; esac
      echo '{"jsonrpc":"2.0","id":'"$id"',"result":{"stopReason":"end_turn"}}';;
  esac
done
"#;

fn next(agent: &Agent) -> Update {
    agent
        .updates
        .recv_timeout(Duration::from_secs(10))
        .expect("an update before the timeout")
}

#[test]
fn a_scripted_acp_agent_gets_tools_but_no_shell_files_or_other_permissions() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("agent.sh");
    std::fs::write(&script, SCRIPT).unwrap();
    let agent = Agent::spawn(
        Spec {
            program: "bash".into(),
            args: vec![script.to_string_lossy().into_owned()],
            env: vec![],
            cwd: dir.path().into(),
            protocol: Protocol::Acp,
            system_prompt: "Be helpful".into(),
            mcp: Some(Launch {
                command: "/usr/bin/tursobench".into(),
                args: vec!["--mcp-relay".into(), "/run/x.sock".into()],
                env: vec![("TURSOBENCH_MCP_TOKEN".into(), "t".into())],
            }),
            model: Some("fake-large".into()),
        },
        Arc::new(|| {}),
    )
    .unwrap();
    // A prompt sent before the session exists waits for it.
    agent.prompt("Hello");
    assert_eq!(
        next(&agent),
        Update::Ready {
            model: Some("fake-large".into())
        }
    );
    assert_eq!(next(&agent), Update::Text("Hi".into()));
    assert!(matches!(next(&agent), Update::Notice(text) if text.contains("Run shell: ls")));
    assert!(matches!(next(&agent), Update::ToolCall { name, .. } if name == "list_tables"));
    assert!(matches!(next(&agent), Update::ToolResult { ok: true, text, .. } if text == "[]"));
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
}
