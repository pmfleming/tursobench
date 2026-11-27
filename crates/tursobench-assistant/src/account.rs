//! Short-lived account/discovery processes, isolated from database conversations.
//! Only status, method and plan leave the parser. Credentials remain in the CLI.
use crate::{
    agent::{Protocol, Wake},
    vendor::Provider,
};
use serde_json::{Value as Json, json};
use std::{
    collections::VecDeque,
    io::{BufRead, BufReader, Write},
    os::unix::process::CommandExt,
    path::PathBuf,
    process::{Child, ChildStdin, Command, Stdio},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver},
    },
    time::{Duration, Instant},
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum SignIn {
    SignedIn,
    SignedOut,
    #[default]
    Unknown,
    Missing,
}
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Model {
    pub id: String,
    pub name: String,
    /// Reasoning levels this model accepts, when the agent lists them per model.
    pub efforts: Vec<String>,
    pub default_effort: Option<String>,
    /// Whether this is the model the agent uses when none is chosen.
    pub default: bool,
}
/// A reasoning level an agent offers whatever the model.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Effort {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
}
#[derive(Debug, Clone, Default)]
pub struct Status {
    pub state: SignIn,
    pub version: Option<String>,
    pub method: Option<String>,
    pub plan: Option<String>,
    pub models: Vec<Model>,
    /// Reasoning levels for every model (ACP agents); Codex lists them per model.
    pub efforts: Vec<Effort>,
    pub auth_methods: Vec<AuthMethod>,
    pub issue: Option<String>,
}
#[derive(Clone, Default)]
pub struct Target {
    pub provider: Provider,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
}
impl Target {
    /// `program` in a process group of its own, so [`kill_group`] reaches
    /// everything it starts, with this target's environment and directory.
    fn command(&self, program: &std::path::Path) -> Command {
        let mut command = Command::new(program);
        command
            .envs(self.env.iter().cloned())
            .current_dir(&self.cwd)
            .process_group(0);
        command
    }
}
#[derive(Clone)]
pub enum Operation {
    Check,
    Login { method: Option<String> },
    Logout,
}
pub enum Event {
    Browser(String),
    Finished(Result<Status, String>),
    /// Stopped by [`Job::cancel`]; nothing is known beyond the last status.
    Cancelled,
}
pub struct Job {
    pub events: Receiver<Event>,
    cancel: Arc<AtomicBool>,
}
impl Job {
    pub fn start(target: Target, operation: Operation, wake: Wake) -> Self {
        let (tx, events) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let stop = cancel.clone();
        let failure = tx.clone();
        let failure_wake = wake.clone();
        let spawned = std::thread::Builder::new()
            .name("agent-account".into())
            .spawn(move || {
                let result = run(&target, &operation, &stop, |url| {
                    let _ = tx.send(Event::Browser(url));
                    wake();
                });
                let _ = tx.send(match result {
                    Err(_) if stop.load(Ordering::Relaxed) => Event::Cancelled,
                    result => Event::Finished(result),
                });
                wake();
            });
        if spawned.is_err() {
            let _ = failure.send(Event::Finished(Err(
                "Could not start the account worker".into()
            )));
            failure_wake();
        }
        Self { events, cancel }
    }
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}
impl Drop for Job {
    fn drop(&mut self) {
        self.cancel();
    }
}

const CHECK: Duration = Duration::from_secs(20);
/// How long `claude auth status` may keep running after printing its answer.
const STATUS_EXIT: Duration = Duration::from_secs(5);
const LOGIN: Duration = Duration::from_secs(600);
const LINE_LIMIT: usize = 1024 * 1024;
const OUTPUT_LIMIT: usize = 4 * LINE_LIMIT;

