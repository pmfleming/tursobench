//! The installed binary's `--mcp-relay` mode carries MCP between an agent's
//! stdio and the running app's private socket.
use serde_json::{Value, json};
use std::{
    io::{BufRead, BufReader, Write},
    process::{Child, Command, Stdio},
    sync::Arc,
};
use tursobench_assistant::{
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::{Endpoint, Launch},
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

/// An endpoint in a private temporary directory.
fn endpoint() -> (tempfile::TempDir, Endpoint) {
    let runtime = tempfile::tempdir().unwrap();
    let exe = env!("CARGO_BIN_EXE_tursobench").into();
    let endpoint = Endpoint::start_in(runtime.path(), Arc::new(Upper), exe).unwrap();
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

const INITIALIZE: &str =
    r#"{"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#;

#[test]
fn relay_process_carries_mcp_to_the_app_socket() {
    let (_runtime, endpoint) = endpoint();
    let launch = endpoint.launch().clone();
    let mut child = relay(&launch, |c| {
        c.envs(launch.env.iter().cloned());
    });
    let mut stdin = child.stdin.take().unwrap();
    writeln!(stdin, "{INITIALIZE}").unwrap();
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

/// Agents wait for each reply with their input still open, and start MCP
/// servers with a bare environment.
#[test]
fn relay_answers_each_request_while_the_agent_waits() {
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
    let reply = |id: u64| -> Value {
        let line = rx
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap_or_else(|_| panic!("no reply to request {id} while input is open"));
        let value: Value = serde_json::from_str(&line).unwrap();
        assert_eq!(value["id"], id);
        value
    };
    writeln!(stdin, "{INITIALIZE}").unwrap();
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
