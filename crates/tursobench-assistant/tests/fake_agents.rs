//! Drive scripted stand-ins for each protocol through whole conversations:
//! handshakes, MCP setup, streaming, declined permissions and refused requests,
//! models and reasoning levels, interrupts and exits.
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update},
    relay::Launch,
};

fn next(agent: &Agent) -> Update {
    agent
        .updates
        .recv_timeout(Duration::from_secs(10))
        .expect("an update before the timeout")
}

/// Run `script` with `interpreter` as `spec` describes, and send `prompt` at
/// once: a prompt sent before the conversation exists waits for it.
fn start(interpreter: &str, script: &str, spec: Spec, prompt: &str) -> (tempfile::TempDir, Agent) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("agent");
    std::fs::write(&path, script).unwrap();
    let spec = Spec {
        program: interpreter.into(),
        args: vec![path.to_string_lossy().into_owned()],
        cwd: dir.path().into(),
        ..spec
    };
    let agent = Agent::spawn(spec, Arc::new(|| {})).unwrap();
    agent.prompt(prompt);
    (dir, agent)
}

/// A conversation TursoBench serves its tools to.
fn served(protocol: Protocol) -> Spec {
    Spec {
        protocol,
        system_prompt: "Be helpful".into(),
        mcp: Some(Launch {
            command: "/usr/bin/tursobench".into(),
            args: vec!["--mcp-relay".into(), "/run/x.sock".into()],
            env: vec![("TURSOBENCH_MCP_TOKEN".into(), "t".into())],
        }),
        ..Spec::default()
    }
}

fn assert_ready(agent: &Agent, model: &str) {
    assert_eq!(
        next(agent),
        Update::Ready {
            model: Some(model.into())
        }
    );
}

/// The updates of a turn that reads a table through TursoBench's tools.
fn assert_tool_turn(agent: &Agent) {
    assert!(matches!(next(agent), Update::ToolCall { name, .. } if name == "list_tables"));
    assert!(matches!(next(agent), Update::ToolResult { ok: true, text, .. } if text == "[]"));
    assert_eq!(next(agent), Update::TurnEnded { error: None });
}

const ACP: &str = r#"
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
      echo '{"jsonrpc":"2.0","id":900,"method":"session/request_permission","params":{"sessionId":"s1","toolCall":{"toolCallId":"x","title":"Run shell: touch /tmp/tursobench-probe"},"options":[{"optionId":"yes","name":"Allow","kind":"allow_once"},{"optionId":"no","name":"Reject","kind":"reject_once"}]}}'
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

#[test]
fn a_scripted_acp_agent_gets_tools_but_no_shell_files_or_other_permissions() {
    let spec = Spec {
        model: Some("fake-large".into()),
        ..served(Protocol::Acp)
    };
    let (_dir, agent) = start("bash", ACP, spec, "Hello");
    assert_ready(&agent, "fake-large");
    assert_eq!(next(&agent), Update::Text("Hi".into()));
    assert!(
        matches!(next(&agent), Update::Notice(text) if text.contains("Run shell: touch /tmp/tursobench-probe"))
    );
    assert_tool_turn(&agent);
}

const ACP_SETTINGS: &str = r#"
import json,sys,select,os
def read_line():
 data=b''
 while not data.endswith(b'\n'):
  b=os.read(0,1)
  if not b: return ''
  data+=b
 return data.decode()
def send(v): print(json.dumps(v),flush=True)
efforts=[];prompts=0;unacked=[]
for line in iter(read_line, ''):
 m=json.loads(line);method=m['method'];i=m.get('id')
 if method=='initialize': send({'id':i,'result':{'protocolVersion':1}})
 elif method=='session/new': send({'id':i,'result':{'sessionId':'s','configOptions':[{'id':'mode','category':'mode','currentValue':'auto','options':[{'value':'default'}]},{'id':'model','category':'model','currentValue':'small','options':[{'name':'Group','options':[{'value':'large','name':'Large'}]}]},{'id':'effort','category':'thought_level','currentValue':'medium','options':[{'value':'low'},{'value':'medium'},{'value':'high','name':'High'}]}]}})
 elif method=='session/set_config_option':
  config=m['params']['configId'];value=m['params']['value']
  if config=='mode': assert value=='default'
  elif config=='model': assert value=='large'
  else: efforts.append(value)
  # Acknowledge once the client goes quiet; a prompt before then is an error.
  unacked.append(i)
  if not select.select([sys.stdin],[],[],0.1)[0]:
   for j in unacked: send({'id':j,'result':{}})
   unacked=[]
 elif method=='session/prompt':
  assert not unacked, 'prompt sent before the settings were acknowledged'
  prompts+=1
  assert efforts==(['high'] if prompts==1 else ['high','medium']), efforts
  send({'id':i,'result':{'stopReason':'end_turn'}})
 else: raise Exception('unexpected '+str(method))
"#;

