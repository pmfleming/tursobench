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

/// Agents wait for each reply with their input still open, and start MCP
/// servers with a bare environment.
#[test]
fn relay_answers_each_request_while_the_agent_waits() {
    let runtime = tempfile::tempdir().unwrap();
    // SAFETY: set before this test starts any thread that reads the environment.
    unsafe { std::env::set_var("XDG_RUNTIME_DIR", runtime.path()) };
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_tursobench"));
    let endpoint = Endpoint::start(Arc::new(Upper), exe).unwrap();
    let launch = endpoint.launch().clone();
    let mut child = Command::new(&launch.command)
        .args(&launch.args)
        .env_clear()
        .env("PATH", "/usr/bin:/bin")
        .envs(launch.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
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
    let reply = |id: u64| -> Value {
        let line = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("no reply to request {id} while input is open"));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], id);
        value
    };
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","id":0,"method":"initialize","params":{{"protocolVersion":"2025-06-18"}}}}"#
    )
    .unwrap();
    assert_eq!(reply(0)["result"]["serverInfo"]["name"], "tursobench");
    writeln!(
        stdin,
        r#"{{"jsonrpc":"2.0","method":"notifications/initialized"}}"#
    )
    .unwrap();
    writeln!(stdin, r#"{{"jsonrpc":"2.0","id":1,"method":"tools/list"}}"#).unwrap();
    assert_eq!(reply(1)["result"]["tools"][0]["name"], "upper");
    drop(stdin);
    assert!(child.wait().unwrap().success());
}
