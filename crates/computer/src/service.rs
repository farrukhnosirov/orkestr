//! [`ComputerService`]: catalogue + user sets + matcher index, runs with
//! confirmations (approvals inbox / Dangerous Action Guard), AI plans,
//! "remember this as …", timers, global stop, history, audit and events.

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use parking_lot::{Mutex, RwLock};
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use orkestr_domain::{ApprovalStatus, AuditOutcome, Capability, DangerCategory, RiskLevel};
use orkestr_events::EventBus;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::computer::{
    AssistantCommand, CatalogueParams, CatalogueResult, CategoryCount, CommandMatch, CommandResult, CommandSet,
    CommandSetSummary, CommandSummary, ComputerAction, ComputerCommandPayload, ComputerStatus, ExportSetResult,
    HandleResult, HistoryEntry, LocalizedText, PendingConfirmation, PlanProposal, ProcessInfo, RiskReason, RunOutcome,
    ComputerRunReport, SafetyLevel, StopResult, WindowInfo,
};
use orkestr_protocol::NewEvent;
use orkestr_security::{ApprovalDecision, ApprovalRequest, NewAudit, Security, SecurityError};

use crate::backend::{ComputerBackend, InstalledApp};
use crate::catalogue::builtin_sets;
use crate::error::{ComputerError, ComputerResult};
use crate::executor::{execute, substitute, ExecEnv, TimerFn};
use crate::matcher::{Entry, Matcher, MATCH_THRESHOLD};
use crate::planner::{parse_plan, system_prompt, user_prompt, PlanModel};
use crate::safety;
use crate::store::{export_document, parse_set_document, SetStore};

/// Default user set for remembered commands.
pub const MY_SET: &str = "my-commands";
const HISTORY_MAX: usize = 100;
const CONFIRM_TTL: Duration = Duration::from_secs(10 * 60);

/// Dependencies.
pub struct ComputerDeps {
    /// Event bus.
    pub events: EventBus,
    /// Security (approvals, audit, guard).
    pub security: Arc<Security>,
    /// `<data>/computer`.
    pub data_dir: PathBuf,
    /// OS backend.
    pub backend: Arc<dyn ComputerBackend>,
}

struct Index {
    matcher: Matcher,
    owners: Vec<(usize, usize)>,
}

struct Pending {
    title: String,
    command_id: Option<String>,
    matched: Option<CommandMatch>,
    steps: Vec<ComputerAction>,
    reasons: Vec<RiskReason>,
    approval_id: Option<orkestr_domain::ApprovalId>,
    text: String,
    result: watch::Sender<Option<HandleResult>>,
}

/// Where a run came from.
#[derive(Debug, Clone, Default)]
pub struct RunSpec {
    /// Title.
    pub title: String,
    /// Command.
    pub command_id: Option<String>,
    /// Declared safety.
    pub declared: SafetyLevel,
    /// Steps (templates).
    pub steps: Vec<ComputerAction>,
    /// Slot values.
    pub slots: BTreeMap<String, String>,
    /// Match.
    pub matched: Option<CommandMatch>,
    /// Utterance.
    pub text: String,
    /// Source (`voice`, `text`, …).
    pub source: String,
}

/// The computer assistant.
pub struct ComputerService {
    deps: ComputerDeps,
    store: SetStore,
    sets: RwLock<Vec<CommandSet>>,
    index: RwLock<Arc<Index>>,
    installed: RwLock<(Arc<Vec<InstalledApp>>, Option<Instant>)>,
    pending: Mutex<HashMap<String, Pending>>,
    plans: Mutex<HashMap<String, PlanProposal>>,
    last_plan: Mutex<Option<String>>,
    model: RwLock<Option<Arc<dyn PlanModel>>>,
    stop: Mutex<CancellationToken>,
    timers: Mutex<Vec<CancellationToken>>,
    running: AtomicU32,
    history: Mutex<VecDeque<HistoryEntry>>,
    own_pids: Mutex<Vec<u32>>,
    catalogue_size: u32,
}

impl std::fmt::Debug for ComputerService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ComputerService").field("backend", &self.deps.backend.name()).finish_non_exhaustive()
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

fn now() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now()
}

impl ComputerService {
    /// Loads the catalogue and the user sets.
    #[must_use]
    pub fn new(deps: ComputerDeps) -> Arc<Self> {
        let store = SetStore::new(deps.data_dir.clone());
        let mut sets = builtin_sets().unwrap_or_else(|e| {
            tracing::error!(error = %e, "built-in command catalogue is broken");
            Vec::new()
        });
        let catalogue_size = u32::try_from(sets.iter().map(|s| s.commands.len()).sum::<usize>()).unwrap_or(0);
        let state = store.state();
        for s in &mut sets {
            s.enabled = !state.disabled_builtin.contains(&s.id);
        }
        sets.extend(store.load_user_sets());
        let svc = Arc::new(Self {
            deps,
            store,
            sets: RwLock::new(sets),
            index: RwLock::new(Arc::new(Index { matcher: Matcher::default(), owners: Vec::new() })),
            installed: RwLock::new((Arc::new(Vec::new()), None)),
            pending: Mutex::new(HashMap::new()),
            plans: Mutex::new(HashMap::new()),
            last_plan: Mutex::new(None),
            model: RwLock::new(None),
            stop: Mutex::new(CancellationToken::new()),
            timers: Mutex::new(Vec::new()),
            running: AtomicU32::new(0),
            history: Mutex::new(VecDeque::new()),
            own_pids: Mutex::new(vec![std::process::id()]),
            catalogue_size,
        });
        svc.rebuild_index();
        svc
    }

