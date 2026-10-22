//! Worker-side job host: accepts `jobs.offer`, verifies the signed policy
//! snapshot, materializes `ORKESTR_HOME/jobs/<id>`, runs the job, publishes
//! `JobProgress` / `JobCheckpointed` on topic `jobs` (Phase 6). **Owner: WP-C.**
//!
//! Integration seams:
//! * [`host`] / [`install`] — the per-core [`WorkerJobs`] (keyed by node id;
//!   the default host uses the core's shared `FsBlobStore` — the store
//!   `node.blobPut` writes into; `install` replaces it in tests);
//! * [`WorkerJobs::trust_coordinator`] — WP-A calls it with the coordinator's
//!   CIK public key from the pairing record (offers from untrusted
//!   coordinators are rejected, fail closed);
//! * [`WorkerJobs::renew`] / [`WorkerJobs::revoke_of`] / [`WorkerJobs::leases_of`]
//!   — the hub's `node.heartbeat` (`nodes::wiring::HubWorker`) revokes,
//!   renews and reports the leases of the calling coordinator only
//!   (`bind_coordinator_device` at `node.hello`); runs interrupted by a
//!   restart are reported as `Failed` until revoked.
//!
//! Job kinds: `Command` (tool shims from the node profile apply; files written
//! to `$ORKESTR_JOB_OUTPUT_DIR` (alias `$ORKESTR_ARTIFACTS_DIR`) become artifact
//! blobs; stdout lines `::orkestr-side-effect {"action","kind","detail"}` enter
//! the side-effect log) and `AgentTask`, driven
//! by the deterministic plan provider ([`PlanStep`], the "mock" provider: one
//! prompt line = one tool step). Safe points sit between steps (drain, cancel,
//! lease checks, periodic checkpoints), so a resumed job never repeats a
//! completed step.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, LazyLock};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orkestr_domain::{CheckpointId, DeviceId, Digest, JobId, LeaseEpoch, NodeId, Timestamp, TrustLevel};
use orkestr_events::EventBus;
use orkestr_node::exec::{check_epoch, classify_side_effect, JobControl, JobError, JobExecutor, ToolShims};
use orkestr_node::{BlobError, BlobStore};
use orkestr_protocol::event::{EventKind, NewEvent};
use orkestr_protocol::methods::agents::{
    Handoff, HandoffBudget, HandoffFile, HandoffParty, HandoffReason, HandoffTask, HandoffWorkspace,
};
use orkestr_protocol::methods::jobs::{
    AgentStateCheckpoint, GitStateDescriptor, JobCheckpoint, JobCheckpointedPayload, JobKind, JobOffer, JobOfferResult,
    JobProgressItem, JobProgressPayload, SideEffectKind, SideEffectRecord,
};
use orkestr_protocol::methods::nodes::{LeasePhase, LeaseRenewal, LeaseState};
use orkestr_protocol::Event;
use orkestr_vcs::GitCli;
use parking_lot::Mutex;
use sha2::{Digest as _, Sha256};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::sync::{broadcast, mpsc, oneshot, watch};

use super::lease::PolicyVerifier;
use crate::services::Services;

/// Worker configuration.
#[derive(Clone)]
pub struct WorkerConfig {
    /// This node.
    pub node_id: NodeId,
    /// `ORKESTR_HOME/jobs`.
    pub jobs_root: PathBuf,
    /// Blob store shared with `node.blob*`.
    pub blobs: Arc<dyn BlobStore>,
    /// Tool shims of the node profile.
    pub shims: ToolShims,
    /// Max concurrently running jobs.
    pub max_concurrent: usize,
    /// Minimum interval between periodic checkpoints at safe points (`None` = only on drain / finish).
    pub checkpoint_every: Option<Duration>,
}

impl std::fmt::Debug for WorkerConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerConfig")
            .field("node_id", &self.node_id)
            .field("jobs_root", &self.jobs_root)
            .finish_non_exhaustive()
    }
}

/// How a run ended.
enum Outcome {
    Finished { success: bool, exit_code: Option<i32>, detail: Option<String> },
    Drained,
    Cancelled(String),
}

/// One job held by this worker.
struct Slot {
    offer: JobOffer,
    epoch: LeaseEpoch,
    dir: PathBuf,
    lease_until: Mutex<Timestamp>,
    phase: Mutex<LeasePhase>,
    progress: Mutex<Option<f32>>,
    last_checkpoint: Mutex<Option<CheckpointId>>,
    cancel_tx: watch::Sender<bool>,
    immediate: AtomicBool,
    drain_requested: AtomicBool,
    drain_reply: Mutex<Option<oneshot::Sender<Result<JobCheckpoint, String>>>>,
}

impl Slot {
    fn state(&self) -> LeaseState {
        LeaseState {
            job_id: self.offer.spec.job_id,
            epoch: self.epoch,
            phase: *self.phase.lock(),
            progress: *self.progress.lock(),
            last_checkpoint: *self.last_checkpoint.lock(),
            lease_until: Some(*self.lease_until.lock()),
        }
    }

    fn active(&self) -> bool {
        matches!(*self.phase.lock(), LeasePhase::Starting | LeasePhase::Running | LeasePhase::Draining)
    }
}

/// Mutable run state (what a checkpoint captures).
#[derive(Default)]
struct RunState {
    completed: Vec<String>,
    side_effects: Vec<SideEffectRecord>,
    artifacts: Vec<Digest>,
    files_changed: Vec<String>,
    base: Option<String>,
    branch: Option<String>,
    bundle: Option<Digest>,
    last_checkpoint_at: Option<Instant>,
}

/// Worker-side job host (one per core).
pub struct WorkerJobs {
    cfg: WorkerConfig,
    git: Arc<GitCli>,
    events: EventBus,
    verifier: PolicyVerifier,
    slots: Mutex<HashMap<JobId, Arc<Slot>>>,
    /// Copy of every job event this host publishes (the coordinator's
    /// in-process link to its own worker host reads it).
    tap: broadcast::Sender<Event>,
    /// Paired coordinator devices → their node ids (bound at `node.hello`).
    devices: Mutex<HashMap<DeviceId, NodeId>>,
    /// Runs interrupted by a crash / restart of this core (lease markers left
    /// on disk), reported as `Failed` until their coordinator revokes them.
    orphans: Mutex<HashMap<JobId, Orphan>>,
}

