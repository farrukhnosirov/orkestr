//! Supervises one long-lived `claude` process per chat session.
//!
//! Each process runs in stream-json mode with `--permission-prompt-tool
//! stdio`, so every tool permission the CLI would prompt for in a terminal
//! arrives as a `can_use_tool` control request. The [`PermissionPolicy`]
//! (implemented by the core with the `DangerousActionGuard` + audit log)
//! classifies it; forbidden commands are denied at once, everything else
//! waits for the user's answer ([`ClaudeSessionManager::respond`]).
//!
//! Processes are spawned through `orkestr-platform` (no shell, no console
//! window, own process group) with an allow-listed environment — the Orkestr
//! core token and provider keys never reach the CLI. Stopping a session (or
//! an emergency stop through the session's cancellation token) kills the
//! whole process tree.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{now, ClaudeSessionId, ProjectId};
use orkestr_platform::ProcessControl;
use orkestr_protocol::methods::claude_code::{
    ClaudeCodeEvent, ClaudePermissionDecision, ClaudePermissionMode, ClaudeRisk, ClaudeRiskLevel, ClaudeSessionInfo,
    ClaudeSessionState,
};
use parking_lot::Mutex;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::control;
use super::parse::{capabilities, Inbound, StreamParser};

/// Live sessions allowed at once (all projects).
pub const MAX_SESSIONS: usize = 16;
/// Longest user message accepted (characters).
pub const MAX_MESSAGE_CHARS: usize = 200_000;
/// How long Orkestr waits for the CLI to answer a control request.
pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(20);
const STDERR_TAIL: usize = 4096;

/// Environment variables passed to the CLI (everything else is withheld).
const EXTRA_ENV: &[&str] = &["CLAUDE_CODE_GIT_BASH_PATH", "NO_COLOR", "FORCE_COLOR"];

/// Something for the core to publish.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// A chat item (`ClaudeCodeEvent`).
    Item {
        /// Session.
        session_id: ClaudeSessionId,
        /// Project.
        project_id: ProjectId,
        /// Working directory (for path confinement of `fileChanged`).
        cwd: PathBuf,
        /// Item.
        event: ClaudeCodeEvent,
    },
    /// The session's state changed (`ClaudeCodeStatus`).
    Status(ClaudeSessionInfo),
}

/// Context of a permission decision.
#[derive(Debug, Clone)]
pub struct PolicyContext {
    /// Session.
    pub session_id: ClaudeSessionId,
    /// Project.
    pub project_id: ProjectId,
    /// Working directory.
    pub cwd: PathBuf,
}

/// The policy's classification of a tool request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PolicyVerdict {
    /// Risk (shell commands flagged by the guard).
    pub risk: Option<ClaudeRisk>,
}

/// How a permission request ended (for the audit log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionOutcome {
    /// Denied by policy without asking.
    AutoDenied,
    /// Allowed by the user (`session` = for the rest of the session).
    Allowed {
        /// Remembered for the session.
        session: bool,
    },
    /// Denied by the user.
    Denied,
    /// Withdrawn by the CLI or cleared by an interrupt.
    Cancelled,
}

/// Classifies and records tool permissions (implemented by the core).
#[async_trait]
pub trait PermissionPolicy: Send + Sync + 'static {
    /// Risk of a tool call.
    fn assess(&self, ctx: &PolicyContext, tool_name: &str, input: &Value) -> PolicyVerdict;
    /// Records a decision (audit log).
    async fn record(
        &self,
        ctx: &PolicyContext,
        tool_name: &str,
        input: &Value,
        risk: Option<&ClaudeRisk>,
        outcome: PermissionOutcome,
    );
}

/// A policy that flags nothing and records nothing (tests).
#[derive(Debug, Default)]
pub struct NoPolicy;

#[async_trait]
impl PermissionPolicy for NoPolicy {
    fn assess(&self, _ctx: &PolicyContext, _tool: &str, _input: &Value) -> PolicyVerdict {
        PolicyVerdict::default()
    }
    async fn record(&self, _: &PolicyContext, _: &str, _: &Value, _: Option<&ClaudeRisk>, _: PermissionOutcome) {}
}

