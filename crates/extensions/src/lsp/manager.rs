//! Language-server process supervision.
//!
//! One session = one server process speaking JSON-RPC over stdio. The
//! supervisor writes framed client messages to stdin, decodes stdout into
//! messages, keeps a short stderr tail for diagnostics, restarts a crashed
//! server with exponential backoff (the restart budget refills after the
//! server stayed up for a while) and kills the whole process tree on stop.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{LspSessionId, ProjectId};
use orkestr_platform::ProcessControl;
use orkestr_protocol::methods::lsp::{LspSessionInfo, LspState};
use parking_lot::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use super::framing::{self, Decoder};
use crate::error::{ExtError, ExtResult};

/// Max size of one client message.
pub const MAX_CLIENT_MESSAGE: usize = 16 << 20;

/// Something the core publishes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LspEvent {
    /// A server message.
    Message {
        /// Session.
        session_id: LspSessionId,
        /// Project.
        project_id: ProjectId,
        /// JSON text.
        message: String,
    },
    /// Session state changed.
    Status(LspSessionInfo),
}

/// What to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchSpec {
    /// Recipe id.
    pub server_id: String,
    /// Project.
    pub project_id: ProjectId,
    /// `file://` root URI.
    pub root_uri: String,
    /// Working directory (project root).
    pub cwd: PathBuf,
    /// Program.
    pub program: PathBuf,
    /// Arguments.
    pub args: Vec<String>,
    /// External stop signal (emergency stop); the session stops when it fires.
    pub stop: Option<CancellationToken>,
}

/// Supervision tunables.
#[derive(Debug, Clone, Copy)]
pub struct LspConfig {
    /// Restarts allowed within one unhealthy streak.
    pub max_restarts: u32,
    /// First backoff.
    pub base_backoff: Duration,
    /// Backoff cap.
    pub max_backoff: Duration,
    /// Uptime after which the restart budget refills.
    pub healthy_after: Duration,
    /// Sessions without client traffic for this long are stopped.
    pub idle_timeout: Duration,
    /// Max concurrent sessions.
    pub max_sessions: usize,
}

impl Default for LspConfig {
    fn default() -> Self {
        Self {
            max_restarts: 5,
            base_backoff: Duration::from_millis(500),
            max_backoff: Duration::from_secs(30),
            healthy_after: Duration::from_secs(120),
            idle_timeout: Duration::from_secs(30 * 60),
            max_sessions: 24,
        }
    }
}

/// Backoff before restart number `attempt` (1-based).
#[must_use]
pub fn backoff(cfg: &LspConfig, attempt: u32) -> Duration {
    let factor = 2u32.saturating_pow(attempt.saturating_sub(1).min(16));
    cfg.base_backoff.saturating_mul(factor).min(cfg.max_backoff)
}

struct Handle {
    info: Arc<Mutex<LspSessionInfo>>,
    tx: mpsc::Sender<String>,
    cancel: CancellationToken,
    last_activity: Arc<Mutex<Instant>>,
}

struct Inner {
    sessions: Mutex<HashMap<LspSessionId, Handle>>,
    events: mpsc::Sender<LspEvent>,
    config: LspConfig,
    process: Arc<dyn ProcessControl>,
}

/// Session registry + supervisors.
#[derive(Clone)]
pub struct LspManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for LspManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LspManager").field("sessions", &self.inner.sessions.lock().len()).finish()
    }
}

impl LspManager {
    /// New manager; events go to `events`.
    #[must_use]
    pub fn new(events: mpsc::Sender<LspEvent>, config: LspConfig, process: Arc<dyn ProcessControl>) -> Self {
        Self { inner: Arc::new(Inner { sessions: Mutex::new(HashMap::new()), events, config, process }) }
    }

    /// Live sessions.
    #[must_use]
    pub fn list(&self) -> Vec<LspSessionInfo> {
        let mut v: Vec<_> = self.inner.sessions.lock().values().map(|h| h.info.lock().clone()).collect();
        v.sort_by_key(|i| i.session_id);
        v
    }

    /// One session.
    #[must_use]
    pub fn get(&self, id: LspSessionId) -> Option<LspSessionInfo> {
        self.inner.sessions.lock().get(&id).map(|h| h.info.lock().clone())
    }

