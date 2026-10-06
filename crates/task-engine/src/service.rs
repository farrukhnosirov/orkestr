//! [`TaskService`]: the task engine's transactional API.
//!
//! Every mutating operation runs in **one SQLite transaction** that performs
//! the requested change, writes `task_history` rows, opens/closes execution
//! attempts and applies dependency cascades (auto-promote / auto-block). Only
//! after commit are the resulting events published:
//! `TaskCreated`, `TaskUpdated` (full task, once per touched task),
//! `TaskStatusChanged` (one per status change) and `TaskDeleted` — all scoped
//! to `projectId` + `taskId`.

use std::collections::{BTreeMap, BTreeSet};

use orkestr_domain::{
    now, AcceptanceCriterion, DependencyKind, ExecutionStatus, ProjectId, Task, TaskActor, TaskAttempt, TaskDependency,
    TaskHistoryEntry, TaskId, TaskStatus, Timestamp,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, TaskDeletedPayload, TaskPayload, TaskStatusChangedPayload};
use orkestr_protocol::methods::tasks::{
    validate_criteria, validate_description, validate_files, validate_reason, validate_tags, validate_title,
    CreateTaskParams, ListTasksParams, TaskGraph, TaskGraphEdge, TaskGraphNode, TaskPatch, TaskStatusChange,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::rusqlite::Connection;
use orkestr_storage::Db;

use crate::dag::{dependency_reaction, Dag, Trigger};
use crate::scheduler::{ResourceSnapshot, SchedTask, SchedulePlan, Scheduler};
use crate::state_machine::transition_as;
use crate::store::{self, HistoryRecord, TaskFilter};
use crate::{TaskError, MAX_DEPTH};

/// Result of a mutating operation on one task.
#[derive(Debug, Clone, PartialEq)]
pub struct TaskChange {
    /// The addressed task after the operation.
    pub task: Task,
    /// Status changes performed (requested first, then cascades).
    pub changes: Vec<TaskStatusChange>,
}

/// Bookkeeping written by the agent runtime on a task (never its status —
/// status changes go through [`TaskService::transition`]).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AgentRunUpdate {
    /// Assign this agent.
    pub agent_id: Option<orkestr_domain::AgentId>,
    /// Model in use.
    pub model: Option<orkestr_domain::ModelRef>,
    /// Worktree in use.
    pub worktree: Option<orkestr_domain::WorktreeRef>,
    /// Usage to add to the task's total.
    pub usage_delta: Option<orkestr_domain::UsageSummary>,
    /// Files to merge into `filesTouched` (deduplicated, capped).
    pub files_touched: Vec<String>,
    /// Final result.
    pub result: Option<orkestr_domain::TaskResult>,
}

/// Outcome of crash/shutdown recovery.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RecoveryReport {
    /// Tasks moved to `Interrupted`.
    pub interrupted: Vec<TaskId>,
}

#[derive(Debug, Default)]
struct Effects {
    created: Vec<Task>,
    changes: Vec<(TaskStatusChange, ProjectId)>,
    updated: BTreeMap<TaskId, Task>,
    deleted: Vec<(TaskId, ProjectId)>,
}

/// Transaction context: connection + accumulated effects.
struct Tx<'a> {
    c: &'a Connection,
    now: Timestamp,
    fx: Effects,
}

fn status_name(s: TaskStatus) -> String {
    format!("{s:?}")
}

impl<'a> Tx<'a> {
    fn new(c: &'a Connection, now: Timestamp) -> Self {
        Self { c, now, fx: Effects::default() }
    }

    fn hard_dep_statuses(&self, id: TaskId) -> Result<Vec<TaskStatus>, TaskError> {
        let deps: Vec<TaskId> = store::dependencies_of(self.c, id)?
            .into_iter()
            .filter(|d| d.kind == DependencyKind::FinishToStart)
            .map(|d| d.depends_on)
            .collect();
        Ok(store::statuses(self.c, &deps)?.into_values().collect())
    }

