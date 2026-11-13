//! A client for the Agent Client Protocol (v1, JSON-RPC over stdio), used for
//! Copilot CLI, Codex through `codex-acp`, and any other ACP agent.
//!
//! TursoBench advertises no file-system or terminal capabilities, answers
//! agent requests for them with "method not found", grants permission only for
//! its own tools and declines everything else. It never calls `authenticate`:
//! signing in stays in the agent's own CLI.
use crate::agent::{Input, Link, Spec, Update, content_text, tool_name};
use serde_json::{Value as Json, json};
use std::{collections::VecDeque, sync::mpsc::Receiver};

const PROTOCOL_VERSION: u64 = 1;
const INITIALIZE: u64 = 1;
const NEW_SESSION: u64 = 2;
const SET_MODEL: u64 = 3;

#[derive(Default)]
struct State {
    session: Option<String>,
    next_id: u64,
    prompt: Option<u64>,
    queued: VecDeque<String>,
    /// The standing instructions travel with the first prompt.
    instructed: bool,
    failed: bool,
}

pub(crate) fn drive(spec: &Spec, input: &Receiver<Input>, link: &mut Link) {
    let mut state = State {
        next_id: 10,
        ..Default::default()
    };
    let initialize = json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE,
        "method": "initialize",
        "params": {
            "protocolVersion": PROTOCOL_VERSION,
            "clientCapabilities": {
                "fs": {"readTextFile": false, "writeTextFile": false},
                "terminal": false,
            },
            "clientInfo": {"name": "tursobench", "title": "TursoBench", "version": env!("CARGO_PKG_VERSION")},
        },
    });
    if let Err(e) = link.write(&initialize) {
        link.emit(Update::TurnEnded {
            error: Some(format!("Could not reach the agent: {e}")),
        });
        return;
    }
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => {
                state.queued.push_back(text);
                flush(spec, &mut state, link);
            }
            Input::Cancel => {
                if let Some(session) = &state.session {
                    let _ = link.write(&json!({
                        "jsonrpc": "2.0",
                        "method": "session/cancel",
                        "params": {"sessionId": session},
                    }));
                } else if !state.queued.is_empty() {
                    state.queued.clear();
                    link.emit(Update::TurnEnded { error: None });
                }
            }
            Input::Line(line) => {
                let Ok(message) = serde_json::from_str::<Json>(&line) else {
                    continue;
                };
                handle(spec, &mut state, link, &message);
                flush(spec, &mut state, link);
            }
            Input::Eof => return,
        }
    }
}

/// Send the next queued prompt once a session exists and no turn is running.
fn flush(spec: &Spec, state: &mut State, link: &mut Link) {
    if state.failed {
        while state.queued.pop_front().is_some() {
            link.emit(Update::TurnEnded {
                error: Some("The agent could not start a session".into()),
            });
        }
        return;
    }
    let Some(session) = state.session.clone() else {
        return;
    };
    if state.prompt.is_some() {
        return;
    }
    let Some(mut text) = state.queued.pop_front() else {
        return;
    };
    if !state.instructed && !spec.system_prompt.is_empty() {
        state.instructed = true;
        text = format!(
            "<tursobench-instructions>\n{}\n</tursobench-instructions>\n\n{text}",
            spec.system_prompt
        );
    }
    state.next_id += 1;
    let id = state.next_id;
    state.prompt = Some(id);
    let request = json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/prompt",
        "params": {"sessionId": session, "prompt": [{"type": "text", "text": text}]},
    });
    if let Err(e) = link.write(&request) {
        state.prompt = None;
        link.emit(Update::TurnEnded {
            error: Some(format!("Could not reach the agent: {e}")),
        });
    }
}

fn handle(spec: &Spec, state: &mut State, link: &mut Link, message: &Json) {
    let method = message.get("method").and_then(Json::as_str);
    let id = message.get("id");
    match (method, id) {
        (Some(method), Some(id)) => request(link, method, id.clone(), &message["params"]),
        (Some("session/update"), None) => {
            for update in session_update(&message["params"]["update"]) {
                link.emit(update);
            }
        }
        (Some(_), None) => {}
        (None, Some(id)) => response(spec, state, link, id.as_u64(), message),
        (None, None) => {}
    }
}

