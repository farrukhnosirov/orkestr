//! Coordinator job driver: place в†’ offer (epoch + 1) в†’ accept в†’ progress
//! intake (stale epochs dropped) в†’ completion through the task engine, which
//! stays the only task-status writer (Phase 6). **Owner: WP-C.**
//!
//! [`CoordinatorJobs`] is wired by the integrator with three small traits:
//! * [`LinkProvider`] вЂ” `Arc<dyn NodeLink>` per node + current snapshots (WP-A registry);
//! * [`JobPlacer`] вЂ” placement with an exclusion list (wrap WP-B's
//!   `NodeScheduler` with [`SchedulerPlacer`]; [`FirstFitPlacer`] for tests);
//! * WP-B's `JobStore` (jobs table) is the persisted source of truth for job
//!   rows / epochs; [`CoordinatorJobs::dispatcher`] is the `JobDispatcher` for
//!   WP-B's `jobs.submit` hooks.
//!
//! Blobs move with [`BlobTransfer`] ([`RpcBlobTransfer`] = WP-D's `push_blob`
//! and `node.blobGet`). WP-A's heartbeat loop calls
//! [`CoordinatorJobs::heartbeat_params`] before and
//! [`CoordinatorJobs::heartbeat_ok`] after each `node.heartbeat`; a periodic
//! [`CoordinatorJobs::tick`] (see [`CoordinatorJobs::spawn_lease_loop`])
//! turns expired leases into unplanned migrations.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use orkestr_domain::{
    Actor, AuditOutcome, Digest, ExecutorRef, JobId, LeaseEpoch, NodeId, TaskActor, TaskStatus, Timestamp,
};
use orkestr_node::link::call_typed;
use orkestr_node::scheduling::NodeScheduler;
use orkestr_node::{BlobError, BlobStore, NodeError, NodeLink};
use orkestr_protocol::event::{EventKind, NewEvent};
use orkestr_protocol::methods::jobs::{
    self as jm, DrainJobResult, GitStateDescriptor, JobCheckpoint, JobCheckpointedPayload, JobInputs,
    JobMigratedPayload, JobOffer, JobProgressItem, JobProgressPayload, JobSpec, JobState, JobView, Placement,
    PlacementPolicy, PolicySnapshot, SideEffectRecord, WorkerJobParams,
};
use orkestr_protocol::methods::models::{BlobGetParams, BLOB_CHUNK_SIZE};
use orkestr_protocol::methods::nodes::{LeaseRenewal, LeaseState, NodeSnapshot};
use orkestr_security::audit::NewAudit;
use orkestr_task_engine::jobs::{JobDispatcher, JobFilter, JobStore};
use parking_lot::Mutex;
use tokio::sync::Notify;

use super::lease::{Lease, LeaseBook, LeaseConfig, PolicySigner};
use super::worker::sha_digest;
use crate::services::Services;

