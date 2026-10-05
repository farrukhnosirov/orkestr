//! `manager.*` — the AI Manager (§1, §5, §9, §11, §53, §57).
//!
//! The Manager is a per-project conversation with a model chosen in the AI
//! Manager settings (`providers.getManager`). It sees the real state of
//! Orkestr through read-only, permission-checked state tools, proposes
//! structured plans (goal → epics/features → tasks with acceptance criteria,
//! roles, dependencies, priorities, estimated cost, parallelism), turns an
//! approved plan into task-engine tasks and supervises a bounded run that
//! spawns isolated agents (own worktree each) for ready tasks.
//!
//! Only visible output is stored or streamed — never hidden chain-of-thought
//! (§1.14, §21).
//!
//! Events (topic `project:<id>`):
//! * `ManagerMessage` — streamed text deltas of the Manager's reply (ephemeral);
//! * `ManagerMessageAdded` — a complete message was stored (user, Manager or
//!   system event; ephemeral — history comes from `manager.history`);
//! * `ManagerPlanUpdated` — a plan was proposed / edited / approved / rejected;
//! * `ManagerRunUpdated` — a supervised run changed (progress, status, report).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentId, AgentRole, ApprovalMode, ConversationId, ManagerConfig, ManagerPlanId, ManagerRunId, ModelRef, ProjectId,
    SnapshotId, TaskId, TaskPriority, TaskStatus, Timestamp, UsageSummary,
};

use super::method;

/// Max characters of one user message.
pub const MAX_CHAT_TEXT: usize = 20_000;
/// Max tasks in one plan.
pub const MAX_PLAN_TASKS: usize = 40;

// ---------------------------------------------------------------------------
// Conversation
// ---------------------------------------------------------------------------

/// Who wrote a Manager conversation message.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ManagerMessageRole {
    /// The user.
    User,
    /// The AI Manager (visible output only).
    Manager,
    /// A system event (plan approved, run finished, errors, …).
    SystemEvent,
}

/// A tool the Manager used while answering (visible tool event, no reasoning).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerToolEvent {
    /// Tool name (`list_tasks`, `propose_plan`, …).
    pub name: String,
    /// One-line, redacted summary of the result.
    pub summary: String,
    /// The tool failed or was denied.
    pub is_error: bool,
}

/// One stored conversation message.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerChatMessage {
    /// Message id (also the `messageId` of its streamed deltas).
    pub id: String,
    /// Project.
    pub project_id: ProjectId,
    /// Conversation.
    pub conversation_id: ConversationId,
    /// Author.
    pub role: ManagerMessageRole,
    /// Markdown text.
    pub text: String,
    /// Tools used for this reply.
    #[serde(default)]
    pub tool_events: Vec<ManagerToolEvent>,
    /// Plan proposed / referenced by this message.
    #[serde(default)]
    pub plan_id: Option<ManagerPlanId>,
    /// Run referenced by this message.
    #[serde(default)]
    pub run_id: Option<ManagerRunId>,
    /// Model that produced a Manager reply.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Usage of a Manager reply.
    #[serde(default)]
    pub usage: Option<UsageSummary>,
    /// Creation time.
    pub created_at: Timestamp,
}

/// Summary of an archived conversation (`manager.clear`) or of the older part
/// of a long conversation. Extractive (built from stored messages and plans),
/// never model reasoning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConversationSummary {
    /// Conversation.
    pub conversation_id: ConversationId,
    /// Project.
    pub project_id: ProjectId,
    /// Markdown summary.
    pub summary: String,
    /// Messages covered.
    pub message_count: u32,
    /// First message time.
    pub started_at: Timestamp,
    /// Archive time.
    pub ended_at: Timestamp,
}

/// Code attached to a request (Code panel "Ask Manager about selection").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ChatAttachment {
    /// File path relative to the project root.
    pub path: String,
    /// First line (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub start_line: Option<u32>,
    /// Last line (1-based).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub end_line: Option<u32>,
    /// Language id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Selected text (≤ 20 000 chars).
    #[serde(default)]
    pub text: String,
}