    /// Wires the AI planner model.
    pub fn set_model(&self, model: Arc<dyn PlanModel>) {
        *self.model.write() = Some(model);
    }

    /// Adds processes that count as "Orkestr itself" (never input targets).
    pub fn add_own_pid(&self, pid: u32) {
        let mut v = self.own_pids.lock();
        if !v.contains(&pid) {
            v.push(pid);
        }
    }

    /// Indexes Start-menu apps in the background (refreshed every 10 min on use).
    pub fn refresh_apps(self: &Arc<Self>) {
        {
            let mut g = self.installed.write();
            if g.1.is_some_and(|t| t.elapsed() < Duration::from_secs(600)) {
                return;
            }
            g.1 = Some(Instant::now());
        }
        let me = Arc::clone(self);
        tokio::task::spawn_blocking(move || match me.deps.backend.installed_apps() {
            Ok(apps) => {
                tracing::debug!(count = apps.len(), "indexed Start-menu apps");
                me.installed.write().0 = Arc::new(apps);
            }
            Err(e) => tracing::warn!(error = %e, "could not index Start-menu apps"),
        });
    }

    fn rebuild_index(&self) {
        let sets = self.sets.read();
        let mut entries = Vec::new();
        let mut owners = Vec::new();
        for (si, s) in sets.iter().enumerate() {
            if !s.enabled {
                continue;
            }
            for (ci, c) in s.commands.iter().enumerate() {
                entries.push(Entry {
                    phrases: c.phrases.clone(),
                    context: c.context.clone().or_else(|| s.context.clone()),
                    bonus: if s.builtin { 0.0 } else { 0.02 },
                });
                owners.push((si, ci));
            }
        }
        *self.index.write() = Arc::new(Index { matcher: Matcher::new(entries), owners });
    }

    fn stop_token(&self) -> CancellationToken {
        self.stop.lock().clone()
    }

    fn env(self: &Arc<Self>) -> ExecEnv {
        let me = Arc::downgrade(self);
        let timers: TimerFn = Arc::new(move |secs, msg| {
            if let Some(s) = me.upgrade() {
                s.start_timer(secs, msg);
            }
        });
        ExecEnv {
            backend: Arc::clone(&self.deps.backend),
            installed: Arc::clone(&self.installed.read().0),
            cancel: self.stop_token().child_token(),
            own_pids: self.own_pids.lock().clone(),
            timers,
            screenshot_dir: directories::UserDirs::new()
                .and_then(|u| u.picture_dir().map(|p| p.join("Screenshots")))
                .unwrap_or_else(|| self.deps.data_dir.join("screenshots")),
        }
    }

    /// The user's window (not Orkestr's).
    async fn user_window(&self) -> Option<WindowInfo> {
        let b = Arc::clone(&self.deps.backend);
        let own = self.own_pids.lock().clone();
        let ws = tokio::task::spawn_blocking(move || b.windows()).await.ok()?.ok()?;
        let is_own = |w: &WindowInfo| {
            let p = crate::procs::norm(&w.process);
            own.contains(&w.pid) || p == "orkestr-desktop" || p == "orkestrd"
        };
        ws.iter().find(|w| w.active && !is_own(w)).or_else(|| ws.iter().find(|w| !is_own(w) && !w.minimized)).cloned()
    }

    // ── status / matching ─────────────────────────────────────────────────

    /// Status.
    #[must_use]
    pub fn status(&self) -> ComputerStatus {
        let avail = self.deps.backend.available();
        let sets = self.sets.read();
        ComputerStatus {
            available: avail.is_ok(),
            backend: self.deps.backend.name().into(),
            unavailable_reason: avail.err().map(|e| e.to_string()),
            catalogue_commands: self.catalogue_size,
            user_commands: u32::try_from(sets.iter().filter(|s| !s.builtin).map(|s| s.commands.len()).sum::<usize>())
                .unwrap_or(0),
            enabled_sets: u32::try_from(sets.iter().filter(|s| s.enabled).count()).unwrap_or(0),
            running: self.running.load(Ordering::Relaxed),
            pending: u32::try_from(self.pending.lock().len()).unwrap_or(0),
            indexed_apps: u32::try_from(self.installed.read().0.len()).unwrap_or(0),
        }
    }

    fn to_match(&self, raw: &crate::matcher::RawMatch, idx: &Index, lang: &str) -> Option<CommandMatch> {
        let (si, ci) = *idx.owners.get(raw.owner)?;
        let sets = self.sets.read();
        let s = sets.get(si)?;
        let c = s.commands.get(ci)?;
        Some(CommandMatch {
            set_id: s.id.clone(),
            command_id: c.id.clone(),
            title: title_of(c, lang),
            phrase: raw.phrase.clone(),
            score: raw.score,
            slots: raw.slots.clone(),
            safety: declared_level(c),
        })
    }

    /// Matches only.
    pub async fn match_text(&self, text: &str, lang: &str, limit: usize) -> (Vec<CommandMatch>, u64) {
        let active = self.user_window().await;
        let idx = Arc::clone(&self.index.read());
        let t = Instant::now();
        let raw = idx.matcher.find(text, active.as_ref(), limit.clamp(1, 50), 0.35);
        let us = u64::try_from(t.elapsed().as_micros()).unwrap_or(u64::MAX);
        (raw.iter().filter_map(|r| self.to_match(r, &idx, lang)).collect(), us)
    }

