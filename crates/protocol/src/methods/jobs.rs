//! `jobs.*` — remote jobs: placement of a task on a node, leases, checkpoints
//! and migration (Phase 6, node-protocol.md §5–7, ADR-0033).
//!
//! Two audiences share the area:
//! * **UI** (desktop / paired devices, role Operator): `jobs.submit`,
//!   `jobs.list`, `jobs.get`, `jobs.migrate`, `jobs.cancel`;
//! * **worker side** (called by the coordinator over the E2E session, principal
//!   `coordinator`): `jobs.offer`, `jobs.drain`, `node.jobCancel`.
//!
//! Job progress travels worker → coordinator as events (`JobProgress`,
//! `JobCheckpointed`) on topic `jobs` of the same E2E session; every job-side
//! message carries the lease `epoch` (fencing, SR-81).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentId, AgentTemplateId, ApprovalId, Arch, CheckpointId, Digest, EnvironmentKind, ExecutorRef, JobId, LeaseEpoch,
    ModelRef, NodeId, OsKind, ProjectId, TaskId, TaskPriority, Timestamp, TrustLevel,
};

use super::agents::Handoff;
use super::method;

/// What a job runs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum JobKind {
    /// A command line in the job worktree (builds, tests). Tool shims apply.
    #[serde(rename_all = "camelCase")]
    Command {
        /// Program.
        program: String,
        /// Arguments.
        #[serde(default)]
        args: Vec<String>,
        /// Extra environment (never secrets).
        #[serde(default)]
        env: BTreeMap<String, String>,
        /// Timeout in seconds.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        timeout_secs: Option<u64>,
    },
    /// An agent working on the task (mock / Ollama providers in Phase 6).
    #[serde(rename_all = "camelCase")]
    AgentTask {
        /// Agent template.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        template_id: Option<AgentTemplateId>,
        /// Routed model.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        model: Option<ModelRef>,
        /// Instructions / prompt.
        prompt: String,
    },
    /// Long-running service (deferred: `unimplementedFeature`).
    Service,
}

/// GPU requirement.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GpuRequirement {
    /// Minimum VRAM in MiB.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub min_vram_mb: Option<u64>,
    /// Vendor (`nvidia`, `amd`, `apple`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub vendor: Option<String>,
}

/// Hard constraints (scheduler filter).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Requirements {
    /// OS family.
    #[ts(optional)]
    pub os: Option<OsKind>,
    /// Architecture.
    #[ts(optional)]
    pub arch: Option<Arch>,
    /// Tools that must be on PATH (or shimmed).
    pub tools: Vec<String>,
    /// Minimum free memory in MiB.
    #[ts(optional)]
    pub min_mem_mb: Option<u64>,
    /// GPU.
    #[ts(optional)]
    pub gpu: Option<GpuRequirement>,
    /// Local models that must be installed (`name:tag`).
    pub local_models: Vec<String>,
    /// Labels that must match.
    pub labels: BTreeMap<String, String>,
}

/// Resource estimate (scheduler headroom).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct ResourceEstimate {
    /// CPU cores.
    pub cpu_cores: f32,
    /// Memory MiB.
    pub mem_mb: u64,
    /// VRAM MiB.
    pub vram_mb: u64,
    /// Disk MiB.
    pub disk_mb: u64,
    /// Expected duration in seconds.
    pub duration_s: u64,
}

/// What the job needs from the trust model.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Needs {
    /// Network access.
    pub network: bool,
    /// Secret references (non-empty ⇒ Infeasible on remote nodes until HPKE delivery lands).
    pub secrets: Vec<String>,
    /// Target environment.
    #[ts(optional)]
    pub environment: Option<EnvironmentKind>,
    /// Minimum trust level (derived from kind/needs when unset).
    #[ts(optional)]
    pub min_trust: Option<TrustLevel>,
}

/// Placement preferences.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct Affinity {
    /// Preferred nodes.
    pub preferred: Vec<NodeId>,
    /// Forbidden nodes (migration excludes the source node here).
    pub forbidden: Vec<NodeId>,
    /// Prefer nodes with a warm project cache.
    pub sticky_to_cache: bool,
}

/// A job to place and run (node-protocol.md §5).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobSpec {
    /// Job.
    pub job_id: JobId,
    /// Task.
    pub task_id: TaskId,
    /// Project.
    pub project_id: ProjectId,
    /// Kind.
    pub kind: JobKind,
    /// Hard constraints.
    #[serde(default)]
    pub requirements: Requirements,
    /// Resource estimate.
    #[serde(default)]
    pub estimate: ResourceEstimate,
    /// Trust needs.
    #[serde(default)]
    pub needs: Needs,
    /// Priority.
    #[serde(default)]
    pub priority: TaskPriority,
    /// Affinity.
    #[serde(default)]
    pub affinity: Affinity,
}

