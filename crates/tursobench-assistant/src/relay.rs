//! The agent starts `tursobench --mcp-relay <socket>` as its MCP server. That
//! relay only copies bytes to a private Unix socket owned by the running app,
//! so tools reach the app's own database session; no second process opens it.
use crate::mcp::{self, ToolHost};
use std::{
    io::{self, BufRead, BufReader, Read, Write},
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
        net::{UnixListener, UnixStream},
    },
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::Duration,
};

pub const TOKEN_ENV: &str = "TURSOBENCH_MCP_TOKEN";
pub const RELAY_FLAG: &str = "--mcp-relay";
pub const SERVER_NAME: &str = "tursobench";
static NEXT: AtomicU64 = AtomicU64::new(1);

/// How an agent should launch the relay for one endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub command: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

/// A listening socket serving one conversation's tools. Dropping it stops
/// accepting, removes its files and ends open connections.
pub struct Endpoint {
    socket: PathBuf,
    config: PathBuf,
    stop: Arc<AtomicBool>,
    open: Arc<std::sync::Mutex<std::collections::HashMap<u64, UnixStream>>>,
    launch: Launch,
}
impl Endpoint {
    pub fn start(host: Arc<dyn ToolHost>, relay: PathBuf) -> io::Result<Self> {
        let dir = private_dir()?;
        let name = format!(
            "assistant-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        );
        let socket = dir.join(format!("{name}.sock"));
        let config = dir.join(format!("{name}.json"));
        let _ = std::fs::remove_file(&socket);
        let listener = UnixListener::bind(&socket)?;
        std::fs::set_permissions(&socket, std::fs::Permissions::from_mode(0o600))?;
        let token = random_token()?;
        let stop = Arc::new(AtomicBool::new(false));
        let launch = Launch {
            command: relay,
            args: vec![RELAY_FLAG.into(), socket.to_string_lossy().into_owned()],
            env: vec![(TOKEN_ENV.into(), token.clone())],
        };
        let open = Arc::new(std::sync::Mutex::new(Default::default()));
        let endpoint = Self {
            socket,
            config,
            stop: stop.clone(),
            open: open.clone(),
            launch,
        };
        endpoint.write_config()?;
        std::thread::Builder::new()
            .name("assistant-tools".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    if stop.load(Ordering::Acquire) {
                        return;
                    }
                    let Ok(stream) = stream else { continue };
                    let id = NEXT.fetch_add(1, Ordering::Relaxed);
                    if let Ok(copy) = stream.try_clone() {
                        open.lock().unwrap().insert(id, copy);
                    }
                    let open = open.clone();
                    let host = host.clone();
                    let token = token.clone();
                    let stop = stop.clone();
                    let _ = std::thread::Builder::new()
                        .name("assistant-tool-session".into())
                        .spawn(move || {
                            session(stream, &token, &*host, &stop);
                            open.lock().unwrap().remove(&id);
                        });
                }
            })?;
        Ok(endpoint)
    }
    pub fn launch(&self) -> &Launch {
        &self.launch
    }
    /// A Claude Code `--mcp-config` file, readable only by this user.
    pub fn config_path(&self) -> &Path {
        &self.config
    }
    fn write_config(&self) -> io::Result<()> {
        let env: serde_json::Map<_, _> = self
            .launch
            .env
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
            .collect();
        let config = serde_json::json!({
            "mcpServers": {
                SERVER_NAME: {
                    "type": "stdio",
                    "command": self.launch.command,
                    "args": self.launch.args,
                    "env": env,
                }
            }
        });
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&self.config)?;
        file.write_all(config.to_string().as_bytes())
    }
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Wake the accept loop so it sees the stop flag.
        let _ = UnixStream::connect(&self.socket);
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.config);
        for (_, stream) in self.open.lock().unwrap().drain() {
            let _ = stream.shutdown(std::net::Shutdown::Both);
        }
    }
}

fn session(stream: UnixStream, token: &str, host: &dyn ToolHost, stop: &AtomicBool) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(10)));
    let Ok(writer) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(stream);
    let mut first = String::new();
    if Read::take(&mut reader, 256).read_line(&mut first).is_err()
        || !constant_time_eq(first.trim_end().as_bytes(), token.as_bytes())
        || stop.load(Ordering::Acquire)
    {
        return;
    }
    let _ = reader.get_ref().set_read_timeout(None);
    mcp::serve(StopReader { reader, stop }, writer, host);
}