/// Errors of the coordinator job driver.
#[derive(Debug, thiserror::Error)]
pub enum JobsError {
    /// Unknown job.
    #[error("job {0} not found")]
    NotFound(JobId),
    /// No link to the node.
    #[error("node {0} is not connected")]
    NoLink(NodeId),
    /// Placement did not produce a node.
    #[error("job cannot be placed: {0}")]
    NotPlaced(String),
    /// The worker refused the offer.
    #[error("offer rejected by node {node}: {reason}")]
    Rejected {
        /// Node.
        node: NodeId,
        /// Reason.
        reason: String,
    },
    /// The job is not in a state allowing the operation.
    #[error("{0}")]
    State(String),
    /// Transport / RPC.
    #[error(transparent)]
    Node(#[from] NodeError),
    /// Blob transfer.
    #[error(transparent)]
    Blob(#[from] BlobError),
    /// Anything else.
    #[error("{0}")]
    Internal(String),
}

impl From<JobsError> for orkestr_protocol::RpcError {
    fn from(e: JobsError) -> Self {
        match e {
            JobsError::NotFound(_) => Self::not_found(e.to_string()),
            JobsError::State(_) | JobsError::Rejected { .. } => Self::conflict(e.to_string()),
            JobsError::NotPlaced(_) | JobsError::NoLink(_) => Self::conflict(e.to_string()),
            _ => Self::internal(e.to_string()),
        }
    }
}

/// Links to worker nodes (WP-A registry / connector).
pub trait LinkProvider: Send + Sync {
    /// Link to `node`, when connected.
    fn link(&self, node: NodeId) -> Option<Arc<dyn NodeLink>>;
    /// Current node snapshots (placement input).
    fn snapshots(&self) -> Vec<NodeSnapshot>;
    /// Every known node (emergency broadcast); defaults to the snapshot ids.
    fn node_ids(&self) -> Vec<NodeId> {
        self.snapshots().iter().map(|s| s.node.id).collect()
    }
}

/// Placement with exclusions (re-placement after node loss excludes the old node).
pub trait JobPlacer: Send + Sync {
    /// Places `spec`, never on a node in `exclude`.
    fn place(&self, spec: &JobSpec, exclude: &[NodeId]) -> Placement;
}

/// Moves blobs between the coordinator store and workers.
#[async_trait]
pub trait BlobTransfer: Send + Sync {
    /// Coordinator в†’ worker.
    async fn push(&self, src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<(), BlobError>;
    /// Worker в†’ coordinator (hash verified by `dst` on finalize).
    async fn fetch(&self, link: &dyn NodeLink, d: &Digest, dst: &dyn BlobStore) -> Result<(), BlobError>;
}

type LinkEntry = (Arc<dyn NodeLink>, Option<NodeSnapshot>);

/// Static link set (tests; also usable as a simple registry adapter).
#[derive(Default)]
pub struct StaticLinks {
    links: Mutex<Vec<LinkEntry>>,
}

impl std::fmt::Debug for StaticLinks {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StaticLinks").finish_non_exhaustive()
    }
}

impl StaticLinks {
    /// Adds / replaces a node (`snapshot` feeds scheduler-based placement).
    pub fn add(&self, link: Arc<dyn NodeLink>, snapshot: Option<NodeSnapshot>) {
        let mut l = self.links.lock();
        l.retain(|(x, _)| x.node_id() != link.node_id());
        l.push((link, snapshot));
    }

    /// Removes a node.
    pub fn remove(&self, node: NodeId) {
        self.links.lock().retain(|(x, _)| x.node_id() != node);
    }
}

impl LinkProvider for StaticLinks {
    fn link(&self, node: NodeId) -> Option<Arc<dyn NodeLink>> {
        self.links.lock().iter().find(|(l, _)| l.node_id() == node).map(|(l, _)| Arc::clone(l))
    }

    fn snapshots(&self) -> Vec<NodeSnapshot> {
        self.links.lock().iter().filter_map(|(_, s)| s.clone()).collect()
    }

    fn node_ids(&self) -> Vec<NodeId> {
        self.links.lock().iter().map(|(l, _)| l.node_id()).collect()
    }
}

/// Deterministic placer over an ordered candidate list: the first node that
/// is not excluded / forbidden (tests and single-target setups).
#[derive(Debug, Default)]
pub struct FirstFitPlacer(pub Mutex<Vec<NodeId>>);

impl JobPlacer for FirstFitPlacer {
    fn place(&self, spec: &JobSpec, exclude: &[NodeId]) -> Placement {
        let ids = self.0.lock().clone();
        match ids.into_iter().find(|n| !exclude.contains(n) && !spec.affinity.forbidden.contains(n)) {
            Some(node_id) => {
                Placement::Place { node_id, score: 1.0, explanation: vec![format!("first fit: {node_id}")] }
            }
            None => Placement::Wait {
                reason: jm::JobWaitReason::NodesUnavailable,
                explanation: vec!["no eligible node".into()],
            },
        }
    }
}

/// Placer over a [`NodeScheduler`] (WP-B) with exclusions applied to the snapshots.
pub struct SchedulerPlacer {
    /// Scheduler.
    pub scheduler: Arc<dyn NodeScheduler>,
    /// Node source.
    pub links: Arc<dyn LinkProvider>,
    /// Policy.
    pub policy: PlacementPolicy,
}

impl JobPlacer for SchedulerPlacer {
    fn place(&self, spec: &JobSpec, exclude: &[NodeId]) -> Placement {
        let nodes: Vec<NodeSnapshot> =
            self.links.snapshots().into_iter().filter(|s| !exclude.contains(&s.node.id)).collect();
        self.scheduler.place(spec, &nodes, &self.policy)
    }
}

/// Blob transfer over the node RPCs (`push_blob` from WP-D; `node.blobGet` chunks).
#[derive(Debug, Default)]
pub struct RpcBlobTransfer;

#[async_trait]
impl BlobTransfer for RpcBlobTransfer {
    async fn push(&self, src: &dyn BlobStore, link: &dyn NodeLink, d: &Digest) -> Result<(), BlobError> {
        orkestr_node::push_blob(src, link, d).await
    }

    async fn fetch(&self, link: &dyn NodeLink, d: &Digest, dst: &dyn BlobStore) -> Result<(), BlobError> {
        if dst.has(d).await {
            return Ok(());
        }
        let mut offset = 0u64;
        loop {
            let p = BlobGetParams { digest: d.clone(), offset, len: BLOB_CHUNK_SIZE };
            let r = call_typed::<orkestr_protocol::methods::models::BlobGet>(link, &p)
                .await
                .map_err(|e| BlobError::Transfer(e.to_string()))?;
            let bytes = STANDARD.decode(r.data.as_bytes()).map_err(|e| BlobError::Transfer(e.to_string()))?;
            if bytes.is_empty() && offset < r.total {
                return Err(BlobError::Transfer("empty chunk".into()));
            }
            offset = dst.put_chunk(d, offset, r.total, &bytes).await?;
            if offset >= r.total {
                return Ok(());
            }
        }
    }
}

/// Coordinator-side bookkeeping of one job.
#[derive(Debug, Clone)]
pub(super) struct JobEntry {
    pub(super) view: JobView,
    pub(super) worktree: Option<PathBuf>,
    pub(super) inputs: JobInputs,
    /// Side effects reported since the last checkpoint (unplanned-migration gate).
    pub(super) effects_since_checkpoint: Vec<SideEffectRecord>,
    pub(super) last_checkpoint: Option<JobCheckpoint>,
    pub(super) excluded: Vec<NodeId>,
}

/// Coordinator configuration.
#[derive(Debug, Clone)]
pub struct CoordinatorConfig {
    /// This (coordinator) node.
    pub node_id: NodeId,
    /// Lease timings.
    pub lease: LeaseConfig,
    /// Policy snapshot template (signed per offer).
    pub policy: PolicySnapshot,
}

/// The coordinator job driver.
pub struct CoordinatorJobs {
    pub(super) services: Arc<Services>,
    pub(super) cfg: CoordinatorConfig,
    pub(super) links: Arc<dyn LinkProvider>,
    pub(super) placer: Arc<dyn JobPlacer>,
    pub(super) transfer: Arc<dyn BlobTransfer>,
    pub(super) blobs: Arc<dyn BlobStore>,
    pub(super) store: JobStore,
    pub(super) signer: PolicySigner,
    /// Leases (epochs, renewals, expiry).
    pub leases: LeaseBook,
    pub(super) jobs: Mutex<HashMap<JobId, JobEntry>>,
    attached: Mutex<HashSet<NodeId>>,
    pending_revokes: Mutex<HashMap<NodeId, Vec<JobId>>>,
    stale_dropped: AtomicU64,
    stale_audited: Mutex<HashSet<(JobId, NodeId, LeaseEpoch)>>,
    changed: Notify,
}

impl std::fmt::Debug for CoordinatorJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoordinatorJobs").field("node_id", &self.cfg.node_id).finish_non_exhaustive()
    }
}

static COORDINATORS: LazyLock<Mutex<HashMap<NodeId, Arc<CoordinatorJobs>>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

