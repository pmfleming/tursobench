//! One running agent CLI. A driver thread owns its stdin and protocol state;
//! the UI sends prompts and receives protocol-neutral updates.
use serde_json::Value as Json;
use std::{
    collections::VecDeque,
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

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Protocol {
    /// Claude Code headless: `claude -p` with stream-json input and output.
    #[default]
    ClaudeStream,
    /// The Agent Client Protocol over stdio.
    Acp,
    /// Codex's own app server: `codex app-server` over stdio.
    CodexAppServer,
}

/// What the user lets the agent CLI keep or report, beyond the conversation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Privacy {
    /// Ask the CLI to send no telemetry, metrics or error reports.
    pub telemetry_off: bool,
    /// Let the CLI save the conversation in its own history.
    pub keep_history: bool,
}
impl Default for Privacy {
    fn default() -> Self {
        Self {
            telemetry_off: true,
            keep_history: false,
        }
    }
}

/// Everything needed to start and talk to one agent.
#[derive(Debug, Clone, Default)]
pub struct Spec {
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
    pub protocol: Protocol,
    /// Sent in the protocol where the command line cannot carry it (ACP, Codex).
    pub system_prompt: String,
    /// The tool relay, for protocols that configure MCP per session (ACP, Codex).
    pub mcp: Option<crate::relay::Launch>,
    pub model: Option<String>,
    /// A reasoning level to use instead of the agent's own; it can change later.
    pub effort: Option<String>,
    /// History and telemetry choices, for protocols that set them per conversation.
    pub privacy: Privacy,
}
impl Spec {
    /// Offer `endpoint`'s tools as the protocol takes them: Claude Code reads
    /// its config file and may use only `tools`; Codex and ACP agents receive
    /// the relay with their conversation.
    pub fn serve(mut self, endpoint: &crate::relay::Endpoint, tools: &[&str]) -> Self {
        match self.protocol {
            Protocol::ClaudeStream => {
                crate::claude::serve(&mut self, endpoint.config_path(), tools);
            }
            Protocol::CodexAppServer => crate::codex::serve(&mut self, endpoint.launch().clone()),
            Protocol::Acp => self.mcp = Some(endpoint.launch().clone()),
        }
        self
    }
}

pub(crate) enum Input {
    Line(String),
    Eof,
    Prompt(String),
    Cancel,
    /// Use this reasoning level from the next turn; `None` restores the agent's own.
    Effort(Option<String>),
}

/// The driver side: write to the agent, report to the UI.
pub(crate) struct Link {
    pub stdin: Option<ChildStdin>,
    pub updates: Sender<Update>,
    pub wake: Wake,
}
impl Link {
    /// A link with no agent behind it, and the updates it reports.
    #[cfg(test)]
    pub fn detached() -> (Self, Receiver<Update>) {
        let (updates, received) = mpsc::channel();
        let wake = Arc::new(|| {});
        let link = Self {
            stdin: None,
            updates,
            wake,
        };
        (link, received)
    }
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
    /// Write `message`, or end the turn saying `agent` cannot be reached.
    pub fn write_or_end_turn(&mut self, message: &Json, agent: &str) -> bool {
        let written = self.write(message);
        if let Err(e) = &written {
            self.emit(Update::TurnEnded {
                error: Some(format!("Could not reach {agent}: {e}")),
            });
        }
        written.is_ok()
    }
}