    fn command(&self, set_id: Option<&str>, command_id: &str) -> Option<(String, AssistantCommand)> {
        let sets = self.sets.read();
        sets.iter()
            .filter(|s| set_id.is_none_or(|id| s.id == id))
            .find_map(|s| s.commands.iter().find(|c| c.id == command_id).map(|c| (s.id.clone(), c.clone())))
    }

    // ── handle ───────────────────────────────────────────────────────────

    /// Match + run, "remember this as …", or an AI plan.
    ///
    /// # Errors
    /// `Forbidden`, `Invalid`, backend errors.
    pub async fn handle(
        self: &Arc<Self>,
        text: &str,
        lang: &str,
        plan_unknown: bool,
        source: &str,
    ) -> ComputerResult<HandleResult> {
        let text = text.trim();
        if text.is_empty() {
            return Err(ComputerError::Invalid("empty command".into()));
        }
        if text.chars().count() > 2000 {
            return Err(ComputerError::Invalid("command too long (max 2000 characters)".into()));
        }
        self.refresh_apps();
        if let Some(phrase) = remember_phrase(text) {
            let plan_id = self.last_plan.lock().clone();
            if let Some(plan_id) = plan_id {
                let r = self.remember_plan(&plan_id, vec![phrase], None, None)?;
                self.record(text, Some(r.command.id.clone()), &title_of(&r.command, lang), "Remembered");
                return Ok(HandleResult::Remembered { set_id: r.set_id, command: r.command });
            }
        }
        let active = self.user_window().await;
        let idx = Arc::clone(&self.index.read());
        let raw = idx.matcher.find(text, active.as_ref(), 5, 0.35);
        let best = raw.first().filter(|r| r.score >= MATCH_THRESHOLD).cloned();
        if let Some(best) = best {
            if let Some(m) = self.to_match(&best, &idx, lang) {
                if let Some((_, c)) = self.command(Some(&m.set_id), &m.command_id) {
                    let spec = RunSpec {
                        title: title_of(&c, lang),
                        command_id: Some(c.id.clone()),
                        declared: c.safety,
                        steps: c.steps.clone(),
                        slots: m.slots.clone(),
                        matched: Some(m),
                        text: text.to_owned(),
                        source: source.to_owned(),
                    };
                    return self.run_spec(spec, false).await;
                }
            }
        }
        let suggestions: Vec<CommandMatch> = raw.iter().take(3).filter_map(|r| self.to_match(r, &idx, lang)).collect();
        if plan_unknown {
            match self.plan(text, None, None, lang).await {
                Ok(plan) => return Ok(HandleResult::Proposed { plan }),
                Err(e) => {
                    self.record(text, None, text, "NotUnderstood");
                    return Ok(HandleResult::NotUnderstood { suggestions, reason: Some(e.to_string()) });
                }
            }
        }
        self.record(text, None, text, "NotUnderstood");
        Ok(HandleResult::NotUnderstood { suggestions, reason: None })
    }

    /// Runs a command by id.
    ///
    /// # Errors
    /// `NotFound` and run errors.
    pub async fn run_command(
        self: &Arc<Self>,
        set_id: Option<&str>,
        command_id: &str,
        slots: BTreeMap<String, String>,
        lang: &str,
    ) -> ComputerResult<HandleResult> {
        let (_, c) =
            self.command(set_id, command_id).ok_or_else(|| ComputerError::NotFound(format!("command {command_id}")))?;
        let spec = RunSpec {
            title: title_of(&c, lang),
            command_id: Some(c.id.clone()),
            declared: c.safety,
            steps: c.steps.clone(),
            slots,
            matched: None,
            text: title_of(&c, lang),
            source: "run".into(),
        };
        self.run_spec(spec, false).await
    }

    /// Runs ad-hoc steps.
    ///
    /// # Errors
    /// Validation / run errors.
    pub async fn execute_steps(
        self: &Arc<Self>,
        title: &str,
        steps: Vec<ComputerAction>,
        slots: BTreeMap<String, String>,
    ) -> ComputerResult<HandleResult> {
        let probe = AssistantCommand {
            id: "adhoc".into(),
            category: String::new(),
            title: LocalizedText::default(),
            phrases: vec!["x".into()],
            steps: steps.clone(),
            safety: SafetyLevel::Safe,
            context: None,
        };
        crate::catalogue::validate_command(&probe)?;
        let title = if title.trim().is_empty() { "Test".to_owned() } else { title.trim().to_owned() };
        let spec =
            RunSpec { title: title.clone(), steps, slots, text: title, source: "execute".into(), ..RunSpec::default() };
        self.run_spec(spec, false).await
    }

    async fn run_spec(self: &Arc<Self>, spec: RunSpec, confirmed: bool) -> ComputerResult<HandleResult> {
        let resolved = substitute(&spec.steps, &spec.slots)?;
        let mut reasons = match safety::classify(&resolved, Some(&self.deps.security.guard)) {
            Ok(r) => r,
            Err(e) => {
                self.audit(&spec, AuditOutcome::Denied, &e.to_string()).await;
                self.record(&spec.text, spec.command_id.clone(), &spec.title, "Denied");
                return Err(e);
            }
        };
        if reasons.is_empty() && spec.declared > SafetyLevel::Safe {
            reasons.push(RiskReason {
                step: 0,
                code: "declared".into(),
                detail: spec.title.clone(),
                level: spec.declared,
            });
        }
        if !confirmed && !reasons.is_empty() {
            return Ok(self.create_pending(&spec, resolved, reasons).await);
        }
        Ok(self.run_resolved(spec, resolved, confirmed).await)
    }

