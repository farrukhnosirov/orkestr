//! `teams.*` — persistent Agent Teams (§6), the optional hierarchy Manager →
//! Team Lead → Executors → Reviewers, and cross-project assignment with
//! per-project permission scoping (§68.27).
//!
//! Events: `TeamUpdated` (global), `TeamDeleted` (global), `TeamLeadRunUpdated`
//! (topic `project:<id>`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentId, AgentRole, AgentTeam, AgentTeamId, AgentTemplateId, Capability, ManagerPlanId, ManagerRunId, ModelRef,
    ProjectId, TaskId, TaskStatus, TeamKind, TeamMember, TeamPolicies, Timestamp,
};

use super::method;

/// Max members (slots) of one team.
pub const MAX_TEAM_MEMBERS: usize = 16;
/// Max instances of one member slot.
pub const MAX_MEMBER_COUNT: u32 = 8;
/// Max length of a team name.
pub const MAX_TEAM_NAME_LEN: usize = 80;
/// Max length of a team description.
pub const MAX_TEAM_DESCRIPTION_LEN: usize = 2_000;
/// Hard ceiling of a lead's fan-out (executor sub-tasks per delegated task).
pub const MAX_FAN_OUT_LIMIT: u32 = 12;
/// Hierarchy depth below the Manager: Lead (1) → Executors / Reviewers (2).
pub const MAX_HIERARCHY_DEPTH: u32 = 2;

/// What a team's agents may do inside one assigned project. Agents of a
/// cross-project team never see another project's secrets unless that
/// project's assignment explicitly allows it (§67, §68.27).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct TeamProjectPermissions {
    /// The team's agents may read this project's secrets (audited; still
    /// subject to the permission engine and the vault lock).
    pub secrets: bool,
    /// Capabilities denied to the team's agents in this project (installed
    /// as agent-scope `Deny` rules on spawn — they only ever narrow).
    pub denied: Vec<Capability>,
}

/// A team assigned to a project (and optionally pinned to one Manager plan).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamAssignment {
    /// Team.
    pub team_id: AgentTeamId,
    /// Project.
    pub project_id: ProjectId,
    /// The Manager plan this team executes (Manager delegation target).
    #[serde(default)]
    pub plan_id: Option<ManagerPlanId>,
    /// Per-project permission scope.
    #[serde(default)]
    pub permissions: TeamProjectPermissions,
    /// When.
    pub assigned_at: Timestamp,
}

/// Status of a team lead's delegated task.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum TeamLeadStatus {
    /// The lead is decomposing the task into executor sub-tasks.
    Planning,
    /// Executors are working.
    Executing,
    /// A reviewer checks the combined result.
    Reviewing,
    /// Done; the parent task completed.
    Completed,
    /// Stopped by a bound or a failure; the parent task failed.
    Failed,
    /// Cancelled (run cancel, emergency stop).
    Cancelled,
}

impl TeamLeadStatus {
    /// Final statuses.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

/// One sub-task of a lead (executor work or the final review).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamLeadSubtask {
    /// Task-engine task (its `parentId` is the delegated task).
    pub task_id: TaskId,
    /// Key within the lead's decomposition.
    pub key: String,
    /// Title.
    pub title: String,
    /// Role.
    pub role: AgentRole,
    /// Team member template that executes it.
    pub template_id: AgentTemplateId,
    /// Status (mirrors the task).
    pub status: TaskStatus,
    /// Current / last agent.
    #[serde(default)]
    pub agent_id: Option<AgentId>,
    /// Agents started for it.
    pub attempts: u32,
    /// The reviewer's sub-task.
    #[serde(default)]
    pub is_review: bool,
    /// Last error / note.
    #[serde(default)]
    pub note: Option<String>,
}

