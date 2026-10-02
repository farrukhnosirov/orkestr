//! `tasks.*` — Task Engine (§10–11). The state machine lives in
//! `orkestr-task-engine` (contract §5); these are the wire types.
//!
//! Events: `TaskCreated`, `TaskUpdated` (full task after every change,
//! including status changes and dependency edits), `TaskStatusChanged`
//! (one per status change, also for cascaded changes) and `TaskDeleted`.
//! All carry `projectId` + `taskId` scopes (topics `project:<id>`, `task:<id>`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    AcceptanceCriterion, AgentId, DependencyKind, ProjectId, Task, TaskAttempt, TaskHistoryEntry, TaskId, TaskPriority,
    TaskStatus,
};

use super::method;

/// Max title length (characters).
pub const MAX_TITLE_LEN: usize = 200;
/// Max description length (characters).
pub const MAX_DESCRIPTION_LEN: usize = 20_000;
/// Max number of acceptance criteria.
pub const MAX_CRITERIA: usize = 100;
/// Max length of one acceptance criterion.
pub const MAX_CRITERION_LEN: usize = 500;
/// Max length of a transition reason.
pub const MAX_REASON_LEN: usize = 2_000;
/// Max number of files in `filesTouched`.
pub const MAX_FILES: usize = 1_000;

/// Params of `tasks.list`. All filters are optional and combined with AND.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTasksParams {
    /// Only tasks of this project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Only tasks in one of these statuses.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status: Option<Vec<TaskStatus>>,
    /// Only direct children of this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_id: Option<TaskId>,
    /// Only tasks assigned to this agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
}

/// Result of `tasks.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListTasksResult {
    /// Tasks ordered by creation time (oldest first).
    pub tasks: Vec<Task>,
}

/// Params of methods addressing one task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskIdParams {
    /// Target task.
    pub task_id: TaskId,
}

/// Result of methods returning one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SingleTaskResult {
    /// The task.
    pub task: Task,
}

/// Params of `tasks.create`. The task starts `Planned`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateTaskParams {
    /// Owning project.
    pub project_id: ProjectId,
    /// Title (1..=200 chars, trimmed).
    pub title: String,
    /// Markdown description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Acceptance criteria (texts; all start unmet).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub acceptance_criteria: Option<Vec<String>>,
    /// Priority (default Normal).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<TaskPriority>,
    /// Parent task (epic/story decomposition, same project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_id: Option<TaskId>,
    /// Prerequisites (finish-to-start, same project).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub depends_on: Option<Vec<TaskId>>,
    /// Tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
    /// Estimate in minutes (critical-path weight).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub estimate_minutes: Option<u32>,
    /// Files the task is expected to touch (scheduler conflict avoidance).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub files: Option<Vec<String>>,
}

/// Partial update of a task's fields (never its status — use
/// `tasks.transition`). Absent fields are unchanged.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskPatch {
    /// New title.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// New description.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub description: Option<String>,
    /// Replaces the acceptance criteria (text + met flag).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub acceptance_criteria: Option<Vec<AcceptanceCriterion>>,
    /// New priority.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub priority: Option<TaskPriority>,
    /// New parent (same project, no parent cycles, depth ≤ 6).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub parent_id: Option<TaskId>,
    /// `true` removes the parent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub clear_parent: Option<bool>,
    /// Assign an agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// `true` unassigns the agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub clear_agent: Option<bool>,
    /// New tags.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tags: Option<Vec<String>>,
    /// New file list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub files_touched: Option<Vec<String>>,
    /// New estimate in minutes (`0` clears).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub estimate_minutes: Option<u32>,
}

/// Params of `tasks.update`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateTaskParams {
    /// Target task.
    pub task_id: TaskId,
    /// Optimistic concurrency: fail with `conflict` if the task's version differs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_version: Option<u32>,
    /// Changes.
    pub patch: TaskPatch,
}

/// Result of `tasks.delete`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteTaskResult {
    /// Deleted task.
    pub task_id: TaskId,
}

/// Params of `tasks.transition`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TransitionTaskParams {
    /// Target task.
    pub task_id: TaskId,
    /// Requested status.
    pub to: TaskStatus,
    /// Reason / note (≤ 2000 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reason: Option<String>,
    /// Optimistic concurrency guard.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub expected_version: Option<u32>,
}

/// One status change (requested or cascaded).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskStatusChange {
    /// Task.
    pub task_id: TaskId,
    /// Previous status.
    pub from: TaskStatus,
    /// New status.
    pub to: TaskStatus,
    /// Reason code.
    pub reason: Option<String>,
}

