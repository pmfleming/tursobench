//! Codex through its own app server (`codex app-server`, JSON-RPC over stdio),
//! the protocol the Codex IDE extensions use with the user's own sign-in.
//!
//! The thread runs in Codex's read-only sandbox and asks before any action it
//! does not trust; every such request is declined. Unless the user allows it,
//! the thread is ephemeral and Codex reports no analytics or telemetry. Codex's
//! shell, web search, apps, plugins, browser and computer use are switched off;
//! its MCP servers are replaced by TursoBench's alone. TursoBench never calls
//! the login, logout, account or token methods.
use crate::agent::{Input, Link, Queue, Spec, Update, content_text, initialize, tool_name};
use serde_json::{Value as Json, json};
use std::sync::mpsc::Receiver;

const INITIALIZE: u64 = 1;
const START_THREAD: u64 = 2;

/// Codex features that give the agent tools of its own.
const DISABLED_FEATURES: &[&str] = &[
    "shell_tool",
    "unified_exec",
    // Copies the user's shell environment to ~/.codex; no shell runs here.
    "shell_snapshot",
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

/// Overrides that stop Codex's analytics, OpenTelemetry export and feedback
/// uploads; only the conversation itself reaches OpenAI.
pub(crate) const QUIET_CONFIG: &[&str] = &[
    "analytics.enabled=false",
    "otel.exporter=\"none\"",
    "otel.metrics_exporter=\"none\"",
    "otel.trace_exporter=\"none\"",
    "feedback.enabled=false",
];
pub(crate) const NO_HISTORY: &str = "history.persistence=\"none\"";

/// `codex app-server` with Codex's own tools switched off and `config` overrides.
pub(crate) fn app_server<'a>(config: impl IntoIterator<Item = &'a str>) -> Vec<String> {
    let mut args = vec!["app-server".to_owned()];
    for feature in DISABLED_FEATURES {
        args.extend(["--disable".into(), (*feature).into()]);
    }
    for setting in config {
        args.extend(["-c".into(), setting.into()]);
    }
    args
}

/// Run `codex app-server` without web search. The tool relay and its token
/// travel in `thread/start`, not on the command line. The spec's privacy
/// decides whether Codex saves the thread and reports usage.
pub(crate) fn serve(spec: &mut Spec, mcp: crate::relay::Launch) {
    let privacy = spec.privacy;
    let quiet = QUIET_CONFIG.iter().filter(|_| privacy.telemetry_off);
    let history = (!privacy.keep_history).then_some(NO_HISTORY);
    let config = std::iter::once("web_search=\"disabled\"").chain(quiet.copied());
    spec.args.extend(app_server(config.chain(history)));
    spec.mcp = Some(mcp);
}

#[derive(Default)]
struct State {
    /// The reasoning level the user chose; `None` uses `original`.
    effort: Option<String>,
    /// The level the thread started with, as `thread/start` reported it.
    original: Option<String>,
    thread: Option<String>,
    /// The running turn: the request that starts it, then its id.
    starting: Option<u64>,
    turn: Option<String>,
    cancel_requested: bool,
    queued: Queue,
    next_id: u64,
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
        effort: spec.effort.clone(),
        ..Default::default()
    };
    if !link.write_or_end_turn(&initialize(INITIALIZE, json!({})), "Codex") {
        return;
    }
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => state.queued.push(text),
            Input::Cancel => cancel(&mut state, link),
            Input::Effort(level) => state.effort = level,
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
        _ => state.queued.cancel(link),
    }
}

/// Start the next queued prompt once a thread exists and no turn is running.
fn flush(state: &mut State, link: &mut Link) {
    if state
        .queued
        .refuse(link, "Codex could not start a conversation")
    {
        return;
    }
    let Some(thread) = state.thread.clone() else {
        return;
    };
    if state.starting.is_some() || state.turn.is_some() {
        return;
    }
    let Some(text) = state.queued.pop() else {
        return;
    };
    state.next_id += 1;
    state.starting = Some(state.next_id);
    state.spoke = false;
    state.streamed.clear();
    state.error = None;
    let mut params = json!({"threadId": thread, "input": [{"type": "text", "text": text}]});
    // A turn's effort sticks for later turns, so the original level is sent
    // explicitly to undo an earlier choice.
    if let Some(effort) = state.effort.as_ref().or(state.original.as_ref()) {
        params["effort"] = json!(effort);
    }
    let request = json!({
        "jsonrpc": "2.0",
        "id": state.next_id,
        "method": "turn/start",
        "params": params,
    });
    if !link.write_or_end_turn(&request, "Codex") {
        state.starting = None;
    }
}

fn handle(spec: &Spec, state: &mut State, link: &mut Link, message: &Json) {
    let method = message.get("method").and_then(Json::as_str);
    match (method, message.get("id")) {
        (Some(method), Some(id)) => request(link, method, id, &message["params"]),
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
                return state.queued.fail(link, error);
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
                return state.queued.fail(link, error);
            }
            let Some(thread) = result["thread"]["id"].as_str() else {
                return state
                    .queued
                    .fail(link, "Codex returned no conversation".into());
            };
            state.thread = Some(thread.to_owned());
            state.original = result["reasoningEffort"].as_str().map(str::to_owned);
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
        "ephemeral": !spec.privacy.keep_history,
        "sandbox": "read-only",
        "approvalPolicy": "untrusted",
        "baseInstructions": spec.system_prompt,
        "config": {"mcp_servers": servers},
    });
    if let Some(model) = spec
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    {
        params["model"] = json!(model);
    }
    params
}

