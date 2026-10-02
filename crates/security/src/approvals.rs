//! Approval requests (§23, permission model §5.1).
//!
//! `request` persists an [`Approval`] (redacted), emits `ApprovalRequested`
//! and returns immediately; `wait` blocks the caller (an agent tool call)
//! until the user resolves it, the timeout expires (→ `Expired`) or the
//! caller's cancellation token fires (→ `Cancelled`). `resolve` is the user
//! path (`approvals.resolve`) and emits `ApprovalResolved`. Only users
//! resolve approvals (hard policies P1/P2).

use std::collections::HashMap;
use std::time::Duration;

use orkestr_domain::{
    now, Actor, AgentId, Approval, ApprovalId, ApprovalStatus, AuditOutcome, Capability, DangerCategory, ProjectId,
    RiskLevel, TaskId, ToolCallId,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::{ApprovalRequestedPayload, ApprovalResolvedPayload, EventKind};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration};
use parking_lot::Mutex;
use std::sync::Arc;
use tokio::sync::watch;
use tokio_util::sync::CancellationToken;

use crate::audit::{AuditLog, NewAudit};
use crate::error::{SecurityError, SecurityResult};
use crate::redact::redact;

/// Migration creating the approvals table.
pub const MIGRATION: Migration = Migration {
    id: "security_0004_approvals",
    owner: "security",
    up: r"
CREATE TABLE approvals (
    id TEXT PRIMARY KEY NOT NULL,
    project_id TEXT,
    agent_id TEXT,
    status TEXT NOT NULL,
    requested_at TEXT NOT NULL,
    expires_at TEXT,
    doc TEXT NOT NULL
);
CREATE INDEX approvals_status ON approvals(status, requested_at);
CREATE INDEX approvals_project ON approvals(project_id, requested_at);
",
    down: "DROP TABLE IF EXISTS approvals;",
};

/// Default time an approval stays pending.
pub const DEFAULT_APPROVAL_TTL: Duration = Duration::from_secs(24 * 3600);
/// Max length of preview / summary texts.
const MAX_TEXT: usize = 8 * 1024;

/// A new approval request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalRequest {
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Requesting agent.
    pub agent_id: Option<AgentId>,
    /// Tool call awaiting the decision.
    pub tool_call_id: Option<ToolCallId>,
    /// Capability requested.
    pub capability: Capability,
    /// One-line description.
    pub action_summary: String,
    /// Exact command / diff / SQL.
    pub command_preview: Option<String>,
    /// Risk.
    pub risk: RiskLevel,
    /// Why approval is needed.
    pub reason: String,
    /// Guard category.
    pub danger: Option<DangerCategory>,
    /// Pending lifetime (default 24 h).
    pub ttl: Option<Duration>,
}

impl ApprovalRequest {
    /// Minimal request.
    #[must_use]
    pub fn new(
        capability: Capability,
        action_summary: impl Into<String>,
        reason: impl Into<String>,
        risk: RiskLevel,
    ) -> Self {
        Self {
            project_id: None,
            task_id: None,
            agent_id: None,
            tool_call_id: None,
            capability,
            action_summary: action_summary.into(),
            command_preview: None,
            risk,
            reason: reason.into(),
            danger: None,
            ttl: None,
        }
    }
}

/// User decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApprovalDecision {
    /// Approve.
    Approve,
    /// Reject.
    Reject,
}

/// Which pending approvals to cancel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CancelScope {
    /// All pending approvals.
    All,
    /// A project's.
    Project(ProjectId),
    /// An agent's.
    Agent(AgentId),
}

/// List filter.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApprovalFilter {
    /// Status.
    pub status: Option<ApprovalStatus>,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Max rows (default 200, max 1000).
    pub limit: Option<u32>,
}

/// The approval service.
pub struct ApprovalService {
    db: Db,
    events: EventBus,
    audit: Arc<AuditLog>,
    waiters: Mutex<HashMap<ApprovalId, watch::Sender<ApprovalStatus>>>,
}

impl std::fmt::Debug for ApprovalService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ApprovalService").finish_non_exhaustive()
    }
}

fn status_str(s: ApprovalStatus) -> &'static str {
    match s {
        ApprovalStatus::Pending => "Pending",
        ApprovalStatus::Approved => "Approved",
        ApprovalStatus::Rejected => "Rejected",
        ApprovalStatus::Expired => "Expired",
        ApprovalStatus::Cancelled => "Cancelled",
    }
}

