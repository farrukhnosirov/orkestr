//! `permissions.*` — permission rules, approval modes and dry-run checks (§23).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    Actor, AgentId, ApprovalMode, Capability, DangerCategory, PermissionRule, PermissionRuleRecord, ProjectId,
    RiskLevel, TaskId, Timestamp,
};

use super::method;

/// Params of `permissions.list`. Without filters every rule is returned;
/// with filters: Global rules plus those of the given project/agent/task.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRulesParams {
    /// Project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
}

/// Result of `permissions.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRulesResult {
    /// Rules (oldest first).
    pub rules: Vec<PermissionRuleRecord>,
}

/// Params of `permissions.set` (create when `id` is absent, else replace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetRuleParams {
    /// Existing rule id to replace.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub id: Option<String>,
    /// The rule.
    pub rule: PermissionRule,
    /// Note.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
    /// Expiry.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expires_at: Option<Timestamp>,
}

/// Result of `permissions.set`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RuleResult {
    /// The stored rule.
    pub rule: PermissionRuleRecord,
}

/// Params of `permissions.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteRuleParams {
    /// Rule id.
    pub id: String,
}

/// Result of delete methods.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeletedResult {
    /// Whether something was deleted.
    pub deleted: bool,
}

/// Shell of a command resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CommandShell {
    /// PowerShell 5.1 / 7.
    PowerShell,
    /// cmd.exe.
    Cmd,
    /// bash / zsh / Git Bash / WSL.
    Bash,
    /// POSIX sh / fish.
    Sh,
}

/// What a checked action touches.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum CheckResource {
    /// Capability-level check.
    None,
    /// Filesystem path.
    Path {
        /// Path (relative to the worktree root, or absolute).
        path: String,
        /// Recursive (for Delete).
        #[serde(default)]
        recursive: bool,
    },
    /// Shell command.
    Command {
        /// Command line.
        command: String,
        /// Shell.
        shell: CommandShell,
        /// Working directory.
        #[serde(default)]
        cwd: Option<String>,
    },
    /// Git arguments.
    Git {
        /// Arguments (without `git`).
        args: Vec<String>,
    },
    /// URL.
    Url {
        /// URL.
        url: String,
    },
    /// Secret name.
    Secret {
        /// Name.
        name: String,
    },
    /// SQL statement.
    Sql {
        /// Statement.
        statement: String,
    },
    /// Deployment.
    Deploy {
        /// Environment.
        environment: String,
        /// Protected environment.
        protected: bool,
    },
    /// Other.
    Other {
        /// Value.
        value: String,
    },
}

/// Params of `permissions.check` (dry run; nothing is executed or requested).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckParams {
    /// Actor (default: an agent with `agentId`, or the Manager).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub actor: Option<Actor>,
    /// Capability.
    pub capability: Capability,
    /// Resource.
    pub resource: CheckResource,
    /// Project (its approval mode is used).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Confinement root (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree_root: Option<String>,
    /// Override the approval mode.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_mode: Option<ApprovalMode>,
}

/// Decision kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DecisionKind {
    /// Allowed.
    Allow,
    /// Needs approval.
    Ask,
    /// Denied.
    Deny,
}

/// Wire form of a permission decision.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DecisionView {
    /// Kind.
    pub decision: DecisionKind,
    /// Machine code (empty for Allow).
    pub code: String,
    /// Human message.
    pub message: String,
    /// Risk (Ask).
    pub risk: Option<RiskLevel>,
    /// Guard category.
    pub danger: Option<DangerCategory>,
    /// "Remember" may be offered (Ask).
    pub rememberable: bool,
    /// Hard deny (Deny).
    pub hard: bool,
    /// Matching rule.
    pub rule_id: Option<String>,
    /// Effective approval mode used.
    pub effective_mode: ApprovalMode,
}

/// Params of `permissions.getMode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetModeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent (for its override).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Params of `permissions.setMode`: with `agentId` sets (or with `mode: null`
/// clears) the agent override; otherwise sets the project mode.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetModeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Mode (`null` clears an agent override; required for projects).
    pub mode: Option<ApprovalMode>,
}

/// Result of the mode methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ModeResult {
    /// Project.
    pub project_id: ProjectId,
    /// Project mode.
    pub project_mode: ApprovalMode,
    /// Agent.
    pub agent_id: Option<AgentId>,
    /// Agent override.
    pub agent_mode: Option<ApprovalMode>,
    /// Effective mode (after the emergency FullAuto cap).
    pub effective_mode: ApprovalMode,
    /// Emergency `fullAutoDisabled` flag.
    pub full_auto_disabled: bool,
}

method!(
    /// `permissions.list`.
    List, "permissions.list", ListRulesParams, ListRulesResult
);
method!(
    /// `permissions.set` — users only; audited.
    Set, "permissions.set", SetRuleParams, RuleResult
);
method!(
    /// `permissions.delete` — audited.
    Delete, "permissions.delete", DeleteRuleParams, DeletedResult
);
method!(
    /// `permissions.check` — dry-run evaluation.
    Check, "permissions.check", CheckParams, DecisionView
);
method!(
    /// `permissions.getMode`.
    GetMode, "permissions.getMode", GetModeParams, ModeResult
);
method!(
    /// `permissions.setMode` — emits `ProjectUpdated` for project modes.
    SetMode, "permissions.setMode", SetModeParams, ModeResult
);
