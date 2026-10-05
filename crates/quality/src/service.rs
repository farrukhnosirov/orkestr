//! The quality service: profiles, gate configs, runs and evaluation.

use std::collections::HashMap;
use std::path::{Component, Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{AgentId, ProjectId, Task, TaskId};
use orkestr_events::EventBus;
use orkestr_protocol::event::{BuildFinishedPayload, BuildStartedPayload, EventKind, TestRunFinishedPayload};
use orkestr_protocol::methods::quality::{
    CheckDef, CheckKind, CheckResult, CheckStatus, Detection, DiagnosticSeverity, DodSetting, GateConfig, GateReport,
    GateSetting, QualityProfile, QualityRun, RunTarget, DEFAULT_CHECK_TIMEOUT_SECS, DEFAULT_RUN_BUDGET_SECS,
    MAX_CHECK_TIMEOUT_SECS, MAX_RUN_BUDGET_SECS,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::Db;
use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::detect::{detect, DetectOptions};
use crate::error::{QualityError, QualityResult};
use crate::executor::{CheckExecutor, ExecError, ExecRequest};
use crate::gates::{self, EvaluationInput};
use crate::parse;
use crate::sources::{NoSecurityScanner, ReviewStatusSource, SecurityFindingsSource, TaskFieldReview};
use crate::store::QualityStore;

/// Redaction hook applied to captured output before it is stored or published.
pub type Redactor = Arc<dyn Fn(&str) -> String + Send + Sync>;

/// Max checks in one profile.
pub const MAX_CHECKS: usize = 64;
/// Max command length.
pub const MAX_COMMAND_LEN: usize = 4096;

/// Tunables.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityConfig {
    /// Detection options.
    pub detect: DetectOptions,
    /// Bytes of combined output kept as `rawTail`.
    pub raw_tail_bytes: usize,
    /// Max captured bytes per stream.
    pub max_output: usize,
}

impl Default for QualityConfig {
    fn default() -> Self {
        Self { detect: DetectOptions::default(), raw_tail_bytes: 8 * 1024, max_output: 4 * 1024 * 1024 }
    }
}

/// Dependencies.
pub struct QualityDeps {
    /// Shared database (migrations applied by the composition root).
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Command executor.
    pub executor: Arc<dyn CheckExecutor>,
    /// Output redaction.
    pub redact: Redactor,
    /// Tunables.
    pub config: QualityConfig,
}

/// A request to start a run in a resolved workspace.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartRun {
    /// Project.
    pub project_id: ProjectId,
    /// Task scope.
    pub task_id: Option<TaskId>,
    /// Agent scope (commands are gated as this agent).
    pub agent_id: Option<AgentId>,
    /// Target as requested (stored for display).
    pub target: RunTarget,
    /// Absolute workspace directory (project root or a worktree).
    pub workspace: PathBuf,
    /// Checks of the profile.
    pub checks: Vec<CheckDef>,
    /// Kinds subset (empty = all enabled).
    pub kinds: Vec<CheckKind>,
    /// Explicit check ids (run even if disabled).
    pub check_ids: Vec<String>,
    /// Overall time budget.
    pub budget: Duration,
}

struct ActiveRun {
    workspace_key: String,
    token: CancellationToken,
    done: watch::Receiver<bool>,
    state: Arc<Mutex<QualityRun>>,
}

struct Inner {
    store: QualityStore,
    events: EventBus,
    executor: Arc<dyn CheckExecutor>,
    redact: Redactor,
    config: QualityConfig,
    review: RwLock<Arc<dyn ReviewStatusSource>>,
    security: RwLock<Arc<dyn SecurityFindingsSource>>,
    active: Mutex<HashMap<String, ActiveRun>>,
    shutdown: CancellationToken,
}

/// Quality runner + gates.
#[derive(Clone)]
pub struct QualityService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for QualityService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("QualityService").field("active_runs", &self.inner.active.lock().len()).finish_non_exhaustive()
    }
}

fn workspace_key(p: &Path) -> String {
    p.to_string_lossy().replace('\\', "/").trim_end_matches('/').to_lowercase()
}

