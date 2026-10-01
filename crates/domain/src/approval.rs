//! Approval requests (§23, §37).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::Capability;
use crate::common::{Actor, RiskLevel, Timestamp};
use crate::ids::{AgentId, ApprovalId, ProjectId, TaskId, ToolCallId};

/// Lifecycle of an approval.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApprovalStatus {
    /// Awaiting a decision.
    Pending,
    /// Approved.
    Approved,
    /// Rejected.
    Rejected,
    /// Timed out.
    Expired,
    /// The requesting action was cancelled.
    Cancelled,
}

/// A request for the user to approve an action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Approval {
    /// Id.
    pub id: ApprovalId,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Requesting agent.
    pub agent_id: Option<AgentId>,
    /// Tool call awaiting the decision.
    pub tool_call_id: Option<ToolCallId>,
    /// Capability requested.
    pub capability: Capability,
    /// One-line description of the action ("Run `git push --force` in agent/backend").
    pub action_summary: String,
    /// Risk.
    pub risk: RiskLevel,
    /// Why approval is needed (policy/guard reason).
    pub reason: String,
    /// Status.
    pub status: ApprovalStatus,
    /// Request time.
    pub requested_at: Timestamp,
    /// Optional expiry.
    pub expires_at: Option<Timestamp>,
    /// Decision time.
    pub resolved_at: Option<Timestamp>,
    /// Who decided.
    pub resolved_by: Option<Actor>,
    /// Optional note by the resolver.
    pub resolution_note: Option<String>,
}
