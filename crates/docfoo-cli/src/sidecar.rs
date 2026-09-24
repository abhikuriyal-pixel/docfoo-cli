//! Pi sidecar client.
//!
//! `docfoo-agent` is a Bun-compiled completion service: it embeds Pi's
//! `ModelRuntime` and answers one JSON object per line (PLAN.md §3.4). The CLI
//! spawns it lazily, routes tagged responses to the waiting request, and kills
//! it when the command exits.
//!
//! The client is thread-safe: KG indexing fans completions out across worker
//! threads, so `request`/`complete`/`models` take `&self` and share the child's
//! stdin behind a mutex. Deltas are routed to the request that owns them.
//!
//! Resolution order for the sidecar executable:
//!   1. `DOCFOO_SIDECAR_BIN`
//!   2. `docfoo-agent` next to the CLI binary
//!   3. `DOCFOO_SIDECAR_TS` run with `bun` (development fallback)

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use crate::error::{CliError, Result};
use crate::workspace::Workspace;

/// Outer ceiling for one completion (pi's own transport timeouts fire first).
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(300);
/// How often a waiter re-checks the cancel flag.
const CANCEL_POLL: Duration = Duration::from_millis(100);
/// Maximum process starts for one command before giving up.
const MAX_SPAWNS: u32 = 3;

#[derive(Debug)]
pub enum SidecarError {
    Spawn(String),
    Transport(String),
    Model(String),
    Cancelled,
}

impl std::fmt::Display for SidecarError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SidecarError::Spawn(message)
            | SidecarError::Transport(message)
            | SidecarError::Model(message) => write!(f, "{message}"),
            SidecarError::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::error::Error for SidecarError {}

impl From<SidecarError> for CliError {
    fn from(error: SidecarError) -> Self {
        CliError::Message(error.to_string())
    }
}

// ---------------------------------------------------------------------------
// Pending-request routing
// ---------------------------------------------------------------------------

struct PendingEntry {
    sender: Sender<Value>,
}

/// One entry per in-flight request. Deltas keep the entry; final frames remove
/// it. Kept separate from the process so it can be unit-tested.
#[derive(Default)]
struct PendingMap {
    entries: Mutex<HashMap<String, PendingEntry>>,
}

impl PendingMap {
    fn register(&self, request_id: &str, sender: Sender<Value>) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.insert(request_id.to_string(), PendingEntry { sender });
        }
    }

    fn remove(&self, request_id: &str) {
        if let Ok(mut entries) = self.entries.lock() {
            entries.remove(request_id);
        }
    }

    /// Route one sidecar frame. Returns true when the frame belonged to the
    /// protocol (so the reader does not treat it as a stray line).
    fn route(&self, value: &Value) -> bool {
        let frame_type = value
            .get("type")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if frame_type == "fatal" {
            let message = value
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("the sidecar failed");
            self.fail_all(message);
            return true;
        }
        let is_delta = frame_type == "stream_delta";
        let is_final = matches!(
            frame_type,
            "complete_response"
                | "models_response"
                | "auth_status_response"
                | "auth_set_response"
                | "auth_logout_response"
                | "pong"
                | "error_response"
        );
        if !is_delta && !is_final {
            return false;
        }
        let request_id = value
            .get("requestId")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if request_id.is_empty() {
            return false;
        }
        let sender = {
            let Ok(mut entries) = self.entries.lock() else {
                return false;
            };
            if is_delta {
                entries.get(request_id).map(|entry| entry.sender.clone())
            } else {
                entries.remove(request_id).map(|entry| entry.sender)
            }
        };
        if let Some(sender) = sender {
            let _ = sender.send(value.clone());
        }
        true
    }

    /// Fail every waiter (sidecar exit, fatal frame) so no request hangs.
    fn fail_all(&self, reason: &str) {
        let drained: Vec<PendingEntry> = match self.entries.lock() {
            Ok(mut entries) => entries.drain().map(|(_, entry)| entry).collect(),
            Err(_) => Vec::new(),
        };
        for entry in drained {
            let _ = entry.sender.send(json!({
                "type": "transport_error",
                "success": false,
                "error": reason,
            }));
        }
    }
}

// ---------------------------------------------------------------------------
// Requests and responses
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Value>,
    pub max_tokens: Option<u32>,
    pub reasoning: Option<String>,
    pub stream: bool,
    pub response_format: Option<Value>,
}