/// A CLI update (or a concurrent fork inheriting a just-written script) can
/// briefly leave its executable busy on Linux.
fn spawn(command: &mut Command) -> std::io::Result<Child> {
    let mut attempts = 0;
    loop {
        match command.spawn() {
            Err(error)
                if error.raw_os_error() == Some(rustix::io::Errno::TXTBSY.raw_os_error())
                    && attempts < 5 =>
            {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
    }
}

/// Kill the process group `child` leads (it was started with `process_group(0)`)
/// and reap it, so no helper it started keeps running.
fn kill_group(child: &mut Child) {
    use rustix::process::{Pid, Signal, kill_process_group};
    let _ = kill_process_group(Pid::from_child(child), Signal::KILL);
    let _ = child.wait();
}

/// The next line of agent output, bounded before it is allocated so a
/// malformed or flooding peer cannot accumulate unbounded output; `None` at
/// the end of output.
fn next_line(reader: impl BufRead) -> Option<Result<String, String>> {
    let mut bytes = Vec::new();
    match reader
        .take(LINE_LIMIT as u64 + 1)
        .read_until(b'\n', &mut bytes)
    {
        Err(_) => Some(Err("Could not read agent output".into())),
        Ok(0) => None,
        Ok(_) if bytes.len() > LINE_LIMIT => Some(Err("The agent returned too much output".into())),
        Ok(_) => Some(Ok(String::from_utf8_lossy(&bytes).into_owned())),
    }
}

struct Process {
    child: Child,
    stdin: ChildStdin,
    lines: Receiver<Result<String, String>>,
    notices: VecDeque<Json>,
    bytes: usize,
    deadline: Instant,
}
impl Process {
    fn spawn(target: &Target, args: &[String], duration: Duration) -> Result<Self, String> {
        let mut command = target.command(&target.program);
        command
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        let mut child = spawn(&mut command).map_err(|e| -> String {
            if e.kind() == std::io::ErrorKind::NotFound {
                "The agent executable was not found".into()
            } else {
                format!("Could not start the agent executable ({:?})", e.kind())
            }
        })?;
        let stdin = child.stdin.take().expect("piped stdin");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped stdout"));
        let (tx, lines) = mpsc::sync_channel(16);
        let reader = std::thread::Builder::new()
            .name("account-output".into())
            .spawn(move || {
                while let Some(line) = next_line(&mut stdout) {
                    let failed = line.is_err();
                    if tx.send(line).is_err() || failed {
                        return;
                    }
                }
            });
        if reader.is_err() {
            let _ = child.kill();
            let _ = child.wait();
            return Err("Could not read agent output".into());
        }
        Ok(Self {
            child,
            stdin,
            lines,
            notices: VecDeque::new(),
            bytes: 0,
            deadline: Instant::now() + duration,
        })
    }
    /// Stop waiting once cancelled or past the deadline; `late` says what the
    /// agent did not do in time.
    fn waiting(&self, cancel: &AtomicBool, late: &str) -> Result<(), String> {
        if cancel.load(Ordering::Relaxed) {
            return Err("Cancelled".into());
        }
        if Instant::now() >= self.deadline {
            return Err(format!("The agent did not {late} before the timeout"));
        }
        Ok(())
    }
    /// Count a line against the output limit.
    fn count(&mut self, line: Result<String, String>) -> Result<String, String> {
        let line = line?;
        self.bytes += line.len();
        if self.bytes > OUTPUT_LIMIT {
            return Err("The agent returned too much output".into());
        }
        Ok(line)
    }
    fn line(&mut self, cancel: &AtomicBool) -> Result<String, String> {
        loop {
            self.waiting(cancel, "respond")?;
            match self.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => return self.count(line),
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err("The agent closed its output".into());
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        }
    }
    fn write(&mut self, message: Json) -> Result<(), String> {
        writeln!(self.stdin, "{message}")
            .and_then(|_| self.stdin.flush())
            .map_err(|_| "Could not reach the agent".into())
    }
    fn send(&mut self, id: u64, method: &str, params: Json) -> Result<(), String> {
        self.write(json!({"jsonrpc":"2.0", "id":id, "method":method, "params":params}))
    }
    fn message(&mut self, cancel: &AtomicBool) -> Result<Json, String> {
        loop {
            let line = self.line(cancel)?;
            let Ok(message) = serde_json::from_str::<Json>(&line) else {
                continue;
            };
            if let (Some(id), Some(_)) = (message.get("id"), message.get("method")) {
                self.write(json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"Account processes provide no tools or tokens"}}))?;
            } else {
                return Ok(message);
            }
        }
    }
    fn response(&mut self, id: u64, cancel: &AtomicBool) -> Result<Json, String> {
        loop {
            let message = self.message(cancel)?;
            if message["id"] == id {
                return Ok(message);
            }
            if self.notices.len() >= 64 {
                self.notices.pop_front();
            }
            self.notices.push_back(message);
        }
    }
    fn rpc(
        &mut self,
        id: u64,
        method: &str,
        params: Json,
        cancel: &AtomicBool,
    ) -> Result<Json, String> {
        self.send(id, method, params)?;
        let response = self.response(id, cancel)?;
        if response.get("error").is_some() {
            return Err(format!("The agent could not complete {method}"));
        }
        Ok(response["result"].clone())
    }
    fn wait(&mut self, cancel: &AtomicBool) -> Result<bool, String> {
        loop {
            while let Ok(line) = self.lines.try_recv() {
                self.count(line)?;
            }
            self.waiting(cancel, "finish")?;
            if let Some(status) = self
                .child
                .try_wait()
                .map_err(|_| "Could not wait for the agent")?
            {
                return Ok(status.success());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        // Each helper owns a process group, including browser callback and relay
        // children. Cancelling or closing the app must not leave one running.
        kill_group(&mut self.child);
    }
}
fn field(value: &Json) -> Option<String> {
    value
        .as_str()
        .filter(|s| s.len() <= 200 && !s.chars().any(char::is_control))
        .map(str::to_owned)
}
/// A display name, or the id when the agent gives none.
fn name_or(value: &Json, id: &str) -> String {
    field(value).unwrap_or_else(|| id.to_owned())
}
fn auth_error(response: &Json) -> bool {
    let message = response["error"]["message"]
        .as_str()
        .unwrap_or("")
        .to_ascii_lowercase();
    response["error"]["code"] == -32000
        && (message.contains("auth") || message.contains("sign in") || message.contains("login"))
}
fn claude_status(value: &Json) -> Status {
    Status {
        state: match value["loggedIn"].as_bool() {
            Some(true) => SignIn::SignedIn,
            Some(false) => SignIn::SignedOut,
            None => SignIn::Unknown,
        },
        method: field(&value["authMethod"]),
        plan: field(&value["subscriptionType"]),
        ..Default::default()
    }
}
fn codex_status(value: &Json) -> Status {
    let account = &value["account"];
    Status {
        state: if account.is_object() {
            SignIn::SignedIn
        } else if value["requiresOpenaiAuth"] == false {
            SignIn::Unknown
        } else if account.is_null() && value["requiresOpenaiAuth"] == true {
            SignIn::SignedOut
        } else {
            SignIn::Unknown
        },
        method: field(&account["type"]),
        plan: field(&account["planType"]),
        ..Default::default()
    }
}
/// One entry of Codex's `model/list`, unless it is hidden.
fn codex_model(model: &Json) -> Option<Model> {
    let id = field(&model["model"])
        .or_else(|| field(&model["id"]))
        .filter(|_| model["hidden"] != true)?;
    Some(Model {
        name: name_or(&model["displayName"], &id),
        efforts: model["supportedReasoningEfforts"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|e| field(&e["reasoningEffort"]))
            .take(20)
            .collect(),
        default_effort: field(&model["defaultReasoningEffort"]),
        default: model["isDefault"] == true,
        id,
    })
}
/// Claude Code's documented aliases, offered before and after a status check.
pub fn claude_models() -> Vec<Model> {
    crate::claude::MODEL_ALIASES
        .iter()
        .map(|id| Model {
            id: (*id).into(),
            // The alias, capitalised as Anthropic names the family.
            name: id[..1].to_uppercase() + &id[1..],
            ..Default::default()
        })
        .collect()
}
/// Claude Code's documented `--effort` levels; the CLI caps them at what the
/// model supports.
pub fn claude_efforts() -> Vec<Effort> {
    crate::claude::EFFORT_LEVELS
        .iter()
        .map(|id| Effort {
            id: (*id).into(),
            name: (*id).into(),
        })
        .collect()
}
/// An ACP session's config option of `category`.
pub(crate) fn config_option<'a>(result: &'a Json, category: &str) -> Option<&'a Json> {
    result["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["category"] == category)
}
/// A config option's values as `(id, name)`, grouped ones flattened; at most `limit`.
fn option_values(option: &Json, limit: usize) -> Vec<(String, String)> {
    fn collect(values: &Json, limit: usize, found: &mut Vec<(String, String)>) {
        for option in values.as_array().into_iter().flatten().take(limit) {
            if found.len() >= limit {
                return;
            }
            if option["options"].is_array() {
                collect(&option["options"], limit, found);
            } else if let Some(id) = field(&option["value"]) {
                let name = name_or(&option["name"], &id);
                found.push((id, name));
            }
        }
    }
    let mut found = vec![];
    collect(&option["options"], limit, &mut found);
    found
}
/// The levels of an ACP session's `thought_level` config option.
pub fn acp_efforts(result: &Json) -> Vec<Effort> {
    config_option(result, "thought_level")
        .map_or_else(Vec::new, |option| option_values(option, 50))
        .into_iter()
        .map(|(id, name)| Effort { id, name })
        .collect()
}
/// The models of an ACP session's `model` config option, or else of its model list.
pub fn acp_models(result: &Json) -> Vec<Model> {
    let values = match config_option(result, "model") {
        Some(option) => option_values(option, 200),
        None => result["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .take(200)
            .filter_map(|model| {
                let id = field(&model["modelId"])?;
                Some((id.clone(), name_or(&model["name"], &id)))
            })
            .collect(),
    };
    values
        .into_iter()
        .map(|(id, name)| Model {
            id,
            name,
            ..Default::default()
        })
        .collect()
}
fn protocol_args(target: &Target) -> Vec<String> {
    if target.provider.vendor().protocol != Protocol::CodexAppServer {
        return target.args.clone();
    }
    let config = crate::codex::QUIET_CONFIG.iter().copied();
    crate::codex::app_server(config.chain(["mcp_servers={}", crate::codex::NO_HISTORY]))
}
fn initialize(
    target: &Target,
    operation: &Operation,
    cancel: &AtomicBool,
) -> Result<(Process, Json), String> {
    // A browser sign-in waits for the user.
    let duration = match operation {
        Operation::Login { .. } => LOGIN,
        _ => CHECK,
    };
    let mut process = Process::spawn(target, &protocol_args(target), duration)?;
    let result = process.rpc(1, "initialize", json!({"clientInfo":{"name":"tursobench","version":env!("CARGO_PKG_VERSION")}, "protocolVersion":1, "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}), cancel)?;
    if target.provider == Provider::Codex {
        process.write(json!({"method":"initialized","params":{}}))?;
    }
    Ok((process, result))
}
/// Carry out `operation` with the provider's own account commands, then report
/// the resulting status.
fn run(
    target: &Target,
    operation: &Operation,
    cancel: &AtomicBool,
    browser: impl Fn(String),
) -> Result<Status, String> {
    let target = &quiet(target);
    if !target.program.is_file() {
        return Ok(Status {
            state: SignIn::Missing,
            ..Default::default()
        });
    }
    let mut status = match target.provider {
        Provider::Claude => claude_run(target, operation, cancel)?,
        Provider::Codex => codex_run(target, operation, cancel, browser)?,
        _ => acp_run(target, operation, cancel)?,
    };
    status.version = version(target, cancel);
    Ok(status)
}
/// The target, with Claude Code's non-essential traffic switched off.
fn quiet(target: &Target) -> Target {
    let mut target = target.clone();
    if target.provider == Provider::Claude {
        target.env.extend(crate::claude::quiet_env());
    }
    target
}
fn claude_run(
    target: &Target,
    operation: &Operation,
    cancel: &AtomicBool,
) -> Result<Status, String> {
    match operation {
        Operation::Login { .. } => {
            let preferred = std::env::var("TERMINAL").ok();
            let found = terminals(preferred.as_deref(), &crate::vendor::search_dirs());
            claude_login(target, &found, cancel)?;
        }
        Operation::Logout => {
            let mut p = Process::spawn(target, &["auth".into(), "logout".into()], CHECK)?;
            if !p.wait(cancel)? {
                return Err("Claude Code could not sign out".into());
            }
        }
        Operation::Check => {}
    }
    Ok(Status {
        models: claude_models(),
        ..claude_check(target, cancel)?
    })
}
fn codex_run(
    target: &Target,
    operation: &Operation,
    cancel: &AtomicBool,
    browser: impl Fn(String),
) -> Result<Status, String> {
    let (mut p, _) = initialize(target, operation, cancel)?;
    match operation {
        Operation::Login { .. } => codex_login(&mut p, cancel, browser)?,
        Operation::Logout => {
            p.rpc(2, "account/logout", json!({}), cancel)?;
        }
        Operation::Check => {}
    }
    let value = p.rpc(4, "account/read", json!({"refreshToken":false}), cancel)?;
    let mut status = codex_status(&value);
    codex_models(&mut p, &mut status, cancel);
    Ok(status)
}
/// Open ChatGPT sign-in in the browser and wait until Codex reports it done.
fn codex_login(
    p: &mut Process,
    cancel: &AtomicBool,
    browser: impl Fn(String),
) -> Result<(), String> {
    let result = p.rpc(2, "account/login/start", json!({"type":"chatgpt"}), cancel)?;
    browser(field_url(&result["authUrl"]).ok_or("Codex returned no browser sign-in URL")?);
    let login_id = &result["loginId"];
    loop {
        let event = match p.notices.pop_front().map_or_else(|| p.message(cancel), Ok) {
            Ok(event) => event,
            Err(error) => {
                let _ = p.send(3, "account/login/cancel", json!({"loginId":login_id}));
                return Err(error);
            }
        };
        if event["method"] == "account/login/completed" && event["params"]["loginId"] == *login_id {
            if event["params"]["success"] != true {
                return Err(
                    "Codex sign-in did not complete. Try again or use codex login in a terminal."
                        .into(),
                );
            }
            return Ok(());
        }
    }
}
/// Codex's visible models, page by page; a page that fails becomes the issue.
fn codex_models(p: &mut Process, status: &mut Status, cancel: &AtomicBool) {
    let mut cursor = Json::Null;
    for page in 0..10 {
        let params = json!({"limit":100,"cursor":cursor});
        let result = match p.rpc(10 + page, "model/list", params, cancel) {
            Ok(result) => result,
            Err(error) => {
                status.issue = Some(error);
                return;
            }
        };
        for model in result["data"].as_array().into_iter().flatten() {
            if let Some(model) = codex_model(model)
                && !status.models.iter().any(|m| m.id == model.id)
            {
                status.models.push(model);
            }
        }
        cursor = result["nextCursor"].clone();
        if cursor.is_null() {
            return;
        }
    }
}
fn acp_run(target: &Target, operation: &Operation, cancel: &AtomicBool) -> Result<Status, String> {
    let (mut p, initialized) = initialize(target, operation, cancel)?;
    let methods = auth_methods(&initialized);
    match operation {
        Operation::Login { method } => {
            let method = login_method(&methods, method.as_deref())
                .ok_or("This agent requires sign-in through its terminal command")?;
            p.rpc(2, "authenticate", json!({"methodId":method.id}), cancel)?;
        }
        Operation::Logout => return Err("Use this agent's own terminal command to sign out".into()),
        Operation::Check => {}
    }
    let closes = initialized["agentCapabilities"]["sessionCapabilities"]["close"].is_object();
    Ok(Status {
        auth_methods: methods,
        ..acp_session(&mut p, &target.cwd, closes, cancel)?
    })
}
/// Sign-in methods an ACP client can start; terminal and secret-based ones
/// need the agent's own command.
fn auth_methods(initialized: &Json) -> Vec<AuthMethod> {
    initialized["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = field(&m["id"]).filter(|id| {
                !(id.contains("key") || id.contains("terminal") || id == "agent-platform")
            })?;
            Some(AuthMethod {
                name: name_or(&m["name"], &id),
                id,
            })
        })
        .take(20)
        .collect()
}
/// The chosen method, or else the personal OAuth one, or else the first.
fn login_method<'a>(methods: &'a [AuthMethod], chosen: Option<&str>) -> Option<&'a AuthMethod> {
    match chosen {
        Some(id) => methods.iter().find(|m| m.id == id),
        None => methods
            .iter()
            .find(|m| m.id == "oauth-personal")
            .or_else(|| methods.first()),
    }
}
/// Whether an empty session starts, with the models and levels it offers. The
/// session is closed again when the agent `closes` sessions.
fn acp_session(
    p: &mut Process,
    cwd: &std::path::Path,
    closes: bool,
    cancel: &AtomicBool,
) -> Result<Status, String> {
    let issue = |issue: &str| Status {
        issue: Some(issue.into()),
        ..Default::default()
    };
    p.send(3, "session/new", json!({"cwd":cwd,"mcpServers":[]}))?;
    let response = p.response(3, cancel)?;
    if response.get("error").is_some() {
        return Ok(if auth_error(&response) {
            Status {
                state: SignIn::SignedOut,
                ..Default::default()
            }
        } else {
            issue("The agent could not create a status session")
        });
    }
    let result = &response["result"];
    let Some(session) = result["sessionId"].as_str() else {
        return Ok(issue("The agent returned no status session"));
    };
    let mut status = Status {
        state: SignIn::SignedIn,
        models: acp_models(result),
        efforts: acp_efforts(result),
        ..Default::default()
    };
    if closes
        && p.rpc(4, "session/close", json!({"sessionId":session}), cancel)
            .is_err()
    {
        status.issue = Some("The empty status session could not be closed".into());
    }
    Ok(status)
}
fn field_url(value: &Json) -> Option<String> {
    value
        .as_str()
        .filter(|url| {
            url.len() <= 16384 && url.starts_with("https://") && !url.chars().any(char::is_control)
        })
        .map(str::to_owned)
}
fn version(target: &Target, cancel: &AtomicBool) -> Option<String> {
    if target.provider == Provider::Custom {
        return None;
    }
    let mut p = Process::spawn(target, &["--version".into()], Duration::from_secs(5)).ok()?;
    let line = p.line(cancel).ok()?;
    let line = line.trim();
    (line.len() <= 200).then(|| line.to_owned())
}
/// Terminals that run a command in a new window, and the arguments that go
/// before the command. Those that wait for the window come first.
const TERMINALS: &[(&str, &[&str])] = &[
    ("xdg-terminal-exec", &[]),
    ("x-terminal-emulator", &["-e"]),
    ("gnome-terminal", &["--wait", "--"]),
    ("konsole", &["--nofork", "-e"]),
    ("ghostty", &["-e"]),
    ("wezterm", &["start", "--always-new-process", "--"]),
    ("kitty", &["--"]),
    ("alacritty", &["-e"]),
    ("foot", &["-e"]),
    ("xterm", &["-e"]),
];
/// A terminal that exits this soon handed the window to a running instance.
const HANDOFF: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_secs(1);

