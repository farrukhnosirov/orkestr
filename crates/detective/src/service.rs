//! [`Detective`]: the incident workflow facade used by `core/src/rpc/detective.rs`.
//!
//! One background run per incident drives the stage machine. Every stage
//! change is persisted, published as `IncidentStageChanged` and audited.
//! Two human gates (`AwaitingFixApproval`, `AwaitingMergeApproval`) are
//! backed by approval requests, so they can be decided either through
//! `detective.approveFix` / `detective.approveMerge` or in the approvals
//! inbox. `main` is never written before gate 2. Cancel works at every stage
//! and stops child agents and test runs before it returns.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{
    now, Actor, AgentId, ApprovalId, ApprovalStatus, AuditOutcome, Capability, IncidentId, RiskLevel,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, NewEvent};
use orkestr_protocol::methods::detective::{
    Evidence, Hypothesis, Incident, IncidentOpenedPayload, IncidentStage, IncidentStageChangedPayload, IncidentTrigger,
    ListIncidentsParams, OpenIncidentParams,
};
use orkestr_security::{ApprovalDecision, ApprovalRequest, NewAudit, Security};
use orkestr_storage::Db;
use parking_lot::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::collectors::{
    cap, AlertCollector, CodeSnippetCollector, DeploymentsCollector, LogsCollector, MetricsCollector, QualityCollector,
    VcsCollector,
};
use crate::evidence::{EvidenceBudget, EvidenceCollector};
use crate::ports::{FixRequest, Ports, ReviewRequest, TestRequest};
use crate::store::{DetectiveSettings, IncidentStore};
use crate::{hypothesis, stage, DetectiveError};

use IncidentStage as S;

/// Services the detective needs. Agent runtime, providers, quality, reviews,
/// VCS, monitor and deploy are injected later as [`Ports`] through
/// [`Detective::install`] (they are built after this service in core).
#[derive(Clone)]
pub struct DetectiveDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Approvals, audit.
    pub security: Arc<Security>,
    /// `<data_dir>/detective`.
    pub data_dir: PathBuf,
}

/// Total evidence kept per incident (bytes).
const MAX_EVIDENCE_BYTES: usize = 160 * 1024;
/// Total evidence items per incident.
const MAX_EVIDENCE_ITEMS: usize = 48;
/// Prompt budget for the hypothesis (bytes).
const PROMPT_BYTES: usize = 64 * 1024;
/// Gate approvals stay pending this long.
const GATE_TTL: Duration = Duration::from_secs(7 * 24 * 3600);
/// Per-collector deadline.
const COLLECT_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `cancel` waits for children to stop.
const STOP_WAIT: Duration = Duration::from_secs(15);
/// Largest budget accepted (USD).
const MAX_BUDGET_USD: f64 = 1_000.0;

#[derive(Default)]
struct Live {
    cancel: CancellationToken,
    agent: Option<AgentId>,
    handles: Vec<JoinHandle<()>>,
}

struct Inner {
    deps: DetectiveDeps,
    store: IncidentStore,
    ports: RwLock<Option<Ports>>,
    live: Mutex<HashMap<IncidentId, Live>>,
    merging: Mutex<HashSet<IncidentId>>,
    gate: tokio::sync::Mutex<()>,
    shutdown: CancellationToken,
    budget: RwLock<EvidenceBudget>,
    listener: Mutex<Option<JoinHandle<()>>>,
}

/// Bug Detective.
#[derive(Clone)]
pub struct Detective {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Detective {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Detective").field("data_dir", &self.inner.deps.data_dir).finish_non_exhaustive()
    }
}

fn fix_branch(id: IncidentId) -> String {
    format!("detective/{id}")
}

fn first_line(s: &str, max: usize) -> String {
    let line = s.lines().find(|l| !l.trim().is_empty()).unwrap_or("").trim();
    cap(line, max)
}

