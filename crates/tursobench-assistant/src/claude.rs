//! Claude Code in headless mode, the route Anthropic documents for using the
//! unmodified `claude` binary with the user's own subscription sign-in.
use crate::agent::{Input, Link, Spec, Update, content_text, tool_name};
use serde_json::{Value as Json, json};
use std::{path::Path, sync::mpsc::Receiver};

/// Environment that stops Claude Code's telemetry, error reports and other
/// non-essential traffic; only the conversation itself reaches Anthropic.
pub(crate) fn quiet_env() -> impl Iterator<Item = (String, String)> {
    [
        "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC",
        "DISABLE_TELEMETRY",
        "DISABLE_ERROR_REPORTING",
    ]
    .into_iter()
    .map(|name| (name.into(), "1".into()))
}

/// The model aliases `claude --model` documents; Claude Code has no command
/// that lists models, so these stand in for one.
pub const MODEL_ALIASES: &[&str] = &["sonnet", "opus", "haiku", "fable"];

/// The levels `claude --effort` documents. They are applied with the
/// `apply_flag_settings` control request, which also works mid-conversation.
pub const EFFORT_LEVELS: &[&str] = &["low", "medium", "high", "xhigh", "max"];
const SETTINGS_REQUEST: &str = "tursobench-settings";

/// Arguments that start a conversation with every built-in tool removed and no
/// user or project settings, hooks, slash commands or MCP servers but
/// `config`'s, of which only `tools` may be used. The spec's privacy decides
/// whether Claude Code saves the session and reports usage.
pub(crate) fn serve(spec: &mut Spec, config: &Path, tools: &[&str]) {
    let allowed = tools
        .iter()
        .map(|t| format!("mcp__{}__{t}", crate::relay::SERVER_NAME))
        .collect::<Vec<_>>()
        .join(",");
    let args = &mut spec.args;
    args.extend(
        [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--restricted",
            "--tools",
            "",
            "--strict-mcp-config",
            "--disable-slash-commands",
        ]
        .map(String::from),
    );
    if !spec.privacy.keep_history {
        args.push("--no-session-persistence".into());
    }
    args.extend([
        "--mcp-config".into(),
        config.to_string_lossy().into_owned(),
        "--allowedTools".into(),
        allowed,
        "--system-prompt".into(),
        spec.system_prompt.clone(),
    ]);
    if let Some(model) = spec
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        args.extend(["--model".into(), model.into()]);
    }
    if spec.privacy.telemetry_off {
        spec.env.extend(quiet_env());
    }
}

pub(crate) fn drive(spec: &Spec, input: &Receiver<Input>, link: &mut Link) {
    let mut interrupts = 0u64;
    let mut changes = 0u64;
    // The level Claude Code starts with, so the default can be restored. Only
    // the applied effort is read from the reply.
    let mut original: Option<String> = None;
    let _ = link.write(&json!({
        "type": "control_request",
        "request_id": SETTINGS_REQUEST,
        "request": {"subtype": "get_settings"},
    }));
    if let Some(level) = &spec.effort {
        set_effort(link, &mut changes, Some(level));
    }
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => {
                let message = json!({
                    "type": "user",
                    "message": {"role": "user", "content": [{"type": "text", "text": text}]},
                });
                link.write_or_end_turn(&message, "the agent");
            }
            Input::Cancel => {
                interrupts += 1;
                let _ = link.write(&json!({
                    "type": "control_request",
                    "request_id": format!("tursobench-interrupt-{interrupts}"),
                    "request": {"subtype": "interrupt"},
                }));
            }
            Input::Effort(level) => {
                let level = level.or_else(|| original.clone());
                set_effort(link, &mut changes, level.as_deref());
            }
            Input::Line(line) => {
                let Ok(message) = serde_json::from_str::<Json>(&line) else {
                    continue;
                };
                if message["type"] == "control_response" {
                    control_response(link, &message["response"], &mut original);
                }
                for update in parse(&message) {
                    link.emit(update);
                }
            }
            Input::Eof => return,
        }
    }
}

/// Note the level read at start, and report a refused level change.
fn control_response(link: &mut Link, response: &Json, original: &mut Option<String>) {
    let id = response["request_id"].as_str().unwrap_or_default();
    if id == SETTINGS_REQUEST {
        *original = response["response"]["applied"]["effort"]
            .as_str()
            .map(str::to_owned);
    } else if id.starts_with("tursobench-effort") && response["subtype"] == "error" {
        link.emit(Update::Notice(format!(
            "Claude Code kept its reasoning level: {}",
            response["error"]
                .as_str()
                .unwrap_or("it refused the change")
        )));
    }
}