    /// Guards that hold for every actor (task-state-machine.md §4).
    fn check_guards(&self, task: &Task, to: TaskStatus) -> Result<(), TaskError> {
        let entering_ready = to == TaskStatus::Ready;
        let starting = to == TaskStatus::Running && matches!(task.status, TaskStatus::Ready | TaskStatus::Recovering);
        if entering_ready || starting {
            let pending: Vec<TaskStatus> =
                self.hard_dep_statuses(task.id)?.into_iter().filter(|s| *s != TaskStatus::Completed).collect();
            if !pending.is_empty() {
                return Err(TaskError::GuardFailed {
                    guard: "dependencies_completed",
                    detail: format!(
                        "{} dependenc{} not completed yet",
                        pending.len(),
                        if pending.len() == 1 { "y is" } else { "ies are" }
                    ),
                });
            }
        }
        if to == TaskStatus::Completed {
            let mut open = 0usize;
            for child in store::children_of(self.c, task.id)? {
                let st = store::statuses(self.c, &[child])?;
                if st.get(&child).is_some_and(|s| !matches!(s, TaskStatus::Completed | TaskStatus::Cancelled)) {
                    open += 1;
                }
            }
            if open > 0 {
                return Err(TaskError::GuardFailed {
                    guard: "children_completed",
                    detail: format!("{open} subtask(s) are not completed"),
                });
            }
        }
        Ok(())
    }

    /// Applies one transition with history, attempts and effects (no cascade).
    fn apply(
        &mut self,
        task: &mut Task,
        to: TaskStatus,
        actor: TaskActor,
        reason: Option<&str>,
    ) -> Result<(), TaskError> {
        let applied = transition_as(task, to, actor, self.now)?;
        if applied.new_attempt {
            let attempt = store::open_attempt(self.c, task, self.now)?;
            task.executions.push(attempt.id);
        } else {
            let closing = match to {
                TaskStatus::Completed => Some(ExecutionStatus::Succeeded),
                TaskStatus::Failed => Some(ExecutionStatus::Failed),
                TaskStatus::Cancelled => Some(ExecutionStatus::Cancelled),
                TaskStatus::Interrupted => Some(ExecutionStatus::Interrupted),
                _ => None,
            };
            if let Some(st) = closing {
                store::close_open_attempt(self.c, task.id, st, reason, self.now)?;
            }
        }
        store::save_task(self.c, task)?;
        store::insert_history(
            self.c,
            &HistoryRecord {
                task: task.id,
                from: Some(applied.from),
                to,
                actor,
                reason,
                forced: applied.forced,
                at: self.now,
            },
        )?;
        tracing::info!(task_id = %task.id, from = ?applied.from, to = ?to, ?actor, forced = applied.forced, "task transition");
        self.fx.changes.push((
            TaskStatusChange { task_id: task.id, from: applied.from, to, reason: reason.map(str::to_owned) },
            task.project_id,
        ));
        self.fx.updated.insert(task.id, task.clone());
        Ok(())
    }

    /// Re-evaluates one task against its dependencies.
    fn reconcile(&mut self, id: TaskId, trigger: Trigger) -> Result<(), TaskError> {
        let Some(mut task) = store::load_task(self.c, id)? else { return Ok(()) };
        let deps = self.hard_dep_statuses(id)?;
        if let Some((to, reason)) = dependency_reaction(task.status, &deps, trigger) {
            self.apply(&mut task, to, TaskActor::System, Some(reason))?;
        }
        Ok(())
    }

    /// Re-evaluates the dependents of `id` after it changed status.
    fn cascade_from(&mut self, id: TaskId, status: TaskStatus) -> Result<(), TaskError> {
        let trigger = match status {
            TaskStatus::Completed => Trigger::DependencyCompleted,
            TaskStatus::Failed | TaskStatus::Cancelled => Trigger::Other,
            _ => return Ok(()),
        };
        for edge in store::dependents_of(self.c, id)? {
            if edge.kind == DependencyKind::FinishToStart {
                self.reconcile(edge.task_id, trigger)?;
            }
        }
        Ok(())
    }

