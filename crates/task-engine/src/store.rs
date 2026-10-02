//! SQLite persistence of tasks, dependencies, history and execution attempts.
//!
//! Tasks are stored as a JSON document plus indexed columns. On load the
//! columns are authoritative for `status`, `parentId`, `version` and
//! `readySince`; `dependencies` and `executions` are filled from their tables.
//! All functions are synchronous and run inside `Db::call` (usually within a
//! transaction opened by [`crate::service::TaskService`]).

use std::collections::BTreeMap;

use orkestr_domain::{
    DependencyKind, ExecutionId, ExecutionStatus, ProjectId, Task, TaskActor, TaskAttempt, TaskDependency,
    TaskHistoryEntry, TaskId, TaskStatus, Timestamp, UsageSummary,
};
use orkestr_storage::rusqlite::{self, params, Connection, OptionalExtension, Row};
use orkestr_storage::Migration;

use crate::TaskError;

/// Migrations owned by the task engine (applied after `storage_*`, which
/// creates `projects`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "task_0001_tasks",
    owner: "task-engine",
    up: "CREATE TABLE tasks (
            id           TEXT PRIMARY KEY NOT NULL,
            project_id   TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            parent_id    TEXT REFERENCES tasks(id) ON DELETE SET NULL,
            status       TEXT NOT NULL,
            priority     TEXT NOT NULL,
            agent_id     TEXT,
            version      INTEGER NOT NULL DEFAULT 0,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            ready_since  TEXT,
            doc          TEXT NOT NULL
         );
         CREATE INDEX tasks_project_status ON tasks(project_id, status);
         CREATE INDEX tasks_parent ON tasks(parent_id);
         CREATE INDEX tasks_agent ON tasks(agent_id);
         CREATE INDEX tasks_status ON tasks(status);
         CREATE TABLE task_dependencies (
            task_id     TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            depends_on  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            kind        TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            PRIMARY KEY (task_id, depends_on),
            CHECK (task_id <> depends_on)
         );
         CREATE INDEX task_dependencies_depends_on ON task_dependencies(depends_on);
         CREATE TABLE task_history (
            seq          INTEGER PRIMARY KEY AUTOINCREMENT,
            task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            from_status  TEXT,
            to_status    TEXT NOT NULL,
            actor        TEXT NOT NULL,
            reason       TEXT,
            forced       INTEGER NOT NULL DEFAULT 0,
            at           TEXT NOT NULL
         );
         CREATE INDEX task_history_task ON task_history(task_id, seq);
         CREATE TABLE task_executions (
            id           TEXT PRIMARY KEY NOT NULL,
            task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            attempt      INTEGER NOT NULL,
            agent_id     TEXT,
            status       TEXT NOT NULL,
            usage        TEXT NOT NULL,
            summary      TEXT,
            started_at   TEXT NOT NULL,
            finished_at  TEXT
         );
         CREATE INDEX task_executions_task ON task_executions(task_id, attempt);",
    down: "DROP INDEX IF EXISTS task_executions_task;
           DROP TABLE IF EXISTS task_executions;
           DROP INDEX IF EXISTS task_history_task;
           DROP TABLE IF EXISTS task_history;
           DROP INDEX IF EXISTS task_dependencies_depends_on;
           DROP TABLE IF EXISTS task_dependencies;
           DROP INDEX IF EXISTS tasks_status;
           DROP INDEX IF EXISTS tasks_agent;
           DROP INDEX IF EXISTS tasks_parent;
           DROP INDEX IF EXISTS tasks_project_status;
           DROP TABLE IF EXISTS tasks;",
}];

fn enum_str<T: serde::Serialize>(v: &T) -> Result<String, TaskError> {
    match serde_json::to_value(v)? {
        serde_json::Value::String(s) => Ok(s),
        other => Err(TaskError::Storage(format!("expected string enum, got {other}"))),
    }
}

fn parse_enum<T: serde::de::DeserializeOwned>(s: &str) -> Result<T, TaskError> {
    Ok(serde_json::from_value(serde_json::Value::String(s.to_owned()))?)
}

fn parse_ts(s: &str) -> Result<Timestamp, TaskError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| TaskError::Storage(format!("bad timestamp `{s}`: {e}")))
}

