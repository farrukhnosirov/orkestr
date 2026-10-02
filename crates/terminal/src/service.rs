//! The terminal service: session registry, PTY lifecycle, gated agent input
//! and non-PTY command execution.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{now, AgentId, ProjectId, TerminalId};
use orkestr_platform::{NativeProcessControl, NativeShellDetector, ProcessControl, ShellDetector};
use parking_lot::{Mutex, RwLock};
use portable_pty::{CommandBuilder, PtySize};
use tokio::io::AsyncReadExt;
use tokio::sync::{mpsc, watch};
use tokio_util::sync::CancellationToken;

use crate::confine::{resolve_cwd, resolve_free_cwd};
use crate::env::{build_env, is_valid_env_name, EnvVar};
use crate::error::{TerminalError, TerminalResult};
use crate::gate::{AgentInput, CommandGate, GateDecision, GateMode, GateRequest, LineAssembler, UserOnlyGate};
use crate::history::{CommandLog, CommandRecord, NoopCommandLog};
use crate::scrollback::{HistorySlice, Scrollback};
use crate::session::{
    pump, spawn_reader, spawn_waiter, PtyHandles, PumpConfig, PumpMsg, Session, SessionState, PUMP_CAPACITY,
};
use crate::shell::{command_line, ResolvedShell, ShellCatalog, ShellOption, ShellSpec};
use crate::sink::{NullSink, TerminalSink};
use crate::types::{
    OpenRequest, RunOutput, RunRequest, SessionKind, TerminalConfig, TerminalInfo, TerminalOwner, TerminalStatus,
};
use crate::utf8::Utf8Decoder;

/// Max columns / rows accepted.
pub const MAX_COLS: u16 = 1000;
/// Max rows accepted.
pub const MAX_ROWS: u16 = 500;
/// Max title length (chars).
pub const MAX_TITLE: usize = 80;
/// Max command length (bytes).
pub const MAX_COMMAND: usize = 32 * 1024;
/// Max injected env vars.
pub const MAX_ENV: usize = 64;

struct RunEntry {
    owner: TerminalOwner,
    project_id: Option<ProjectId>,
    token: CancellationToken,
}

struct Inner {
    config: TerminalConfig,
    detector: Arc<dyn ShellDetector>,
    catalog: RwLock<Option<ShellCatalog>>,
    process: Arc<dyn ProcessControl>,
    gate: Arc<dyn CommandGate>,
    sink: Arc<dyn TerminalSink>,
    log: Arc<dyn CommandLog>,
    sessions: RwLock<HashMap<TerminalId, Arc<Session>>>,
    runs: Mutex<HashMap<u64, RunEntry>>,
    next_run: AtomicU64,
    shutdown: CancellationToken,
}

/// Builder for [`TerminalService`].
pub struct TerminalServiceBuilder {
    config: TerminalConfig,
    detector: Arc<dyn ShellDetector>,
    process: Arc<dyn ProcessControl>,
    gate: Arc<dyn CommandGate>,
    sink: Arc<dyn TerminalSink>,
    log: Arc<dyn CommandLog>,
}

impl std::fmt::Debug for TerminalServiceBuilder {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalServiceBuilder").field("config", &self.config).finish_non_exhaustive()
    }
}

impl Default for TerminalServiceBuilder {
    fn default() -> Self {
        Self {
            config: TerminalConfig::default(),
            detector: Arc::new(NativeShellDetector::default()),
            process: Arc::new(NativeProcessControl),
            gate: Arc::new(UserOnlyGate),
            sink: Arc::new(NullSink),
            log: Arc::new(NoopCommandLog),
        }
    }
}