/// A run found on disk after a restart.
#[derive(Debug, Clone, Copy)]
struct Orphan {
    epoch: LeaseEpoch,
    coordinator: NodeId,
}

#[derive(serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
struct LeaseMarker {
    coordinator: NodeId,
    epoch: LeaseEpoch,
}

/// `jobs_root/<job>/e<epoch>.lease` markers of runs that did not end.
fn scan_orphans(jobs_root: &Path) -> HashMap<JobId, Orphan> {
    let mut out: HashMap<JobId, Orphan> = HashMap::new();
    let Ok(jobs) = std::fs::read_dir(jobs_root) else { return out };
    for job_dir in jobs.flatten() {
        let Ok(job) = job_dir.file_name().to_string_lossy().parse::<JobId>() else { continue };
        let Ok(files) = std::fs::read_dir(job_dir.path()) else { continue };
        for f in files.flatten() {
            if f.path().extension().is_none_or(|e| e != "lease") {
                continue;
            }
            let Some(m) = std::fs::read(f.path()).ok().and_then(|b| serde_json::from_slice::<LeaseMarker>(&b).ok())
            else {
                continue;
            };
            if out.get(&job).is_none_or(|o| o.epoch < m.epoch) {
                out.insert(job, Orphan { epoch: m.epoch, coordinator: m.coordinator });
            }
        }
    }
    out
}

impl std::fmt::Debug for WorkerJobs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkerJobs").field("node_id", &self.cfg.node_id).finish_non_exhaustive()
    }
}

static HOSTS: LazyLock<Mutex<HashMap<NodeId, Arc<WorkerJobs>>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// The worker host of the core behind `services` (created on first use with
/// the core's shared blob store and the process `ORKESTR_NODE_PROFILE` shims).
#[must_use]
pub fn host(services: &Services) -> Arc<WorkerJobs> {
    let mut hosts = HOSTS.lock();
    Arc::clone(hosts.entry(services.node_id).or_insert_with(|| {
        let profile = orkestr_node::profile::load_profile(&orkestr_node::profile::EnvSource::process());
        // the store `node.blobPut` / `node.blobGet` use (WP-D)
        let blobs: Arc<dyn BlobStore> = match crate::rpc::models::blob_store(services) {
            Ok(b) => b,
            Err(e) => {
                tracing::error!(error = %e.message, "blob store unavailable; worker jobs use an in-memory store");
                Arc::new(orkestr_node::blobs::MemBlobStore::new())
            }
        };
        let cfg = WorkerConfig {
            node_id: services.node_id,
            jobs_root: services.paths.data_dir.join("jobs"),
            blobs,
            shims: ToolShims::from_profile(profile.as_ref()),
            max_concurrent: profile.and_then(|p| p.max_concurrent_jobs).map_or(4, |n| n.max(1) as usize),
            checkpoint_every: Some(Duration::from_secs(300)),
        };
        let host = Arc::new(WorkerJobs::new(cfg, Arc::clone(services.vcs.git()), services.events.clone()));
        spawn_local_emergency_watch(&host, Arc::clone(&services.security.emergency));
        host
    }))
}

/// The worker honours its own emergency stop independently (§52): every
/// active job is cancelled when the local `emergency.stopAll` fires.
fn spawn_local_emergency_watch(host: &Arc<WorkerJobs>, emergency: Arc<orkestr_security::EmergencyController>) {
    let Ok(rt) = tokio::runtime::Handle::try_current() else { return };
    let weak = Arc::downgrade(host);
    rt.spawn(async move {
        loop {
            let Ok(token) = emergency.register_global() else {
                tokio::time::sleep(Duration::from_millis(500)).await;
                continue;
            };
            token.cancelled().await;
            let Some(host) = weak.upgrade() else { break };
            let n = host.cancel_all();
            tracing::warn!(cancelled = n, "local emergency stop: worker jobs cancelled");
        }
    });
}

/// Side-effect marker printed by job commands on stdout.
pub const SIDE_EFFECT_MARKER: &str = "::orkestr-side-effect ";

#[derive(serde::Deserialize)]
struct MarkerRecord {
    action: String,
    #[serde(default = "unknown_kind")]
    kind: SideEffectKind,
    #[serde(default)]
    detail: String,
}

fn unknown_kind() -> SideEffectKind {
    SideEffectKind::Unknown
}

/// Parses a `::orkestr-side-effect {json}` line.
#[must_use]
pub fn parse_side_effect_marker(line: &str) -> Option<SideEffectRecord> {
    let json = line.trim().strip_prefix(SIDE_EFFECT_MARKER.trim_end())?.trim();
    let m: MarkerRecord = serde_json::from_str(json).ok()?;
    Some(SideEffectRecord { action: m.action, kind: m.kind, detail: m.detail, at: now() })
}

fn output_line(
    progress: &(impl Fn(Vec<JobProgressItem>) + Sync),
    effects: &mut Vec<SideEffectRecord>,
    stream: String,
    text: String,
) {
    let mut items = Vec::with_capacity(2);
    if stream == "stdout" {
        if let Some(record) = parse_side_effect_marker(&text) {
            effects.push(record.clone());
            items.push(JobProgressItem::SideEffect { record });
        }
    }
    items.insert(0, JobProgressItem::Output { stream, text });
    progress(items);
}

/// Replaces the worker host of `node` (integration / tests).
pub fn install(node: NodeId, host: Arc<WorkerJobs>) {
    HOSTS.lock().insert(node, host);
}

fn now() -> Timestamp {
    chrono::Utc::now()
}

/// `jobs_root/<job>/e<epoch>.lease` next to the run directory `.../e<epoch>`.
fn marker_path(run_dir: &Path) -> PathBuf {
    let name = run_dir.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    run_dir.with_file_name(format!("{name}.lease"))
}

fn blob_err(e: BlobError) -> String {
    e.to_string()
}