/// Updates carried by one stream-json message.
pub(crate) fn parse(message: &Json) -> Vec<Update> {
    let kind = message.get("type").and_then(Json::as_str).unwrap_or("");
    let text = |v: &Json, key: &str| v.get(key).and_then(Json::as_str).map(str::to_owned);
    match kind {
        "system" if text(message, "subtype").as_deref() == Some("init") => vec![Update::Ready {
            model: text(message, "model"),
        }],
        "stream_event" => {
            // Subagent output has a parent; only the main conversation is shown.
            if message
                .get("parent_tool_use_id")
                .is_some_and(|p| !p.is_null())
            {
                return vec![];
            }
            let event = &message["event"];
            if event["type"] != "content_block_delta" {
                return vec![];
            }
            let delta = &event["delta"];
            match delta["type"].as_str() {
                Some("text_delta") => text(delta, "text").map(Update::Text).into_iter().collect(),
                Some("thinking_delta") => text(delta, "thinking")
                    .map(Update::Thought)
                    .into_iter()
                    .collect(),
                _ => vec![],
            }
        }
        "assistant" => blocks(message)
            .filter(|b| b["type"] == "tool_use")
            .map(|b| Update::ToolCall {
                id: text(b, "id").unwrap_or_default(),
                name: tool_name(b["name"].as_str().unwrap_or_default()),
                input: b.get("input").cloned().unwrap_or(Json::Null),
            })
            .collect(),
        "user" => blocks(message)
            .filter(|b| b["type"] == "tool_result")
            .map(|b| Update::ToolResult {
                id: text(b, "tool_use_id").unwrap_or_default(),
                ok: !b["is_error"].as_bool().unwrap_or(false),
                text: content_text(&b["content"]),
            })
            .collect(),
        "result" => {
            let failed = message["is_error"].as_bool().unwrap_or(false)
                || text(message, "subtype").is_some_and(|s| s != "success");
            let error = failed.then(|| {
                text(message, "result")
                    .filter(|r| !r.trim().is_empty())
                    .or_else(|| {
                        message["errors"]
                            .as_array()
                            .and_then(|e| e.first())
                            .and_then(Json::as_str)
                            .map(str::to_owned)
                    })
                    .or_else(|| text(message, "subtype"))
                    .unwrap_or_else(|| "The turn failed".into())
            });
            // An interrupted turn is the user's choice, not a failure to report.
            let error = error
                .filter(|_| text(message, "terminal_reason").as_deref() != Some("interrupted"));
            vec![Update::TurnEnded { error }]
        }
        "rate_limit_event" => {
            let info = &message["rate_limit_info"];
            match info["status"].as_str() {
                Some("allowed") | None => vec![],
                Some(status) => vec![Update::Notice(format!(
                    "Your plan's usage limit is {}",
                    status.replace('_', " ")
                ))],
            }
        }
        _ => vec![],
    }
}

fn blocks(message: &Json) -> impl Iterator<Item = &Json> {
    message["message"]["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|_| message.get("parent_tool_use_id").is_none_or(Json::is_null))
}

/// Ask Claude Code to use `level`, or its own default when `None` is unknown.
fn set_effort(link: &mut Link, changes: &mut u64, level: Option<&str>) {
    *changes += 1;
    let _ = link.write(&json!({
        "type": "control_request",
        "request_id": format!("tursobench-effort-{changes}"),
        "request": {"subtype": "apply_flag_settings", "settings": {"effortLevel": level}},
    }));
}

#[cfg(test)]
mod tests {
    use super::{Path, Spec, Update, json, parse, serve};
    use crate::agent::Privacy;
    fn parsed(line: &str) -> Vec<Update> {
        parse(&serde_json::from_str(line).unwrap())
    }
    #[test]
    fn stream_json_messages_become_updates() {
        assert_eq!(
            parsed(r#"{"type":"system","subtype":"init","model":"claude-x","tools":[]}"#),
            [Update::Ready {
                model: Some("claude-x".into())
            }]
        );
        assert_eq!(
            parsed(
                r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Hi"}},"parent_tool_use_id":null}"#
            ),
            [Update::Text("Hi".into())]
        );
        assert_eq!(
            parsed(
                r#"{"type":"assistant","message":{"content":[{"type":"text","text":"x"},{"type":"tool_use","id":"t1","name":"mcp__tursobench__run_query","input":{"sql":"SELECT 1"}}]},"parent_tool_use_id":null}"#
            ),
            [Update::ToolCall {
                id: "t1".into(),
                name: "run_query".into(),
                input: json!({"sql": "SELECT 1"})
            }]
        );
        assert_eq!(
            parsed(
                r#"{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","content":[{"type":"text","text":"{}"}],"is_error":true}]}}"#
            ),
            [Update::ToolResult {
                id: "t1".into(),
                ok: false,
                text: "{}".into()
            }]
        );
        assert_eq!(
            parsed(r#"{"type":"result","subtype":"success","is_error":false,"result":"ok"}"#),
            [Update::TurnEnded { error: None }]
        );
        assert_eq!(
            parsed(
                r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#
            ),
            [Update::TurnEnded {
                error: Some("Not logged in · Please run /login".into())
            }]
        );
        assert_eq!(
            parsed(r#"{"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning"}}"#),
            [Update::Notice(
                "Your plan's usage limit is allowed warning".into()
            )]
        );
        assert!(parsed(r#"{"type":"control_response"}"#).is_empty());
    }
    #[test]
    fn arguments_remove_built_in_tools_and_allow_only_ours() {
        let served = |tools: &[&str], model: Option<&str>, privacy: Privacy| {
            let mut spec = Spec {
                system_prompt: "Be brief".into(),
                model: model.map(str::to_owned),
                privacy,
                ..Spec::default()
            };
            serve(&mut spec, Path::new("/run/x.json"), tools);
            spec
        };
        let spec = served(
            &["run_query", "describe_table"],
            Some(" sonnet "),
            Privacy::default(),
        );
        let joined = spec.args.join(" ");
        assert!(spec.env.contains(&(
            "CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC".into(),
            "1".into()
        )));
        assert!(joined.contains("--tools  --strict-mcp-config"));
        assert!(joined.contains("--restricted"));
        assert!(joined.contains("--no-session-persistence"));
        assert!(
            joined.contains(
                "--allowedTools mcp__tursobench__run_query,mcp__tursobench__describe_table"
            )
        );
        assert!(joined.ends_with("--model sonnet"));
        assert!(!joined.contains("--bare"));
        let open = served(
            &["run_query"],
            None,
            Privacy {
                telemetry_off: false,
                keep_history: true,
            },
        );
        assert!(open.env.is_empty());
        assert!(!open.args.contains(&"--no-session-persistence".to_owned()));
    }
}
