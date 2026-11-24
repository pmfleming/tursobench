//! The installed binary's `--mcp-relay` mode carries MCP between an agent's
//! stdio and the running app's private socket.
//!
//! The ignored tests run the user's real, signed-in Claude Code, Codex and
//! Antigravity CLIs through that relay, with one synthetic tool and no
//! database. They use a little of the user's plan, so they only run on request:
//! `cargo test -p tursobench-ui --test relay -- --ignored [claude|codex|antigravity]`
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update},
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::{self, Endpoint, Launch},
    vendor::{resolve, search_dirs},
};

/// Upper-cases text, counting its calls.
#[derive(Default)]
struct Upper(AtomicUsize);
impl ToolHost for Upper {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "upper",
            description: "Upper-case the given text",
            input_schema: json!({
                "type": "object",
                "properties": {"text": {"type": "string"}},
                "required": ["text"],
            }),
        }]
    }
    fn call(&self, _: &str, arguments: &Value) -> ToolOutput {
        self.0.fetch_add(1, Ordering::SeqCst);
        let text = arguments["text"].as_str().unwrap_or_default();
        ToolOutput::ok(text.to_uppercase())
    }
}

/// An endpoint in a private temporary directory.
fn endpoint() -> (tempfile::TempDir, Endpoint) {
    let runtime = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_tursobench").into();
    let endpoint = Endpoint::start_in(runtime.path(), Arc::new(Upper::default()), exe).unwrap();
    (runtime, endpoint)
}

/// The relay process for `launch`, with piped stdio and the environment `env` sets.
fn relay(launch: &Launch, env: impl FnOnce(&mut Command)) -> Child {
    let mut command = Command::new(&launch.command);
    command.args(&launch.args);
    env(&mut command);
    command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap()
}

/// Agents wait for each reply with their input still open, and start MCP
/// servers with a bare environment. Without the token the relay gets nothing.
#[test]
fn relay_process_answers_each_request_on_the_app_socket() {
    let (_runtime, endpoint) = endpoint();
    let launch = endpoint.launch().clone();
    let mut child = relay(&launch, |c| {
        c.env_clear()
            .env("PATH", "/usr/bin:/bin")
            .envs(launch.env.iter().cloned());
    });
    let mut stdin = child.stdin.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    let stdout = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines() {
            let Ok(line) = line else { return };
            if tx.send(line).is_err() {
                return;
            }
        }
    });
    let mut send = |message: Value| writeln!(stdin, "{message}").unwrap();
    let reply = |id: u64| -> Value {
        let line = rx
            .recv_timeout(Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("no reply to request {id} while input is open"));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], id);
        value
    };
    let initialize = json!({"protocolVersion": "2025-06-18"});
    send(json!({"jsonrpc": "2.0", "id": 0, "method": "initialize", "params": initialize}));
    assert_eq!(reply(0)["result"]["serverInfo"]["name"], "tursobench");
    send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}));
    send(json!({"jsonrpc": "2.0", "id": 1, "method": "tools/list"}));
    assert_eq!(reply(1)["result"]["tools"][0]["name"], "upper");
    let call = json!({"name": "upper", "arguments": {"text": "turso"}});
    send(json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": call}));
    assert_eq!(reply(2)["result"]["content"][0]["text"], "TURSO");
    drop(stdin);
    assert!(child.wait().unwrap().success());

    let mut refused = relay(&launch, |c| {
        c.env("TURSOBENCH_MCP_TOKEN", "wrong");
    });
    writeln!(
        refused.stdin.take().unwrap(),
        r#"{{"jsonrpc":"2.0","id":1,"method":"ping"}}"#
    )
    .unwrap();
    let mut out = String::new();
    BufReader::new(refused.stdout.take().unwrap())
        .read_line(&mut out)
        .unwrap();
    assert_eq!(out, "");
    let _ = refused.wait();
}

const INSTRUCTIONS: &str = "You test tools. Do exactly what you are asked.";

/// Serves `upper` from this user's private directory to the agent `spec`
/// describes, asks for one call, and checks the call, its result and the reply.
fn assert_calls_upper(spec: Spec) {
    let upper = Arc::new(Upper::default());
    let program = env!("CARGO_BIN_EXE_tursobench").into();
    let endpoint = Endpoint::start(upper.clone(), program).unwrap();
    let spec = Spec {
        cwd: relay::private_dir().unwrap(),
        ..spec
    };
    let agent = Agent::spawn(spec.serve(&endpoint, &["upper"]), Arc::new(|| {})).unwrap();
    agent.prompt(
        "Call the upper tool once with the text 'turso bench', then reply with only its output.",
    );
    let mut seen = vec![];
    while !matches!(
        seen.last(),
        Some(Update::TurnEnded { .. } | Update::Exited { .. })
    ) {
        let update = agent.updates.recv_timeout(Duration::from_secs(120));
        seen.push(update.expect("an update"));
    }
    println!("{seen:#?}");
    assert!(
        seen.iter()
            .any(|u| matches!(u, Update::ToolCall { name, .. } if name == "upper"))
    );
    assert!(
        seen.iter().any(
            |u| matches!(u, Update::ToolResult { ok: true, text, .. } if text == "TURSO BENCH")
        )
    );
    assert!(matches!(
        seen.last(),
        Some(Update::TurnEnded { error: None })
    ));
    assert_eq!(upper.0.load(Ordering::SeqCst), 1);
    let reply = seen.iter().filter_map(|u| match u {
        Update::Text(text) => Some(text.as_str()),
        _ => None,
    });
    assert!(reply.collect::<String>().contains("TURSO BENCH"));
}

#[test]
#[ignore = "uses the signed-in claude CLI and its plan"]
fn signed_in_claude_calls_relay_tools_with_built_in_tools_removed() {
    assert_calls_upper(Spec {
        program: "claude".into(),
        system_prompt: INSTRUCTIONS.into(),
        model: Some("haiku".into()),
        effort: Some("low".into()),
        ..Spec::default()
    });
}

#[test]
#[ignore = "uses the signed-in codex CLI and its plan"]
fn signed_in_codex_calls_relay_tools_in_a_read_only_thread() {
    assert_calls_upper(Spec {
        program: "codex".into(),
        protocol: Protocol::CodexAppServer,
        system_prompt: INSTRUCTIONS.into(),
        effort: Some("low".into()),
        ..Spec::default()
    });
}

#[test]
#[ignore = "uses the signed-in Antigravity CLI and its plan"]
fn signed_in_antigravity_calls_relay_tools() {
    assert_calls_upper(Spec {
        program: resolve(std::path::Path::new("agy"), &search_dirs()).unwrap(),
        args: vec!["acp".into(), "--disable-telemetry".into()],
        protocol: Protocol::Acp,
        system_prompt: "You test tools. Use only the supplied TursoBench MCP tool.".into(),
        ..Spec::default()
    });
}
