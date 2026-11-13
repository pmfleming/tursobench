//! Runs the user's real, signed-in `claude` through the relay. It uses a little
//! of their plan, so it only runs on request:
//! `cargo test -p tursobench-ui --test live_claude -- --ignored`
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    agent::{Agent, Update},
    claude,
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::{self, Endpoint},
};

struct Upper;
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
        ToolOutput::ok(
            arguments["text"]
                .as_str()
                .unwrap_or_default()
                .to_uppercase(),
        )
    }
}

#[test]
#[ignore = "uses the signed-in claude CLI and its plan"]
fn signed_in_claude_calls_relay_tools_with_built_in_tools_removed() {
    let exe = std::path::PathBuf::from(env!("CARGO_BIN_EXE_tursobench"));
    let endpoint = Endpoint::start(Arc::new(Upper), exe).unwrap();
    let spec = claude::spec(
        "claude".into(),
        relay::private_dir().unwrap(),
        endpoint.config_path(),
        &["upper"],
        "You test tools. Do exactly what you are asked.",
        Some("haiku"),
    );
    let agent = Agent::spawn(spec, Arc::new(|| {})).unwrap();
    agent.prompt(
        "Call the upper tool with the text 'turso bench', then reply with only its output.",
    );
    let mut seen = vec![];
    loop {
        let update = agent
            .updates
            .recv_timeout(Duration::from_secs(120))
            .expect("an update");
        let done = matches!(update, Update::TurnEnded { .. } | Update::Exited { .. });
        seen.push(update);
        if done {
            break;
        }
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
}