/// Registers the coordinator driver of the core `node` (used by `jobs.migrate` / `jobs.cancel`).
pub fn install(node: NodeId, c: Arc<CoordinatorJobs>) {
    COORDINATORS.lock().insert(node, c);
}

/// The coordinator driver of the core `node`, when installed.
#[must_use]
pub fn coordinator(node: NodeId) -> Option<Arc<CoordinatorJobs>> {
    COORDINATORS.lock().get(&node).cloned()
}

pub(super) fn now() -> Timestamp {
    chrono::Utc::now()
}

/// Builder arguments of [`CoordinatorJobs::new`].
pub struct CoordinatorDeps {
    /// Services of the coordinator core.
    pub services: Arc<Services>,
    /// Config.
    pub cfg: CoordinatorConfig,
    /// Links.
    pub links: Arc<dyn LinkProvider>,
    /// Placement.
    pub placer: Arc<dyn JobPlacer>,
    /// Blob transfer.
    pub transfer: Arc<dyn BlobTransfer>,
    /// Coordinator blob store.
    pub blobs: Arc<dyn BlobStore>,
    /// Policy signer (coordinator CIK).
    pub signer: PolicySigner,
}

impl CoordinatorJobs {
    /// New driver.
    #[must_use]
    pub fn new(d: CoordinatorDeps) -> Arc<Self> {
        let store = crate::rpc::jobs_ui::job_store(&d.services);
        Arc::new(Self {
            services: d.services,
            cfg: d.cfg,
            links: d.links,
            placer: d.placer,
            transfer: d.transfer,
            blobs: d.blobs,
            store,
            signer: d.signer,
            leases: LeaseBook::new(),
            jobs: Mutex::new(HashMap::new()),
            attached: Mutex::new(HashSet::new()),
            pending_revokes: Mutex::new(HashMap::new()),
            stale_dropped: AtomicU64::new(0),
            stale_audited: Mutex::new(HashSet::new()),
            changed: Notify::new(),
        })
    }

    /// Number of progress / checkpoint messages dropped for a stale epoch.
    #[must_use]
    pub fn stale_dropped(&self) -> u64 {
        self.stale_dropped.load(Ordering::SeqCst)
    }

    /// Current view of a job.
    #[must_use]
    pub fn get(&self, job: JobId) -> Option<JobView> {
        self.jobs.lock().get(&job).map(|e| e.view.clone())
    }

    /// Every job view.
    #[must_use]
    pub fn list(&self) -> Vec<JobView> {
        self.jobs.lock().values().map(|e| e.view.clone()).collect()
    }

    /// Latest checkpoint of a job.
    #[must_use]
    pub fn last_checkpoint(&self, job: JobId) -> Option<JobCheckpoint> {
        self.jobs.lock().get(&job).and_then(|e| e.last_checkpoint.clone())
    }