/// Validates a relative, `/`-separated cwd that stays inside the workspace.
fn validate_cwd(cwd: &str) -> QualityResult<PathBuf> {
    let c = cwd.trim();
    if c.is_empty() || c == "." {
        return Ok(PathBuf::from("."));
    }
    let p = Path::new(c);
    if p.is_absolute() || c.starts_with('/') || c.starts_with('\\') || c.contains(':') {
        return Err(QualityError::Invalid(format!("check cwd `{c}` must be relative to the workspace")));
    }
    for comp in p.components() {
        match comp {
            Component::Normal(_) | Component::CurDir => {}
            _ => return Err(QualityError::Invalid(format!("check cwd `{c}` must stay inside the workspace"))),
        }
    }
    Ok(p.to_path_buf())
}

/// Validates profile checks.
pub fn validate_checks(checks: &[CheckDef]) -> QualityResult<()> {
    if checks.len() > MAX_CHECKS {
        return Err(QualityError::Invalid(format!("at most {MAX_CHECKS} checks")));
    }
    let mut ids = std::collections::HashSet::new();
    for c in checks {
        if c.id.trim().is_empty() || c.id.len() > 200 {
            return Err(QualityError::Invalid("check id must be 1..=200 characters".into()));
        }
        if !ids.insert(c.id.as_str()) {
            return Err(QualityError::Invalid(format!("duplicate check id `{}`", c.id)));
        }
        let cmd = c.command.trim();
        if cmd.is_empty() || cmd.len() > MAX_COMMAND_LEN {
            return Err(QualityError::Invalid(format!(
                "check `{}`: command must be 1..={MAX_COMMAND_LEN} bytes",
                c.id
            )));
        }
        if cmd.contains(['\n', '\r', '\0']) {
            return Err(QualityError::Invalid(format!("check `{}`: command must be a single line", c.id)));
        }
        if c.label.len() > 200 {
            return Err(QualityError::Invalid(format!("check `{}`: label too long", c.id)));
        }
        if let Some(t) = c.timeout_secs {
            if t == 0 || t > MAX_CHECK_TIMEOUT_SECS {
                return Err(QualityError::Invalid(format!(
                    "check `{}`: timeout must be 1..={MAX_CHECK_TIMEOUT_SECS} seconds",
                    c.id
                )));
            }
        }
        validate_cwd(&c.cwd)?;
    }
    Ok(())
}

/// Picks the checks of a run, in execution (kind) order.
#[must_use]
pub fn select_checks(checks: &[CheckDef], kinds: &[CheckKind], ids: &[String]) -> Vec<CheckDef> {
    let mut out: Vec<CheckDef> = if ids.is_empty() {
        checks.iter().filter(|c| c.enabled && (kinds.is_empty() || kinds.contains(&c.kind))).cloned().collect()
    } else {
        checks.iter().filter(|c| ids.iter().any(|i| i == &c.id)).cloned().collect()
    };
    out.sort_by_key(|c| c.kind);
    out
}

fn tail(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut start = s.len() - max;
    while !s.is_char_boundary(start) {
        start += 1;
    }
    &s[start..]
}

fn ms(d: Duration) -> u64 {
    u64::try_from(d.as_millis()).unwrap_or(u64::MAX)
}

fn queued(c: &CheckDef) -> CheckResult {
    CheckResult {
        check_id: c.id.clone(),
        kind: c.kind,
        toolchain: c.toolchain,
        label: c.label.clone(),
        command: c.command.clone(),
        cwd: c.cwd.clone(),
        status: CheckStatus::Queued,
        exit_code: None,
        duration_ms: 0,
        passed: 0,
        failed: 0,
        skipped: 0,
        failures: Vec::new(),
        diagnostics: Vec::new(),
        raw_tail: String::new(),
        summary: String::new(),
        approval_id: None,
        started_at: None,
    }
}