/// A delegated task handled by a team lead (hierarchy level 1).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamLeadRun {
    /// Id (UUID v7).
    pub id: String,
    /// Team.
    pub team_id: AgentTeamId,
    /// Project.
    pub project_id: ProjectId,
    /// Manager run that delegated it.
    #[serde(default)]
    pub run_id: Option<ManagerRunId>,
    /// The delegated (parent) task.
    pub parent_task_id: TaskId,
    /// The lead's agent record (shown in the Agents panel and the hierarchy tree).
    #[serde(default)]
    pub lead_agent_id: Option<AgentId>,
    /// Title of the delegated task.
    pub title: String,
    /// Status.
    pub status: TeamLeadStatus,
    /// Why (bounds, failures).
    #[serde(default)]
    pub status_reason: Option<String>,
    /// Depth below the Manager (always 1 for a lead).
    pub depth: u32,
    /// Sub-tasks (executors, then the review).
    pub subtasks: Vec<TeamLeadSubtask>,
    /// Budget inherited from the run (USD), split between the executors.
    #[serde(default)]
    pub budget_usd: Option<f64>,
    /// Spent so far (lead decomposition + sub-tasks).
    pub cost_usd: f64,
    /// Lead's summary of the decomposition / result (visible text only).
    #[serde(default)]
    pub summary: Option<String>,
    /// Normalisation notes.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Model of the lead.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
    /// Finished.
    #[serde(default)]
    pub finished_at: Option<Timestamp>,
}

/// A team with its assignments and recent lead runs.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamView {
    /// The team.
    pub team: AgentTeam,
    /// Projects (and plans) it is assigned to.
    pub assignments: Vec<TeamAssignment>,
    /// Recent delegated tasks (newest first, ≤ 10).
    pub lead_runs: Vec<TeamLeadRun>,
}

/// Params of `teams.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTeamsParams {
    /// Only global teams, teams of this project and teams assigned to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `teams.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTeamsResult {
    /// Teams (built-ins first, then by name).
    pub teams: Vec<TeamView>,
}

/// Params of `teams.get` / `teams.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TeamIdParams {
    /// Team.
    pub team_id: AgentTeamId,
}

/// Params of `teams.create`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateTeamParams {
    /// Name (1..=80).
    pub name: String,
    /// Kind (default `Custom(name)`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<TeamKind>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Members (at most one lead).
    pub members: Vec<TeamMember>,
    /// Policies (defaults otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policies: Option<TeamPolicies>,
    /// Project-specific team (global when absent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Params of `teams.update` (absent fields unchanged).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateTeamParams {
    /// Team.
    pub team_id: AgentTeamId,
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<TeamKind>,
    /// Description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Members.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub members: Option<Vec<TeamMember>>,
    /// Policies.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub policies: Option<TeamPolicies>,
}

/// Result of `teams.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteTeamResult {
    /// Deleted team.
    pub team_id: AgentTeamId,
}

/// Params of `teams.assign`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssignTeamParams {
    /// Team.
    pub team_id: AgentTeamId,
    /// Project.
    pub project_id: ProjectId,
    /// Also make the team execute this (proposed or approved) Manager plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plan_id: Option<ManagerPlanId>,
    /// Per-project permissions (default: no secrets, nothing extra denied).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub permissions: Option<TeamProjectPermissions>,
    /// Remove the assignment instead.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unassign: Option<bool>,
}

method!(
    /// `teams.list` — teams with assignments and recent lead runs.
    List, "teams.list", ListTeamsParams, ListTeamsResult
);
method!(
    /// `teams.get` — one team.
    Get, "teams.get", TeamIdParams, TeamView
);
method!(
    /// `teams.create` — create a custom team.
    Create, "teams.create", CreateTeamParams, TeamView
);
method!(
    /// `teams.update` — edit a team (members, lead, policies).
    Update, "teams.update", UpdateTeamParams, TeamView
);
method!(
    /// `teams.delete` — delete a team (refused while one of its leads works).
    Delete, "teams.delete", TeamIdParams, DeleteTeamResult
);
method!(
    /// `teams.assign` — assign a team to a project / plan (or unassign).
    Assign, "teams.assign", AssignTeamParams, TeamView
);