    /// Requested transition: guards + apply + cascades.
    fn transition(
        &mut self,
        task: &mut Task,
        to: TaskStatus,
        actor: TaskActor,
        reason: Option<&str>,
    ) -> Result<(), TaskError> {
        if !crate::can_transition_as(task.status, to, actor) {
            return Err(crate::InvalidTransition { from: task.status, to }.into());
        }
        self.check_guards(task, to)?;
        self.apply(task, to, actor, reason)?;
        self.cascade_from(task.id, to)
    }

    fn mark_updated(&mut self, id: TaskId) -> Result<Task, TaskError> {
        let t = store::require_task(self.c, id)?;
        self.fx.updated.insert(id, t.clone());
        Ok(t)
    }

    fn depth_of(&self, id: TaskId) -> Result<usize, TaskError> {
        let mut depth = 0;
        let mut cur = store::parent_of(self.c, id)?;
        while let Some(p) = cur {
            depth += 1;
            if depth > 64 {
                return Err(TaskError::Storage("parent chain too deep or cyclic".into()));
            }
            cur = store::parent_of(self.c, p)?;
        }
        Ok(depth)
    }

    fn height_of(&self, id: TaskId) -> Result<usize, TaskError> {
        let mut level = vec![id];
        let mut height = 0;
        let mut seen = BTreeSet::from([id]);
        loop {
            let mut next = Vec::new();
            for t in &level {
                for c in store::children_of(self.c, *t)? {
                    if seen.insert(c) {
                        next.push(c);
                    }
                }
            }
            if next.is_empty() {
                return Ok(height);
            }
            height += 1;
            level = next;
        }
    }

    /// Validates `parent` as the parent of `child` (`None` child = new task).
    fn check_parent(&self, project: ProjectId, child: Option<TaskId>, parent: TaskId) -> Result<(), TaskError> {
        if Some(parent) == child {
            return Err(TaskError::Cycle(vec![parent, parent]));
        }
        let p = store::load_task(self.c, parent)?
            .ok_or_else(|| TaskError::Invalid(format!("parent task {parent} not found")))?;
        if p.project_id != project {
            return Err(TaskError::Invalid("parent task belongs to another project".into()));
        }
        if let Some(child) = child {
            // The new parent must not be inside the child's subtree.
            let mut path = vec![child, parent];
            let mut cur = parent;
            while let Some(x) = store::parent_of(self.c, cur)? {
                path.push(x);
                if x == child {
                    return Err(TaskError::Cycle(path));
                }
                if path.len() > 64 {
                    return Err(TaskError::Storage("parent chain too deep or cyclic".into()));
                }
                cur = x;
            }
        }
        let height = child.map(|c| self.height_of(c)).transpose()?.unwrap_or(0);
        if self.depth_of(parent)? + 1 + height >= MAX_DEPTH {
            return Err(TaskError::Invalid(format!("task hierarchy is limited to {MAX_DEPTH} levels")));
        }
        Ok(())
    }
}

/// The task engine service.
#[derive(Clone)]
pub struct TaskService {
    db: Db,
    events: EventBus,
}

impl std::fmt::Debug for TaskService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TaskService").finish_non_exhaustive()
    }
}

fn check_version(task: &Task, expected: Option<u32>) -> Result<(), TaskError> {
    match expected {
        Some(v) if v != task.version => Err(TaskError::VersionConflict { current: task.version }),
        _ => Ok(()),
    }
}

fn invalid(e: impl std::fmt::Display) -> TaskError {
    TaskError::Invalid(e.to_string())
}

impl TaskService {
    /// Creates the service over a database migrated with
    /// [`crate::task_migrations`] (after the storage migrations).
    #[must_use]
    pub fn new(db: Db, events: EventBus) -> Self {
        Self { db, events }
    }