impl CompletionRequest {
    pub fn new(model: impl Into<String>, messages: Vec<Value>) -> Self {
        Self {
            model: model.into(),
            messages,
            max_tokens: None,
            reasoning: None,
            stream: false,
            response_format: None,
        }
    }

    fn to_payload(&self) -> Value {
        let mut payload = json!({
            "type": "complete",
            "model": self.model,
            "messages": self.messages,
            "stream": self.stream,
        });
        if let Some(max_tokens) = self.max_tokens {
            payload["maxTokens"] = json!(max_tokens);
        }
        if let Some(reasoning) = &self.reasoning {
            payload["reasoning"] = json!(reasoning);
        }
        if let Some(format) = &self.response_format {
            payload["responseFormat"] = format.clone();
        }
        payload
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(
        default,
        rename = "contextWindow",
        skip_serializing_if = "Option::is_none"
    )]
    pub context_window: Option<u64>,
    #[serde(default, rename = "maxTokens", skip_serializing_if = "Option::is_none")]
    pub max_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub api: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderInfo {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub configured: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, rename = "modelCount")]
    pub model_count: usize,
    #[serde(default)]
    pub models: Vec<ModelInfo>,
}

// ---------------------------------------------------------------------------
// Process management
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum SidecarLaunch {
    Binary(PathBuf),
    Ts { script: PathBuf, bun: PathBuf },
}

impl SidecarLaunch {
    pub fn discover() -> Result<Self> {
        if let Some(path) = std::env::var_os("DOCFOO_SIDECAR_BIN").filter(|value| !value.is_empty()) {
            let path = PathBuf::from(path);
            if path.is_file() {
                return Ok(SidecarLaunch::Binary(path));
            }
            return Err(CliError::Message(format!(
                "DOCFOO_SIDECAR_BIN points at a missing file: {}",
                path.display()
            )));
        }
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let name = if cfg!(windows) {
                    "docfoo-agent.exe"
                } else {
                    "docfoo-agent"
                };
                let candidate = dir.join(name);
                if candidate.is_file() {
                    return Ok(SidecarLaunch::Binary(candidate));
                }
            }
        }
        if let Some(script) =
            std::env::var_os("DOCFOO_SIDECAR_TS").filter(|value| !value.is_empty())
        {
            let script = PathBuf::from(script);
            let bun = find_on_path(if cfg!(windows) { "bun.exe" } else { "bun" }).ok_or_else(
                || {
                    CliError::Message(
                        "DOCFOO_SIDECAR_TS is set but bun was not found on PATH".to_string(),
                    )
                },
            )?;
            return Ok(SidecarLaunch::Ts { script, bun });
        }
        Err(CliError::Message(
            "the model sidecar is not built — run sidecar/build.sh (or build.ps1) or set DOCFOO_SIDECAR_TS"
                .to_string(),
        ))
    }

    fn command(&self) -> Command {
        match self {
            SidecarLaunch::Binary(path) => Command::new(path),
            SidecarLaunch::Ts { script, bun } => {
                let mut command = Command::new(bun);
                command.arg(script);
                command
            }
        }
    }
}

fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

pub struct SidecarClient {
    transport: Transport,
    pending: Arc<PendingMap>,
    alive: Arc<AtomicBool>,
}

enum Transport {
    Child {
        child: Mutex<Child>,
        stdin: Mutex<ChildStdin>,
    },
    /// Connection to a long-lived `docfoo-agent --socket` daemon.
    #[cfg(unix)]
    Socket(Mutex<std::os::unix::net::UnixStream>),
}

/// Read protocol frames until EOF, then fail every waiter.
fn start_reader<R: std::io::Read + Send + 'static>(
    reader: R,
    pending: Arc<PendingMap>,
    alive: Arc<AtomicBool>,
) {
    thread::spawn(move || {
        let reader = BufReader::new(reader);
        for line in reader.lines() {
            match line {
                Ok(line) if line.trim().is_empty() => continue,
                Ok(line) => match serde_json::from_str::<Value>(&line) {
                    Ok(value) => {
                        pending.route(&value);
                    }
                    Err(_) => { /* malformed frames are ignored */ }
                },
                Err(_) => break,
            }
        }
        alive.store(false, Ordering::SeqCst);
        pending.fail_all("the sidecar stopped before answering");
    });
}