    /// Waits until `pred` holds for the job's view (or `timeout` elapses).
    pub async fn wait_for(
        &self,
        job: JobId,
        timeout: std::time::Duration,
        pred: impl Fn(&JobView) -> bool,
    ) -> Option<JobView> {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.changed.notified();
            if let Some(v) = self.get(job) {
                if pred(&v) {
                    return Some(v);
                }
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return self.get(job).filter(|v| pred(v));
            }
        }
    }

    pub(super) async fn update<R>(&self, job: JobId, f: impl FnOnce(&mut JobEntry) -> R) -> Option<R> {
        let (r, view) = {
            let mut jobs = self.jobs.lock();
            let e = jobs.get_mut(&job)?;
            let r = f(e);
            e.view.updated_at = now();
            (r, e.view.clone())
        };
        let epoch = view.epoch;
        let persisted = self
            .store
            .update_fenced(job, epoch, move |s| {
                s.state = view.state;
                s.placement = view.placement;
                s.last_checkpoint = view.last_checkpoint;
                s.progress = view.progress;
                s.exit_code = view.exit_code;
                s.detail = view.detail;
                s.artifacts = view.artifacts;
            })
            .await;
        if let Err(e) = persisted {
            tracing::debug!(job_id = %job, error = %e, "job row not updated");
        }
        self.changed.notify_waiters();
        Some(r)
    }

    pub(super) async fn audit(
        &self,
        action: &str,
        job: JobId,
        node: NodeId,
        outcome: AuditOutcome,
        details: serde_json::Value,
    ) {
        let project = self.jobs.lock().get(&job).map(|e| e.view.spec.project_id);
        let mut d = details;
        if let Some(obj) = d.as_object_mut() {
            obj.insert("nodeId".into(), serde_json::json!(node));
            obj.insert("jobId".into(), serde_json::json!(job));
        }
        self.services
            .security
            .audit
            .record(
                NewAudit::new(Actor::System, action, format!("node:{node}/job:{job}"), outcome)
                    .project(project)
                    .details(d),
            )
            .await;
    }

    pub(super) async fn publish(&self, kind: EventKind, node: Option<NodeId>) {
        let mut ev = NewEvent::new(kind);
        if let Some(n) = node {
            ev = ev.node(n);
        }
        if let Err(e) = self.services.events.publish(ev).await {
            tracing::warn!(error = %e, "job event publish failed");
        }
    }

    /// Best-effort task transition through the task engine (the only status writer).
    pub(super) async fn task_to(&self, job: JobId, to: TaskStatus, actor: TaskActor, reason: &str) -> bool {
        let Some(task_id) = self.jobs.lock().get(&job).map(|e| e.view.spec.task_id) else { return false };
        match self.services.tasks.transition(task_id, to, actor, Some(reason.to_owned()), None).await {
            Ok(_) => true,
            Err(e) => {
                tracing::debug!(task_id = %task_id, ?to, error = %e, "task transition skipped");
                false
            }
        }
    }

    pub(super) async fn task_status(&self, job: JobId) -> Option<TaskStatus> {
        let task_id = self.jobs.lock().get(&job).map(|e| e.view.spec.task_id)?;
        self.services.tasks.get(task_id).await.ok().map(|t| t.status)
    }

    /// Prepares workspace inputs from a coordinator worktree: git bundle +
    /// binary patch of the dirty tree, both stored as blobs.
    ///
    /// # Errors
    /// git / blob store failures.
    pub async fn workspace_inputs(&self, worktree: &std::path::Path) -> Result<GitStateDescriptor, JobsError> {
        let git = self.services.vcs.git();
        let head = git
            .rev_parse(worktree, "HEAD")
            .await
            .map_err(|e| JobsError::Internal(e.to_string()))?
            .ok_or_else(|| JobsError::Internal("worktree has no commits".into()))?;
        let branch = git.current_branch(worktree).await.ok().flatten();
        let tmp = self.services.paths.data_dir.join("jobs-out");
        tokio::fs::create_dir_all(&tmp).await.map_err(|e| JobsError::Internal(e.to_string()))?;
        let bundle_path = tmp.join(format!("{}.bundle", sha_digest(head.as_bytes()).hex()));
        git.create_bundle(worktree, &bundle_path).await.map_err(|e| JobsError::Internal(e.to_string()))?;
        let bundle = self.blobs.put_file(&bundle_path).await?;
        let _ = tokio::fs::remove_file(&bundle_path).await;
        let patch = git.working_patch(worktree, &head).await.map_err(|e| JobsError::Internal(e.to_string()))?;
        let dirty = if patch.is_empty() {
            None
        } else {
            let p = tmp.join(format!("{}.patch", sha_digest(&patch).hex()));
            tokio::fs::write(&p, &patch).await.map_err(|e| JobsError::Internal(e.to_string()))?;
            let d = self.blobs.put_file(&p).await?;
            let _ = tokio::fs::remove_file(&p).await;
            Some(d)
        };
        Ok(GitStateDescriptor { branch, head_commit: Some(head), bundle: Some(bundle), dirty_tar: dirty })
    }

    /// Loads the runtime entry of a job from the jobs table (WP-B) when it
    /// is not known yet; the task's worktree (if any) is shipped as inputs.
    pub(super) async fn ensure_entry(&self, job: JobId) -> Result<(), JobsError> {
        if self.jobs.lock().contains_key(&job) {
            return Ok(());
        }
        let view = self.store.get(job).await.map_err(store_err)?;
        let worktree = self
            .services
            .tasks
            .get(view.spec.task_id)
            .await
            .ok()
            .and_then(|t| t.worktree)
            .map(|w| PathBuf::from(w.path))
            .filter(|p| p.join(".git").exists());
        let git = match &worktree {
            Some(w) => self.workspace_inputs(w).await?,
            None => GitStateDescriptor::default(),
        };
        self.jobs.lock().entry(job).or_insert(JobEntry {
            view,
            worktree,
            inputs: JobInputs { git, ..JobInputs::default() },
            effects_since_checkpoint: Vec::new(),
            last_checkpoint: None,
            excluded: Vec::new(),
        });
        Ok(())
    }

    /// Submits a job for `spec` (task must exist on this coordinator): creates
    /// the job row, places it and offers it. With a `worktree`, its state is
    /// shipped to the worker and the result diff is applied back into it.
    /// (`jobs.submit` of WP-B places itself and hands over through
    /// [`CoordinatorJobs::dispatcher`].)
    ///
    /// # Errors
    /// Workspace preparation / storage; placement / offer errors are reflected in the view.
    pub async fn submit(self: &Arc<Self>, spec: JobSpec, worktree: Option<PathBuf>) -> Result<JobView, JobsError> {
        let git = match &worktree {
            Some(w) => self.workspace_inputs(w).await?,
            None => GitStateDescriptor::default(),
        };
        let job = spec.job_id;
        let view = self.store.create(spec, None).await.map_err(store_err)?;
        self.jobs.lock().insert(
            job,
            JobEntry {
                view,
                worktree,
                inputs: JobInputs { git, ..JobInputs::default() },
                effects_since_checkpoint: Vec::new(),
                last_checkpoint: None,
                excluded: Vec::new(),
            },
        );
        if let Err(e) = self.place_and_dispatch(job, Vec::new(), None).await {
            tracing::info!(job_id = %job, error = %e, "job not started yet");
        }
        self.get(job).ok_or(JobsError::NotFound(job))
    }

    /// Places (excluding `exclude`) and offers; on `Wait` / `Infeasible` the
    /// view records the placement. `resume` carries the checkpoint + origin of a migration.
    pub(super) async fn place_and_dispatch(
        self: &Arc<Self>,
        job: JobId,
        exclude: Vec<NodeId>,
        resume: Option<(JobCheckpoint, NodeId, String)>,
    ) -> Result<(), JobsError> {
        let (spec, mut excl) = {
            let jobs = self.jobs.lock();
            let e = jobs.get(&job).ok_or(JobsError::NotFound(job))?;
            (e.view.spec.clone(), e.excluded.clone())
        };
        excl.extend(exclude);
        let placement = self.placer.place(&spec, &excl);
        match &placement {
            Placement::Place { node_id, .. } => {
                let node = *node_id;
                self.update(job, |e| e.view.placement = Some(placement.clone())).await;
                let r = self.dispatch(job, node, resume).await;
                if let Err(e) = &r {
                    let detail = e.to_string();
                    self.update(job, |en| {
                        en.view.state = JobState::Pending;
                        en.view.detail = Some(detail);
                    })
                    .await;
                }
                r
            }
            Placement::Wait { .. } => {
                self.update(job, |e| {
                    e.view.placement = Some(placement.clone());
                    e.view.state = JobState::Pending;
                })
                .await;
                Ok(())
            }
            Placement::Infeasible { reason, .. } => {
                let reason = reason.clone();
                self.update(job, |e| {
                    e.view.placement = Some(placement.clone());
                    e.view.state = JobState::Infeasible;
                    e.view.detail = Some(reason.clone());
                })
                .await;
                let why = truncate_reason(&format!("job infeasible: {reason}"));
                self.task_to(job, TaskStatus::Blocked, TaskActor::System, &why).await;
                Err(JobsError::NotPlaced(reason))
            }
        }
    }

    /// Assigns `job` to `node` under a fresh epoch (job row `assign`, which
    /// publishes `JobAssigned`) and offers it.
    pub(super) async fn dispatch(
        self: &Arc<Self>,
        job: JobId,
        node: NodeId,
        resume: Option<(JobCheckpoint, NodeId, String)>,
    ) -> Result<(), JobsError> {
        if self.links.link(node).is_none() {
            return Err(JobsError::NoLink(node));
        }
        let persisted = self.store.get(job).await.map_err(store_err)?.epoch;
        let epoch = self.leases.next_epoch(job, persisted);
        let stored = self.store.assign(job, node, epoch).await.map_err(store_err)?;
        self.update(job, |e| {
            e.view.epoch = stored.epoch;
            e.view.owner = stored.owner;
            e.view.state = JobState::Offered;
        })
        .await;
        self.offer(job, node, epoch, resume).await
    }

    /// Delivers a job that the jobs table already assigned (WP-B `jobs.submit`).
    ///
    /// # Errors
    /// Unknown job, not assigned to a node, offer failure.
    pub async fn dispatch_assigned(self: &Arc<Self>, job: JobId) -> Result<(), JobsError> {
        self.ensure_entry(job).await?;
        let view = self.store.get(job).await.map_err(store_err)?;
        let Some(ExecutorRef::Node { node_id: node }) = view.owner else {
            return Err(JobsError::State("job is not assigned to a node".into()));
        };
        self.update(job, |e| {
            e.view.epoch = view.epoch;
            e.view.owner = view.owner;
            e.view.state = view.state;
            e.view.placement.clone_from(&view.placement);
        })
        .await;
        self.leases.observe_epoch(job, view.epoch);
        self.offer(job, node, view.epoch, None).await
    }

    /// The [`JobDispatcher`] to install in WP-B's `JobsUiHooks`.
    #[must_use]
    pub fn dispatcher(self: &Arc<Self>) -> Arc<dyn JobDispatcher> {
        Arc::new(CoordinatorDispatcher(Arc::downgrade(self)))
    }

    /// Offers `job` to `node` at `epoch` (pushes input blobs first).
    async fn offer(
        self: &Arc<Self>,
        job: JobId,
        node: NodeId,
        epoch: LeaseEpoch,
        resume: Option<(JobCheckpoint, NodeId, String)>,
    ) -> Result<(), JobsError> {
        let link = self.links.link(node).ok_or(JobsError::NoLink(node))?;
        self.attach(&link);
        let (spec, mut inputs) = {
            let jobs = self.jobs.lock();
            let e = jobs.get(&job).ok_or(JobsError::NotFound(job))?;
            (e.view.spec.clone(), e.inputs.clone())
        };
        if let Some((cp, _, _)) = &resume {
            inputs.handoff.clone_from(&cp.handoff);
            inputs.checkpoint = Some(cp.clone());
        }
        let git = inputs.checkpoint.as_ref().map_or(&inputs.git, |c| &c.git);
        let mut to_push: Vec<Digest> = [git.bundle.clone(), git.dirty_tar.clone()].into_iter().flatten().collect();
        to_push.extend(inputs.blobs.iter().cloned());
        for d in &to_push {
            self.transfer.push(self.blobs.as_ref(), link.as_ref(), d).await?;
        }
        let lease_until = self.cfg.lease.until(now());
        let mut policy = self.cfg.policy.clone();
        policy.issued_at = Some(now());
        self.signer.sign(job, epoch, &mut policy);
        let offer =
            JobOffer { spec: spec.clone(), epoch, lease_until, inputs, policy, coordinator_node_id: self.cfg.node_id };
        // grant before the call so early progress is not fenced out
        self.leases.grant(job, Lease { node_id: node, epoch, until: lease_until });
        self.audit("jobs.offer", job, node, AuditOutcome::Allowed, serde_json::json!({"epoch": epoch})).await;
        match call_typed::<jm::Offer>(link.as_ref(), &offer).await {
            Ok(r) if r.accepted => {}
            Ok(r) => {
                self.leases.release(job);
                let reason = r.reason.unwrap_or_default();
                self.audit(
                    "jobs.reject",
                    job,
                    node,
                    AuditOutcome::Rejected,
                    serde_json::json!({"epoch": epoch, "reason": reason}),
                )
                .await;
                return Err(JobsError::Rejected { node, reason });
            }
            Err(e) => {
                self.leases.release(job);
                self.audit(
                    "jobs.offer",
                    job,
                    node,
                    AuditOutcome::Failed,
                    serde_json::json!({"epoch": epoch, "error": e.to_string()}),
                )
                .await;
                return Err(e.into());
            }
        }
        self.update(job, |e| {
            e.view.state = JobState::Running;
            e.view.detail = None;
            e.effects_since_checkpoint.clear();
        })
        .await;
        if let Err(e) = self.store.set_lease_until(job, epoch, Some(lease_until)).await {
            tracing::debug!(job_id = %job, error = %e, "lease end not persisted");
        }
        if let Err(e) = self.store.set_task_owner(spec.task_id, Some(ExecutorRef::Node { node_id: node })).await {
            tracing::warn!(job_id = %job, error = %e, "task owner not set");
        }
        self.audit("jobs.accept", job, node, AuditOutcome::Executed, serde_json::json!({"epoch": epoch})).await;
        match self.task_status(job).await {
            Some(TaskStatus::Recovering) => {
                self.task_to(job, TaskStatus::Running, TaskActor::SystemRecovery, "resumed on node").await;
            }
            Some(TaskStatus::Planned) => {
                self.task_to(job, TaskStatus::Ready, TaskActor::System, "assigned to node").await;
                self.task_to(job, TaskStatus::Running, TaskActor::System, "assigned to node").await;
            }
            Some(TaskStatus::Ready) => {
                self.task_to(job, TaskStatus::Running, TaskActor::System, "assigned to node").await;
            }
            _ => {}
        }
        if let Some((cp, from, reason)) = resume {
            self.publish(
                EventKind::JobMigrated(JobMigratedPayload {
                    job_id: job,
                    task_id: spec.task_id,
                    from_node_id: from,
                    to_node_id: node,
                    epoch,
                    checkpoint_id: Some(cp.checkpoint_id),
                    reason: reason.clone(),
                }),
                Some(node),
            )
            .await;
            self.audit(
                "jobs.migrate",
                job,
                node,
                AuditOutcome::Executed,
                serde_json::json!({"from": from, "epoch": epoch, "reason": reason}),
            )
            .await;
        }
        Ok(())
    }

    /// Re-places `Pending` jobs (placement `Wait`, failed offers, pending migrations).
    pub async fn replace_pending(self: &Arc<Self>) {
        let rows = self.store.list(JobFilter { active_only: true, ..JobFilter::default() }).await.unwrap_or_default();
        for v in rows.into_iter().filter(|v| v.state == JobState::Pending) {
            let job = v.spec.job_id;
            if self.ensure_entry(job).await.is_err() {
                continue;
            }
            let resume = self.last_checkpoint(job).map(|c| {
                let from = c.node_id;
                (c, from, "re-placed".to_owned())
            });
            let _ = self.place_and_dispatch(job, Vec::new(), resume).await;
        }
    }

    /// Starts consuming the worker's `jobs` events (once per node).
    pub fn attach(self: &Arc<Self>, link: &Arc<dyn NodeLink>) {
        let node = link.node_id();
        if !self.attached.lock().insert(node) {
            return;
        }
        let mut rx = link.events();
        let this = Arc::downgrade(self);
        let link = Arc::clone(link);
        tokio::spawn(async move {
            loop {
                let ev = match rx.recv().await {
                    Ok(ev) => ev,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(node_id = %node, n, "job events lagged");
                        continue;
                    }
                    Err(_) => break,
                };
                let Some(this) = this.upgrade() else { break };
                match ev.kind {
                    EventKind::JobProgress(p) => this.on_progress(&link, p).await,
                    EventKind::JobCheckpointed(c) => this.on_checkpoint(&link, *c.checkpoint).await,
                    _ => {}
                }
            }
            if let Some(this) = this.upgrade() {
                this.attached.lock().remove(&node);
            }
        });
    }

    async fn fenced(&self, job: JobId, node: NodeId, epoch: LeaseEpoch) -> bool {
        if self.leases.is_current(job, node, epoch) {
            return false;
        }
        self.stale_dropped.fetch_add(1, Ordering::SeqCst);
        tracing::debug!(job_id = %job, node_id = %node, epoch = epoch.0, "stale job message dropped");
        if self.stale_audited.lock().insert((job, node, epoch)) {
            let current = self.leases.get(job).map(|l| l.epoch.0);
            self.audit(
                "jobs.staleEpochFenced",
                job,
                node,
                AuditOutcome::Denied,
                serde_json::json!({"epoch": epoch, "currentEpoch": current}),
            )
            .await;
        }
        true
    }

    /// Intake of one `JobProgress` from `link` (the link's node id is authoritative).
    pub async fn on_progress(self: &Arc<Self>, link: &Arc<dyn NodeLink>, p: JobProgressPayload) {
        let node = link.node_id();
        if self.fenced(p.job_id, node, p.epoch).await {
            return;
        }
        let job = p.job_id;
        let mut finished = None;
        self.update(job, |e| {
            for item in &p.items {
                match item {
                    JobProgressItem::Progress { fraction } => e.view.progress = Some(*fraction),
                    JobProgressItem::SideEffect { record } => e.effects_since_checkpoint.push(record.clone()),
                    JobProgressItem::Finished { success, exit_code, artifacts, detail } => {
                        finished = Some((*success, *exit_code, artifacts.clone(), detail.clone()));
                    }
                    _ => {}
                }
            }
        })
        .await;
        let mut relay = p.clone();
        relay.node_id = node;
        self.publish(EventKind::JobProgress(relay), Some(node)).await;
        if let Some((success, exit_code, artifacts, detail)) = finished {
            self.complete(link, job, success, exit_code, artifacts, detail).await;
        }
    }

    /// Intake of one `JobCheckpointed`: stores it and pulls its blobs eagerly
    /// (so an unplanned migration can resume after the node is gone).
    pub async fn on_checkpoint(self: &Arc<Self>, link: &Arc<dyn NodeLink>, cp: JobCheckpoint) {
        let node = link.node_id();
        if self.fenced(cp.job_id, node, cp.epoch).await {
            return;
        }
        if let Err(e) = self.fetch_checkpoint_blobs(link, &cp).await {
            tracing::warn!(job_id = %cp.job_id, error = %e, "checkpoint blobs unavailable");
            return;
        }
        self.record_checkpoint(cp.clone()).await;
        self.publish(
            EventKind::JobCheckpointed(JobCheckpointedPayload { checkpoint: Box::new(cp.clone()) }),
            Some(node),
        )
        .await;
        self.audit(
            "jobs.checkpoint",
            cp.job_id,
            node,
            AuditOutcome::Executed,
            serde_json::json!({"epoch": cp.epoch, "checkpointId": cp.checkpoint_id}),
        )
        .await;
    }

    pub(super) async fn record_checkpoint(&self, cp: JobCheckpoint) {
        let job = cp.job_id;
        self.update(job, |e| {
            e.view.last_checkpoint = Some(cp.checkpoint_id);
            e.effects_since_checkpoint.clear();
            e.last_checkpoint = Some(cp);
        })
        .await;
    }

    pub(super) async fn fetch_checkpoint_blobs(
        &self,
        link: &Arc<dyn NodeLink>,
        cp: &JobCheckpoint,
    ) -> Result<(), JobsError> {
        for d in [cp.git.bundle.as_ref(), cp.git.dirty_tar.as_ref()].into_iter().flatten() {
            self.transfer.fetch(link.as_ref(), d, self.blobs.as_ref()).await?;
        }
        Ok(())
    }

    async fn complete(
        self: &Arc<Self>,
        link: &Arc<dyn NodeLink>,
        job: JobId,
        success: bool,
        exit_code: Option<i32>,
        artifacts: Vec<Digest>,
        detail: Option<String>,
    ) {
        let node = link.node_id();
        self.leases.release(job);
        let mut ok = success;
        let mut detail = detail;
        if success {
            for d in &artifacts {
                let fetched = self.transfer.fetch(link.as_ref(), d, self.blobs.as_ref()).await;
                if let Err(e) = fetched.map_err(JobsError::from).and(if self.blobs.has(d).await {
                    Ok(())
                } else {
                    Err(JobsError::Internal(format!("artifact {d} missing after fetch")))
                }) {
                    ok = false;
                    detail = Some(format!("artifact {d}: {e}"));
                    break;
                }
            }
            if ok {
                if let Err(e) = self.apply_result(job).await {
                    ok = false;
                    detail = Some(format!("result diff not applied: {e}"));
                }
            }
        }
        let outcome = if ok { AuditOutcome::Executed } else { AuditOutcome::Failed };
        self.audit("jobs.complete", job, node, outcome, serde_json::json!({"success": ok, "exitCode": exit_code}))
            .await;
        if ok {
            self.task_to(job, TaskStatus::Completed, TaskActor::System, "remote job completed").await;
        } else {
            let reason = detail.clone().unwrap_or_else(|| "remote job failed".into());
            self.task_to(job, TaskStatus::Failed, TaskActor::System, &truncate_reason(&reason)).await;
        }
        self.update(job, |e| {
            e.view.state = if ok { JobState::Completed } else { JobState::Failed };
            e.view.exit_code = exit_code;
            e.view.artifacts.clone_from(&artifacts);
            e.view.progress = Some(1.0);
            e.view.detail.clone_from(&detail);
        })
        .await;
    }

    /// Applies the final checkpoint's diff into the coordinator worktree: the
    /// shipped dirty patch is reverted first, then the worker's patch (both
    /// relative to the same base commit) is applied.
    async fn apply_result(&self, job: JobId) -> Result<(), JobsError> {
        let (worktree, inputs, cp) = {
            let jobs = self.jobs.lock();
            let e = jobs.get(&job).ok_or(JobsError::NotFound(job))?;
            (e.worktree.clone(), e.inputs.clone(), e.last_checkpoint.clone())
        };
        let (Some(wt), Some(cp)) = (worktree, cp) else { return Ok(()) };
        if cp.git.head_commit != inputs.git.head_commit {
            return Err(JobsError::State("the worker created commits; fetch the result bundle manually".into()));
        }
        let git = self.services.vcs.git();
        let read = |d: &Digest| self.blobs.path(d).ok_or_else(|| JobsError::Internal(format!("blob {d} missing")));
        if let Some(d) = &inputs.git.dirty_tar {
            let shipped = tokio::fs::read(read(d)?).await.map_err(|e| JobsError::Internal(e.to_string()))?;
            git.revert_patch(&wt, &shipped).await.map_err(|e| JobsError::Internal(e.to_string()))?;
        }
        if let Some(d) = &cp.git.dirty_tar {
            let result = tokio::fs::read(read(d)?).await.map_err(|e| JobsError::Internal(e.to_string()))?;
            git.apply_patch(&wt, &result).await.map_err(|e| JobsError::Internal(e.to_string()))?;
        }
        Ok(())
    }

    // --- leases / heartbeat ---------------------------------------------------

    /// Renewals + revocations to send in the next `node.heartbeat` to `node`.
    #[must_use]
    pub fn heartbeat_params(&self, node: NodeId) -> (Vec<LeaseRenewal>, Vec<JobId>) {
        let renew = self.leases.renewals_for(node, now(), &self.cfg.lease);
        let revoke = self.pending_revokes.lock().get(&node).cloned().unwrap_or_default();
        (renew, revoke)
    }

    /// After a successful heartbeat: commits the renewals and reconciles the
    /// node's reported leases (unknown / stale jobs are revoked next time).
    pub fn heartbeat_ok(&self, node: NodeId, renewed: &[LeaseRenewal], revoked: &[JobId], states: &[LeaseState]) {
        self.leases.commit_renewals(node, renewed);
        let mut pending = self.pending_revokes.lock();
        let list = pending.entry(node).or_default();
        list.retain(|j| !revoked.contains(j));
        for s in states {
            let active = matches!(
                s.phase,
                orkestr_protocol::methods::nodes::LeasePhase::Starting
                    | orkestr_protocol::methods::nodes::LeasePhase::Running
                    | orkestr_protocol::methods::nodes::LeasePhase::Draining
            );
            if active && !self.leases.is_current(s.job_id, node, s.epoch) && !list.contains(&s.job_id) {
                list.push(s.job_id);
            }
        }
    }

    pub(super) fn queue_revoke(&self, node: NodeId, job: JobId) {
        let mut p = self.pending_revokes.lock();
        let l = p.entry(node).or_default();
        if !l.contains(&job) {
            l.push(job);
        }
    }

    /// Expired leases в†’ unplanned migration. Returns the affected jobs.
    pub async fn tick(self: &Arc<Self>) -> Vec<JobId> {
        let expired = self.leases.expired(now());
        let mut out = Vec::new();
        for (job, lease) in expired {
            out.push(job);
            self.recover_unplanned(job, lease, "node_lost: lease expired").await;
        }
        self.replace_pending().await;
        out
    }

    /// Spawns the lease-expiry loop (every heartbeat interval) and the
    /// emergency-stop watcher.
    pub fn spawn_lease_loop(self: &Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let this = Arc::downgrade(self);
        let every = self.cfg.lease.heartbeat.min(std::time::Duration::from_secs(1));
        let sd = shutdown.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = sd.cancelled() => break,
                    () = tokio::time::sleep(every) => {}
                }
                let Some(this) = this.upgrade() else { break };
                this.tick().await;
            }
        });
        self.spawn_emergency_watch(shutdown);
    }

    // --- cancel / emergency ---------------------------------------------------

    /// `jobs.cancel`: cancels on the owning node and marks the task cancelled.
    ///
    /// # Errors
    /// Unknown job.
    pub async fn cancel(self: &Arc<Self>, job: JobId, immediate: bool) -> Result<JobView, JobsError> {
        let view = self.get(job).ok_or(JobsError::NotFound(job))?;
        if let Some(lease) = self.leases.release(job) {
            if let Some(link) = self.links.link(lease.node_id) {
                let p = WorkerJobParams { job_id: job, epoch: lease.epoch, immediate };
                if let Err(e) = call_typed::<jm::JobCancel>(link.as_ref(), &p).await {
                    tracing::warn!(job_id = %job, error = %e, "remote cancel failed; revoking at next heartbeat");
                    self.queue_revoke(lease.node_id, job);
                }
            } else {
                self.queue_revoke(lease.node_id, job);
            }
            self.audit(
                "jobs.cancel",
                job,
                lease.node_id,
                AuditOutcome::Executed,
                serde_json::json!({"immediate": immediate, "epoch": lease.epoch}),
            )
            .await;
        }
        if !matches!(view.state, JobState::Completed | JobState::Failed | JobState::Cancelled) {
            self.task_to(job, TaskStatus::Cancelled, TaskActor::User, "job cancelled").await;
            self.update(job, |e| {
                e.view.state = JobState::Cancelled;
                e.view.detail = Some("cancelled".into());
            })
            .await;
        }
        self.get(job).ok_or(JobsError::NotFound(job))
    }

    /// Emergency stop: immediate cancel of every leased job on every node.
    pub async fn emergency_stop_all(self: &Arc<Self>) -> usize {
        let all = self.leases.all();
        let mut n = 0;
        for (job, lease) in all {
            if self.cancel(job, true).await.is_ok() {
                n += 1;
            }
            tracing::warn!(job_id = %job, node_id = %lease.node_id, "remote job cancelled by emergency stop");
        }
        // every node also runs its own emergency stop (cancels its jobs + local AI work)
        for id in self.links.node_ids() {
            if let Some(link) = self.links.link(id) {
                if let Err(e) = link.call("emergency.stopAll", serde_json::json!({})).await {
                    tracing::warn!(node_id = %id, error = %e, "emergency stop broadcast failed");
                }
            }
        }
        n
    }

    fn spawn_emergency_watch(self: &Arc<Self>, shutdown: tokio_util::sync::CancellationToken) {
        let this = Arc::downgrade(self);
        let emergency = Arc::clone(&self.services.security.emergency);
        tokio::spawn(async move {
            loop {
                let token = match emergency.register_global() {
                    Ok(t) => t,
                    Err(_) => {
                        // stop active: wait for a reset
                        tokio::select! {
                            () = shutdown.cancelled() => break,
                            () = tokio::time::sleep(std::time::Duration::from_millis(500)) => continue,
                        }
                    }
                };
                tokio::select! {
                    () = shutdown.cancelled() => break,
                    () = token.cancelled() => {}
                }
                let Some(this) = this.upgrade() else { break };
                let n = this.emergency_stop_all().await;
                tracing::warn!(cancelled = n, "emergency stop broadcast to nodes");
            }
        });
    }

    /// Drains `job` on its node (planned migration step 1): returns the checkpoint.
    pub(super) async fn drain(self: &Arc<Self>, job: JobId) -> Result<(JobCheckpoint, NodeId), JobsError> {
        let lease = self.leases.get(job).ok_or_else(|| JobsError::State("job holds no lease".into()))?;
        let link = self.links.link(lease.node_id).ok_or(JobsError::NoLink(lease.node_id))?;
        self.update(job, |e| e.view.state = JobState::Migrating).await;
        let p = WorkerJobParams { job_id: job, epoch: lease.epoch, immediate: false };
        let r: DrainJobResult = match call_typed::<jm::Drain>(link.as_ref(), &p).await {
            Ok(r) => r,
            Err(e) => {
                self.update(job, |e| e.view.state = JobState::Running).await;
                return Err(e.into());
            }
        };
        self.leases.release(job);
        self.fetch_checkpoint_blobs(&link, &r.checkpoint).await?;
        self.record_checkpoint(r.checkpoint.clone()).await;
        self.publish(
            EventKind::JobCheckpointed(JobCheckpointedPayload { checkpoint: Box::new(r.checkpoint.clone()) }),
            Some(lease.node_id),
        )
        .await;
        self.audit(
            "jobs.drain",
            job,
            lease.node_id,
            AuditOutcome::Executed,
            serde_json::json!({"epoch": lease.epoch, "checkpointId": r.checkpoint.checkpoint_id}),
        )
        .await;
        Ok((r.checkpoint, lease.node_id))
    }
}

