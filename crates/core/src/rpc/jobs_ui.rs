//! UI `jobs.submit` / `jobs.list` / `jobs.get` (Phase 6). **Owner: WP-B.**
//!
//! `jobs.submit {taskId, spec}`:
//! 1. validates the task (`Planned`/`Ready`/`Blocked`, no active job) and the spec
//!    (`Service` → `unimplementedFeature`);
//! 2. runs [`DefaultNodeScheduler`] over the current [`NodeSnapshot`]s from the
//!    installed [`NodeSnapshotSource`] (fallback: the local node as coordinator);
//! 3. stores the job row with the placement decision + explanation
//!    (`JobView.placement`, `jobs.list {taskId}`);
//! 4. `Place` → `JobStore::assign(job, node, epoch 1)` (state `Offered`,
//!    `JobAssigned` event), `Task.owner = node`, then [`JobDispatcher::dispatch`];
//!    `Wait` → job stays `Pending` (the dispatcher's owner re-places it);
//!    `Infeasible` → job `Infeasible`, task → `Blocked` through the task engine.
//!
//! ## Integration seam (until `Services` grows slots)
//! `Services` is not owned by WP-B, so the node registry (WP-A) and the job
//! dispatcher (WP-C) are wired with [`install_hooks`] keyed by the core's
//! `Services::node_id`. The integrator may move [`JobsUiHooks`] into `Services`
//! and replace [`hooks_for`] — nothing else changes.

use std::collections::HashMap;
use std::sync::{Arc, OnceLock, RwLock};

use async_trait::async_trait;
use orkestr_domain::{ExecutorRef, JobId, NodeId, TaskActor, TaskStatus, TrustLevel};
use orkestr_node::NodeScheduler;
use orkestr_protocol::methods::jobs::{
    self as m, JobIdParams, JobKind, JobResult, JobSpec, ListJobsParams, ListJobsResult, Placement, PlacementPolicy,
    ScoreWeights, SubmitJobParams,
};
use orkestr_protocol::methods::nodes::{NodeSlots, NodeSnapshot};
use orkestr_protocol::RpcError;
use orkestr_task_engine::can_transition;
use orkestr_task_engine::jobs::{JobDispatcher, JobError, JobFilter, JobStore};
use orkestr_task_engine::node_scheduler::DefaultNodeScheduler;

use super::tasks::task_to_rpc;
use super::{RpcContext, RpcRouter};
use crate::services::Services;

/// Job slots of the local node when no registry is installed.
pub const DEFAULT_LOCAL_SLOTS: u32 = 4;

/// Current coordinator view of all nodes (WP-A: `SqlNodeRegistry` snapshots,
/// the local node included with `is_coordinator = true`).
#[async_trait]
pub trait NodeSnapshotSource: Send + Sync {
    /// Snapshots used for placement.
    async fn snapshots(&self) -> Vec<NodeSnapshot>;
}

/// Late-bound collaborators of `jobs.*` (see the module docs).
#[derive(Clone, Default)]
pub struct JobsUiHooks {
    /// Node snapshots (default: the local node only).
    pub snapshots: Option<Arc<dyn NodeSnapshotSource>>,
    /// Receives every placed job (default: none — the job stays `Offered`).
    pub dispatcher: Option<Arc<dyn JobDispatcher>>,
    /// Score weights (default: node-protocol.md §5).
    pub weights: Option<ScoreWeights>,
}

impl std::fmt::Debug for JobsUiHooks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobsUiHooks")
            .field("snapshots", &self.snapshots.is_some())
            .field("dispatcher", &self.dispatcher.is_some())
            .field("weights", &self.weights)
            .finish()
    }
}

fn registry() -> &'static RwLock<HashMap<NodeId, JobsUiHooks>> {
    static HOOKS: OnceLock<RwLock<HashMap<NodeId, JobsUiHooks>>> = OnceLock::new();
    HOOKS.get_or_init(RwLock::default)
}

/// Installs the hooks of the core whose `Services::node_id` is `node_id`.
pub fn install_hooks(node_id: NodeId, hooks: JobsUiHooks) {
    registry().write().unwrap_or_else(std::sync::PoisonError::into_inner).insert(node_id, hooks);
}

/// Removes the hooks of a core (shutdown / tests).
pub fn clear_hooks(node_id: NodeId) {
    registry().write().unwrap_or_else(std::sync::PoisonError::into_inner).remove(&node_id);
}

/// Hooks of a core (defaults when none are installed).
#[must_use]
pub fn hooks_for(services: &Services) -> JobsUiHooks {
    registry()
        .read()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .get(&services.node_id)
        .cloned()
        .unwrap_or_default()
}

/// The job store over the core database.
#[must_use]
pub fn job_store(services: &Services) -> JobStore {
    JobStore::new(services.db.clone(), services.events.clone())
}