    /// Boxed with an explicit `Send` bound: runs → pending → (approval) → runs is a cycle.
    fn run_resolved(
        self: &Arc<Self>,
        spec: RunSpec,
        resolved: Vec<ComputerAction>,
        confirmed: bool,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = HandleResult> + Send + '_>> {
        Box::pin(self.run_resolved_inner(spec, resolved, confirmed))
    }

    async fn run_resolved_inner(
        self: &Arc<Self>,
        spec: RunSpec,
        resolved: Vec<ComputerAction>,
        confirmed: bool,
    ) -> HandleResult {
        let started_at = now();
        let env = self.env();
        self.running.fetch_add(1, Ordering::Relaxed);
        let r = execute(&env, &resolved, confirmed).await;
        self.running.fetch_sub(1, Ordering::Relaxed);
        if let Some(c) = r.confirm {
            let rest = resolved[c.resume_at..].to_vec();
            let mut p = self.create_pending(&spec, rest, c.reasons).await;
            if let HandleResult::NeedsConfirmation { pending, .. } = &mut p {
                pending.title = spec.title.clone();
            }
            return p;
        }
        let outcome = r.outcome.unwrap_or(RunOutcome::Failed);
        let run = ComputerRunReport {
            run_id: new_id("run"),
            command_id: spec.command_id.clone(),
            title: spec.title.clone(),
            outcome,
            steps: r.steps,
            effects: r.effects,
            started_at,
            duration_ms: u64::try_from(r.elapsed.as_millis()).unwrap_or(u64::MAX),
        };
        let audit_outcome =
            if outcome == RunOutcome::Succeeded { AuditOutcome::Executed } else { AuditOutcome::Failed };
        self.audit(&spec, audit_outcome, &format!("{outcome:?}")).await;
        self.record(&spec.text, spec.command_id.clone(), &spec.title, &format!("{outcome:?}"));
        self.publish(&run.run_id, spec.command_id.clone(), &spec.title, &format!("{outcome:?}"), None).await;
        HandleResult::Executed { matched: spec.matched, run }
    }

    async fn create_pending(
        self: &Arc<Self>,
        spec: &RunSpec,
        steps: Vec<ComputerAction>,
        reasons: Vec<RiskReason>,
    ) -> HandleResult {
        let id = new_id("pending");
        let level = reasons.iter().map(|r| r.level).max().unwrap_or(SafetyLevel::Confirm);
        let summary = format!(
            "Computer: {} — {}",
            spec.title,
            reasons.iter().map(|r| format!("{} {}", r.code, r.detail)).collect::<Vec<_>>().join("; ")
        );
        let mut req = ApprovalRequest::new(
            Capability::ComputerControl,
            summary.chars().take(300).collect::<String>(),
            "The computer assistant needs your confirmation for a risky step",
            if level == SafetyLevel::Dangerous { RiskLevel::High } else { RiskLevel::Medium },
        );
        req.command_preview = Some(serde_json::to_string_pretty(&steps).unwrap_or_default());
        req.ttl = Some(CONFIRM_TTL);
        req.danger = reasons.iter().find_map(|r| match r.code.as_str() {
            "power" => Some(DangerCategory::SystemConfig),
            "fileDelete" => Some(DangerCategory::RecursiveDelete),
            _ => None,
        });
        let approval_id = match self.deps.security.approvals.request(req).await {
            Ok(a) => Some(a.id),
            Err(e) => {
                tracing::warn!(error = %e, "approval request failed; confirmation stays local");
                None
            }
        };
        let (tx, _rx) = watch::channel(None);
        let pending = PendingConfirmation {
            pending_id: id.clone(),
            approval_id,
            title: spec.title.clone(),
            reasons: reasons.clone(),
            steps: steps.clone(),
        };
        self.pending.lock().insert(
            id.clone(),
            Pending {
                title: spec.title.clone(),
                command_id: spec.command_id.clone(),
                matched: spec.matched.clone(),
                steps,
                reasons,
                approval_id,
                text: spec.text.clone(),
                result: tx,
            },
        );
        self.audit(spec, AuditOutcome::Allowed, "needsConfirmation").await;
        self.record(&spec.text, spec.command_id.clone(), &spec.title, "NeedsConfirmation");
        self.publish(&id, spec.command_id.clone(), &spec.title, "NeedsConfirmation", None).await;
        if let Some(aid) = approval_id {
            // Resolution from anywhere (this assistant, the approvals inbox, a phone) runs it.
            let me = Arc::clone(self);
            let cancel = self.stop_token();
            let pid = id.clone();
            tokio::spawn(async move {
                let status = me
                    .deps
                    .security
                    .approvals
                    .wait(aid, CONFIRM_TTL, &cancel)
                    .await
                    .unwrap_or(ApprovalStatus::Cancelled);
                me.finish_pending(&pid, status == ApprovalStatus::Approved).await;
            });
        }
        HandleResult::NeedsConfirmation { matched: spec.matched.clone(), pending }
    }