    async fn read<T, F>(&self, f: F) -> Result<T, TaskError>
    where
        T: Send + 'static,
        F: FnOnce(&Connection) -> Result<T, TaskError> + Send + 'static,
    {
        self.db.call(move |c| Ok(f(c))).await.map_err(TaskError::from)?
    }

    async fn write<T, F>(&self, f: F) -> Result<T, TaskError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Tx<'_>) -> Result<T, TaskError> + Send + 'static,
    {
        let (out, fx) = self
            .db
            .call(move |c| {
                let run = || -> Result<(T, Effects), TaskError> {
                    let tx = c.transaction()?;
                    let mut ctx = Tx::new(&tx, now());
                    let out = f(&mut ctx)?;
                    let fx = ctx.fx;
                    tx.commit()?;
                    Ok((out, fx))
                };
                Ok(run())
            })
            .await
            .map_err(TaskError::from)??;
        self.publish(fx).await;
        Ok(out)
    }

    async fn publish(&self, fx: Effects) {
        let mut events: Vec<NewEvent> = Vec::new();
        for t in fx.created {
            events.push(NewEvent::new(EventKind::TaskCreated(TaskPayload { task: t })));
        }
        let mut changed: BTreeSet<TaskId> = BTreeSet::new();
        for (c, project) in fx.changes {
            changed.insert(c.task_id);
            events.push(
                NewEvent::new(EventKind::TaskStatusChanged(TaskStatusChangedPayload {
                    task_id: c.task_id,
                    from: c.from,
                    to: c.to,
                    reason: c.reason,
                }))
                .project(project),
            );
        }
        for (_, t) in fx.updated {
            events.push(NewEvent::new(EventKind::TaskUpdated(TaskPayload { task: t })));
        }
        for (task_id, project_id) in fx.deleted {
            events.push(NewEvent::new(EventKind::TaskDeleted(TaskDeletedPayload { task_id, project_id })));
        }
        for ev in events {
            // State is committed; a failed publish must not turn into an RPC
            // error (the client would retry a change that already happened).
            if let Err(e) = self.events.publish(ev).await {
                tracing::error!(error = %e, "failed to publish task event");
            }
        }
    }

    /// Lists tasks (filters combined with AND), oldest first.
    pub async fn list(&self, p: ListTasksParams) -> Result<Vec<Task>, TaskError> {
        let filter = TaskFilter {
            project_id: p.project_id,
            status: p.status,
            parent_id: p.parent_id,
            agent_id: p.agent_id.map(|a| a.to_string()),
        };
        self.read(move |c| store::list_tasks(c, &filter)).await
    }

    /// Loads a task.
    pub async fn get(&self, id: TaskId) -> Result<Task, TaskError> {
        self.read(move |c| store::require_task(c, id)).await
    }