fn notification(state: &mut State, method: &str, params: &Json) -> Vec<Update> {
    match method {
        "turn/started" => {
            state.turn = params["turn"]["id"].as_str().map(str::to_owned);
            vec![]
        }
        "item/agentMessage/delta" => match params["delta"].as_str().filter(|d| !d.is_empty()) {
            Some(delta) => say(state, params["itemId"].as_str().unwrap_or_default(), delta),
            None => vec![],
        },
        "item/reasoning/summaryTextDelta" => params["delta"]
            .as_str()
            .filter(|d| !d.is_empty())
            .map(|d| Update::Thought(d.to_owned()))
            .into_iter()
            .collect(),
        "item/started" => item_started(state, &params["item"]),
        "item/completed" => item_completed(state, &params["item"]),
        "error" => reported_error(state, params).into_iter().collect(),
        "turn/completed" => vec![turn_completed(state, &params["turn"])],
        "mcpServer/startupStatus/updated" => tools_failed(params).into_iter().collect(),
        "account/rateLimits/updated" => params["rateLimits"]["primary"]["usedPercent"]
            .as_f64()
            .filter(|used| *used >= 90.0)
            .map(|used| Update::Notice(format!("Your plan's Codex usage is at {used:.0}%")))
            .into_iter()
            .collect(),
        _ => vec![],
    }
}

/// Message text, after a paragraph break when it follows other text.
fn say(state: &mut State, item: &str, text: &str) -> Vec<Update> {
    let mut updates = vec![];
    if !state.streamed.iter().any(|s| s == item) {
        if state.spoke {
            updates.push(Update::Text("\n\n".into()));
        }
        state.streamed.push(item.to_owned());
    }
    state.spoke = true;
    updates.push(Update::Text(text.to_owned()));
    updates
}

/// A retried error is shown at once; another one ends the turn if it fails.
fn reported_error(state: &mut State, params: &Json) -> Option<Update> {
    let message = params["error"]["message"]
        .as_str()
        .unwrap_or("Codex reported an error")
        .to_owned();
    if params["willRetry"].as_bool().unwrap_or(false) {
        return Some(Update::Notice(format!("{message} (Codex is retrying)")));
    }
    state.error = Some(message);
    None
}

fn turn_completed(state: &mut State, turn: &Json) -> Update {
    state.turn = None;
    state.starting = None;
    state.cancel_requested = false;
    let error = (turn["status"] == "failed").then(|| {
        turn["error"]["message"]
            .as_str()
            .map(str::to_owned)
            .or_else(|| state.error.take())
            .unwrap_or_else(|| "The turn failed".into())
    });
    Update::TurnEnded { error }
}

/// TursoBench's relay did not start in Codex.
fn tools_failed(params: &Json) -> Option<Update> {
    if params["name"] != crate::relay::SERVER_NAME
        || matches!(params["status"].as_str(), Some("starting" | "ready"))
    {
        return None;
    }
    let reason = params["error"]
        .as_str()
        .or(params["failureReason"].as_str())
        .unwrap_or("no reason given");
    Some(Update::Notice(format!(
        "TursoBench's tools did not start in Codex ({}): {reason}",
        params["status"].as_str().unwrap_or("unknown")
    )))
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
            match item["text"].as_str().filter(|t| !t.is_empty()) {
                Some(text) => say(state, id, text),
                None => vec![],
            }
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
fn request(link: &mut Link, method: &str, id: &Json, params: &Json) {
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
    use super::{Json, Spec, State, Update, json, notification, serve, thread_params};

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
        // Only TursoBench's own relay failing to start is worth a notice.
        let startup = |name: &str, status: &str| {
            let params = json!({"name": name, "status": status, "error": "spawn failed"});
            notification(
                &mut State::default(),
                "mcpServer/startupStatus/updated",
                &params,
            )
        };
        assert!(startup("tursobench", "ready").is_empty());
        assert!(startup("other", "failed").is_empty());
        assert_eq!(
            startup("tursobench", "failed"),
            [Update::Notice(
                "TursoBench's tools did not start in Codex (failed): spawn failed".into()
            )]
        );
    }

    #[test]
    fn the_thread_is_ephemeral_read_only_and_carries_only_our_server() {
        let mut spec = Spec {
            cwd: "/run/tb".into(),
            system_prompt: "Be brief".into(),
            model: Some(" gpt-x ".into()),
            ..Spec::default()
        };
        let launch = crate::relay::Launch {
            command: "/usr/bin/tursobench".into(),
            args: vec!["--mcp-relay".into(), "/run/tb/s.sock".into()],
            env: vec![(crate::relay::TOKEN_ENV.into(), "secret".into())],
        };
        serve(&mut spec, launch);
        let joined = spec.args.join(" ");
        assert!(joined.starts_with("app-server "));
        assert!(joined.contains("--disable shell_tool"));
        assert!(joined.contains("web_search=\"disabled\""));
        assert!(joined.contains("-c analytics.enabled=false"));
        assert!(joined.contains("-c otel.metrics_exporter=\"none\""));
        assert!(joined.contains("-c history.persistence=\"none\""));
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