    async fn finish_pending(self: &Arc<Self>, pending_id: &str, approve: bool) {
        let Some(p) = self.pending.lock().remove(pending_id) else { return };
        let spec = RunSpec {
            title: p.title.clone(),
            command_id: p.command_id.clone(),
            matched: p.matched.clone(),
            text: p.text.clone(),
            source: "confirm".into(),
            ..RunSpec::default()
        };
        let result = if approve {
            self.run_resolved(spec, p.steps.clone(), true).await
        } else {
            self.audit(&spec, AuditOutcome::Rejected, "rejected").await;
            self.record(&p.text, p.command_id.clone(), &p.title, "Rejected");
            self.publish(pending_id, p.command_id.clone(), &p.title, "Rejected", None).await;
            HandleResult::Executed {
                matched: p.matched.clone(),
                run: ComputerRunReport {
                    run_id: new_id("run"),
                    command_id: p.command_id.clone(),
                    title: p.title.clone(),
                    outcome: RunOutcome::Rejected,
                    steps: Vec::new(),
                    effects: Vec::new(),
                    started_at: now(),
                    duration_ms: 0,
                },
            }
        };
        let _ = p.result.send(Some(result));
        drop(p.reasons);
    }

    /// Approves / rejects a pending run and returns its result.
    ///
    /// # Errors
    /// `NotFound` for unknown / finished ids.
    pub async fn confirm(self: &Arc<Self>, pending_id: &str, approve: bool) -> ComputerResult<HandleResult> {
        let (approval_id, mut rx) = {
            let g = self.pending.lock();
            let p = g.get(pending_id).ok_or_else(|| ComputerError::NotFound(format!("pending run {pending_id}")))?;
            (p.approval_id, p.result.subscribe())
        };
        match approval_id {
            Some(aid) => {
                let decision = if approve { ApprovalDecision::Approve } else { ApprovalDecision::Reject };
                let user = self.deps.security.user();
                match self.deps.security.approvals.resolve(aid, decision, None, user).await {
                    Ok(_) | Err(SecurityError::Conflict(_)) => {}
                    Err(e) => return Err(ComputerError::Backend(format!("approval: {e}"))),
                }
            }
            None => {
                let me = Arc::clone(self);
                let id = pending_id.to_owned();
                tokio::spawn(async move { me.finish_pending(&id, approve).await });
            }
        }
        let wait = async {
            loop {
                if let Some(r) = rx.borrow().clone() {
                    return Ok(r);
                }
                if rx.changed().await.is_err() {
                    return rx.borrow().clone().ok_or(ComputerError::Stopped);
                }
            }
        };
        tokio::time::timeout(Duration::from_secs(15 * 60), wait).await.map_err(|_| ComputerError::Stopped)?
    }

    // ── plans ────────────────────────────────────────────────────────────

    /// Asks the model for a plan (or a corrected plan).
    ///
    /// # Errors
    /// `Planner` (no model, bad answer), `NotFound` (plan id).
    pub async fn plan(
        self: &Arc<Self>,
        text: &str,
        plan_id: Option<&str>,
        correction: Option<&str>,
        lang: &str,
    ) -> ComputerResult<PlanProposal> {
        let model = self
            .model
            .read()
            .clone()
            .ok_or_else(|| ComputerError::Planner("no AI model is configured (connect a provider)".into()))?;
        let (request, mut corrections, previous) = match plan_id {
            Some(id) => {
                let p =
                    self.plans.lock().get(id).cloned().ok_or_else(|| ComputerError::NotFound(format!("plan {id}")))?;
                (p.request, p.corrections, Some(p.steps))
            }
            None => (text.trim().to_owned(), Vec::new(), None),
        };
        if request.is_empty() {
            return Err(ComputerError::Invalid("empty request".into()));
        }
        if let Some(c) = correction.map(str::trim).filter(|c| !c.is_empty()) {
            corrections.push(c.chars().take(1000).collect());
        }
        let active = self.user_window().await.map(|w| format!("{} ({})", w.title, w.process));
        let user = user_prompt(&request, &corrections, previous.as_deref(), active.as_deref(), lang);
        let (answer, model_label) = model.complete(system_prompt(), user, self.stop_token().child_token()).await?;
        let (steps, explanation) = parse_plan(&answer)?;
        let risks = safety::classify(&steps, Some(&self.deps.security.guard))?;
        let plan = PlanProposal {
            plan_id: plan_id.map_or_else(|| new_id("plan"), str::to_owned),
            request: request.clone(),
            corrections,
            safety: safety::effective(SafetyLevel::Safe, &risks),
            risks,
            steps,
            explanation,
            model: model_label,
            created_at: now(),
        };
        self.plans.lock().insert(plan.plan_id.clone(), plan.clone());
        self.record(&request, None, &request, "Proposed");
        self.publish(&plan.plan_id, None, &request, "Proposed", None).await;
        Ok(plan)
    }

    /// Runs an approved plan (risky steps still ask).
    ///
    /// # Errors
    /// `NotFound`, run errors.
    pub async fn approve_plan(self: &Arc<Self>, plan_id: &str) -> ComputerResult<HandleResult> {
        let p = self
            .plans
            .lock()
            .get(plan_id)
            .cloned()
            .ok_or_else(|| ComputerError::NotFound(format!("plan {plan_id}")))?;
        *self.last_plan.lock() = Some(plan_id.to_owned());
        let spec = RunSpec {
            title: p.request.clone(),
            steps: p.steps.clone(),
            text: p.request.clone(),
            source: "plan".into(),
            ..RunSpec::default()
        };
        self.run_spec(spec, false).await
    }