/// How to launch a session process.
#[derive(Debug, Clone)]
pub struct LaunchSpec {
    /// Project.
    pub project_id: ProjectId,
    /// Working directory.
    pub cwd: PathBuf,
    /// The `claude` binary.
    pub program: PathBuf,
    /// Model alias / id (`None` or `default` = the CLI's default).
    pub model: Option<String>,
    /// Initial permission mode.
    pub mode: ClaudePermissionMode,
    /// Launch with `--allow-dangerously-skip-permissions` (the user confirmed bypass).
    pub allow_bypass: bool,
    /// Conversation to resume.
    pub resume: Option<String>,
    /// Emergency-stop token of the project.
    pub stop: Option<CancellationToken>,
}

/// Whether a model name is a plain token (no spaces / shell or flag syntax).
#[must_use]
pub fn is_valid_model(model: &str) -> bool {
    !model.is_empty()
        && model.len() <= 100
        && !model.starts_with('-')
        && model
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | ':' | '[' | ']' | '/' | '@'))
}

impl LaunchSpec {
    /// Command-line arguments (pure; tested).
    #[must_use]
    pub fn args(&self) -> Vec<String> {
        let mut a: Vec<String> = [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
        ]
        .iter()
        .map(|s| (*s).to_owned())
        .collect();
        let mode = if self.mode == ClaudePermissionMode::BypassPermissions && !self.allow_bypass {
            ClaudePermissionMode::Default
        } else {
            self.mode
        };
        a.push(mode.as_cli().to_owned());
        if self.allow_bypass {
            a.push("--allow-dangerously-skip-permissions".into());
        }
        if let Some(m) = self.model.as_deref().filter(|m| *m != "default" && is_valid_model(m)) {
            a.push("--model".into());
            a.push(m.to_owned());
        }
        if let Some(r) = self.resume.as_deref().filter(|r| super::history::is_session_id(r)) {
            a.push("--resume".into());
            a.push(r.to_owned());
        }
        a
    }
}

/// Session errors.
#[derive(Debug, thiserror::Error)]
pub enum SessionError {
    /// Unknown session.
    #[error("Claude Code session {0} not found")]
    NotFound(ClaudeSessionId),
    /// Invalid input.
    #[error("{0}")]
    BadRequest(String),
    /// Not possible in the session's state.
    #[error("{0}")]
    Conflict(String),
    /// The process could not start.
    #[error("could not start Claude Code: {0}")]
    Spawn(String),
    /// The CLI rejected a control request.
    #[error("Claude Code rejected the request: {0}")]
    Protocol(String),
    /// The CLI did not answer.
    #[error("Claude Code did not answer: {0}")]
    Timeout(String),
}

type SessionResult<T> = Result<T, SessionError>;

struct PendingPermission {
    tool_name: String,
    input: Value,
    suggestions: Option<Value>,
    risk: Option<ClaudeRisk>,
    can_remember: bool,
}

struct Proc {
    stdin: mpsc::UnboundedSender<String>,
    cancel: CancellationToken,
}

struct Session {
    id: ClaudeSessionId,
    spec: Mutex<LaunchSpec>,
    info: Mutex<ClaudeSessionInfo>,
    proc: Mutex<Option<Proc>>,
    generation: AtomicU64,
    pending: Mutex<HashMap<String, PendingPermission>>,
    ctrl: Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>,
    next_req: AtomicU64,
    stop: CancellationToken,
}

impl Session {
    fn ctx(&self) -> PolicyContext {
        let info = self.info.lock();
        PolicyContext { session_id: self.id, project_id: info.project_id, cwd: PathBuf::from(&info.cwd) }
    }

    fn request_id(&self, kind: &str) -> String {
        format!("orkestr-{kind}-{}", self.next_req.fetch_add(1, Ordering::Relaxed) + 1)
    }

    fn write(&self, line: String) -> SessionResult<()> {
        let guard = self.proc.lock();
        let proc =
            guard.as_ref().ok_or_else(|| SessionError::Conflict("the Claude Code process is not running".into()))?;
        proc.stdin.send(line).map_err(|_| SessionError::Conflict("the Claude Code process is not running".into()))
    }
}

struct Inner {
    sessions: Mutex<HashMap<ClaudeSessionId, Arc<Session>>>,
    tx: mpsc::Sender<SessionEvent>,
    policy: Arc<dyn PermissionPolicy>,
    process: Arc<dyn ProcessControl>,
}

/// Supervisor of all chat sessions.
#[derive(Clone)]
pub struct ClaudeSessionManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ClaudeSessionManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeSessionManager").field("sessions", &self.inner.sessions.lock().len()).finish()
    }
}