impl WorkerJobs {
    /// New host.
    #[must_use]
    pub fn new(cfg: WorkerConfig, git: Arc<GitCli>, events: EventBus) -> Self {
        let (tap, _) = broadcast::channel(1024);
        let orphans = scan_orphans(&cfg.jobs_root);
        if !orphans.is_empty() {
            tracing::warn!(
                count = orphans.len(),
                "job runs interrupted by a restart; reporting them to their coordinators"
            );
        }
        Self {
            cfg,
            git,
            events,
            verifier: PolicyVerifier::default(),
            slots: Mutex::new(HashMap::new()),
            tap,
            devices: Mutex::new(HashMap::new()),
            orphans: Mutex::new(orphans),
        }
    }

    /// Node id.
    #[must_use]
    pub fn node_id(&self) -> NodeId {
        self.cfg.node_id
    }

    /// Blob store of this worker.
    #[must_use]
    pub fn blobs(&self) -> &Arc<dyn BlobStore> {
        &self.cfg.blobs
    }

    /// Trusts `key` as the policy-signing CIK of coordinator `node`.
    pub fn trust_coordinator(&self, node: NodeId, key: p256::ecdsa::VerifyingKey) {
        self.verifier.trust(node, key);
    }

    /// Remembers that the paired `device` is the coordinator core `node`
    /// (heartbeats from it then only see / revoke that coordinator's jobs).
    pub fn bind_coordinator_device(&self, device: DeviceId, node: NodeId) {
        self.devices.lock().insert(device, node);
    }

    /// The coordinator node bound to a paired device.
    #[must_use]
    pub fn coordinator_of_device(&self, device: DeviceId) -> Option<NodeId> {
        self.devices.lock().get(&device).copied()
    }

    /// Lease states of the jobs offered by `coordinator` (every job for
    /// `None`), including runs interrupted by a restart (`Failed`).
    #[must_use]
    pub fn leases_of(&self, coordinator: Option<NodeId>) -> Vec<LeaseState> {
        let mine = |c: NodeId| coordinator.is_none_or(|x| x == c);
        let mut v: Vec<LeaseState> =
            self.slots.lock().values().filter(|s| mine(s.offer.coordinator_node_id)).map(|s| s.state()).collect();
        v.extend(self.orphans.lock().iter().filter(|(_, o)| mine(o.coordinator)).map(|(job, o)| LeaseState {
            job_id: *job,
            epoch: o.epoch,
            phase: LeasePhase::Failed,
            progress: None,
            last_checkpoint: None,
            lease_until: None,
        }));
        v.sort_by_key(|s| s.job_id.to_string());
        v
    }

    /// [`Self::revoke`] limited to the jobs of `coordinator` (`None` = any);
    /// revoked restart leftovers are deleted from disk.
    pub fn revoke_of(&self, coordinator: Option<NodeId>, jobs: &[JobId]) {
        let mine = |c: NodeId| coordinator.is_none_or(|x| x == c);
        let own: Vec<JobId> = {
            let slots = self.slots.lock();
            jobs.iter().copied().filter(|j| slots.get(j).is_some_and(|s| mine(s.offer.coordinator_node_id))).collect()
        };
        self.revoke(&own);
        let gone: Vec<(JobId, Orphan)> = {
            let mut orphans = self.orphans.lock();
            let ids: Vec<JobId> =
                jobs.iter().copied().filter(|j| orphans.get(j).is_some_and(|o| mine(o.coordinator))).collect();
            ids.into_iter().filter_map(|j| orphans.remove(&j).map(|o| (j, o))).collect()
        };
        for (job, o) in gone {
            tracing::info!(job_id = %job, epoch = o.epoch.0, "interrupted run revoked by its coordinator; removed");
            let dir = self.cfg.jobs_root.join(job.to_string());
            let _ = std::fs::remove_file(dir.join(format!("e{}.lease", o.epoch.0)));
            let _ = std::fs::remove_dir_all(dir.join(format!("e{}", o.epoch.0)));
            let _ = std::fs::remove_dir_all(dir.join(format!("e{}-artifacts", o.epoch.0)));
        }
    }

    fn write_marker(&self, slot: &Slot) {
        let path = marker_path(&slot.dir);
        let marker = LeaseMarker { coordinator: slot.offer.coordinator_node_id, epoch: slot.epoch };
        let res = path
            .parent()
            .map_or(Ok(()), std::fs::create_dir_all)
            .and_then(|()| std::fs::write(&path, serde_json::to_vec(&marker).unwrap_or_default()));
        if let Err(e) = res {
            tracing::warn!(error = %e, "job lease marker not written");
        }
    }

    /// Handles `jobs.offer` (`trust` = trust level granted to the calling
    /// coordinator; `None` for local callers).
    pub async fn offer(self: &Arc<Self>, offer: JobOffer, trust: Option<TrustLevel>) -> JobOfferResult {
        let epoch = offer.epoch;
        let reject = |reason: String| JobOfferResult { accepted: false, epoch, reason: Some(reason) };
        let job = offer.spec.job_id;
        if let Some(t) = trust {
            if offer.spec.needs.min_trust.is_some_and(|m| t < m) {
                return reject("trust level below the job's requirement".into());
            }
            if t == TrustLevel::Restricted && matches!(offer.spec.kind, JobKind::AgentTask { .. }) {
                return reject("restricted nodes do not run agent tasks".into());
            }
        }
        if matches!(offer.spec.kind, JobKind::Service) {
            return reject("unimplemented-feature: service jobs".into());
        }
        if !offer.spec.needs.secrets.is_empty() {
            return reject("unimplemented-feature: secrets on remote nodes".into());
        }
        if let Err(e) = self.verifier.verify(offer.coordinator_node_id, job, epoch, &offer.policy) {
            return reject(format!("policy snapshot rejected: {e}"));
        }
        if offer.lease_until <= now() {
            return reject("lease already expired".into());
        }
        {
            let slots = self.slots.lock();
            if let Some(prev) = slots.get(&job) {
                if prev.epoch >= epoch {
                    return reject(format!("stale epoch {epoch} (current {})", prev.epoch));
                }
            }
            let running = slots.values().filter(|s| s.active()).count();
            if running >= self.cfg.max_concurrent {
                return reject("no free job slot".into());
            }
        }
        let (progress_tx, progress_rx) = mpsc::unbounded_channel();
        let (cancel_tx, cancel_rx) = watch::channel(false);
        let ctl = JobControl { progress: progress_tx, cancelled: cancel_rx };
        let dir = self.cfg.jobs_root.join(job.to_string()).join(format!("e{}", epoch.0));
        let slot = Arc::new(Slot {
            dir,
            epoch,
            lease_until: Mutex::new(offer.lease_until),
            phase: Mutex::new(LeasePhase::Starting),
            progress: Mutex::new(None),
            last_checkpoint: Mutex::new(None),
            cancel_tx,
            immediate: AtomicBool::new(false),
            drain_requested: AtomicBool::new(false),
            drain_reply: Mutex::new(None),
            offer,
        });
        // a newer epoch supersedes an older local run of the same job
        if let Some(prev) = self.slots.lock().insert(job, Arc::clone(&slot)) {
            tracing::info!(job_id = %job, old = prev.epoch.0, new = epoch.0, "newer epoch supersedes the local run");
            prev.immediate.store(true, Ordering::SeqCst);
            let _ = prev.cancel_tx.send(true);
        }
        self.write_marker(&slot);
        self.orphans.lock().remove(&job);
        self.spawn_forwarder(progress_rx);
        let this = Arc::clone(self);
        tokio::spawn(async move { this.run(slot, ctl).await });
        tracing::info!(job_id = %job, epoch = epoch.0, "job offer accepted");
        JobOfferResult { accepted: true, epoch, reason: None }
    }