fn parse_id<T: std::str::FromStr>(s: &str) -> Result<T, TaskError>
where
    T::Err: std::fmt::Display,
{
    s.parse::<T>().map_err(|e| TaskError::Storage(e.to_string()))
}

/// Filter for [`list_tasks`].
#[derive(Debug, Clone, Default)]
pub struct TaskFilter {
    /// Project.
    pub project_id: Option<ProjectId>,
    /// Statuses (any of).
    pub status: Option<Vec<TaskStatus>>,
    /// Parent.
    pub parent_id: Option<TaskId>,
    /// Agent.
    pub agent_id: Option<String>,
}

struct RawTask {
    doc: String,
    status: String,
    parent_id: Option<String>,
    version: i64,
    ready_since: Option<String>,
}

fn raw(r: &Row<'_>) -> rusqlite::Result<RawTask> {
    Ok(RawTask { doc: r.get(0)?, status: r.get(1)?, parent_id: r.get(2)?, version: r.get(3)?, ready_since: r.get(4)? })
}

const SELECT: &str = "SELECT doc, status, parent_id, version, ready_since FROM tasks";

fn hydrate(c: &Connection, raw: RawTask) -> Result<Task, TaskError> {
    let mut task: Task = serde_json::from_str(&raw.doc)?;
    task.status = parse_enum(&raw.status)?;
    task.parent_id = raw.parent_id.as_deref().map(parse_id).transpose()?;
    task.version = u32::try_from(raw.version).unwrap_or(u32::MAX);
    task.ready_since = raw.ready_since.as_deref().map(parse_ts).transpose()?;
    task.dependencies = dependencies_of(c, task.id)?.into_iter().map(|d| d.depends_on).collect();
    let mut stmt =
        c.prepare_cached("SELECT id FROM task_executions WHERE task_id = ?1 ORDER BY attempt, started_at")?;
    let ids = stmt.query_map([task.id.to_string()], |r| r.get::<_, String>(0))?;
    task.executions = ids.map(|s| parse_id(&s?)).collect::<Result<_, _>>()?;
    Ok(task)
}

/// Loads one task.
pub fn load_task(c: &Connection, id: TaskId) -> Result<Option<Task>, TaskError> {
    let row = c.query_row(&format!("{SELECT} WHERE id = ?1"), [id.to_string()], raw).optional()?;
    row.map(|r| hydrate(c, r)).transpose()
}

/// Loads one task or fails with `NotFound`.
pub fn require_task(c: &Connection, id: TaskId) -> Result<Task, TaskError> {
    load_task(c, id)?.ok_or(TaskError::NotFound(id))
}

/// Lists tasks matching `filter`, oldest first.
pub fn list_tasks(c: &Connection, filter: &TaskFilter) -> Result<Vec<Task>, TaskError> {
    let mut sql = format!("{SELECT} WHERE 1=1");
    let mut args: Vec<String> = Vec::new();
    if let Some(p) = filter.project_id {
        args.push(p.to_string());
        sql.push_str(&format!(" AND project_id = ?{}", args.len()));
    }
    if let Some(p) = filter.parent_id {
        args.push(p.to_string());
        sql.push_str(&format!(" AND parent_id = ?{}", args.len()));
    }
    if let Some(a) = &filter.agent_id {
        args.push(a.clone());
        sql.push_str(&format!(" AND agent_id = ?{}", args.len()));
    }
    if let Some(statuses) = &filter.status {
        if statuses.is_empty() {
            return Ok(Vec::new());
        }
        let mut marks = Vec::new();
        for s in statuses {
            args.push(enum_str(s)?);
            marks.push(format!("?{}", args.len()));
        }
        sql.push_str(&format!(" AND status IN ({})", marks.join(",")));
    }
    sql.push_str(" ORDER BY created_at, id");
    let mut stmt = c.prepare(&sql)?;
    let rows = stmt.query_map(rusqlite::params_from_iter(args.iter()), raw)?;
    let raws: Vec<RawTask> = rows.collect::<Result<_, _>>()?;
    raws.into_iter().map(|r| hydrate(c, r)).collect()
}