/// One-line English summary (clients localize from the structured fields).
fn summarize(r: &CheckResult) -> String {
    match r.status {
        CheckStatus::TimedOut => format!("timed out after {}s", r.duration_ms / 1000),
        CheckStatus::Cancelled => "cancelled".into(),
        CheckStatus::Skipped | CheckStatus::Blocked | CheckStatus::Error if !r.summary.is_empty() => r.summary.clone(),
        _ => {
            if r.kind.is_test() && (r.passed + r.failed + r.skipped) > 0 {
                return format!("{} passed, {} failed, {} skipped", r.passed, r.failed, r.skipped);
            }
            let errors = r.diagnostics.iter().filter(|d| d.severity == DiagnosticSeverity::Error).count();
            let warnings = r.diagnostics.iter().filter(|d| d.severity == DiagnosticSeverity::Warning).count();
            if errors + warnings > 0 {
                let first = r
                    .diagnostics
                    .first()
                    .map(|d| match (&d.file, d.line) {
                        (Some(f), Some(l)) => format!(" — {f}:{l}: {}", d.message),
                        _ => format!(" — {}", d.message),
                    })
                    .unwrap_or_default();
                return format!("{errors} errors, {warnings} warnings{first}");
            }
            match r.exit_code {
                Some(0) => "ok".into(),
                Some(c) => format!("exit code {c}"),
                None => "no exit code".into(),
            }
        }
    }
}