/// A `tokio` command for the CLI: platform-supervised stdio child with the
/// allow-listed environment.
#[must_use]
pub fn sanitized_command(program: &std::path::Path, args: &[String]) -> tokio::process::Command {
    let mut cmd = orkestr_platform::process::stdio_command(program, args);
    cmd.env_clear();
    for (k, v) in std::env::vars_os() {
        if let Some(key) = k.to_str() {
            let allowed =
                crate::adapters::cli::ENV_ALLOWLIST.iter().chain(EXTRA_ENV).any(|a| a.eq_ignore_ascii_case(key));
            if allowed {
                cmd.env(&k, v);
            }
        }
    }
    cmd
}

fn shorten(text: &str, max: usize) -> String {
    let one_line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if one_line.chars().count() > max {
        format!("{}…", one_line.chars().take(max).collect::<String>())
    } else {
        one_line
    }
}

impl ClaudeSessionManager {
    /// Creates the manager; events go to `tx`.
    #[must_use]
    pub fn new(
        tx: mpsc::Sender<SessionEvent>,
        policy: Arc<dyn PermissionPolicy>,
        process: Arc<dyn ProcessControl>,
    ) -> Self {
        Self { inner: Arc::new(Inner { sessions: Mutex::default(), tx, policy, process }) }
    }

    fn session(&self, id: ClaudeSessionId) -> SessionResult<Arc<Session>> {
        self.inner.sessions.lock().get(&id).cloned().ok_or(SessionError::NotFound(id))
    }

    /// One session's info.
    pub fn get(&self, id: ClaudeSessionId) -> SessionResult<ClaudeSessionInfo> {
        Ok(self.session(id)?.info.lock().clone())
    }

    /// Sessions (of one project, or all), oldest first.
    #[must_use]
    pub fn list(&self, project: Option<ProjectId>) -> Vec<ClaudeSessionInfo> {
        let mut v: Vec<ClaudeSessionInfo> = self
            .inner
            .sessions
            .lock()
            .values()
            .map(|s| s.info.lock().clone())
            .filter(|i| project.is_none_or(|p| i.project_id == p))
            .collect();
        v.sort_by_key(|i| i.started_at);
        v
    }

    /// Starts a session.
    pub fn start(&self, spec: LaunchSpec) -> SessionResult<ClaudeSessionInfo> {
        if let Some(m) = spec.model.as_deref() {
            if !is_valid_model(m) {
                return Err(SessionError::BadRequest(format!("invalid model name `{m}`")));
            }
        }
        if let Some(r) = spec.resume.as_deref() {
            if !super::history::is_session_id(r) {
                return Err(SessionError::BadRequest("invalid conversation id".into()));
            }
        }
        if spec.mode == ClaudePermissionMode::BypassPermissions && !spec.allow_bypass {
            return Err(SessionError::BadRequest("bypassPermissions needs an explicit confirmation".into()));
        }
        {
            let mut map = self.inner.sessions.lock();
            map.retain(|_, s| !s.info.lock().state.is_final());
            if map.len() >= MAX_SESSIONS {
                return Err(SessionError::Conflict(format!(
                    "too many Claude Code sessions are open (limit {MAX_SESSIONS}); close one first"
                )));
            }
        }
        let id = ClaudeSessionId::new();
        let stop = spec.stop.as_ref().map_or_else(CancellationToken::new, CancellationToken::child_token);
        let info = ClaudeSessionInfo {
            session_id: id,
            project_id: spec.project_id,
            cwd: spec.cwd.display().to_string(),
            claude_session_id: spec.resume.clone(),
            model: spec.model.clone().filter(|m| m != "default"),
            permission_mode: spec.mode,
            bypass_allowed: spec.allow_bypass,
            state: ClaudeSessionState::Starting,
            title: None,
            pid: None,
            started_at: now(),
            error: None,
        };
        let session = Arc::new(Session {
            id,
            spec: Mutex::new(spec),
            info: Mutex::new(info),
            proc: Mutex::new(None),
            generation: AtomicU64::new(1),
            pending: Mutex::default(),
            ctrl: Mutex::default(),
            next_req: AtomicU64::new(0),
            stop,
        });
        spawn_proc(&self.inner, &session, 1)?;
        self.inner.sessions.lock().insert(id, Arc::clone(&session));
        let info = session.info.lock().clone();
        Ok(info)
    }