/// Per-request overrides (Expert mode, §53). Absent fields use the AI
/// Manager settings.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerRequestOptions {
    /// Model for this request (must be allowed by the project's privacy policy).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Budget (USD) of the run started from this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub budget_usd: Option<f64>,
    /// Max parallel agents of the run started from this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_parallel_agents: Option<u32>,
    /// Time limit (minutes) of the run started from this request.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub time_limit_mins: Option<u32>,
    /// The user's resolved UI language (`ru`, `en`, …): the Manager and the
    /// agents of this request reply in it. Absent = keep the current one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub language: Option<String>,
    /// Delegate the run started from this request to an Agent Team (§6):
    /// its members execute the tasks, and with the team's hierarchy policy
    /// its lead decomposes each task for executors and a reviewer. Phase 2.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<orkestr_domain::AgentTeamId>,
}

/// Params of `manager.chat`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerChatParams {
    /// Project.
    pub project_id: ProjectId,
    /// User text (1..=20 000 chars).
    pub text: String,
    /// Attached code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub attachment: Option<ChatAttachment>,
    /// Expert-mode overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub options: Option<ManagerRequestOptions>,
}

/// Result of `manager.chat`. The reply is produced in the background and
/// streamed as `ManagerMessage` deltas, then stored (`ManagerMessageAdded`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerChatResult {
    /// The stored user message.
    pub user_message: ManagerChatMessage,
    /// Id the reply will be streamed and stored under.
    pub reply_message_id: String,
}

/// Params of `manager.history`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerHistoryParams {
    /// Project.
    pub project_id: ProjectId,
    /// Max messages (newest; default 200, max 1000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `manager.history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerHistoryResult {
    /// Active conversation.
    pub conversation_id: ConversationId,
    /// Messages, oldest first.
    pub messages: Vec<ManagerChatMessage>,
    /// Summaries of archived conversations, newest first.
    pub summaries: Vec<ConversationSummary>,
    /// A reply is being produced right now.
    pub busy: bool,
    /// Plans of the project that are proposed or executing (newest first),
    /// followed by finished plans the returned messages refer to.
    pub plans: Vec<ManagerPlan>,
    /// Active (running / paused / interrupted) run, if any.
    pub active_run: Option<ManagerRun>,
    /// Current AI Manager settings (model indicator).
    pub config: ManagerConfig,
}

/// Params with only a project.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerProjectParams {
    /// Project.
    pub project_id: ProjectId,
}

/// Result of `manager.clear`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerClearResult {
    /// The new, empty conversation.
    pub conversation_id: ConversationId,
    /// Summary of the archived conversation (absent when it was empty).
    pub archived: Option<ConversationSummary>,
}

// ---------------------------------------------------------------------------
// Plans
// ---------------------------------------------------------------------------

/// Lifecycle of a plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PlanStatus {
    /// Shown to the user, waiting for approval.
    Proposed,
    /// Approved; tasks were created.
    Approved,
    /// Rejected by the user.
    Rejected,
    /// Replaced by a newer proposal.
    Superseded,
    /// A run is executing it.
    Running,
    /// Its run completed.
    Completed,
    /// Its run failed or was stopped.
    Failed,
    /// Its run was cancelled.
    Cancelled,
}

/// How the plan JSON was obtained from the model.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PlanSource {
    /// Provider-enforced JSON schema (structured output).
    Structured,
    /// JSON extracted from free text and validated against the schema.
    Extracted,
}

/// One task of a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanTask {
    /// Key unique within the plan (dependencies refer to it).
    pub key: String,
    /// Title.
    pub title: String,
    /// Description (markdown).
    pub description: String,
    /// Acceptance criteria.
    pub acceptance_criteria: Vec<String>,
    /// Agent role that should do it.
    pub role: AgentRole,
    /// Keys of prerequisite tasks.
    pub depends_on: Vec<String>,
    /// Priority.
    pub priority: TaskPriority,
    /// Epic it belongs to (§9).
    #[serde(default)]
    pub epic: Option<String>,
    /// Feature it belongs to (§9).
    #[serde(default)]
    pub feature: Option<String>,
    /// Files it is expected to touch (file-overlap scheduling).
    #[serde(default)]
    pub files: Vec<String>,
    /// Estimated tokens (model's estimate).
    #[serde(default)]
    pub estimated_tokens: Option<u64>,
    /// Estimated cost (USD) with the routed model, when its price is known.
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    /// Task created on approval.
    #[serde(default)]
    pub task_id: Option<TaskId>,
}

