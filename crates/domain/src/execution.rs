//! Executions (task attempts) and tool calls (§10, §21 Replay).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::Capability;
use crate::common::{ModelRef, Timestamp, UsageSummary};
use crate::ids::{AgentId, ApprovalId, ExecutionId, TaskId, ToolCallId};

/// Status of an execution attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ExecutionStatus {
    /// In progress.
    Running,
    /// Finished successfully.
    Succeeded,
    /// Finished with failure.
    Failed,
    /// Cancelled by user/budget/emergency stop.
    Cancelled,
    /// Interrupted by crash/shutdown.
    Interrupted,
}

/// One attempt of an agent at a task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Execution {
    /// Id.
    pub id: ExecutionId,
    /// Task.
    pub task_id: TaskId,
    /// Agent.
    pub agent_id: AgentId,
    /// Attempt number starting at 1.
    pub attempt: u32,
    /// Status.
    pub status: ExecutionStatus,
    /// Model used (may change on fallback; see events).
    pub model: Option<ModelRef>,
    /// Usage.
    pub usage: UsageSummary,
    /// Summary of what happened (no hidden chain-of-thought).
    pub summary: Option<String>,
    /// Error (redacted).
    pub error: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Option<Timestamp>,
}

/// Status of a tool call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ToolCallStatus {
    /// Queued.
    Pending,
    /// Waiting for user approval.
    AwaitingApproval,
    /// Executing.
    Running,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Denied by policy or user.
    Denied,
    /// Cancelled.
    Cancelled,
}

/// A tool invocation by an agent. Inputs/outputs are stored redacted.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ToolCall {
    /// Id.
    pub id: ToolCallId,
    /// Execution.
    pub execution_id: ExecutionId,
    /// Agent.
    pub agent_id: AgentId,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Tool name, e.g. `fs.write`, `terminal.run`.
    pub tool: String,
    /// Capability checked for this call.
    pub capability: Capability,
    /// Redacted input arguments.
    pub input: serde_json::Value,
    /// Short output summary (full output goes to the object store).
    pub output_summary: Option<String>,
    /// Status.
    pub status: ToolCallStatus,
    /// Approval, when one was required.
    pub approval_id: Option<ApprovalId>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    pub finished_at: Option<Timestamp>,
}
