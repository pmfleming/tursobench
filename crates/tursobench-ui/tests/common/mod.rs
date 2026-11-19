//! What the live Claude and Codex tests share: a synthetic tool served through
//! the installed binary's relay, and a turn that must call it.
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tursobench_assistant::{
    agent::{Agent, Spec, Update},
    mcp::{ToolHost, ToolOutput, ToolSpec},
    relay::Endpoint,
};

pub const INSTRUCTIONS: &str = "You test tools. Do exactly what you are asked.";

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

/// The `upper` tool, served in this user's private directory.
pub fn endpoint() -> Endpoint {
    Endpoint::start(Arc::new(Upper), env!("CARGO_BIN_EXE_tursobench").into()).unwrap()
}

/// Ask the agent to call `upper`; it must, and the turn must succeed.
pub fn assert_calls_upper(spec: Spec) {
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
