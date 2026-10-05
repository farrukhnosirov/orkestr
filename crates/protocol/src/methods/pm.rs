//! `pm.*` — the AI Product Manager (§9, §68.47): idea → Epic → Features →
//! Stories → Acceptance Criteria → Tasks + Dependencies, as a structured,
//! validated, editable draft. Approval turns it into task-engine tasks
//! (parent hierarchy epic → feature → story → task, dependencies between
//! tasks) and can hand the tasks to a Manager run.
//!
//! Event: `PmDraftUpdated` (topic `project:<id>`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AgentRole, AgentTeamId, ManagerPlanId, ManagerRunId, ModelRef, ProjectId, TaskId, TaskPriority, Timestamp,
};

use super::manager::{ManagerPlan, ManagerRequestOptions, ManagerRun};
use super::method;

/// Max features of an epic.
pub const MAX_PM_FEATURES: usize = 12;
/// Max stories of a feature.
pub const MAX_PM_STORIES: usize = 12;
/// Max executable tasks of a draft (same bound as a Manager plan).
pub const MAX_PM_TASKS: usize = 40;
/// Max acceptance criteria of a story.
pub const MAX_PM_CRITERIA: usize = 20;
/// Max characters of an idea.
pub const MAX_PM_IDEA: usize = 20_000;

/// Draft status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PmDraftStatus {
    /// Editable.
    Draft,
    /// Converted into tasks.
    Approved,
    /// Thrown away.
    Discarded,
}

/// An executable task of a story.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmTask {
    /// Key unique within the draft (dependencies refer to it).
    pub key: String,
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Agent role.
    pub role: AgentRole,
    /// Priority.
    #[serde(default)]
    pub priority: TaskPriority,
    /// Keys of prerequisite tasks (any story of the draft).
    #[serde(default)]
    pub depends_on: Vec<String>,
    /// Estimate in minutes.
    #[serde(default)]
    pub estimate_minutes: Option<u32>,
    /// Task created on approval.
    #[serde(default)]
    pub task_id: Option<TaskId>,
}

/// A user story with acceptance criteria.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmStory {
    /// Key unique within the draft.
    pub key: String,
    /// Title ("As a … I want … so that …" or a short name).
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Acceptance criteria (testable).
    pub acceptance_criteria: Vec<String>,
    /// Tasks implementing the story.
    pub tasks: Vec<PmTask>,
    /// Task created on approval.
    #[serde(default)]
    pub task_id: Option<TaskId>,
}

/// A feature of the epic.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmFeature {
    /// Key unique within the draft.
    pub key: String,
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Stories.
    pub stories: Vec<PmStory>,
    /// Task created on approval.
    #[serde(default)]
    pub task_id: Option<TaskId>,
}

/// The epic (root of the draft).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmEpic {
    /// Title.
    pub title: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Features.
    pub features: Vec<PmFeature>,
    /// Task created on approval.
    #[serde(default)]
    pub task_id: Option<TaskId>,
}

/// Progress of an approved draft (from the task engine).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmProgress {
    /// Executable tasks.
    pub total: u32,
    /// Completed ones.
    pub completed: u32,
    /// Failed / cancelled ones.
    pub failed: u32,
}

/// An AI Product Manager draft.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmDraft {
    /// Id (UUID v7).
    pub id: String,
    /// Project.
    pub project_id: ProjectId,
    /// The idea as the user wrote it.
    pub idea: String,
    /// Status.
    pub status: PmDraftStatus,
    /// The structured breakdown.
    pub epic: PmEpic,
    /// Normalisation / validation notes.
    #[serde(default)]
    pub warnings: Vec<String>,
    /// Model that drafted it.
    #[serde(default)]
    pub model: Option<ModelRef>,
    /// Manager plan created on approval (hand-off to a run).
    #[serde(default)]
    pub plan_id: Option<ManagerPlanId>,
    /// Run started on approval.
    #[serde(default)]
    pub run_id: Option<ManagerRunId>,
    /// Team the run was delegated to.
    #[serde(default)]
    pub team_id: Option<AgentTeamId>,
    /// Progress (approved drafts).
    #[serde(default)]
    pub progress: Option<PmProgress>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

/// Params of `pm.draft`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmDraftParams {
    /// Project.
    pub project_id: ProjectId,
    /// The idea (1..=20 000 chars).
    pub idea: String,
    /// Model / language overrides.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub options: Option<ManagerRequestOptions>,
}

/// Params of `pm.get` / `pm.discard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmDraftIdParams {
    /// Draft.
    pub draft_id: String,
}

/// Params of `pm.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmListParams {
    /// Project.
    pub project_id: ProjectId,
    /// Include discarded drafts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_discarded: Option<bool>,
}

/// Result of `pm.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmListResult {
    /// Drafts, newest first.
    pub drafts: Vec<PmDraft>,
}

/// Params of `pm.update` — replaces the breakdown of a `Draft` (validated).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmUpdateParams {
    /// Draft.
    pub draft_id: String,
    /// The edited breakdown.
    pub epic: PmEpic,
}

/// Params of `pm.approve`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmApproveParams {
    /// Draft.
    pub draft_id: String,
    /// Hand the tasks to a Manager run right away (default false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub start_run: Option<bool>,
    /// Delegate the run to this team.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub team_id: Option<AgentTeamId>,
}

/// Result of `pm.approve`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PmApproveResult {
    /// The approved draft (task ids filled in).
    pub draft: PmDraft,
    /// The Manager plan wrapping the executable tasks.
    pub plan: ManagerPlan,
    /// The run, when one was started.
    #[serde(default)]
    pub run: Option<ManagerRun>,
}

method!(
    /// `pm.draft` — turn an idea into a structured draft (model call).
    Draft, "pm.draft", PmDraftParams, PmDraft
);
method!(
    /// `pm.get` — one draft (with progress once approved).
    Get, "pm.get", PmDraftIdParams, PmDraft
);
method!(
    /// `pm.list` — drafts of a project.
    List, "pm.list", PmListParams, PmListResult
);
method!(
    /// `pm.update` — save an edited breakdown.
    Update, "pm.update", PmUpdateParams, PmDraft
);
method!(
    /// `pm.approve` — create the task hierarchy (and optionally start a run).
    Approve, "pm.approve", PmApproveParams, PmApproveResult
);
method!(
    /// `pm.discard` — throw a draft away.
    Discard, "pm.discard", PmDraftIdParams, PmDraft
);