    fn spawn_forwarder(&self, mut rx: mpsc::UnboundedReceiver<JobProgressPayload>) {
        let events = self.events.clone();
        let tap = self.tap.clone();
        let node = self.cfg.node_id;
        tokio::spawn(async move {
            while let Some(p) = rx.recv().await {
                let ev = NewEvent::new(EventKind::JobProgress(p)).node(node);
                match events.publish(ev).await {
                    Ok(ev) => {
                        let _ = tap.send((*ev).clone());
                    }
                    Err(e) => tracing::warn!(error = %e, "JobProgress publish failed"),
                }
            }
        });
    }

    /// Job events published by this host (`JobProgress`, `JobCheckpointed`).
    #[must_use]
    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.tap.subscribe()
    }

    /// Jobs currently holding a slot (starting / running / draining).
    #[must_use]
    pub fn active_jobs(&self) -> usize {
        self.slots.lock().values().filter(|s| s.active()).count()
    }

    fn slot(&self, job: JobId) -> Result<Arc<Slot>, JobError> {
        self.slots.lock().get(&job).cloned().ok_or(JobError::NotFound(job))
    }

    /// Lease renewals from `node.heartbeat`: renewals with the current epoch
    /// extend the lease; stale ones are ignored. Returns every lease state.
    pub fn renew(&self, renewals: &[LeaseRenewal]) -> Vec<LeaseState> {
        {
            let slots = self.slots.lock();
            for r in renewals {
                if let Some(s) = slots.get(&r.job_id) {
                    if s.epoch == r.epoch {
                        let mut until = s.lease_until.lock();
                        if r.lease_until > *until {
                            *until = r.lease_until;
                        }
                    }
                }
            }
        }
        self.leases()
    }

    /// Jobs the coordinator no longer recognises: cancelled immediately and forgotten.
    pub fn revoke(&self, jobs: &[JobId]) {
        let mut slots = self.slots.lock();
        for j in jobs {
            if let Some(s) = slots.remove(j) {
                tracing::info!(job_id = %j, epoch = s.epoch.0, "job revoked by the coordinator");
                s.immediate.store(true, Ordering::SeqCst);
                let _ = s.cancel_tx.send(true);
            }
        }
    }

    /// Lease states of every job held here.
    #[must_use]
    pub fn leases(&self) -> Vec<LeaseState> {
        let mut v: Vec<LeaseState> = self.slots.lock().values().map(|s| s.state()).collect();
        v.sort_by_key(|s| s.job_id.to_string());
        v
    }

    /// Cancels every active job (local emergency stop / revocation of the coordinator).
    pub fn cancel_all(&self) -> usize {
        let slots = self.slots.lock();
        let mut n = 0;
        for s in slots.values().filter(|s| s.active()) {
            s.immediate.store(true, Ordering::SeqCst);
            let _ = s.cancel_tx.send(true);
            n += 1;
        }
        n
    }

    async fn publish_checkpoint(&self, cp: &JobCheckpoint) {
        let ev = NewEvent::new(EventKind::JobCheckpointed(JobCheckpointedPayload { checkpoint: Box::new(cp.clone()) }))
            .node(self.cfg.node_id);
        match self.events.publish(ev).await {
            Ok(ev) => {
                let _ = self.tap.send((*ev).clone());
            }
            Err(e) => tracing::warn!(error = %e, "JobCheckpointed publish failed"),
        }
    }

    async fn run(self: Arc<Self>, slot: Arc<Slot>, ctl: JobControl) {
        let mut st = RunState::default();
        let outcome = match self.materialize(&slot, &mut st).await {
            Ok(()) => {
                *slot.phase.lock() = LeasePhase::Running;
                self.execute(&slot, &ctl, &mut st).await
            }
            Err(e) => Outcome::Finished { success: false, exit_code: None, detail: Some(format!("materialize: {e}")) },
        };
        let _ = std::fs::remove_file(marker_path(&slot.dir));
        let send = |items: Vec<JobProgressItem>| {
            let _ = ctl.progress.send(JobProgressPayload {
                job_id: slot.offer.spec.job_id,
                task_id: slot.offer.spec.task_id,
                node_id: self.cfg.node_id,
                epoch: slot.epoch,
                items,
            });
        };
        match outcome {
            Outcome::Drained => {
                let cp = self.checkpoint(&slot, &mut st, true, HandoffReason::Stopped).await;
                *slot.phase.lock() = LeasePhase::Cancelled;
                if let Ok(cp) = &cp {
                    self.publish_checkpoint(cp).await;
                }
                if let Some(tx) = slot.drain_reply.lock().take() {
                    let _ = tx.send(cp);
                }
            }
            Outcome::Cancelled(reason) => {
                *slot.phase.lock() = LeasePhase::Cancelled;
                send(vec![JobProgressItem::Finished {
                    success: false,
                    exit_code: None,
                    artifacts: Vec::new(),
                    detail: Some(reason),
                }]);
                if let Some(tx) = slot.drain_reply.lock().take() {
                    let _ = tx.send(Err("job cancelled".into()));
                }
            }
            Outcome::Finished { success, exit_code, detail } => {
                let detail = match self.checkpoint(&slot, &mut st, true, HandoffReason::Completed).await {
                    Ok(cp) => {
                        self.publish_checkpoint(&cp).await;
                        detail
                    }
                    Err(e) => Some(format!("{} (result checkpoint failed: {e})", detail.unwrap_or_default())),
                };
                *slot.phase.lock() = if success { LeasePhase::Finished } else { LeasePhase::Failed };
                *slot.progress.lock() = Some(1.0);
                send(vec![JobProgressItem::Finished { success, exit_code, artifacts: st.artifacts.clone(), detail }]);
                if let Some(tx) = slot.drain_reply.lock().take() {
                    let _ = tx.send(Err("job already finished".into()));
                }
            }
        }
    }

    async fn materialize(&self, slot: &Slot, st: &mut RunState) -> Result<(), String> {
        let inputs = &slot.offer.inputs;
        let git = inputs.checkpoint.as_ref().map_or(&inputs.git, |c| &c.git);
        if let Some(cp) = &inputs.checkpoint {
            if let Some(agent) = &cp.agent {
                st.completed.clone_from(&agent.completed_steps);
            }
            st.artifacts.clone_from(&cp.artifacts);
            st.side_effects.clone_from(&cp.side_effects);
        }
        if slot.dir.exists() {
            let _ = tokio::fs::remove_dir_all(&slot.dir).await;
        }
        let parent = slot.dir.parent().map(Path::to_path_buf).unwrap_or_default();
        tokio::fs::create_dir_all(&parent).await.map_err(|e| e.to_string())?;
        st.branch.clone_from(&git.branch);
        match (&git.bundle, &git.head_commit) {
            (Some(bundle), Some(head)) => {
                let path = self.cfg.blobs.path(bundle).ok_or_else(|| format!("missing input blob {bundle}"))?;
                self.git
                    .clone_bundle(&path, &slot.dir, head, git.branch.as_deref())
                    .await
                    .map_err(|e| e.to_string())?;
                if let Some(patch) = &git.dirty_tar {
                    let p = self.cfg.blobs.path(patch).ok_or_else(|| format!("missing input blob {patch}"))?;
                    let bytes = tokio::fs::read(&p).await.map_err(|e| e.to_string())?;
                    self.git.apply_patch(&slot.dir, &bytes).await.map_err(|e| e.to_string())?;
                }
                st.base = Some(head.clone());
                st.bundle = Some(bundle.clone());
            }
            _ => tokio::fs::create_dir_all(&slot.dir).await.map_err(|e| e.to_string())?,
        }
        tokio::fs::create_dir_all(self.artifacts_dir(slot)).await.map_err(|e| e.to_string())?;
        Ok(())
    }

    fn artifacts_dir(&self, slot: &Slot) -> PathBuf {
        slot.dir.with_file_name(format!("e{}-artifacts", slot.epoch.0))
    }

    /// Safe point: cancel / drain / lease checks and periodic checkpoints.
    async fn safe_point(&self, slot: &Slot, ctl: &JobControl, st: &mut RunState) -> Option<Outcome> {
        if *ctl.cancelled.borrow() {
            return Some(Outcome::Cancelled("cancelled".into()));
        }
        if slot.drain_requested.load(Ordering::SeqCst) {
            return Some(Outcome::Drained);
        }
        if *slot.lease_until.lock() < now() {
            tracing::warn!(job_id = %slot.offer.spec.job_id, epoch = slot.epoch.0, "lease expired: job self-fenced");
            return Some(Outcome::Cancelled("lease expired (self-fenced)".into()));
        }
        if let Some(every) = self.cfg.checkpoint_every {
            if st.last_checkpoint_at.is_none_or(|t| t.elapsed() >= every) && !st.completed.is_empty() {
                if let Ok(cp) = self.checkpoint(slot, st, false, HandoffReason::Stopped).await {
                    self.publish_checkpoint(&cp).await;
                }
            }
        }
        None
    }

    async fn execute(&self, slot: &Slot, ctl: &JobControl, st: &mut RunState) -> Outcome {
        let spec = &slot.offer.spec;
        let progress = |items: Vec<JobProgressItem>| {
            let _ = ctl.progress.send(JobProgressPayload {
                job_id: spec.job_id,
                task_id: spec.task_id,
                node_id: self.cfg.node_id,
                epoch: slot.epoch,
                items,
            });
        };
        if let Some(h) = slot.offer.inputs.checkpoint.as_ref().and_then(|c| c.handoff.as_ref()).or(slot
            .offer
            .inputs
            .handoff
            .as_ref())
        {
            progress(vec![JobProgressItem::Step { name: format!("resumed from handoff {}", h.handoff_id) }]);
        }
        let steps: Vec<PlanStep> = match &spec.kind {
            JobKind::Command { program, args, env, timeout_secs } => vec![PlanStep::Run {
                program: program.clone(),
                args: args.clone(),
                env: env.clone().into_iter().collect(),
                timeout: timeout_secs.map(Duration::from_secs),
            }],
            JobKind::AgentTask { prompt, .. } => PlanStep::parse_plan(prompt),
            JobKind::Service => {
                return Outcome::Finished {
                    success: false,
                    exit_code: None,
                    detail: Some("service jobs unsupported".into()),
                }
            }
        };
        let total = steps.len().max(1);
        let mut last_exit = None;
        for (i, step) in steps.iter().enumerate() {
            let key = format!("{}:{}", i + 1, step.label());
            if st.completed.contains(&key) {
                continue;
            }
            if let Some(o) = self.safe_point(slot, ctl, st).await {
                return o;
            }
            if let Some(effect) = step.side_effect() {
                progress(vec![JobProgressItem::SideEffect { record: effect.clone() }]);
                st.side_effects.push(effect);
            }
            match self.run_step(slot, ctl, step, &progress, st).await {
                Ok(code) => last_exit = code,
                Err(StepError::Cancelled) => return Outcome::Cancelled("cancelled".into()),
                Err(StepError::Failed(detail, code)) => {
                    return Outcome::Finished { success: false, exit_code: code, detail: Some(detail) }
                }
            }
            st.completed.push(key.clone());
            #[allow(clippy::cast_precision_loss)]
            let fraction = (i + 1) as f32 / total as f32;
            *slot.progress.lock() = Some(fraction);
            progress(vec![JobProgressItem::Step { name: key }, JobProgressItem::Progress { fraction }]);
        }
        if let Some(o) = self.safe_point(slot, ctl, st).await {
            if matches!(o, Outcome::Drained | Outcome::Cancelled(_)) {
                return o;
            }
        }
        // collect artifacts
        match self.collect_artifacts(slot).await {
            Ok(mut digests) => {
                for d in digests.drain(..) {
                    if !st.artifacts.contains(&d) {
                        st.artifacts.push(d);
                    }
                }
            }
            Err(e) => {
                return Outcome::Finished {
                    success: false,
                    exit_code: last_exit,
                    detail: Some(format!("artifacts: {e}")),
                }
            }
        }
        Outcome::Finished { success: true, exit_code: last_exit.or(Some(0)), detail: None }
    }

    async fn collect_artifacts(&self, slot: &Slot) -> Result<Vec<Digest>, String> {
        let dir = self.artifacts_dir(slot);
        let mut out = Vec::new();
        let Ok(mut rd) = tokio::fs::read_dir(&dir).await else { return Ok(out) };
        let mut files = Vec::new();
        while let Ok(Some(e)) = rd.next_entry().await {
            if e.file_type().await.is_ok_and(|t| t.is_file()) {
                files.push(e.path());
            }
        }
        files.sort();
        for f in files {
            out.push(self.cfg.blobs.put_file(&f).await.map_err(blob_err)?);
        }
        Ok(out)
    }

    async fn run_step(
        &self,
        slot: &Slot,
        ctl: &JobControl,
        step: &PlanStep,
        progress: &(impl Fn(Vec<JobProgressItem>) + Sync),
        st: &mut RunState,
    ) -> Result<Option<i32>, StepError> {
        match step {
            PlanStep::Write { path, text, append } => {
                let rel = sanitize_rel(path).ok_or_else(|| StepError::Failed(format!("invalid path {path}"), None))?;
                let p = slot.dir.join(&rel);
                if let Some(parent) = p.parent() {
                    tokio::fs::create_dir_all(parent).await.map_err(|e| StepError::Failed(e.to_string(), None))?;
                }
                let mut body =
                    if *append { tokio::fs::read_to_string(&p).await.unwrap_or_default() } else { String::new() };
                body.push_str(text);
                body.push('\n');
                tokio::fs::write(&p, body).await.map_err(|e| StepError::Failed(e.to_string(), None))?;
                if !st.files_changed.contains(&rel) {
                    st.files_changed.push(rel);
                }
                Ok(None)
            }
            PlanStep::Sleep(d) => {
                let mut c = ctl.cancelled.clone();
                tokio::select! {
                    () = tokio::time::sleep(*d) => Ok(None),
                    () = wait_cancel(&mut c) => Err(StepError::Cancelled),
                }
            }
            PlanStep::Deploy(target) => {
                progress(vec![JobProgressItem::Output {
                    stream: "stdout".into(),
                    text: format!("deploying {target} (simulated)\n"),
                }]);
                Ok(None)
            }
            PlanStep::Note(_) => Ok(None),
            PlanStep::Run { program, args, env, timeout } => self
                .run_command(slot, ctl, program, args, env, *timeout, progress, &mut st.side_effects)
                .await
                .map(Some),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_command(
        &self,
        slot: &Slot,
        ctl: &JobControl,
        program: &str,
        args: &[String],
        env: &[(String, String)],
        timeout: Option<Duration>,
        progress: &(impl Fn(Vec<JobProgressItem>) + Sync),
        effects: &mut Vec<SideEffectRecord>,
    ) -> Result<i32, StepError> {
        let (prog, args) = self.cfg.shims.resolve(program, args);
        let resolved = orkestr_platform::process::which(&prog).unwrap_or_else(|| PathBuf::from(&prog));
        let mut cmd = tokio::process::Command::new(resolved);
        cmd.args(&args)
            .current_dir(&slot.dir)
            .env("ORKESTR_JOB_ID", slot.offer.spec.job_id.to_string())
            .env("ORKESTR_ARTIFACTS_DIR", self.artifacts_dir(slot))
            .env("ORKESTR_JOB_OUTPUT_DIR", self.artifacts_dir(slot))
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().map_err(|e| StepError::Failed(format!("spawn {prog}: {e}"), None))?;
        let (line_tx, mut line_rx) = mpsc::unbounded_channel::<(String, String)>();
        for (stream, reader) in [
            ("stdout", child.stdout.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>)),
            ("stderr", child.stderr.take().map(|s| Box::new(s) as Box<dyn tokio::io::AsyncRead + Send + Unpin>)),
        ] {
            if let Some(r) = reader {
                let tx = line_tx.clone();
                tokio::spawn(async move {
                    let mut lines = BufReader::new(r).lines();
                    while let Ok(Some(l)) = lines.next_line().await {
                        let _ = tx.send((stream.to_owned(), format!("{l}\n")));
                    }
                });
            }
        }
        drop(line_tx);
        let mut cancel = ctl.cancelled.clone();
        let deadline = tokio::time::sleep(timeout.unwrap_or(Duration::from_secs(24 * 3600)));
        tokio::pin!(deadline);
        let status = loop {
            tokio::select! {
                Some((stream, text)) = line_rx.recv() => output_line(progress, effects, stream, text),
                s = child.wait() => break s.map_err(|e| StepError::Failed(e.to_string(), None))?,
                () = wait_cancel(&mut cancel) => {
                    let _ = child.kill().await;
                    return Err(StepError::Cancelled);
                }
                () = &mut deadline => {
                    let _ = child.kill().await;
                    return Err(StepError::Failed("command timed out".into(), None));
                }
            }
        };
        while let Ok((stream, text)) = line_rx.try_recv() {
            output_line(progress, effects, stream, text);
        }
        let code = status.code().unwrap_or(-1);
        if status.success() {
            Ok(code)
        } else {
            Err(StepError::Failed(format!("{program} exited with {code}"), Some(code)))
        }
    }

    async fn store_bytes(&self, slot: &Slot, name: &str, bytes: &[u8]) -> Result<Digest, String> {
        let tmp = slot.dir.with_file_name(format!("e{}-{name}", slot.epoch.0));
        tokio::fs::write(&tmp, bytes).await.map_err(|e| e.to_string())?;
        let d = self.cfg.blobs.put_file(&tmp).await.map_err(blob_err);
        let _ = tokio::fs::remove_file(&tmp).await;
        d
    }

    async fn checkpoint(
        &self,
        slot: &Slot,
        st: &mut RunState,
        is_final: bool,
        reason: HandoffReason,
    ) -> Result<JobCheckpoint, String> {
        let spec = &slot.offer.spec;
        let mut git = GitStateDescriptor { branch: st.branch.clone(), ..GitStateDescriptor::default() };
        if let Some(base) = st.base.clone() {
            let head = self.git.rev_parse(&slot.dir, "HEAD").await.map_err(|e| e.to_string())?.unwrap_or(base.clone());
            if head != base || st.bundle.is_none() {
                let bundle_path = slot.dir.with_file_name(format!("e{}-ws.bundle", slot.epoch.0));
                self.git.create_bundle(&slot.dir, &bundle_path).await.map_err(|e| e.to_string())?;
                st.bundle = Some(self.cfg.blobs.put_file(&bundle_path).await.map_err(blob_err)?);
                let _ = tokio::fs::remove_file(&bundle_path).await;
                st.base = Some(head.clone());
            }
            let patch = self.git.working_patch(&slot.dir, &head).await.map_err(|e| e.to_string())?;
            if !patch.is_empty() {
                git.dirty_tar = Some(self.store_bytes(slot, "ws.patch", &patch).await?);
            }
            git.head_commit = Some(head);
            git.bundle.clone_from(&st.bundle);
        }
        let (template_id, model, prompt) = match &spec.kind {
            JobKind::AgentTask { template_id, model, prompt } => (*template_id, model.clone(), prompt.clone()),
            JobKind::Command { program, args, .. } => (None, None, format!("{program} {}", args.join(" "))),
            JobKind::Service => (None, None, String::new()),
        };
        let remaining: Vec<String> = match &spec.kind {
            JobKind::AgentTask { prompt, .. } => PlanStep::parse_plan(prompt)
                .iter()
                .enumerate()
                .map(|(i, s)| format!("{}:{}", i + 1, s.label()))
                .filter(|k| !st.completed.contains(k))
                .collect(),
            _ => Vec::new(),
        };
        let turn = u32::try_from(st.completed.len()).unwrap_or(u32::MAX);
        let summary = format!(
            "{} of {} steps done on node {}",
            st.completed.len(),
            st.completed.len() + remaining.len(),
            self.cfg.node_id
        );
        let created_at = now();
        let handoff = Handoff {
            handoff_id: format!("ho-{}", CheckpointId::new()),
            schema_version: 1,
            created_at,
            from: HandoffParty { agent_id: None, role: None, model: model.clone() },
            to: None,
            reason,
            task: HandoffTask {
                task_id: spec.task_id,
                title: truncate(&prompt, 80),
                goal: truncate(&prompt, 1000),
                acceptance_criteria: Vec::new(),
            },
            context: summary.clone(),
            relevant_files: st
                .files_changed
                .iter()
                .map(|p| HandoffFile { path: p.clone(), why: "changed by the job".into() })
                .collect(),
            decisions: Vec::new(),
            done: st.completed.clone(),
            remaining: remaining.clone(),
            unresolved_issues: Vec::new(),
            test_state: None,
            artifacts: Vec::new(),
            workspace: HandoffWorkspace {
                branch: st.branch.clone(),
                worktree: None,
                snapshot_id: None,
                files_changed: st.files_changed.clone(),
            },
            budget_remaining: HandoffBudget {
                turns: u32::try_from(remaining.len()).unwrap_or(u32::MAX),
                cost_usd: None,
            },
        };
        let cp = JobCheckpoint {
            checkpoint_id: CheckpointId::new(),
            job_id: spec.job_id,
            task_id: spec.task_id,
            epoch: slot.epoch,
            node_id: self.cfg.node_id,
            created_at,
            git,
            agent: Some(AgentStateCheckpoint {
                template_id,
                model,
                turn,
                tool_log_cursor: u64::from(turn),
                conversation_summary: summary,
                pending_plan: remaining,
                completed_steps: st.completed.clone(),
                ..AgentStateCheckpoint::default()
            }),
            handoff: Some(handoff),
            artifacts: st.artifacts.clone(),
            side_effects: st.side_effects.clone(),
            is_final,
        };
        *slot.last_checkpoint.lock() = Some(cp.checkpoint_id);
        st.last_checkpoint_at = Some(Instant::now());
        Ok(cp)
    }
}