impl QualityService {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: QualityDeps) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: QualityStore::new(deps.db),
                events: deps.events,
                executor: deps.executor,
                redact: deps.redact,
                config: deps.config,
                review: RwLock::new(Arc::new(TaskFieldReview)),
                security: RwLock::new(Arc::new(NoSecurityScanner)),
                active: Mutex::new(HashMap::new()),
                shutdown: CancellationToken::new(),
            }),
        }
    }

    /// Startup: runs left running by a previous session become `Cancelled`.
    pub async fn init(&self) -> QualityResult<()> {
        let n = self.inner.store.interrupt_stale(orkestr_domain::now()).await?;
        if n > 0 {
            tracing::info!(count = n, "quality runs interrupted by the previous shutdown were marked cancelled");
        }
        Ok(())
    }

    /// Plugs in the review area's verdict source.
    pub fn set_review_source(&self, source: Arc<dyn ReviewStatusSource>) {
        *self.inner.review.write() = source;
    }

    /// Plugs in a security findings source.
    pub fn set_security_source(&self, source: Arc<dyn SecurityFindingsSource>) {
        *self.inner.security.write() = source;
    }

    /// Detects toolchains under `root` (blocking I/O on the blocking pool).
    pub async fn detect(&self, root: &Path) -> Detection {
        let root = root.to_path_buf();
        let opts = self.inner.config.detect;
        tokio::task::spawn_blocking(move || detect(&root, opts)).await.unwrap_or_else(|e| Detection {
            roots: Vec::new(),
            checks: Vec::new(),
            notes: vec![format!("detection failed: {e}")],
        })
    }

    /// Saved overrides, else live detection in `root` (empty without a root).
    pub async fn profile(&self, project_id: ProjectId, root: Option<&Path>) -> QualityResult<QualityProfile> {
        if let Some((checks, at)) = self.inner.store.profile(project_id).await? {
            return Ok(QualityProfile {
                project_id,
                checks,
                customized: true,
                updated_at: Some(at),
                roots: Vec::new(),
            });
        }
        let detection = match root {
            Some(r) => self.detect(r).await,
            None => Detection { roots: Vec::new(), checks: Vec::new(), notes: Vec::new() },
        };
        Ok(QualityProfile {
            project_id,
            checks: detection.checks,
            customized: false,
            updated_at: None,
            roots: detection.roots,
        })
    }

    /// Saves overrides (or resets to detection).
    pub async fn set_profile(
        &self,
        project_id: ProjectId,
        checks: Vec<CheckDef>,
        reset: bool,
        root: Option<&Path>,
    ) -> QualityResult<QualityProfile> {
        if reset {
            self.inner.store.delete_profile(project_id).await?;
            return self.profile(project_id, root).await;
        }
        validate_checks(&checks)?;
        let checks: Vec<CheckDef> = checks
            .into_iter()
            .map(|mut c| {
                c.command = c.command.trim().to_owned();
                c.cwd = if c.cwd.trim().is_empty() { ".".into() } else { c.cwd.trim().replace('\\', "/") };
                if c.label.trim().is_empty() {
                    c.label.clone_from(&c.command);
                }
                c
            })
            .collect();
        let now = orkestr_domain::now();
        self.inner.store.save_profile(project_id, &checks, now).await?;
        Ok(QualityProfile { project_id, checks, customized: true, updated_at: Some(now), roots: Vec::new() })
    }

    /// Gate configuration (saved or defaults).
    pub async fn gates(&self, project_id: ProjectId) -> QualityResult<GateConfig> {
        Ok(match self.inner.store.gates(project_id).await? {
            Some(cfg) => {
                let mut n = gates::normalize(project_id, &cfg.gates, &cfg.dod);
                n.customized = true;
                n.updated_at = cfg.updated_at;
                n
            }
            None => gates::default_config(project_id),
        })
    }

    /// Updates the gate configuration.
    pub async fn set_gates(
        &self,
        project_id: ProjectId,
        gates_in: &[GateSetting],
        dod: &[DodSetting],
        reset: bool,
    ) -> QualityResult<GateConfig> {
        if reset {
            self.inner.store.delete_gates(project_id).await?;
            return Ok(gates::default_config(project_id));
        }
        let current = self.gates(project_id).await?;
        // Missing entries keep their current values.
        let merged_gates: Vec<GateSetting> = current
            .gates
            .iter()
            .map(|g| gates_in.iter().find(|s| s.gate == g.gate).cloned().unwrap_or_else(|| g.clone()))
            .collect();
        let merged_dod: Vec<DodSetting> = current
            .dod
            .iter()
            .map(|d| dod.iter().find(|s| s.item == d.item).cloned().unwrap_or_else(|| d.clone()))
            .collect();
        let mut cfg = gates::normalize(project_id, &merged_gates, &merged_dod);
        cfg.customized = true;
        cfg.updated_at = Some(orkestr_domain::now());
        self.inner.store.save_gates(&cfg).await?;
        Ok(cfg)
    }

    /// A run (live state for active runs).
    pub async fn run(&self, run_id: &str) -> QualityResult<QualityRun> {
        if let Some(state) = self.inner.active.lock().get(run_id).map(|a| Arc::clone(&a.state)) {
            return Ok(state.lock().clone());
        }
        self.inner.store.run(run_id).await?.ok_or_else(|| QualityError::NotFound(format!("quality run {run_id}")))
    }

    /// Runs of a project / task, newest first.
    pub async fn runs(
        &self,
        project_id: ProjectId,
        task_id: Option<TaskId>,
        limit: u32,
    ) -> QualityResult<Vec<QualityRun>> {
        let mut runs = self.inner.store.runs(project_id, task_id, limit.clamp(1, 200)).await?;
        let active = self.inner.active.lock();
        for r in &mut runs {
            if let Some(a) = active.get(&r.id) {
                *r = a.state.lock().clone();
            }
        }
        Ok(runs)
    }

    /// Signals cancellation of an active run.
    #[must_use]
    pub fn cancel(&self, run_id: &str) -> bool {
        match self.inner.active.lock().get(run_id) {
            Some(a) => {
                a.token.cancel();
                true
            }
            None => false,
        }
    }

    /// Cancels every active run of a project (emergency stop).
    pub fn cancel_project(&self, project_id: ProjectId) -> usize {
        let active = self.inner.active.lock();
        let mut n = 0;
        for a in active.values() {
            if a.state.lock().project_id == project_id {
                a.token.cancel();
                n += 1;
            }
        }
        n
    }

    /// Cancels every active run (emergency stop all).
    pub fn cancel_all(&self) -> usize {
        let active = self.inner.active.lock();
        for a in active.values() {
            a.token.cancel();
        }
        active.len()
    }

    /// Waits until a run is final and returns it.
    pub async fn wait(&self, run_id: &str) -> QualityResult<QualityRun> {
        let rx = self.inner.active.lock().get(run_id).map(|a| a.done.clone());
        if let Some(mut rx) = rx {
            let _ = rx.wait_for(|d| *d).await;
        }
        self.run(run_id).await
    }

    /// Starts a run in the background and returns its initial state.
    pub async fn start(&self, req: StartRun) -> QualityResult<QualityRun> {
        if self.inner.shutdown.is_cancelled() {
            return Err(QualityError::ShuttingDown);
        }
        validate_checks(&req.checks)?;
        let checks = select_checks(&req.checks, &req.kinds, &req.check_ids);
        if checks.is_empty() {
            return Err(QualityError::NothingToRun(if req.checks.is_empty() {
                "no checks are configured for this project".into()
            } else {
                "no enabled check matches the requested kinds".into()
            }));
        }
        if !req.workspace.is_dir() {
            return Err(QualityError::Invalid(format!("workspace {} does not exist", req.workspace.display())));
        }
        let key = workspace_key(&req.workspace);
        let budget = req.budget.clamp(Duration::from_secs(1), Duration::from_secs(u64::from(MAX_RUN_BUDGET_SECS)));
        let run = QualityRun {
            id: uuid::Uuid::now_v7().to_string(),
            project_id: req.project_id,
            task_id: req.task_id,
            agent_id: req.agent_id,
            target: req.target,
            workspace: req.workspace.to_string_lossy().into_owned(),
            kinds: req.kinds,
            status: CheckStatus::Running,
            results: checks.iter().map(queued).collect(),
            started_at: orkestr_domain::now(),
            finished_at: None,
            duration_ms: 0,
        };
        let token = self.inner.shutdown.child_token();
        let (tx, rx) = watch::channel(false);
        let state = Arc::new(Mutex::new(run.clone()));
        {
            let mut active = self.inner.active.lock();
            if active.values().any(|a| a.workspace_key == key) {
                return Err(QualityError::Busy(run.workspace));
            }
            active.insert(
                run.id.clone(),
                ActiveRun { workspace_key: key, token: token.clone(), done: rx, state: Arc::clone(&state) },
            );
        }
        if let Err(e) = self.inner.store.save_run(&run).await {
            self.inner.active.lock().remove(&run.id);
            return Err(e.into());
        }
        let inner = Arc::clone(&self.inner);
        let workspace = req.workspace;
        tokio::spawn(async move {
            execute_run(&inner, &state, &checks, &workspace, budget, &token).await;
            let final_run = state.lock().clone();
            if let Err(e) = inner.store.save_run(&final_run).await {
                tracing::error!(error = %e, run = %final_run.id, "failed to persist quality run");
            }
            inner.active.lock().remove(&final_run.id);
            let _ = tx.send(true);
        });
        Ok(run)
    }

    /// Default overall budget.
    #[must_use]
    pub fn default_budget() -> Duration {
        Duration::from_secs(u64::from(DEFAULT_RUN_BUDGET_SECS))
    }

    /// Evaluates gates + DoD. `configured_kinds`: kinds with an enabled
    /// check in the profile; `task_snapshots`: snapshots recorded for the task.
    pub async fn evaluate(
        &self,
        project_id: ProjectId,
        task: Option<&Task>,
        configured_kinds: &[CheckKind],
        task_snapshots: usize,
    ) -> QualityResult<GateReport> {
        let config = self.gates(project_id).await?;
        let runs = self.runs(project_id, task.map(|t| t.id), 50).await?;
        let run_id = runs.first().map(|r| r.id.clone());
        let results: Vec<(String, CheckResult)> =
            runs.iter().flat_map(|r| r.results.iter().map(|c| (r.id.clone(), c.clone()))).collect();
        let review = Arc::clone(&*self.inner.review.read());
        let security = Arc::clone(&*self.inner.security.read());
        let review = review.review_status(project_id, task).await;
        let security = security.security_status(project_id, task).await;
        let input = EvaluationInput {
            project_id,
            task,
            config: &config,
            configured_kinds,
            results: &results,
            run_id,
            review,
            security,
            task_snapshots,
        };
        Ok(gates::evaluate(&input, orkestr_domain::now()))
    }

    /// Cancels every active run and waits (bounded) for them to stop.
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        let waits: Vec<watch::Receiver<bool>> = self.inner.active.lock().values().map(|a| a.done.clone()).collect();
        for mut rx in waits {
            let _ = tokio::time::timeout(Duration::from_secs(10), rx.wait_for(|d| *d)).await;
        }
    }
}