    /// Creates a `Planned` task (with optional parent and dependencies).
    pub async fn create(&self, p: CreateTaskParams, actor: TaskActor) -> Result<Task, TaskError> {
        let title = validate_title(&p.title).map_err(invalid)?;
        let mut task = Task::new(p.project_id, title, now());
        if let Some(d) = p.description {
            validate_description(&d).map_err(invalid)?;
            task.description = d;
        }
        if let Some(c) = p.acceptance_criteria {
            let items: Vec<AcceptanceCriterion> =
                c.into_iter().map(|text| AcceptanceCriterion { text, met: false }).collect();
            task.acceptance_criteria = validate_criteria(&items).map_err(invalid)?;
        }
        if let Some(pr) = p.priority {
            task.priority = pr;
        }
        if let Some(tags) = p.tags {
            task.tags = validate_tags(&tags).map_err(invalid)?;
        }
        if let Some(files) = p.files {
            task.files_touched = validate_files(&files).map_err(invalid)?;
        }
        task.estimate_minutes = p.estimate_minutes.filter(|m| *m > 0);
        task.parent_id = p.parent_id;
        let mut deps: Vec<TaskId> = p.depends_on.unwrap_or_default();
        deps.sort();
        deps.dedup();
        self.write(move |tx| {
            if !store::project_exists(tx.c, task.project_id)? {
                return Err(TaskError::ProjectNotFound(task.project_id));
            }
            if let Some(parent) = task.parent_id {
                tx.check_parent(task.project_id, None, parent)?;
            }
            for d in &deps {
                let dep = store::load_task(tx.c, *d)?
                    .ok_or_else(|| TaskError::Invalid(format!("dependency {d} not found")))?;
                if dep.project_id != task.project_id {
                    return Err(TaskError::Invalid("dependencies must belong to the same project".into()));
                }
            }
            store::insert_task(tx.c, &task)?;
            store::insert_history(
                tx.c,
                &HistoryRecord {
                    task: task.id,
                    from: None,
                    to: TaskStatus::Planned,
                    actor,
                    reason: Some("created"),
                    forced: false,
                    at: tx.now,
                },
            )?;
            for d in &deps {
                store::insert_dependency(tx.c, store::hard(task.id, *d), tx.now)?;
            }
            tx.reconcile(task.id, Trigger::Other)?;
            let created = store::require_task(tx.c, task.id)?;
            // A task blocked at creation is reported via TaskCreated (final state)
            // plus its TaskStatusChanged; no separate TaskUpdated.
            tx.fx.updated.remove(&task.id);
            tx.fx.created.push(created.clone());
            tracing::info!(task_id = %created.id, project_id = %created.project_id, "task created");
            Ok(created)
        })
        .await
    }

    /// Applies a field patch (never the status).
    pub async fn update(&self, id: TaskId, expected_version: Option<u32>, patch: TaskPatch) -> Result<Task, TaskError> {
        self.write(move |tx| {
            let mut task = store::require_task(tx.c, id)?;
            check_version(&task, expected_version)?;
            if let Some(t) = patch.title {
                task.title = validate_title(&t).map_err(invalid)?;
            }
            if let Some(d) = patch.description {
                validate_description(&d).map_err(invalid)?;
                task.description = d;
            }
            if let Some(c) = patch.acceptance_criteria {
                task.acceptance_criteria = validate_criteria(&c).map_err(invalid)?;
            }
            if let Some(p) = patch.priority {
                task.priority = p;
            }
            if patch.clear_parent == Some(true) {
                task.parent_id = None;
            } else if let Some(parent) = patch.parent_id {
                if task.parent_id != Some(parent) {
                    tx.check_parent(task.project_id, Some(task.id), parent)?;
                    task.parent_id = Some(parent);
                }
            }
            if patch.clear_agent == Some(true) {
                task.agent_id = None;
            } else if let Some(a) = patch.agent_id {
                task.agent_id = Some(a);
            }
            if let Some(tags) = patch.tags {
                task.tags = validate_tags(&tags).map_err(invalid)?;
            }
            if let Some(files) = patch.files_touched {
                task.files_touched = validate_files(&files).map_err(invalid)?;
            }
            if let Some(m) = patch.estimate_minutes {
                task.estimate_minutes = (m > 0).then_some(m);
            }
            task.updated_at = tx.now;
            store::save_task(tx.c, &mut task)?;
            tx.fx.updated.insert(task.id, task.clone());
            Ok(task)
        })
        .await
    }

    /// Sets the review state shown on the task (Phase 2: written by the
    /// automatic code review; never changes the status).
    pub async fn set_review_state(&self, id: TaskId, state: orkestr_domain::ReviewState) -> Result<Task, TaskError> {
        self.write(move |tx| {
            let mut task = store::require_task(tx.c, id)?;
            if task.review_state == state {
                return Ok(task);
            }
            task.review_state = state;
            task.updated_at = tx.now;
            store::save_task(tx.c, &mut task)?;
            tx.fx.updated.insert(task.id, task.clone());
            Ok(task)
        })
        .await
    }

