//! Claude Code in headless mode, the route Anthropic documents for using the
//! unmodified `claude` binary with the user's own subscription sign-in.
use crate::agent::{Input, Link, Privacy, Protocol, Spec, Update, content_text, tool_name};
use serde_json::{Value as Json, json};
use std::{path::PathBuf, sync::mpsc::Receiver};

/// Environment that stops Claude Code's telemetry, error reports and other
/// non-essential traffic; only the conversation itself reaches Anthropic.
pub const QUIET_ENV: &[(&str, &str)] = &[
    ("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1"),
    ("DISABLE_TELEMETRY", "1"),
    ("DISABLE_ERROR_REPORTING", "1"),
];

/// Arguments that start a conversation with every built-in tool removed and no
/// user or project settings, hooks, slash commands or other MCP servers.
/// `privacy` decides whether Claude Code saves the session and reports usage.
pub fn spec(
    program: PathBuf,
    cwd: PathBuf,
    config: &std::path::Path,
    tools: &[&str],
    system_prompt: &str,
    model: Option<&str>,
    privacy: Privacy,
) -> Spec {
    let allowed = tools
        .iter()
        .map(|t| format!("mcp__{}__{t}", crate::relay::SERVER_NAME))
        .collect::<Vec<_>>()
        .join(",");
    let mut args: Vec<String> = [
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
    .map(String::from)
    .into();
    if !privacy.keep_history {
        args.push("--no-session-persistence".into());
    }
    args.extend([
        "--mcp-config".into(),
        config.to_string_lossy().into_owned(),
        "--allowedTools".into(),
        allowed,
        "--system-prompt".into(),
        system_prompt.into(),
    ]);
    if let Some(model) = model.filter(|m| !m.trim().is_empty()) {
        args.extend(["--model".into(), model.trim().into()]);
    }
    Spec {
        program,
        args,
        env: if privacy.telemetry_off {
            QUIET_ENV
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into()))
                .collect()
        } else {
            vec![]
        },
        cwd,
        protocol: Protocol::ClaudeStream,
        system_prompt: system_prompt.into(),
        mcp: None,
        model: model.map(str::to_owned),
        privacy,
    }
}

pub(crate) fn drive(input: &Receiver<Input>, link: &mut Link) {
    let mut interrupts = 0u64;
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => {
                let message = json!({
                    "type": "user",
                    "message": {"role": "user", "content": [{"type": "text", "text": text}]},
                });
                if let Err(e) = link.write(&message) {
                    link.emit(Update::TurnEnded {
                        error: Some(format!("Could not reach the agent: {e}")),
                    });
                }
            }
            Input::Cancel => {
                interrupts += 1;
                let _ = link.write(&json!({
                    "type": "control_request",
                    "request_id": format!("tursobench-interrupt-{interrupts}"),
                    "request": {"subtype": "interrupt"},
                }));
            }
            Input::Line(line) => {
                let Ok(message) = serde_json::from_str::<Json>(&line) else {
                    continue;
                };
                for update in parse(&message) {
                    link.emit(update);
                }
            }
            Input::Eof => return,
        }
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

#[cfg(test)]
mod tests {
    use super::*;
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
        let spec = spec(
            "claude".into(),
            "/tmp".into(),
            std::path::Path::new("/run/x.json"),
            &["run_query", "describe_table"],
            "Be brief",
            Some("sonnet"),
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
        let open = super::spec(
            "claude".into(),
            "/tmp".into(),
            std::path::Path::new("/run/x.json"),
            &["run_query"],
            "Be brief",
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