/// A structured Manager plan (§1 steps 2–4, §9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerPlan {
    /// Id.
    pub id: ManagerPlanId,
    /// Project.
    pub project_id: ProjectId,
    /// Conversation it was proposed in.
    #[serde(default)]
    pub conversation_id: Option<ConversationId>,
    /// User goal.
    pub goal: String,
    /// Summary shown on the plan card.
    pub summary: String,
    /// Assumptions made.
    #[serde(default)]
    pub assumptions: Vec<String>,
    /// Risks / open questions.
    #[serde(default)]
    pub risks: Vec<String>,
    /// Tasks (topologically valid, no cycles).
    pub tasks: Vec<PlanTask>,
    /// Sum of known task estimates (USD); `None` when no price is known.
    #[serde(default)]
    pub estimated_cost_usd: Option<f64>,
    /// Widest set of tasks that can run at the same time (DAG width).
    pub max_parallelism: u32,
    /// Agents that will run in parallel (`min(width, maxParallelAgents)`).
    pub parallelism: u32,
    /// Status.
    pub status: PlanStatus,
    /// How the JSON was obtained.
    pub source: PlanSource,
    /// Model that planned.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Normalisation notes (unknown roles mapped, dependencies dropped, …).
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Request overrides captured at planning time.
    #[serde(default)]
    pub options: ManagerRequestOptions,
    /// Run executing it.
    #[serde(default)]
    pub run_id: Option<ManagerRunId>,
    /// Rejection reason.
    #[serde(default)]
    pub rejection_reason: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

/// Params of `manager.plan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerPlanParams {
    /// Project.
    pub project_id: ProjectId,
    /// Goal (1..=20 000 chars).
    pub goal: String,
    /// Expert-mode overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub options: Option<ManagerRequestOptions>,
}

/// A plan result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerPlanResult {
    /// The plan.
    pub plan: ManagerPlan,
}

/// Inline edit of a task title on the plan card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanTaskTitleEdit {
    /// Task key.
    pub key: String,
    /// New title.
    pub title: String,
}

/// User edits applied when approving.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanEdits {
    /// Tasks to drop (dependencies on them are removed).
    #[serde(default)]
    pub remove_tasks: Vec<String>,
    /// Retitled tasks.
    #[serde(default)]
    pub titles: Vec<PlanTaskTitleEdit>,
}

/// Params of `manager.approvePlan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovePlanParams {
    /// Plan.
    pub plan_id: ManagerPlanId,
    /// Edits made on the plan card.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub edits: Option<PlanEdits>,
    /// Start a supervised run right away (default true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub start_run: Option<bool>,
}

/// Result of `manager.approvePlan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovePlanResult {
    /// The approved plan (tasks carry their `taskId`).
    pub plan: ManagerPlan,
    /// The started run.
    pub run: Option<ManagerRun>,
}

/// Params of `manager.rejectPlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RejectPlanParams {
    /// Plan.
    pub plan_id: ManagerPlanId,
    /// Why (fed back to the Manager).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Runs
// ---------------------------------------------------------------------------

/// Status of a supervised run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ManagerRunStatus {
    /// Scheduling and supervising agents.
    Running,
    /// No new agents are started; running agents are paused.
    Paused,
    /// Every task completed.
    Completed,
    /// Stopped by a bound (failures, budget, time, agent limit) or stuck.
    Failed,
    /// Cancelled by the user or an emergency stop.
    Cancelled,
    /// The core stopped while the run was active; resumable.
    Interrupted,
}

impl ManagerRunStatus {
    /// Whether the run has ended for good.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// Hard bounds of a run (§64: no uncontrolled autonomous loops).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunLimits {
    /// Agents running at the same time.
    pub max_parallel_agents: u32,
    /// Agents spawned in total (including retries).
    pub max_agents: u32,
    /// Total spend (USD) of the run.
    pub max_cost_usd: Option<f64>,
    /// Wall-clock limit (minutes).
    pub time_limit_mins: Option<u32>,
    /// Consecutive task failures before the run stops.
    pub max_consecutive_failures: u32,
    /// Attempts per task (1 = no retry).
    pub max_attempts_per_task: u32,
}

/// One task of a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunTask {
    /// Task.
    pub task_id: TaskId,
    /// Plan key.
    pub key: String,
    /// Title.
    pub title: String,
    /// Role.
    pub role: AgentRole,
    /// Current task status.
    pub status: TaskStatus,
    /// Current / last agent.
    pub agent_id: Option<AgentId>,
    /// Agents spawned for it.
    pub attempts: u32,
    /// Agent branch (dedicated worktree).
    pub branch: Option<String>,
    /// Worktree path.
    pub worktree_path: Option<String>,
    /// Spend so far (USD).
    pub cost_usd: f64,
    /// Files changed.
    pub files_changed: Vec<String>,
    /// Last failure.
    pub last_error: Option<String>,
    /// The agent finished; the result waits for the user's review (Ask policy).
    pub awaiting_review: bool,
    /// Result summary of the agent.
    pub summary: Option<String>,
}