    /// Deletes a `Planned` or `Cancelled` task. Dependents are re-evaluated
    /// (removing a prerequisite may unblock them); children become roots.
    pub async fn delete(&self, id: TaskId) -> Result<TaskId, TaskError> {
        self.write(move |tx| {
            let task = store::require_task(tx.c, id)?;
            if !matches!(task.status, TaskStatus::Planned | TaskStatus::Cancelled) {
                return Err(TaskError::Conflict(format!(
                    "only Planned or Cancelled tasks can be deleted (task is {})",
                    status_name(task.status)
                )));
            }
            let dependents: Vec<TaskId> = store::dependents_of(tx.c, id)?.into_iter().map(|d| d.task_id).collect();
            let children = store::children_of(tx.c, id)?;
            store::delete_task(tx.c, id)?;
            for d in dependents {
                tx.reconcile(d, Trigger::Other)?;
                tx.mark_updated(d)?;
            }
            for c in children {
                tx.mark_updated(c)?;
            }
            tx.fx.deleted.push((id, task.project_id));
            tracing::info!(task_id = %id, "task deleted");
            Ok(id)
        })
        .await
    }

    /// Requested status change (+ cascades).
    pub async fn transition(
        &self,
        id: TaskId,
        to: TaskStatus,
        actor: TaskActor,
        reason: Option<String>,
        expected_version: Option<u32>,
    ) -> Result<TaskChange, TaskError> {
        if let Some(r) = &reason {
            validate_reason(r).map_err(invalid)?;
        }
        self.write(move |tx| {
            let mut task = store::require_task(tx.c, id)?;
            check_version(&task, expected_version)?;
            tx.transition(&mut task, to, actor, reason.as_deref().filter(|r| !r.trim().is_empty()))?;
            let task = store::require_task(tx.c, id)?;
            let changes = tx.fx.changes.iter().map(|(c, _)| c.clone()).collect();
            Ok(TaskChange { task, changes })
        })
        .await
    }

    /// Retries a task: `Failed → Ready`, or `Interrupted → Recovering → Ready`.
    pub async fn retry(&self, id: TaskId, actor: TaskActor) -> Result<TaskChange, TaskError> {
        self.write(move |tx| {
            let mut task = store::require_task(tx.c, id)?;
            match task.status {
                TaskStatus::Failed => tx.transition(&mut task, TaskStatus::Ready, actor, Some("retry"))?,
                TaskStatus::Interrupted => {
                    tx.transition(&mut task, TaskStatus::Recovering, actor, Some("resume"))?;
                    tx.transition(&mut task, TaskStatus::Ready, actor, Some("recovered_fresh"))?;
                }
                other => {
                    return Err(TaskError::Conflict(format!(
                        "only Failed or Interrupted tasks can be retried (task is {})",
                        status_name(other)
                    )))
                }
            }
            let task = store::require_task(tx.c, id)?;
            let changes = tx.fx.changes.iter().map(|(c, _)| c.clone()).collect();
            Ok(TaskChange { task, changes })
        })
        .await
    }

    /// Adds `task → depends_on` (rejects cycles, cross-project edges and
    /// in-flight tasks); a `Ready` task with a pending new prerequisite
    /// becomes `Blocked`.
    pub async fn add_dependency(
        &self,
        task_id: TaskId,
        depends_on: TaskId,
        kind: DependencyKind,
    ) -> Result<TaskChange, TaskError> {
        self.write(move |tx| {
            if task_id == depends_on {
                return Err(TaskError::Cycle(vec![task_id, task_id]));
            }
            let task = store::require_task(tx.c, task_id)?;
            let dep = store::require_task(tx.c, depends_on)?;
            if task.project_id != dep.project_id {
                return Err(TaskError::Invalid("dependencies must belong to the same project".into()));
            }
            if !matches!(
                task.status,
                TaskStatus::Planned
                    | TaskStatus::Ready
                    | TaskStatus::Blocked
                    | TaskStatus::Failed
                    | TaskStatus::Interrupted
                    | TaskStatus::Recovering
            ) {
                return Err(TaskError::Conflict(format!(
                    "cannot add a dependency to a task in status {}",
                    status_name(task.status)
                )));
            }
            let tasks =
                store::list_tasks(tx.c, &TaskFilter { project_id: Some(task.project_id), ..TaskFilter::default() })?;
            let edges = store::project_edges(tx.c, task.project_id)?;
            if let Some(cycle) = Dag::from_tasks(&tasks, &edges).cycle_if_added(task_id, depends_on) {
                return Err(TaskError::Cycle(cycle));
            }
            store::insert_dependency(tx.c, TaskDependency { task_id, depends_on, kind }, tx.now)?;
            tx.reconcile(task_id, Trigger::Other)?;
            let task = tx.mark_updated(task_id)?;
            let changes = tx.fx.changes.iter().map(|(c, _)| c.clone()).collect();
            Ok(TaskChange { task, changes })
        })
        .await
    }

