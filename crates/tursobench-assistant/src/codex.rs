//! Codex through its own app server (`codex app-server`, JSON-RPC over stdio),
//! the protocol the Codex IDE extensions use with the user's own sign-in.
//!
//! The thread is ephemeral, runs in Codex's read-only sandbox, asks before any
//! action it does not trust, and every such request is declined. Codex's
//! shell, web search, apps, plugins, browser and computer use are switched off;
//! its MCP servers are replaced by TursoBench's alone. TursoBench never calls
//! the login, logout, account or token methods.
use crate::agent::{Input, Link, Protocol, Spec, Update, content_text, tool_name};
use serde_json::{Value as Json, json};
use std::{collections::VecDeque, path::PathBuf, sync::mpsc::Receiver};

const INITIALIZE: u64 = 1;
const START_THREAD: u64 = 2;

/// Codex features that give the agent tools of its own.
pub const DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    "apps",
    "plugins",
    "remote_plugin",
    "browser_use",
    "browser_use_external",
    "in_app_browser",
    "computer_use",
    "image_generation",
    "view_image",
    "multi_agent",
    "goals",
    "hooks",
    "sleep_tool",
    "tool_suggest",
    "skill_search",
    "skill_mcp_dependency_install",
];

/// `codex app-server` with Codex's own tools switched off. The tool relay and
/// its token travel in `thread/start`, not on the command line.
pub fn spec(
    program: PathBuf,
    cwd: PathBuf,
    mcp: crate::relay::Launch,
    system_prompt: &str,
    model: Option<&str>,
) -> Spec {
    let mut args = vec!["app-server".to_owned()];
    for feature in DISABLED_FEATURES {
        args.extend(["--disable".into(), (*feature).into()]);
    }
    args.extend(["-c".into(), "web_search=\"disabled\"".into()]);
    Spec {
        program,
        args,
        env: vec![],
        cwd,
        protocol: Protocol::CodexAppServer,
        system_prompt: system_prompt.into(),
        mcp: Some(mcp),
        model: model
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned),
    }
}

#[derive(Default)]
struct State {
    thread: Option<String>,
    /// The running turn: the request that starts it, then its id.
    starting: Option<u64>,
    turn: Option<String>,
    cancel_requested: bool,
    queued: VecDeque<String>,
    next_id: u64,
    failed: bool,
    /// Text was shown since the last tool, so a new message needs a break.
    spoke: bool,
    /// Messages whose text arrived as deltas.
    streamed: Vec<String>,
    /// A non-retried error reported during the turn.
    error: Option<String>,
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
            "clientInfo": {"name": "tursobench", "title": "TursoBench", "version": env!("CARGO_PKG_VERSION")},
        },
    });
    if let Err(e) = link.write(&initialize) {
        link.emit(Update::TurnEnded {
            error: Some(format!("Could not reach Codex: {e}")),
        });
        return;
    }
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => state.queued.push_back(text),
            Input::Cancel => cancel(&mut state, link),
            Input::Line(line) => {
                let Ok(message) = serde_json::from_str::<Json>(&line) else {
                    continue;
                };
                handle(spec, &mut state, link, &message);
            }
            Input::Eof => return,
        }
        flush(&mut state, link);
    }
}

fn cancel(state: &mut State, link: &mut Link) {
    match (&state.thread, &state.turn) {
        (Some(thread), Some(turn)) => {
            state.next_id += 1;
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": state.next_id,
                "method": "turn/interrupt",
                "params": {"threadId": thread, "turnId": turn},
            }));
        }
        // The turn id is not known yet; interrupt as soon as it is.
        _ if state.starting.is_some() => state.cancel_requested = true,
        _ if !state.queued.is_empty() => {
            state.queued.clear();
            link.emit(Update::TurnEnded { error: None });
        }
        _ => {}
    }
}

/// Start the next queued prompt once a thread exists and no turn is running.
fn flush(state: &mut State, link: &mut Link) {
    if state.failed {
        while state.queued.pop_front().is_some() {
            link.emit(Update::TurnEnded {
                error: Some("Codex could not start a conversation".into()),
            });
        }
        return;
    }
    let Some(thread) = state.thread.clone() else {
        return;
    };
    if state.starting.is_some() || state.turn.is_some() {
        return;
    }
    let Some(text) = state.queued.pop_front() else {
        return;
    };
    state.next_id += 1;
    state.starting = Some(state.next_id);
    state.spoke = false;
    state.streamed.clear();
    state.error = None;
    let request = json!({
        "jsonrpc": "2.0",
        "id": state.next_id,
        "method": "turn/start",
        "params": {"threadId": thread, "input": [{"type": "text", "text": text}]},
    });
    if let Err(e) = link.write(&request) {
        state.starting = None;
        link.emit(Update::TurnEnded {
            error: Some(format!("Could not reach Codex: {e}")),
        });
    }
}