impl TerminalServiceBuilder {
    /// Limits and defaults.
    #[must_use]
    pub fn config(mut self, config: TerminalConfig) -> Self {
        self.config = config;
        self
    }
    /// Shell detector.
    #[must_use]
    pub fn detector(mut self, detector: Arc<dyn ShellDetector>) -> Self {
        self.detector = detector;
        self
    }
    /// Process control (tree kill).
    #[must_use]
    pub fn process(mut self, process: Arc<dyn ProcessControl>) -> Self {
        self.process = process;
        self
    }
    /// Command gate (security check before agent commands).
    #[must_use]
    pub fn gate(mut self, gate: Arc<dyn CommandGate>) -> Self {
        self.gate = gate;
        self
    }
    /// Output sink.
    #[must_use]
    pub fn sink(mut self, sink: Arc<dyn TerminalSink>) -> Self {
        self.sink = sink;
        self
    }
    /// Command history.
    #[must_use]
    pub fn log(mut self, log: Arc<dyn CommandLog>) -> Self {
        self.log = log;
        self
    }
    /// Builds the service.
    #[must_use]
    pub fn build(self) -> TerminalService {
        TerminalService {
            inner: Arc::new(Inner {
                config: self.config,
                detector: self.detector,
                catalog: RwLock::new(None),
                process: self.process,
                gate: self.gate,
                sink: self.sink,
                log: self.log,
                sessions: RwLock::new(HashMap::new()),
                runs: Mutex::new(HashMap::new()),
                next_run: AtomicU64::new(1),
                shutdown: CancellationToken::new(),
            }),
        }
    }
}

/// Terminal sessions and command execution (cheap to clone).
#[derive(Clone)]
pub struct TerminalService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for TerminalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TerminalService").field("sessions", &self.inner.sessions.read().len()).finish_non_exhaustive()
    }
}

fn validate_size(cols: u16, rows: u16) -> TerminalResult<()> {
    if cols == 0 || rows == 0 || cols > MAX_COLS || rows > MAX_ROWS {
        return Err(TerminalError::Invalid(format!("size must be 1..={MAX_COLS} cols and 1..={MAX_ROWS} rows")));
    }
    Ok(())
}

fn validate_env(env: &[EnvVar]) -> TerminalResult<()> {
    if env.len() > MAX_ENV {
        return Err(TerminalError::Invalid(format!("at most {MAX_ENV} environment variables")));
    }
    for v in env {
        if !is_valid_env_name(&v.name) {
            return Err(TerminalError::Invalid(format!("invalid environment variable name `{}`", v.name)));
        }
        if v.value.contains('\0') {
            return Err(TerminalError::Invalid(format!("environment variable `{}` contains NUL", v.name)));
        }
    }
    Ok(())
}

fn validate_title(title: &str) -> TerminalResult<String> {
    let t = title.trim();
    if t.is_empty() || t.chars().count() > MAX_TITLE || t.chars().any(char::is_control) {
        return Err(TerminalError::Invalid(format!("title must be 1..={MAX_TITLE} printable characters")));
    }
    Ok(t.to_owned())
}

/// Converts lone `\n` into `\r\n` for terminal display.
fn crlf(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 16);
    let mut prev = '\0';
    for c in text.chars() {
        if c == '\n' && prev != '\r' {
            out.push('\r');
        }
        out.push(c);
        prev = c;
    }
    out
}

impl TerminalService {
    /// Builder with native defaults (conservative [`UserOnlyGate`]).
    #[must_use]
    pub fn builder() -> TerminalServiceBuilder {
        TerminalServiceBuilder::default()
    }

    /// Configuration.
    #[must_use]
    pub fn config(&self) -> &TerminalConfig {
        &self.inner.config
    }

    // ---------------------------------------------------------------- shells

    fn catalog(&self) -> ShellCatalog {
        if let Some(c) = self.inner.catalog.read().as_ref() {
            return c.clone();
        }
        let c = ShellCatalog::detect(self.inner.detector.as_ref());
        *self.inner.catalog.write() = Some(c.clone());
        c
    }

    /// Detected shells (default first). `refresh` re-runs detection.
    #[must_use]
    pub fn shells(&self, refresh: bool) -> Vec<ShellOption> {
        if refresh {
            *self.inner.catalog.write() = None;
        }
        self.catalog().shells().to_vec()
    }

    fn resolve_shell(&self, spec: &ShellSpec) -> TerminalResult<ResolvedShell> {
        self.catalog().resolve(spec)
    }

    fn resolve_dir(owner: TerminalOwner, root: Option<&Path>, cwd: Option<&Path>) -> TerminalResult<PathBuf> {
        match root {
            Some(root) => resolve_cwd(root, cwd),
            None if owner.is_agent() => {
                Err(TerminalError::Forbidden("agent terminals and commands need a project or worktree root".into()))
            }
            None => match cwd {
                Some(c) => resolve_free_cwd(c),
                None => {
                    let home = std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                        .map(PathBuf::from)
                        .unwrap_or_else(std::env::temp_dir);
                    resolve_free_cwd(&home)
                }
            },
        }
    }