/// Installed terminals in preference order: `$TERMINAL` first, then the
/// desktop's chosen terminal, then common ones.
pub fn terminals(preferred: Option<&str>, dirs: &[PathBuf]) -> Vec<(PathBuf, Vec<String>)> {
    let known = |program: &std::path::Path| {
        let name = program.file_name()?.to_str()?;
        TERMINALS.iter().find(|(n, _)| *n == name).map(|(_, a)| *a)
    };
    let mut found: Vec<(PathBuf, Vec<String>)> = Vec::new();
    if let Some(words) = preferred.and_then(|t| crate::vendor::command_words(t).ok())
        && let Some(program) = crate::vendor::resolve(std::path::Path::new(&words[0]), dirs)
    {
        let mut args = words[1..].to_vec();
        match known(&program) {
            Some(prefix) => args.extend(prefix.iter().map(|s| (*s).to_owned())),
            None => args.push("-e".into()),
        }
        found.push((program, args));
    }
    for (name, prefix) in TERMINALS {
        if let Some(program) = crate::vendor::resolve(std::path::Path::new(name), dirs)
            && !found.iter().any(|(p, _)| *p == program)
        {
            found.push((program, prefix.iter().map(|s| (*s).to_owned()).collect()));
        }
    }
    found
}

/// Open the sign-in command in a terminal. `Ok(true)` means the terminal
/// returned at once, so the sign-in may still be running in another window.
fn terminal_login(
    target: &Target,
    terminals: &[(PathBuf, Vec<String>)],
    cancel: &AtomicBool,
) -> Result<bool, String> {
    for (terminal, prefix) in terminals {
        // The window belongs to the user: no pipes, and it is not killed when
        // the terminal process hands it to another instance and exits.
        let mut command = target.command(terminal);
        command
            .args(prefix)
            .arg(&target.program)
            .args(target.provider.vendor().login)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Ok(mut child) = spawn(&mut command) {
            return await_terminal(&mut child, cancel);
        }
    }
    Err("No supported terminal was found. Set $TERMINAL, or copy the sign-in command and run it in your terminal, then Scan again.".into())
}

