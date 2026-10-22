//! Planned (drain) and unplanned (lease expiry) migration, side-effect gate
//! (`needs_user_decision` + `ApprovalRequested`) (Phase 6, node-protocol.md §7.2).
//! **Owner: WP-C.**
//!
//! Task transitions (all through the task engine, privileged ones as
//! `SystemRecovery`): `Running → Interrupted → Recovering → Running` (resumed
//! elsewhere with epoch + 1) or `Recovering → Failed` (`needs_user_decision`).

use std::sync::Arc;

use orkestr_domain::{AuditOutcome, Capability, JobId, NodeId, RiskLevel, TaskActor, TaskStatus};
use orkestr_node::exec::needs_user_decision;
use orkestr_protocol::methods::jobs::{JobState, JobView, SideEffectKind};
use orkestr_security::approvals::ApprovalRequest;

use super::coordinator_jobs::{CoordinatorJobs, JobsError};
use super::lease::Lease;

/// Reason recorded on the task when a resume needs the user.
pub const NEEDS_USER_DECISION: &str = "needs_user_decision";

impl CoordinatorJobs {
    async fn interrupt(&self, job: JobId, reason: &str) {
        if matches!(
            self.task_status(job).await,
            Some(TaskStatus::Running | TaskStatus::Waiting | TaskStatus::Review | TaskStatus::Testing)
        ) {
            self.task_to(job, TaskStatus::Interrupted, TaskActor::SystemRecovery, reason).await;
        }
        if self.task_status(job).await == Some(TaskStatus::Interrupted) {
            self.task_to(job, TaskStatus::Recovering, TaskActor::SystemRecovery, reason).await;
        }
    }

    /// Unplanned migration after node loss / lease expiry (§7.2).
    pub async fn recover_unplanned(self: &Arc<Self>, job: JobId, lease: Lease, reason: &str) {
        if self.leases.get(job).is_some_and(|l| l.epoch == lease.epoch) {
            self.leases.release(job);
        }
        self.queue_revoke(lease.node_id, job);
        let Some(view) = self.get(job) else { return };
        if matches!(view.state, JobState::Completed | JobState::Failed | JobState::Cancelled) {
            return;
        }
        self.update(job, |e| {
            e.view.state = JobState::Migrating;
            if !e.excluded.contains(&lease.node_id) {
                e.excluded.push(lease.node_id);
            }
        })
        .await;
        self.audit(
            "jobs.leaseExpired",
            job,
            lease.node_id,
            AuditOutcome::Failed,
            serde_json::json!({"epoch": lease.epoch, "reason": reason}),
        )
        .await;
        self.interrupt(job, reason).await;
        let (effects, cp) = {
            let jobs = self.jobs.lock();
            let Some(e) = jobs.get(&job) else { return };
            (e.effects_since_checkpoint.clone(), e.last_checkpoint.clone())
        };
        if needs_user_decision(&effects) {
            self.gate(job, lease.node_id, &effects).await;
            return;
        }
        // without a checkpoint the job restarts from its original inputs
        // (nothing irreversible happened, per the gate above)
        let resume = cp.map(|c| (c, lease.node_id, reason.to_owned()));
        if let Err(e) = self.place_and_dispatch(job, vec![lease.node_id], resume).await {
            tracing::warn!(job_id = %job, error = %e, "re-placement after node loss failed");
            self.update(job, |en| {
                en.view.state = JobState::Pending;
                en.view.detail = Some(format!("waiting for a node: {e}"));
            })
            .await;
        }
    }

    /// Side-effect gate: `Recovering → Failed(needs_user_decision)` + `ApprovalRequested`.
    async fn gate(
        self: &Arc<Self>,
        job: JobId,
        node: NodeId,
        effects: &[orkestr_protocol::methods::jobs::SideEffectRecord],
    ) {
        let listed: Vec<String> = effects
            .iter()
            .filter(|e| e.kind != SideEffectKind::Idempotent)
            .map(|e| format!("{} ({:?}): {}", e.action, e.kind, e.detail))
            .collect();
        let Some(view) = self.get(job) else { return };
        self.update(job, |e| {
            e.view.state = JobState::Failed;
            e.view.detail = Some(NEEDS_USER_DECISION.to_owned());
        })
        .await;
        self.task_to(job, TaskStatus::Failed, TaskActor::SystemRecovery, NEEDS_USER_DECISION).await;
        let mut req = ApprovalRequest::new(
            Capability::Deploy,
            format!("Resume job {job} after node {node} was lost?"),
            format!(
                "{NEEDS_USER_DECISION}: side effects with unknown outcome since the last checkpoint: {}",
                listed.join("; ")
            ),
            RiskLevel::High,
        );
        req.project_id = Some(view.spec.project_id);
        req.task_id = Some(view.spec.task_id);
        req.command_preview = Some(listed.join("\n"));
        if let Err(e) = self.services.security.approvals.request(req).await {
            tracing::warn!(job_id = %job, error = %e, "approval request for resume failed");
        }
        self.audit(
            "jobs.needsUserDecision",
            job,
            node,
            AuditOutcome::Denied,
            serde_json::json!({"sideEffects": listed}),
        )
        .await;
    }

    /// Planned migration (`jobs.migrate`): drain on the current node →
    /// checkpoint → offer to `target` (or a re-placement excluding the source)
    /// with epoch + 1; the target resumes from the checkpoint + handoff.
    ///
    /// # Errors
    /// Unknown job, no lease, drain / offer failures.
    pub async fn migrate(self: &Arc<Self>, job: JobId, target: Option<NodeId>) -> Result<JobView, JobsError> {
        let view = self.get(job).ok_or(JobsError::NotFound(job))?;
        if view.state != JobState::Running {
            return Err(JobsError::State(format!("job is {:?}, not running", view.state)));
        }
        if let Some(t) = target {
            if self.leases.get(job).is_some_and(|l| l.node_id == t) {
                return Err(JobsError::State("target is the current node".into()));
            }
        }
        let (cp, from) = self.drain(job).await?;
        self.interrupt(job, "migration").await;
        let resume = Some((cp, from, "planned".to_owned()));
        let r = match target {
            Some(t) => {
                self.update(job, |e| e.view.placement = None).await;
                self.dispatch(job, t, vec![format!("migration target {t} chosen by the user")], resume).await
            }
            None => self.place_and_dispatch(job, vec![from], resume).await,
        };
        if let Err(e) = r {
            self.update(job, |en| {
                en.view.state = JobState::Pending;
                en.view.detail = Some(format!("migration pending: {e}"));
            })
            .await;
            return Err(e);
        }
        self.get(job).ok_or(JobsError::NotFound(job))
    }
}