    /// Saves a plan as a user command.
    ///
    /// # Errors
    /// `NotFound`, `Invalid`, IO.
    pub fn remember_plan(
        &self,
        plan_id: &str,
        phrases: Vec<String>,
        title: Option<String>,
        set_id: Option<String>,
    ) -> ComputerResult<CommandResult> {
        let p = self
            .plans
            .lock()
            .get(plan_id)
            .cloned()
            .ok_or_else(|| ComputerError::NotFound(format!("plan {plan_id}")))?;
        let mut phrases: Vec<String> =
            phrases.into_iter().map(|s| s.trim().to_owned()).filter(|s| !s.is_empty()).collect();
        if phrases.is_empty() {
            phrases.push(p.request.clone());
        }
        phrases.dedup();
        let title = title.filter(|t| !t.trim().is_empty()).unwrap_or_else(|| phrases[0].clone());
        let command = AssistantCommand {
            id: format!("user.{}", &uuid::Uuid::now_v7().simple().to_string()[20..]),
            category: "custom".into(),
            title: LocalizedText { en: title.clone(), ru: title },
            phrases,
            steps: p.steps.clone(),
            safety: p.safety,
            context: None,
        };
        crate::catalogue::validate_command(&command)?;
        let set_id = set_id.unwrap_or_else(|| MY_SET.to_owned());
        let saved = {
            let mut sets = self.sets.write();
            let pos = sets.iter().position(|s| s.id == set_id);
            let set = match pos {
                Some(i) => &mut sets[i],
                None => {
                    sets.push(CommandSet {
                        id: set_id.clone(),
                        name: if set_id == MY_SET { "My commands".into() } else { set_id.clone() },
                        description: String::new(),
                        enabled: true,
                        builtin: false,
                        context: None,
                        commands: Vec::new(),
                        updated_at: None,
                    });
                    sets.last_mut().ok_or_else(|| ComputerError::Invalid("set".into()))?
                }
            };
            if set.builtin {
                return Err(ComputerError::Forbidden("built-in sets are read-only".into()));
            }
            set.commands.push(command.clone());
            set.updated_at = Some(now());
            set.clone()
        };
        self.store.save(&saved)?;
        self.rebuild_index();
        Ok(CommandResult { set_id: saved.id, command })
    }

    // ── catalogue & sets ─────────────────────────────────────────────────

    /// Catalogue page.
    #[must_use]
    pub fn catalogue(&self, p: &CatalogueParams) -> CatalogueResult {
        let sets = self.sets.read();
        let q = p.query.trim().to_lowercase();
        let qk = crate::apps::name_key(&q);
        let mut cats: BTreeMap<String, u32> = BTreeMap::new();
        let mut rows = Vec::new();
        for s in sets.iter() {
            if p.set_id.as_deref().is_some_and(|id| id != s.id) {
                continue;
            }
            for c in &s.commands {
                *cats.entry(c.category.clone()).or_default() += 1;
                if p.category.as_deref().is_some_and(|cat| cat != c.category) {
                    continue;
                }
                if !q.is_empty() {
                    let hay = format!("{} {} {}", c.title.en, c.title.ru, c.phrases.join(" | ")).to_lowercase();
                    let keys = crate::apps::name_key(&hay);
                    if !hay.contains(&q) && !(qk.split(' ').all(|w| !w.is_empty() && keys.contains(w))) {
                        continue;
                    }
                }
                rows.push(summary(s, c, &p.lang));
            }
        }
        let total = u32::try_from(rows.len()).unwrap_or(u32::MAX);
        let limit = p.limit.unwrap_or(50).clamp(1, 500) as usize;
        let commands = rows.into_iter().skip(p.offset as usize).take(limit).collect();
        CatalogueResult {
            commands,
            total,
            categories: cats.into_iter().map(|(category, count)| CategoryCount { category, count }).collect(),
        }
    }

    /// Sets.
    #[must_use]
    pub fn list_sets(&self) -> Vec<CommandSetSummary> {
        self.sets
            .read()
            .iter()
            .map(|s| CommandSetSummary {
                id: s.id.clone(),
                name: s.name.clone(),
                description: s.description.clone(),
                enabled: s.enabled,
                builtin: s.builtin,
                context: s.context.clone(),
                command_count: u32::try_from(s.commands.len()).unwrap_or(0),
                updated_at: s.updated_at,
            })
            .collect()
    }

