//! Strongly typed identifiers.
//!
//! Every entity id is a UUID v7 (time-ordered) wrapped in a newtype so that a
//! `TaskId` can never be passed where a `ProjectId` is expected. On the wire
//! (serde, TypeScript) ids are plain strings.

use std::fmt;
use std::str::FromStr;

use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

/// Error returned when a string is not a valid id.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("invalid {kind} id `{value}`: {reason}")]
pub struct IdParseError {
    /// Human readable id kind, e.g. `project`.
    pub kind: &'static str,
    /// The offending input (truncated to 64 chars).
    pub value: String,
    /// Underlying parse failure.
    pub reason: String,
}

macro_rules! define_id {
    ($(#[$meta:meta])* $name:ident, $kind:literal) => {
        $(#[$meta])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
        #[ts(export)]
        pub struct $name(Uuid);

        impl $name {
            /// Human readable kind of this id (used in error messages and topics).
            pub const KIND: &'static str = $kind;

            /// Generates a fresh, time-ordered (UUID v7) id.
            #[must_use]
            pub fn new() -> Self {
                Self(Uuid::now_v7())
            }

            /// Wraps an existing UUID.
            #[must_use]
            pub const fn from_uuid(uuid: Uuid) -> Self {
                Self(uuid)
            }

            /// Returns the inner UUID.
            #[must_use]
            pub const fn as_uuid(&self) -> &Uuid {
                &self.0
            }

            /// Parses an id from its string form.
            pub fn parse(value: &str) -> Result<Self, IdParseError> {
                Uuid::parse_str(value).map(Self).map_err(|e| IdParseError {
                    kind: $kind,
                    value: value.chars().take(64).collect(),
                    reason: e.to_string(),
                })
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl FromStr for $name {
            type Err = IdParseError;
            fn from_str(s: &str) -> Result<Self, Self::Err> {
                Self::parse(s)
            }
        }

        impl From<Uuid> for $name {
            fn from(value: Uuid) -> Self {
                Self(value)
            }
        }

        impl From<$name> for Uuid {
            fn from(value: $name) -> Self {
                value.0
            }
        }
    };
}

define_id!(/// Local user profile id.
    UserId, "user");
define_id!(/// Paired device (desktop, browser, phone) id.
    DeviceId, "device");
define_id!(/// Compute node (§38) id.
    NodeId, "node");
define_id!(/// Project id.
    ProjectId, "project");
define_id!(/// Repository id.
    RepositoryId, "repository");
define_id!(/// Workspace (main checkout or worktree) id.
    WorkspaceId, "workspace");
define_id!(/// Workspace snapshot id (§25).
    SnapshotId, "snapshot");
define_id!(/// Configured AI provider instance id.
    ProviderId, "provider");
define_id!(/// Provider credential reference id.
    CredentialRefId, "credential");
define_id!(/// Agent template id (§6).
    AgentTemplateId, "agentTemplate");
define_id!(/// Running agent instance id. Floating robots (§30) bind to this.
    AgentId, "agent");
define_id!(/// Agent team id.
    AgentTeamId, "agentTeam");
define_id!(/// Task id (§10).
    TaskId, "task");
define_id!(/// Execution (one attempt of a task by an agent) id.
    ExecutionId, "execution");
define_id!(/// Tool call id.
    ToolCallId, "toolCall");
define_id!(/// Approval request id.
    ApprovalId, "approval");
define_id!(/// Memory item id (§20).
    MemoryItemId, "memory");
define_id!(/// Knowledge source id.
    KnowledgeSourceId, "knowledge");
define_id!(/// Decision log entry id (§21).
    DecisionId, "decision");
define_id!(/// Environment id (§26).
    EnvironmentId, "environment");
define_id!(/// Deployment id.
    DeploymentId, "deployment");
define_id!(/// Secret reference id (§24). Never the secret itself.
    SecretRefId, "secret");
define_id!(/// Usage record id (§22).
    UsageRecordId, "usage");
define_id!(/// Audit event id.
    AuditEventId, "audit");
define_id!(/// Plugin id (§48).
    PluginId, "plugin");
define_id!(/// MCP server id (§19).
    McpServerId, "mcpServer");
define_id!(/// Notification id.
    NotificationId, "notification");
define_id!(/// Event id (each published event gets one in addition to its `seq`).
    EventId, "event");
define_id!(/// Correlation id linking events/logs that belong to one user request (§50).
    CorrelationId, "correlation");
define_id!(/// Terminal session id.
    TerminalId, "terminal");
define_id!(/// RPC client session id.
    SessionId, "session");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_roundtrip_through_strings_and_json() {
        let id = ProjectId::new();
        let s = id.to_string();
        assert_eq!(ProjectId::parse(&s).unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{s}\""));
        let back: ProjectId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn ids_are_v7_and_time_ordered() {
        let a = TaskId::new();
        let b = TaskId::new();
        assert_eq!(a.as_uuid().get_version_num(), 7);
        assert!(a <= b, "v7 ids must sort by creation time");
    }

    #[test]
    fn invalid_id_reports_kind() {
        let err = AgentId::parse("nope").unwrap_err();
        assert_eq!(err.kind, "agent");
        assert!(err.to_string().contains("invalid agent id"));
    }
}