fn response(spec: &Spec, state: &mut State, link: &mut Link, id: Option<u64>, message: &Json) {
    let error = message.get("error").map(|e| {
        let text = e["message"]
            .as_str()
            .unwrap_or("The agent reported an error");
        let detail = e["data"]
            .get("details")
            .or_else(|| e["data"].get("message"))
            .and_then(Json::as_str);
        match detail {
            Some(detail) if !detail.is_empty() => format!("{text}: {detail}"),
            _ => text.to_owned(),
        }
    });
    let result = &message["result"];
    match id {
        Some(INITIALIZE) => {
            if let Some(error) = error {
                return fail(state, link, error);
            }
            let mut servers = vec![];
            if let Some(mcp) = &spec.mcp {
                servers.push(json!({
                    "name": crate::relay::SERVER_NAME,
                    "command": mcp.command,
                    "args": mcp.args,
                    "env": mcp.env.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>(),
                }));
            }
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": NEW_SESSION,
                "method": "session/new",
                "params": {"cwd": spec.cwd, "mcpServers": servers},
            }));
        }
        Some(NEW_SESSION) => {
            if let Some(error) = error {
                let hint = if message["error"]["code"] == -32000
                    || error.to_lowercase().contains("auth")
                {
                    format!(
                        "{error}. Sign in with the agent's own CLI in a terminal, then try again."
                    )
                } else {
                    error
                };
                return fail(state, link, hint);
            }
            let Some(session) = result["sessionId"].as_str() else {
                return fail(state, link, "The agent returned no session".into());
            };
            state.session = Some(session.to_owned());
            let model = choose_model(spec, state, link, result);
            link.emit(Update::Ready { model });
        }
        Some(SET_MODEL) => {
            if let Some(error) = error {
                link.emit(Update::Notice(format!(
                    "The agent kept its default model: {error}"
                )));
            }
        }
        Some(other) if Some(other) == state.prompt => {
            state.prompt = None;
            let error = error.or_else(|| match result["stopReason"].as_str() {
                Some("refusal") => Some("The agent declined to answer".into()),
                Some("max_tokens") => Some("The reply reached the agent's length limit".into()),
                Some("max_turn_requests") => {
                    Some("The agent reached its limit of steps for one turn".into())
                }
                _ => None,
            });
            link.emit(Update::TurnEnded { error });
        }
        _ => {}
    }
}

fn fail(state: &mut State, link: &mut Link, error: String) {
    state.failed = true;
    state.queued.clear();
    link.emit(Update::TurnEnded { error: Some(error) });
}

/// Apply the preferred model through config options or the model list; return
/// the name of the model in use when the agent says.
fn choose_model(spec: &Spec, state: &mut State, link: &mut Link, result: &Json) -> Option<String> {
    let session = state.session.clone()?;
    let wanted = spec
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty());
    let option = result["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["category"] == "model");
    if let Some(option) = option {
        let current = option["currentValue"].as_str().map(str::to_owned);
        let Some(wanted) = wanted else { return current };
        let value = option["options"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|o| o["value"].as_str())
            .find(|v| v.eq_ignore_ascii_case(wanted) || v.contains(wanted));
        if let (Some(value), Some(config)) = (value, option["id"].as_str()) {
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": SET_MODEL,
                "method": "session/set_config_option",
                "params": {"sessionId": session, "configId": config, "value": value},
            }));
            return Some(value.to_owned());
        }
        link.emit(Update::Notice(format!(
            "The agent does not offer the model {wanted}"
        )));
        return current;
    }
    let models = &result["models"];
    let current = models["currentModelId"].as_str().map(str::to_owned);
    let Some(wanted) = wanted else { return current };
    let found = models["availableModels"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| m["modelId"].as_str())
        .find(|m| m.eq_ignore_ascii_case(wanted) || m.contains(wanted));
    match found {
        Some(model) => {
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": SET_MODEL,
                "method": "session/set_model",
                "params": {"sessionId": session, "modelId": model},
            }));
            Some(model.to_owned())
        }
        None => {
            link.emit(Update::Notice(format!(
                "The agent does not offer the model {wanted}"
            )));
            current
        }
    }
}