fn write_params(t: &Task) -> Result<(String, String, String, Option<String>, String), TaskError> {
    Ok((
        enum_str(&t.status)?,
        enum_str(&t.priority)?,
        serde_json::to_string(t)?,
        t.ready_since.map(|r| r.to_rfc3339()),
        t.updated_at.to_rfc3339(),
    ))
}

/// Inserts a new task (version 0).
pub fn insert_task(c: &Connection, t: &Task) -> Result<(), TaskError> {
    let (status, priority, doc, ready_since, updated_at) = write_params(t)?;
    c.execute(
        "INSERT INTO tasks (id, project_id, parent_id, status, priority, agent_id, version, created_at, updated_at, ready_since, doc)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            t.id.to_string(),
            t.project_id.to_string(),
            t.parent_id.map(|p| p.to_string()),
            status,
            priority,
            t.agent_id.map(|a| a.to_string()),
            i64::from(t.version),
            t.created_at.to_rfc3339(),
            updated_at,
            ready_since,
            doc
        ],
    )?;
    Ok(())
}

/// Persists a modified task whose stored version is `t.version`; bumps
/// `t.version` on success, `VersionConflict` otherwise.
pub fn save_task(c: &Connection, t: &mut Task) -> Result<(), TaskError> {
    let old = t.version;
    t.version = old.saturating_add(1);
    let (status, priority, doc, ready_since, updated_at) = write_params(t)?;
    let n = c.execute(
        "UPDATE tasks SET parent_id = ?3, status = ?4, priority = ?5, agent_id = ?6, version = ?7, updated_at = ?8,
         ready_since = ?9, doc = ?10 WHERE id = ?1 AND version = ?2",
        params![
            t.id.to_string(),
            i64::from(old),
            t.parent_id.map(|p| p.to_string()),
            status,
            priority,
            t.agent_id.map(|a| a.to_string()),
            i64::from(t.version),
            updated_at,
            ready_since,
            doc
        ],
    )?;
    if n == 0 {
        t.version = old;
        let current: Option<i64> =
            c.query_row("SELECT version FROM tasks WHERE id = ?1", [t.id.to_string()], |r| r.get(0)).optional()?;
        return match current {
            None => Err(TaskError::NotFound(t.id)),
            Some(v) => Err(TaskError::VersionConflict { current: u32::try_from(v).unwrap_or(u32::MAX) }),
        };
    }
    Ok(())
}

/// Deletes a task row (edges, history, executions cascade; children's
/// `parent_id` is set to NULL).
pub fn delete_task(c: &Connection, id: TaskId) -> Result<bool, TaskError> {
    Ok(c.execute("DELETE FROM tasks WHERE id = ?1", [id.to_string()])? > 0)
}

/// Whether a project exists.
pub fn project_exists(c: &Connection, id: ProjectId) -> Result<bool, TaskError> {
    Ok(c.query_row("SELECT 1 FROM projects WHERE id = ?1", [id.to_string()], |_| Ok(())).optional()?.is_some())
}

fn dep_row(r: &Row<'_>) -> rusqlite::Result<(String, String, String)> {
    Ok((r.get(0)?, r.get(1)?, r.get(2)?))
}

fn to_dep((t, d, k): (String, String, String)) -> Result<TaskDependency, TaskError> {
    Ok(TaskDependency { task_id: parse_id(&t)?, depends_on: parse_id(&d)?, kind: parse_enum(&k)? })
}

/// Prerequisite edges of a task.
pub fn dependencies_of(c: &Connection, id: TaskId) -> Result<Vec<TaskDependency>, TaskError> {
    let mut stmt = c.prepare_cached(
        "SELECT task_id, depends_on, kind FROM task_dependencies WHERE task_id = ?1 ORDER BY created_at, depends_on",
    )?;
    let rows = stmt.query_map([id.to_string()], dep_row)?;
    rows.map(|r| to_dep(r?)).collect()
}

/// Edges where `id` is the prerequisite.
pub fn dependents_of(c: &Connection, id: TaskId) -> Result<Vec<TaskDependency>, TaskError> {
    let mut stmt = c.prepare_cached(
        "SELECT task_id, depends_on, kind FROM task_dependencies WHERE depends_on = ?1 ORDER BY created_at, task_id",
    )?;
    let rows = stmt.query_map([id.to_string()], dep_row)?;
    rows.map(|r| to_dep(r?)).collect()
}

