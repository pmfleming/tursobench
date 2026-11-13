//! The installed binary's `--mcp-relay` mode carries MCP between an agent's
//! stdio and the running app's private socket.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Command, Stdio},
    sync::Arc,
};
use tursobench_assistant::{
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::Endpoint,
};

struct Upper;
impl ToolHost for Upper {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "upper",
            description: "Upper-case the text",
            input_schema: json!({"type": "object"}),
        }]
    }
    fn call(&self, _: &str, arguments: &Value) -> ToolOutput {
        ToolOutput::ok(
            arguments["text"]
                .as_str()
                .unwrap_or_default()
                .to_uppercase(),
        )
    }
}

#[test]
fn relay_process_carries_mcp_to_the_app_socket() {
    let runtime = tempfile::tempdir().unwrap();
    // SAFETY: set before this test starts any thread that reads the environment.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", runtime.path()) };
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_tursobench"));
    let endpoint = Endpoint::start(Arc::new(Upper), exe).unwrap();
    let launch = endpoint.launch().clone();
    let mut child = Command::new(&launch.command)
        .args(&launch.args)
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":1,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#
    )
    .unwrap();
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{{"name":"upper","arguments":{{"text":"turso"}}}}}}"#
    )
    .unwrap();
    drop(stdin);
    let lines: Vec<Value> = BufReader::new(child.stdout.take().unwrap())
        .lines()
        .map(|l| serde_json::from_str(&l.unwrap()).unwrap())
        .collect();
    assert!(child.wait().unwrap().success());
    assert_eq!(lines[0]["result"]["serverInfo"]["name"], "tursobench");
    assert_eq!(lines[1]["result"]["content"][0]["text"], "TURSO");

    // Without the token the relay is refused and gets nothing back.
    let mut refused = Command::new(&launch.command)
        .args(&launch.args)
        .env("TURSOBENCH_MCP_TOKEN", "wrong")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
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