/// Wait for a sign-in terminal; `Ok(true)` when it handed the window off. Its
/// group is killed if the sign-in is cancelled or takes too long.
fn await_terminal(child: &mut Child, cancel: &AtomicBool) -> Result<bool, String> {
    let started = Instant::now();
    loop {
        let stop = if cancel.load(Ordering::Relaxed) {
            Some("Cancelled")
        } else {
            (started.elapsed() >= LOGIN)
                .then_some("Terminal sign-in did not finish before the timeout")
        };
        if let Some(reason) = stop {
            kill_group(child);
            return Err(reason.into());
        }
        match child.try_wait() {
            Ok(Some(status)) if status.success() => return Ok(started.elapsed() < HANDOFF),
            Ok(Some(_)) => {
                return Err("Terminal sign-in did not complete; use the copyable command".into());
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(50)),
            Err(_) => return Err("Could not wait for the terminal".into()),
        }
    }
}

/// Claude Code signs in only in a terminal; wait for it to report signed in.
fn claude_login(
    target: &Target,
    terminals: &[(PathBuf, Vec<String>)],
    cancel: &AtomicBool,
) -> Result<(), String> {
    let started = Instant::now();
    if !terminal_login(target, terminals, cancel)? {
        return Ok(());
    }
    while started.elapsed() < LOGIN {
        if claude_check(target, cancel)?.state == SignIn::SignedIn {
            return Ok(());
        }
        let pause = Instant::now();
        while pause.elapsed() < POLL {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
    }
    Ok(())
}

fn claude_check(target: &Target, cancel: &AtomicBool) -> Result<Status, String> {
    let mut p = Process::spawn(target, &["auth".into(), "status".into()], CHECK)?;
    let mut output = String::new();
    let value = loop {
        output.push_str(&p.line(cancel)?);
        if let Ok(value) = serde_json::from_str::<Json>(&output) {
            break value;
        }
    };
    // Let it exit on its own: it may still be saving a refreshed token, and
    // killing it then can sign the user out. Exit 1 only means signed out.
    p.deadline = p.deadline.min(Instant::now() + STATUS_EXIT);
    let _ = p.wait(cancel);
    Ok(claude_status(&value))
}

#[cfg(test)]
mod tests {
    use super::{
        CHECK, Effort, Event, Job, Operation, Process, Provider, SignIn, Status, Target,
        acp_models, auth_error, claude_login, codex_status, run, terminals,
    };
    use serde_json::json;
    use std::{
        os::unix::fs::PermissionsExt,
        path::{Path, PathBuf},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    fn executable(path: &Path, body: &str) {
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// A Python stand-in for the agent; `send` writes one JSON line.
    fn scripted(provider: Provider, body: &str) -> (tempfile::TempDir, Target) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("agent");
        executable(
            &program,
            &format!(
                "#!/usr/bin/env python3\nimport sys,json\nif '--version' in sys.argv:\n print('test-agent 1.0');sys.exit(0)\n\
                 def send(v): print(json.dumps(v),flush=True)\n{body}"
            ),
        );
        let cwd = dir.path().into();
        let target = Target {
            provider,
            program,
            cwd,
            ..Target::default()
        };
        (dir, target)
    }
    fn operate(target: &Target, operation: Operation) -> Result<Status, String> {
        run(target, &operation, &AtomicBool::new(false), |_| panic!())
    }
    fn check(target: &Target) -> Status {
        operate(target, Operation::Check).unwrap()
    }
    fn private(status: &Status) -> bool {
        let text = format!("{status:?}");
        text.contains("private@example.com") || text.contains("SECRET")
    }

    #[test]
    fn claude_status_discards_secrets_handles_pretty_json_and_may_finish_writing() {
        let (dir, target) = scripted(
            Provider::Claude,
            "import time,pathlib\n\
             print(json.dumps({'loggedIn':True,'authMethod':'claude.ai','subscriptionType':'pro','email':'private@example.com','accessToken':'SECRET','nested':{'ignored':True}},indent=2),flush=True)\n\
             time.sleep(0.3)\npathlib.Path('finished').write_text('1')",
        );
        let status = check(&target);
        assert_eq!(status.state, SignIn::SignedIn);
        assert_eq!(status.method.as_deref(), Some("claude.ai"));
        assert_eq!(status.version.as_deref(), Some("test-agent 1.0"));
        assert!(!private(&status));
        // Claude Code may still be saving a refreshed token after it answers.
        assert!(dir.path().join("finished").exists());
    }

    /// A Codex app server that asks for tokens, signs in through the browser,
    /// signs out and lists its models over two pages.
    const CODEX: &str = r#"
logged_out=False
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');i=m.get('id')
 if method=='initialize':
  send({'id':91,'method':'account/chatgptAuthTokens/refresh','params':{}})
  reply=json.loads(sys.stdin.readline());assert reply['error']['code']==-32601
  send({'id':i,'result':{}})
 elif method=='initialized': pass
 elif method=='account/login/start':
  assert m['params']=={'type':'chatgpt'}
  send({'id':i,'result':{'loginId':'login-1','type':'chatgpt','authUrl':'https://auth.openai.com/test-login'}})
  send({'method':'account/login/completed','params':{'loginId':'unrelated','success':False}})
  send({'method':'account/login/completed','params':{'loginId':'login-1','success':True}})
 elif method=='account/logout': logged_out=True;send({'id':i,'result':{}})
 elif method=='account/read':
  assert m['params']=={'refreshToken':False}
  send({'id':i,'result':{'account':None,'requiresOpenaiAuth':True} if logged_out else {'account':{'type':'chatgpt','planType':'pro','email':'private@example.com','token':'SECRET'}}})
 elif method=='model/list':
  cursor=m['params']['cursor'];model='first' if cursor is None else 'second'
  send({'id':i,'result':{'data':[{'id':model,'model':model,'displayName':model,'isDefault':model=='second','defaultReasoningEffort':'low','supportedReasoningEfforts':[{'reasoningEffort':'low','description':'Fast'},{'reasoningEffort':'high','description':'Deep'}]}], 'nextCursor':'next' if cursor is None else None}})
 else: raise Exception('unexpected method '+str(method))
"#;

    #[test]
    fn codex_refuses_tokens_paginates_models_and_rechecks_after_login_and_logout() {
        let (_dir, target) = scripted(Provider::Codex, CODEX);
        let status = check(&target);
        assert_eq!(status.state, SignIn::SignedIn);
        assert_eq!(
            status
                .models
                .iter()
                .map(|m| m.id.as_str())
                .collect::<Vec<_>>(),
            ["first", "second"]
        );
        // Each model carries its own reasoning levels and default.
        let second = &status.models[1];
        assert!(second.default && !status.models[0].default);
        assert_eq!(second.efforts, ["low", "high"]);
        assert_eq!(second.default_effort.as_deref(), Some("low"));
        assert!(!private(&status));

        let browser = std::cell::RefCell::new(Vec::new());
        let status = run(
            &target,
            &Operation::Login { method: None },
            &AtomicBool::new(false),
            |url| browser.borrow_mut().push(url),
        )
        .unwrap();
        assert_eq!(status.state, SignIn::SignedIn);
        assert_eq!(*browser.borrow(), ["https://auth.openai.com/test-login"]);

        let status = operate(&target, Operation::Logout).unwrap();
        assert_eq!(status.state, SignIn::SignedOut);
    }

    #[test]
    fn a_cancelled_codex_login_stops_without_waiting_for_the_browser() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': send({'id':m['id'],'result':{}})
 elif m.get('method')=='account/login/start': send({'id':m['id'],'result':{'loginId':'l','authUrl':'https://auth.openai.com/test-login'}})
 elif m.get('method')=='account/login/cancel': sys.exit(0)
"#,
        );
        let cancel = AtomicBool::new(false);
        let result = run(&target, &Operation::Login { method: None }, &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        });
        assert_eq!(result.unwrap_err(), "Cancelled");

        // A cancelled job reports that, not a failure.
        let job = Job::start(target, Operation::Login { method: None }, Arc::new(|| {}));
        let wait = Duration::from_secs(5);
        assert!(matches!(
            job.events.recv_timeout(wait),
            Ok(Event::Browser(_))
        ));
        job.cancel();
        assert!(matches!(
            job.events.recv_timeout(wait),
            Ok(Event::Cancelled)
        ));
    }

    #[test]
    fn acp_probes_without_database_or_tools_and_signs_in_only_with_an_advertised_method() {
        let (dir, target) = scripted(
            Provider::Antigravity,
            r#"
for line in sys.stdin:
 m=json.loads(line);method=m['method'];i=m['id']
 if method=='initialize':
  assert not m['params']['clientCapabilities']['terminal']
  send({'id':i,'result':{'agentCapabilities':{'sessionCapabilities':{'close':{}}},'authMethods':[{'id':'oauth-personal','name':'Google'},{'id':'gemini-api-key','name':'Key'},{'id':'oauth-business','name':'Enterprise'}]}})
 elif method=='authenticate':
  assert m['params']=={'methodId':'oauth-business'};open('authenticated','w').close()
  send({'id':i,'result':{}})
 elif method=='session/new':
  assert m['params']['mcpServers']==[]
  send({'id':i,'result':{'sessionId':'empty','configOptions':[{'category':'model','options':[{'value':'m','name':'Model'}]},{'category':'thought_level','options':[{'value':'deep','name':'Deep'}]}]}})
 elif method=='session/close':
  assert m['params']=={'sessionId':'empty'}
  send({'id':i,'result':{}})
 else: raise Exception('unexpected method '+method)
"#,
        );
        let status = check(&target);
        assert_eq!(status.state, SignIn::SignedIn);
        let methods: Vec<_> = status.auth_methods.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(methods, ["oauth-personal", "oauth-business"]);
        assert_eq!(status.models[0].id, "m");
        assert_eq!(
            status.efforts,
            [Effort {
                id: "deep".into(),
                name: "Deep".into()
            }]
        );
        assert_eq!(status.issue, None);
        assert!(!dir.path().join("authenticated").exists());

        let login = |method: &str| {
            operate(
                &target,
                Operation::Login {
                    method: Some(method.into()),
                },
            )
        };
        assert_eq!(login("oauth-business").unwrap().state, SignIn::SignedIn);
        assert!(dir.path().join("authenticated").exists());
        assert!(login("not-advertised").is_err());
    }

    #[test]
    fn authentication_errors_are_distinct_from_unrelated_rpc_errors() {
        assert!(auth_error(
            &json!({"error":{"code":-32000,"message":"Authentication required"}})
        ));
        assert!(!auth_error(
            &json!({"error":{"code":-32000,"message":"Bad workspace"}})
        ));
        assert!(!auth_error(
            &json!({"error":{"code":-32601,"message":"Unknown method"}})
        ));
        assert_eq!(
            codex_status(&json!({"account":null,"requiresOpenaiAuth":false})).state,
            SignIn::Unknown
        );
        // Without a model config option, an ACP session's model list is read.
        let models = acp_models(&json!({"models": {"availableModels": [
            {"modelId": "a", "name": "Alpha"}, {"modelId": "b"}, {"name": "no id"}]}}));
        let named: Vec<_> = models.iter().map(|m| (&*m.id, &*m.name)).collect();
        assert_eq!(named, [("a", "Alpha"), ("b", "b")]);
    }

    #[test]
    fn timeouts_and_cancellation_reap_the_process() {
        let (_dir, target) = scripted(Provider::Custom, "import time\ntime.sleep(30)");
        let stop = AtomicBool::new(false);
        let mut p = Process::spawn(&target, &[], Duration::from_millis(30)).unwrap();
        let pid = p.child.id();
        assert!(p.line(&stop).unwrap_err().contains("timeout"));
        drop(p);
        assert!(!Path::new(&format!("/proc/{pid}")).exists());
        let mut p = Process::spawn(&target, &[], CHECK).unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(p.line(&stop).unwrap_err(), "Cancelled");
    }

    #[test]
    fn terminal_preference_comes_first_and_keeps_known_arguments() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["xterm", "ghostty", "myterm"] {
            executable(&dir.path().join(name), "#!/bin/sh\n");
        }
        let dirs = [dir.path().to_owned()];
        let names = |found: Vec<(PathBuf, Vec<String>)>| {
            found
                .into_iter()
                .map(|(p, a)| (p.file_name().unwrap().to_string_lossy().into_owned(), a))
                .collect::<Vec<_>>()
        };
        let s = |v: &[&str]| v.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert_eq!(
            names(terminals(None, &dirs)),
            [("ghostty".into(), s(&["-e"])), ("xterm".into(), s(&["-e"]))]
        );
        assert_eq!(
            names(terminals(Some("myterm --class x"), &dirs))[0],
            ("myterm".into(), s(&["--class", "x", "-e"]))
        );
        let preferred = names(terminals(Some("xterm"), &dirs));
        assert_eq!(preferred[0].0, "xterm");
        assert_eq!(preferred.len(), 2);
        assert_eq!(names(terminals(Some("missing"), &dirs)).len(), 2);
    }

    /// Sign in to a Claude Code stand-in, whose status follows a marker that
    /// login writes, through a terminal running `terminal`; whether it did.
    fn terminal_sign_in(terminal: &str) -> (Result<(), String>, bool) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("claude");
        executable(
            &program,
            "#!/usr/bin/env bash\nmarker=\"$(dirname \"$0\")/signed-in\"\ncase \"$1 $2\" in\n\
             'auth login') touch \"$marker\";;\n\
             'auth status') if [[ -e $marker ]]; then echo '{\"loggedIn\":true}'; else echo '{\"loggedIn\":false}'; fi;;\n\
             esac\n",
        );
        let cwd = dir.path().into();
        let target = Target {
            program,
            cwd,
            ..Target::default()
        };
        let script = dir.path().join("term");
        executable(&script, terminal);
        let result = claude_login(
            &target,
            &[(script, vec!["-e".into()])],
            &AtomicBool::new(false),
        );
        (result, dir.path().join("signed-in").exists())
    }

    #[test]
    fn a_waiting_terminal_runs_the_sign_in_command() {
        let signed_in = terminal_sign_in("#!/usr/bin/env bash\nshift\nexec \"$@\"\n");
        assert_eq!(signed_in, (Ok(()), true));
    }

    #[test]
    fn a_terminal_that_hands_off_is_polled_and_its_window_survives() {
        let started = Instant::now();
        // Exit at once and finish the sign-in later, as a single-instance terminal does.
        let signed_in = terminal_sign_in(
            "#!/usr/bin/env bash\nshift\n( sleep 0.5; \"$@\" ) >/dev/null 2>&1 &\nexit 0\n",
        );
        assert_eq!(signed_in, (Ok(()), true));
        assert!(started.elapsed() >= Duration::from_millis(500));
    }

    #[test]
    fn a_failed_terminal_is_reported_and_none_found_explains_terminal() {
        let (result, _) = terminal_sign_in("#!/bin/sh\nexit 1\n");
        assert!(result.unwrap_err().contains("did not complete"));
        let (_dir, target) = scripted(Provider::Claude, "");
        assert!(
            claude_login(&target, &[], &AtomicBool::new(false))
                .unwrap_err()
                .contains("$TERMINAL")
        );
    }
}