    /// Starts a session.
    pub fn start(&self, spec: LaunchSpec) -> ExtResult<LspSessionInfo> {
        let mut sessions = self.inner.sessions.lock();
        if sessions.len() >= self.inner.config.max_sessions {
            return Err(ExtError::Conflict(format!(
                "too many language servers running (limit {})",
                self.inner.config.max_sessions
            )));
        }
        let info = LspSessionInfo {
            session_id: LspSessionId::new(),
            server_id: spec.server_id.clone(),
            project_id: spec.project_id,
            root_uri: spec.root_uri.clone(),
            state: LspState::Starting,
            restarts: 0,
            pid: None,
            error: None,
        };
        let (tx, rx) = mpsc::channel(1024);
        let cancel = spec.stop.as_ref().map_or_else(CancellationToken::new, CancellationToken::child_token);
        let handle = Handle {
            info: Arc::new(Mutex::new(info.clone())),
            tx,
            cancel,
            last_activity: Arc::new(Mutex::new(Instant::now())),
        };
        let sup = Supervisor {
            spec,
            info: Arc::clone(&handle.info),
            cancel: handle.cancel.clone(),
            inner: Arc::clone(&self.inner),
        };
        sessions.insert(info.session_id, handle);
        drop(sessions);
        tokio::spawn(sup.run(rx));
        Ok(info)
    }

    /// Forwards one client message.
    pub fn send(&self, id: LspSessionId, message: String) -> ExtResult<()> {
        if message.len() > MAX_CLIENT_MESSAGE {
            return Err(ExtError::TooLarge(format!("LSP message of {} bytes", message.len())));
        }
        let v: serde_json::Value = serde_json::from_str(&message)?;
        if v.get("jsonrpc").and_then(serde_json::Value::as_str) != Some("2.0") {
            return Err(ExtError::Invalid("not a JSON-RPC 2.0 message".into()));
        }
        let sessions = self.inner.sessions.lock();
        let h = sessions.get(&id).ok_or_else(|| ExtError::NotFound(format!("language server session {id}")))?;
        let state = h.info.lock().state;
        if state != LspState::Running {
            return Err(ExtError::Conflict(format!("the language server is not running ({state:?})")));
        }
        *h.last_activity.lock() = Instant::now();
        h.tx.try_send(message).map_err(|_| ExtError::Conflict("the language server is not keeping up".into()))
    }

    /// Stops a session (no-op when unknown).
    pub fn stop(&self, id: LspSessionId) -> bool {
        self.inner.sessions.lock().get(&id).map(|h| h.cancel.cancel()).is_some()
    }

    /// Stops every session of a project.
    pub fn stop_project(&self, project: ProjectId) -> usize {
        let sessions = self.inner.sessions.lock();
        let mut n = 0;
        for h in sessions.values() {
            if h.info.lock().project_id == project {
                h.cancel.cancel();
                n += 1;
            }
        }
        n
    }

    /// Stops everything.
    pub fn stop_all(&self) -> usize {
        let sessions = self.inner.sessions.lock();
        for h in sessions.values() {
            h.cancel.cancel();
        }
        sessions.len()
    }

    /// Stops sessions without client traffic for longer than the idle timeout.
    pub fn reap_idle(&self) -> usize {
        let timeout = self.inner.config.idle_timeout;
        let sessions = self.inner.sessions.lock();
        let mut n = 0;
        for h in sessions.values() {
            if h.last_activity.lock().elapsed() > timeout {
                h.cancel.cancel();
                n += 1;
            }
        }
        n
    }

    /// Stops everything and waits (up to `timeout`) until the supervisors finished.
    pub async fn shutdown(&self, timeout: Duration) {
        self.stop_all();
        let deadline = Instant::now() + timeout;
        while !self.inner.sessions.lock().is_empty() && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }
}

struct Supervisor {
    spec: LaunchSpec,
    info: Arc<Mutex<LspSessionInfo>>,
    cancel: CancellationToken,
    inner: Arc<Inner>,
}

enum Outcome {
    Cancelled,
    Exited(String),
}

impl Supervisor {
    async fn emit(&self) {
        let info = self.info.lock().clone();
        let _ = self.inner.events.send(LspEvent::Status(info)).await;
    }