async fn publish(inner: &Inner, run: &QualityRun, kind: EventKind) {
    let mut ev = NewEvent::new(kind).project(run.project_id);
    if let Some(t) = run.task_id {
        ev = ev.task(t);
    }
    if let Some(a) = run.agent_id {
        ev = ev.agent(a);
    }
    if let Err(e) = inner.events.publish(ev).await {
        tracing::warn!(error = %e, "failed to publish quality event");
    }
}

fn set_result(state: &Mutex<QualityRun>, idx: usize, r: CheckResult) {
    let mut s = state.lock();
    if let Some(slot) = s.results.get_mut(idx) {
        *slot = r;
    }
}

async fn persist(inner: &Inner, state: &Mutex<QualityRun>) {
    let snapshot = state.lock().clone();
    if let Err(e) = inner.store.save_run(&snapshot).await {
        tracing::warn!(error = %e, "failed to persist quality run progress");
    }
}

#[allow(clippy::too_many_lines)]
async fn execute_run(
    inner: &Inner,
    state: &Mutex<QualityRun>,
    checks: &[CheckDef],
    workspace: &Path,
    budget: Duration,
    token: &CancellationToken,
) {
    let started = Instant::now();
    let (project_id, task_id, agent_id, run_id) = {
        let s = state.lock();
        (s.project_id, s.task_id, s.agent_id, s.id.clone())
    };
    let mut build_failed = false;
    for (idx, check) in checks.iter().enumerate() {
        let mut result = queued(check);
        if token.is_cancelled() {
            result.status = CheckStatus::Cancelled;
            result.summary = summarize(&result);
            set_result(state, idx, result);
            continue;
        }
        let elapsed = started.elapsed();
        if elapsed >= budget {
            result.status = CheckStatus::Skipped;
            result.summary = "time budget of the run exhausted".into();
            set_result(state, idx, result);
            continue;
        }
        if build_failed && check.kind.is_test() {
            result.status = CheckStatus::Skipped;
            result.summary = "skipped: the build failed".into();
            set_result(state, idx, result);
            continue;
        }
        let cwd = match validate_cwd(&check.cwd) {
            Ok(c) => c,
            Err(e) => {
                result.status = CheckStatus::Error;
                result.summary = e.to_string();
                set_result(state, idx, result);
                continue;
            }
        };
        let per_check = Duration::from_secs(u64::from(check.timeout_secs.unwrap_or(DEFAULT_CHECK_TIMEOUT_SECS)));
        let timeout = per_check.min(budget.saturating_sub(elapsed)).max(Duration::from_secs(1));
        result.status = CheckStatus::Running;
        result.started_at = Some(orkestr_domain::now());
        set_result(state, idx, result.clone());
        persist(inner, state).await;
        let build_id = format!("{run_id}:{}", check.id);
        {
            let s = state.lock().clone();
            publish(
                inner,
                &s,
                EventKind::BuildStarted(BuildStartedPayload {
                    build_id: build_id.clone(),
                    command: (inner.redact)(&check.command),
                }),
            )
            .await;
        }
        let req = ExecRequest {
            project_id,
            task_id,
            agent_id,
            root: workspace.to_path_buf(),
            cwd,
            command: check.command.clone(),
            timeout,
            max_output: inner.config.max_output,
        };
        let t0 = Instant::now();
        match inner.executor.execute(req, token.child_token()).await {
            Ok(out) => {
                let combined = if out.stderr.is_empty() {
                    out.stdout.clone()
                } else if out.stdout.is_empty() {
                    out.stderr.clone()
                } else {
                    format!("{}\n{}", out.stdout, out.stderr)
                };
                let parsed = parse::parse(check.kind, &combined);
                result.exit_code = out.exit_code;
                result.duration_ms = ms(out.duration);
                if let Some(t) = parsed.tests {
                    result.passed = t.passed;
                    result.failed = t.failed;
                    result.skipped = t.skipped;
                }
                let redact = |s: &str| (inner.redact)(s);
                result.failures = parsed
                    .failures
                    .into_iter()
                    .map(|mut f| {
                        f.message = redact(&f.message);
                        f
                    })
                    .collect();
                result.diagnostics = parsed
                    .diagnostics
                    .into_iter()
                    .map(|mut d| {
                        d.message = redact(&d.message);
                        d
                    })
                    .collect();
                result.raw_tail =
                    redact(tail(&parse::strip_ansi(&combined), inner.config.raw_tail_bytes)).trim_end().to_owned();
                result.status = if out.timed_out {
                    CheckStatus::TimedOut
                } else if out.exit_code == Some(0) && result.failed == 0 {
                    CheckStatus::Passed
                } else {
                    CheckStatus::Failed
                };
            }
            Err(ExecError::Cancelled) => {
                result.status = CheckStatus::Cancelled;
                result.duration_ms = ms(t0.elapsed());
            }
            Err(ExecError::Denied(reason)) => {
                result.status = CheckStatus::Blocked;
                result.summary = format!("blocked by the security policy: {reason}");
            }
            Err(ExecError::ApprovalRequired { reason, approval_id }) => {
                result.status = CheckStatus::Blocked;
                result.summary = format!("awaiting approval: {reason}");
                result.approval_id = approval_id;
            }
            Err(ExecError::Failed(msg)) => {
                result.status = CheckStatus::Error;
                result.summary = (inner.redact)(&msg);
                result.duration_ms = ms(t0.elapsed());
            }
        }
        result.summary = summarize(&result);
        if check.kind == CheckKind::Build && result.status != CheckStatus::Passed {
            build_failed = true;
        }
        set_result(state, idx, result.clone());
        let snapshot = state.lock().clone();
        let kind = if check.kind.is_test() {
            EventKind::TestRunFinished(TestRunFinishedPayload {
                run_id: build_id,
                passed: result.passed,
                failed: result.failed,
                skipped: result.skipped,
                duration_ms: result.duration_ms,
                summary: format!("{}: {}", check.label, result.summary),
            })
        } else {
            let payload = BuildFinishedPayload {
                build_id,
                exit_code: result.exit_code,
                duration_ms: result.duration_ms,
                summary: format!("{}: {}", check.label, result.summary),
            };
            if result.status == CheckStatus::Passed {
                EventKind::BuildSucceeded(payload)
            } else {
                EventKind::BuildFailed(payload)
            }
        };
        publish(inner, &snapshot, kind).await;
        persist(inner, state).await;
    }
    let mut s = state.lock();
    let statuses: Vec<CheckStatus> = s.results.iter().map(|r| r.status).collect();
    s.status = if statuses
        .iter()
        .any(|st| matches!(st, CheckStatus::Failed | CheckStatus::TimedOut | CheckStatus::Error | CheckStatus::Blocked))
    {
        CheckStatus::Failed
    } else if token.is_cancelled() || statuses.contains(&CheckStatus::Cancelled) {
        CheckStatus::Cancelled
    } else if statuses.contains(&CheckStatus::Passed) && !statuses.contains(&CheckStatus::Skipped) {
        CheckStatus::Passed
    } else if statuses.contains(&CheckStatus::Passed) {
        // Some checks skipped (budget): not a clean pass.
        CheckStatus::Failed
    } else {
        CheckStatus::Skipped
    };
    s.finished_at = Some(orkestr_domain::now());
    s.duration_ms = ms(started.elapsed());
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::quality::{CheckSource, Toolchain};

    fn def(id: &str, kind: CheckKind, enabled: bool) -> CheckDef {
        CheckDef {
            id: id.into(),
            kind,
            toolchain: Toolchain::Custom,
            label: id.into(),
            command: format!("run {id}"),
            cwd: ".".into(),
            enabled,
            timeout_secs: None,
            source: CheckSource::User,
        }
    }

    #[test]
    fn selection_order_and_filters() {
        let checks =
            vec![def("t", CheckKind::Test, true), def("b", CheckKind::Build, true), def("l", CheckKind::Lint, false)];
        let all = select_checks(&checks, &[], &[]);
        assert_eq!(all.iter().map(|c| c.id.as_str()).collect::<Vec<_>>(), vec!["b", "t"]);
        assert_eq!(select_checks(&checks, &[CheckKind::Test], &[]).len(), 1);
        assert_eq!(select_checks(&checks, &[], &["l".into()])[0].id, "l", "explicit ids run disabled checks");
    }

    #[test]
    fn validation() {
        let mut c = def("a", CheckKind::Build, true);
        assert!(validate_checks(std::slice::from_ref(&c)).is_ok());
        c.cwd = "../x".into();
        assert!(validate_checks(std::slice::from_ref(&c)).is_err());
        c.cwd = "C:/x".into();
        assert!(validate_checks(std::slice::from_ref(&c)).is_err());
        c.cwd = "sub/dir".into();
        c.command = "a\nb".into();
        assert!(validate_checks(std::slice::from_ref(&c)).is_err());
        c.command = "ok".into();
        c.timeout_secs = Some(0);
        assert!(validate_checks(std::slice::from_ref(&c)).is_err());
        let d = def("a", CheckKind::Test, true);
        assert!(validate_checks(&[def("a", CheckKind::Build, true), d]).is_err(), "duplicate ids");
    }

    #[test]
    fn tail_respects_char_boundaries() {
        let s = "ааааа"; // 2 bytes per char
        let t = tail(s, 3);
        assert!(s.ends_with(t));
        assert_eq!(t, "а");
    }
}