/// All edges between tasks of a project.
pub fn project_edges(c: &Connection, project: ProjectId) -> Result<Vec<TaskDependency>, TaskError> {
    let mut stmt = c.prepare(
        "SELECT d.task_id, d.depends_on, d.kind FROM task_dependencies d JOIN tasks t ON t.id = d.task_id
         WHERE t.project_id = ?1 ORDER BY d.created_at, d.task_id",
    )?;
    let rows = stmt.query_map([project.to_string()], dep_row)?;
    rows.map(|r| to_dep(r?)).collect()
}

/// Inserts an edge. Returns `false` if it already existed (kind updated).
pub fn insert_dependency(c: &Connection, dep: TaskDependency, now: Timestamp) -> Result<bool, TaskError> {
    let kind = enum_str(&dep.kind)?;
    let n = c.execute(
        "INSERT INTO task_dependencies (task_id, depends_on, kind, created_at) VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(task_id, depends_on) DO NOTHING",
        params![dep.task_id.to_string(), dep.depends_on.to_string(), kind, now.to_rfc3339()],
    )?;
    if n == 0 {
        c.execute(
            "UPDATE task_dependencies SET kind = ?3 WHERE task_id = ?1 AND depends_on = ?2",
            params![dep.task_id.to_string(), dep.depends_on.to_string(), kind],
        )?;
    }
    Ok(n > 0)
}

/// Removes an edge. Returns whether it existed.
pub fn delete_dependency(c: &Connection, task: TaskId, depends_on: TaskId) -> Result<bool, TaskError> {
    Ok(c.execute(
        "DELETE FROM task_dependencies WHERE task_id = ?1 AND depends_on = ?2",
        params![task.to_string(), depends_on.to_string()],
    )? > 0)
}

/// Direct children ids.
pub fn children_of(c: &Connection, id: TaskId) -> Result<Vec<TaskId>, TaskError> {
    let mut stmt = c.prepare_cached("SELECT id FROM tasks WHERE parent_id = ?1 ORDER BY created_at, id")?;
    let rows = stmt.query_map([id.to_string()], |r| r.get::<_, String>(0))?;
    rows.map(|r| parse_id(&r?)).collect()
}

/// Parent of a task (column), `None` if root or unknown.
pub fn parent_of(c: &Connection, id: TaskId) -> Result<Option<TaskId>, TaskError> {
    let p: Option<Option<String>> =
        c.query_row("SELECT parent_id FROM tasks WHERE id = ?1", [id.to_string()], |r| r.get(0)).optional()?;
    p.flatten().as_deref().map(parse_id).transpose()
}

/// Statuses of a set of tasks.
pub fn statuses(c: &Connection, ids: &[TaskId]) -> Result<BTreeMap<TaskId, TaskStatus>, TaskError> {
    let mut out = BTreeMap::new();
    let mut stmt = c.prepare_cached("SELECT status FROM tasks WHERE id = ?1")?;
    for id in ids {
        if let Some(s) = stmt.query_row([id.to_string()], |r| r.get::<_, String>(0)).optional()? {
            out.insert(*id, parse_enum(&s)?);
        }
    }
    Ok(out)
}

/// A history row to append.
#[derive(Debug, Clone, Copy)]
pub struct HistoryRecord<'a> {
    /// Task.
    pub task: TaskId,
    /// Previous status (`None` = created).
    pub from: Option<TaskStatus>,
    /// New status.
    pub to: TaskStatus,
    /// Actor.
    pub actor: TaskActor,
    /// Reason.
    pub reason: Option<&'a str>,
    /// Privileged edge used.
    pub forced: bool,
    /// When.
    pub at: Timestamp,
}

/// Appends a history row; returns its seq.
pub fn insert_history(c: &Connection, rec: &HistoryRecord<'_>) -> Result<i64, TaskError> {
    let HistoryRecord { task, from, to, actor, reason, forced, at } = *rec;
    c.execute(
        "INSERT INTO task_history (task_id, from_status, to_status, actor, reason, forced, at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            task.to_string(),
            from.map(|f| enum_str(&f)).transpose()?,
            enum_str(&to)?,
            serde_json::to_string(&actor)?,
            reason,
            forced,
            at.to_rfc3339()
        ],
    )?;
    Ok(c.last_insert_rowid())
}