/// Ends a tool session once its endpoint is dropped.
struct StopReader<'a, R> {
    reader: R,
    stop: &'a AtomicBool,
}
impl<R: Read> Read for StopReader<'_, R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(0);
        }
        self.reader.read(buf)
    }
}
impl<R: BufRead> BufRead for StopReader<'_, R> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        if self.stop.load(Ordering::Acquire) {
            return Ok(&[]);
        }
        self.reader.fill_buf()
    }
    fn consume(&mut self, amount: usize) {
        self.reader.consume(amount)
    }
}

fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 24];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

/// `$XDG_RUNTIME_DIR/tursobench`, or a per-user temporary directory, mode 0700.
pub fn private_dir() -> io::Result<PathBuf> {
    let uid = unsafe { libc::getuid() };
    let dir = match std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        Some(runtime) => PathBuf::from(runtime).join("tursobench"),
        None => std::env::temp_dir().join(format!("tursobench-{uid}")),
    };
    match std::fs::DirBuilder::new().mode(0o700).create(&dir) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {}
        Err(e) => return Err(e),
    }
    let meta = std::fs::symlink_metadata(&dir)?;
    if !meta.is_dir() || meta.uid() != uid || meta.mode() & 0o077 != 0 {
        return Err(io::Error::other(format!(
            "{} must be a private directory owned by this user",
            dir.display()
        )));
    }
    Ok(dir)
}

/// The `--mcp-relay` process: stdin to the socket, the socket to stdout.
pub fn run_relay(socket: &Path) -> io::Result<()> {
    let token = std::env::var(TOKEN_ENV)
        .map_err(|_| io::Error::other(format!("{TOKEN_ENV} is not set")))?;
    let mut stream = UnixStream::connect(socket)?;
    stream.write_all(format!("{token}\n").as_bytes())?;
    let mut upstream = stream.try_clone()?;
    std::thread::spawn(move || {
        let _ = io::copy(&mut io::stdin().lock(), &mut upstream);
        let _ = upstream.shutdown(std::net::Shutdown::Write);
    });
    io::copy(&mut stream, &mut io::stdout().lock())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mcp::tests::Echo;

    fn endpoint() -> (tempfile::TempDir, Endpoint) {
        let runtime = tempfile::tempdir().unwrap();
        // SAFETY: tests in this module set the variable before any thread reads it.
        unsafe { std::env::set_var("XDG_RUNTIME_DIR", runtime.path()) };
        let endpoint = Endpoint::start(Arc::new(Echo), "/usr/bin/tursobench".into()).unwrap();
        (runtime, endpoint)
    }

    fn exchange(endpoint: &Endpoint, token: &str, request: &str) -> String {
        let mut stream = UnixStream::connect(&endpoint.socket).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        stream
            .write_all(format!("{token}\n{request}\n").as_bytes())
            .unwrap();
        let mut line = String::new();
        let _ = BufReader::new(stream).read_line(&mut line);
        line
    }

    #[test]
    fn tools_answer_only_with_the_token_and_files_are_private_and_removed() {
        let (_runtime, endpoint) = endpoint();
        let call = r#"{"jsonrpc":"2.0","id":7,"method":"tools/call","params":{"name":"echo","arguments":{"text":"ok"}}}"#;
        let token = endpoint.launch().env[0].1.clone();
        let reply: serde_json::Value =
            serde_json::from_str(&exchange(&endpoint, &token, call)).unwrap();
        assert_eq!(reply["result"]["content"][0]["text"], "ok");
        assert_eq!(exchange(&endpoint, "wrong", call), "");

        let mode = |p: &Path| std::fs::metadata(p).unwrap().mode() & 0o777;
        assert_eq!(mode(endpoint.socket.parent().unwrap()), 0o700);
        assert_eq!(mode(&endpoint.config), 0o600);
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&endpoint.config).unwrap()).unwrap();
        let server = &config["mcpServers"][SERVER_NAME];
        assert_eq!(server["args"][0], RELAY_FLAG);
        assert_eq!(server["env"][TOKEN_ENV], token.as_str());
        assert_eq!(endpoint.launch().env[0].1, token);

        let (socket, config) = (endpoint.socket.clone(), endpoint.config.clone());
        drop(endpoint);
        assert!(!socket.exists() && !config.exists());
    }
}
