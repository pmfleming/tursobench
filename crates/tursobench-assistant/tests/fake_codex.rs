//! Drive a scripted `codex app-server`: handshake, an ephemeral read-only
//! thread carrying only TursoBench's tools, streaming, declined approvals, a
//! refused token request and an interrupted turn.
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    agent::{Agent, Update},
    codex,
    relay::Launch,
};

const SCRIPT: &str = r#"
has() { printf '%s' "$line" | grep -qF -- "$1" || { echo "missing $1 in: $line" >&2; exit 3; }; }
while read -r line; do
  id=$(printf '%s' "$line" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$line" in
    *'"method":"initialize"'*)
      echo '{"id":'"$id"',"result":{"userAgent":"fake"}}'
      read -r line; has '"method":"initialized"';;
    *'"method":"thread/start"'*)
      for want in '"ephemeral":true' '"sandbox":"read-only"' '"approvalPolicy":"untrusted"' \
                  '"baseInstructions":"Be helpful"' '"model":"gpt-fake"' '--mcp-relay' \
                  '"TURSOBENCH_MCP_TOKEN":"t"' '"mcp_servers":{"tursobench"'; do has "$want"; done
      echo '{"id":'"$id"',"result":{"thread":{"id":"th1"},"model":"gpt-fake"}}';;
    *'"method":"turn/start"'*'"text":"Hello"'*)
      has '"threadId":"th1"'
      echo '{"id":'"$id"',"result":{"turn":{"id":"tu1","status":"inProgress"}}}'
      echo '{"method":"turn/started","params":{"threadId":"th1","turn":{"id":"tu1"}}}'
      echo '{"method":"item/agentMessage/delta","params":{"threadId":"th1","turnId":"tu1","itemId":"m1","delta":"Hi"}}'
      echo '{"id":900,"method":"item/commandExecution/requestApproval","params":{"threadId":"th1","turnId":"tu1","itemId":"x","command":"cat /etc/hostname"}}'
      read -r line; has '"id":900'; has '"decision":"decline"'
      echo '{"id":901,"method":"account/chatgptAuthTokens/refresh","params":{}}'
      read -r line; has '"id":901'; has '-32601'
      echo '{"method":"item/started","params":{"item":{"type":"mcpToolCall","id":"c1","server":"tursobench","tool":"list_tables","status":"inProgress","arguments":{}}}}'
      echo '{"method":"item/completed","params":{"item":{"type":"mcpToolCall","id":"c1","server":"tursobench","tool":"list_tables","status":"completed","arguments":{},"result":{"content":[{"type":"text","text":"[]"}],"isError":false},"error":null}}}'
      echo '{"method":"turn/completed","params":{"threadId":"th1","turn":{"id":"tu1","status":"completed","error":null}}}';;
    *'"method":"turn/start"'*'"text":"Long"'*)
      echo '{"id":'"$id"',"result":{"turn":{"id":"tu2","status":"inProgress"}}}'
      read -r line; has '"method":"turn/interrupt"'; has '"turnId":"tu2"'
      echo '{"method":"turn/completed","params":{"threadId":"th1","turn":{"id":"tu2","status":"interrupted","error":null}}}';;
    *'"method":"turn/interrupt"'*) ;;
    *) echo "unexpected: $line" >&2; exit 4;;
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
fn a_scripted_codex_gets_only_our_tools_and_no_approvals_or_tokens() {
    let dir = tempfile::tempdir().unwrap();
    let script = dir.path().join("codex.sh");
    std::fs::write(&script, SCRIPT).unwrap();
    let mut spec = codex::spec(
        "bash".into(),
        dir.path().into(),
        Launch {
            command: "/usr/bin/tursobench".into(),
            args: vec!["--mcp-relay".into(), "/run/x.sock".into()],
            env: vec![("TURSOBENCH_MCP_TOKEN".into(), "t".into())],
        },
        "Be helpful",
        Some("gpt-fake"),
    );
    spec.args = vec![script.to_string_lossy().into_owned()];
    let agent = Agent::spawn(spec, Arc::new(|| {})).unwrap();
    // A prompt sent before the thread exists waits for it.
    agent.prompt("Hello");
    assert_eq!(
        next(&agent),
        Update::Ready {
            model: Some("gpt-fake".into())
        }
    );
    assert_eq!(next(&agent), Update::Text("Hi".into()));
    assert!(matches!(next(&agent), Update::Notice(t) if t.contains("cat /etc/hostname")));
    assert!(matches!(next(&agent), Update::ToolCall { name, .. } if name == "list_tables"));
    assert!(matches!(next(&agent), Update::ToolResult { ok: true, text, .. } if text == "[]"));
    assert_eq!(next(&agent), Update::TurnEnded { error: None });

    agent.prompt("Long");
    // Stop before the turn id is known: the interrupt follows it.
    agent.cancel();
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
}
