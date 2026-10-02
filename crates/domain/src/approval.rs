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
    /// Exact command / action preview (redacted) shown to the approver.
    #[serde(default)]
    pub command_preview: Option<String>,
    /// Dangerous-action category when the guard flagged the action.
    #[serde(default)]
    pub danger: Option<DangerCategory>,
}

/// Category of a dangerous or forbidden action (permission model §6.2–6.3).
/// Append-only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DangerCategory {
    /// Recursive or mass delete (D1).
    RecursiveDelete,
    /// Delete of a protected root: drive/FS root, home, OS dirs (H1).
    ProtectedPathDelete,
    /// Disk / volume destruction: format, diskpart, mkfs, dd to a device (H2).
    DiskDestruction,
    /// Backup / recovery / boot destruction (H3).
    BackupDestruction,
    /// Fork bomb / resource exhaustion (H4).
    ResourceExhaustion,
    /// Disabling security tooling (H5).
    SecurityToolingDisable,
    /// Audit / log wiping (H6).
    LogWipe,
    /// Reading other apps' credentials (H7).
    CredentialAccess,
    /// Tampering with Orkestr itself (H8).
    SelfTamper,
    /// Unsafe agent-CLI modes (H9).
    UnsafeAgentMode,
    /// Exposing local core ports (H10).
    PortExposure,
    /// Destructive / history-rewriting / outward git operation (D3).
    GitDestructive,
    /// Direct writes to `.git` internals.
    GitInternals,
    /// Data destruction: DROP/TRUNCATE/DELETE without WHERE… (D4).
    DataDestruction,
    /// Production write / deploy / publish (D5).
    ProductionChange,
    /// Secret changes or exposure (D6).
    SecretChange,
    /// Privilege escalation: sudo, runas (D7).
    PrivilegeEscalation,
    /// OS configuration / persistence / registry (D8).
    SystemConfig,
    /// Network configuration / tunnels / firewall (D9).
    NetworkConfig,
    /// Download-and-execute / dynamic code (D10).
    RemoteCodeExecution,
    /// Container / VM destruction (D11).
    ContainerDestruction,
    /// Obfuscated or too deeply nested command (D12).
    Obfuscation,
    /// Command could not be parsed (D12).
    Unparseable,
    /// Write/delete outside the confinement root (D2).
    OutsideWorktree,
}