    fn child_env(
        id: Option<TerminalId>,
        project: Option<ProjectId>,
        owner: TerminalOwner,
        tty: bool,
        extra: &[EnvVar],
    ) -> Vec<(OsString, OsString)> {
        let mut defaults: Vec<(String, String)> = if tty {
            vec![
                ("TERM".into(), "xterm-256color".into()),
                ("COLORTERM".into(), "truecolor".into()),
                ("TERM_PROGRAM".into(), "orkestr".into()),
            ]
        } else {
            vec![("TERM".into(), "dumb".into()), ("NO_COLOR".into(), "1".into())]
        };
        if let Some(id) = id {
            defaults.push(("ORKESTR_TERMINAL_ID".into(), id.to_string()));
        }
        if let Some(p) = project {
            defaults.push(("ORKESTR_PROJECT_ID".into(), p.to_string()));
        }
        if let Some(a) = owner.agent_id() {
            defaults.push(("ORKESTR_AGENT_ID".into(), a.to_string()));
        }
        let refs: Vec<(&str, &str)> = defaults.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect();
        build_env(std::env::vars_os(), &refs, extra)
    }

    fn check_capacity(&self) -> TerminalResult<()> {
        if self.inner.shutdown.is_cancelled() {
            return Err(TerminalError::ShuttingDown);
        }
        let n = self.inner.sessions.read().len();
        if n >= self.inner.config.max_sessions {
            return Err(TerminalError::LimitReached(self.inner.config.max_sessions));
        }
        Ok(())
    }

    fn get(&self, id: TerminalId) -> TerminalResult<Arc<Session>> {
        self.inner.sessions.read().get(&id).cloned().ok_or(TerminalError::NotFound(id))
    }

    fn pump_config(&self) -> PumpConfig {
        PumpConfig {
            window: self.inner.config.coalesce_window,
            max_event_bytes: self.inner.config.max_event_bytes.max(1024),
        }
    }

    // ------------------------------------------------------------------ open

    /// Opens an interactive PTY session.
    pub async fn open(&self, req: OpenRequest) -> TerminalResult<TerminalInfo> {
        self.check_capacity()?;
        validate_size(req.cols, req.rows)?;
        validate_env(&req.env)?;
        let title = req.title.as_deref().map(validate_title).transpose()?;
        let shell = self.resolve_shell(&req.shell)?;
        let cwd = Self::resolve_dir(req.owner, req.root.as_deref(), req.cwd.as_deref())?;
        let id = TerminalId::new();
        let env = Self::child_env(Some(id), req.project_id, req.owner, true, &req.env);

        let (cols, rows) = (req.cols, req.rows);
        let spawn_shell = shell.clone();
        let spawn_cwd = cwd.clone();
        let spawned = tokio::task::spawn_blocking(move || -> TerminalResult<_> {
            let pty = portable_pty::native_pty_system();
            let pair = pty
                .openpty(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 })
                .map_err(|e| TerminalError::Pty(format!("openpty failed: {e}")))?;
            let mut cmd = CommandBuilder::new(&spawn_shell.program);
            cmd.args(&spawn_shell.args);
            cmd.cwd(&spawn_cwd);
            cmd.env_clear();
            for (k, v) in env {
                cmd.env(k, v);
            }
            let child = pair
                .slave
                .spawn_command(cmd)
                .map_err(|e| TerminalError::Pty(format!("failed to start {}: {e}", spawn_shell.program.display())))?;
            // The slave must be dropped so EOF/exit propagate.
            drop(pair.slave);
            let reader = pair.master.try_clone_reader().map_err(|e| TerminalError::Pty(e.to_string()))?;
            let writer = pair.master.take_writer().map_err(|e| TerminalError::Pty(e.to_string()))?;
            let killer = child.clone_killer();
            let pid = child.process_id();
            Ok((pair.master, child, reader, writer, killer, pid))
        })
        .await
        .map_err(|e| TerminalError::Pty(format!("spawn task failed: {e}")))??;
        let (master, child, reader, writer, killer, pid) = spawned;