/// A branch ready to be merged (merging itself is Phase 2).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeProposal {
    /// Task.
    pub task_id: TaskId,
    /// Task title.
    pub title: String,
    /// Agent branch.
    pub branch: String,
    /// Files changed (agent report or diff).
    pub files_changed: Vec<String>,
    /// Lines added versus the base (when git could compute it).
    pub additions: Option<u32>,
    /// Lines removed versus the base.
    pub deletions: Option<u32>,
    /// Note (e.g. "contains the changes of `agent/backend-…`").
    pub note: String,
}

/// Final report of a run (§1 step 13).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunReport {
    /// Markdown report.
    pub summary: String,
    /// Completed tasks.
    pub completed: u32,
    /// Failed tasks.
    pub failed: u32,
    /// Cancelled / never started tasks.
    pub not_done: u32,
    /// Files changed across tasks.
    pub files_changed: Vec<String>,
    /// Tests reported by agents.
    pub tests: Vec<String>,
    /// Open issues reported by agents and the supervisor.
    pub open_issues: Vec<String>,
    /// Total spend (USD).
    pub cost_usd: f64,
    /// Duration (seconds).
    pub duration_secs: u64,
    /// Branches to merge (Phase 2 merges them).
    pub merge_proposals: Vec<MergeProposal>,
}

/// A supervised execution of a plan.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerRun {
    /// Id.
    pub id: ManagerRunId,
    /// Project.
    pub project_id: ProjectId,
    /// Plan.
    pub plan_id: ManagerPlanId,
    /// Plan goal.
    pub goal: String,
    /// Status.
    pub status: ManagerRunStatus,
    /// Why it is in this status (waiting for review, budget exhausted, …).
    pub status_reason: Option<String>,
    /// Bounds.
    pub limits: RunLimits,
    /// Approval policy for spawned agents and result acceptance.
    pub approval_policy: ApprovalMode,
    /// Tasks, in plan order.
    pub tasks: Vec<RunTask>,
    /// Agents spawned so far.
    pub agents_spawned: u32,
    /// Most agents that ran at the same time.
    pub peak_parallel: u32,
    /// Current streak of failed tasks.
    pub consecutive_failures: u32,
    /// Spend so far (USD).
    pub cost_usd: f64,
    /// Seconds the run was actively supervised (time limit; survives restarts).
    #[serde(default)]
    pub active_secs: u64,
    /// Started.
    pub started_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
    /// Finished.
    pub finished_at: Option<Timestamp>,
    /// Final report.
    pub report: Option<RunReport>,
}

/// Overrides of the run bounds (absent = Manager settings / defaults).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunLimitsPatch {
    /// Agents in parallel (1..=64).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_parallel_agents: Option<u32>,
    /// Agents in total (1..=500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_agents: Option<u32>,
    /// Budget (USD, ≥ 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_cost_usd: Option<f64>,
    /// Time limit (1..=1440 minutes).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub time_limit_mins: Option<u32>,
    /// Consecutive failures (1..=50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_consecutive_failures: Option<u32>,
    /// Attempts per task (1..=5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_attempts_per_task: Option<u32>,
}

/// Params of `manager.run`: start a run for an approved plan (`planId`) or
/// continue a paused / interrupted run (`runId`).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerRunParams {
    /// Approved plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plan_id: Option<ManagerPlanId>,
    /// Existing run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Bound overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limits: Option<RunLimitsPatch>,
}

/// Params with only a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerRunIdParams {
    /// Run.
    pub run_id: ManagerRunId,
}

/// A run result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerRunResult {
    /// The run.
    pub run: ManagerRun,
}

/// Params of `manager.status`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerStatusParams {
    /// Only this project's runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Only this run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
}

/// Result of `manager.status`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerStatusResult {
    /// Runs, newest first (at most 20).
    pub runs: Vec<ManagerRun>,
    /// Ids of runs supervised live in this process.
    pub live_run_ids: Vec<ManagerRunId>,
}

// ---------------------------------------------------------------------------
// Session resume (§57)
// ---------------------------------------------------------------------------