/// Scoring weights (node-protocol.md §5 defaults).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ScoreWeights {
    /// Free resources.
    pub headroom: f64,
    /// Project cache warmth.
    pub cache_warmth: f64,
    /// 1 − load.
    pub load: f64,
    /// Network.
    pub network: f64,
    /// User preference.
    pub preference: f64,
    /// Energy (AC power).
    pub energy: f64,
}

impl Default for ScoreWeights {
    fn default() -> Self {
        Self { headroom: 0.30, cache_warmth: 0.25, load: 0.15, network: 0.10, preference: 0.15, energy: 0.05 }
    }
}

/// Coordinator-wide placement policy (scheduler input besides the job).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PlacementPolicy {
    /// Score weights.
    pub weights: ScoreWeights,
    /// The coordinator's own node id (tie-break prefers it).
    #[ts(optional)]
    pub coordinator_node_id: Option<NodeId>,
}

/// Why a job waits.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobWaitReason {
    /// Feasible nodes exist but have no free slot / headroom now.
    NoCapacity,
    /// Feasible nodes exist but are Suspect/Offline/Draining now.
    NodesUnavailable,
}

/// Scheduler decision.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "decision", rename_all = "camelCase")]
#[ts(export)]
pub enum Placement {
    /// Run on `node_id`.
    #[serde(rename_all = "camelCase")]
    Place {
        /// Chosen node.
        node_id: NodeId,
        /// Score.
        score: f64,
        /// Human-readable explanation (stored on the task, shown in UI).
        explanation: Vec<String>,
    },
    /// No feasible node now.
    #[serde(rename_all = "camelCase")]
    Wait {
        /// Why.
        reason: JobWaitReason,
        /// Explanation.
        explanation: Vec<String>,
    },
    /// No enrolled node can ever satisfy the job → task Blocked.
    #[serde(rename_all = "camelCase")]
    Infeasible {
        /// Why.
        reason: String,
        /// Explanation (per rejected node).
        explanation: Vec<String>,
    },
}

/// Git state of a workspace (checkpoint / job input).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct GitStateDescriptor {
    /// Branch.
    #[ts(optional)]
    pub branch: Option<String>,
    /// Base commit.
    #[ts(optional)]
    pub head_commit: Option<String>,
    /// `git bundle` of unpushed commits (blob).
    #[ts(optional)]
    pub bundle: Option<Digest>,
    /// Tar of the dirty tree (staged + unstaged + untracked) (blob).
    #[ts(optional)]
    pub dirty_tar: Option<Digest>,
}

/// Job inputs.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct JobInputs {
    /// Workspace git state to materialize.
    pub git: GitStateDescriptor,
    /// Checkpoint to resume from (migration).
    #[ts(optional)]
    pub checkpoint: Option<JobCheckpoint>,
    /// Handoff for a fresh run.
    #[ts(optional)]
    pub handoff: Option<Handoff>,
    /// Extra blobs (models, artifacts) the worker must have before starting.
    pub blobs: Vec<Digest>,
}

/// Coordinator-signed policy snapshot (node enforces the stricter of local and snapshot, SR-83).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct PolicySnapshot {
    /// Capability rules (opaque JSON, permissions model).
    #[ts(type = "unknown")]
    pub capability_rules: serde_json::Value,
    /// Approval mode.
    pub approval_mode: String,
    /// Egress allowlist.
    pub egress_allowlist: Vec<String>,
    /// Budget USD.
    #[ts(optional)]
    pub budget_usd: Option<f64>,
    /// Issued at.
    #[ts(optional)]
    pub issued_at: Option<Timestamp>,
    /// Coordinator signature (CIK, base64url) over the canonical snapshot without `sig`.
    pub sig: String,
}

/// `jobs.offer` params (coordinator → worker).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobOffer {
    /// Job.
    pub spec: JobSpec,
    /// Fencing epoch.
    pub epoch: LeaseEpoch,
    /// Lease end (renewed by heartbeats).
    pub lease_until: Timestamp,
    /// Inputs.
    pub inputs: JobInputs,
    /// Signed policy snapshot.
    pub policy: PolicySnapshot,
    /// Coordinator node.
    pub coordinator_node_id: NodeId,
}

/// Result of `jobs.offer`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobOfferResult {
    /// Accepted.
    pub accepted: bool,
    /// Echoed epoch.
    pub epoch: LeaseEpoch,
    /// Reject reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// State of the agent at a checkpoint (node-protocol.md §7.1). No hidden chain-of-thought.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct AgentStateCheckpoint {
    /// Agent.
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Template.
    #[ts(optional)]
    pub template_id: Option<AgentTemplateId>,
    /// Routed model.
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Turn number.
    pub turn: u32,
    /// Tokens used.
    pub tokens_used: u64,
    /// Cost used (USD).
    pub cost_usd: f64,
    /// Index into the persisted tool-call history.
    pub tool_log_cursor: u64,
    /// Structured summary of the conversation.
    pub conversation_summary: String,
    /// Remaining plan steps.
    pub pending_plan: Vec<String>,
    /// Open approvals.
    pub open_approvals: Vec<ApprovalId>,
    /// Steps already completed (resume must not repeat them).
    pub completed_steps: Vec<String>,
}