    /// Removes an edge; the task is re-evaluated (may unblock).
    pub async fn remove_dependency(&self, task_id: TaskId, depends_on: TaskId) -> Result<TaskChange, TaskError> {
        self.write(move |tx| {
            store::require_task(tx.c, task_id)?;
            if !store::delete_dependency(tx.c, task_id, depends_on)? {
                return Err(TaskError::DependencyNotFound { task: task_id, depends_on });
            }
            tx.reconcile(task_id, Trigger::Other)?;
            let task = tx.mark_updated(task_id)?;
            let changes = tx.fx.changes.iter().map(|(c, _)| c.clone()).collect();
            Ok(TaskChange { task, changes })
        })
        .await
    }

    /// Graph of a project (optionally restricted to an epic subtree).
    pub async fn graph(&self, project: ProjectId, root: Option<TaskId>) -> Result<TaskGraph, TaskError> {
        self.read(move |c| {
            if !store::project_exists(c, project)? {
                return Err(TaskError::ProjectNotFound(project));
            }
            let tasks = store::list_tasks(c, &TaskFilter { project_id: Some(project), ..TaskFilter::default() })?;
            let edges = store::project_edges(c, project)?;
            let full = Dag::from_tasks(&tasks, &edges);
            let keep: BTreeSet<TaskId> = match root {
                Some(r) => {
                    let sub = full.subtree(r);
                    if sub.is_empty() {
                        return Err(TaskError::NotFound(r));
                    }
                    sub.into_iter().collect()
                }
                None => tasks.iter().map(|t| t.id).collect(),
            };
            let tasks: Vec<Task> = tasks.into_iter().filter(|t| keep.contains(&t.id)).collect();
            let edges: Vec<TaskDependency> =
                edges.into_iter().filter(|e| keep.contains(&e.task_id) && keep.contains(&e.depends_on)).collect();
            let dag = Dag::from_tasks(&tasks, &edges);
            let down = dag.downstream_lengths();
            let (critical_path, critical_path_length) = dag.critical_path();
            let ready: Vec<TaskId> = full.ready_set().into_iter().filter(|id| keep.contains(id)).collect();
            let topo_order = dag.topological_order().unwrap_or_else(|cyclic| {
                tracing::error!(?cyclic, "task graph contains a cycle (corrupt data)");
                tasks.iter().map(|t| t.id).collect()
            });
            Ok(TaskGraph {
                nodes: tasks
                    .iter()
                    .map(|t| TaskGraphNode {
                        id: t.id,
                        title: t.title.clone(),
                        status: t.status,
                        priority: t.priority,
                        agent_id: t.agent_id,
                        parent_id: t.parent_id,
                        estimate_minutes: t.estimate_minutes,
                        downstream_length: down.get(&t.id).copied().unwrap_or(0),
                    })
                    .collect(),
                edges: edges
                    .iter()
                    .map(|e| TaskGraphEdge { depends_on: e.depends_on, task_id: e.task_id, kind: e.kind })
                    .collect(),
                ready,
                topo_order,
                critical_path,
                critical_path_length,
            })
        })
        .await
    }