impl Detective {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: DetectiveDeps) -> Self {
        let store = IncidentStore::new(deps.db.clone());
        Self {
            inner: Arc::new(Inner {
                deps,
                store,
                ports: RwLock::new(None),
                live: Mutex::new(HashMap::new()),
                merging: Mutex::new(HashSet::new()),
                gate: tokio::sync::Mutex::new(()),
                shutdown: CancellationToken::new(),
                budget: RwLock::new(EvidenceBudget::default()),
                listener: Mutex::new(None),
            }),
        }
    }

    /// Installs the service seams (once; later calls replace them), recovers
    /// incidents interrupted by a restart and starts the `AlertRaised`
    /// listener (auto-open honours [`DetectiveSettings::auto_open_on_alert`]).
    pub async fn install(&self, ports: Ports) {
        *self.inner.ports.write() = Some(ports);
        if let Err(e) = self.recover().await {
            tracing::warn!(error = %e, "detective: recovery failed");
        }
        let mut l = self.inner.listener.lock();
        if l.is_none() {
            *l = Some(self.spawn_alert_listener());
        }
    }

    /// Whether [`Detective::install`] ran.
    #[must_use]
    pub fn is_installed(&self) -> bool {
        self.inner.ports.read().is_some()
    }

    /// Overrides the per-collector evidence budget.
    pub fn set_evidence_budget(&self, budget: EvidenceBudget) {
        *self.inner.budget.write() = budget;
    }

    /// Settings.
    pub async fn settings(&self) -> Result<DetectiveSettings, DetectiveError> {
        self.inner.store.settings().await
    }

    /// Saves settings.
    pub async fn set_settings(&self, s: DetectiveSettings) -> Result<DetectiveSettings, DetectiveError> {
        if !(s.default_budget_usd > 0.0 && s.default_budget_usd <= MAX_BUDGET_USD) {
            return Err(DetectiveError::Invalid(format!("default budget must be within (0, {MAX_BUDGET_USD}] USD")));
        }
        if !(1..=5).contains(&s.max_fix_attempts) {
            return Err(DetectiveError::Invalid("max fix attempts must be within 1..5".into()));
        }
        self.inner.store.set_settings(&s).await?;
        Ok(s)
    }

    fn ports(&self) -> Result<Ports, DetectiveError> {
        self.inner.ports.read().clone().ok_or(DetectiveError::Unimplemented("detective (services not wired)"))
    }

    // ------------------------------------------------------------------
    // Queries
    // ------------------------------------------------------------------

    /// `detective.get`.
    pub async fn get(&self, id: IncidentId) -> Result<Incident, DetectiveError> {
        self.inner.store.get(id).await?.ok_or_else(|| DetectiveError::NotFound(format!("incident {id}")))
    }

    /// `detective.list`.
    pub async fn list(&self, p: &ListIncidentsParams) -> Result<Vec<Incident>, DetectiveError> {
        self.inner.store.list(p.project_id, p.include_closed, p.limit.unwrap_or(50)).await
    }

    /// Incidents with a live background run or child (tests, diagnostics).
    #[must_use]
    pub fn live_runs(&self) -> usize {
        self.inner.live.lock().values().filter(|l| l.handles.iter().any(|h| !h.is_finished())).count()
    }

    // ------------------------------------------------------------------
    // Commands
    // ------------------------------------------------------------------

    /// `detective.open`: creates the incident and starts the investigation.
    pub async fn open(&self, p: OpenIncidentParams, by: Actor) -> Result<Incident, DetectiveError> {
        let ports = self.ports()?;
        let settings = self.settings().await?;
        let budget = p.budget_usd.unwrap_or(settings.default_budget_usd);
        if !(budget > 0.0 && budget <= MAX_BUDGET_USD) {
            return Err(DetectiveError::Invalid(format!("budgetUsd must be within (0, {MAX_BUDGET_USD}]")));
        }
        let derived = match &p.trigger {
            IncidentTrigger::Alert { alert_id } => match &ports.monitor {
                Some(m) => {
                    let a = m
                        .alert(*alert_id)
                        .await?
                        .ok_or_else(|| DetectiveError::NotFound(format!("alert {alert_id}")))?;
                    if a.project_id.is_some_and(|ap| ap != p.project_id) {
                        return Err(DetectiveError::Invalid("the alert belongs to another project".into()));
                    }
                    a.title
                }
                None => format!("Alert {alert_id}"),
            },
            IncidentTrigger::LogError { source_id, line } => {
                if source_id.trim().is_empty() || line.trim().is_empty() {
                    return Err(DetectiveError::Invalid("sourceId and line are required".into()));
                }
                first_line(line, 120)
            }
            IncidentTrigger::TestFailure { run_id } => {
                if run_id.trim().is_empty() {
                    return Err(DetectiveError::Invalid("runId is required".into()));
                }
                format!("Test failure in run {}", cap(run_id, 40))
            }
            IncidentTrigger::Manual { description } => {
                if description.trim().is_empty() {
                    return Err(DetectiveError::Invalid("description is required".into()));
                }
                first_line(description, 120)
            }
        };
        // Cap and redact free text from the trigger.
        let trigger = match p.trigger {
            IncidentTrigger::LogError { source_id, line } => IncidentTrigger::LogError {
                source_id: cap(&source_id, 200),
                line: orkestr_security::redact::redact(&cap(&line, 20_000)),
            },
            IncidentTrigger::Manual { description } => {
                IncidentTrigger::Manual { description: orkestr_security::redact::redact(&cap(&description, 20_000)) }
            }
            other => other,
        };
        ports.projects.root(p.project_id).await?;
        let title = p.title.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty()).unwrap_or(derived);
        let ts = now();
        let inc = Incident {
            id: IncidentId::new(),
            project_id: p.project_id,
            title: orkestr_security::redact::redact(&cap(&title, 200)),
            trigger,
            stage: S::Opened,
            evidence: Vec::new(),
            hypothesis: None,
            reproduction: None,
            fix_branch: None,
            agent_id: None,
            fix_tests: None,
            review_id: None,
            approval_id: None,
            budget_usd: budget,
            spent_usd: 0.0,
            error: None,
            created_at: ts,
            updated_at: ts,
        };
        self.inner.store.put(&inc).await?;
        self.publish(
            NewEvent::new(EventKind::IncidentOpened(IncidentOpenedPayload { incident: inc.clone() }))
                .project(inc.project_id),
        )
        .await;
        self.audit(by, "detective.open", &inc, AuditOutcome::Executed, serde_json::json!({ "budgetUsd": budget }))
            .await;
        self.inner.live.lock().insert(inc.id, Live { cancel: self.inner.shutdown.child_token(), ..Live::default() });
        let me = self.clone();
        let id = inc.id;
        self.spawn(id, async move { me.drive(id, me.investigate(id)).await });
        Ok(inc)
    }

    /// `detective.approveFix` (gate 1).
    pub async fn approve_fix(
        &self,
        id: IncidentId,
        note: Option<String>,
        by: Actor,
    ) -> Result<Incident, DetectiveError> {
        self.ports()?;
        let approval = self.get(id).await?.approval_id;
        let inc = self.start_fix(id, by, note.clone()).await?;
        if let Some(a) = approval {
            self.resolve_approval(a, by, note).await;
        }
        Ok(inc)
    }

    /// `detective.approveMerge` (gate 2): merges `detective/<id>` into the
    /// project's checked-out branch, then `Resolved`.
    pub async fn approve_merge(
        &self,
        id: IncidentId,
        note: Option<String>,
        by: Actor,
    ) -> Result<Incident, DetectiveError> {
        let inc = self.get(id).await?;
        if inc.stage != S::AwaitingMergeApproval {
            return Err(DetectiveError::Conflict(format!("incident is {:?}, not AwaitingMergeApproval", inc.stage)));
        }
        // Claim the merge before deciding the approval so the inbox watcher
        // (woken by that decision) does not race this call.
        if !self.inner.merging.lock().insert(id) {
            return Err(DetectiveError::Conflict("the merge is already running".into()));
        }
        if let Some(a) = inc.approval_id {
            self.resolve_approval(a, by, note.clone()).await;
        }
        let r = self.merge_inner(id, by, note).await;
        self.inner.merging.lock().remove(&id);
        r
    }

    /// `detective.cancel` at any non-terminal stage. Stops the child agent,
    /// test runs and pending approvals before it returns.
    pub async fn cancel(&self, id: IncidentId, note: Option<String>, by: Actor) -> Result<Incident, DetectiveError> {
        let reason = note.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "cancelled by the user".into());
        let inc = self.transition(id, None, S::Cancelled, Some(reason.clone()), by, |_| {}).await?;
        self.stop_children(id, &reason).await;
        Ok(inc)
    }

    /// Cancels every non-terminal incident (emergency stop). Returns how many.
    pub async fn cancel_all(&self) -> usize {
        let open = match self.inner.store.list(None, false, 500).await {
            Ok(v) => v,
            Err(e) => {
                tracing::error!(error = %e, "detective: cancel_all could not list incidents");
                Vec::new()
            }
        };
        let mut n = 0;
        for inc in open {
            if self.cancel(inc.id, Some("emergency stop".into()), Actor::System).await.is_ok() {
                n += 1;
            }
        }
        // Runs without a row (should not happen) are stopped too.
        let ids: Vec<IncidentId> = self.inner.live.lock().keys().copied().collect();
        for id in ids {
            self.stop_children(id, "emergency stop").await;
        }
        n
    }

    /// Stops background work (daemon shutdown).
    pub async fn shutdown(&self) {
        self.inner.shutdown.cancel();
        if let Some(h) = self.inner.listener.lock().take() {
            h.abort();
        }
    }

    // ------------------------------------------------------------------
    // Stage machine plumbing
    // ------------------------------------------------------------------

    async fn publish(&self, ev: NewEvent) {
        if let Err(e) = self.inner.deps.events.publish(ev).await {
            tracing::warn!(error = %e, "detective: event publish failed");
        }
    }

    async fn audit(&self, by: Actor, action: &str, inc: &Incident, outcome: AuditOutcome, details: serde_json::Value) {
        let mut a = NewAudit::new(by, action, format!("incident:{}", inc.id), outcome);
        a.project_id = Some(inc.project_id);
        a.details = details;
        self.inner.deps.security.audit.record(a).await;
    }

    /// Moves `id` to `to` (when `expect` is set, only from that stage).
    async fn transition(
        &self,
        id: IncidentId,
        expect: Option<IncidentStage>,
        to: IncidentStage,
        message: Option<String>,
        by: Actor,
        mutate: impl FnOnce(&mut Incident),
    ) -> Result<Incident, DetectiveError> {
        let guard = self.inner.gate.lock().await;
        let mut inc = self.get(id).await?;
        let from = inc.stage;
        if let Some(e) = expect {
            if from != e {
                return Err(if from == S::Cancelled {
                    DetectiveError::Cancelled
                } else {
                    DetectiveError::Conflict(format!("incident is {from:?}, expected {e:?}"))
                });
            }
        }
        if !stage::can_transition(from, to) {
            return Err(DetectiveError::Conflict(format!("cannot move an incident from {from:?} to {to:?}")));
        }
        mutate(&mut inc);
        inc.stage = to;
        inc.updated_at = now();
        if matches!(to, S::Failed | S::Cancelled) {
            inc.error = message.clone();
        }
        if !stage::is_gate(to) && stage::is_gate(from) {
            inc.approval_id = None;
        }
        self.inner.store.put(&inc).await?;
        drop(guard);
        tracing::info!(incident = %id, ?from, ?to, "incident stage changed");
        self.publish(
            NewEvent::new(EventKind::IncidentStageChanged(IncidentStageChangedPayload {
                incident_id: id,
                project_id: inc.project_id,
                from,
                to,
                message: message.clone(),
            }))
            .project(inc.project_id),
        )
        .await;
        let outcome = if to == S::Failed { AuditOutcome::Failed } else { AuditOutcome::Executed };
        self.audit(
            by,
            "detective.stage",
            &inc,
            outcome,
            serde_json::json!({ "from": stage::name(from), "to": stage::name(to), "message": message }),
        )
        .await;
        if to.is_terminal() && to != S::Cancelled {
            // Cancel is handled by `stop_children` (it waits for them).
            if let Some(l) = self.inner.live.lock().remove(&id) {
                l.cancel.cancel();
            }
        }
        Ok(inc)
    }

    /// Updates fields of a non-terminal incident.
    async fn update(&self, id: IncidentId, mutate: impl FnOnce(&mut Incident)) -> Result<Incident, DetectiveError> {
        let _g = self.inner.gate.lock().await;
        let mut inc = self.get(id).await?;
        if inc.stage.is_terminal() {
            return Err(DetectiveError::Cancelled);
        }
        mutate(&mut inc);
        inc.updated_at = now();
        self.inner.store.put(&inc).await?;
        Ok(inc)
    }

    /// Adds spend; fails with [`DetectiveError::Budget`] past the cap.
    async fn charge(&self, id: IncidentId, usd: f64) -> Result<Incident, DetectiveError> {
        let inc = self.update(id, |i| i.spent_usd += usd.max(0.0)).await?;
        if inc.spent_usd > inc.budget_usd {
            return Err(DetectiveError::Budget(format!(
                "spent ${:.4} of the ${:.2} budget",
                inc.spent_usd, inc.budget_usd
            )));
        }
        Ok(inc)
    }

    fn check_budget(inc: &Incident) -> Result<f64, DetectiveError> {
        let left = inc.budget_usd - inc.spent_usd;
        if left <= 0.0 {
            return Err(DetectiveError::Budget(format!(
                "spent ${:.4} of the ${:.2} budget",
                inc.spent_usd, inc.budget_usd
            )));
        }
        Ok(left)
    }

    fn token(&self, id: IncidentId) -> Result<CancellationToken, DetectiveError> {
        self.inner.live.lock().get(&id).map(|l| l.cancel.clone()).ok_or(DetectiveError::Cancelled)
    }

    fn spawn(&self, id: IncidentId, fut: impl std::future::Future<Output = ()> + Send + 'static) {
        let h = tokio::spawn(fut);
        let mut live = self.inner.live.lock();
        match live.get_mut(&id) {
            Some(l) => {
                l.handles.retain(|h| !h.is_finished());
                l.handles.push(h);
            }
            None => h.abort(),
        }
    }

    /// Runs a pipeline step; errors other than cancellation fail the incident.
    async fn drive(&self, id: IncidentId, fut: impl std::future::Future<Output = Result<(), DetectiveError>>) {
        match fut.await {
            Ok(()) | Err(DetectiveError::Cancelled) => {}
            Err(e) => {
                let reason = match &e {
                    DetectiveError::Budget(m) => format!("budget cap reached: {m}"),
                    other => other.to_string(),
                };
                tracing::warn!(incident = %id, error = %reason, "incident failed");
                if let Err(e2) = self.transition(id, None, S::Failed, Some(reason), Actor::System, |_| {}).await {
                    tracing::debug!(incident = %id, error = %e2, "could not mark the incident failed");
                }
            }
        }
    }

    async fn stop_children(&self, id: IncidentId, reason: &str) {
        let live = self.inner.live.lock().remove(&id);
        let Some(live) = live else { return };
        live.cancel.cancel();
        if let (Some(agent), Ok(ports)) = (live.agent, self.ports()) {
            ports.agent.stop(agent).await;
        }
        if let Ok(Some(inc)) = self.inner.store.get(id).await {
            if let Some(a) = inc.approval_id {
                let _ = self.inner.deps.security.approvals.cancel(a, reason).await;
            }
        }
        for h in live.handles {
            if tokio::time::timeout(STOP_WAIT, h).await.is_err() {
                tracing::warn!(incident = %id, "a detective task did not stop in time");
            }
        }
    }

    async fn resolve_approval(&self, id: ApprovalId, by: Actor, note: Option<String>) {
        let by = match by {
            Actor::User(_) => by,
            _ => self.inner.deps.security.user(),
        };
        if let Err(e) = self.inner.deps.security.approvals.resolve(id, ApprovalDecision::Approve, note, by).await {
            tracing::debug!(approval = %id, error = %e, "gate approval already decided");
        }
    }

    // ------------------------------------------------------------------
    // Investigation: Collecting → Hypothesizing → Reproducing → gate 1
    // ------------------------------------------------------------------

    async fn investigate(&self, id: IncidentId) -> Result<(), DetectiveError> {
        let ports = self.ports()?;
        let cancel = self.token(id)?;
        let inc = self.transition(id, Some(S::Opened), S::Collecting, None, Actor::System, |_| {}).await?;
        let root = ports.projects.root(inc.project_id).await?;

        // Evidence.
        let evidence = self.collect(&ports, &inc, &root, &cancel).await?;
        let n = evidence.len();
        let inc = self
            .transition(
                id,
                Some(S::Collecting),
                S::Hypothesizing,
                Some(format!("{n} evidence items")),
                Actor::System,
                |i| {
                    i.evidence = evidence;
                },
            )
            .await?;

        // Hypothesis.
        Self::check_budget(&inc)?;
        let (budget, start) = (inc.budget_usd, inc.spent_usd);
        let mut spent = 0.0;
        let generated = hypothesis::generate(ports.model.as_ref(), &inc, PROMPT_BYTES, &cancel, |c| {
            spent += c;
            if start + spent > budget {
                Err(DetectiveError::Budget(format!("spent ${:.4} of the ${budget:.2} budget", start + spent)))
            } else {
                Ok(())
            }
        })
        .await;
        self.charge(id, spent).await?;
        let generated = generated?;
        let h = generated.hypothesis;
        let msg = format!("{} (confidence {:.0}%)", cap(&h.summary, 160), h.confidence * 100.0);
        let inc = self
            .transition(id, Some(S::Hypothesizing), S::Reproducing, Some(msg), Actor::System, |i| {
                i.hypothesis = Some(h.clone());
            })
            .await?;

        // Reproduction (expected to fail).
        let run = tokio::select! {
            r = ports.tests.run(
                TestRequest { project_id: inc.project_id, dir: root.clone(), command: h.reproduction.clone() },
                cancel.child_token(),
            ) => r,
            () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
        };
        let (run, msg) = match run {
            Ok(r) if r.passed => (Some(r), "not reproduced: the tests pass on the current code".to_owned()),
            Ok(r) => (Some(r), "reproduced: the tests fail as expected".to_owned()),
            Err(DetectiveError::Cancelled) => return Err(DetectiveError::Cancelled),
            Err(e) => (None, format!("reproduction could not run: {e}")),
        };
        self.update(id, |i| i.reproduction = run).await?;
        self.enter_gate(id, S::Reproducing, S::AwaitingFixApproval, msg).await
    }

    async fn collect(
        &self,
        ports: &Ports,
        inc: &Incident,
        root: &Path,
        cancel: &CancellationToken,
    ) -> Result<Vec<Evidence>, DetectiveError> {
        let budget = *self.inner.budget.read();
        let mut collectors: Vec<Box<dyn EvidenceCollector>> = Vec::new();
        if let Some(m) = &ports.monitor {
            collectors.push(Box::new(AlertCollector(Arc::clone(m))));
            collectors.push(Box::new(LogsCollector(Arc::clone(m))));
            collectors.push(Box::new(MetricsCollector(Arc::clone(m))));
        }
        if let Some(d) = &ports.deployments {
            collectors.push(Box::new(DeploymentsCollector(Arc::clone(d))));
        }
        collectors.push(Box::new(VcsCollector {
            vcs: Arc::clone(&ports.vcs),
            root: root.to_path_buf(),
            deployments: ports.deployments.clone(),
        }));
        if let Some(q) = &ports.quality {
            collectors.push(Box::new(QualityCollector(Arc::clone(q))));
        }
        let runs = collectors.iter().map(|c| async move {
            let kind = c.kind();
            match tokio::time::timeout(COLLECT_TIMEOUT, c.collect(inc, &budget)).await {
                Ok(Ok(v)) => v,
                Ok(Err(e)) => {
                    tracing::warn!(incident = %inc.id, ?kind, error = %e, "collector failed");
                    Vec::new()
                }
                Err(_) => {
                    tracing::warn!(incident = %inc.id, ?kind, "collector timed out");
                    Vec::new()
                }
            }
        });
        let gathered = tokio::select! {
            v = futures::future::join_all(runs) => v,
            () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
        };
        let mut evidence: Vec<Evidence> = gathered.into_iter().flatten().collect();
        // Stack traces + code snippets read what the others found.
        let mut with = inc.clone();
        with.evidence.clone_from(&evidence);
        let snippets = CodeSnippetCollector { root: root.to_path_buf() };
        match snippets.collect(&with, &budget).await {
            Ok(mut v) => {
                // Code first: it is what the hypothesis needs most.
                v.append(&mut evidence);
                evidence = v;
            }
            Err(e) => tracing::warn!(incident = %inc.id, error = %e, "snippet collector failed"),
        }
        let mut total = 0usize;
        let mut out = Vec::new();
        for mut e in evidence {
            if out.len() >= MAX_EVIDENCE_ITEMS || total >= MAX_EVIDENCE_BYTES {
                break;
            }
            e.content = cap(&e.content, MAX_EVIDENCE_BYTES - total);
            total += e.content.len();
            e.id = format!("ev{}", out.len() + 1);
            out.push(e);
        }
        Ok(out)
    }

    /// Enters a gate stage and requests its approval.
    async fn enter_gate(
        &self,
        id: IncidentId,
        from: IncidentStage,
        to: IncidentStage,
        message: String,
    ) -> Result<(), DetectiveError> {
        // The approval exists before the stage is visible, so a gate stage
        // always carries its approval id.
        let inc = self.get(id).await?;
        let approval = self.request_gate(&inc, to).await?;
        match self
            .transition(id, Some(from), to, Some(message), Actor::System, |i| i.approval_id = Some(approval))
            .await
        {
            Ok(_) => {
                self.watch_gate(id, to, approval);
                Ok(())
            }
            Err(e) => {
                let _ = self.inner.deps.security.approvals.cancel(approval, "the incident moved on").await;
                Err(e)
            }
        }
    }

    async fn request_gate(&self, inc: &Incident, gate: IncidentStage) -> Result<ApprovalId, DetectiveError> {
        let branch = fix_branch(inc.id);
        let summary = inc.hypothesis.as_ref().map(|h| h.summary.clone()).unwrap_or_default();
        let mut req = match gate {
            S::AwaitingFixApproval => ApprovalRequest::new(
                Capability::Write,
                format!("Bug Detective: let an agent fix \"{}\" on {branch}", inc.title),
                format!("Hypothesis: {summary}. The fix is written in its own worktree; main is untouched."),
                RiskLevel::Medium,
            ),
            S::AwaitingMergeApproval => ApprovalRequest::new(
                Capability::Git,
                format!("Bug Detective: merge {branch} for \"{}\"", inc.title),
                format!(
                    "Tests {} on the fix branch; review {}. Merging changes the project branch.",
                    if inc.fix_tests.as_ref().is_some_and(|t| t.passed) { "pass" } else { "did not pass" },
                    inc.review_id.as_deref().unwrap_or("not available")
                ),
                RiskLevel::High,
            ),
            other => return Err(DetectiveError::Internal(format!("{other:?} is not a gate"))),
        };
        req.project_id = Some(inc.project_id);
        req.agent_id = inc.agent_id;
        req.ttl = Some(GATE_TTL);
        req.command_preview = Some(format!("git merge --no-ff {branch}"));
        if gate == S::AwaitingFixApproval {
            req.command_preview = None;
        }
        let approval = self
            .inner
            .deps
            .security
            .approvals
            .request(req)
            .await
            .map_err(|e| DetectiveError::Internal(format!("approval request failed: {e}")))?;
        Ok(approval.id)
    }

    /// Follows a gate approval decided in the approvals inbox.
    fn watch_gate(&self, id: IncidentId, gate: IncidentStage, approval: ApprovalId) {
        let Ok(cancel) = self.token(id) else { return };
        let me = self.clone();
        self.spawn(id, async move {
            let status = me.inner.deps.security.approvals.wait(approval, GATE_TTL, &cancel).await;
            if cancel.is_cancelled() {
                return;
            }
            match status {
                Ok(ApprovalStatus::Approved) => {
                    let r = match gate {
                        S::AwaitingFixApproval => me.start_fix(id, Actor::System, None).await.map(|_| ()),
                        _ => me.merge(id, Actor::System, None).await.map(|_| ()),
                    };
                    if let Err(e) = r {
                        tracing::debug!(incident = %id, error = %e, "gate already passed");
                    }
                }
                Ok(ApprovalStatus::Rejected) => {
                    let _ = me.cancel(id, Some("rejected in the approvals inbox".into()), Actor::System).await;
                }
                Ok(ApprovalStatus::Expired) => {
                    let _ = me
                        .transition(
                            id,
                            Some(gate),
                            S::Failed,
                            Some("the approval expired".into()),
                            Actor::System,
                            |_| {},
                        )
                        .await;
                }
                Ok(_) => {}
                Err(e) => tracing::warn!(incident = %id, error = %e, "gate approval wait failed"),
            }
        });
    }

    // ------------------------------------------------------------------
    // Fix: Fixing → Testing → Reviewing → gate 2
    // ------------------------------------------------------------------

    async fn start_fix(&self, id: IncidentId, by: Actor, note: Option<String>) -> Result<Incident, DetectiveError> {
        let msg = note.filter(|n| !n.trim().is_empty()).unwrap_or_else(|| "fix approved".into());
        let inc = self
            .transition(id, Some(S::AwaitingFixApproval), S::Fixing, Some(msg), by, |i| {
                i.fix_branch = Some(fix_branch(i.id));
            })
            .await?;
        let me = self.clone();
        self.spawn(id, async move { me.drive(id, me.fix(id)).await });
        Ok(inc)
    }

    fn fix_prompt(inc: &Incident, h: &Hypothesis, branch: &str, failing: Option<&str>) -> String {
        let mut p = format!(
            "Fix the production incident \"{}\" in this repository. You work in a dedicated worktree on branch `{branch}`; \
             the main branch is not touched.\n\n## Root cause hypothesis (confidence {:.0}%)\n{}\n\n{}\n\n## Proposed fix\n{}\n",
            inc.title,
            h.confidence * 100.0,
            h.summary,
            h.root_cause,
            h.proposed_fix
        );
        if !h.suspected_files.is_empty() {
            p.push_str(&format!("\n## Suspected files\n{}\n", h.suspected_files.join("\n")));
        }
        if let Some(r) = &h.reproduction {
            p.push_str(&format!("\n## Reproduction\n`{r}` currently fails; it must pass after the fix.\n"));
        }
        p.push_str("\n## Evidence\n");
        for e in inc.evidence.iter().filter(|e| h.evidence_refs.contains(&e.id)) {
            p.push_str(&format!("\n### {} — {}\n{}\n", e.id, e.title, cap(&e.content, 4_000)));
        }
        if let Some(out) = failing {
            p.push_str(&format!("\n## The previous attempt's tests still fail\n{}\n", cap(out, 6_000)));
        }
        p.push_str(
            "\n## Rules\n- Make the smallest change that removes the root cause.\n- Add or update a test that \
             reproduces the bug.\n- Do not touch unrelated files.\n- Commit your changes on this branch when done.\n",
        );
        p
    }

    async fn fix(&self, id: IncidentId) -> Result<(), DetectiveError> {
        let ports = self.ports()?;
        let cancel = self.token(id)?;
        let settings = self.settings().await?;
        let inc = self.get(id).await?;
        let h = inc.hypothesis.clone().ok_or_else(|| DetectiveError::Invalid("no hypothesis".into()))?;
        let root = ports.projects.root(inc.project_id).await?;
        let branch = fix_branch(id);
        let base = ports
            .vcs
            .rev_parse(&root, "HEAD")
            .await?
            .ok_or_else(|| DetectiveError::Invalid("the project repository has no commits".into()))?;
        let worktree = match self.inner.store.worktree(id).await? {
            Some(w) if Path::new(&w).is_dir() => PathBuf::from(w),
            _ => {
                let w = ports.vcs.create_worktree(inc.project_id, &root, &branch, &base).await?;
                self.inner.store.set_worktree(id, Some(w.to_string_lossy().into_owned())).await?;
                w
            }
        };
        let mut failing: Option<String> = None;
        let mut attempt = 1;
        loop {
            let inc = self.get(id).await?;
            if inc.stage != S::Fixing {
                return Err(DetectiveError::Cancelled);
            }
            let left = Self::check_budget(&inc)?;
            let req = FixRequest {
                incident_id: id,
                project_id: inc.project_id,
                project_root: root.clone(),
                worktree: worktree.clone(),
                branch: branch.clone(),
                title: format!("Bug Detective fix: {}", inc.title),
                prompt: Self::fix_prompt(&inc, &h, &branch, failing.as_deref()),
                hypothesis: h.clone(),
                budget_usd: left,
                attempt,
            };
            let me = self.clone();
            let on_agent: Arc<dyn Fn(AgentId) + Send + Sync> = Arc::new(move |agent| {
                if let Some(l) = me.inner.live.lock().get_mut(&id) {
                    l.agent = Some(agent);
                }
                let me2 = me.clone();
                tokio::spawn(async move {
                    let _ = me2.update(id, |i| i.agent_id = Some(agent)).await;
                });
            });
            let outcome = tokio::select! {
                r = ports.agent.run(req, on_agent, cancel.child_token()) => r,
                () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
            };
            let outcome = outcome?;
            if let Some(l) = self.inner.live.lock().get_mut(&id) {
                l.agent = None;
            }
            self.update(id, |i| i.agent_id = outcome.agent_id.or(i.agent_id)).await?;
            self.charge(id, outcome.cost_usd).await?;
            if cancel.is_cancelled() {
                return Err(DetectiveError::Cancelled);
            }
            ports.vcs.commit_all(&worktree, &format!("fix(detective): {}", cap(&h.summary, 72))).await?;
            let head = ports.vcs.rev_parse(&worktree, "HEAD").await?;
            if head.as_deref() == Some(base.as_str()) {
                return Err(DetectiveError::Invalid("the agent did not change anything".into()));
            }

            // Tests on the fix branch.
            self.transition(
                id,
                Some(S::Fixing),
                S::Testing,
                Some(format!("attempt {attempt}: {}", cap(&outcome.summary, 200))),
                Actor::System,
                |_| {},
            )
            .await?;
            let run = tokio::select! {
                r = ports.tests.run(
                    TestRequest { project_id: inc.project_id, dir: worktree.clone(), command: h.reproduction.clone() },
                    cancel.child_token(),
                ) => r?,
                () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
            };
            let passed = run.passed;
            let tail = run.output_tail.clone();
            self.update(id, |i| i.fix_tests = Some(run)).await?;
            if passed {
                return self.review(id, &ports, &cancel, &worktree, &branch, &base, &outcome.task_id).await;
            }
            if attempt >= settings.max_fix_attempts {
                return Err(DetectiveError::Invalid(format!("the tests still fail after {attempt} fix attempt(s)")));
            }
            attempt += 1;
            failing = Some(tail);
            self.transition(
                id,
                Some(S::Testing),
                S::Fixing,
                Some(format!("tests fail; fix attempt {attempt}")),
                Actor::System,
                |_| {},
            )
            .await?;
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn review(
        &self,
        id: IncidentId,
        ports: &Ports,
        cancel: &CancellationToken,
        worktree: &Path,
        branch: &str,
        base: &str,
        task_id: &Option<String>,
    ) -> Result<(), DetectiveError> {
        let inc = self
            .transition(id, Some(S::Testing), S::Reviewing, Some("tests pass".into()), Actor::System, |_| {})
            .await?;
        Self::check_budget(&inc)?;
        let diff = ports.vcs.diff_range(worktree, base, "HEAD", 64 * 1024).await?;
        let req = ReviewRequest {
            project_id: inc.project_id,
            task_id: task_id.clone(),
            worktree: worktree.to_path_buf(),
            branch: branch.to_owned(),
            diff,
        };
        let outcome = tokio::select! {
            r = ports.reviewer.review(req, cancel.child_token()) => r,
            () = cancel.cancelled() => return Err(DetectiveError::Cancelled),
        };
        let (review_id, msg, cost) = match outcome {
            Ok(r) => (
                r.review_id,
                format!(
                    "review {}: {}",
                    if r.approved { "approved" } else { "requested changes" },
                    cap(&r.summary, 300)
                ),
                r.cost_usd,
            ),
            Err(DetectiveError::Cancelled) => return Err(DetectiveError::Cancelled),
            Err(e) => (None, format!("review unavailable: {e}"), 0.0),
        };
        self.update(id, |i| i.review_id.clone_from(&review_id)).await?;
        self.charge(id, cost).await?;
        self.enter_gate(id, S::Reviewing, S::AwaitingMergeApproval, msg).await
    }

    // ------------------------------------------------------------------
    // Merge
    // ------------------------------------------------------------------

    async fn merge(&self, id: IncidentId, by: Actor, note: Option<String>) -> Result<Incident, DetectiveError> {
        if !self.inner.merging.lock().insert(id) {
            return Err(DetectiveError::Conflict("the merge is already running".into()));
        }
        let r = self.merge_inner(id, by, note).await;
        self.inner.merging.lock().remove(&id);
        r
    }

    async fn merge_inner(&self, id: IncidentId, by: Actor, note: Option<String>) -> Result<Incident, DetectiveError> {
        let ports = self.ports()?;
        let inc = self.get(id).await?;
        if inc.stage != S::AwaitingMergeApproval {
            return Err(DetectiveError::Conflict(format!("incident is {:?}, not AwaitingMergeApproval", inc.stage)));
        }
        let branch = inc.fix_branch.clone().unwrap_or_else(|| fix_branch(id));
        let root = ports.projects.root(inc.project_id).await?;
        let target = ports
            .vcs
            .current_branch(&root)
            .await?
            .ok_or_else(|| DetectiveError::Conflict("the project checkout is on a detached HEAD".into()))?;
        let message = format!("Merge {branch}: {}", cap(&inc.title, 72));
        let commit = match ports.vcs.merge(&root, &branch, &target, &message).await {
            Ok(c) => c,
            Err(e) => {
                self.audit(
                    by,
                    "detective.merge",
                    &inc,
                    AuditOutcome::Failed,
                    serde_json::json!({ "error": e.to_string() }),
                )
                .await;
                return Err(e);
            }
        };
        let msg = format!(
            "merged {branch} into {target} ({}){}",
            &commit[..commit.len().min(10)],
            note.map(|n| format!(" — {n}")).unwrap_or_default()
        );
        self.audit(
            by,
            "detective.merge",
            &inc,
            AuditOutcome::Executed,
            serde_json::json!({ "commit": commit, "target": target }),
        )
        .await;
        let inc = self.transition(id, Some(S::AwaitingMergeApproval), S::Resolved, Some(msg), by, |_| {}).await?;
        if let Ok(Some(w)) = self.inner.store.worktree(id).await {
            if let Err(e) = ports.vcs.remove_worktree(inc.project_id, &root, Path::new(&w)).await {
                tracing::debug!(incident = %id, error = %e, "fix worktree not removed");
            }
            let _ = self.inner.store.set_worktree(id, None).await;
        }
        Ok(inc)
    }

    // ------------------------------------------------------------------
    // Recovery + auto-open
    // ------------------------------------------------------------------

    async fn recover(&self) -> Result<(), DetectiveError> {
        for inc in self.inner.store.list(None, false, 500).await? {
            if self.inner.live.lock().contains_key(&inc.id) {
                continue;
            }
            self.inner
                .live
                .lock()
                .insert(inc.id, Live { cancel: self.inner.shutdown.child_token(), ..Live::default() });
            if stage::is_gate(inc.stage) {
                // A decided (or still pending) approval is followed; one the
                // shutdown cancelled is requested again.
                let alive = match inc.approval_id {
                    Some(a) => matches!(
                        self.inner.deps.security.approvals.get(a).await,
                        Ok(Some(ap)) if ap.status != ApprovalStatus::Cancelled
                    ),
                    None => false,
                };
                match (alive, inc.approval_id) {
                    (true, Some(a)) => self.watch_gate(inc.id, inc.stage, a),
                    _ => {
                        let a = self.request_gate(&inc, inc.stage).await?;
                        self.update(inc.id, |i| i.approval_id = Some(a)).await?;
                        self.watch_gate(inc.id, inc.stage, a);
                    }
                }
            } else {
                let msg = format!("interrupted: Orkestr restarted during {}", stage::name(inc.stage));
                self.transition(inc.id, Some(inc.stage), S::Failed, Some(msg), Actor::System, |_| {}).await?;
            }
        }
        Ok(())
    }

    /// Follows `AlertRaised` for the optional auto-open. Tails the persisted
    /// event log (live subscription only on an in-memory bus) instead of
    /// holding a live bus subscription: a permanent
    /// subscriber would make the core believe a client is listening (node
    /// heartbeats would never idle).
    fn spawn_alert_listener(&self) -> JoinHandle<()> {
        const TAIL_POLL: Duration = Duration::from_secs(1);
        const TAIL_PAGE: u32 = 500;
        let me = self.clone();
        let shutdown = self.inner.shutdown.clone();
        tokio::spawn(async move {
            let bus = me.inner.deps.events.clone();
            if !bus.is_persistent() {
                // In-memory bus (tests): no log to tail, follow live events.
                let mut sub = bus.subscribe_all();
                loop {
                    let ev = tokio::select! {
                        ev = sub.recv() => ev,
                        () = shutdown.cancelled() => return,
                    };
                    match ev {
                        None => return,
                        Some(orkestr_events::Received::Event(ev)) => {
                            if let EventKind::AlertRaised(p) = &ev.kind {
                                me.auto_open(&p.alert).await;
                            }
                        }
                        Some(orkestr_events::Received::Lagged(_)) => {}
                    }
                }
            }
            let mut cursor = bus.last_seq();
            loop {
                tokio::select! {
                    () = shutdown.cancelled() => return,
                    () = tokio::time::sleep(TAIL_POLL) => {}
                }
                let last = bus.last_seq();
                if last <= cursor {
                    continue;
                }
                let mut from = cursor + 1;
                loop {
                    let q = orkestr_events::ReplayQuery {
                        from_seq: from,
                        to_seq: Some(last),
                        limit: TAIL_PAGE,
                        topics: None,
                        newest_first: false,
                    };
                    let page = match bus.replay(&q).await {
                        Ok(p) => p,
                        Err(e) => {
                            tracing::warn!(error = %e, "detective: cannot read the event log");
                            break;
                        }
                    };
                    for ev in &page.events {
                        from = ev.seq + 1;
                        if let EventKind::AlertRaised(p) = &ev.kind {
                            me.auto_open(&p.alert).await;
                        }
                    }
                    if !page.has_more || page.events.is_empty() {
                        break;
                    }
                }
                // Ephemeral events are never stored: everything up to `last` is done.
                cursor = last;
            }
        })
    }

    async fn auto_open(&self, alert: &orkestr_protocol::methods::monitor::Alert) {
        let Some(project_id) = alert.project_id else { return };
        match self.settings().await {
            Ok(s) if s.auto_open_on_alert => {}
            _ => return,
        }
        let open = self.inner.store.list(Some(project_id), false, 500).await.unwrap_or_default();
        if open.iter().any(|i| matches!(i.trigger, IncidentTrigger::Alert { alert_id } if alert_id == alert.id)) {
            return;
        }
        let p = OpenIncidentParams {
            project_id,
            trigger: IncidentTrigger::Alert { alert_id: alert.id },
            title: None,
            budget_usd: None,
        };
        match self.open(p, Actor::System).await {
            Ok(i) => tracing::info!(incident = %i.id, alert = %alert.id, "incident auto-opened from an alert"),
            Err(e) => tracing::warn!(alert = %alert.id, error = %e, "auto-open from an alert failed"),
        }
    }
}