        let (tx, rx) = mpsc::channel(PUMP_CAPACITY);
        let (done, _) = watch::channel(false);
        let session = Arc::new(Session {
            id,
            project_id: req.project_id,
            owner: req.owner,
            kind: SessionKind::Pty,
            shell_id: shell.id.clone(),
            shell_kind: shell.kind,
            cwd,
            created_at: now(),
            pid,
            state: Mutex::new(SessionState {
                title: title.unwrap_or_else(|| shell.name.clone()),
                cols,
                rows,
                status: TerminalStatus::Running,
            }),
            scrollback: Mutex::new(Scrollback::new(self.inner.config.scrollback_bytes)),
            pump_tx: tx.clone(),
            pty: Mutex::new(Some(PtyHandles { master, killer })),
            writer: Arc::new(Mutex::new(Some(writer))),
            input: tokio::sync::Mutex::new(LineAssembler::default()),
            done,
        });
        self.inner.sessions.write().insert(id, Arc::clone(&session));
        tokio::spawn(pump(Arc::clone(&session), rx, Arc::clone(&self.inner.sink), self.pump_config(), true));
        if let Err(e) = spawn_reader(id, reader, tx.clone()).and_then(|()| spawn_waiter(id, child, tx)) {
            tracing::error!(terminal_id = %id, error = %e, "failed to start pty threads");
            let _ = self.close(id, TerminalOwner::User).await;
            return Err(TerminalError::Io(e));
        }
        tracing::info!(terminal_id = %id, shell = %shell.id, pid = ?pid, agent = ?req.owner.agent_id(), "terminal opened");
        Ok(session.info())
    }

    // ----------------------------------------------------------------- input

    /// Writes input. User sessions take raw keystrokes from the user only.
    /// Agent sessions are read-only for users; their own agent's input is
    /// assembled into lines and **each line is gated** before it is written.
    pub async fn write(&self, id: TerminalId, by: TerminalOwner, data: &str) -> TerminalResult<()> {
        let session = self.get(id)?;
        if session.kind == SessionKind::CommandView {
            return Err(TerminalError::Forbidden("command views are read-only".into()));
        }
        if !session.is_running() {
            return Err(TerminalError::Exited(id));
        }
        match (session.owner, by) {
            (TerminalOwner::User, TerminalOwner::User) => session.write_bytes(data.as_bytes().to_vec()).await,
            (TerminalOwner::User, TerminalOwner::Agent { .. }) => {
                Err(TerminalError::Forbidden("agents cannot type into user terminals".into()))
            }
            (TerminalOwner::Agent { .. }, TerminalOwner::User) => {
                Err(TerminalError::Forbidden("agent-owned terminals are read-only for users".into()))
            }
            (TerminalOwner::Agent { agent_id: owner }, TerminalOwner::Agent { agent_id }) => {
                if owner != agent_id {
                    return Err(TerminalError::Forbidden("terminal belongs to another agent".into()));
                }
                self.agent_input(&session, data).await
            }
        }
    }

    /// Executes one line in an agent-owned PTY on behalf of its owner (gated).
    pub async fn exec_line(&self, id: TerminalId, line: &str) -> TerminalResult<()> {
        if line.contains(['\r', '\n']) {
            return Err(TerminalError::Invalid("line must not contain line breaks".into()));
        }
        if line.trim().is_empty() {
            return Err(TerminalError::Invalid("line must not be empty".into()));
        }
        let session = self.get(id)?;
        let TerminalOwner::Agent { .. } = session.owner else {
            return Err(TerminalError::Forbidden("execLine is only for agent-owned terminals".into()));
        };
        self.write(id, session.owner, &format!("{line}\r")).await
    }

    async fn agent_input(&self, session: &Arc<Session>, data: &str) -> TerminalResult<()> {
        let mut assembler = session.input.lock().await;
        let inputs = assembler.feed(data)?;
        for input in inputs {
            match input {
                AgentInput::Control(b) => session.write_bytes(vec![b]).await?,
                AgentInput::Line(line) => {
                    let request = GateRequest {
                        owner: session.owner,
                        project_id: session.project_id,
                        task_id: None,
                        shell: session.shell_kind,
                        command: line.clone(),
                        cwd: session.cwd.clone(),
                        mode: GateMode::InteractiveLine,
                        terminal_id: Some(session.id),
                    };
                    let decision = self.inner.gate.check(&request).await;
                    let started = now();
                    self.record(CommandRecord {
                        terminal_id: Some(session.id),
                        project_id: session.project_id,
                        task_id: None,
                        agent_id: session.owner.agent_id(),
                        mode: "line",
                        shell: session.shell_id.clone(),
                        command: line.clone(),
                        decision: decision.label(),
                        exit_code: None,
                        timed_out: false,
                        started_at: started,
                        finished_at: None,
                    })
                    .await;
                    match decision {
                        GateDecision::Allow => session.write_bytes(format!("{line}\r").into_bytes()).await?,
                        GateDecision::Deny { reason } => {
                            session.notice(&format!("command blocked: {reason}"));
                            return Err(TerminalError::Denied(reason));
                        }
                        GateDecision::Ask { reason, approval_id } => {
                            session.notice(&format!("command awaiting approval: {reason}"));
                            return Err(TerminalError::ApprovalRequired { reason, approval_id });
                        }
                    }
                }
            }
        }
        Ok(())
    }

    // ----------------------------------------------------------- management

    /// Resizes a session.
    pub fn resize(&self, id: TerminalId, cols: u16, rows: u16) -> TerminalResult<()> {
        validate_size(cols, rows)?;
        let session = self.get(id)?;
        if session.is_running() {
            session.resize_pty(cols, rows)?;
        }
        let mut st = session.state.lock();
        st.cols = cols;
        st.rows = rows;
        Ok(())
    }

    /// Renames a session.
    pub fn rename(&self, id: TerminalId, title: &str) -> TerminalResult<TerminalInfo> {
        let title = validate_title(title)?;
        let session = self.get(id)?;
        session.state.lock().title = title;
        Ok(session.info())
    }

    /// Session snapshot.
    pub fn info(&self, id: TerminalId) -> TerminalResult<TerminalInfo> {
        Ok(self.get(id)?.info())
    }

    /// Sessions, optionally filtered by project, oldest first.
    #[must_use]
    pub fn list(&self, project: Option<ProjectId>) -> Vec<TerminalInfo> {
        let mut out: Vec<TerminalInfo> = self
            .inner
            .sessions
            .read()
            .values()
            .filter(|s| project.is_none() || s.project_id == project)
            .map(|s| s.info())
            .collect();
        out.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
        out
    }

    /// Scrollback tail (for reconnects / late subscribers).
    pub fn history(&self, id: TerminalId, max_bytes: Option<usize>) -> TerminalResult<HistorySlice> {
        Ok(self.get(id)?.history(max_bytes))
    }

    /// Closes a session: kills the process tree, waits for `TerminalExited`
    /// and removes it from the registry. Agents may only close their own.
    pub async fn close(&self, id: TerminalId, by: TerminalOwner) -> TerminalResult<()> {
        let session = self.get(id)?;
        if by.is_agent() && by != session.owner {
            return Err(TerminalError::Forbidden("agents can only close their own terminals".into()));
        }
        self.terminate(&session).await;
        self.inner.sessions.write().remove(&id);
        tracing::info!(terminal_id = %id, "terminal closed");
        Ok(())
    }

    async fn terminate(&self, session: &Arc<Session>) {
        if !session.is_running() {
            return;
        }
        match session.kind {
            SessionKind::CommandView => {
                let _ = session.pump_tx.send(PumpMsg::Exited(None)).await;
            }
            SessionKind::Pty => {
                if let Some(pid) = session.pid {
                    self.kill_tree(pid).await;
                }
                session.kill_child();
            }
        }
        if !session.wait_done(Duration::from_secs(5)).await {
            tracing::warn!(terminal_id = %session.id, "session did not exit in time; releasing PTY");
            session.release_pty().await;
            let _ = session.wait_done(Duration::from_secs(3)).await;
        }
    }

    async fn kill_tree(&self, pid: u32) {
        let process = Arc::clone(&self.inner.process);
        let res = tokio::task::spawn_blocking(move || crate::kill::kill_process_tree(pid, process.as_ref())).await;
        match res {
            Ok(seen) => tracing::debug!(pid, seen, "process tree killed"),
            Err(e) => tracing::warn!(pid, error = %e, "kill_tree task failed"),
        }
    }

    /// Stops everything an agent runs: cancels its commands and closes its terminals.
    pub async fn stop_agent(&self, agent_id: AgentId) -> usize {
        let owner = TerminalOwner::Agent { agent_id };
        self.stop_where(|o, _| o == owner).await
    }

    /// Stops every agent command and agent terminal (emergency Stop All).
    pub async fn stop_all_agents(&self) -> usize {
        self.stop_where(|o, _| o.is_agent()).await
    }

    /// Stops all agent commands and agent terminals of a project (emergency stop).
    pub async fn stop_project_agents(&self, project_id: ProjectId) -> usize {
        self.stop_where(|o, p| o.is_agent() && p == Some(project_id)).await
    }

    async fn stop_where(&self, pred: impl Fn(TerminalOwner, Option<ProjectId>) -> bool) -> usize {
        let mut n = 0;
        for entry in self.inner.runs.lock().values() {
            if pred(entry.owner, entry.project_id) {
                entry.token.cancel();
                n += 1;
            }
        }
        let sessions: Vec<Arc<Session>> =
            self.inner.sessions.read().values().filter(|s| pred(s.owner, s.project_id)).cloned().collect();
        for s in sessions {
            if s.kind == SessionKind::Pty && s.is_running() {
                self.terminate(&s).await;
                n += 1;
            }
        }
        n
    }

    /// Kills every session and command (core shutdown).
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        for entry in self.inner.runs.lock().values() {
            entry.token.cancel();
        }
        let sessions: Vec<Arc<Session>> = self.inner.sessions.write().drain().map(|(_, s)| s).collect();
        for s in sessions {
            self.terminate(&s).await;
        }
    }

    async fn record(&self, record: CommandRecord) {
        if let Err(e) = self.inner.log.record(record).await {
            tracing::warn!(error = %e, "failed to record terminal command");
        }
    }

    // ------------------------------------------------------------------- run

    fn command_view(
        &self,
        owner: TerminalOwner,
        project_id: Option<ProjectId>,
        cwd: &Path,
        shell: &ResolvedShell,
    ) -> TerminalResult<Arc<Session>> {
        if let Some(existing) = self.inner.sessions.read().values().find(|s| {
            s.kind == SessionKind::CommandView && s.owner == owner && s.project_id == project_id && s.is_running()
        }) {
            return Ok(Arc::clone(existing));
        }
        self.check_capacity()?;
        let id = TerminalId::new();
        let (tx, rx) = mpsc::channel(PUMP_CAPACITY);
        let (done, _) = watch::channel(false);
        let title = if owner.is_agent() { "Agent commands" } else { "Commands" };
        let session = Arc::new(Session {
            id,
            project_id,
            owner,
            kind: SessionKind::CommandView,
            shell_id: shell.id.clone(),
            shell_kind: shell.kind,
            cwd: cwd.to_path_buf(),
            created_at: now(),
            pid: None,
            state: Mutex::new(SessionState {
                title: title.into(),
                cols: 120,
                rows: 30,
                status: TerminalStatus::Running,
            }),
            scrollback: Mutex::new(Scrollback::new(self.inner.config.scrollback_bytes)),
            pump_tx: tx,
            pty: Mutex::new(None),
            writer: Arc::new(Mutex::new(None)),
            input: tokio::sync::Mutex::new(LineAssembler::default()),
            done,
        });
        self.inner.sessions.write().insert(id, Arc::clone(&session));
        tokio::spawn(pump(Arc::clone(&session), rx, Arc::clone(&self.inner.sink), self.pump_config(), false));
        Ok(session)
    }

    /// Runs one command without a PTY: gate check first, then spawn with a
    /// sanitized env inside the confinement root, capture (capped) output,
    /// stream it to a visible command view, and kill the whole process tree on
    /// timeout or cancellation.
    pub async fn run(&self, req: RunRequest, cancel: CancellationToken) -> TerminalResult<RunOutput> {
        if self.inner.shutdown.is_cancelled() {
            return Err(TerminalError::ShuttingDown);
        }
        if req.command.len() > MAX_COMMAND {
            return Err(TerminalError::Invalid(format!("command longer than {MAX_COMMAND} bytes")));
        }
        validate_env(&req.env)?;
        let shell = self.resolve_shell(&req.shell)?;
        let cmdline = command_line(&shell, &req.command)?;
        let cwd = Self::resolve_dir(req.owner, req.root.as_deref(), req.cwd.as_deref())?;
        let cfg = &self.inner.config;
        let timeout =
            req.timeout.unwrap_or(cfg.default_run_timeout).min(cfg.max_run_timeout).max(Duration::from_millis(1));
        let max_output = req.max_output.unwrap_or(cfg.default_max_output).clamp(1, cfg.max_output_cap);

        let view = if req.stream_to_view.unwrap_or(req.owner.is_agent()) {
            Some(self.command_view(req.owner, req.project_id, &cwd, &shell)?)
        } else {
            None
        };
        let view_id = view.as_ref().map(|v| v.id);
        if let Some(v) = &view {
            let header = format!("\r\n\x1b[36m$ {}\x1b[0m\r\n", crlf(&req.command));
            let _ = v.pump_tx.send(PumpMsg::Text(header)).await;
        }

        // Gate BEFORE anything is spawned.
        let gate_req = GateRequest {
            owner: req.owner,
            project_id: req.project_id,
            task_id: req.task_id,
            shell: shell.kind,
            command: req.command.clone(),
            cwd: cwd.clone(),
            mode: GateMode::Run,
            terminal_id: view_id,
        };
        let decision = self.inner.gate.check(&gate_req).await;
        let started_at = now();
        let base_record = CommandRecord {
            terminal_id: view_id,
            project_id: req.project_id,
            task_id: req.task_id,
            agent_id: req.owner.agent_id(),
            mode: "run",
            shell: shell.id.clone(),
            command: req.command.clone(),
            decision: decision.label(),
            exit_code: None,
            timed_out: false,
            started_at,
            finished_at: None,
        };
        match decision {
            GateDecision::Allow => {}
            GateDecision::Deny { reason } => {
                if let Some(v) = &view {
                    v.notice(&format!("command blocked: {reason}"));
                }
                self.record(base_record).await;
                return Err(TerminalError::Denied(reason));
            }
            GateDecision::Ask { reason, approval_id } => {
                if let Some(v) = &view {
                    v.notice(&format!("command awaiting approval: {reason}"));
                }
                self.record(base_record).await;
                return Err(TerminalError::ApprovalRequired { reason, approval_id });
            }
        }

        let env = Self::child_env(view_id, req.project_id, req.owner, false, &req.env);
        let mut cmd = tokio::process::Command::new(&cmdline.program);
        cmd.args(&cmdline.args);
        if let Some(tail) = &cmdline.raw_tail {
            push_raw_arg(&mut cmd, tail);
        }
        cmd.current_dir(&cwd)
            .env_clear()
            .envs(env)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .kill_on_drop(true);
        isolate_process(&mut cmd);

        let start = Instant::now();
        let mut child = cmd.spawn().map_err(|e| {
            TerminalError::Io(std::io::Error::new(
                e.kind(),
                format!("failed to start {}: {e}", cmdline.program.display()),
            ))
        })?;
        let pid = child.id();
        let run_id = self.inner.next_run.fetch_add(1, Ordering::Relaxed);
        let token = cancel.child_token();
        self.inner
            .runs
            .lock()
            .insert(run_id, RunEntry { owner: req.owner, project_id: req.project_id, token: token.clone() });
        let _guard = RunGuard { inner: Arc::clone(&self.inner), run_id };
        let shutdown = self.inner.shutdown.clone();

        let out_task = child.stdout.take().map(|s| {
            let cap = Arc::new(Mutex::new(Capture::default()));
            (tokio::spawn(read_stream(s, max_output, view.clone(), Arc::clone(&cap))), cap)
        });
        let err_task = child.stderr.take().map(|s| {
            let cap = Arc::new(Mutex::new(Capture::default()));
            (tokio::spawn(read_stream(s, max_output, view.clone(), Arc::clone(&cap))), cap)
        });

        enum End {
            Exited(std::io::Result<std::process::ExitStatus>),
            TimedOut,
            Cancelled,
        }
        let end = tokio::select! {
            status = child.wait() => End::Exited(status),
            () = tokio::time::sleep(timeout) => End::TimedOut,
            () = token.cancelled() => End::Cancelled,
            () = shutdown.cancelled() => End::Cancelled,
        };
        let exit_code = match &end {
            End::Exited(Ok(status)) => status.code(),
            End::Exited(Err(e)) => {
                tracing::warn!(error = %e, "waiting for command failed");
                None
            }
            End::TimedOut | End::Cancelled => {
                if let Some(pid) = pid {
                    self.kill_tree(pid).await;
                }
                let _ = child.start_kill();
                if tokio::time::timeout(Duration::from_secs(5), child.wait()).await.is_err() {
                    tracing::warn!(pid = ?pid, "command did not die within 5s after kill");
                }
                None
            }
        };
        let (stdout, out_trunc) = join_reader(out_task).await;
        let (stderr, err_trunc) = join_reader(err_task).await;
        let duration = start.elapsed();
        let timed_out = matches!(end, End::TimedOut);
        let cancelled = matches!(end, End::Cancelled);

        if let Some(v) = &view {
            let ms = duration.as_millis();
            let footer = if timed_out {
                format!("\x1b[31m[timed out after {ms} ms — process tree killed]\x1b[0m\r\n")
            } else if cancelled {
                format!("\x1b[31m[cancelled after {ms} ms — process tree killed]\x1b[0m\r\n")
            } else {
                let code = exit_code.map_or_else(|| "?".to_owned(), |c| c.to_string());
                let color = if exit_code == Some(0) { "32" } else { "31" };
                format!("\x1b[{color}m[exit {code} · {ms} ms]\x1b[0m\r\n")
            };
            let _ = v.pump_tx.send(PumpMsg::Text(footer)).await;
        }
        let mut record = base_record;
        record.exit_code = exit_code;
        record.timed_out = timed_out;
        record.finished_at = Some(now());
        self.record(record).await;
        tracing::info!(shell = %shell.id, exit_code = ?exit_code, timed_out, cancelled, ms = duration.as_millis() as u64, agent = ?req.owner.agent_id(), "command finished");

        if cancelled {
            return Err(TerminalError::Cancelled);
        }
        Ok(RunOutput {
            exit_code,
            stdout,
            stderr,
            truncated: out_trunc || err_trunc,
            timed_out,
            duration,
            view_terminal_id: view_id,
        })
    }
}