/// Snapshot of the local node as coordinator (fallback source).
pub async fn local_snapshot(services: &Services) -> NodeSnapshot {
    let node = services.nodes.local_node();
    let used = job_store(services).active_per_node().await.ok().and_then(|m| m.get(&node.id).copied()).unwrap_or(0);
    NodeSnapshot {
        status: node.status,
        last_heartbeat_at: node.last_heartbeat_at,
        node,
        trust_level: TrustLevel::Trusted,
        allowed_projects: vec!["*".to_owned()],
        slots: NodeSlots { max: DEFAULT_LOCAL_SLOTS, used },
        leases: Vec::new(),
        is_coordinator: true,
        labels: std::collections::BTreeMap::new(),
        model_digests: Vec::new(),
    }
}

/// Maps job-table errors to RPC errors.
#[must_use]
pub fn job_to_rpc(e: JobError) -> RpcError {
    match e {
        JobError::NotFound(id) => RpcError::not_found(format!("job {id} not found")),
        e @ JobError::StaleEpoch { .. } => RpcError::conflict(e.to_string()),
        JobError::Task(t) => task_to_rpc(t),
    }
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::Submit, _, _>(|ctx: RpcContext, p: SubmitJobParams| submit(ctx, p))
        .method::<m::List, _, _>(|ctx: RpcContext, p: ListJobsParams| async move {
            let filter =
                JobFilter { task_id: p.task_id, node_id: p.node_id, active_only: p.active_only.unwrap_or(false) };
            Ok(ListJobsResult { jobs: job_store(&ctx.services).list(filter).await.map_err(job_to_rpc)? })
        })
        .method::<m::Get, _, _>(|ctx: RpcContext, p: JobIdParams| async move {
            Ok(JobResult { job: job_store(&ctx.services).get(p.job_id).await.map_err(job_to_rpc)? })
        });
}

fn validate_kind(kind: &JobKind) -> Result<(), RpcError> {
    match kind {
        JobKind::Command { program, .. } if program.trim().is_empty() => {
            Err(RpcError::bad_request("spec.kind.program must not be empty"))
        }
        JobKind::AgentTask { prompt, .. } if prompt.trim().is_empty() => {
            Err(RpcError::bad_request("spec.kind.prompt must not be empty"))
        }
        JobKind::Service => Err(RpcError::unimplemented_feature("jobs.service")),
        _ => Ok(()),
    }
}

async fn submit(ctx: RpcContext, p: SubmitJobParams) -> Result<JobResult, RpcError> {
    let services = &ctx.services;
    validate_kind(&p.spec.kind)?;
    let task = services.tasks.get(p.task_id).await.map_err(task_to_rpc)?;
    if !matches!(task.status, TaskStatus::Planned | TaskStatus::Ready | TaskStatus::Blocked) {
        return Err(RpcError::conflict(format!(
            "task is {:?}; only Planned, Ready or Blocked tasks can be submitted",
            task.status
        )));
    }
    let store = job_store(services);
    if let Some(active) = store.active_for_task(task.id).await.map_err(job_to_rpc)? {
        return Err(RpcError::conflict(format!("task already has active job {}", active.spec.job_id)));
    }
    let spec = JobSpec {
        job_id: JobId::new(),
        task_id: task.id,
        project_id: task.project_id,
        kind: p.spec.kind,
        requirements: p.spec.requirements,
        estimate: p.spec.estimate,
        needs: p.spec.needs,
        priority: task.priority,
        affinity: p.spec.affinity,
    };
    let hooks = hooks_for(services);
    let nodes = match &hooks.snapshots {
        Some(src) => src.snapshots().await,
        None => vec![local_snapshot(services).await],
    };
    let policy =
        PlacementPolicy { weights: hooks.weights.unwrap_or_default(), coordinator_node_id: Some(services.node_id) };
    let placement = DefaultNodeScheduler.place(&spec, &nodes, &policy);
    let job_id = spec.job_id;
    store.create(spec, Some(placement.clone())).await.map_err(job_to_rpc)?;
    let explanation = match &placement {
        Placement::Place { explanation, .. }
        | Placement::Wait { explanation, .. }
        | Placement::Infeasible { explanation, .. } => explanation.clone(),
    };
    store.set_task_placement(task.id, explanation).await.map_err(job_to_rpc)?;
    match &placement {
        Placement::Place { node_id, .. } => {
            store.assign(job_id, *node_id, orkestr_domain::LeaseEpoch(1)).await.map_err(job_to_rpc)?;
            store.set_task_owner(task.id, Some(ExecutorRef::Node { node_id: *node_id })).await.map_err(job_to_rpc)?;
            match &hooks.dispatcher {
                Some(d) => d.dispatch(job_id),
                None => tracing::warn!(job_id = %job_id, "no job dispatcher installed; job stays offered"),
            }
        }
        Placement::Wait { .. } => tracing::info!(job_id = %job_id, "job waits for capacity"),
        Placement::Infeasible { reason, .. } => {
            store.set_placement(job_id, placement.clone()).await.map_err(job_to_rpc)?;
            if task.status != TaskStatus::Blocked && can_transition(task.status, TaskStatus::Blocked) {
                services
                    .tasks
                    .transition(
                        task.id,
                        TaskStatus::Blocked,
                        TaskActor::System,
                        Some(format!("job infeasible: {reason}")),
                        None,
                    )
                    .await
                    .map_err(task_to_rpc)?;
            }
        }
    }
    Ok(JobResult { job: store.get(job_id).await.map_err(job_to_rpc)? })
}