/// Classification of a side effect.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum SideEffectKind {
    /// Safe to repeat (build, test, local file write).
    Idempotent,
    /// Must not be repeated blindly (deploy, push, DB write, external API).
    NonIdempotent,
    /// Unknown classification (treated as non-idempotent).
    Unknown,
}

/// A side effect performed by a job since the previous checkpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SideEffectRecord {
    /// Action (`deploy`, `git.push`, `http.post`, `command`).
    pub action: String,
    /// Classification.
    pub kind: SideEffectKind,
    /// Target / detail.
    pub detail: String,
    /// When.
    pub at: Timestamp,
}

/// A checkpoint of a job (node-protocol.md §7.1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobCheckpoint {
    /// Checkpoint.
    pub checkpoint_id: CheckpointId,
    /// Job.
    pub job_id: JobId,
    /// Task.
    pub task_id: TaskId,
    /// Epoch it was taken under.
    pub epoch: LeaseEpoch,
    /// Node it was taken on.
    pub node_id: NodeId,
    /// When.
    pub created_at: Timestamp,
    /// Workspace git state (bundle + dirty tar blobs).
    pub git: GitStateDescriptor,
    /// Agent state (AgentTask jobs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent: Option<AgentStateCheckpoint>,
    /// Handoff for the next executor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub handoff: Option<Handoff>,
    /// Artifact blobs.
    #[serde(default)]
    pub artifacts: Vec<Digest>,
    /// Side effects since the previous checkpoint.
    #[serde(default)]
    pub side_effects: Vec<SideEffectRecord>,
    /// Final checkpoint of a drain.
    #[serde(default)]
    pub is_final: bool,
}

/// Job lifecycle on the coordinator.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum JobState {
    /// Waiting for placement.
    Pending,
    /// Offered to a node.
    Offered,
    /// Running on a node.
    Running,
    /// Draining / migrating.
    Migrating,
    /// Completed.
    Completed,
    /// Failed.
    Failed,
    /// Cancelled.
    Cancelled,
    /// Cannot be placed.
    Infeasible,
}

/// A job as seen by the UI.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobView {
    /// Spec.
    pub spec: JobSpec,
    /// State.
    pub state: JobState,
    /// Current owner.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub owner: Option<ExecutorRef>,
    /// Current epoch.
    pub epoch: LeaseEpoch,
    /// Last placement decision (with explanation).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub placement: Option<Placement>,
    /// Latest checkpoint id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_checkpoint: Option<CheckpointId>,
    /// Progress 0..=1.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub progress: Option<f32>,
    /// Exit code (Command jobs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit_code: Option<i32>,
    /// Failure / status detail (e.g. `needs_user_decision`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    /// Result artifact blobs.
    #[serde(default)]
    pub artifacts: Vec<Digest>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

/// `jobs.submit` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SubmitJobParams {
    /// Task the job executes.
    pub task_id: TaskId,
    /// What to run and where it may run (`jobId`, `taskId`, `projectId` are filled by the core).
    pub spec: JobSubmitSpec,
}

/// User-supplied part of a [`JobSpec`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobSubmitSpec {
    /// Kind.
    pub kind: JobKind,
    /// Requirements.
    #[serde(default)]
    pub requirements: Requirements,
    /// Estimate.
    #[serde(default)]
    pub estimate: ResourceEstimate,
    /// Needs.
    #[serde(default)]
    pub needs: Needs,
    /// Affinity.
    #[serde(default)]
    pub affinity: Affinity,
}

/// `jobs.list` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListJobsParams {
    /// Filter by task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Filter by node.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub node_id: Option<NodeId>,
    /// Only non-terminal jobs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active_only: Option<bool>,
}

/// `jobs.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListJobsResult {
    /// Jobs (newest first).
    pub jobs: Vec<JobView>,
}

/// `jobs.get` params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobIdParams {
    /// Job.
    pub job_id: JobId,
}

/// `jobs.migrate` params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MigrateJobParams {
    /// Job.
    pub job_id: JobId,
    /// Target node (scheduler picks when unset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_node_id: Option<NodeId>,
}

/// `jobs.cancel` (UI) params.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CancelJobParams {
    /// Job.
    pub job_id: JobId,
    /// Kill now instead of stopping at the next safe point.
    #[serde(default)]
    pub immediate: bool,
}