fn handle(spec: &Spec, state: &mut State, link: &mut Link, message: &Json) {
    let method = message.get("method").and_then(Json::as_str);
    match (method, message.get("id")) {
        (Some(method), Some(id)) => request(link, method, id.clone(), &message["params"]),
        (Some(method), None) => {
            for update in notification(state, method, &message["params"]) {
                link.emit(update);
            }
        }
        (None, Some(id)) => response(spec, state, link, id.as_u64(), message),
        (None, None) => {}
    }
}

fn error_text(message: &Json) -> Option<String> {
    message.get("error").map(|e| {
        e["message"]
            .as_str()
            .filter(|m| !m.is_empty())
            .unwrap_or("Codex reported an error")
            .to_owned()
    })
}

fn response(spec: &Spec, state: &mut State, link: &mut Link, id: Option<u64>, message: &Json) {
    let error = error_text(message);
    let result = &message["result"];
    match id {
        Some(INITIALIZE) => {
            if let Some(error) = error {
                return fail(state, link, error);
            }
            let _ = link.write(&json!({"jsonrpc": "2.0", "method": "initialized"}));
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": START_THREAD,
                "method": "thread/start",
                "params": thread_params(spec),
            }));
        }
        Some(START_THREAD) => {
            if let Some(error) = error {
                return fail(state, link, error);
            }
            let Some(thread) = result["thread"]["id"].as_str() else {
                return fail(state, link, "Codex returned no conversation".into());
            };
            state.thread = Some(thread.to_owned());
            link.emit(Update::Ready {
                model: result["model"].as_str().map(str::to_owned),
            });
        }
        Some(id) if Some(id) == state.starting => {
            state.starting = None;
            if let Some(error) = error {
                state.cancel_requested = false;
                link.emit(Update::TurnEnded { error: Some(error) });
                return;
            }
            // `turn/started` may already have named the turn.
            if state.turn.is_none() {
                state.turn = result["turn"]["id"].as_str().map(str::to_owned);
            }
            if std::mem::take(&mut state.cancel_requested) {
                cancel(state, link);
            }
        }
        _ => {}
    }
}

/// An ephemeral, read-only thread whose only MCP server is TursoBench's.
fn thread_params(spec: &Spec) -> Json {
    let mut servers = serde_json::Map::new();
    if let Some(mcp) = &spec.mcp {
        servers.insert(
            crate::relay::SERVER_NAME.into(),
            json!({
                "command": mcp.command,
                "args": mcp.args,
                "env": mcp.env.iter()
                    .map(|(name, value)| (name.clone(), json!(value)))
                    .collect::<serde_json::Map<String, Json>>(),
            }),
        );
    }
    let mut params = json!({
        "cwd": spec.cwd,
        "ephemeral": true,
        "sandbox": "read-only",
        "approvalPolicy": "untrusted",
        "baseInstructions": spec.system_prompt,
        "config": {"mcp_servers": servers},
    });
    if let Some(model) = &spec.model {
        params["model"] = json!(model);
    }
    params
}

fn fail(state: &mut State, link: &mut Link, error: String) {
    state.failed = true;
    state.queued.clear();
    link.emit(Update::TurnEnded { error: Some(error) });
}