    /// Sends a user message.
    pub async fn send(&self, id: ClaudeSessionId, text: &str) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        if text.trim().is_empty() {
            return Err(SessionError::BadRequest("the message is empty".into()));
        }
        if text.chars().count() > MAX_MESSAGE_CHARS {
            return Err(SessionError::BadRequest(format!("the message is longer than {MAX_MESSAGE_CHARS} characters")));
        }
        if s.info.lock().state.is_final() {
            return Err(SessionError::Conflict("this Claude Code session has ended; start a new one".into()));
        }
        s.write(control::user_message(text))?;
        let info = {
            let mut info = s.info.lock();
            if info.title.is_none() {
                info.title = Some(shorten(text, 80));
            }
            if info.state != ClaudeSessionState::WaitingPermission {
                info.state = ClaudeSessionState::Running;
            }
            info.clone()
        };
        emit_item(&self.inner, &s, ClaudeCodeEvent::UserMessage { text: text.to_owned() }).await;
        emit_status(&self.inner, &s).await;
        Ok(info)
    }

    async fn control(&self, s: &Arc<Session>, kind: &str, request: Value) -> SessionResult<Value> {
        let rid = s.request_id(kind);
        let (tx, rx) = oneshot::channel();
        s.ctrl.lock().insert(rid.clone(), tx);
        if let Err(e) = s.write(control::control_request(&rid, request)) {
            s.ctrl.lock().remove(&rid);
            return Err(e);
        }
        match tokio::time::timeout(CONTROL_TIMEOUT, rx).await {
            Ok(Ok(Ok(v))) => Ok(v),
            Ok(Ok(Err(e))) => Err(SessionError::Protocol(e)),
            Ok(Err(_)) => Err(SessionError::Conflict("the Claude Code process ended".into())),
            Err(_) => {
                s.ctrl.lock().remove(&rid);
                Err(SessionError::Timeout(kind.to_owned()))
            }
        }
    }

    /// Interrupts the current turn (pending permission requests are denied first).
    pub async fn interrupt(&self, id: ClaudeSessionId) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        let state = s.info.lock().state;
        if !matches!(state, ClaudeSessionState::Running | ClaudeSessionState::WaitingPermission) {
            let info = s.info.lock().clone();
            return Ok(info);
        }
        let pending: Vec<(String, PendingPermission)> = s.pending.lock().drain().collect();
        for (rid, p) in pending {
            let _ = s.write(control::deny(&rid, "The user interrupted this turn."));
            emit_item(
                &self.inner,
                &s,
                ClaudeCodeEvent::PermissionResolved {
                    request_id: rid,
                    decision: ClaudePermissionDecision::Deny,
                    by_policy: false,
                    message: None,
                },
            )
            .await;
            self.inner
                .policy
                .record(&s.ctx(), &p.tool_name, &p.input, p.risk.as_ref(), PermissionOutcome::Cancelled)
                .await;
        }
        self.control(&s, "interrupt", control::interrupt()).await?;
        {
            let mut info = s.info.lock();
            if info.state == ClaudeSessionState::WaitingPermission {
                info.state = ClaudeSessionState::Running;
            }
        }
        emit_status(&self.inner, &s).await;
        let info = s.info.lock().clone();
        Ok(info)
    }

    /// Changes the permission mode. `bypassPermissions` needs `confirmed`; a
    /// process launched without the bypass flag is restarted (resuming the
    /// conversation) when idle.
    pub async fn set_mode(
        &self,
        id: ClaudeSessionId,
        mode: ClaudePermissionMode,
        confirmed: bool,
    ) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        if mode == ClaudePermissionMode::BypassPermissions {
            if !confirmed {
                return Err(SessionError::BadRequest("bypassPermissions needs an explicit confirmation".into()));
            }
            if !s.info.lock().bypass_allowed {
                return self.restart_with_bypass(&s).await;
            }
        }
        self.control(&s, "mode", control::set_permission_mode(mode.as_cli())).await?;
        s.spec.lock().mode = mode;
        s.info.lock().permission_mode = mode;
        emit_status(&self.inner, &s).await;
        let info = s.info.lock().clone();
        Ok(info)
    }

    async fn restart_with_bypass(&self, s: &Arc<Session>) -> SessionResult<ClaudeSessionInfo> {
        let (state, resume) = {
            let info = s.info.lock();
            (info.state, info.claude_session_id.clone())
        };
        if !matches!(state, ClaudeSessionState::Idle | ClaudeSessionState::Starting) {
            return Err(SessionError::Conflict(
                "wait until Claude finishes (or interrupt it) before switching to bypass mode".into(),
            ));
        }
        {
            let mut spec = s.spec.lock();
            spec.allow_bypass = true;
            spec.mode = ClaudePermissionMode::BypassPermissions;
            spec.resume = resume;
        }
        let generation = s.generation.fetch_add(1, Ordering::SeqCst) + 1;
        if let Some(old) = s.proc.lock().take() {
            old.cancel.cancel();
        }
        s.ctrl.lock().clear();
        spawn_proc(&self.inner, s, generation)?;
        {
            let mut info = s.info.lock();
            info.permission_mode = ClaudePermissionMode::BypassPermissions;
            info.bypass_allowed = true;
        }
        emit_item(&self.inner, s, ClaudeCodeEvent::ModeChanged { mode: "bypassPermissions".into() }).await;
        emit_status(&self.inner, s).await;
        let info = s.info.lock().clone();
        Ok(info)
    }

    /// Changes the model.
    pub async fn set_model(&self, id: ClaudeSessionId, model: &str) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        let model = model.trim();
        if !is_valid_model(model) {
            return Err(SessionError::BadRequest(format!("invalid model name `{model}`")));
        }
        self.control(&s, "model", control::set_model(Some(model))).await?;
        let stored = (model != "default").then(|| model.to_owned());
        s.spec.lock().model.clone_from(&stored);
        s.info.lock().model = stored;
        emit_item(&self.inner, &s, ClaudeCodeEvent::ModelChanged { model: model.to_owned() }).await;
        emit_status(&self.inner, &s).await;
        let info = s.info.lock().clone();
        Ok(info)
    }

    /// Answers a permission request.
    pub async fn respond(
        &self,
        id: ClaudeSessionId,
        request_id: &str,
        decision: ClaudePermissionDecision,
        message: Option<&str>,
    ) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        let p = s
            .pending
            .lock()
            .remove(request_id)
            .ok_or_else(|| SessionError::BadRequest("this permission request is no longer pending".into()))?;
        // Dangerous commands are never remembered: "allow for this session" counts once.
        let decision = if decision == ClaudePermissionDecision::AllowSession && !p.can_remember {
            ClaudePermissionDecision::AllowOnce
        } else {
            decision
        };
        let note = message.map(str::trim).filter(|m| !m.is_empty()).map(|m| m.chars().take(2000).collect::<String>());
        let line = match decision {
            ClaudePermissionDecision::AllowOnce => control::allow(request_id, &p.input, None),
            ClaudePermissionDecision::AllowSession => control::allow(
                request_id,
                &p.input,
                Some(control::session_permissions(&p.tool_name, p.suggestions.as_ref())),
            ),
            ClaudePermissionDecision::Deny => {
                let msg = note.as_deref().map_or_else(
                    || "The user denied this action.".to_owned(),
                    |m| format!("The user denied this action: {m}"),
                );
                control::deny(request_id, &msg)
            }
        };
        if let Err(e) = s.write(line) {
            s.pending.lock().insert(request_id.to_owned(), p);
            return Err(e);
        }
        let outcome = match decision {
            ClaudePermissionDecision::AllowOnce => PermissionOutcome::Allowed { session: false },
            ClaudePermissionDecision::AllowSession => PermissionOutcome::Allowed { session: true },
            ClaudePermissionDecision::Deny => PermissionOutcome::Denied,
        };
        self.inner.policy.record(&s.ctx(), &p.tool_name, &p.input, p.risk.as_ref(), outcome).await;
        emit_item(
            &self.inner,
            &s,
            ClaudeCodeEvent::PermissionResolved {
                request_id: request_id.to_owned(),
                decision,
                by_policy: false,
                message: note,
            },
        )
        .await;
        {
            let empty = s.pending.lock().is_empty();
            let mut info = s.info.lock();
            if empty && info.state == ClaudeSessionState::WaitingPermission {
                info.state = ClaudeSessionState::Running;
            }
        }
        emit_status(&self.inner, &s).await;
        let info = s.info.lock().clone();
        Ok(info)
    }

    /// Ends a session (kills its process tree).
    pub async fn stop(&self, id: ClaudeSessionId) -> SessionResult<ClaudeSessionInfo> {
        let s = self.session(id)?;
        s.stop.cancel();
        // Wait briefly for the process task to record the exit.
        for _ in 0..50 {
            if s.info.lock().state.is_final() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        let info = s.info.lock().clone();
        Ok(info)
    }

    /// Stops every session of a project; returns how many were running.
    pub fn stop_project(&self, project: ProjectId) -> usize {
        let sessions: Vec<Arc<Session>> = self.inner.sessions.lock().values().cloned().collect();
        let mut n = 0;
        for s in sessions {
            let info = s.info.lock().clone();
            if info.project_id == project && !info.state.is_final() {
                s.stop.cancel();
                n += 1;
            }
        }
        n
    }

    /// Stops every session; returns how many were running.
    pub fn stop_all(&self) -> usize {
        let sessions: Vec<Arc<Session>> = self.inner.sessions.lock().values().cloned().collect();
        let mut n = 0;
        for s in sessions {
            if !s.info.lock().state.is_final() {
                s.stop.cancel();
                n += 1;
            }
        }
        n
    }

    /// Stops everything and waits (bounded) for the processes to exit.
    pub async fn shutdown(&self, wait: Duration) {
        self.stop_all();
        let deadline = tokio::time::Instant::now() + wait;
        while tokio::time::Instant::now() < deadline {
            let live = self.inner.sessions.lock().values().any(|s| !s.info.lock().state.is_final());
            if !live {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }
}

async fn emit_item(inner: &Inner, s: &Session, event: ClaudeCodeEvent) {
    let (project_id, cwd) = {
        let info = s.info.lock();
        (info.project_id, PathBuf::from(&info.cwd))
    };
    let _ = inner.tx.send(SessionEvent::Item { session_id: s.id, project_id, cwd, event }).await;
}

async fn emit_status(inner: &Inner, s: &Session) {
    let info = s.info.lock().clone();
    let _ = inner.tx.send(SessionEvent::Status(info)).await;
}

fn spawn_proc(inner: &Arc<Inner>, s: &Arc<Session>, generation: u64) -> SessionResult<()> {
    let spec = s.spec.lock().clone();
    if !spec.cwd.is_dir() {
        return Err(SessionError::BadRequest(format!("the folder {} does not exist", spec.cwd.display())));
    }
    let mut cmd = sanitized_command(&spec.program, &spec.args());
    cmd.current_dir(&spec.cwd);
    let mut child = cmd.spawn().map_err(|e| SessionError::Spawn(format!("{}: {e}", spec.program.display())))?;
    let pid = child.id();
    let (Some(mut stdin), Some(stdout), Some(mut stderr)) =
        (child.stdin.take(), child.stdout.take(), child.stderr.take())
    else {
        return Err(SessionError::Spawn("the process has no stdio".into()));
    };
    let cancel = s.stop.child_token();
    let (tx, mut rx) = mpsc::unbounded_channel::<String>();
    *s.proc.lock() = Some(Proc { stdin: tx.clone(), cancel: cancel.clone() });
    {
        let mut info = s.info.lock();
        info.pid = pid;
        info.state = ClaudeSessionState::Starting;
        info.error = None;
        info.bypass_allowed = spec.allow_bypass;
    }
    tracing::info!(session = %s.id, pid, cwd = %spec.cwd.display(), "Claude Code session started");

    // stdin writer
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            if stdin.write_all(line.as_bytes()).await.is_err()
                || stdin.write_all(b"\n").await.is_err()
                || stdin.flush().await.is_err()
            {
                break;
            }
        }
    });
    // stderr tail
    let stderr_task = tokio::spawn(async move {
        let mut buf = Vec::new();
        let mut chunk = [0u8; 2048];
        while let Ok(n) = stderr.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.len() > STDERR_TAIL * 2 {
                buf.drain(..buf.len() - STDERR_TAIL);
            }
        }
        let start = buf.len().saturating_sub(STDERR_TAIL);
        String::from_utf8_lossy(&buf[start..]).trim().to_owned()
    });

    // initialize handshake → capabilities
    let init_id = s.request_id("init");
    let (itx, irx) = oneshot::channel();
    s.ctrl.lock().insert(init_id.clone(), itx);
    let _ = tx.send(control::control_request(&init_id, control::initialize()));
    {
        let (inner, s) = (Arc::clone(inner), Arc::clone(s));
        tokio::spawn(async move {
            if let Ok(Ok(Ok(resp))) = tokio::time::timeout(Duration::from_secs(60), irx).await {
                emit_item(&inner, &s, capabilities(&resp)).await;
                {
                    let mut info = s.info.lock();
                    if info.state == ClaudeSessionState::Starting {
                        info.state = ClaudeSessionState::Idle;
                    }
                }
                emit_status(&inner, &s).await;
            }
        });
    }

    let (inner, s) = (Arc::clone(inner), Arc::clone(s));
    tokio::spawn(async move {
        emit_status(&inner, &s).await;
        let mut parser = StreamParser::live();
        let mut lines = BufReader::new(stdout).lines();
        let mut cancelled = false;
        loop {
            tokio::select! {
                () = cancel.cancelled() => { cancelled = true; break; }
                next = lines.next_line() => match next {
                    Ok(Some(line)) => {
                        for item in parser.line(&line) {
                            handle(&inner, &s, item).await;
                        }
                    }
                    Ok(None) | Err(_) => break,
                },
            }
        }
        if cancelled {
            if let Some(pid) = pid {
                let process = Arc::clone(&inner.process);
                let _ = tokio::task::spawn_blocking(move || process.kill_tree(pid)).await;
            }
            let _ = child.kill().await;
        }
        let code = match tokio::time::timeout(Duration::from_secs(10), child.wait()).await {
            Ok(Ok(st)) => st.code(),
            _ => {
                let _ = child.kill().await;
                None
            }
        };
        let stderr_tail = tokio::time::timeout(Duration::from_secs(2), stderr_task)
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        if s.generation.load(Ordering::SeqCst) != generation {
            return; // replaced by a restart
        }
        *s.proc.lock() = None;
        s.ctrl.lock().clear();
        s.pending.lock().clear();
        {
            let mut info = s.info.lock();
            info.pid = None;
            let failed = !cancelled && code.is_some_and(|c| c != 0);
            info.state = if failed { ClaudeSessionState::Failed } else { ClaudeSessionState::Exited };
            if failed {
                let tail: String = stderr_tail.chars().rev().take(600).collect::<String>().chars().rev().collect();
                info.error = Some(if tail.is_empty() {
                    format!("Claude Code exited with code {}", code.unwrap_or(-1))
                } else {
                    tail
                });
            }
        }
        tracing::info!(session = %s.id, ?code, cancelled, "Claude Code session ended");
        emit_item(&inner, &s, ClaudeCodeEvent::Exited { code }).await;
        emit_status(&inner, &s).await;
    });
    Ok(())
}

