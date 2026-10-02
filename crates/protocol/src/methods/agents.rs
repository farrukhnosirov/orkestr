//! `agents.*` — Agent Runtime (§6, §30, §57; agent-runtime-spec).
//!
//! Events (topics `project:<id>`, `task:<id>`, `agent:<id>`):
//! `AgentSpawned`, `AgentStarted`, `AgentStateChanged`, `AgentUpdated`
//! (usage/turns/model/activity), `AgentStopped` (the agent's run ended; the
//! final state stays visible), `AgentMessage` (streamed text deltas — never
//! hidden reasoning), `ToolCallStarted`, `ToolCallFinished`,
//! `ApprovalRequested`/`ApprovalResolved` (approvals of agent tool calls),
//! `FileChanged` (agent writes), `SnapshotCreated` (checkpoints) and
//! `ProviderFallback` (published by the provider hub).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentId, AgentInstance, AgentRole, AgentState, AgentTemplateId, ApprovalMode, Budget, ExecutionId, ModelRef,
    ProjectId, SnapshotId, Task, TaskId, TaskPriority, Timestamp, ToolCall, ToolCallId, UsageSummary, WorktreePolicy,
};

use super::method;
use crate::event::MessageRole;

/// Max length of an agent display name.
pub const MAX_AGENT_NAME_LEN: usize = 80;

/// A task created together with the agent (`agents.spawn`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct NewTaskSpec {
    /// Title.
    pub title: String,
    /// Description (markdown).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Acceptance criteria.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub acceptance_criteria: Option<Vec<String>>,
    /// Priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<TaskPriority>,
}

/// Params of `agents.spawn`. Exactly one of `taskId` / `newTask` is required.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpawnAgentParams {
    /// Project.
    pub project_id: ProjectId,
    /// Template to instantiate.
    pub template_id: AgentTemplateId,
    /// Existing task (Planned/Ready, or Failed/Interrupted for a retry).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Task to create.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub new_task: Option<NewTaskSpec>,
    /// Model override (otherwise template preference, then routing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Worktree policy override.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_policy: Option<WorktreePolicy>,
    /// Display name (default "<template name> #<n>").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Turn limit override (1..=500; never unlimited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_turns: Option<u32>,
    /// Base branch / commit of a dedicated worktree (default: the project's
    /// current HEAD). The AI Manager bases dependent tasks on the branch of
    /// their prerequisite so the agent sees its changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base_branch: Option<String>,
    /// Approval-mode cap for this agent: it can only make the effective mode
    /// stricter (`Ask` < `SemiAuto` < `FullAuto`), never looser.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_mode: Option<ApprovalMode>,
    /// Budget cap for this run; each limit only tightens the template budget.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub budget: Option<Budget>,
}

/// Result of `agents.spawn`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SpawnAgentResult {
    /// The new agent.
    pub agent: AgentInstance,
    /// Its task (as of spawn).
    pub task: Task,
}

/// Params of `agents.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListAgentsParams {
    /// Only agents of this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Only agents working on this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Only agents whose run is still active.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active_only: Option<bool>,
}

/// Result of `agents.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListAgentsResult {
    /// Agents, newest first.
    pub agents: Vec<AgentInstance>,
}

/// Params addressing one agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentIdParams {
    /// Agent.
    pub agent_id: AgentId,
}

/// Result carrying one agent.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SingleAgentResult {
    /// The agent.
    pub agent: AgentInstance,
}

/// What `agents.stop` does with the agent's task.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentStopMode {
    /// Task → `Interrupted` (resumable via retry). Default.
    #[default]
    Interrupt,
    /// Task → `Cancelled`.
    Cancel,
}

/// Params of `agents.stop`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStopParams {
    /// Agent.
    pub agent_id: AgentId,
    /// Task outcome (default `Interrupt`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub mode: Option<AgentStopMode>,
}

/// Params of `agents.transcript`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TranscriptParams {
    /// Agent.
    pub agent_id: AgentId,
    /// Max messages (newest kept; default 500, max 2000).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// One persisted transcript message (visible text only — never hidden
/// reasoning, §1.14 / §21).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentTranscriptMessage {
    /// Stable id (equals the `messageId` of the streamed `AgentMessage` deltas).
    pub id: String,
    /// Monotonic order.
    pub seq: u64,
    /// Agent.
    pub agent_id: AgentId,
    /// Execution.
    pub execution_id: Option<ExecutionId>,
    /// Author.
    pub role: MessageRole,
    /// Text (redacted).
    pub text: String,
    /// Tool calls requested by this (assistant) message, in order.
    pub tool_call_ids: Vec<ToolCallId>,
    /// Time.
    pub created_at: Timestamp,
}

/// A tool call with its (redacted, truncated) result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentToolCallView {
    /// The call (input redacted).
    pub tool_call: ToolCall,
    /// Result text returned to the model (redacted, ≤ 8 KiB).
    pub result: Option<String>,
    /// Duration in ms.
    pub duration_ms: Option<u64>,
}

/// Result of `agents.transcript`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentTranscriptResult {
    /// The agent.
    pub agent: AgentInstance,
    /// Messages, oldest first.
    pub messages: Vec<AgentTranscriptMessage>,
    /// Tool calls, oldest first.
    pub tool_calls: Vec<AgentToolCallView>,
}

