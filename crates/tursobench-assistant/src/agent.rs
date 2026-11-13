//! One running agent CLI. A driver thread owns its stdin and protocol state;
//! the UI sends prompts and receives protocol-neutral updates.
use serde_json::Value as Json;
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc, Mutex,
        mpsc::{self, Receiver, Sender},
    },
};

pub type Wake = Arc<dyn Fn() + Send + Sync>;
const STDERR_TAIL: usize = 4096;

#[derive(Debug, Clone, PartialEq)]
pub enum Update {
    /// The agent is ready for prompts; `model` when it reports one.
    Ready {
        model: Option<String>,
    },
    /// A fragment of the agent's reply.
    Text(String),
    /// A fragment of visible reasoning.
    Thought(String),
    ToolCall {
        id: String,
        name: String,
        input: Json,
    },
    ToolResult {
        id: String,
        ok: bool,
        text: String,
    },
    /// The current turn finished, with an error message if it failed.
    TurnEnded {
        error: Option<String>,
    },
    /// Something worth showing that is not part of the reply.
    Notice(String),
    /// The process ended; no further updates follow.
    Exited {
        message: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    /// Claude Code headless: `claude -p` with stream-json input and output.
    ClaudeStream,
    /// The Agent Client Protocol over stdio.
    Acp,
}

/// Everything needed to start and talk to one agent.
#[derive(Debug, Clone)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub protocol: Protocol,
    /// Sent in the protocol where the command line cannot carry it (ACP).
    pub system_prompt: String,
    /// The tool relay, for protocols that configure MCP per session (ACP).
    pub mcp: Option<crate::relay::Launch>,
    pub model: Option<String>,
}

pub(crate) enum Input {
    Line(String),
    Eof,
    Prompt(String),
    Cancel,
}

/// The driver side: write to the agent, report to the UI.
pub(crate) struct Link {
    pub stdin: Option<ChildStdin>,
    pub updates: Sender<Update>,
    pub wake: Wake,
}
impl Link {
    pub fn emit(&self, update: Update) {
        if self.updates.send(update).is_ok() {
            (self.wake)();
        }
    }
    pub fn write(&mut self, message: &Json) -> io::Result<()> {
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| io::Error::other("The agent's input is closed"))?;
        let mut line = serde_json::to_string(message)?;
        line.push('\n');
        stdin.write_all(line.as_bytes())?;
        stdin.flush()
    }
}

pub struct Agent {
    inbox: Sender<Input>,
    pub updates: Receiver<Update>,
    child: Arc<Mutex<Child>>,
}
impl Agent {
    pub fn spawn(spec: Spec, wake: Wake) -> io::Result<Self> {
        let mut child = Command::new(&spec.program)
            .args(&spec.args)
            .envs(spec.env.iter().map(|(k, v)| (k, v)))
            .current_dir(&spec.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let child = Arc::new(Mutex::new(child));
        let (inbox, input) = mpsc::channel();
        let (sender, updates) = mpsc::channel();
        let tail = Arc::new(Mutex::new(String::new()));
        {
            let tail = tail.clone();
            std::thread::Builder::new()
                .name("agent-stderr".into())
                .spawn(move || collect_tail(stderr, &tail))?;
        }
        {
            let inbox = inbox.clone();
            std::thread::Builder::new()
                .name("agent-stdout".into())
                .spawn(move || {
                    for line in BufReader::new(stdout).lines() {
                        let Ok(line) = line else { break };
                        if inbox.send(Input::Line(line)).is_err() {
                            return;
                        }
                    }
                    let _ = inbox.send(Input::Eof);
                })?;
        }
        let link = Link {
            stdin,
            updates: sender,
            wake,
        };
        let exit = child.clone();
        std::thread::Builder::new()
            .name("agent-driver".into())
            .spawn(move || {
                let mut link = link;
                match spec.protocol {
                    Protocol::ClaudeStream => crate::claude::drive(&input, &mut link),
                    Protocol::Acp => crate::acp::drive(&spec, &input, &mut link),
                }
                link.stdin = None;
                let status = reap(&exit);
                let detail = tail.lock().unwrap().trim().to_owned();
                let message = match status {
                    Ok(s) if s.success() && detail.is_empty() => "The agent ended".into(),
                    Ok(s) if detail.is_empty() => format!("The agent ended ({s})"),
                    Ok(_) | Err(_) => detail,
                };
                link.emit(Update::Exited { message });
            })?;
        Ok(Self {
            inbox,
            updates,
            child,
        })
    }
    pub fn prompt(&self, text: impl Into<String>) {
        let _ = self.inbox.send(Input::Prompt(text.into()));
    }
    /// Ask the agent to stop the current turn; it still ends it with `TurnEnded`.
    pub fn cancel(&self) {
        let _ = self.inbox.send(Input::Cancel);
    }
}
impl Drop for Agent {
    fn drop(&mut self) {
        // Killing the CLI closes the relay's stdin, which ends the relay too.
        let _ = self.child.lock().unwrap().kill();
        let _ = self.inbox.send(Input::Eof);
    }
}

/// Wait for exit without holding the lock, so `Drop` can always kill; an agent
/// that closed its output but lingers is stopped after two seconds.
fn reap(child: &Mutex<Child>) -> io::Result<std::process::ExitStatus> {
    let started = std::time::Instant::now();
    loop {
        if let Some(status) = child.lock().unwrap().try_wait()? {
            return Ok(status);
        }
        if started.elapsed() > std::time::Duration::from_secs(2) {
            let _ = child.lock().unwrap().kill();
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
}

fn collect_tail(stderr: impl Read, tail: &Mutex<String>) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { return };
        let mut tail = tail.lock().unwrap();
        tail.push_str(&line);
        tail.push('\n');
        if tail.len() > STDERR_TAIL {
            let mut start = tail.len() - STDERR_TAIL;
            while !tail.is_char_boundary(start) {
                start += 1;
            }
            tail.drain(..start);
        }
    }
}

/// Flatten MCP/Anthropic content (a string or a list of text blocks) to text.
pub(crate) fn content_text(content: &Json) -> String {
    match content {
        Json::String(s) => s.clone(),
        Json::Array(parts) => parts
            .iter()
            .filter_map(|p| {
                p.get("text")
                    .and_then(Json::as_str)
                    .or_else(|| p.get("content").and_then(Json::as_str))
            })
            .collect::<Vec<_>>()
            .join("\n"),
        Json::Null => String::new(),
        other => other.to_string(),
    }
}

/// Tool names arrive prefixed by the agent (`mcp__tursobench__run_query`).
pub(crate) fn tool_name(name: &str) -> String {
    let server = crate::relay::SERVER_NAME;
    name.strip_prefix(&format!("mcp__{server}__"))
        .or_else(|| name.strip_prefix(&format!("{server}__")))
        .or_else(|| name.strip_prefix(&format!("{server}/")))
        .or_else(|| name.strip_prefix(&format!("{server}.")))
        .unwrap_or(name)
        .to_owned()
}