#[async_trait]
impl JobExecutor for WorkerJobs {
    async fn start(&self, offer: JobOffer, ctl: JobControl) -> Result<(), JobError> {
        // `WorkerJobs::offer` is the entry point (it needs `Arc<Self>` to spawn).
        let _ = (offer, ctl);
        Err(JobError::Unimplemented("use WorkerJobs::offer"))
    }

    async fn drain(&self, job: JobId, epoch: LeaseEpoch) -> Result<JobCheckpoint, JobError> {
        let slot = self.slot(job)?;
        check_epoch(job, slot.epoch, epoch)?;
        if !slot.active() {
            return Err(JobError::Rejected("job is not running".into()));
        }
        let (tx, rx) = oneshot::channel();
        *slot.drain_reply.lock() = Some(tx);
        *slot.phase.lock() = LeasePhase::Draining;
        slot.drain_requested.store(true, Ordering::SeqCst);
        match rx.await {
            Ok(Ok(cp)) => Ok(cp),
            Ok(Err(e)) => Err(JobError::Rejected(e)),
            Err(_) => Err(JobError::Failed("job ended before the safe point".into())),
        }
    }

    async fn cancel(&self, job: JobId, epoch: LeaseEpoch, immediate: bool) -> Result<(), JobError> {
        let slot = self.slot(job)?;
        check_epoch(job, slot.epoch, epoch)?;
        tracing::info!(job_id = %job, epoch = epoch.0, immediate, "job cancelled by the coordinator");
        slot.immediate.store(immediate, Ordering::SeqCst);
        let _ = slot.cancel_tx.send(true);
        Ok(())
    }
}

