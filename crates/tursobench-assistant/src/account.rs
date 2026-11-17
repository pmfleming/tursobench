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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SignIn {
    SignedIn,
    SignedOut,
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
#[derive(Debug, Clone)]
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
impl Default for Status {
    fn default() -> Self {
        Self {
            state: SignIn::Unknown,
            version: None,
            method: None,
            plan: None,
            models: vec![],
            efforts: vec![],
            auth_methods: vec![],
            issue: None,
        }
    }
}
#[derive(Clone)]
pub struct Target {
    pub provider: Provider,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: PathBuf,
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
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 5 => {
                attempts += 1;
                std::thread::sleep(Duration::from_millis(20));
            }
            result => return result,
        }
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
        let mut command = Command::new(&target.program);
        command
            .args(args)
            .envs(target.env.iter().cloned())
            .current_dir(&target.cwd);
        Self::command(command, duration)
    }
    fn command(mut command: Command, duration: Duration) -> Result<Self, String> {
        command
            .process_group(0)
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
        let stdout = child.stdout.take().expect("piped stdout");
        let (tx, lines) = mpsc::sync_channel(16);
        let reader = std::thread::Builder::new()
            .name("account-output".into())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                loop {
                    let mut bytes = Vec::new();
                    // Bound each line before allocating it; malformed/flooding peers cannot
                    // accumulate unbounded output in either the reader or event queue.
                    loop {
                        let chunk = match reader.fill_buf() {
                            Ok(b) => b,
                            Err(_) => {
                                let _ = tx.send(Err("Could not read agent output".into()));
                                return;
                            }
                        };
                        if chunk.is_empty() {
                            if !bytes.is_empty() {
                                let _ = tx.send(Ok(String::from_utf8_lossy(&bytes).into_owned()));
                            }
                            return;
                        }
                        let n = chunk
                            .iter()
                            .position(|b| *b == b'\n')
                            .map_or(chunk.len(), |i| i + 1);
                        if bytes.len() + n > LINE_LIMIT {
                            let _ = tx.send(Err("The agent returned too much output".into()));
                            return;
                        }
                        let done = chunk[n - 1] == b'\n';
                        bytes.extend_from_slice(&chunk[..n]);
                        reader.consume(n);
                        if done {
                            break;
                        }
                    }
                    if tx
                        .send(Ok(String::from_utf8_lossy(&bytes).into_owned()))
                        .is_err()
                    {
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
    fn line(&mut self, cancel: &AtomicBool) -> Result<String, String> {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            if Instant::now() >= self.deadline {
                return Err("The agent did not respond before the timeout".into());
            }
            match self.lines.recv_timeout(Duration::from_millis(50)) {
                Ok(line) => {
                    let line = line?;
                    self.bytes += line.len();
                    if self.bytes > OUTPUT_LIMIT {
                        return Err("The agent returned too much output".into());
                    }
                    return Ok(line);
                }
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
            for line in self.lines.try_iter() {
                self.bytes += line?.len();
                if self.bytes > OUTPUT_LIMIT {
                    return Err("The agent returned too much output".into());
                }
            }
            if cancel.load(Ordering::Relaxed) {
                return Err("Cancelled".into());
            }
            if Instant::now() >= self.deadline {
                return Err("The agent did not finish before the timeout".into());
            }
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
        unsafe {
            libc::kill(-(self.child.id() as i32), libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}
fn field(value: &Json) -> Option<String> {
    value
        .as_str()
        .filter(|s| s.len() <= 200 && !s.chars().any(char::is_control))
        .map(str::to_owned)
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
/// Claude Code's documented aliases, offered before and after a status check.
pub fn claude_models() -> Vec<Model> {
    crate::claude::MODEL_ALIASES
        .iter()
        .map(|id| Model {
            id: (*id).into(),
            name: (*id).into(),
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
/// The levels of an ACP session's `thought_level` config option.
pub fn acp_efforts(result: &Json) -> Vec<Effort> {
    let mut efforts = vec![];
    fn options(values: &Json, efforts: &mut Vec<Effort>) {
        for option in values.as_array().into_iter().flatten().take(50) {
            if option["options"].is_array() {
                options(&option["options"], efforts);
            } else if let Some(id) = field(&option["value"]) {
                efforts.push(Effort {
                    name: field(&option["name"]).unwrap_or_else(|| id.clone()),
                    id,
                });
            }
        }
    }
    if let Some(option) = result["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["category"] == "thought_level")
    {
        options(&option["options"], &mut efforts);
    }
    efforts.truncate(50);
    efforts
}
pub fn acp_models(result: &Json) -> Vec<Model> {
    let option = result["configOptions"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|o| o["category"] == "model");
    let mut models = Vec::new();
    fn options(values: &Json, models: &mut Vec<Model>) {
        for option in values.as_array().into_iter().flatten().take(200) {
            if option["options"].is_array() {
                options(&option["options"], models);
            } else if let Some(id) = field(&option["value"]) {
                models.push(Model {
                    name: field(&option["name"]).unwrap_or_else(|| id.clone()),
                    id,
                    ..Default::default()
                });
            }
            if models.len() >= 200 {
                break;
            }
        }
    }
    if let Some(option) = option {
        options(&option["options"], &mut models);
    } else {
        for model in result["models"]["availableModels"]
            .as_array()
            .into_iter()
            .flatten()
            .take(200)
        {
            if let Some(id) = field(&model["modelId"]) {
                models.push(Model {
                    name: field(&model["name"]).unwrap_or_else(|| id.clone()),
                    id,
                    ..Default::default()
                });
            }
        }
    }
    models
}
fn protocol_args(target: &Target) -> Vec<String> {
    if target.provider.vendor().protocol != Protocol::CodexAppServer {
        return target.args.clone();
    }
    let mut args = vec!["app-server".into()];
    for feature in crate::codex::DISABLED_FEATURES {
        args.extend(["--disable".into(), (*feature).into()]);
    }
    for config in crate::codex::QUIET_CONFIG
        .iter()
        .copied()
        .chain(["mcp_servers={}", "history.persistence=\"none\""])
    {
        args.extend(["-c".into(), config.into()]);
    }
    args
}
fn initialize(
    target: &Target,
    duration: Duration,
    cancel: &AtomicBool,
) -> Result<(Process, Json), String> {
    let mut process = Process::spawn(target, &protocol_args(target), duration)?;
    let result = process.rpc(1, "initialize", json!({"clientInfo":{"name":"tursobench","version":env!("CARGO_PKG_VERSION")}, "protocolVersion":1, "clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}), cancel)?;
    if target.provider == Provider::Codex {
        process.write(json!({"method":"initialized","params":{}}))?;
    }
    Ok((process, result))
}
fn run(
    target: &Target,
    operation: &Operation,
    cancel: &AtomicBool,
    browser: impl Fn(String),
) -> Result<Status, String> {
    let mut quiet_target = target.clone();
    if target.provider == Provider::Claude {
        quiet_target.env.extend(
            crate::claude::QUIET_ENV
                .iter()
                .map(|(k, v)| ((*k).into(), (*v).into())),
        );
    }
    let target = &quiet_target;
    if !target.program.is_file() {
        return Ok(Status {
            state: SignIn::Missing,
            ..Default::default()
        });
    }
    if target.provider == Provider::Claude {
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
        let mut status = claude_check(target, cancel)?;
        status.models = claude_models();
        status.version = version(target, cancel);
        return Ok(status);
    }
    let duration = if matches!(operation, Operation::Login { .. }) {
        LOGIN
    } else {
        CHECK
    };
    let (mut p, initialized) = initialize(target, duration, cancel)?;
    if target.provider == Provider::Codex {
        match operation {
            Operation::Login { .. } => {
                let result = p.rpc(2, "account/login/start", json!({"type":"chatgpt"}), cancel)?;
                let url =
                    field_url(&result["authUrl"]).ok_or("Codex returned no browser sign-in URL")?;
                browser(url);
                let login_id = result["loginId"].clone();
                loop {
                    let event = match p.notices.pop_front() {
                        Some(event) => event,
                        None => match p.message(cancel) {
                            Ok(event) => event,
                            Err(error) => {
                                let _ =
                                    p.send(3, "account/login/cancel", json!({"loginId":login_id}));
                                return Err(error);
                            }
                        },
                    };
                    if event["method"] == "account/login/completed"
                        && event["params"]["loginId"] == login_id
                    {
                        if event["params"]["success"] != true {
                            return Err("Codex sign-in did not complete. Try again or use codex login in a terminal.".into());
                        }
                        break;
                    }
                }
            }
            Operation::Logout => {
                p.rpc(2, "account/logout", json!({}), cancel)?;
            }
            Operation::Check => {}
        }
        let value = p.rpc(4, "account/read", json!({"refreshToken":false}), cancel)?;
        let mut status = codex_status(&value);
        let mut cursor = Json::Null;
        for page in 0..10 {
            match p.rpc(
                10 + page,
                "model/list",
                json!({"limit":100,"cursor":cursor}),
                cancel,
            ) {
                Ok(result) => {
                    for model in result["data"].as_array().into_iter().flatten() {
                        if let Some(id) = field(&model["model"]).or_else(|| field(&model["id"]))
                            && model["hidden"] != true
                            && !status.models.iter().any(|m| m.id == id)
                        {
                            let efforts = model["supportedReasoningEfforts"]
                                .as_array()
                                .into_iter()
                                .flatten()
                                .filter_map(|e| field(&e["reasoningEffort"]))
                                .take(20)
                                .collect();
                            status.models.push(Model {
                                name: field(&model["displayName"]).unwrap_or_else(|| id.clone()),
                                id,
                                efforts,
                                default_effort: field(&model["defaultReasoningEffort"]),
                                default: model["isDefault"] == true,
                            });
                        }
                    }
                    cursor = result["nextCursor"].clone();
                    if cursor.is_null() {
                        break;
                    }
                }
                Err(error) => {
                    status.issue = Some(error);
                    break;
                }
            }
        }
        status.version = version(target, cancel);
        return Ok(status);
    }
    let methods: Vec<_> = initialized["authMethods"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|m| {
            let id = field(&m["id"])?;
            // Terminal and secret-based methods cannot be served by our ACP client.
            if id.contains("key") || id.contains("terminal") || id == "agent-platform" {
                return None;
            }
            Some(AuthMethod {
                name: field(&m["name"]).unwrap_or_else(|| id.clone()),
                id,
            })
        })
        .take(20)
        .collect();
    match operation {
        Operation::Login { method } => {
            let method = match method.as_deref() {
                Some(id) => methods.iter().find(|m| m.id == id),
                None => methods
                    .iter()
                    .find(|m| m.id == "oauth-personal")
                    .or_else(|| methods.first()),
            };
            let Some(method) = method else {
                return Err("This agent requires sign-in through its terminal command".into());
            };
            p.rpc(2, "authenticate", json!({"methodId":method.id}), cancel)?;
        }
        Operation::Logout => return Err("Use this agent's own terminal command to sign out".into()),
        Operation::Check => {}
    }
    p.send(3, "session/new", json!({"cwd":target.cwd,"mcpServers":[]}))?;
    let result = p.response(3, cancel)?;
    let mut status = Status {
        auth_methods: methods,
        ..Default::default()
    };
    if result.get("error").is_some() {
        if auth_error(&result) {
            status.state = SignIn::SignedOut;
        } else {
            status.issue = Some("The agent could not create a status session".into());
        }
    } else if let Some(session) = result["result"]["sessionId"].as_str() {
        status.state = SignIn::SignedIn;
        status.models = acp_models(&result["result"]);
        status.efforts = acp_efforts(&result["result"]);
        if initialized["agentCapabilities"]["sessionCapabilities"]["close"].is_object()
            && p.rpc(4, "session/close", json!({"sessionId":session}), cancel)
                .is_err()
        {
            status.issue = Some("The empty status session could not be closed".into());
        }
    } else {
        status.issue = Some("The agent returned no status session".into());
    }
    status.version = version(target, cancel);
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
        let started = Instant::now();
        // The window belongs to the user: no pipes, and it is not killed when
        // the terminal process hands it to another instance and exits.
        let mut command = Command::new(terminal);
        command
            .args(prefix)
            .arg(&target.program)
            .args(target.provider.vendor().login)
            .envs(target.env.iter().cloned())
            .current_dir(&target.cwd)
            .process_group(0)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let Ok(mut child) = spawn(&mut command) else {
            continue;
        };
        let stop = |child: &mut Child| {
            unsafe {
                libc::kill(-(child.id() as i32), libc::SIGKILL);
            }
            let _ = child.wait();
        };
        loop {
            if cancel.load(Ordering::Relaxed) {
                stop(&mut child);
                return Err("Cancelled".into());
            }
            if started.elapsed() >= LOGIN {
                stop(&mut child);
                return Err("Terminal sign-in did not finish before the timeout".into());
            }
            match child.try_wait() {
                Ok(Some(status)) if status.success() => return Ok(started.elapsed() < HANDOFF),
                Ok(Some(_)) => {
                    return Err(
                        "Terminal sign-in did not complete; use the copyable command".into(),
                    );
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(50)),
                Err(_) => return Err("Could not wait for the terminal".into()),
            }
        }
    }
    Err("No supported terminal was found. Set $TERMINAL, or copy the sign-in command and run it in your terminal, then Scan again.".into())
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
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    pub(super) fn scripted(provider: Provider, body: &str) -> (tempfile::TempDir, Target) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("agent");
        std::fs::write(&program, format!("#!/usr/bin/env python3\nimport sys,json\nif '--version' in sys.argv:\n print('test-agent 1.0');sys.exit(0)\n{body}")).unwrap();
        std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
        let target = Target {
            provider,
            program,
            args: vec![],
            env: vec![],
            cwd: dir.path().into(),
        };
        (dir, target)
    }
    #[test]
    fn claude_status_discards_email_and_secrets_and_handles_pretty_json() {
        let (_dir, target) = scripted(
            Provider::Claude,
            "print(json.dumps({'loggedIn':True,'authMethod':'claude.ai','subscriptionType':'pro','email':'private@example.com','accessToken':'SECRET','nested':{'ignored':True}},indent=2))",
        );
        let status = run(
            &target,
            &Operation::Check,
            &AtomicBool::new(false),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(status.state, SignIn::SignedIn);
        assert_eq!(status.method.as_deref(), Some("claude.ai"));
        assert_eq!(status.version.as_deref(), Some("test-agent 1.0"));
        assert!(!format!("{status:?}").contains("private@example.com"));
        assert!(!format!("{status:?}").contains("SECRET"));
    }
    #[test]
    fn codex_status_rejects_token_requests_and_paginates_models_without_starting_threads() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');i=m.get('id')
 if method=='initialize':
  send({'id':91,'method':'account/chatgptAuthTokens/refresh','params':{}})
  reply=json.loads(sys.stdin.readline());assert reply['error']['code']==-32601
  send({'id':i,'result':{}})
 elif method=='initialized': pass
 elif method=='account/read':
  assert m['params']=={'refreshToken':False}
  send({'id':i,'result':{'account':{'type':'chatgpt','planType':'pro','email':'private@example.com','token':'SECRET'}}})
 elif method=='model/list':
  cursor=m['params']['cursor'];model='first' if cursor is None else 'second'
  send({'id':i,'result':{'data':[{'id':model,'model':model,'displayName':model,'isDefault':model=='second','defaultReasoningEffort':'low','supportedReasoningEfforts':[{'reasoningEffort':'low','description':'Fast'},{'reasoningEffort':'high','description':'Deep'}]}], 'nextCursor':'next' if cursor is None else None}})
 else: raise Exception('unexpected method '+str(method))
"#,
        );
        let status = run(
            &target,
            &Operation::Check,
            &AtomicBool::new(false),
            |_| panic!(),
        )
        .unwrap();
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
        assert!(!format!("{status:?}").contains("SECRET"));
        assert!(!format!("{status:?}").contains("private@example.com"));
    }
    #[test]
    fn acp_probe_sends_no_database_or_tools_and_closes_the_empty_session() {
        let (_dir, target) = scripted(
            Provider::Antigravity,
            r#"
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 m=json.loads(line);method=m['method'];i=m['id']
 if method=='initialize':
  assert not m['params']['clientCapabilities']['terminal']
  send({'id':i,'result':{'agentCapabilities':{'sessionCapabilities':{'close':{}}},'authMethods':[{'id':'oauth-personal','name':'Google'},{'id':'gemini-api-key','name':'Key'}]}})
 elif method=='session/new':
  assert m['params']['mcpServers']==[]
  send({'id':i,'result':{'sessionId':'empty','configOptions':[{'category':'model','options':[{'value':'m','name':'Model'}]},{'category':'thought_level','options':[{'value':'deep','name':'Deep'}]}]}})
 elif method=='session/close':
  assert m['params']=={'sessionId':'empty'}
  send({'id':i,'result':{}})
 else: raise Exception('unexpected method '+method)
"#,
        );
        let status = run(
            &target,
            &Operation::Check,
            &AtomicBool::new(false),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(status.state, SignIn::SignedIn);
        assert_eq!(status.auth_methods.len(), 1);
        assert_eq!(status.models[0].id, "m");
        assert_eq!(
            status.efforts,
            [Effort {
                id: "deep".into(),
                name: "Deep".into()
            }]
        );
        assert_eq!(status.issue, None);
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
    }
    #[test]
    fn timeouts_and_cancellation_reap_the_process() {
        let (_dir, target) = scripted(Provider::Custom, "import time\ntime.sleep(30)");
        let stop = AtomicBool::new(false);
        let mut p = Process::spawn(&target, &[], Duration::from_millis(30)).unwrap();
        let pid = p.child.id();
        assert!(p.line(&stop).unwrap_err().contains("timeout"));
        drop(p);
        assert_eq!(unsafe { libc::kill(pid as i32, 0) }, -1);
        let mut p = Process::spawn(&target, &[], CHECK).unwrap();
        stop.store(true, Ordering::Relaxed);
        assert_eq!(p.line(&stop).unwrap_err(), "Cancelled");
    }
}

#[cfg(test)]
mod login_tests {
    use super::tests::scripted;
    use super::*;
    #[test]
    fn codex_browser_login_completes_then_rechecks_status() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
def send(v): print(json.dumps(v),flush=True)
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');i=m.get('id')
 if method=='initialize': send({'id':i,'result':{}})
 elif method=='initialized': pass
 elif method=='account/login/start':
  assert m['params']=={'type':'chatgpt'}
  send({'id':i,'result':{'loginId':'login-1','type':'chatgpt','authUrl':'https://auth.openai.com/test-login'}})
  send({'method':'account/login/completed','params':{'loginId':'unrelated','success':False}})
  send({'method':'account/login/completed','params':{'loginId':'login-1','success':True}})
 elif method=='account/read':
  assert m['params']['refreshToken']==False
  send({'id':i,'result':{'account':{'type':'chatgpt','planType':'plus'}}})
 elif method=='model/list': send({'id':i,'result':{'data':[],'nextCursor':None}})
 else: raise Exception('unexpected '+str(method))
"#,
        );
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
    }
    #[test]
    fn codex_login_can_be_cancelled_without_waiting_for_the_browser() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='account/login/start': print(json.dumps({'id':m['id'],'result':{'loginId':'l','authUrl':'https://auth.openai.com/test-login'}}),flush=True)
 elif m.get('method')=='account/login/cancel': sys.exit(0)
"#,
        );
        let cancel = AtomicBool::new(false);
        let result = run(&target, &Operation::Login { method: None }, &cancel, |_| {
            cancel.store(true, Ordering::Relaxed)
        });
        assert_eq!(result.unwrap_err(), "Cancelled");
    }
    #[test]
    fn acp_login_uses_only_the_selected_advertised_method() {
        let (_dir, target) = scripted(
            Provider::Antigravity,
            r#"
authed=False
for line in sys.stdin:
 m=json.loads(line);method=m['method'];i=m['id'];result={}
 if method=='initialize': result={'authMethods':[{'id':'oauth-personal','name':'Personal'},{'id':'oauth-business','name':'Enterprise'}]}
 elif method=='authenticate':
  assert m['params']=={'methodId':'oauth-business'};authed=True
 elif method=='session/new':
  assert authed;assert m['params']['mcpServers']==[];result={'sessionId':'new'}
 else: raise Exception('unexpected '+method)
 print(json.dumps({'id':i,'result':result}),flush=True)
"#,
        );
        let status = run(
            &target,
            &Operation::Login {
                method: Some("oauth-business".into()),
            },
            &AtomicBool::new(false),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(status.state, SignIn::SignedIn);
        assert!(
            run(
                &target,
                &Operation::Login {
                    method: Some("not-advertised".into())
                },
                &AtomicBool::new(false),
                |_| panic!()
            )
            .is_err()
        );
    }
    fn executable(path: &std::path::Path, body: &str) {
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(path, body).unwrap();
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).unwrap();
    }
    /// A Claude Code stand-in whose status follows a marker that login writes.
    fn terminal_claude() -> (tempfile::TempDir, Target) {
        let dir = tempfile::tempdir().unwrap();
        let program = dir.path().join("claude");
        executable(
            &program,
            "#!/usr/bin/env bash\nmarker=\"$(dirname \"$0\")/signed-in\"\ncase \"$1 $2\" in\n\
             'auth login') touch \"$marker\";;\n\
             'auth status') if [[ -e $marker ]]; then echo '{\"loggedIn\":true}'; else echo '{\"loggedIn\":false}'; fi;;\n\
             esac\n",
        );
        let target = Target {
            provider: Provider::Claude,
            program,
            args: vec![],
            env: vec![],
            cwd: dir.path().into(),
        };
        (dir, target)
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
    #[test]
    fn a_waiting_terminal_runs_the_sign_in_command() {
        let (dir, target) = terminal_claude();
        let terminal = dir.path().join("term");
        executable(&terminal, "#!/usr/bin/env bash\nshift\nexec \"$@\"\n");
        claude_login(
            &target,
            &[(terminal, vec!["-e".into()])],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(dir.path().join("signed-in").exists());
    }
    #[test]
    fn a_terminal_that_hands_off_is_polled_and_its_window_survives() {
        let (dir, target) = terminal_claude();
        let terminal = dir.path().join("term");
        // Exit at once and finish the sign-in later, as a single-instance terminal does.
        executable(
            &terminal,
            "#!/usr/bin/env bash\nshift\n( sleep 0.5; \"$@\" ) >/dev/null 2>&1 &\nexit 0\n",
        );
        let started = Instant::now();
        claude_login(
            &target,
            &[(terminal, vec!["-e".into()])],
            &AtomicBool::new(false),
        )
        .unwrap();
        assert!(dir.path().join("signed-in").exists());
        assert!(started.elapsed() >= Duration::from_millis(500));
    }
    #[test]
    fn a_failed_terminal_is_reported_and_none_found_explains_terminal() {
        let (dir, target) = terminal_claude();
        let terminal = dir.path().join("term");
        executable(&terminal, "#!/bin/sh\nexit 1\n");
        let cancel = AtomicBool::new(false);
        assert!(
            claude_login(&target, &[(terminal, vec![])], &cancel)
                .unwrap_err()
                .contains("did not complete")
        );
        assert!(
            claude_login(&target, &[], &cancel)
                .unwrap_err()
                .contains("$TERMINAL")
        );
    }
    #[test]
    fn claude_status_may_finish_writing_after_it_answers() {
        let (dir, target) = scripted(
            Provider::Claude,
            "import time,pathlib\nprint(json.dumps({'loggedIn':True}),flush=True)\n\
             time.sleep(0.3)\npathlib.Path('finished').write_text('1')",
        );
        let status = run(
            &target,
            &Operation::Check,
            &AtomicBool::new(false),
            |_| panic!(),
        )
        .unwrap();
        assert_eq!(status.state, SignIn::SignedIn);
        assert!(dir.path().join("finished").exists());
    }
    #[test]
    fn a_cancelled_job_reports_cancelled_not_a_failure() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
for line in sys.stdin:
 m=json.loads(line)
 if m.get('method')=='initialize': print(json.dumps({'id':m['id'],'result':{}}),flush=True)
 elif m.get('method')=='account/login/start': print(json.dumps({'id':m['id'],'result':{'loginId':'l','authUrl':'https://auth.openai.com/test-login'}}),flush=True)
"#,
        );
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
    fn codex_logout_rechecks_the_signed_out_state() {
        let (_dir, target) = scripted(
            Provider::Codex,
            r#"
logged_out=False
for line in sys.stdin:
 m=json.loads(line);method=m.get('method');i=m.get('id');result={}
 if method=='initialized': continue
 elif method=='account/logout': logged_out=True
 elif method=='account/read':
  assert logged_out;result={'account':None,'requiresOpenaiAuth':True}
 elif method=='model/list': result={'data':[],'nextCursor':None}
 elif method!='initialize': raise Exception('unexpected '+str(method))
 print(json.dumps({'id':i,'result':result}),flush=True)
"#,
        );
        assert_eq!(
            run(
                &target,
                &Operation::Logout,
                &AtomicBool::new(false),
                |_| panic!()
            )
            .unwrap()
            .state,
            SignIn::SignedOut
        );
    }
}