/// Prompts that wait for a conversation to start, which it may never do.
#[derive(Default)]
pub(crate) struct Queue {
    prompts: VecDeque<String>,
    failed: bool,
}
impl Queue {
    pub fn push(&mut self, prompt: String) {
        self.prompts.push_back(prompt);
    }
    pub fn pop(&mut self) -> Option<String> {
        self.prompts.pop_front()
    }
    /// The conversation will not start: end the turn with `error`, and every
    /// later prompt with [`Queue::refuse`].
    pub fn fail(&mut self, link: &Link, error: String) {
        self.failed = true;
        self.prompts.clear();
        link.emit(Update::TurnEnded { error: Some(error) });
    }
    /// Once the conversation failed, end each waiting turn with `error`;
    /// whether it failed.
    pub fn refuse(&mut self, link: &Link, error: &str) -> bool {
        if self.failed {
            for _ in self.prompts.drain(..) {
                link.emit(Update::TurnEnded {
                    error: Some(error.into()),
                });
            }
        }
        self.failed
    }
    /// Drop the waiting prompts, ending their turn.
    pub fn cancel(&mut self, link: &Link) {
        if !self.prompts.is_empty() {
            self.prompts.clear();
            link.emit(Update::TurnEnded { error: None });
        }
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
        let lines = inbox.clone();
        std::thread::Builder::new()
            .name("agent-stdout".into())
            .spawn(move || read_lines(stdout, &lines))?;
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
                    Protocol::ClaudeStream => crate::claude::drive(&spec, &input, &mut link),
                    Protocol::Acp => crate::acp::drive(&spec, &input, &mut link),
                    Protocol::CodexAppServer => crate::codex::drive(&spec, &input, &mut link),
                }
                link.stdin = None;
                let status = reap(&exit);
                let message = exit_message(status, tail.lock().unwrap().trim());
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
    /// Change the reasoning level from the next message; `None` goes back to the
    /// level the agent started with.
    pub fn set_effort(&self, effort: Option<String>) {
        let _ = self.inbox.send(Input::Effort(effort));
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

/// Pass each line of the agent's output to its driver, then the end of it.
fn read_lines(stdout: impl Read, inbox: &Sender<Input>) {
    for line in BufReader::new(stdout).lines() {
        let Ok(line) = line else { break };
        if inbox.send(Input::Line(line)).is_err() {
            return;
        }
    }
    let _ = inbox.send(Input::Eof);
}

/// Why the agent ended: the last of its error output, or its exit status.
fn exit_message(status: io::Result<std::process::ExitStatus>, detail: &str) -> String {
    match status {
        Ok(s) if s.success() && detail.is_empty() => "The agent ended".into(),
        Ok(s) if detail.is_empty() => format!("The agent ended ({s})"),
        Ok(_) | Err(_) => detail.to_owned(),
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
        .or_else(|| name.strip_prefix(&format!("mcp_{server}_")))
        .or_else(|| name.strip_prefix(&format!("{server}__")))
        .or_else(|| name.strip_prefix(&format!("{server}_")))
        .or_else(|| name.strip_prefix(&format!("{server}/")))
        .or_else(|| name.strip_prefix(&format!("{server}.")))
        .unwrap_or(name)
        .to_owned()
}

/// The JSON-RPC `initialize` request ACP and Codex sessions open with:
/// `params` plus this client's identity.
pub(crate) fn initialize(id: u64, mut params: Json) -> Json {
    params["clientInfo"] = serde_json::json!({
        "name": "tursobench",
        "title": "TursoBench",
        "version": env!("CARGO_PKG_VERSION"),
    });
    serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": params})
}

#[cfg(test)]
mod tests {
    use super::{Link, Queue, Update};

    #[test]
    fn waiting_prompts_end_when_cancelled_or_refused() {
        let (link, updates) = Link::detached();
        let mut queue = Queue::default();
        // Nothing waits, so there is no turn to end.
        queue.cancel(&link);
        queue.push("a".into());
        queue.cancel(&link);
        queue.fail(&link, "no session".into());
        queue.push("b".into());
        assert!(queue.refuse(&link, "refused") && queue.pop().is_none());
        let ended = |error: Option<&str>| Update::TurnEnded {
            error: error.map(str::to_owned),
        };
        let updates: Vec<_> = updates.try_iter().collect();
        let expected = [
            ended(None),
            ended(Some("no session")),
            ended(Some("refused")),
        ];
        assert_eq!(updates, expected);
    }
}