    async fn set(&self, f: impl FnOnce(&mut LspSessionInfo)) {
        f(&mut self.info.lock());
        self.emit().await;
    }

    async fn kill(&self, pid: Option<u32>, child: &mut tokio::process::Child) {
        if let Some(pid) = pid {
            let process = Arc::clone(&self.inner.process);
            let _ = tokio::task::spawn_blocking(move || process.kill_tree(pid)).await;
        }
        let _ = child.kill().await;
    }

    async fn run(self, mut rx: mpsc::Receiver<String>) {
        let id = self.info.lock().session_id;
        let mut streak = 0u32;
        loop {
            if self.cancel.is_cancelled() {
                self.set(|i| {
                    i.state = LspState::Stopped;
                    i.pid = None;
                })
                .await;
                break;
            }
            let mut cmd = orkestr_platform::process::stdio_command(&self.spec.program, &self.spec.args);
            cmd.current_dir(&self.spec.cwd);
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let msg = format!("could not start {}: {e}", self.spec.program.display());
                    tracing::warn!(session = %id, error = %msg, "language server failed to start");
                    self.set(|i| {
                        i.state = LspState::Failed;
                        i.error = Some(msg);
                    })
                    .await;
                    break;
                }
            };
            let pid = child.id();
            let (Some(mut stdin), Some(mut stdout), Some(mut stderr)) =
                (child.stdin.take(), child.stdout.take(), child.stderr.take())
            else {
                self.kill(pid, &mut child).await;
                self.set(|i| {
                    i.state = LspState::Failed;
                    i.error = Some("language server stdio is unavailable".into());
                })
                .await;
                break;
            };
            self.set(|i| {
                i.state = LspState::Running;
                i.pid = pid;
            })
            .await;
            let started = Instant::now();

            let events = self.inner.events.clone();
            let project_id = self.spec.project_id;
            let reader = tokio::spawn(async move {
                let mut dec = Decoder::new();
                let mut buf = vec![0u8; 64 * 1024];
                loop {
                    let n = match stdout.read(&mut buf).await {
                        Ok(0) | Err(_) => break,
                        Ok(n) => n,
                    };
                    dec.push(&buf[..n]);
                    loop {
                        match dec.next_message() {
                            Ok(Some(message)) => {
                                if events.send(LspEvent::Message { session_id: id, project_id, message }).await.is_err()
                                {
                                    return;
                                }
                            }
                            Ok(None) => break,
                            Err(e) => {
                                tracing::warn!(session = %id, error = %e, "language server output is malformed");
                                return;
                            }
                        }
                    }
                }
            });
            let tail = Arc::new(Mutex::new(String::new()));
            let tail_w = Arc::clone(&tail);
            let err_reader = tokio::spawn(async move {
                let mut buf = vec![0u8; 8 * 1024];
                while let Ok(n) = stderr.read(&mut buf).await {
                    if n == 0 {
                        break;
                    }
                    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
                    tracing::debug!(session = %id, stderr = %text.trim_end(), "language server stderr");
                    let mut t = tail_w.lock();
                    t.push_str(&text);
                    if t.len() > 2048 {
                        let cut = t.len() - 2048;
                        let cut = (cut..t.len()).find(|i| t.is_char_boundary(*i)).unwrap_or(t.len());
                        t.drain(..cut);
                    }
                }
            });

