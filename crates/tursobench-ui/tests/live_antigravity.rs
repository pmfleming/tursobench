//! Opt-in integration through the installed Antigravity CLI; synthetic tools only.
use serde_json::{Value, json};
use std::{
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    time::{Duration, Instant},
};
use tursobench_assistant::{
    agent::{Agent, Protocol, Spec, Update},
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::{self, Endpoint},
    vendor::{resolve, search_dirs},
};
struct Probe(Arc<AtomicUsize>);
impl ToolHost for Probe {
    fn tools(&self) -> Vec<ToolSpec> {
        vec![ToolSpec {
            name: "list_tables",
            description: "Return a synthetic test response",
            input_schema: json!({"type":"object","properties":{}}),
        }]
    }
    fn call(&self, _: &str, _: &Value) -> ToolOutput {
        self.0.fetch_add(1, Ordering::SeqCst);
        ToolOutput::ok("TURSO BENCH")
    }
}
#[test]
#[ignore = "uses the signed-in Antigravity CLI and its plan; no real database"]
fn signed_in_antigravity_calls_our_relay_tools() {
    let calls = Arc::new(AtomicUsize::new(0));
    let endpoint = Endpoint::start(
        Arc::new(Probe(calls.clone())),
        env!("CARGO_BIN_EXE_tursobench").into(),
    )
    .unwrap();
    let agent = Agent::spawn(
        Spec {
            program: resolve(std::path::Path::new("agy"), &search_dirs()).unwrap(),
            args: vec!["acp".into(), "--disable-telemetry".into()],
            env: vec![],
            cwd: relay::private_dir().unwrap(),
            protocol: Protocol::Acp,
            system_prompt: "You test tools. Use only the supplied TursoBench MCP tool.".into(),
            mcp: Some(endpoint.launch().clone()),
            model: None,
            privacy: Default::default(),
            effort: None,
        },
        Arc::new(|| {}),
    )
    .unwrap();
    agent.prompt(
        "Call the tursobench list_tables tool once, then reply with only the returned text.",
    );
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut called = false;
    let mut result = false;
    let mut reply = String::new();
    loop {
        let update = agent
            .updates
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("agent update before deadline");
        match update {
            Update::ToolCall { name, .. } if name == "list_tables" => called = true,
            Update::ToolResult { ok: true, .. } => result = true,
            Update::Text(text) => reply.push_str(&text),
            Update::TurnEnded { error } => {
                assert_eq!(error, None);
                break;
            }
            Update::Exited { message } => panic!("agent stopped: {message}"),
            _ => {}
        }
    }
    assert!(
        called && result,
        "the synthetic relay tool must be called successfully"
    );
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert!(reply.contains("TURSO BENCH"));
}