fn notification(state: &mut State, method: &str, params: &Json) -> Vec<Update> {
    match method {
        "turn/started" => {
            state.turn = params["turn"]["id"].as_str().map(str::to_owned);
            vec![]
        }
        "item/agentMessage/delta" => {
            let Some(delta) = params["delta"].as_str().filter(|d| !d.is_empty()) else {
                return vec![];
            };
            let item = params["itemId"].as_str().unwrap_or_default().to_owned();
            let mut updates = vec![];
            if !state.streamed.contains(&item) {
                if state.spoke {
                    updates.push(Update::Text("\n\n".into()));
                }
                state.streamed.push(item);
            }
            state.spoke = true;
            updates.push(Update::Text(delta.to_owned()));
            updates
        }
        "item/reasoning/summaryTextDelta" => params["delta"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(|d| Update::Thought(d.to_owned()))
            .into_iter()
            .collect(),
        "item/started" => item_started(state, &params["item"]),
        "item/completed" => item_completed(state, &params["item"]),
        "error" => {
            let message = params["error"]["message"]
                .as_str()
                .unwrap_or("Codex reported an error")
                .to_owned();
            if params["willRetry"].as_bool().unwrap_or(false) {
                vec![Update::Notice(format!("{message} (Codex is retrying)"))]
            } else {
                state.error = Some(message);
                vec![]
            }
        }
        "turn/completed" => {
            let turn = &params["turn"];
            state.turn = None;
            state.starting = None;
            state.cancel_requested = false;
            let error = match turn["status"].as_str() {
                Some("failed") => Some(
                    turn["error"]["message"]
                        .as_str()
                        .map(str::to_owned)
                        .or_else(|| state.error.take())
                        .unwrap_or_else(|| "The turn failed".into()),
                ),
                _ => None,
            };
            vec![Update::TurnEnded { error }]
        }
        "mcpServer/startupStatus/updated"
            if params["name"] == crate::relay::SERVER_NAME
                && !matches!(params["status"].as_str(), Some("starting" | "ready")) =>
        {
            let reason = params["error"]
                .as_str()
                .or(params["failureReason"].as_str())
                .unwrap_or("no reason given");
            vec![Update::Notice(format!(
                "TursoBench's tools did not start in Codex ({}): {reason}",
                params["status"].as_str().unwrap_or("unknown")
            ))]
        }
        "account/rateLimits/updated" => {
            let used = params["rateLimits"]["primary"]["usedPercent"].as_f64();
            match used {
                Some(used) if used >= 90.0 => vec![Update::Notice(format!(
                    "Your plan's Codex usage is at {used:.0}%"
                ))],
                _ => vec![],
            }
        }
        _ => vec![],
    }
}

fn item_started(state: &mut State, item: &Json) -> Vec<Update> {
    let id = item["id"].as_str().unwrap_or_default().to_owned();
    match item["type"].as_str() {
        Some("mcpToolCall") => {
            state.spoke = false;
            let server = item["server"].as_str().unwrap_or_default();
            let tool = item["tool"].as_str().unwrap_or("tool");
            let name = if server == crate::relay::SERVER_NAME {
                tool_name(tool)
            } else {
                format!("{server}.{tool}")
            };
            vec![Update::ToolCall {
                id,
                name,
                input: item.get("arguments").cloned().unwrap_or(Json::Null),
            }]
        }
        Some(
            kind @ ("commandExecution"
            | "fileChange"
            | "webSearch"
            | "imageView"
            | "collabAgentToolCall"
            | "dynamicToolCall"),
        ) => {
            vec![Update::Notice(format!(
                "Codex started its own {}",
                describe(kind)
            ))]
        }
        _ => vec![],
    }
}

fn item_completed(state: &mut State, item: &Json) -> Vec<Update> {
    let id = item["id"].as_str().unwrap_or_default();
    match item["type"].as_str() {
        Some("mcpToolCall") => {
            let result = &item["result"];
            let failed = item["status"] != "completed"
                || result["isError"].as_bool().unwrap_or(false)
                || !item["error"].is_null();
            let text = if item["error"].is_null() {
                content_text(&result["content"])
            } else {
                item["error"]["message"]
                    .as_str()
                    .map_or_else(|| item["error"].to_string(), str::to_owned)
            };
            vec![Update::ToolResult {
                id: id.to_owned(),
                ok: !failed,
                text,
            }]
        }
        // Text that did not arrive as deltas still reaches the panel.
        Some("agentMessage") if !state.streamed.iter().any(|s| s == id) => {
            let text = item["text"].as_str().unwrap_or_default();
            if text.is_empty() {
                return vec![];
            }
            let mut updates = vec![];
            if state.spoke {
                updates.push(Update::Text("\n\n".into()));
            }
            state.spoke = true;
            state.streamed.push(id.to_owned());
            updates.push(Update::Text(text.to_owned()));
            updates
        }
        _ => vec![],
    }
}

fn describe(kind: &str) -> &'static str {
    match kind {
        "commandExecution" => "command",
        "fileChange" => "file edit",
        "webSearch" => "web search",
        "imageView" => "image viewer",
        "collabAgentToolCall" => "helper agent",
        _ => "tool",
    }
}