    /// One set.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn get_set(&self, id: &str) -> ComputerResult<CommandSet> {
        self.sets
            .read()
            .iter()
            .find(|s| s.id == id)
            .cloned()
            .ok_or_else(|| ComputerError::NotFound(format!("set {id}")))
    }

    /// Creates / replaces a user set.
    ///
    /// # Errors
    /// `Forbidden` for built-in ids, `Invalid`, IO.
    pub fn save_set(&self, mut set: CommandSet) -> ComputerResult<CommandSet> {
        if self.sets.read().iter().any(|s| s.id == set.id && s.builtin) {
            return Err(ComputerError::Forbidden("built-in sets are read-only (copy it to a new set)".into()));
        }
        set.builtin = false;
        set.updated_at = Some(now());
        self.store.save(&set)?;
        {
            let mut sets = self.sets.write();
            match sets.iter_mut().find(|s| s.id == set.id) {
                Some(s) => *s = set.clone(),
                None => sets.push(set.clone()),
            }
        }
        self.rebuild_index();
        Ok(set)
    }

    /// Deletes a user set.
    ///
    /// # Errors
    /// `Forbidden`, `NotFound`, IO.
    pub fn delete_set(&self, id: &str) -> ComputerResult<()> {
        let s = self.get_set(id)?;
        if s.builtin {
            return Err(ComputerError::Forbidden("built-in sets cannot be deleted (disable them instead)".into()));
        }
        self.store.delete(id)?;
        self.sets.write().retain(|s| s.id != id);
        self.rebuild_index();
        Ok(())
    }

    /// Enables / disables a set.
    ///
    /// # Errors
    /// `NotFound`, IO.
    pub fn set_enabled(&self, id: &str, enabled: bool) -> ComputerResult<CommandSet> {
        let mut s = self.get_set(id)?;
        s.enabled = enabled;
        if s.builtin {
            let mut st = self.store.state();
            st.disabled_builtin.retain(|x| x != id);
            if !enabled {
                st.disabled_builtin.push(id.to_owned());
            }
            self.store.save_state(&st)?;
        } else {
            self.store.save(&s)?;
        }
        if let Some(x) = self.sets.write().iter_mut().find(|x| x.id == id) {
            x.enabled = enabled;
        }
        self.rebuild_index();
        Ok(s)
    }

    /// Export document.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn export_set(&self, id: &str) -> ComputerResult<ExportSetResult> {
        let s = self.get_set(id)?;
        Ok(ExportSetResult { file_name: format!("{}.orkestr-commands.json", s.id), json: export_document(&s) })
    }

    /// Imports a set.
    ///
    /// # Errors
    /// `Invalid`, IO.
    pub fn import_set(&self, json: &str, replace: bool) -> ComputerResult<CommandSet> {
        let mut set = parse_set_document(json)?;
        let exists = |id: &str| self.sets.read().iter().any(|s| s.id == id);
        if exists(&set.id) && (!replace || self.sets.read().iter().any(|s| s.id == set.id && s.builtin)) {
            let base = set.id.clone();
            let mut n = 2;
            while exists(&format!("{base}-{n}")) {
                n += 1;
            }
            set.id = format!("{base}-{n}");
        }
        self.save_set(set)
    }

    // ── misc ─────────────────────────────────────────────────────────────

    /// Open windows.
    ///
    /// # Errors
    /// Backend.
    pub async fn windows(&self) -> ComputerResult<Vec<WindowInfo>> {
        let b = Arc::clone(&self.deps.backend);
        tokio::task::spawn_blocking(move || b.windows()).await.map_err(|e| ComputerError::Backend(e.to_string()))?
    }

    /// Processes.
    ///
    /// # Errors
    /// Backend.
    pub async fn processes(&self) -> ComputerResult<Vec<ProcessInfo>> {
        let b = Arc::clone(&self.deps.backend);
        tokio::task::spawn_blocking(move || b.processes()).await.map_err(|e| ComputerError::Backend(e.to_string()))?
    }

    /// Global stop.
    pub async fn stop(&self) -> StopResult {
        let runs = self.running.load(Ordering::Relaxed);
        let timers: Vec<CancellationToken> = std::mem::take(&mut *self.timers.lock());
        let timers_cancelled = timers.iter().filter(|t| !t.is_cancelled()).count();
        {
            let mut g = self.stop.lock();
            g.cancel();
            *g = CancellationToken::new();
        }
        for t in timers {
            t.cancel();
        }
        let pend: Vec<(String, Pending)> = self.pending.lock().drain().collect();
        for (_, p) in &pend {
            if let Some(a) = p.approval_id {
                let _ = self.deps.security.approvals.cancel(a, "computer assistant stopped").await;
            }
            let _ = p
                .result
                .send(Some(HandleResult::NotUnderstood { suggestions: Vec::new(), reason: Some("stopped".into()) }));
        }
        self.deps
            .security
            .audit
            .record(NewAudit::new(self.deps.security.user(), "computer.stop", "computer", AuditOutcome::Executed))
            .await;
        StopResult {
            runs_stopped: runs,
            pending_cancelled: u32::try_from(pend.len()).unwrap_or(0),
            timers_cancelled: u32::try_from(timers_cancelled).unwrap_or(0),
        }
    }

    /// Recent history (newest first).
    #[must_use]
    pub fn history(&self, limit: usize) -> Vec<HistoryEntry> {
        self.history.lock().iter().rev().take(limit.clamp(1, HISTORY_MAX)).cloned().collect()
    }

    fn record(&self, text: &str, command_id: Option<String>, title: &str, outcome: &str) {
        let mut h = self.history.lock();
        h.push_back(HistoryEntry {
            at: now(),
            text: text.chars().take(200).collect(),
            command_id,
            title: title.chars().take(200).collect(),
            outcome: outcome.into(),
        });
        while h.len() > HISTORY_MAX {
            h.pop_front();
        }
    }

    async fn audit(&self, spec: &RunSpec, outcome: AuditOutcome, note: &str) {
        let s = &self.deps.security;
        let steps: Vec<serde_json::Value> = spec.steps.iter().map(audit_step).collect();
        s.audit
            .record(
                NewAudit::new(
                    s.user(),
                    "computer.run",
                    spec.command_id.clone().unwrap_or_else(|| "adhoc".into()),
                    outcome,
                )
                .capability(Capability::ComputerControl)
                .details(serde_json::json!({
                    "title": spec.title,
                    "source": spec.source,
                    "note": note,
                    "steps": steps,
                })),
            )
            .await;
    }

    async fn publish(&self, id: &str, command_id: Option<String>, title: &str, outcome: &str, message: Option<String>) {
        let ev = NewEvent::new(EventKind::ComputerCommand(ComputerCommandPayload {
            id: id.to_owned(),
            command_id,
            title: title.chars().take(200).collect(),
            outcome: outcome.to_owned(),
            message,
        }));
        if let Err(e) = self.deps.events.publish(ev).await {
            tracing::warn!(error = %e, "could not publish ComputerCommand");
        }
    }

    fn start_timer(self: &Arc<Self>, secs: u32, message: String) {
        let token = self.stop_token().child_token();
        self.timers.lock().push(token.clone());
        let me = Arc::clone(self);
        tokio::spawn(async move {
            tokio::select! {
                () = tokio::time::sleep(Duration::from_secs(u64::from(secs))) => {}
                () = token.cancelled() => return,
            }
            let body = if message.is_empty() { format!("{secs} s") } else { message.clone() };
            let b = Arc::clone(&me.deps.backend);
            let b2 = body.clone();
            let _ = tokio::task::spawn_blocking(move || b.notify("Orkestr", &b2)).await;
            me.publish(&new_id("timer"), None, &body, "TimerFired", Some(body.clone())).await;
            token.cancel();
            me.timers.lock().retain(|t| !t.is_cancelled());
        });
    }
}

