//! `approvals.*` — approval inbox (§23). Requests are created by engines
//! inside the core (`ApprovalRequested` event); clients list and resolve them.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{Approval, ApprovalId, ApprovalStatus, ProjectId};

use super::method;

/// Params of `approvals.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListApprovalsParams {
    /// Status filter (e.g. `Pending`); absent = history of all statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<ApprovalStatus>,
    /// Project filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Max rows (default 200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `approvals.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListApprovalsResult {
    /// Approvals, newest first.
    pub approvals: Vec<Approval>,
}

/// Params of `approvals.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovalIdParams {
    /// Approval.
    pub approval_id: ApprovalId,
}

/// Result with one approval.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ApprovalResult {
    /// The approval.
    pub approval: Approval,
}

/// User decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApprovalDecisionKind {
    /// Approve.
    Approve,
    /// Reject.
    Reject,
}

/// Params of `approvals.resolve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolveApprovalParams {
    /// Approval.
    pub approval_id: ApprovalId,
    /// Decision.
    pub decision: ApprovalDecisionKind,
    /// Optional note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
}

method!(
    /// `approvals.list`.
    List, "approvals.list", ListApprovalsParams, ListApprovalsResult
);
method!(
    /// `approvals.get`.
    Get, "approvals.get", ApprovalIdParams, ApprovalResult
);
method!(
    /// `approvals.resolve` — emits `ApprovalResolved`; `conflict` if not pending.
    Resolve, "approvals.resolve", ResolveApprovalParams, ApprovalResult
);
