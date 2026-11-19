//! A client for the Agent Client Protocol (v1, JSON-RPC over stdio), used for
//! Copilot CLI and any other ACP agent.
//!
//! TursoBench advertises no file-system or terminal capabilities, answers
//! agent requests for them with "method not found", grants permission only for
//! its own tools and declines everything else. It never calls `authenticate`:
//! the separate account module starts vendor-owned sign-in flows.
use crate::{
    account::config_option,
    agent::{Input, Link, Queue, Spec, Update, content_text, tool_name},
};
use serde_json::{Value as Json, json};
use std::sync::mpsc::Receiver;

const PROTOCOL_VERSION: u64 = 1;
const INITIALIZE: u64 = 1;
const NEW_SESSION: u64 = 2;
const SET_MODEL: u64 = 3;
const SET_MODE: u64 = 4;
const SET_EFFORT: u64 = 5;

#[derive(Default)]
struct State {
    session: Option<String>,
    next_id: u64,
    prompt: Option<u64>,
    queued: Queue,
    /// The standing instructions travel with the first prompt.
    instructed: bool,
    mode_pending: bool,
    model_pending: bool,
    original_model: Option<String>,
    /// The reasoning level the user chose; `None` restores `original_effort`.
    effort: Option<String>,
    effort_pending: bool,
    /// The session's `thought_level` config option, its levels and first value.
    effort_option: Option<String>,
    effort_values: Vec<String>,
    original_effort: Option<String>,
}

