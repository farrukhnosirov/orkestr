//! `replay.*` — Execution Replay (§21, §68.50).
//!
//! Reconstructs, from the persisted event log plus the Manager's stored plans
//! and runs, the timeline of a Manager run or of a single task:
//! User request → Manager plan → Agent creation → Delegation → Tool events →
//! Files changed → Tests → Reviews → Approvals → Merge — with timestamps,
//! correlation ids and per-agent filtering. Only visible, persisted output is
//! used (event payloads, stored messages); hidden chain-of-thought is never
//! stored, so it can never be replayed.
//!
//! `replay.step` is the step-through API: it returns one step plus the state
//! reconstructed up to it (task statuses, agent states, files changed, tool,
//! test, review and approval counters).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentId, AgentRole, AgentState, ApprovalId, ApprovalStatus, CorrelationId, ManagerPlanId, ManagerRunId, ProjectId,
    ReviewState, TaskId, TaskStatus, Timestamp, ToolCallStatus,
};

use super::manager::ManagerRunStatus;
use super::method;

/// Max steps of one timeline (older steps are dropped, `truncated` is set).
pub const MAX_REPLAY_STEPS: usize = 5000;
/// Max events scanned for one timeline.
pub const MAX_REPLAY_SCAN: usize = 200_000;

/// Phase of the execution a step belongs to (§21 order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReplayPhase {
    /// The user's request.
    Request,
    /// Manager plan proposed / approved, tasks created, decisions.
    Plan,
    /// Agents created / stopped / state changes worth showing.
    Agents,
    /// Work handed to an agent (agent started, task running).
    Delegation,
    /// Tool calls.
    Tools,
    /// Files changed, snapshots, rollbacks.
    Files,
    /// Builds and test runs.
    Tests,
    /// Reviews.
    Reviews,
    /// Approvals requested / resolved.
    Approvals,
    /// Merge proposals and merges.
    Merge,
    /// Run / task lifecycle (status changes, completion, failures, stops).
    Run,
}

/// Outcome tone of a step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReplayStepStatus {
    /// Neutral.
    Info,
    /// Succeeded.
    Success,
    /// Needs attention (approval asked, changes requested, …).
    Warning,
    /// Failed / denied / error.
    Error,
}

/// One step of a timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayStep {
    /// Position in the (filtered) timeline, 0-based.
    pub index: u32,
    /// Source event seq (`None` for steps built from stored records, e.g. the
    /// user request).
    pub seq: Option<u64>,
    /// Time.
    pub ts: Timestamp,
    /// Phase.
    pub phase: ReplayPhase,
    /// Event kind name (`ToolCallStarted`, …) or a synthetic kind
    /// (`UserRequest`, `MergeProposals`).
    pub kind: String,
    /// Outcome tone.
    pub status: ReplayStepStatus,
    /// Main text: tool name, task title, file path, plan goal, request text…
    /// (visible content, redacted at the source; not translated).
    pub title: String,
    /// Secondary text (summaries reported by tools / agents / reviewers).
    #[serde(default)]
    pub detail: Option<String>,
    /// Agent.
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    /// Task.
    #[serde(default)]
    pub task_id: Option<TaskId>,
    /// Correlation id of the source event.
    #[serde(default)]
    pub correlation_id: Option<CorrelationId>,
    /// Files touched by this step.
    #[serde(default)]
    pub files: Vec<String>,
    /// Duration (tool calls, builds, tests).
    #[serde(default)]
    pub duration_ms: Option<u64>,
    /// New task status (task steps).
    #[serde(default)]
    pub task_status: Option<TaskStatus>,
    /// New agent state (agent steps).
    #[serde(default)]
    pub agent_state: Option<AgentState>,
    /// Tool call status (tool steps; updated by `ToolCallFinished`).
    #[serde(default)]
    pub tool_status: Option<ToolCallStatus>,
    /// Approval.
    #[serde(default)]
    pub approval_id: Option<ApprovalId>,
    /// Approval status (approval steps).
    #[serde(default)]
    pub approval_status: Option<ApprovalStatus>,
    /// Review verdict (review steps).
    #[serde(default)]
    pub verdict: Option<ReviewState>,
    /// Tests passed (test steps).
    #[serde(default)]
    pub tests_passed: Option<u32>,
    /// Tests failed (test steps).
    #[serde(default)]
    pub tests_failed: Option<u32>,
    /// Run status (run lifecycle steps).
    #[serde(default)]
    pub run_status: Option<ManagerRunStatus>,
}

/// An agent that appears in a timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayAgent {
    /// Agent.
    pub agent_id: AgentId,
    /// Name.
    pub name: String,
    /// Role.
    pub role: Option<AgentRole>,
    /// Task it worked on.
    pub task_id: Option<TaskId>,
    /// Steps attributed to it (unfiltered timeline).
    pub steps: u32,
}

/// A task that appears in a timeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayTaskRef {
    /// Task.
    pub task_id: TaskId,
    /// Title.
    pub title: String,
    /// Final / current status.
    pub status: TaskStatus,
}

/// Steps per phase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayPhaseSummary {
    /// Phase.
    pub phase: ReplayPhase,
    /// Steps in it.
    pub steps: u32,
    /// Index of its first step.
    pub first_index: u32,
}

/// What a timeline replays.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReplaySubject {
    /// A supervised Manager run.
    Run,
    /// One task (with every agent that worked on it).
    Task,
}

