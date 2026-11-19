//! Runs the user's real, signed-in `claude` through the relay. It uses a little
//! of their plan, so it only runs on request:
//! `cargo test -p tursobench-ui --test live_claude -- --ignored`
mod common;
use tursobench_assistant::{claude, relay};

#[test]
#[ignore = "uses the signed-in claude CLI and its plan"]
fn signed_in_claude_calls_relay_tools_with_built_in_tools_removed() {
    let endpoint = common::endpoint();
    let mut spec = claude::spec(
        "claude".into(),
        relay::private_dir().unwrap(),
        endpoint.config_path(),
        &["upper"],
        common::INSTRUCTIONS,
        Some("haiku"),
        Default::default(),
    );
    spec.effort = Some("low".into());
    common::assert_calls_upper(spec);
}