/// Result carrying one job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobResult {
    /// The job.
    pub job: JobView,
}

/// Worker-side `jobs.drain` / `node.jobCancel` params (fenced by epoch).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkerJobParams {
    /// Job.
    pub job_id: JobId,
    /// Epoch the coordinator believes current.
    pub epoch: LeaseEpoch,
    /// Cancel only: kill now.
    #[serde(default)]
    pub immediate: bool,
}

/// Result of worker-side `jobs.drain`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DrainJobResult {
    /// Final checkpoint.
    pub checkpoint: JobCheckpoint,
}

/// Result of worker-side `node.jobCancel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorkerCancelResult {
    /// Whether a running job was stopped.
    pub cancelled: bool,
}

// --- events -------------------------------------------------------------------

/// `JobAssigned` event payload (coordinator).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobAssignedPayload {
    /// Job.
    pub job_id: JobId,
    /// Task.
    pub task_id: TaskId,
    /// Project.
    pub project_id: ProjectId,
    /// Node.
    pub node_id: NodeId,
    /// Epoch.
    pub epoch: LeaseEpoch,
    /// Placement explanation.
    pub explanation: Vec<String>,
}

/// One progress item of a running job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum JobProgressItem {
    /// Output chunk (stdout/stderr or agent message).
    #[serde(rename_all = "camelCase")]
    Output {
        /// `stdout`, `stderr` or `agent`.
        stream: String,
        /// Text.
        text: String,
    },
    /// Fraction done.
    #[serde(rename_all = "camelCase")]
    Progress {
        /// 0..=1.
        fraction: f32,
    },
    /// A step finished (agent tool call / plan step).
    #[serde(rename_all = "camelCase")]
    Step {
        /// Step name.
        name: String,
    },
    /// A side effect was performed.
    #[serde(rename_all = "camelCase")]
    SideEffect {
        /// The record.
        record: SideEffectRecord,
    },
    /// An approval is needed (forwarded as `ApprovalRequested` on the coordinator).
    #[serde(rename_all = "camelCase")]
    ApprovalNeeded {
        /// What.
        summary: String,
    },
    /// The job finished.
    #[serde(rename_all = "camelCase")]
    Finished {
        /// Success.
        success: bool,
        /// Exit code (Command jobs).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        exit_code: Option<i32>,
        /// Result artifact blobs.
        #[serde(default)]
        artifacts: Vec<Digest>,
        /// Detail.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        detail: Option<String>,
    },
}

/// `JobProgress` event payload (worker → coordinator, topic `jobs`; fenced by epoch).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobProgressPayload {
    /// Job.
    pub job_id: JobId,
    /// Task.
    pub task_id: TaskId,
    /// Node.
    pub node_id: NodeId,
    /// Epoch.
    pub epoch: LeaseEpoch,
    /// Items.
    pub items: Vec<JobProgressItem>,
}

/// `JobCheckpointed` event payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobCheckpointedPayload {
    /// The checkpoint.
    pub checkpoint: Box<JobCheckpoint>,
}

/// `JobMigrated` event payload.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct JobMigratedPayload {
    /// Job.
    pub job_id: JobId,
    /// Task.
    pub task_id: TaskId,
    /// Source node.
    pub from_node_id: NodeId,
    /// Target node.
    pub to_node_id: NodeId,
    /// New epoch.
    pub epoch: LeaseEpoch,
    /// Checkpoint resumed from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub checkpoint_id: Option<CheckpointId>,
    /// `planned` (drain / user) or `nodeLost` (lease expiry).
    pub reason: String,
}

// --- methods ------------------------------------------------------------------

method!(
    /// `jobs.submit` — place and run a task on the best node (Phase 6).
    Submit, "jobs.submit", SubmitJobParams, JobResult
);
method!(
    /// `jobs.list`.
    List, "jobs.list", ListJobsParams, ListJobsResult
);
method!(
    /// `jobs.get`.
    Get, "jobs.get", JobIdParams, JobResult
);
method!(
    /// `jobs.migrate` — planned migration to another node.
    Migrate, "jobs.migrate", MigrateJobParams, JobResult
);
method!(
    /// `jobs.cancel` — cancel a job (UI).
    Cancel, "jobs.cancel", CancelJobParams, JobResult
);
method!(
    /// Worker side `jobs.offer` (coordinator → worker).
    Offer, "jobs.offer", JobOffer, JobOfferResult
);
method!(
    /// Worker side `jobs.drain` — run to the next safe point and return a final checkpoint.
    Drain, "jobs.drain", WorkerJobParams, DrainJobResult
);
method!(
    /// Worker side `node.jobCancel` — stop a job (graceful or immediate).
    JobCancel, "node.jobCancel", WorkerJobParams, WorkerCancelResult
);
