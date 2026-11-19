//! Runs the user's real, signed-in `codex app-server` through the relay. It uses a little
//! of their plan, so it only runs on request:
//! `cargo test -p tursobench-ui --test live_codex -- --ignored`
mod common;
use tursobench_assistant::{codex, relay};

#[test]
#[ignore = "uses the signed-in codex CLI and its plan"]
fn signed_in_codex_calls_relay_tools_in_a_read_only_thread() {
    let endpoint = common::endpoint();
    let mut spec = codex::spec(
        "codex".into(),
        relay::private_dir().unwrap(),
        endpoint.launch().clone(),
        common::INSTRUCTIONS,
        None,
        Default::default(),
    );
    spec.effort = Some("low".into());
    common::assert_calls_upper(spec);
}