fn clip(s: &str) -> String {
    let r = redact(s);
    if r.len() <= MAX_TEXT {
        return r;
    }
    let mut end = MAX_TEXT;
    while !r.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &r[..end])
}

impl ApprovalService {
    /// Creates the service. Approvals left pending by a previous session are
    /// cancelled (their requesters no longer exist).
    pub async fn open(db: Db, events: EventBus, audit: Arc<AuditLog>) -> SecurityResult<Self> {
        let svc = Self { db, events, audit, waiters: Mutex::new(HashMap::new()) };
        let stale = svc
            .list(&ApprovalFilter { status: Some(ApprovalStatus::Pending), limit: Some(1000), ..Default::default() })
            .await?;
        for a in stale {
            svc.finish(a.id, ApprovalStatus::Cancelled, Some(Actor::System), Some("core restarted".into())).await.ok();
        }
        Ok(svc)
    }

    async fn store(&self, a: &Approval) -> SecurityResult<()> {
        let doc = serde_json::to_string(a)?;
        let (id, project, agent, status, requested, expires) = (
            a.id.to_string(),
            a.project_id.map(|p| p.to_string()),
            a.agent_id.map(|p| p.to_string()),
            status_str(a.status),
            a.requested_at.to_rfc3339(),
            a.expires_at.map(|e| e.to_rfc3339()),
        );
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO approvals (id, project_id, agent_id, status, requested_at, expires_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, doc = excluded.doc",
                    params![id, project, agent, status, requested, expires, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// Creates a pending approval and emits `ApprovalRequested`.
    pub async fn request(&self, req: ApprovalRequest) -> SecurityResult<Approval> {
        let summary = req.action_summary.trim();
        if summary.is_empty() {
            return Err(SecurityError::Invalid("approval action summary must not be empty".into()));
        }
        let t = now();
        let ttl = req.ttl.unwrap_or(DEFAULT_APPROVAL_TTL);
        let approval = Approval {
            id: ApprovalId::new(),
            project_id: req.project_id,
            task_id: req.task_id,
            agent_id: req.agent_id,
            tool_call_id: req.tool_call_id,
            capability: req.capability,
            action_summary: clip(summary),
            risk: req.risk,
            reason: clip(&req.reason),
            status: ApprovalStatus::Pending,
            requested_at: t,
            expires_at: chrono::Duration::from_std(ttl).ok().map(|d| t + d),
            resolved_at: None,
            resolved_by: None,
            resolution_note: None,
            command_preview: req.command_preview.as_deref().map(clip),
            danger: req.danger,
        };
        self.store(&approval).await?;
        let (tx, _rx) = watch::channel(ApprovalStatus::Pending);
        self.waiters.lock().insert(approval.id, tx);
        self.events
            .publish(NewEvent::new(EventKind::ApprovalRequested(ApprovalRequestedPayload {
                approval: approval.clone(),
            })))
            .await?;
        tracing::info!(approval_id = %approval.id, capability = ?approval.capability, risk = ?approval.risk, "approval requested");
        Ok(approval)
    }

    /// One approval.
    pub async fn get(&self, id: ApprovalId) -> SecurityResult<Option<Approval>> {
        let ids = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM approvals WHERE id = ?1", [ids], |r| r.get(0)).optional()?)
            })
            .await?;
        Ok(doc.map(|d| serde_json::from_str(&d)).transpose()?)
    }

    /// Lists approvals, newest first.
    pub async fn list(&self, f: &ApprovalFilter) -> SecurityResult<Vec<Approval>> {
        let status = f.status.map(status_str);
        let project = f.project_id.map(|p| p.to_string());
        let limit = i64::from(f.limit.unwrap_or(200).clamp(1, 1000));
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM approvals WHERE (?1 IS NULL OR status = ?1) AND (?2 IS NULL OR project_id = ?2)
                     ORDER BY requested_at DESC, id DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![status, project, limit], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for r in rows {
                    out.push(r?);
                }
                Ok(out)
            })
            .await?;
        let mut out = Vec::with_capacity(docs.len());
        for d in docs {
            out.push(serde_json::from_str(&d)?);
        }
        Ok(out)
    }

    /// Moves a pending approval to a final status, emits `ApprovalResolved`
    /// and wakes waiters. `Conflict` if it is not pending anymore.
    async fn finish(
        &self,
        id: ApprovalId,
        status: ApprovalStatus,
        by: Option<Actor>,
        note: Option<String>,
    ) -> SecurityResult<Approval> {
        let mut a = self.get(id).await?.ok_or_else(|| SecurityError::NotFound(format!("approval {id}")))?;
        if a.status != ApprovalStatus::Pending {
            return Err(SecurityError::Conflict(format!("approval {id} is already {:?}", a.status)));
        }
        a.status = status;
        a.resolved_at = Some(now());
        a.resolved_by = by;
        a.resolution_note = note.map(|n| clip(n.trim())).filter(|n| !n.is_empty());
        // Compare-and-set on status so concurrent resolutions cannot both win.
        let doc = serde_json::to_string(&a)?;
        let ids = id.to_string();
        let st = status_str(status);
        let changed = self
            .db
            .call(move |c| {
                Ok(c.execute(
                    "UPDATE approvals SET status = ?1, doc = ?2 WHERE id = ?3 AND status = 'Pending'",
                    params![st, doc, ids],
                )?)
            })
            .await?;
        if changed == 0 {
            return Err(SecurityError::Conflict(format!("approval {id} was resolved concurrently")));
        }
        if let Some(tx) = self.waiters.lock().remove(&id) {
            let _ = tx.send(status);
        }
        let mut ev = NewEvent::new(EventKind::ApprovalResolved(ApprovalResolvedPayload {
            approval_id: id,
            status,
            resolved_by: by,
        }));
        if let Some(p) = a.project_id {
            ev = ev.project(p);
        }
        if let Some(t) = a.task_id {
            ev = ev.task(t);
        }
        if let Some(ag) = a.agent_id {
            ev = ev.agent(ag);
        }
        self.events.publish(ev).await?;
        let outcome = match status {
            ApprovalStatus::Approved => AuditOutcome::Approved,
            _ => AuditOutcome::Rejected,
        };
        self.audit
            .record(
                NewAudit::new(by.unwrap_or(Actor::System), "approvals.resolve", a.action_summary.clone(), outcome)
                    .capability(a.capability)
                    .project(a.project_id)
                    .details(serde_json::json!({ "approvalId": id, "status": status, "note": a.resolution_note })),
            )
            .await;
        tracing::info!(approval_id = %id, ?status, "approval resolved");
        Ok(a)
    }

    /// User decision (`approvals.resolve`).
    pub async fn resolve(
        &self,
        id: ApprovalId,
        decision: ApprovalDecision,
        note: Option<String>,
        by: Actor,
    ) -> SecurityResult<Approval> {
        if !matches!(by, Actor::User(_)) {
            return Err(SecurityError::Forbidden("only users can resolve approvals (P1/P2)".into()));
        }
        let status = match decision {
            ApprovalDecision::Approve => ApprovalStatus::Approved,
            ApprovalDecision::Reject => ApprovalStatus::Rejected,
        };
        if let Some(a) = self.get(id).await? {
            if a.expires_at.is_some_and(|e| e <= now()) && a.status == ApprovalStatus::Pending {
                self.finish(id, ApprovalStatus::Expired, Some(Actor::System), Some("expired".into())).await?;
                return Err(SecurityError::Conflict(format!("approval {id} has expired")));
            }
        }
        self.finish(id, status, Some(by), note).await
    }

    /// Cancels one pending approval (requester went away).
    pub async fn cancel(&self, id: ApprovalId, reason: &str) -> SecurityResult<Approval> {
        self.finish(id, ApprovalStatus::Cancelled, Some(Actor::System), Some(reason.to_owned())).await
    }

    /// Cancels every pending approval in scope. Returns how many.
    pub async fn cancel_scope(&self, scope: CancelScope, reason: &str) -> SecurityResult<u32> {
        let pending = self
            .list(&ApprovalFilter { status: Some(ApprovalStatus::Pending), limit: Some(1000), ..Default::default() })
            .await?;
        let mut n = 0u32;
        for a in pending {
            let hit = match scope {
                CancelScope::All => true,
                CancelScope::Project(p) => a.project_id == Some(p),
                CancelScope::Agent(ag) => a.agent_id == Some(ag),
            };
            if hit && self.cancel(a.id, reason).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Expires pending approvals whose deadline passed. Returns how many.
    pub async fn expire_due(&self) -> SecurityResult<u32> {
        let pending = self
            .list(&ApprovalFilter { status: Some(ApprovalStatus::Pending), limit: Some(1000), ..Default::default() })
            .await?;
        let t = now();
        let mut n = 0u32;
        for a in pending.into_iter().filter(|a| a.expires_at.is_some_and(|e| e <= t)) {
            if self.finish(a.id, ApprovalStatus::Expired, Some(Actor::System), Some("expired".into())).await.is_ok() {
                n += 1;
            }
        }
        Ok(n)
    }

    /// Periodically expires due approvals until `shutdown` fires.
    pub async fn run_expiry_loop(self: Arc<Self>, every: Duration, shutdown: CancellationToken) {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(every) => {
                    if let Err(e) = self.expire_due().await {
                        tracing::warn!(error = %e, "approval expiry sweep failed");
                    }
                }
            }
        }
    }

    /// Waits for a decision. On timeout the approval becomes `Expired`; when
    /// `cancel` fires it becomes `Cancelled`. Returns the final status.
    pub async fn wait(
        &self,
        id: ApprovalId,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> SecurityResult<ApprovalStatus> {
        let mut rx = {
            let mut w = self.waiters.lock();
            w.entry(id).or_insert_with(|| watch::channel(ApprovalStatus::Pending).0).subscribe()
        };
        // Re-check persisted state (it may have been resolved before we subscribed).
        let current = self.get(id).await?.ok_or_else(|| SecurityError::NotFound(format!("approval {id}")))?;
        if current.status != ApprovalStatus::Pending {
            self.waiters.lock().remove(&id);
            return Ok(current.status);
        }
        let outcome = tokio::select! {
            r = async {
                loop {
                    if rx.changed().await.is_err() {
                        return None;
                    }
                    let s = *rx.borrow();
                    if s != ApprovalStatus::Pending {
                        return Some(s);
                    }
                }
            } => r,
            () = tokio::time::sleep(timeout) => Some(ApprovalStatus::Expired),
            () = cancel.cancelled() => Some(ApprovalStatus::Cancelled),
        };
        match outcome {
            Some(ApprovalStatus::Expired) | Some(ApprovalStatus::Cancelled) | None => {
                let target = if outcome == Some(ApprovalStatus::Cancelled) {
                    ApprovalStatus::Cancelled
                } else {
                    ApprovalStatus::Expired
                };
                match self
                    .finish(
                        id,
                        target,
                        Some(Actor::System),
                        Some(if target == ApprovalStatus::Cancelled {
                            "cancelled by requester".into()
                        } else {
                            "timed out".into()
                        }),
                    )
                    .await
                {
                    Ok(a) => Ok(a.status),
                    // Lost the race against a user decision: report what happened.
                    Err(SecurityError::Conflict(_)) => Ok(self.get(id).await?.map_or(target, |a| a.status)),
                    Err(e) => Err(e),
                }
            }
            Some(s) => Ok(s),
        }
    }

    /// `request` + `wait`.
    pub async fn request_and_wait(
        &self,
        req: ApprovalRequest,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> SecurityResult<(Approval, ApprovalStatus)> {
        let a = self.request(req).await?;
        let status = self.wait(a.id, timeout, cancel).await?;
        Ok((a, status))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_domain::UserId;
    use orkestr_events::Received;

    async fn svc() -> (EventBus, Arc<ApprovalService>) {
        let db = Db::open_in_memory().unwrap();
        db.migrate(crate::migrations()).await.unwrap();
        let bus = EventBus::in_memory(256);
        let audit = Arc::new(AuditLog::new(db.clone()));
        (bus.clone(), Arc::new(ApprovalService::open(db, bus, audit).await.unwrap()))
    }

    fn req(p: ProjectId) -> ApprovalRequest {
        let mut r = ApprovalRequest::new(Capability::Terminal, "Run `git push --force`", "dangerous", RiskLevel::High);
        r.project_id = Some(p);
        r.command_preview = Some("git push --force # token=abcdef123".into());
        r.danger = Some(DangerCategory::GitDestructive);
        r
    }

    #[tokio::test]
    async fn request_resolve_and_wait() {
        let (bus, s) = svc().await;
        let p = ProjectId::new();
        let mut sub = bus.subscribe([format!("project:{p}")]).unwrap();
        let a = s.request(req(p)).await.unwrap();
        assert_eq!(a.status, ApprovalStatus::Pending);
        assert!(a.command_preview.as_deref().unwrap().contains("token=[REDACTED]"), "previews are redacted");
        let Some(Received::Event(ev)) = sub.recv().await else { panic!() };
        assert_eq!(ev.kind_name(), "ApprovalRequested");

        let waiter = {
            let s = Arc::clone(&s);
            tokio::spawn(async move { s.wait(a.id, Duration::from_secs(10), &CancellationToken::new()).await })
        };
        tokio::time::sleep(Duration::from_millis(50)).await;
        let user = Actor::User(UserId::new());
        let resolved = s.resolve(a.id, ApprovalDecision::Approve, Some("ok".into()), user).await.unwrap();
        assert_eq!(resolved.status, ApprovalStatus::Approved);
        assert_eq!(resolved.resolution_note.as_deref(), Some("ok"));
        assert_eq!(waiter.await.unwrap().unwrap(), ApprovalStatus::Approved);
        let Some(Received::Event(ev)) = sub.recv().await else { panic!() };
        assert_eq!(ev.kind_name(), "ApprovalResolved");
        // Second resolution conflicts.
        assert!(matches!(s.resolve(a.id, ApprovalDecision::Reject, None, user).await, Err(SecurityError::Conflict(_))));
        // Waiting on an already resolved approval returns immediately.
        assert_eq!(
            s.wait(a.id, Duration::from_millis(10), &CancellationToken::new()).await.unwrap(),
            ApprovalStatus::Approved
        );
        // History / filters.
        assert_eq!(
            s.list(&ApprovalFilter { status: Some(ApprovalStatus::Approved), ..Default::default() })
                .await
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            s.list(&ApprovalFilter { status: Some(ApprovalStatus::Pending), ..Default::default() })
                .await
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn only_users_resolve() {
        let (_bus, s) = svc().await;
        let a = s.request(req(ProjectId::new())).await.unwrap();
        for actor in [Actor::Manager, Actor::Agent(AgentId::new()), Actor::System] {
            assert!(matches!(
                s.resolve(a.id, ApprovalDecision::Approve, None, actor).await,
                Err(SecurityError::Forbidden(_))
            ));
        }
    }

    #[tokio::test]
    async fn timeout_expires_and_cancel_cancels() {
        let (_bus, s) = svc().await;
        let a = s.request(req(ProjectId::new())).await.unwrap();
        assert_eq!(
            s.wait(a.id, Duration::from_millis(30), &CancellationToken::new()).await.unwrap(),
            ApprovalStatus::Expired
        );
        assert_eq!(s.get(a.id).await.unwrap().unwrap().status, ApprovalStatus::Expired);

        let b = s.request(req(ProjectId::new())).await.unwrap();
        let token = CancellationToken::new();
        let t2 = token.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(30)).await;
            t2.cancel();
        });
        assert_eq!(s.wait(b.id, Duration::from_secs(10), &token).await.unwrap(), ApprovalStatus::Cancelled);
    }

    #[tokio::test]
    async fn expiry_sweep_and_scope_cancel() {
        let (_bus, s) = svc().await;
        let p1 = ProjectId::new();
        let mut short = req(p1);
        short.ttl = Some(Duration::from_millis(1));
        let a = s.request(short).await.unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
        assert_eq!(s.expire_due().await.unwrap(), 1);
        assert_eq!(s.get(a.id).await.unwrap().unwrap().status, ApprovalStatus::Expired);
        // Resolving an expired one conflicts.
        assert!(s.resolve(a.id, ApprovalDecision::Approve, None, Actor::User(UserId::new())).await.is_err());

        s.request(req(p1)).await.unwrap();
        s.request(req(p1)).await.unwrap();
        s.request(req(ProjectId::new())).await.unwrap();
        assert_eq!(s.cancel_scope(CancelScope::Project(p1), "stop").await.unwrap(), 2);
        assert_eq!(s.cancel_scope(CancelScope::All, "stop").await.unwrap(), 1);
    }
}