    /// Records agent-run data on a task (agent, model, worktree, usage, files,
    /// result) and publishes `TaskUpdated`. Never changes the status.
    pub async fn record_agent_run(&self, id: TaskId, update: AgentRunUpdate) -> Result<Task, TaskError> {
        self.write(move |tx| {
            let mut task = store::require_task(tx.c, id)?;
            if let Some(a) = update.agent_id {
                task.agent_id = Some(a);
            }
            if let Some(m) = update.model {
                task.model = Some(m);
            }
            if let Some(w) = update.worktree {
                task.worktree = Some(w);
            }
            if let Some(u) = update.usage_delta {
                task.usage.add(&u);
            }
            for f in update.files_touched {
                let f = f.trim().replace('\\', "/");
                if f.is_empty() || f.len() > 1024 || task.files_touched.contains(&f) {
                    continue;
                }
                if task.files_touched.len() >= orkestr_protocol::methods::tasks::MAX_FILES {
                    break;
                }
                task.files_touched.push(f);
            }
            if let Some(mut r) = update.result {
                if r.summary.chars().count() > 20_000 {
                    r.summary = r.summary.chars().take(20_000).collect();
                }
                task.result = Some(r);
            }
            task.updated_at = tx.now;
            store::save_task(tx.c, &mut task)?;
            tx.fx.updated.insert(task.id, task.clone());
            Ok(task)
        })
        .await
    }

    /// Status history and execution attempts of a task.
    pub async fn history(&self, id: TaskId) -> Result<(Vec<TaskHistoryEntry>, Vec<TaskAttempt>), TaskError> {
        self.read(move |c| {
            store::require_task(c, id)?;
            Ok((store::history(c, id)?, store::attempts(c, id)?))
        })
        .await
    }

    /// Crash / shutdown recovery: every `Running/Waiting/Testing` task
    /// becomes `Interrupted` through the privileged `SystemRecovery` edges
    /// (`Review` stays: finished work waiting for a human survives a restart)
    /// (open attempts are closed as `Interrupted`). `reason` is
    /// `crash_recovery` or `shutdown`.
    pub async fn recover_interrupted(&self, reason: &'static str) -> Result<RecoveryReport, TaskError> {
        self.write(move |tx| {
            let statuses = TaskStatus::ALL.into_iter().filter(|s| s.is_interruptible_on_crash()).collect();
            let tasks = store::list_tasks(tx.c, &TaskFilter { status: Some(statuses), ..TaskFilter::default() })?;
            let mut report = RecoveryReport::default();
            for mut t in tasks {
                tx.apply(&mut t, TaskStatus::Interrupted, TaskActor::SystemRecovery, Some(reason))?;
                report.interrupted.push(t.id);
            }
            if !report.interrupted.is_empty() {
                tracing::warn!(count = report.interrupted.len(), reason, "tasks interrupted by recovery");
            }
            Ok(report)
        })
        .await
    }

    /// Runs the scheduler over the persisted `Ready` queue and the active
    /// (`Running/Waiting/Review/Testing`) tasks. Pure decision — starting the
    /// returned tasks (agent spawn + `Ready → Running`) is the orchestrator's job.
    pub async fn next_runnable(
        &self,
        scheduler: Scheduler,
        resources: ResourceSnapshot,
    ) -> Result<SchedulePlan, TaskError> {
        let at = now();
        self.read(move |c| {
            let ready =
                store::list_tasks(c, &TaskFilter { status: Some(vec![TaskStatus::Ready]), ..TaskFilter::default() })?;
            let active = store::list_tasks(
                c,
                &TaskFilter {
                    status: Some(vec![
                        TaskStatus::Running,
                        TaskStatus::Waiting,
                        TaskStatus::Review,
                        TaskStatus::Testing,
                    ]),
                    ..TaskFilter::default()
                },
            )?;
            let to_sched = SchedTask::from_task;
            let candidates: Vec<SchedTask> = ready.iter().map(to_sched).collect();
            let running: Vec<SchedTask> = active.iter().map(to_sched).collect();
            Ok(scheduler.next_runnable(&candidates, &running, &resources, at))
        })
        .await
    }
}
