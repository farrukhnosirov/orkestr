//! Agent templates, instances and teams (§6, §30).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::PermissionRule;
use crate::common::{Budget, ModelRef, Timestamp, UsageSummary, WorktreeRef};
use crate::ids::{AgentId, AgentTeamId, AgentTemplateId, ProjectId, TaskId};
use crate::memory::MemoryAccess;

/// Built-in agent roles (§6). `Custom` carries a free-form role name.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentRole {
    /// The AI Manager itself.
    Manager,
    /// Architect.
    Architect,
    /// Backend engineer.
    Backend,
    /// Frontend engineer.
    Frontend,
    /// Database engineer.
    Database,
    /// QA / testing.
    Qa,
    /// Security reviewer.
    Security,
    /// DevOps.
    DevOps,
    /// UI/UX designer.
    UiUx,
    /// AI Product Manager (§9).
    ProductManager,
    /// Code reviewer.
    Reviewer,
    /// Researcher.
    Researcher,
    /// User-defined role.
    Custom(String),
}

/// Runtime state of an agent instance (contract §5). Robot states (§30) derive from these.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AgentState {
    /// Spawned, no work.
    #[default]
    Idle,
    /// Listening to voice input.
    Listening,
    /// Waiting on the model.
    Thinking,
    /// Editing code / running tools.
    Coding,
    /// Running tests.
    Testing,
    /// Reviewing.
    Reviewing,
    /// Waiting for approval/user/provider.
    Waiting,
    /// Failed.
    Error,
    /// Finished its task.
    Completed,
    /// Stopped (by user, budget or emergency stop).
    Stopped,
}

impl AgentState {
    /// Whether the agent is actively consuming resources.
    #[must_use]
    pub fn is_busy(self) -> bool {
        matches!(self, AgentState::Thinking | AgentState::Coding | AgentState::Testing | AgentState::Reviewing)
    }

    /// Whether the state is final for the current run.
    #[must_use]
    pub fn is_finished(self) -> bool {
        matches!(self, AgentState::Completed | AgentState::Stopped | AgentState::Error)
    }
}

/// How an agent gets a working copy (§16).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WorktreePolicy {
    /// Own branch + worktree (default for parallel agents).
    #[default]
    DedicatedWorktree,
    /// Shares the task's worktree (pair work, reviewers).
    SharedWorktree,
    /// Read-only access to the main checkout (researchers, reviewers).
    ReadOnlyMain,
    /// No filesystem access.
    None,
}

/// Quality gates (§58).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum QualityGate {
    /// Build passes.
    Build,
    /// Lint passes.
    Lint,
    /// Typecheck passes.
    Typecheck,
    /// Tests pass.
    Tests,
    /// No critical security finding.
    Security,
    /// Reviewer approved.
    Review,
    /// Acceptance criteria met.
    AcceptanceCriteria,
}

/// TTS voice of an agent (§33 voice per agent).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VoiceProfile {
    /// TTS provider id (e.g. `system`, `openai`, `elevenlabs`).
    pub provider: String,
    /// Voice name.
    pub voice: String,
    /// BCP-47 language.
    pub language: String,
    /// Optional speaking style.
    pub style: Option<String>,
}

/// Visual representation of an agent (§30 floating robots).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AvatarConfig {
    /// Robot model/skin id from the Robot Designer.
    pub robot: String,
    /// Accent color (hex) — UI falls back to theme tokens when absent.
    pub accent: Option<String>,
    /// Scale factor (0.25..4).
    pub scale: f32,
    /// Whether this agent shows a floating robot.
    pub floating: bool,
}

/// Where a template comes from (§6 Agent Marketplace).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TemplateSource {
    /// Shipped with Orkestr.
    Builtin,
    /// Official marketplace.
    Official,
    /// Community marketplace.
    Community,
    /// Private (team/company) catalog.
    Private,
    /// Created locally by the user.
    Custom,
}