/// Why a handoff was created (§57).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum HandoffReason {
    /// The model failed and the router switched to the next one.
    ProviderFallback,
    /// The agent was stopped (user / emergency / limit).
    Stopped,
    /// The agent finished (result submitted).
    Completed,
    /// The agent failed.
    Failed,
}

/// Agent side of a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffParty {
    /// Agent (absent for "to be decided").
    pub agent_id: Option<AgentId>,
    /// Role.
    pub role: Option<AgentRole>,
    /// Model.
    pub model: Option<ModelRef>,
}

/// Task section of a handoff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffTask {
    /// Task.
    pub task_id: TaskId,
    /// Title.
    pub title: String,
    /// Goal (description, truncated).
    pub goal: String,
    /// Acceptance criteria.
    pub acceptance_criteria: Vec<String>,
}

/// A file relevant to the work.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffFile {
    /// Worktree-relative path.
    pub path: String,
    /// Why it matters.
    pub why: String,
}

/// A decision taken (explicit summary, never chain-of-thought).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffDecision {
    /// Title.
    pub title: String,
    /// The choice.
    pub choice: String,
    /// User-facing rationale summary.
    pub rationale_summary: Option<String>,
}

/// Last known test state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffTestState {
    /// Command.
    pub command: String,
    /// Whether it passed (exit code 0).
    pub passed: bool,
    /// Short summary.
    pub summary: String,
}

/// An artifact reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffArtifact {
    /// `snapshot`, `commit`, `log`, …
    pub kind: String,
    /// Reference (snapshot id, commit id, object id).
    pub reference: String,
    /// Label.
    pub label: String,
}

/// Workspace state at handoff time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffWorkspace {
    /// Branch.
    pub branch: Option<String>,
    /// Worktree path.
    pub worktree: Option<String>,
    /// Latest checkpoint snapshot.
    pub snapshot_id: Option<SnapshotId>,
    /// Files changed by the agent.
    pub files_changed: Vec<String>,
}

/// Remaining budget.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandoffBudget {
    /// Turns left.
    pub turns: u32,
    /// USD left (when the agent has a cost limit).
    pub cost_usd: Option<f64>,
}

/// Structured handoff (§57, agent-runtime-spec §8). Never contains the raw
/// chat history; capped at ≈ 4k tokens.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Handoff {
    /// Id.
    pub handoff_id: String,
    /// Schema version (1).
    pub schema_version: u32,
    /// Creation time.
    pub created_at: Timestamp,
    /// Producer.
    pub from: HandoffParty,
    /// Receiver (when known).
    pub to: Option<HandoffParty>,
    /// Why.
    pub reason: HandoffReason,
    /// Task.
    pub task: HandoffTask,
    /// Concise state of the work (≤ 1500 tokens).
    pub context: String,
    /// Relevant files.
    pub relevant_files: Vec<HandoffFile>,
    /// Decisions.
    pub decisions: Vec<HandoffDecision>,
    /// Done so far.
    pub done: Vec<String>,
    /// Remaining work.
    pub remaining: Vec<String>,
    /// Unresolved issues.
    pub unresolved_issues: Vec<String>,
    /// Test state.
    pub test_state: Option<HandoffTestState>,
    /// Artifacts.
    pub artifacts: Vec<HandoffArtifact>,
    /// Workspace.
    pub workspace: HandoffWorkspace,
    /// Remaining budget.
    pub budget_remaining: HandoffBudget,
}

/// Result of `agents.handoff`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentHandoffsResult {
    /// Handoffs of the agent, newest first.
    pub handoffs: Vec<Handoff>,
}

/// Params of `agents.stats`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStatsParams {
    /// Only this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Agents in one state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStateCount {
    /// State.
    pub state: AgentState,
    /// Count.
    pub count: u32,
}

/// Result of `agents.stats`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentStats {
    /// Agents ever spawned (in scope).
    pub total: u32,
    /// Agents whose run is active.
    pub active: u32,
    /// Per-state counts.
    pub by_state: Vec<AgentStateCount>,
    /// Accumulated usage.
    pub usage: UsageSummary,
    /// Tool calls made.
    pub tool_calls: u64,
    /// Tool calls denied (policy or user).
    pub tool_calls_denied: u64,
    /// Turns used.
    pub turns: u64,
}

method!(
    /// `agents.list`.
    List, "agents.list", ListAgentsParams, ListAgentsResult
);
method!(
    /// `agents.get`.
    Get, "agents.get", AgentIdParams, SingleAgentResult
);
method!(
    /// `agents.spawn` — creates the instance (+ worktree) and starts its run.
    Spawn, "agents.spawn", SpawnAgentParams, SpawnAgentResult
);
method!(
    /// `agents.stop` — cancels the run locally (no provider round-trip).
    Stop, "agents.stop", AgentStopParams, SingleAgentResult
);
method!(
    /// `agents.pause` — the agent waits at the next turn/tool boundary.
    Pause, "agents.pause", AgentIdParams, SingleAgentResult
);
method!(
    /// `agents.resume` — resumes a paused agent.
    Resume, "agents.resume", AgentIdParams, SingleAgentResult
);
method!(
    /// `agents.transcript` — persisted messages + tool calls.
    Transcript, "agents.transcript", TranscriptParams, AgentTranscriptResult
);
method!(
    /// `agents.handoff` — structured handoffs produced by the agent.
    HandoffList, "agents.handoff", AgentIdParams, AgentHandoffsResult
);
method!(
    /// `agents.stats` — aggregate counters.
    Stats, "agents.stats", AgentStatsParams, AgentStats
);
