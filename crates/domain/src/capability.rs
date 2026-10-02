//! Capabilities, permission rules and approval modes (§23, ENGINEERING_CONTRACT §6).
//!
//! The *evaluation* of rules lives in `orkestr-security`; this module only
//! defines the vocabulary.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::common::Timestamp;
use crate::ids::{AgentId, ProjectId, TaskId};

/// Something an actor may be allowed to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Capability {
    /// Read files inside the confined root.
    Read,
    /// Create/modify files.
    Write,
    /// Delete files.
    Delete,
    /// Run terminal commands.
    Terminal,
    /// Git operations.
    Git,
    /// Outbound network access.
    Network,
    /// Browser automation (§18).
    Browser,
    /// Database reads.
    DbRead,
    /// Database writes.
    DbWrite,
    /// Docker / containers.
    Docker,
    /// SSH to remote hosts.
    Ssh,
    /// Deployments (§26).
    Deploy,
    /// Mouse/keyboard/screen control (§18). Never granted by default.
    ComputerControl,
    /// Access to secrets (§24).
    Secrets,
}

impl Capability {
    /// All capabilities in a stable order.
    pub const ALL: [Capability; 14] = [
        Capability::Read,
        Capability::Write,
        Capability::Delete,
        Capability::Terminal,
        Capability::Git,
        Capability::Network,
        Capability::Browser,
        Capability::DbRead,
        Capability::DbWrite,
        Capability::Docker,
        Capability::Ssh,
        Capability::Deploy,
        Capability::ComputerControl,
        Capability::Secrets,
    ];

    /// Capabilities that are never granted implicitly and default to `Ask`/`Deny`.
    #[must_use]
    pub fn is_sensitive(self) -> bool {
        matches!(
            self,
            Capability::Delete
                | Capability::DbWrite
                | Capability::Ssh
                | Capability::Deploy
                | Capability::ComputerControl
                | Capability::Secrets
        )
    }
}

/// How much autonomy agents get (§23). `FullAuto` never overrides hard policies
/// or the dangerous-action guard.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ApprovalMode {
    /// Every non-read action asks.
    #[default]
    Ask,
    /// Routine actions inside the sandbox run; risky ones ask.
    SemiAuto,
    /// Everything allowed by policy runs; dangerous actions still ask.
    FullAuto,
}

/// Effect of a permission rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PermissionEffect {
    /// Allowed without asking.
    Allow,
    /// Requires user approval.
    Ask,
    /// Never allowed.
    Deny,
}

/// Scope a permission rule applies to. Narrowest explicit rule wins.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(tag = "scope", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum PermissionScope {
    /// Applies everywhere.
    Global,
    /// Applies to one project.
    Project {
        /// Target project.
        project_id: ProjectId,
    },
    /// Applies to one agent instance.
    Agent {
        /// Target agent.
        agent_id: AgentId,
    },
    /// Applies to one task.
    Task {
        /// Target task.
        task_id: TaskId,
    },
}

impl PermissionScope {
    /// Specificity rank: higher = narrower (Global 0 … Task 3).
    #[must_use]
    pub fn specificity(&self) -> u8 {
        match self {
            PermissionScope::Global => 0,
            PermissionScope::Project { .. } => 1,
            PermissionScope::Agent { .. } => 2,
            PermissionScope::Task { .. } => 3,
        }
    }
}

/// A single permission rule.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PermissionRule {
    /// Capability governed by this rule.
    pub capability: Capability,
    /// Effect when the rule matches.
    pub effect: PermissionEffect,
    /// Optional resource glob (path, host, command prefix…). `None` = any.
    pub resource: Option<String>,
    /// Where the rule applies.
    pub scope: PermissionScope,
}

/// A persisted permission rule with identity and metadata (Phase 1 additive).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PermissionRuleRecord {
    /// Rule id (UUID v7 string).
    pub id: String,
    /// The rule itself.
    pub rule: PermissionRule,
    /// Optional note ("allow cargo in backend").
    #[serde(default)]
    pub note: Option<String>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Optional expiry; expired rules are ignored.
    #[serde(default)]
    pub expires_at: Option<Timestamp>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_specificity_orders_narrowest_last() {
        let g = PermissionScope::Global.specificity();
        let p = PermissionScope::Project { project_id: ProjectId::new() }.specificity();
        let a = PermissionScope::Agent { agent_id: AgentId::new() }.specificity();
        let t = PermissionScope::Task { task_id: TaskId::new() }.specificity();
        assert!(g < p && p < a && a < t);
    }

    #[test]
    fn computer_control_is_sensitive() {
        assert!(Capability::ComputerControl.is_sensitive());
        assert!(!Capability::Read.is_sensitive());
        assert_eq!(Capability::ALL.len(), 14);
    }

    #[test]
    fn scope_json_shape() {
        let id = ProjectId::new();
        let v = serde_json::to_value(PermissionScope::Project { project_id: id }).unwrap();
        assert_eq!(v["scope"], "Project");
        assert_eq!(v["projectId"], id.to_string());
    }
}