pub(crate) fn drive(spec: &Spec, input: &Receiver<Input>, link: &mut Link) {
    let mut state = State {
        next_id: 10,
        effort: spec.effort.clone(),
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
    if !link.write_or_end_turn(&initialize, "the agent") {
        return;
    }
    while let Ok(message) = input.recv() {
        match message {
            Input::Prompt(text) => {
                state.queued.push(text);
                flush(spec, &mut state, link);
            }
            Input::Effort(level) => {
                state.effort = level;
                if state.session.is_some() {
                    apply_effort(&mut state, link);
                }
            }
            Input::Cancel => {
                if let Some(session) = &state.session {
                    let _ = link.write(&json!({
                        "jsonrpc": "2.0",
                        "method": "session/cancel",
                        "params": {"sessionId": session},
                    }));
                } else {
                    state.queued.cancel(link);
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
    if state
        .queued
        .refuse(link, "The agent could not start a session")
    {
        return;
    }
    let Some(session) = state.session.clone() else {
        return;
    };
    if state.prompt.is_some() || state.mode_pending || state.model_pending || state.effort_pending {
        return;
    }
    let Some(mut text) = state.queued.pop() else {
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
    if !link.write_or_end_turn(&request, "the agent") {
        state.prompt = None;
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

/// An error reply's message, with the agent's detail when it gives one.
fn error_text(message: &Json) -> Option<String> {
    message.get("error").map(|e| {
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
    })
}

/// Why a turn that ended without an error reply still did not answer.
const STOP_REASONS: &[(&str, &str)] = &[
    ("refusal", "The agent declined to answer"),
    ("max_tokens", "The reply reached the agent's length limit"),
    (
        "max_turn_requests",
        "The agent reached its limit of steps for one turn",
    ),
];

fn response(spec: &Spec, state: &mut State, link: &mut Link, id: Option<u64>, message: &Json) {
    match id {
        Some(SET_MODEL) => state.model_pending = false,
        Some(SET_EFFORT) => state.effort_pending = false,
        Some(SET_MODE) => state.mode_pending = false,
        _ => {}
    }
    let result = &message["result"];
    match (id, error_text(message)) {
        (Some(INITIALIZE), None) => new_session(spec, link),
        (Some(INITIALIZE), Some(error)) => state.queued.fail(link, error),
        (Some(NEW_SESSION), None) => session_started(spec, state, link, result),
        (Some(NEW_SESSION), Some(error)) => state.queued.fail(link, sign_in_hint(message, error)),
        (Some(SET_MODEL), Some(error)) => {
            link.emit(Update::Ready {
                model: state.original_model.clone(),
            });
            link.emit(Update::Notice(format!(
                "The agent kept its default model: {error}"
            )));
        }
        (Some(SET_EFFORT), Some(error)) => link.emit(Update::Notice(format!(
            "The agent kept its reasoning level: {error}"
        ))),
        (Some(SET_MODE), Some(error)) => {
            let error = format!("Could not select the agent's default permission mode: {error}");
            state.queued.fail(link, error);
        }
        (Some(other), error) if Some(other) == state.prompt => {
            state.prompt = None;
            let error = error.or_else(|| {
                let reason = result["stopReason"].as_str()?;
                let (_, text) = STOP_REASONS.iter().find(|(r, _)| *r == reason)?;
                Some((*text).to_owned())
            });
            link.emit(Update::TurnEnded { error });
        }
        _ => {}
    }
}

/// A session the agent refused, most often for want of a sign-in.
fn sign_in_hint(message: &Json, error: String) -> String {
    if message["error"]["code"] == -32000 || error.to_lowercase().contains("auth") {
        format!("{error}. Sign in with the agent's own CLI in a terminal, then try again.")
    } else {
        error
    }
}

/// A session whose only MCP server is TursoBench's relay.
fn new_session(spec: &Spec, link: &mut Link) {
    let servers: Vec<_> = spec
        .mcp
        .iter()
        .map(|mcp| {
            json!({
                "name": crate::relay::SERVER_NAME,
                "command": mcp.command,
                "args": mcp.args,
                "env": mcp.env.iter().map(|(name, value)| json!({"name": name, "value": value})).collect::<Vec<_>>(),
            })
        })
        .collect();
    let _ = link.write(&json!({
        "jsonrpc": "2.0",
        "id": NEW_SESSION,
        "method": "session/new",
        "params": {"cwd": spec.cwd, "mcpServers": servers},
    }));
}

/// Adopt the new session: keep the normal approval mode, then apply the
/// chosen reasoning level and model.
fn session_started(spec: &Spec, state: &mut State, link: &mut Link, result: &Json) {
    let Some(session) = result["sessionId"].as_str() else {
        return state
            .queued
            .fail(link, "The agent returned no session".into());
    };
    state.session = Some(session.to_owned());
    if let Err(error) = default_mode(state, link, session, result) {
        return state.queued.fail(link, error);
    }
    let model = config_option(result, "model");
    state.original_model = model
        .and_then(|o| o["currentValue"].as_str())
        .or_else(|| result["models"]["currentModelId"].as_str())
        .map(str::to_owned);
    let thought = config_option(result, "thought_level");
    state.effort_option = thought.and_then(|o| o["id"].as_str()).map(str::to_owned);
    state.original_effort = thought
        .and_then(|o| o["currentValue"].as_str())
        .map(str::to_owned);
    state.effort_values = crate::account::acp_efforts(result)
        .into_iter()
        .map(|e| e.id)
        .collect();
    if state.effort.is_some() {
        apply_effort(state, link);
    }
    let model = choose_model(spec, state, link, result);
    link.emit(Update::Ready { model });
}

/// Never inherit Auto when the agent offers the normal approval mode.
fn default_mode(
    state: &mut State,
    link: &mut Link,
    session: &str,
    result: &Json,
) -> Result<(), String> {
    let mode_option = result["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| {
            o["category"] == "mode"
                && o["options"]
                    .as_array()
                    .is_some_and(|options| options.iter().any(|v| v["value"] == "default"))
        });
    if let Some(config) = mode_option.and_then(|o| o["id"].as_str()) {
        state.mode_pending = true;
        return set_config(link, SET_MODE, session, config, "default")
            .map_err(|_| "Could not select the agent's default permission mode".into());
    }
    if result["modes"]["availableModes"]
        .as_array()
        .is_some_and(|modes| modes.iter().any(|m| m["id"] == "default"))
    {
        state.mode_pending = true;
        let _ = link.write(&json!({"jsonrpc": "2.0", "id": SET_MODE, "method": "session/set_mode", "params": {"sessionId": session, "modeId": "default"}}));
    }
    Ok(())
}

fn set_config(
    link: &mut Link,
    id: u64,
    session: &str,
    config: &str,
    value: &str,
) -> std::io::Result<()> {
    link.write(&json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": "session/set_config_option",
        "params": {"sessionId": session, "configId": config, "value": value},
    }))
}

/// Set the session's `thought_level` to the chosen level, or back to the one
/// it started with.
fn apply_effort(state: &mut State, link: &mut Link) {
    let Some(session) = state.session.clone() else {
        return;
    };
    let Some(config) = state.effort_option.clone() else {
        if state.effort.is_some() {
            link.emit(Update::Notice(
                "The agent does not offer reasoning levels".into(),
            ));
        }
        return;
    };
    let Some(value) = state
        .effort
        .clone()
        .or_else(|| state.original_effort.clone())
    else {
        return;
    };
    if !state.effort_values.contains(&value) {
        link.emit(Update::Notice(format!(
            "The agent does not offer the reasoning level {value}"
        )));
        return;
    }
    state.effort_pending = true;
    let _ = set_config(link, SET_EFFORT, &session, &config, &value);
}

/// Apply the preferred model through config options or the model list; return
/// the name of the model in use when the agent says.
fn choose_model(spec: &Spec, state: &mut State, link: &mut Link, result: &Json) -> Option<String> {
    let session = state.session.clone()?;
    let option = config_option(result, "model");
    let current = option
        .map_or(&result["models"]["currentModelId"], |o| &o["currentValue"])
        .as_str()
        .map(str::to_owned);
    let Some(wanted) = spec
        .model
        .as_deref()
        .map(str::trim)
        .filter(|m| !m.is_empty())
    else {
        return current;
    };
    let offered = |id: &&str| id.eq_ignore_ascii_case(wanted) || id.contains(wanted);
    let change = match option {
        Some(option) => {
            let models = crate::account::acp_models(result);
            let value = models.iter().map(|m| m.id.as_str()).find(offered);
            value.zip(option["id"].as_str()).map(|(value, config)| {
                let params = json!({"sessionId": session, "configId": config, "value": value});
                (value.to_owned(), "session/set_config_option", params)
            })
        }
        None => result["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|m| m["modelId"].as_str())
            .find(offered)
            .map(|model| {
                let params = json!({"sessionId": session, "modelId": model});
                (model.to_owned(), "session/set_model", params)
            }),
    };
    let Some((model, method, params)) = change else {
        link.emit(Update::Notice(format!(
            "The agent does not offer the model {wanted}"
        )));
        return current;
    };
    state.model_pending = true;
    let _ =
        link.write(&json!({"jsonrpc": "2.0", "id": SET_MODEL, "method": method, "params": params}));
    Some(model)
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
    const TOOLS: &[&str] = &["list_tables", "describe_table", "run_query", "propose_sql"];
    let qualified = |name: &str| {
        [
            "mcp__tursobench__",
            "mcp_tursobench_",
            "tursobench/",
            "tursobench.",
            "tursobench__",
        ]
        .iter()
        .any(|prefix| {
            name.strip_prefix(prefix)
                .is_some_and(|tool| TOOLS.contains(&tool))
        })
    };
    // A title is display text, often containing shell arguments or file paths.
    // Accept only a complete qualified tool name (and the known "Tool: " label).
    let title = call["title"].as_str().unwrap_or_default();
    let identity = call["toolName"].as_str().or_else(|| call["name"].as_str());
    match identity {
        Some(name) => {
            qualified(name)
                || (call["rawInput"]["server"] == crate::relay::SERVER_NAME
                    && TOOLS.contains(&name))
        }
        None => qualified(title.strip_prefix("Tool: ").unwrap_or(title)),
    }
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
        .map(|c| content_text(&json!([c.get("content").unwrap_or(c)])))
        .filter(|t| !t.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    let text = if text.is_empty() {
        update
            .get("rawOutput")
            .filter(|o| !o.is_null())
            .map(|o| match o {
                Json::String(s) => s.clone(),
                other => {
                    let text = content_text(&other["content"]).trim().to_owned();
                    if text.is_empty() {
                        other.to_string()
                    } else {
                        text
                    }
                }
            })
            .filter(|t| !t.is_empty())
            .unwrap_or_else(|| {
                if ok {
                    "Completed; the agent did not include tool output".into()
                } else {
                    "Failed; the agent did not include tool output".into()
                }
            })
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
    use super::{Update, is_ours, json, session_update, short_title, tool_name};
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
        // A finished call's output comes from its content, its raw output or neither.
        for (fields, ok, text) in [
            (
                json!({"status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "{\"rows\":[]}"}}]}),
                true,
                "{\"rows\":[]}",
            ),
            (
                json!({"status": "completed", "content": [{"type": "text", "text": "result"}]}),
                true,
                "result",
            ),
            (
                json!({"status": "completed", "rawOutput": {"rows": []}}),
                true,
                "{\"rows\":[]}",
            ),
            (
                json!({"status": "completed", "rawOutput": {"content": [{"type": "text", "text": " ok "}]}}),
                true,
                "ok",
            ),
            (
                json!({"status": "failed", "rawOutput": "denied"}),
                false,
                "denied",
            ),
            (
                json!({"status": "completed"}),
                true,
                "Completed; the agent did not include tool output",
            ),
            (
                json!({"status": "failed"}),
                false,
                "Failed; the agent did not include tool output",
            ),
        ] {
            let mut update = json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1"});
            let fields = fields.as_object().unwrap().clone();
            update.as_object_mut().unwrap().extend(fields);
            let result = Update::ToolResult {
                id: "c1".into(),
                ok,
                text: text.into(),
            };
            assert_eq!(session_update(&update), [result], "{update}");
        }
        assert!(
            session_update(
                &json!({"sessionUpdate": "tool_call_update", "toolCallId": "c1", "status": "in_progress"})
            )
            .is_empty()
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
        for title in [
            "Run shell: touch /tmp/tursobench-probe",
            "Edit file /tmp/tursobench/config",
            "tursobench/run_query; rm -rf ~",
            "other_tursobench/run_query",
            "tursobench/unknown",
        ] {
            assert!(!is_ours(&json!({"title": title})), "{title}");
        }
        assert!(!is_ours(
            &json!({"title": "tursobench/run_query", "toolName": "shell"})
        ));
        assert!(!is_ours(
            &json!({"title": "shell", "rawInput": {"server": "tursobench"}})
        ));
        assert!(is_ours(
            &json!({"toolName": "run_query", "rawInput": {"server": "tursobench"}})
        ));
        assert!(is_ours(
            &json!({"name":"mcp_tursobench_list_tables","title":"Run mcp_tursobench_list_tables"})
        ));
        assert!(!is_ours(
            &json!({"name":"mcp_tursobench_list_tables; touch /tmp/probe"})
        ));
        assert!(!is_ours(
            &json!({"name":"shell","title":"Run mcp_tursobench_list_tables"})
        ));
        assert_eq!(
            tool_name(&short_title("Run mcp_tursobench_list_tables")),
            "list_tables"
        );
        assert_eq!(
            short_title("Tool: tursobench.describe_table"),
            "tursobench.describe_table"
        );
        assert_eq!(tool_name("tursobench.describe_table"), "describe_table");
    }
}