async fn handle(inner: &Arc<Inner>, s: &Arc<Session>, item: Inbound) {
    match item {
        Inbound::Event(event) => {
            let mut status_changed = false;
            {
                let mut info = s.info.lock();
                match &event {
                    ClaudeCodeEvent::Init { claude_session_id, model, .. } => {
                        info.claude_session_id = Some(claude_session_id.clone());
                        if info.model.is_none() {
                            info.model.clone_from(model);
                        }
                        status_changed = true;
                    }
                    ClaudeCodeEvent::ModeChanged { mode } => {
                        if let Some(m) = ClaudePermissionMode::from_cli(mode) {
                            info.permission_mode = m;
                            s.spec.lock().mode = m;
                            status_changed = true;
                        }
                    }
                    ClaudeCodeEvent::Result { .. } => {
                        if s.pending.lock().is_empty() {
                            info.state = ClaudeSessionState::Idle;
                        }
                        status_changed = true;
                    }
                    ClaudeCodeEvent::Error { message, .. } => {
                        info.error = Some(message.clone());
                        status_changed = true;
                    }
                    _ => {}
                }
            }
            emit_item(inner, s, event).await;
            if status_changed {
                emit_status(inner, s).await;
            }
        }
        Inbound::ControlRequest { request_id, request } => {
            if request.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                permission_request(inner, s, request_id, &request).await;
            } else {
                let _ = s.write(control::error_response(&request_id, "not supported by Orkestr"));
            }
        }
        Inbound::ControlResponse { request_id, result } => {
            if let Some(tx) = s.ctrl.lock().remove(&request_id) {
                let _ = tx.send(result);
            }
        }
        Inbound::ControlCancel { request_id } => {
            let removed = s.pending.lock().remove(&request_id);
            if let Some(p) = removed {
                inner
                    .policy
                    .record(&s.ctx(), &p.tool_name, &p.input, p.risk.as_ref(), PermissionOutcome::Cancelled)
                    .await;
                emit_item(
                    inner,
                    s,
                    ClaudeCodeEvent::PermissionResolved {
                        request_id,
                        decision: ClaudePermissionDecision::Deny,
                        by_policy: true,
                        message: None,
                    },
                )
                .await;
            }
        }
    }
}