#[test]
fn acp_applies_permission_mode_grouped_model_and_effort_before_sending_prompts() {
    let spec = Spec {
        protocol: Protocol::Acp,
        model: Some("large".into()),
        effort: Some("high".into()),
        ..Spec::default()
    };
    let (_dir, agent) = start("python3", ACP_SETTINGS, spec, "Hello");
    assert_ready(&agent, "large");
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
    // Back to the agent's own level: the session's first value is set again.
    agent.set_effort(None);
    agent.prompt("Again");
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
}

const CLAUDE: &str = r#"
read -r settings
case "$settings" in *'"subtype":"get_settings"'*) ;; *) echo "no settings read: $settings" >&2; exit 5;; esac
echo '{"type":"control_response","response":{"subtype":"success","request_id":"tursobench-settings","response":{"applied":{"effort":"high"}}}}'
read -r first
case "$first" in *'"type":"user"'*) ;; *) echo "bad prompt: $first" >&2; exit 3;; esac
echo '{"type":"system","subtype":"init","model":"fake-model"}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"Hel"}},"parent_tool_use_id":null}'
echo '{"type":"stream_event","event":{"type":"content_block_delta","delta":{"type":"text_delta","text":"lo"}},"parent_tool_use_id":null}'
echo '{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t1","name":"mcp__tursobench__list_tables","input":{}}]},"parent_tool_use_id":null}'
echo '{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":"[]"}]},"parent_tool_use_id":null}'
echo '{"type":"result","subtype":"success","is_error":false,"result":"Hello"}'
read -r change
case "$change" in *'"effortLevel":"max"'*) ;; *) echo "bad change: $change" >&2; exit 6;; esac
case "$change" in *'"subtype":"apply_flag_settings"'*) ;; *) exit 6;; esac
echo '{"type":"control_response","response":{"subtype":"error","request_id":"tursobench-effort-1","error":"max is not available"}}'
read -r restore
case "$restore" in *'"effortLevel":"high"'*) ;; *) echo "bad restore: $restore" >&2; exit 7;; esac
read -r second
read -r interrupt
case "$interrupt" in *'"subtype":"interrupt"'*) ;; *) exit 4;; esac
echo '{"type":"result","subtype":"error_during_execution","is_error":true,"terminal_reason":"interrupted"}'
echo 'farewell' >&2
exit 0
"#;

#[test]
fn a_scripted_claude_streams_a_turn_accepts_an_interrupt_and_exits() {
    let (_dir, agent) = start("bash", CLAUDE, Spec::default(), "What tables are there?");
    assert_ready(&agent, "fake-model");
    assert_eq!(next(&agent), Update::Text("Hel".into()));
    assert_eq!(next(&agent), Update::Text("lo".into()));
    assert_tool_turn(&agent);
    // A refused level is reported; the default restores the level read at start.
    agent.set_effort(Some("max".into()));
    assert_eq!(
        next(&agent),
        Update::Notice("Claude Code kept its reasoning level: max is not available".into())
    );
    agent.set_effort(None);
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
    let spec = Spec {
        program: "/nonexistent/claude".into(),
        cwd: std::env::temp_dir(),
        ..Spec::default()
    };
    let error = Agent::spawn(spec, Arc::new(|| {})).err().unwrap();
    assert_eq!(error.kind(), std::io::ErrorKind::NotFound);
}

const CODEX: &str = r#"
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
      echo '{"id":'"$id"',"result":{"thread":{"id":"th1"},"model":"gpt-fake","reasoningEffort":"low"}}';;
    *'"method":"turn/start"'*'"text":"Hello"'*)
      has '"threadId":"th1"'
      has '"effort":"high"'
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
      has '"effort":"low"'
      echo '{"id":'"$id"',"result":{"turn":{"id":"tu2","status":"inProgress"}}}'
      read -r line; has '"method":"turn/interrupt"'; has '"turnId":"tu2"'
      echo '{"method":"turn/completed","params":{"threadId":"th1","turn":{"id":"tu2","status":"interrupted","error":null}}}';;
    *'"method":"turn/interrupt"'*) ;;
    *) echo "unexpected: $line" >&2; exit 4;;
  esac
done
"#;

#[test]
fn a_scripted_codex_gets_only_our_tools_and_no_approvals_or_tokens() {
    let spec = Spec {
        model: Some("gpt-fake".into()),
        effort: Some("high".into()),
        ..served(Protocol::CodexAppServer)
    };
    let (_dir, agent) = start("bash", CODEX, spec, "Hello");
    assert_ready(&agent, "gpt-fake");
    assert_eq!(next(&agent), Update::Text("Hi".into()));
    assert!(matches!(next(&agent), Update::Notice(t) if t.contains("cat /etc/hostname")));
    assert_tool_turn(&agent);

    // Back to the agent's own level: the thread's original is sent again.
    agent.set_effort(None);
    agent.prompt("Long");
    // Stop before the turn id is known: the interrupt follows it.
    agent.cancel();
    assert_eq!(next(&agent), Update::TurnEnded { error: None });
}