/// Approvals are declined; Codex's other requests are refused.
fn request(link: &mut Link, method: &str, id: Json, params: &Json) {
    let declined = |what: &str| {
        Update::Notice(format!(
            "Declined Codex's request to {what}; only TursoBench's read-only tools are allowed"
        ))
    };
    let result = match method {
        "item/commandExecution/requestApproval" => {
            let command = params["command"].as_str().unwrap_or("a command");
            link.emit(declined(&format!("run `{command}`")));
            json!({"decision": "decline"})
        }
        "item/fileChange/requestApproval" => {
            link.emit(declined("change files"));
            json!({"decision": "decline"})
        }
        "execCommandApproval" | "applyPatchApproval" => {
            link.emit(declined("run a command or change files"));
            json!({"decision": "denied"})
        }
        "item/permissions/requestApproval" => {
            link.emit(declined("widen its sandbox"));
            json!({"permissions": {}, "scope": "turn"})
        }
        "mcpServer/elicitation/request" => {
            link.emit(declined("ask for input"));
            json!({"action": "decline"})
        }
        // Includes `account/chatgptAuthTokens/refresh`: TursoBench never
        // handles sign-in tokens.
        _ => {
            let _ = link.write(&json!({
                "jsonrpc": "2.0",
                "id": id,
                "error": {"code": -32601, "message": format!("TursoBench does not provide {method}")},
            }));
            return;
        }
    };
    let _ = link.write(&json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deltas_tools_and_turns_become_updates() {
        let mut state = State::default();
        let n =
            |state: &mut State, method: &str, params: Json| notification(state, method, &params);
        assert!(n(&mut state, "turn/started", json!({"turn": {"id": "t1"}})).is_empty());
        assert_eq!(state.turn.as_deref(), Some("t1"));
        assert_eq!(
            n(
                &mut state,
                "item/agentMessage/delta",
                json!({"itemId": "m1", "delta": "Hi"})
            ),
            [Update::Text("Hi".into())]
        );
        // A second message in the same turn starts a new paragraph.
        assert_eq!(
            n(
                &mut state,
                "item/agentMessage/delta",
                json!({"itemId": "m2", "delta": "Then"})
            ),
            [Update::Text("\n\n".into()), Update::Text("Then".into())]
        );
        assert!(
            n(
                &mut state,
                "item/completed",
                json!({"item": {"type": "agentMessage", "id": "m2", "text": "Then"}})
            )
            .is_empty()
        );
        assert_eq!(
            n(
                &mut state,
                "item/started",
                json!({"item": {"type": "mcpToolCall", "id": "c1", "server": "tursobench", "tool": "run_query", "arguments": {"sql": "SELECT 1"}}})
            ),
            [Update::ToolCall {
                id: "c1".into(),
                name: "run_query".into(),
                input: json!({"sql": "SELECT 1"})
            }]
        );
        assert_eq!(
            n(
                &mut state,
                "item/completed",
                json!({"item": {"type": "mcpToolCall", "id": "c1", "server": "tursobench", "tool": "run_query", "status": "completed", "result": {"content": [{"type": "text", "text": "[]"}], "isError": false}, "error": null}})
            ),
            [Update::ToolResult {
                id: "c1".into(),
                ok: true,
                text: "[]".into()
            }]
        );
        assert_eq!(
            n(
                &mut state,
                "item/completed",
                json!({"item": {"type": "agentMessage", "id": "m3", "text": "Done"}})
            ),
            [Update::Text("Done".into())]
        );
        assert!(matches!(
            &n(&mut state, "item/started", json!({"item": {"type": "commandExecution", "id": "x"}}))[..],
            [Update::Notice(t)] if t.contains("command")
        ));
        assert!(
            n(
                &mut state,
                "error",
                json!({"error": {"message": "unauthorized"}, "willRetry": false})
            )
            .is_empty()
        );
        assert_eq!(
            n(
                &mut state,
                "turn/completed",
                json!({"turn": {"id": "t1", "status": "failed", "error": null}})
            ),
            [Update::TurnEnded {
                error: Some("unauthorized".into())
            }]
        );
        assert_eq!(state.turn, None);
        assert_eq!(
            n(
                &mut state,
                "turn/completed",
                json!({"turn": {"id": "t2", "status": "interrupted", "error": null}})
            ),
            [Update::TurnEnded { error: None }]
        );
    }

    #[test]
    fn the_thread_is_ephemeral_read_only_and_carries_only_our_server() {
        let spec = spec(
            "codex".into(),
            "/run/tb".into(),
            crate::relay::Launch {
                command: "/usr/bin/tursobench".into(),
                args: vec!["--mcp-relay".into(), "/run/tb/s.sock".into()],
                env: vec![(crate::relay::TOKEN_ENV.into(), "secret".into())],
            },
            "Be brief",
            Some(" gpt-x "),
        );
        let joined = spec.args.join(" ");
        assert!(joined.starts_with("app-server "));
        assert!(joined.contains("--disable shell_tool"));
        assert!(joined.contains("web_search=\"disabled\""));
        assert!(
            !joined.contains("secret"),
            "the token stays off the command line"
        );
        let params = thread_params(&spec);
        assert_eq!(params["ephemeral"], true);
        assert_eq!(params["sandbox"], "read-only");
        assert_eq!(params["approvalPolicy"], "untrusted");
        assert_eq!(params["baseInstructions"], "Be brief");
        assert_eq!(params["model"], "gpt-x");
        let servers = params["config"]["mcp_servers"].as_object().unwrap();
        assert_eq!(servers.len(), 1);
        assert_eq!(
            servers["tursobench"]["env"][crate::relay::TOKEN_ENV],
            "secret"
        );
    }
}