async fn permission_request(inner: &Arc<Inner>, s: &Arc<Session>, request_id: String, request: &Value) {
    let str_of = |k: &str| request.get(k).and_then(Value::as_str).map(str::to_owned);
    let tool_name = str_of("tool_name").unwrap_or_default();
    let input = request.get("input").cloned().unwrap_or(Value::Null);
    let ctx = s.ctx();
    let verdict = inner.policy.assess(&ctx, &tool_name, &input);
    let forbidden = verdict.risk.as_ref().is_some_and(|r| r.level == ClaudeRiskLevel::Forbidden);
    emit_item(
        inner,
        s,
        ClaudeCodeEvent::PermissionRequest {
            request_id: request_id.clone(),
            tool_name: tool_name.clone(),
            tool_use_id: str_of("tool_use_id"),
            input: input.clone(),
            description: str_of("description"),
            blocked_path: str_of("blocked_path"),
            risk: verdict.risk.clone(),
            can_remember: verdict.risk.is_none(),
        },
    )
    .await;
    if forbidden {
        let reason = verdict.risk.as_ref().map(|r| r.reason.clone()).unwrap_or_default();
        let msg = format!("Orkestr's security policy forbids this command ({reason}). Do not try to work around it.");
        let _ = s.write(control::deny(&request_id, &msg));
        inner.policy.record(&ctx, &tool_name, &input, verdict.risk.as_ref(), PermissionOutcome::AutoDenied).await;
        emit_item(
            inner,
            s,
            ClaudeCodeEvent::PermissionResolved {
                request_id,
                decision: ClaudePermissionDecision::Deny,
                by_policy: true,
                message: Some(reason),
            },
        )
        .await;
        return;
    }
    let can_remember = verdict.risk.is_none();
    s.pending.lock().insert(
        request_id,
        PendingPermission {
            tool_name,
            input,
            suggestions: request.get("permission_suggestions").cloned(),
            risk: verdict.risk,
            can_remember,
        },
    );
    s.info.lock().state = ClaudeSessionState::WaitingPermission;
    emit_status(inner, s).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> LaunchSpec {
        LaunchSpec {
            project_id: ProjectId::new(),
            cwd: PathBuf::from("."),
            program: PathBuf::from("claude"),
            model: Some("opus".into()),
            mode: ClaudePermissionMode::AcceptEdits,
            allow_bypass: false,
            resume: Some("06a417de-ea3e-4314-9648-2072200e551d".into()),
            stop: None,
        }
    }

    #[test]
    fn launch_args() {
        let a = spec().args().join(" ");
        assert!(a.starts_with(
            "--print --input-format stream-json --output-format stream-json --verbose --include-partial-messages --permission-prompt-tool stdio --permission-mode acceptEdits"
        ));
        assert!(a.ends_with("--model opus --resume 06a417de-ea3e-4314-9648-2072200e551d"));
        assert!(!a.contains("dangerously"));
        let mut s = spec();
        s.mode = ClaudePermissionMode::BypassPermissions;
        assert!(s.args().join(" ").contains("--permission-mode default"), "no bypass without the flag");
        s.allow_bypass = true;
        let a = s.args().join(" ");
        assert!(a.contains("--permission-mode bypassPermissions --allow-dangerously-skip-permissions"));
        s.model = Some("default".into());
        s.resume = Some("../evil".into());
        let a = s.args().join(" ");
        assert!(!a.contains("--model") && !a.contains("--resume"));
    }

    #[test]
    fn model_names() {
        assert!(is_valid_model("claude-sonnet-5-5"));
        assert!(is_valid_model("opus[1m]"));
        assert!(!is_valid_model("--dangerously-skip-permissions"));
        assert!(!is_valid_model("opus; rm -rf /"));
        assert!(!is_valid_model(""));
    }
}