/// Result of `tasks.transition`, `tasks.retry`, `tasks.addDependency`,
/// `tasks.removeDependency`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskChangeResult {
    /// The addressed task after the change.
    pub task: Task,
    /// Every status change performed, in order (the requested one first,
    /// then dependency cascades).
    pub changes: Vec<TaskStatusChange>,
}

/// Params of `tasks.addDependency`: `taskId` will wait for `dependsOn`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddDependencyParams {
    /// Dependent task.
    pub task_id: TaskId,
    /// Prerequisite.
    pub depends_on: TaskId,
    /// Kind (default FinishToStart).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<DependencyKind>,
}

/// Params of `tasks.removeDependency`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveDependencyParams {
    /// Dependent task.
    pub task_id: TaskId,
    /// Prerequisite.
    pub depends_on: TaskId,
}

/// Params of `tasks.graph`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskGraphParams {
    /// Project.
    pub project_id: ProjectId,
    /// Restrict to the subtree (parent/child) of this task (epic view).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub root_id: Option<TaskId>,
}

/// A node of the task graph.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskGraphNode {
    /// Task id.
    pub id: TaskId,
    /// Title.
    pub title: String,
    /// Status.
    pub status: TaskStatus,
    /// Priority.
    pub priority: TaskPriority,
    /// Assigned agent.
    pub agent_id: Option<AgentId>,
    /// Parent.
    pub parent_id: Option<TaskId>,
    /// Estimate in minutes.
    pub estimate_minutes: Option<u32>,
    /// Longest remaining weighted chain starting at this node (inclusive).
    pub downstream_length: u32,
}

/// A dependency edge `dependsOn → taskId` (prerequisite first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskGraphEdge {
    /// Prerequisite.
    pub depends_on: TaskId,
    /// Dependent.
    pub task_id: TaskId,
    /// Kind.
    pub kind: DependencyKind,
}

/// Result of `tasks.graph`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskGraph {
    /// Nodes.
    pub nodes: Vec<TaskGraphNode>,
    /// Dependency edges among the returned nodes.
    pub edges: Vec<TaskGraphEdge>,
    /// Tasks whose hard dependencies are all `Completed` and that are
    /// `Planned`, `Ready` or `Blocked` (i.e. runnable once approved).
    pub ready: Vec<TaskId>,
    /// Topological order (prerequisites first).
    pub topo_order: Vec<TaskId>,
    /// Remaining critical path (longest weighted chain of unfinished work).
    pub critical_path: Vec<TaskId>,
    /// Its length (sum of estimates in minutes; unknown estimate = 1).
    pub critical_path_length: u32,
}

/// Result of `tasks.history`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskHistoryResult {
    /// Status changes, oldest first.
    pub entries: Vec<TaskHistoryEntry>,
    /// Execution attempts, oldest first.
    pub executions: Vec<TaskAttempt>,
}

/// Validation error for task input.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TaskValidationError {
    /// Empty title.
    #[error("task title must not be empty")]
    EmptyTitle,
    /// Title too long.
    #[error("task title must be at most {MAX_TITLE_LEN} characters")]
    TitleTooLong,
    /// Description too long.
    #[error("task description must be at most {MAX_DESCRIPTION_LEN} characters")]
    DescriptionTooLong,
    /// Too many / too long acceptance criteria.
    #[error("at most {MAX_CRITERIA} acceptance criteria of 1..={MAX_CRITERION_LEN} characters")]
    BadCriteria,
    /// Reason too long.
    #[error("reason must be at most {MAX_REASON_LEN} characters")]
    ReasonTooLong,
    /// Bad tags.
    #[error("at most 32 tags of 1..=40 characters")]
    BadTags,
    /// Bad file list.
    #[error("at most {MAX_FILES} relative file paths of 1..=500 characters")]
    BadFiles,
}

/// Validates and normalizes a title.
pub fn validate_title(title: &str) -> Result<String, TaskValidationError> {
    let t = title.trim();
    if t.is_empty() {
        return Err(TaskValidationError::EmptyTitle);
    }
    if t.chars().count() > MAX_TITLE_LEN {
        return Err(TaskValidationError::TitleTooLong);
    }
    Ok(t.to_owned())
}

/// Validates a description.
pub fn validate_description(d: &str) -> Result<(), TaskValidationError> {
    if d.chars().count() > MAX_DESCRIPTION_LEN {
        return Err(TaskValidationError::DescriptionTooLong);
    }
    Ok(())
}