/// History of a task, oldest first.
pub fn history(c: &Connection, task: TaskId) -> Result<Vec<TaskHistoryEntry>, TaskError> {
    let mut stmt = c.prepare(
        "SELECT seq, from_status, to_status, actor, reason, forced, at FROM task_history WHERE task_id = ?1 ORDER BY seq",
    )?;
    let rows = stmt.query_map([task.to_string()], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, Option<String>>(1)?,
            r.get::<_, String>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, Option<String>>(4)?,
            r.get::<_, bool>(5)?,
            r.get::<_, String>(6)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (seq, from, to, actor, reason, forced, at) = row?;
        out.push(TaskHistoryEntry {
            seq,
            task_id: task,
            from: from.as_deref().map(parse_enum).transpose()?,
            to: parse_enum(&to)?,
            actor: serde_json::from_str(&actor)?,
            reason,
            forced,
            at: parse_ts(&at)?,
        });
    }
    Ok(out)
}

/// Opens a new execution attempt.
pub fn open_attempt(c: &Connection, task: &Task, now: Timestamp) -> Result<TaskAttempt, TaskError> {
    let attempt = TaskAttempt {
        id: ExecutionId::new(),
        task_id: task.id,
        attempt: task.attempts.max(1),
        agent_id: task.agent_id,
        status: ExecutionStatus::Running,
        usage: UsageSummary::default(),
        summary: None,
        started_at: now,
        finished_at: None,
    };
    c.execute(
        "INSERT INTO task_executions (id, task_id, attempt, agent_id, status, usage, summary, started_at, finished_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, NULL, ?7, NULL)",
        params![
            attempt.id.to_string(),
            task.id.to_string(),
            i64::from(attempt.attempt),
            attempt.agent_id.map(|a| a.to_string()),
            enum_str(&attempt.status)?,
            serde_json::to_string(&attempt.usage)?,
            now.to_rfc3339()
        ],
    )?;
    Ok(attempt)
}

/// Closes the open (`Running`) attempt of a task, if any.
pub fn close_open_attempt(
    c: &Connection,
    task: TaskId,
    status: ExecutionStatus,
    summary: Option<&str>,
    now: Timestamp,
) -> Result<bool, TaskError> {
    let running = enum_str(&ExecutionStatus::Running)?;
    Ok(c.execute(
        "UPDATE task_executions SET status = ?2, summary = COALESCE(?3, summary), finished_at = ?4
         WHERE task_id = ?1 AND status = ?5",
        params![task.to_string(), enum_str(&status)?, summary, now.to_rfc3339(), running],
    )? > 0)
}

/// Attempts of a task, oldest first.
pub fn attempts(c: &Connection, task: TaskId) -> Result<Vec<TaskAttempt>, TaskError> {
    let mut stmt = c.prepare(
        "SELECT id, attempt, agent_id, status, usage, summary, started_at, finished_at FROM task_executions
         WHERE task_id = ?1 ORDER BY attempt, started_at",
    )?;
    let rows = stmt.query_map([task.to_string()], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, i64>(1)?,
            r.get::<_, Option<String>>(2)?,
            r.get::<_, String>(3)?,
            r.get::<_, String>(4)?,
            r.get::<_, Option<String>>(5)?,
            r.get::<_, String>(6)?,
            r.get::<_, Option<String>>(7)?,
        ))
    })?;
    let mut out = Vec::new();
    for row in rows {
        let (id, attempt, agent, status, usage, summary, started, finished) = row?;
        out.push(TaskAttempt {
            id: parse_id(&id)?,
            task_id: task,
            attempt: u32::try_from(attempt).unwrap_or(0),
            agent_id: agent.as_deref().map(parse_id).transpose()?,
            status: parse_enum(&status)?,
            usage: serde_json::from_str(&usage)?,
            summary,
            started_at: parse_ts(&started)?,
            finished_at: finished.as_deref().map(parse_ts).transpose()?,
        });
    }
    Ok(out)
}

/// Default dependency kind helper.
#[must_use]
pub fn hard(task_id: TaskId, depends_on: TaskId) -> TaskDependency {
    TaskDependency { task_id, depends_on, kind: DependencyKind::FinishToStart }
}