enum StepError {
    Cancelled,
    Failed(String, Option<i32>),
}

fn truncate(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

fn sanitize_rel(p: &str) -> Option<String> {
    let p = p.replace('\\', "/");
    let ok = !p.is_empty()
        && !p.starts_with('/')
        && !p.contains(':')
        && p.split('/').all(|c| !c.is_empty() && c != ".." && c != "." && c != ".git");
    ok.then_some(p)
}

/// One step of the deterministic plan provider ("mock" AgentTask provider):
/// each non-empty prompt line is a tool step.
///
/// `write <path> <text>` · `append <path> <text>` · `run <program> [args…]` ·
/// `deploy <target>` (non-idempotent, simulated) · `sleep <ms>` · anything
/// else is a note (no side effect). `\n` in text becomes a newline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PlanStep {
    /// Writes / appends a file in the job worktree.
    Write {
        /// Worktree-relative path.
        path: String,
        /// Content.
        text: String,
        /// Append instead of replace.
        append: bool,
    },
    /// Runs a command (tool shims apply).
    Run {
        /// Program.
        program: String,
        /// Arguments.
        args: Vec<String>,
        /// Extra environment.
        env: Vec<(String, String)>,
        /// Timeout.
        timeout: Option<Duration>,
    },
    /// Simulated deployment (non-idempotent side effect).
    Deploy(String),
    /// Waits (cancellable).
    Sleep(Duration),
    /// Free text.
    Note(String),
}