            let outcome = loop {
                tokio::select! {
                    () = self.cancel.cancelled() => break Outcome::Cancelled,
                    status = child.wait() => {
                        break Outcome::Exited(match status {
                            Ok(s) => s.to_string(),
                            Err(e) => e.to_string(),
                        });
                    }
                    msg = rx.recv() => match msg {
                        Some(m) => {
                            if let Err(e) = stdin.write_all(&framing::encode(&m)).await {
                                tracing::debug!(session = %id, error = %e, "write to language server failed");
                            }
                            let _ = stdin.flush().await;
                        }
                        None => break Outcome::Cancelled,
                    },
                }
            };
            drop(stdin);
            match outcome {
                Outcome::Cancelled => {
                    self.kill(pid, &mut child).await;
                    reader.abort();
                    err_reader.abort();
                    self.set(|i| {
                        i.state = LspState::Stopped;
                        i.pid = None;
                    })
                    .await;
                    break;
                }
                Outcome::Exited(status) => {
                    // Let the readers drain what the server printed before exiting.
                    let _ = tokio::time::timeout(Duration::from_millis(500), reader).await;
                    err_reader.abort();
                    if started.elapsed() >= self.inner.config.healthy_after {
                        streak = 0;
                    }
                    streak += 1;
                    let stderr_tail = tail.lock().trim().to_owned();
                    let error = if stderr_tail.is_empty() {
                        format!("exited with {status}")
                    } else {
                        format!("exited with {status}: {stderr_tail}")
                    };
                    if streak > self.inner.config.max_restarts {
                        tracing::warn!(session = %id, %error, "language server keeps crashing; giving up");
                        self.set(|i| {
                            i.state = LspState::Failed;
                            i.pid = None;
                            i.error = Some(error);
                        })
                        .await;
                        break;
                    }
                    self.set(|i| {
                        i.state = LspState::Restarting;
                        i.pid = None;
                        i.restarts += 1;
                        i.error = Some(error);
                    })
                    .await;
                    // Messages for the dead process are dropped; the client re-initializes.
                    while rx.try_recv().is_ok() {}
                    let wait = backoff(&self.inner.config, streak);
                    tokio::select! {
                        () = self.cancel.cancelled() => {}
                        () = tokio::time::sleep(wait) => {}
                    }
                }
            }
        }
        self.inner.sessions.lock().remove(&id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let cfg = LspConfig::default();
        assert_eq!(backoff(&cfg, 1), Duration::from_millis(500));
        assert_eq!(backoff(&cfg, 2), Duration::from_secs(1));
        assert_eq!(backoff(&cfg, 3), Duration::from_secs(2));
        assert_eq!(backoff(&cfg, 10), Duration::from_secs(30));
        assert_eq!(backoff(&cfg, 40), Duration::from_secs(30));
    }

    fn manager(cfg: LspConfig) -> (LspManager, mpsc::Receiver<LspEvent>) {
        let (tx, rx) = mpsc::channel(1024);
        (LspManager::new(tx, cfg, Arc::new(orkestr_platform::NativeProcessControl)), rx)
    }

    /// A shell command that echoes framed messages back: a tiny "server".
    fn echo_spec(dir: &std::path::Path) -> Option<LaunchSpec> {
        // Use the platform's interpreter: PowerShell on Windows, sh elsewhere.
        #[cfg(windows)]
        {
            let script = dir.join("echo.ps1");
            std::fs::write(
                &script,
                r#"$in = [Console]::OpenStandardInput(); $out = [Console]::OpenStandardOutput()
$buf = New-Object byte[] 65536
while (($n = $in.Read($buf, 0, $buf.Length)) -gt 0) { $out.Write($buf, 0, $n); $out.Flush() }"#,
            )
            .ok()?;
            let ps = orkestr_platform::process::which("powershell")?;
            Some(LaunchSpec {
                server_id: "echo".into(),
                project_id: ProjectId::new(),
                root_uri: "file:///tmp".into(),
                cwd: dir.to_path_buf(),
                program: ps,
                stop: None,
                args: vec![
                    "-NoProfile".into(),
                    "-NonInteractive".into(),
                    "-ExecutionPolicy".into(),
                    "Bypass".into(),
                    "-File".into(),
                    script.display().to_string(),
                ],
            })
        }
        #[cfg(not(windows))]
        {
            Some(LaunchSpec {
                server_id: "echo".into(),
                project_id: ProjectId::new(),
                root_uri: "file:///tmp".into(),
                cwd: dir.to_path_buf(),
                program: orkestr_platform::process::which("cat")?,
                args: vec![],
                stop: None,
            })
        }
    }

    async fn next_status(rx: &mut mpsc::Receiver<LspEvent>, want: LspState) -> LspSessionInfo {
        loop {
            let ev = tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.expect("timeout").expect("closed");
            if let LspEvent::Status(i) = ev {
                if i.state == want {
                    return i;
                }
            }
        }
    }

    #[tokio::test]
    async fn proxies_messages_and_stops() {
        let dir = tempfile::tempdir().unwrap();
        let Some(spec) = echo_spec(dir.path()) else { return };
        let (m, mut rx) = manager(LspConfig::default());
        let info = m.start(spec).unwrap();
        next_status(&mut rx, LspState::Running).await;
        assert!(m.send(info.session_id, "not json".into()).is_err());
        assert!(m.send(info.session_id, r#"{"id":1}"#.into()).is_err(), "jsonrpc 2.0 required");
        let msg = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"x":"ü"}}"#;
        m.send(info.session_id, msg.into()).unwrap();
        let echoed = loop {
            match tokio::time::timeout(Duration::from_secs(30), rx.recv()).await.unwrap().unwrap() {
                LspEvent::Message { message, session_id, .. } => {
                    assert_eq!(session_id, info.session_id);
                    break message;
                }
                LspEvent::Status(_) => {}
            }
        };
        assert_eq!(echoed, msg);
        assert!(m.stop(info.session_id));
        next_status(&mut rx, LspState::Stopped).await;
        tokio::time::timeout(Duration::from_secs(10), async {
            while m.get(info.session_id).is_some() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
        assert!(m.send(info.session_id, msg.into()).is_err());
    }

    #[tokio::test]
    async fn restarts_with_backoff_then_gives_up() {
        let dir = tempfile::tempdir().unwrap();
        // A "server" that exits immediately.
        #[cfg(windows)]
        let (program, args) =
            (orkestr_platform::process::which("cmd").unwrap(), vec!["/c".to_string(), "exit 3".to_string()]);
        #[cfg(not(windows))]
        let (program, args) = (orkestr_platform::process::which("false").unwrap(), Vec::<String>::new());
        let cfg = LspConfig { max_restarts: 2, base_backoff: Duration::from_millis(10), ..LspConfig::default() };
        let (m, mut rx) = manager(cfg);
        let spec = LaunchSpec {
            server_id: "crash".into(),
            project_id: ProjectId::new(),
            root_uri: "file:///tmp".into(),
            cwd: dir.path().to_path_buf(),
            program,
            args,
            stop: None,
        };
        let info = m.start(spec).unwrap();
        let r1 = next_status(&mut rx, LspState::Restarting).await;
        assert_eq!(r1.restarts, 1);
        assert!(r1.error.as_deref().unwrap_or("").contains("exit"));
        let r2 = next_status(&mut rx, LspState::Restarting).await;
        assert_eq!(r2.restarts, 2);
        let failed = next_status(&mut rx, LspState::Failed).await;
        assert_eq!(failed.session_id, info.session_id);
        assert_eq!(failed.restarts, 2);
    }

    #[tokio::test]
    async fn spawn_failure_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (m, mut rx) = manager(LspConfig::default());
        let spec = LaunchSpec {
            server_id: "missing".into(),
            project_id: ProjectId::new(),
            root_uri: "file:///tmp".into(),
            cwd: dir.path().to_path_buf(),
            program: dir.path().join("does-not-exist-server"),
            args: vec![],
            stop: None,
        };
        m.start(spec).unwrap();
        let f = next_status(&mut rx, LspState::Failed).await;
        assert!(f.error.unwrap().contains("could not start"));
    }

    #[tokio::test]
    async fn external_stop_token_stops_the_session() {
        let dir = tempfile::tempdir().unwrap();
        let Some(mut spec) = echo_spec(dir.path()) else { return };
        let emergency = CancellationToken::new();
        spec.stop = Some(emergency.clone());
        let (m, mut rx) = manager(LspConfig::default());
        m.start(spec).unwrap();
        next_status(&mut rx, LspState::Running).await;
        emergency.cancel();
        next_status(&mut rx, LspState::Stopped).await;
    }

    #[tokio::test]
    async fn session_limit() {
        let (m, _rx) = manager(LspConfig { max_sessions: 0, ..LspConfig::default() });
        let spec = LaunchSpec {
            server_id: "x".into(),
            project_id: ProjectId::new(),
            root_uri: String::new(),
            cwd: PathBuf::from("."),
            program: PathBuf::from("x"),
            args: vec![],
            stop: None,
        };
        assert!(matches!(m.start(spec), Err(ExtError::Conflict(_))));
    }
}