/// Removes the run from the registry when the run future ends (or is dropped).
struct RunGuard {
    inner: Arc<Inner>,
    run_id: u64,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.inner.runs.lock().remove(&self.run_id);
    }
}

#[cfg(windows)]
fn push_raw_arg(cmd: &mut tokio::process::Command, tail: &str) {
    cmd.raw_arg(tail);
}

#[cfg(not(windows))]
fn push_raw_arg(cmd: &mut tokio::process::Command, tail: &str) {
    cmd.arg(tail);
}

/// No console window on Windows; own process group on Unix (tree kill).
#[cfg(windows)]
fn isolate_process(cmd: &mut tokio::process::Command) {
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    cmd.creation_flags(CREATE_NO_WINDOW);
}

#[cfg(unix)]
fn isolate_process(cmd: &mut tokio::process::Command) {
    cmd.process_group(0);
}

#[cfg(not(any(windows, unix)))]
fn isolate_process(_cmd: &mut tokio::process::Command) {}

/// Captured output of one stream (shared so a stuck reader can be abandoned
/// without losing what it already read).
#[derive(Debug, Default)]
struct Capture {
    bytes: Vec<u8>,
    truncated: bool,
}

async fn read_stream<R>(mut reader: R, cap: usize, view: Option<Arc<Session>>, capture: Arc<Mutex<Capture>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    let mut decoder = Utf8Decoder::new();
    let mut buf = vec![0u8; 8192];
    loop {
        let n = match reader.read(&mut buf).await {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                tracing::debug!(error = %e, "command output read failed");
                break;
            }
        };
        let chunk = &buf[..n];
        {
            let mut c = capture.lock();
            let room = cap.saturating_sub(c.bytes.len());
            if room >= n {
                c.bytes.extend_from_slice(chunk);
            } else {
                c.bytes.extend_from_slice(&chunk[..room]);
                c.truncated = true;
            }
        }
        if let Some(v) = &view {
            let text = decoder.decode(chunk);
            if !text.is_empty() {
                let _ = v.pump_tx.send(PumpMsg::Text(crlf(&text))).await;
            }
        }
    }
    if let Some(v) = &view {
        let rest = decoder.finish();
        if !rest.is_empty() {
            let _ = v.pump_tx.send(PumpMsg::Text(rest)).await;
        }
    }
}

async fn join_reader(task: Option<(tokio::task::JoinHandle<()>, Arc<Mutex<Capture>>)>) -> (String, bool) {
    let Some((mut task, capture)) = task else { return (String::new(), false) };
    // A surviving grandchild could keep the pipe open; don't hang on it.
    let mut incomplete = false;
    match tokio::time::timeout(Duration::from_secs(3), &mut task).await {
        Ok(Ok(())) => {}
        Ok(Err(e)) => tracing::warn!(error = %e, "output reader task failed"),
        Err(_) => {
            task.abort();
            incomplete = true;
            tracing::warn!("output pipe still open after process exit; output may be incomplete");
        }
    }
    let c = std::mem::take(&mut *capture.lock());
    (String::from_utf8_lossy(&c.bytes).into_owned(), c.truncated || incomplete)
}