impl PlanStep {
    /// Parses a prompt into steps.
    #[must_use]
    pub fn parse_plan(prompt: &str) -> Vec<Self> {
        prompt.lines().map(str::trim).filter(|l| !l.is_empty()).map(Self::parse).collect()
    }

    fn parse(line: &str) -> Self {
        let (verb, rest) = line.split_once(char::is_whitespace).unwrap_or((line, ""));
        // tolerate "Run: `cmd args`" (prose-style plans)
        let verb = verb.trim_end_matches(':');
        let rest = rest.trim().trim_matches('`').trim();
        match verb.to_ascii_lowercase().as_str() {
            "write" | "append" => {
                let (path, text) = rest.split_once(char::is_whitespace).unwrap_or((rest, ""));
                Self::Write {
                    path: path.to_owned(),
                    text: text.replace("\\n", "\n"),
                    append: verb.eq_ignore_ascii_case("append"),
                }
            }
            "run" => {
                let mut parts = orkestr_node::exec::split_command_line(rest).into_iter();
                let program = parts.next().unwrap_or_default();
                Self::Run { program, args: parts.collect(), env: Vec::new(), timeout: None }
            }
            "deploy" => Self::Deploy(rest.to_owned()),
            "sleep" => Self::Sleep(Duration::from_millis(rest.parse().unwrap_or(0))),
            _ => Self::Note(line.to_owned()),
        }
    }