pub(super) fn truncate_reason(s: &str) -> String {
    s.chars().take(200).collect()
}

/// Content digest helper re-exported for callers verifying artifacts.
#[must_use]
pub fn digest_of(bytes: &[u8]) -> Digest {
    sha_digest(bytes)
}

fn store_err(e: orkestr_task_engine::jobs::JobError) -> JobsError {
    match e {
        orkestr_task_engine::jobs::JobError::NotFound(j) => JobsError::NotFound(j),
        orkestr_task_engine::jobs::JobError::StaleEpoch { .. } => JobsError::State(e.to_string()),
        other => JobsError::Internal(other.to_string()),
    }
}

/// WP-B `JobDispatcher` over the coordinator driver (non-blocking: spawns the offer).
pub struct CoordinatorDispatcher(std::sync::Weak<CoordinatorJobs>);

impl JobDispatcher for CoordinatorDispatcher {
    fn dispatch(&self, job_id: JobId) {
        let Some(c) = self.0.upgrade() else { return };
        tokio::spawn(async move {
            if let Err(e) = c.dispatch_assigned(job_id).await {
                tracing::warn!(job_id = %job_id, error = %e, "dispatch failed; job re-placed later");
                let detail = e.to_string();
                c.update(job_id, |en| {
                    en.view.state = JobState::Pending;
                    en.view.detail = Some(detail);
                })
                .await;
            }
        });
    }
}
