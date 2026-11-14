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
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Model {
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
                let _ = tx.send(Event::Finished(result));
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
const LOGIN: Duration = Duration::from_secs(600);
const LINE_LIMIT: usize = 1024 * 1024;
const OUTPUT_LIMIT: usize = 4 * LINE_LIMIT;

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
        // A CLI update (or a concurrent fork inheriting a just-written script)
        // can briefly leave its executable busy on Linux.
        let mut attempts = 0;
        let mut child = loop {
            match command.spawn() {
                Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && attempts < 5 => {
                    attempts += 1;
                    std::thread::sleep(Duration::from_millis(20));
                }
                result => break result,
            }
        }
        .map_err(|e| -> String {
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
    if !target.program.is_file() {
        return Ok(Status {
            state: SignIn::Missing,
            ..Default::default()
        });
    }
    if target.provider == Provider::Claude {
        match operation {
            Operation::Login { .. } => terminal_login(target, cancel)?,
            Operation::Logout => {
                let mut p = Process::spawn(target, &["auth".into(), "logout".into()], CHECK)?;
                if !p.wait(cancel)? {
                    return Err("Claude Code could not sign out".into());
                }
            }
            Operation::Check => {}
        }
        let mut p = Process::spawn(target, &["auth".into(), "status".into()], CHECK)?;
        let mut output = String::new();
        let value = loop {
            output.push_str(&p.line(cancel)?);
            if let Ok(value) = serde_json::from_str::<Json>(&output) {
                break value;
            }
        };
        let mut status = claude_status(&value);
        status.models = ["sonnet", "opus", "haiku"]
            .map(|id| Model {
                id: id.into(),
                name: id.into(),
            })
            .to_vec();
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
                        if let Some(id) = field(&model["model"]).or_else(|| field(&model["id"])) {
                            if model["hidden"] != true && !status.models.iter().any(|m| m.id == id)
                            {
                                status.models.push(Model {
                                    name: field(&model["displayName"])
                                        .unwrap_or_else(|| id.clone()),
                                    id,
                                });
                            }
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
            let method = method
                .as_deref()
                .and_then(|id| methods.iter().find(|m| m.id == id))
                .or_else(|| methods.iter().find(|m| m.id == "oauth-personal"))
                .or_else(|| methods.first());
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
        if initialized["agentCapabilities"]["sessionCapabilities"]["close"].is_object() {
            if p.rpc(4, "session/close", json!({"sessionId":session}), cancel)
                .is_err()
            {
                status.issue = Some("The empty status session could not be closed".into());
            }
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
fn terminal_login(target: &Target, cancel: &AtomicBool) -> Result<(), String> {
    let dirs = crate::vendor::search_dirs();
    for (terminal, prefix) in [
        ("x-terminal-emulator", &["-e"][..]),
        ("gnome-terminal", &["--wait", "--"][..]),
        ("konsole", &["--nofork", "-e"][..]),
        ("kitty", &["--"][..]),
        ("alacritty", &["-e"][..]),
        ("foot", &["-e"][..]),
        ("xterm", &["-e"][..]),
    ] {
        if let Some(program) = crate::vendor::resolve(std::path::Path::new(terminal), &dirs) {
            let mut command = Command::new(program);
            command
                .args(prefix)
                .arg(&target.program)
                .args(target.provider.vendor().login)
                .current_dir(&target.cwd);
            let mut process = Process::command(command, LOGIN)?;
            if !process.wait(cancel)? {
                return Err("Terminal sign-in did not complete; use the copyable command".into());
            }
            return Ok(());
        }
    }
    Err("No supported terminal was found. Copy the sign-in command and run it in your terminal, then Scan again.".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn scripted(provider: Provider, body: &str) -> (tempfile::TempDir, Target) {
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
  send({'id':i,'result':{'data':[{'id':model,'model':model,'displayName':model}], 'nextCursor':'next' if cursor is None else None}})
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
  send({'id':i,'result':{'sessionId':'empty','configOptions':[{'category':'model','options':[{'value':'m','name':'Model'}]}]}})
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