/// Recognizes "remember this as …" / "запомни это как …".
#[must_use]
pub fn remember_phrase(text: &str) -> Option<String> {
    let lower = text.trim().to_lowercase();
    for p in [
        "запомни это как ",
        "запомни как ",
        "запомни команду как ",
        "сохрани это как ",
        "сохрани как команду ",
        "remember this as ",
        "remember it as ",
        "save this as ",
        "save this command as ",
    ] {
        if let Some(i) = lower.find(p) {
            if i == 0 {
                let start = text.trim().char_indices().nth(p.chars().count()).map_or(text.len(), |(b, _)| b);
                let rest = text.trim()[start..].trim().trim_matches(|c| matches!(c, '"' | '«' | '»' | '“' | '”' | '.'));
                if !rest.is_empty() {
                    return Some(rest.to_owned());
                }
            }
        }
    }
    None
}

fn title_of(c: &AssistantCommand, lang: &str) -> String {
    let t = c.title.get(lang);
    if t.is_empty() {
        c.phrases.first().cloned().unwrap_or_else(|| c.id.clone())
    } else {
        t.to_owned()
    }
}

fn declared_level(c: &AssistantCommand) -> SafetyLevel {
    // static view without resolving slots: declared or obvious step levels
    let reasons = safety::classify(&c.steps, None).unwrap_or_default();
    safety::effective(c.safety, &reasons)
}

fn summary(s: &CommandSet, c: &AssistantCommand, lang: &str) -> CommandSummary {
    let ru = lang.starts_with("ru");
    let cyr = |p: &String| p.chars().any(|ch| ('\u{0400}'..='\u{04FF}').contains(&ch));
    let mut phrases: Vec<String> = c.phrases.iter().filter(|p| cyr(p) == ru).take(3).cloned().collect();
    if phrases.is_empty() {
        phrases = c.phrases.iter().take(3).cloned().collect();
    }
    CommandSummary {
        set_id: s.id.clone(),
        command_id: c.id.clone(),
        category: c.category.clone(),
        title: title_of(c, lang),
        phrases,
        safety: declared_level(c),
        contextual: c.context.is_some() || s.context.is_some(),
    }
}

/// Audit view of a step: type + target, never typed text / clipboard content.
fn audit_step(s: &ComputerAction) -> serde_json::Value {
    use ComputerAction as A;
    let target = match s {
        A::Launch { target } => target.clone(),
        A::OpenUrl { url } => url.clone(),
        A::OpenSettings { page } => page.clone(),
        A::OpenPath { path } | A::FileCreate { path, .. } | A::FolderCreate { path } | A::FileDelete { path } => {
            path.clone()
        }
        A::FileMove { from, to } => format!("{from} -> {to}"),
        A::CloseApp { app, discard } => format!("{app}{}", if *discard { " (discard)" } else { "" }),
        A::KillProcess { name } => name.clone(),
        A::FocusApp { app } => app.clone(),
        A::Keys { combo, .. } => combo.clone(),
        A::ClickElement { name } => name.clone(),
        A::System { op } => format!("{op:?}"),
        A::TypeText { text } => format!("{} chars", text.chars().count()),
        _ => String::new(),
    };
    serde_json::json!({ "type": s.type_name(), "target": target })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remember_phrases() {
        assert_eq!(remember_phrase("Запомни это как «список покупок»").as_deref(), Some("список покупок"));
        assert_eq!(remember_phrase("remember this as morning routine").as_deref(), Some("morning routine"));
        assert_eq!(remember_phrase("запомни"), None);
        assert_eq!(remember_phrase("открой блокнот"), None);
    }
}