/// Default socket for a per-workspace `docfoo-agent --socket` daemon.
#[cfg(unix)]
fn socket_path(agent_dir: &Path) -> Option<PathBuf> {
    if let Some(value) = std::env::var_os("DOCFOO_SIDECAR_SOCKET").filter(|value| !value.is_empty()) {
        return Some(PathBuf::from(value));
    }
    Some(agent_dir.join("sidecar.sock"))
}

#[cfg(not(unix))]
fn socket_path(_agent_dir: &Path) -> Option<PathBuf> {
    None
}

impl SidecarClient {
    pub fn spawn(launch: &SidecarLaunch, agent_dir: &Path) -> Result<Self> {
        Self::spawn_with_env(launch, agent_dir, &[])
    }

    /// Spawn with extra environment variables (tests use this for the fake
    /// sidecar's behavior switches).
    pub fn spawn_with_env(
        launch: &SidecarLaunch,
        agent_dir: &Path,
        extra_env: &[(&str, &str)],
    ) -> Result<Self> {
        let mut command = launch.command();
        command
            .env("DOCFOO_AGENT_DIR", agent_dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (key, value) in extra_env {
            command.env(key, value);
        }
        let mut child = command
            .spawn()
            .map_err(|error| SidecarError::Spawn(format!("could not start the model sidecar: {error}")))?;
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| SidecarError::Spawn("sidecar stdin unavailable".to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| SidecarError::Spawn("sidecar stdout unavailable".to_string()))?;

        let pending = Arc::new(PendingMap::default());
        let alive = Arc::new(AtomicBool::new(true));
        start_reader(stdout, Arc::clone(&pending), Arc::clone(&alive));

        Ok(Self {
            transport: Transport::Child {
                child: Mutex::new(child),
                stdin: Mutex::new(stdin),
            },
            pending,
            alive,
        })
    }

    /// Connect to a long-lived sidecar daemon over a Unix socket.
    #[cfg(unix)]
    pub fn connect_socket(path: &Path) -> Result<Self> {
        use std::os::unix::net::UnixStream;

        let stream = UnixStream::connect(path).map_err(|error| {
            SidecarError::Spawn(format!(
                "could not connect to the sidecar socket {}: {error}",
                path.display()
            ))
        })?;
        let reader = stream.try_clone().map_err(|error| {
            SidecarError::Spawn(format!("could not duplicate the sidecar socket: {error}"))
        })?;
        let pending = Arc::new(PendingMap::default());
        let alive = Arc::new(AtomicBool::new(true));
        start_reader(reader, Arc::clone(&pending), Arc::clone(&alive));
        Ok(Self {
            transport: Transport::Socket(Mutex::new(stream)),
            pending,
            alive,
        })
    }

    pub fn is_alive(&self) -> bool {
        if !self.alive.load(Ordering::SeqCst) {
            return false;
        }
        match &self.transport {
            Transport::Child { child, .. } => match child.lock() {
                Ok(mut child) => matches!(child.try_wait(), Ok(None)),
                Err(_) => false,
            },
            #[cfg(unix)]
            Transport::Socket(_) => true,
        }
    }

    fn send(&self, value: &Value) -> std::result::Result<(), SidecarError> {
        let mut line = serde_json::to_string(value)
            .map_err(|error| SidecarError::Transport(error.to_string()))?;
        line.push('\n');
        match &self.transport {
            Transport::Child { stdin, .. } => {
                let mut stdin = stdin.lock().map_err(|_| {
                    SidecarError::Transport("the sidecar pipe is poisoned".to_string())
                })?;
                stdin.write_all(line.as_bytes()).map_err(|error| {
                    SidecarError::Transport(format!("could not write to the sidecar: {error}"))
                })?;
                stdin.flush().map_err(|error| {
                    SidecarError::Transport(format!("could not flush the sidecar pipe: {error}"))
                })
            }
            #[cfg(unix)]
            Transport::Socket(stream) => {
                let mut stream = stream.lock().map_err(|_| {
                    SidecarError::Transport("the sidecar socket is poisoned".to_string())
                })?;
                stream.write_all(line.as_bytes()).map_err(|error| {
                    SidecarError::Transport(format!("could not write to the sidecar socket: {error}"))
                })?;
                stream.flush().map_err(|error| {
                    SidecarError::Transport(format!("could not flush the sidecar socket: {error}"))
                })
            }
        }
    }

    fn request(
        &self,
        mut payload: Value,
        cancel: Option<&AtomicBool>,
        mut on_delta: Option<&mut dyn FnMut(&str)>,
        timeout: Duration,
    ) -> std::result::Result<Value, SidecarError> {
        if !self.is_alive() {
            return Err(SidecarError::Transport("the sidecar is not running".to_string()));
        }
        let request_id = next_request_id();
        let (sender, receiver) = mpsc::channel();
        self.pending.register(&request_id, sender);
        payload["requestId"] = json!(request_id);
        if let Err(error) = self.send(&payload) {
            self.pending.remove(&request_id);
            return Err(error);
        }

        let deadline = Instant::now() + timeout;
        loop {
            if cancel.is_some_and(|flag| flag.load(Ordering::Relaxed)) {
                self.pending.remove(&request_id);
                let _ = self.send(&json!({ "type": "cancel", "requestId": request_id }));
                return Err(SidecarError::Cancelled);
            }
            match receiver.recv_timeout(CANCEL_POLL) {
                Ok(value) => {
                    let frame_type = value.get("type").and_then(Value::as_str).unwrap_or_default();
                    if frame_type == "stream_delta" {
                        if let Some(callback) = on_delta.as_mut() {
                            callback(value.get("text").and_then(Value::as_str).unwrap_or_default());
                        }
                        continue;
                    }
                    if frame_type == "transport_error" {
                        return Err(SidecarError::Transport(
                            value
                                .get("error")
                                .and_then(Value::as_str)
                                .unwrap_or("the sidecar stopped")
                                .to_string(),
                        ));
                    }
                    if frame_type == "pong" {
                        return Ok(value);
                    }
                    if value.get("success").and_then(Value::as_bool) == Some(true) {
                        return Ok(value);
                    }
                    return Err(SidecarError::Model(
                        value
                            .get("error")
                            .and_then(Value::as_str)
                            .unwrap_or("the sidecar request failed")
                            .to_string(),
                    ));
                }
                Err(RecvTimeoutError::Timeout) => {
                    if Instant::now() >= deadline {
                        self.pending.remove(&request_id);
                        let _ = self.send(&json!({ "type": "cancel", "requestId": request_id }));
                        return Err(SidecarError::Transport(
                            "the sidecar did not answer in time".to_string(),
                        ));
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.pending.remove(&request_id);
                    return Err(SidecarError::Transport(
                        "the sidecar stopped before answering".to_string(),
                    ));
                }
            }
        }
    }

    pub fn complete(
        &self,
        request: CompletionRequest,
        cancel: Option<&AtomicBool>,
        on_delta: Option<&mut dyn FnMut(&str)>,
        timeout: Duration,
    ) -> std::result::Result<String, SidecarError> {
        let value = self.request(request.to_payload(), cancel, on_delta, timeout)?;
        Ok(value
            .get("text")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string())
    }

    pub fn models(&self, refresh: bool) -> std::result::Result<Vec<ProviderInfo>, SidecarError> {
        let mut payload = json!({ "type": "models" });
        if refresh {
            payload["refresh"] = json!(true);
        }
        let value = self.request(payload, None, None, DEFAULT_TIMEOUT)?;
        providers_from(&value)
    }

    pub fn auth_status(
        &self,
        provider: Option<&str>,
    ) -> std::result::Result<Vec<ProviderInfo>, SidecarError> {
        let mut payload = json!({ "type": "auth_status" });
        if let Some(provider) = provider {
            payload["provider"] = json!(provider);
        }
        let value = self.request(payload, None, None, DEFAULT_TIMEOUT)?;
        providers_from(&value)
    }

    pub fn auth_set(&self, provider: &str, key: &str) -> std::result::Result<(), SidecarError> {
        self.request(
            json!({ "type": "auth_set", "provider": provider, "key": key }),
            None,
            None,
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    pub fn auth_logout(&self, provider: &str) -> std::result::Result<(), SidecarError> {
        self.request(
            json!({ "type": "auth_logout", "provider": provider }),
            None,
            None,
            DEFAULT_TIMEOUT,
        )?;
        Ok(())
    }

    pub fn ping(&self) -> std::result::Result<(), SidecarError> {
        self.request(
            json!({ "type": "ping" }),
            None,
            None,
            Duration::from_secs(10),
        )?;
        Ok(())
    }
}

impl Drop for SidecarClient {
    fn drop(&mut self) {
        // A socket connection belongs to a daemon: close it without killing.
        if let Transport::Child { child, .. } = &self.transport {
            if let Ok(mut child) = child.lock() {
                let _ = child.kill();
                let _ = child.wait();
            }
        }
    }
}

fn providers_from(value: &Value) -> std::result::Result<Vec<ProviderInfo>, SidecarError> {
    let providers = value
        .get("providers")
        .cloned()
        .unwrap_or_else(|| json!([]));
    serde_json::from_value(providers).map_err(|error| {
        SidecarError::Transport(format!(
            "could not parse the sidecar provider list: {error}"
        ))
    })
}

fn next_request_id() -> String {
    static SEQ: AtomicU64 = AtomicU64::new(0);
    format!("req-{}", SEQ.fetch_add(1, Ordering::Relaxed) + 1)
}

// ---------------------------------------------------------------------------
// Lazy, restartable handle
// ---------------------------------------------------------------------------

pub struct Sidecar {
    launch: SidecarLaunch,
    agent_dir: PathBuf,
    socket: Option<PathBuf>,
    client: Option<Arc<SidecarClient>>,
    spawns: u32,
}

impl Sidecar {
    pub fn locate(workspace: &Workspace) -> Result<Self> {
        Ok(Self {
            launch: SidecarLaunch::discover()?,
            agent_dir: workspace.agent_dir.clone(),
            socket: socket_path(&workspace.agent_dir),
            client: None,
            spawns: 0,
        })
    }

    /// The live client: a running daemon's socket when available, otherwise a
    /// freshly spawned process (respawning after a crash on demand). The
    /// returned `Arc` is safe to share with worker threads.
    pub fn client(&mut self) -> Result<Arc<SidecarClient>> {
        let needs_client = self
            .client
            .as_ref()
            .map_or(true, |client| !client.is_alive());
        if needs_client {
            #[cfg(unix)]
            if let Some(path) = self.socket.clone() {
                if let Ok(client) = SidecarClient::connect_socket(&path) {
                    self.client = Some(Arc::new(client));
                    return Ok(Arc::clone(
                        self.client.as_ref().expect("sidecar client was set"),
                    ));
                }
            }
            if self.spawns >= MAX_SPAWNS {
                return Err(CliError::Message(
                    "the model sidecar keeps stopping — check `docfoo version --verbose`"
                        .to_string(),
                ));
            }
            if self.spawns > 0 {
                thread::sleep(Duration::from_secs(1 << (self.spawns - 1)));
            }
            self.client = Some(Arc::new(SidecarClient::spawn(
                &self.launch,
                &self.agent_dir,
            )?));
            self.spawns += 1;
        }
        Ok(Arc::clone(self.client.as_ref().expect("sidecar was spawned")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn routes_delta_then_response_to_the_same_request() {
        let map = PendingMap::default();
        let (sender, receiver) = mpsc::channel();
        map.register("req-1", sender);

        assert!(map.route(&json!({"type":"stream_delta","requestId":"req-1","text":"a"})));
        assert!(map.route(
            &json!({"type":"complete_response","requestId":"req-1","success":true,"text":"ab"})
        ));

        assert_eq!(receiver.recv().unwrap()["text"], "a");
        assert_eq!(receiver.recv().unwrap()["text"], "ab");
        // The final frame removed the entry; a late delta is consumed but dropped.
        assert!(map.route(&json!({"type":"stream_delta","requestId":"req-1","text":"late"})));
    }

    #[test]
    fn unknown_frame_types_are_not_consumed() {
        let map = PendingMap::default();
        assert!(!map.route(&json!({"type":"ready"})));
        assert!(!map.route(&json!({"type":"something_else"})));
    }

    #[test]
    fn fatal_frame_fails_every_pending_request() {
        let map = PendingMap::default();
        let (sender, receiver) = mpsc::channel();
        map.register("req-1", sender);

        assert!(map.route(&json!({"type":"fatal","message":"boom"})));
        let frame = receiver.recv().unwrap();
        assert_eq!(frame["type"], "transport_error");
        assert_eq!(frame["error"], "boom");
    }

    #[test]
    fn completion_payload_carries_optional_fields() {
        let mut request =
            CompletionRequest::new("p/m", vec![json!({"role":"user","content":"hi"})]);
        request.max_tokens = Some(64);
        request.reasoning = Some("off".to_string());
        request.stream = true;
        request.response_format = Some(json!({"type":"json_object"}));
        let payload = request.to_payload();
        assert_eq!(payload["type"], "complete");
        assert_eq!(payload["maxTokens"], 64);
        assert_eq!(payload["reasoning"], "off");
        assert_eq!(payload["stream"], true);
        assert_eq!(payload["responseFormat"]["type"], "json_object");
    }
}
