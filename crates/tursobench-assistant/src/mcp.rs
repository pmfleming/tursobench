//! A minimal Model Context Protocol server: newline-delimited JSON-RPC 2.0
//! carrying only `initialize`, `ping`, `tools/list` and `tools/call`.
use serde_json::{Value as Json, json};
use std::io::{BufRead, Read, Write};

/// Protocol revisions this server understands, newest first.
const VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];
const MAX_LINE: usize = 4 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub name: &'static str,
    pub description: &'static str,
    pub input_schema: Json,
}

/// What a tool returns: text for the model, and whether it is an error.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}
impl ToolOutput {
    pub fn ok(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: false,
        }
    }
    pub fn error(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            is_error: true,
        }
    }
}

/// The application side of the tool server. Calls arrive on server threads.
pub trait ToolHost: Send + Sync {
    fn tools(&self) -> Vec<ToolSpec>;
    fn call(&self, name: &str, arguments: &Json) -> ToolOutput;
    fn instructions(&self) -> String {
        String::new()
    }
}

/// Serves one client until it disconnects. I/O errors end the session.
pub fn serve(mut reader: impl BufRead, mut writer: impl Write, host: &dyn ToolHost) {
    let mut line = String::new();
    loop {
        line.clear();
        match Read::take(&mut reader, MAX_LINE as u64).read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        if line.len() >= MAX_LINE && !line.ends_with('\n') {
            return;
        }
        if line.trim().is_empty() {
            continue;
        }
        let Some(reply) = handle(line.trim(), host) else {
            continue;
        };
        let Ok(mut text) = serde_json::to_string(&reply) else {
            return;
        };
        text.push('\n');
        if writer.write_all(text.as_bytes()).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

/// The reply to one message, or `None` for notifications and responses.
pub fn handle(line: &str, host: &dyn ToolHost) -> Option<Json> {
    let message: Json = match serde_json::from_str(line) {
        Ok(m) => m,
        Err(e) => return Some(error(Json::Null, -32700, &format!("Parse error: {e}"))),
    };
    let id = message.get("id").cloned()?;
    let Some(method) = message.get("method").and_then(Json::as_str) else {
        // A response to something we never sent; ignore it.
        return None;
    };
    let params = message.get("params").cloned().unwrap_or(Json::Null);
    Some(match method {
        "initialize" => {
            let requested = params
                .get("protocolVersion")
                .and_then(Json::as_str)
                .unwrap_or_default();
            let version = VERSIONS
                .iter()
                .find(|v| **v == requested)
                .unwrap_or(&VERSIONS[0]);
            result(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "tursobench", "version": env!("CARGO_PKG_VERSION")},
                    "instructions": host.instructions(),
                }),
            )
        }
        "ping" => result(id, json!({})),
        "tools/list" => result(
            id,
            json!({
                "tools": host.tools().into_iter().map(|t| json!({
                    "name": t.name,
                    "description": t.description,
                    "inputSchema": t.input_schema,
                    // Every TursoBench tool only reads; agents that honour the
                    // hint (Codex) then call them without asking.
                    "annotations": {"readOnlyHint": true},
                })).collect::<Vec<_>>(),
            }),
        ),
        "tools/call" => {
            let Some(name) = params.get("name").and_then(Json::as_str) else {
                return Some(error(id, -32602, "Missing tool name"));
            };
            if !host.tools().iter().any(|t| t.name == name) {
                return Some(error(id, -32602, &format!("Unknown tool: {name}")));
            }
            let arguments = params.get("arguments").cloned().unwrap_or(json!({}));
            let output = host.call(name, &arguments);
            result(
                id,
                json!({
                    "content": [{"type": "text", "text": output.text}],
                    "isError": output.is_error,
                }),
            )
        }
        other => error(id, -32601, &format!("Method not found: {other}")),
    })
}

fn result(id: Json, result: Json) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}
fn error(id: Json, code: i64, message: &str) -> Json {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    pub(crate) struct Echo;
    impl ToolHost for Echo {
        fn tools(&self) -> Vec<ToolSpec> {
            vec![ToolSpec {
                name: "echo",
                description: "Repeat the text",
                input_schema: json!({"type": "object", "properties": {"text": {"type": "string"}}}),
            }]
        }
        fn call(&self, _: &str, arguments: &Json) -> ToolOutput {
            match arguments.get("text").and_then(Json::as_str) {
                Some(text) => ToolOutput::ok(text),
                None => ToolOutput::error("text is required"),
            }
        }
        fn instructions(&self) -> String {
            "Use echo.".into()
        }
    }

    #[test]
    fn a_session_initializes_lists_and_calls_tools() {
        let input = [
            r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-03-26","capabilities":{},"clientInfo":{"name":"t","version":"1"}}}"#,
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"text":"hi"}}}"#,
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{}}}"#,
            r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"shell"}}"#,
            r#"{"jsonrpc":"2.0","id":"p","method":"ping"}"#,
            r#"{"jsonrpc":"2.0","id":6,"method":"resources/list"}"#,
            "not json",
        ]
        .join("\n");
        let mut out = vec![];
        serve(input.as_bytes(), &mut out, &Echo);
        let replies: Vec<Json> = String::from_utf8(out)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(replies.len(), 8);
        assert_eq!(replies[0]["result"]["protocolVersion"], "2025-03-26");
        assert_eq!(replies[0]["result"]["instructions"], "Use echo.");
        assert_eq!(replies[1]["result"]["tools"][0]["name"], "echo");
        assert_eq!(
            replies[1]["result"]["tools"][0]["annotations"]["readOnlyHint"],
            true
        );
        assert_eq!(replies[2]["result"]["content"][0]["text"], "hi");
        assert_eq!(replies[3]["result"]["isError"], true);
        assert_eq!(replies[4]["error"]["code"], -32602);
        assert_eq!(replies[5]["id"], "p");
        assert_eq!(replies[6]["error"]["code"], -32601);
        assert_eq!(replies[7]["error"]["code"], -32700);
    }
}