fn request(link: &mut Link, method: &str, id: Json, params: &Json) {
    let reply = match method {
        "session/request_permission" => {
            let ours = is_ours(&params["toolCall"]);
            let options = params["options"].as_array().cloned().unwrap_or_default();
            let pick = |kinds: &[&str]| {
                options
                    .iter()
                    .find(|o| kinds.contains(&o["kind"].as_str().unwrap_or("")))
                    .and_then(|o| o["optionId"].as_str())
                    .map(str::to_owned)
            };
            let choice = if ours {
                pick(&["allow_once", "allow_always"])
            } else {
                pick(&["reject_once", "reject_always"])
            };
            if !ours {
                let title = params["toolCall"]["title"].as_str().unwrap_or("a tool");
                link.emit(Update::Notice(format!(
                    "Declined the agent's request to use {title}; only TursoBench's \
                     read-only tools are allowed"
                )));
            }
            let outcome = match choice {
                Some(option) => json!({"outcome": "selected", "optionId": option}),
                None => json!({"outcome": "cancelled"}),
            };
            json!({"jsonrpc": "2.0", "id": id, "result": {"outcome": outcome}})
        }
        _ => json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": {"code": -32601, "message": format!("TursoBench does not provide {method}")},
        }),
    };
    let _ = link.write(&reply);
}

/// Only calls to TursoBench's own MCP tools are approved.
fn is_ours(call: &Json) -> bool {
    let server = crate::relay::SERVER_NAME;
    let text = [
        &call["title"],
        &call["toolName"],
        &call["name"],
        &call["rawInput"]["server"],
    ]
    .iter()
    .filter_map(|v| v.as_str())
    .collect::<Vec<_>>()
    .join(" ")
    .to_lowercase();
    text.contains(server)
}

fn session_update(update: &Json) -> Vec<Update> {
    let text = |v: &Json| v["content"]["text"].as_str().map(str::to_owned);
    match update["sessionUpdate"].as_str() {
        Some("agent_message_chunk") => text(update).map(Update::Text).into_iter().collect(),
        Some("agent_thought_chunk") => text(update).map(Update::Thought).into_iter().collect(),
        Some("tool_call") => {
            let id = update["toolCallId"].as_str().unwrap_or_default().to_owned();
            let title = update["title"].as_str().unwrap_or("tool");
            let mut updates = vec![Update::ToolCall {
                id: id.clone(),
                name: tool_name(&short_title(title)),
                input: update.get("rawInput").cloned().unwrap_or(Json::Null),
            }];
            updates.extend(finished(&id, update));
            updates
        }
        Some("tool_call_update") => {
            let id = update["toolCallId"].as_str().unwrap_or_default();
            finished(id, update).into_iter().collect()
        }
        _ => vec![],
    }
}

/// `tursobench/run_query`, `Tool: tursobench.run_query` and similar titles.
fn short_title(title: &str) -> String {
    let server = crate::relay::SERVER_NAME;
    match title.find(server) {
        Some(at) => title[at..]
            .split_whitespace()
            .next()
            .unwrap_or(title)
            .to_owned(),
        None => title.to_owned(),
    }
}

fn finished(id: &str, update: &Json) -> Option<Update> {
    let ok = match update["status"].as_str() {
        Some("completed") => true,
        Some("failed") => false,
        _ => return None,
    };
    let text = update["content"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| content_text(&json!([c["content"]])))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let text = if text.is_empty() {
        update
            .get("rawOutput")
            .map(|o| match o {
                Json::String(s) => s.clone(),
                other => content_text(&other["content"]).trim().to_owned(),
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| update["rawOutput"].to_string())
    } else {
        text
    };
    Some(Update::ToolResult {
        id: id.to_owned(),
        ok,
        text,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn updates_and_titles_map_to_tursobench_tools() {
        let updates = session_update(&json!({
            "sessionUpdate": "tool_call",
            "toolCallId": "c1",
            "title": "tursobench/run_query",
            "status": "pending",
            "rawInput": {"sql": "SELECT 1"},
        }));
        assert_eq!(
            updates,
            [Update::ToolCall {
                id: "c1".into(),
                name: "run_query".into(),
                input: json!({"sql": "SELECT 1"})
            }]
        );
        let done = session_update(&json!({
            "sessionUpdate": "tool_call_update",
            "toolCallId": "c1",
            "status": "completed",
            "content": [{"type": "content", "content": {"type": "text", "text": "{\"rows\":[]}"}}],
        }));
        assert_eq!(
            done,
            [Update::ToolResult {
                id: "c1".into(),
                ok: true,
                text: "{\"rows\":[]}".into()
            }]
        );
        assert_eq!(
            session_update(
                &json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": "Hi"}})
            ),
            [Update::Text("Hi".into())]
        );
        assert!(is_ours(
            &json!({"title": "Tool: tursobench.describe_table"})
        ));
        assert!(!is_ours(&json!({"title": "Run shell command: rm -rf ~"})));
        assert_eq!(
            short_title("Tool: tursobench.describe_table"),
            "tursobench.describe_table"
        );
        assert_eq!(tool_name("tursobench.describe_table"), "describe_table");
    }
}