    /// Stable label (part of the completed-step key).
    #[must_use]
    pub fn label(&self) -> String {
        let mut h = Sha256::new();
        let s = match self {
            Self::Write { path, text, append } => format!("{}:{path}:{text}", if *append { "append" } else { "write" }),
            Self::Run { program, args, .. } => format!("run:{program}:{}", args.join(" ")),
            Self::Deploy(t) => format!("deploy:{t}"),
            Self::Sleep(d) => format!("sleep:{}", d.as_millis()),
            Self::Note(n) => format!("note:{n}"),
        };
        h.update(s.as_bytes());
        let kind = s.split(':').next().unwrap_or("step").to_owned();
        format!("{kind}-{}", &hex::encode(h.finalize())[..12])
    }

    /// The side-effect record of the step (none for notes / sleeps).
    #[must_use]
    pub fn side_effect(&self) -> Option<SideEffectRecord> {
        let (action, kind, detail) = match self {
            Self::Write { path, .. } => ("write".to_owned(), SideEffectKind::Idempotent, path.clone()),
            Self::Run { program, args, .. } => {
                (format!("run {program}"), classify_side_effect(program, args), format!("{program} {}", args.join(" ")))
            }
            Self::Deploy(t) => ("deploy".to_owned(), SideEffectKind::NonIdempotent, t.clone()),
            Self::Sleep(_) | Self::Note(_) => return None,
        };
        Some(SideEffectRecord { action, kind, detail, at: now() })
    }
}

async fn wait_cancel(rx: &mut watch::Receiver<bool>) {
    if rx.wait_for(|v| *v).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_parsing_and_side_effects() {
        let steps = PlanStep::parse_plan("write a.txt hello\\nworld\n\nrun cargo build\ndeploy prod\nsleep 10\nthink");
        assert_eq!(steps.len(), 5);
        assert_eq!(steps[0], PlanStep::Write { path: "a.txt".into(), text: "hello\nworld".into(), append: false });
        assert_eq!(steps[1].side_effect().unwrap().kind, SideEffectKind::Idempotent);
        assert_eq!(steps[2].side_effect().unwrap().kind, SideEffectKind::NonIdempotent);
        assert!(steps[3].side_effect().is_none());
        assert_ne!(steps[0].label(), steps[1].label());
        assert_eq!(
            PlanStep::parse_plan("Run: `node fake.mjs --steps 2`")[0],
            PlanStep::Run {
                program: "node".into(),
                args: vec!["fake.mjs".into(), "--steps".into(), "2".into()],
                env: Vec::new(),
                timeout: None
            }
        );
        assert!(sanitize_rel("../x").is_none());
        assert!(sanitize_rel("C:/x").is_none());
        assert_eq!(sanitize_rel("src\\a.rs").as_deref(), Some("src/a.rs"));
    }
}