/// Validates and trims acceptance criteria texts (empty entries dropped).
pub fn validate_criteria(items: &[AcceptanceCriterion]) -> Result<Vec<AcceptanceCriterion>, TaskValidationError> {
    if items.len() > MAX_CRITERIA {
        return Err(TaskValidationError::BadCriteria);
    }
    let mut out = Vec::with_capacity(items.len());
    for c in items {
        let text = c.text.trim();
        if text.is_empty() {
            continue;
        }
        if text.chars().count() > MAX_CRITERION_LEN {
            return Err(TaskValidationError::BadCriteria);
        }
        out.push(AcceptanceCriterion { text: text.to_owned(), met: c.met });
    }
    Ok(out)
}

/// Validates tags (dedup case-insensitively).
pub fn validate_tags(tags: &[String]) -> Result<Vec<String>, TaskValidationError> {
    if tags.len() > 32 {
        return Err(TaskValidationError::BadTags);
    }
    let mut out: Vec<String> = Vec::new();
    for t in tags {
        let t = t.trim();
        if t.is_empty() || t.chars().count() > 40 {
            return Err(TaskValidationError::BadTags);
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(t)) {
            out.push(t.to_owned());
        }
    }
    Ok(out)
}

/// Validates a list of relative file paths (dedup, `\` normalized to `/`).
pub fn validate_files(files: &[String]) -> Result<Vec<String>, TaskValidationError> {
    if files.len() > MAX_FILES {
        return Err(TaskValidationError::BadFiles);
    }
    let mut out: Vec<String> = Vec::new();
    for f in files {
        let f = f.trim().replace('\\', "/");
        if f.is_empty() || f.chars().count() > 500 {
            return Err(TaskValidationError::BadFiles);
        }
        if !out.contains(&f) {
            out.push(f);
        }
    }
    Ok(out)
}

/// Validates a reason.
pub fn validate_reason(reason: &str) -> Result<(), TaskValidationError> {
    if reason.chars().count() > MAX_REASON_LEN {
        return Err(TaskValidationError::ReasonTooLong);
    }
    Ok(())
}

method!(
    /// `tasks.list`.
    List, "tasks.list", ListTasksParams, ListTasksResult
);
method!(
    /// `tasks.get`.
    Get, "tasks.get", TaskIdParams, SingleTaskResult
);
method!(
    /// `tasks.create` — emits `TaskCreated`.
    Create, "tasks.create", CreateTaskParams, SingleTaskResult
);
method!(
    /// `tasks.update` — emits `TaskUpdated`.
    Update, "tasks.update", UpdateTaskParams, SingleTaskResult
);
method!(
    /// `tasks.delete` — only `Planned`/`Cancelled` tasks; emits `TaskDeleted`.
    Delete, "tasks.delete", TaskIdParams, DeleteTaskResult
);
method!(
    /// `tasks.transition` — emits `TaskStatusChanged` + `TaskUpdated` per change.
    Transition, "tasks.transition", TransitionTaskParams, TaskChangeResult
);
method!(
    /// `tasks.addDependency` — rejects cycles (`conflict`, `data.cycle`).
    AddDependency, "tasks.addDependency", AddDependencyParams, TaskChangeResult
);
method!(
    /// `tasks.removeDependency`.
    RemoveDependency, "tasks.removeDependency", RemoveDependencyParams, TaskChangeResult
);
method!(
    /// `tasks.graph` — nodes, edges, ready set, topological order, critical path.
    Graph, "tasks.graph", TaskGraphParams, TaskGraph
);
method!(
    /// `tasks.history` — status history + execution attempts.
    History, "tasks.history", TaskIdParams, TaskHistoryResult
);
method!(
    /// `tasks.retry` — `Failed → Ready`, or `Interrupted → Recovering → Ready`.
    Retry, "tasks.retry", TaskIdParams, TaskChangeResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn title_rules() {
        assert_eq!(validate_title("  Build API ").unwrap(), "Build API");
        assert_eq!(validate_title(" "), Err(TaskValidationError::EmptyTitle));
        assert_eq!(validate_title(&"x".repeat(201)), Err(TaskValidationError::TitleTooLong));
    }

    #[test]
    fn criteria_and_files() {
        let c = validate_criteria(&[
            AcceptanceCriterion { text: " a ".into(), met: true },
            AcceptanceCriterion { text: "  ".into(), met: false },
        ])
        .unwrap();
        assert_eq!(c, vec![AcceptanceCriterion { text: "a".into(), met: true }]);
        assert_eq!(validate_files(&["src\\a.rs".into(), "src/a.rs".into()]).unwrap(), vec!["src/a.rs".to_owned()]);
        assert!(validate_files(&[String::new()]).is_err());
    }

    #[test]
    fn create_params_minimal_json() {
        let p: CreateTaskParams =
            serde_json::from_str(r#"{"projectId":"0190a000-0000-7000-8000-000000000000","title":"X"}"#).unwrap();
        assert_eq!(p.title, "X");
        assert!(p.depends_on.is_none());
    }
}