/// Progress of one goal (plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GoalProgress {
    /// Plan.
    pub plan_id: ManagerPlanId,
    /// Goal.
    pub goal: String,
    /// Completed tasks.
    pub completed: u32,
    /// Tasks.
    pub total: u32,
    /// `completed / total` in percent.
    pub percent: u32,
}

/// The last snapshot of a project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SnapshotBrief {
    /// Snapshot.
    pub id: SnapshotId,
    /// Label.
    pub label: String,
    /// Created.
    pub created_at: Timestamp,
}

/// "Last session" summary of a project (§57).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ManagerResumeSummary {
    /// Project.
    pub project_id: ProjectId,
    /// One line, e.g. "Last session: 3 of 4 tasks completed (75%); 1 approval pending".
    pub headline: String,
    /// Detail lines (goals, issues, approvals, snapshot).
    pub lines: Vec<String>,
    /// Tasks of the project.
    pub tasks_total: u32,
    /// Completed.
    pub tasks_completed: u32,
    /// Running / waiting / review / testing.
    pub tasks_in_progress: u32,
    /// Failed / interrupted.
    pub tasks_failed: u32,
    /// Blocked.
    pub tasks_blocked: u32,
    /// Cancelled (not counted in `percent_complete`).
    #[serde(default)]
    pub tasks_cancelled: u32,
    /// Completed in percent.
    pub percent_complete: u32,
    /// Per-goal progress, newest first.
    pub goals: Vec<GoalProgress>,
    /// Issues (failed tasks, interrupted runs, agent errors).
    pub issues: Vec<String>,
    /// Pending approvals of the project.
    pub pending_approvals: u32,
    /// Last snapshot.
    pub last_snapshot: Option<SnapshotBrief>,
    /// Active / resumable run.
    pub active_run: Option<ManagerRun>,
    /// Last Manager message or task change.
    pub last_activity_at: Option<Timestamp>,
    /// Nothing happened in this project yet.
    pub is_empty: bool,
}

method!(
    /// `manager.chat` — send a message; the reply streams as events.
    Chat, "manager.chat", ManagerChatParams, ManagerChatResult
);
method!(
    /// `manager.history` — conversation, summaries, plans, active run.
    History, "manager.history", ManagerHistoryParams, ManagerHistoryResult
);
method!(
    /// `manager.clear` — archive the conversation (with a summary) and start a new one.
    Clear, "manager.clear", ManagerProjectParams, ManagerClearResult
);
method!(
    /// `manager.plan` — study the project and propose a structured plan.
    PlanMethod, "manager.plan", ManagerPlanParams, ManagerPlanResult
);
method!(
    /// `manager.approvePlan` — create the tasks (+ dependencies) and start a run.
    ApprovePlan, "manager.approvePlan", ApprovePlanParams, ApprovePlanResult
);
method!(
    /// `manager.rejectPlan`.
    RejectPlan, "manager.rejectPlan", RejectPlanParams, ManagerPlanResult
);
method!(
    /// `manager.run` — start a run for an approved plan or continue one.
    Run, "manager.run", ManagerRunParams, ManagerRunResult
);
method!(
    /// `manager.pause` — stop scheduling and pause running agents.
    Pause, "manager.pause", ManagerRunIdParams, ManagerRunResult
);
method!(
    /// `manager.resume` — continue a paused or interrupted run.
    Resume, "manager.resume", ManagerRunIdParams, ManagerRunResult
);
method!(
    /// `manager.cancel` — stop the run and its agents; unfinished tasks are cancelled.
    Cancel, "manager.cancel", ManagerRunIdParams, ManagerRunResult
);
method!(
    /// `manager.status` — runs and their progress.
    Status, "manager.status", ManagerStatusParams, ManagerStatusResult
);
method!(
    /// `manager.resumeSummary` — "Last session" summary (§57).
    ResumeSummaryMethod, "manager.resumeSummary", ManagerProjectParams, ManagerResumeSummary
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_are_optional_on_the_wire() {
        let p: ManagerChatParams =
            serde_json::from_value(serde_json::json!({"projectId": ProjectId::new(), "text": "hi"})).unwrap();
        assert!(p.options.is_none() && p.attachment.is_none());
        let r: ManagerRunParams = serde_json::from_value(serde_json::json!({})).unwrap();
        assert_eq!(r, ManagerRunParams::default());
        assert!(ManagerRunStatus::Completed.is_final());
        assert!(!ManagerRunStatus::Interrupted.is_final());
    }
}
