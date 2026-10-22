//! Job control (Phase 6). **Owner: WP-C.**
//!
//! * UI: `jobs.migrate`, `jobs.cancel` (need the coordinator driver installed
//!   with `nodes::coordinator_jobs::install`; otherwise `unimplemented_feature`);
//! * worker side (coordinator principals only, enforced by `authz`):
//!   `jobs.offer`, `jobs.drain`, `node.jobCancel` → `nodes::worker::host`.

use orkestr_node::exec::{JobError, JobExecutor};
use orkestr_protocol::methods::jobs as m;
use orkestr_protocol::RpcError;

use super::{Principal, RpcContext, RpcRouter};
use crate::nodes::{coordinator_jobs, worker};

fn job_err(e: JobError) -> RpcError {
    match e {
        JobError::NotFound(_) => RpcError::not_found(e.to_string()),
        JobError::StaleEpoch { .. } | JobError::Rejected(_) => RpcError::conflict(e.to_string()),
        JobError::Unimplemented(f) => RpcError::unimplemented_feature(f),
        JobError::Failed(_) => RpcError::internal(e.to_string()),
    }
}

/// Audits a fenced (stale-epoch) job-side call on the worker.
async fn audit_stale(ctx: &RpcContext, method: &str, job: orkestr_domain::JobId, detail: &str) {
    ctx.services
        .security
        .audit
        .record(
            orkestr_security::NewAudit::new(
                orkestr_domain::Actor::System,
                "jobs.staleEpochRejected",
                format!("node:{}/job:{job}", ctx.services.node_id),
                orkestr_domain::AuditOutcome::Denied,
            )
            .details(
                serde_json::json!({"method": method, "jobId": job, "nodeId": ctx.services.node_id, "detail": detail}),
            ),
        )
        .await;
}

async fn worker_err(ctx: &RpcContext, method: &str, job: orkestr_domain::JobId, e: JobError) -> RpcError {
    if matches!(e, JobError::StaleEpoch { .. }) {
        audit_stale(ctx, method, job, &e.to_string()).await;
    }
    job_err(e)
}

fn coordinator(ctx: &RpcContext) -> Result<std::sync::Arc<coordinator_jobs::CoordinatorJobs>, RpcError> {
    coordinator_jobs::coordinator(ctx.services.node_id)
        .ok_or_else(|| RpcError::unimplemented_feature("jobs coordinator (no worker nodes configured)"))
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router.method::<m::Migrate, _, _>(|ctx: RpcContext, p: m::MigrateJobParams| async move {
        let c = coordinator(&ctx)?;
        let job = c.migrate(p.job_id, p.target_node_id).await?;
        Ok(m::JobResult { job })
    });
    router.method::<m::Cancel, _, _>(|ctx: RpcContext, p: m::CancelJobParams| async move {
        let c = coordinator(&ctx)?;
        let job = c.cancel(p.job_id, p.immediate).await?;
        Ok(m::JobResult { job })
    });
    router.method::<m::Offer, _, _>(|ctx: RpcContext, offer: m::JobOffer| async move {
        let trust = match &ctx.principal {
            Principal::Local => None,
            Principal::Remote(rp) => {
                if !rp.coordinator {
                    return Err(RpcError::forbidden("jobs.offer requires a coordinator principal"));
                }
                Some(rp.trust_level.unwrap_or(orkestr_domain::TrustLevel::Restricted))
            }
        };
        let job = offer.spec.job_id;
        let r = worker::host(&ctx.services).offer(offer, trust).await;
        if r.reason.as_deref().is_some_and(|x| x.starts_with("stale epoch")) {
            audit_stale(&ctx, "jobs.offer", job, r.reason.as_deref().unwrap_or_default()).await;
        }
        Ok(r)
    });
    router.method::<m::Drain, _, _>(|ctx: RpcContext, p: m::WorkerJobParams| async move {
        let checkpoint = match worker::host(&ctx.services).drain(p.job_id, p.epoch).await {
            Ok(c) => c,
            Err(e) => return Err(worker_err(&ctx, "jobs.drain", p.job_id, e).await),
        };
        Ok(m::DrainJobResult { checkpoint })
    });
    router.method::<m::JobCancel, _, _>(|ctx: RpcContext, p: m::WorkerJobParams| async move {
        if let Err(e) = worker::host(&ctx.services).cancel(p.job_id, p.epoch, p.immediate).await {
            return Err(worker_err(&ctx, "node.jobCancel", p.job_id, e).await);
        }
        Ok(m::WorkerCancelResult { cancelled: true })
    });
}