/// Agent template (§6) — all fields from the spec.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentTemplate {
    /// Id.
    pub id: AgentTemplateId,
    /// Name.
    pub name: String,
    /// Role.
    pub role: AgentRole,
    /// Short description.
    pub description: String,
    /// System prompt.
    pub system_prompt: String,
    /// Preferred model.
    pub preferred_model: Option<ModelRef>,
    /// Fallback chain (§7), in order.
    pub fallback_models: Vec<ModelRef>,
    /// Tool names the agent may use.
    pub tools: Vec<String>,
    /// Permission rules (evaluated by orkestr-security).
    pub permissions: Vec<PermissionRule>,
    /// Memory visibility.
    pub memory_access: MemoryAccess,
    /// Budget per run.
    pub budget: Option<Budget>,
    /// Max tool-loop turns per run.
    pub max_turns: u32,
    /// Worktree policy.
    pub worktree_policy: WorktreePolicy,
    /// Quality gates the agent's output must pass.
    pub quality_gates: Vec<QualityGate>,
    /// Voice.
    pub voice: Option<VoiceProfile>,
    /// Avatar / robot.
    pub avatar: Option<AvatarConfig>,
    /// Initial visual state of the robot.
    pub visual_state: AgentState,
    /// Origin.
    pub source: TemplateSource,
    /// Template version (semver string).
    pub version: String,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
}

/// A running (or finished) agent. Robots bind to `id`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentInstance {
    /// Id.
    pub id: AgentId,
    /// Template it was spawned from.
    pub template_id: Option<AgentTemplateId>,
    /// Project.
    pub project_id: ProjectId,
    /// Current task.
    pub task_id: Option<TaskId>,
    /// Team membership.
    pub team_id: Option<AgentTeamId>,
    /// Parent in the hierarchy (Manager → Lead → Executor).
    pub parent_agent_id: Option<AgentId>,
    /// Display name ("Backend Agent #2").
    pub name: String,
    /// Role.
    pub role: AgentRole,
    /// State.
    pub state: AgentState,
    /// Model currently used.
    pub model: Option<ModelRef>,
    /// Worktree.
    pub worktree: Option<WorktreeRef>,
    /// Accumulated usage.
    pub usage: UsageSummary,
    /// Turns used in the current run.
    pub turns_used: u32,
    /// Last error message (redacted).
    pub last_error: Option<String>,
    /// Spawn time.
    pub created_at: Timestamp,
    /// Start of the current run.
    pub started_at: Option<Timestamp>,
    /// Stop time.
    pub stopped_at: Option<Timestamp>,
    /// Last activity.
    pub last_activity_at: Option<Timestamp>,
    /// Template version pinned at spawn (Phase 1, additive).
    #[serde(default)]
    pub template_version: Option<String>,
    /// Worktree policy the agent runs with (Phase 1, additive).
    #[serde(default)]
    pub worktree_policy: WorktreePolicy,
    /// Turn limit of the current run (Phase 1, additive; 0 = unknown).
    #[serde(default)]
    pub max_turns: u32,
    /// Budget of the current run (Phase 1, additive).
    #[serde(default)]
    pub budget: Option<Budget>,
    /// Current execution (Phase 1, additive).
    #[serde(default)]
    pub execution_id: Option<crate::ids::ExecutionId>,
    /// Execution style: `chat` (tool loop) or `cli` (official CLI coding agent). Phase 1, additive.
    #[serde(default)]
    pub execution_style: Option<String>,
    /// CLI session id of a coding-agent run (resume). Phase 1, additive.
    #[serde(default)]
    pub cli_session: Option<String>,
    /// Short human-readable current activity ("Running cargo test"). Phase 1, additive.
    #[serde(default)]
    pub activity: Option<String>,
}

/// Predefined team kinds (§6).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TeamKind {
    /// ERP team.
    Erp,
    /// Web team.
    Web,
    /// Mobile team.
    Mobile,
    /// Security team.
    Security,
    /// Rapid prototype team.
    RapidPrototype,
    /// Custom.
    Custom(String),
}

/// A member slot of a team.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamMember {
    /// Template to instantiate.
    pub template_id: AgentTemplateId,
    /// Team lead in the hierarchy.
    pub is_lead: bool,
    /// How many instances to spawn.
    pub count: u32,
}

/// Agent team (§6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AgentTeam {
    /// Id.
    pub id: AgentTeamId,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: TeamKind,
    /// Description.
    pub description: String,
    /// Members.
    pub members: Vec<TeamMember>,
    /// Project-specific team, or global when `None`.
    pub project_id: Option<ProjectId>,
    /// Creation time.
    pub created_at: Timestamp,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_json() {
        assert_eq!(serde_json::to_value(AgentRole::Backend).unwrap(), "Backend");
        let v = serde_json::to_value(AgentRole::Custom("Payroll".into())).unwrap();
        assert_eq!(v, serde_json::json!({"Custom": "Payroll"}));
    }

    #[test]
    fn state_helpers() {
        assert!(AgentState::Coding.is_busy());
        assert!(!AgentState::Waiting.is_busy());
        assert!(AgentState::Stopped.is_finished());
    }
}
