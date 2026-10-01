//! Audit events (§23, §50, §52). Details must be redacted before construction.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::Capability;
use crate::common::{Actor, Timestamp};
use crate::ids::{AuditEventId, CorrelationId, ProjectId};

/// Outcome recorded in the audit log.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AuditOutcome {
    /// Allowed by policy.
    Allowed,
    /// Denied by policy.
    Denied,
    /// Approved by a user.
    Approved,
    /// Rejected by a user.
    Rejected,
    /// Executed successfully.
    Executed,
    /// Execution failed.
    Failed,
}

/// Immutable audit record.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AuditEvent {
    /// Id.
    pub id: AuditEventId,
    /// Time.
    pub ts: Timestamp,
    /// Actor.
    pub actor: Actor,
    /// Action name (`fs.delete`, `secrets.read`, `deploy.run`, …).
    pub action: String,
    /// Capability involved.
    pub capability: Option<Capability>,
    /// Resource (path, host, secret name — never a secret value).
    pub resource: String,
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Outcome.
    pub outcome: AuditOutcome,
    /// Redacted details.
    pub details: serde_json::Value,
    /// Correlation id.
    pub correlation_id: Option<CorrelationId>,
}