/// A reconstructed execution timeline.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayTimeline {
    /// Project.
    pub project_id: ProjectId,
    /// Subject kind.
    pub subject: ReplaySubject,
    /// Run (run subjects).
    pub run_id: Option<ManagerRunId>,
    /// Plan of the run.
    pub plan_id: Option<ManagerPlanId>,
    /// Task (task subjects).
    pub task_id: Option<TaskId>,
    /// Goal of the run or title of the task.
    pub title: String,
    /// Run status (run subjects).
    pub run_status: Option<ManagerRunStatus>,
    /// Task status (task subjects).
    pub task_status: Option<TaskStatus>,
    /// First step time.
    pub started_at: Option<Timestamp>,
    /// Last step time.
    pub ended_at: Option<Timestamp>,
    /// Steps in time order (after the agent / phase filters).
    pub steps: Vec<ReplayStep>,
    /// Agents of the subject (unfiltered).
    pub agents: Vec<ReplayAgent>,
    /// Tasks of the subject.
    pub tasks: Vec<ReplayTaskRef>,
    /// Steps per phase (of `steps`), in phase order.
    pub phases: Vec<ReplayPhaseSummary>,
    /// Steps before filtering.
    pub total_steps: u32,
    /// Events read from the log.
    pub events_scanned: u32,
    /// The timeline hit a limit (scan or step cap) and is partial.
    pub truncated: bool,
}

/// A run in the run picker.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayRunSummary {
    /// Run.
    pub run_id: ManagerRunId,
    /// Plan.
    pub plan_id: ManagerPlanId,
    /// Goal.
    pub goal: String,
    /// Status.
    pub status: ManagerRunStatus,
    /// Started.
    pub started_at: Timestamp,
    /// Finished.
    pub finished_at: Option<Timestamp>,
    /// Tasks.
    pub tasks: Vec<ReplayTaskRef>,
    /// Agents spawned.
    pub agents_spawned: u32,
    /// Spend (USD).
    pub cost_usd: f64,
}

/// Params of `replay.runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayRunsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Max runs (newest; default 50, max 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `replay.runs`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayRunsResult {
    /// Runs, newest first.
    pub runs: Vec<ReplayRunSummary>,
}

/// Params of `replay.timeline`. Exactly one of `runId` / `taskId`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayTimelineParams {
    /// Project.
    pub project_id: ProjectId,
    /// Run to replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Task to replay.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Only steps of this agent (plus its task's agent-less steps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Only these phases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phases: Option<Vec<ReplayPhase>>,
}

/// Result of `replay.timeline`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayTimelineResult {
    /// The timeline.
    pub timeline: ReplayTimeline,
}

/// Cursor movement of `replay.step`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReplayDirection {
    /// First step.
    First,
    /// Step before `index`.
    Prev,
    /// Step after `index`.
    Next,
    /// Last step.
    Last,
    /// Exactly `index`.
    #[default]
    Goto,
    /// First step of `phase` after `index` (wraps to the first of the phase).
    NextPhase,
}

/// Params of `replay.step`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayStepParams {
    /// Timeline selector (same rules as `replay.timeline`).
    #[serde(flatten)]
    pub timeline: ReplayTimelineParams,
    /// Current cursor (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub index: Option<u32>,
    /// Movement (default `Goto`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub direction: Option<ReplayDirection>,
    /// Target phase of `NextPhase`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub phase: Option<ReplayPhase>,
}

/// State of a task at a replay cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayTaskState {
    /// Task.
    pub task_id: TaskId,
    /// Title.
    pub title: String,
    /// Status at the cursor (`None` = not reached yet).
    pub status: Option<TaskStatus>,
}

/// State of an agent at a replay cursor.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayAgentState {
    /// Agent.
    pub agent_id: AgentId,
    /// Name.
    pub name: String,
    /// Last known state.
    pub state: Option<AgentState>,
    /// Spawned and not stopped at the cursor.
    pub active: bool,
}

/// State reconstructed up to (and including) a step.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayState {
    /// Tasks of the subject.
    pub tasks: Vec<ReplayTaskState>,
    /// Agents created so far.
    pub agents: Vec<ReplayAgentState>,
    /// Distinct files changed so far, in first-change order.
    pub files_changed: Vec<String>,
    /// Tool calls so far.
    pub tool_calls: u32,
    /// Failed / denied tool calls so far.
    pub tool_failures: u32,
    /// Approvals requested and not resolved at the cursor.
    pub approvals_pending: u32,
    /// Approvals resolved so far.
    pub approvals_resolved: u32,
    /// Tests passed (sum of test runs so far).
    pub tests_passed: u32,
    /// Tests failed (sum of test runs so far).
    pub tests_failed: u32,
    /// Reviews so far.
    pub reviews: u32,
    /// Run status at the cursor.
    pub run_status: Option<ManagerRunStatus>,
}

/// Result of `replay.step`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReplayStepResult {
    /// The step at the new cursor (`None` when the timeline is empty).
    pub step: Option<ReplayStep>,
    /// New cursor.
    pub index: u32,
    /// Steps in the (filtered) timeline.
    pub total: u32,
    /// A previous step exists.
    pub has_prev: bool,
    /// A next step exists.
    pub has_next: bool,
    /// State up to and including the step.
    pub state: ReplayState,
}

method!(
    /// `replay.runs`.
    Runs, "replay.runs", ReplayRunsParams, ReplayRunsResult
);
method!(
    /// `replay.timeline`.
    Timeline, "replay.timeline", ReplayTimelineParams, ReplayTimelineResult
);
method!(
    /// `replay.step`.
    Step, "replay.step", ReplayStepParams, ReplayStepResult
);
