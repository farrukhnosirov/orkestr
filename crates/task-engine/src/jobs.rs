//! Remote job table (Phase 6, node-protocol.md §6). **Owner: WP-B.**
//!
//! One row per job: current lease epoch, owner node, state and the stored
//! placement explanation. The task engine stays the only writer of task status;
//! job rows only track placement/lease bookkeeping. [`JobStore::set_task_owner`]
//! writes `Task.owner` (not the status) inside the task engine; status changes
//! go through [`crate::TaskService::transition`].
//!
//! ## API (consumed by `jobs.submit` and WP-C)
//! * [`JobStore::create`] — new `Pending` row (epoch 0) for a spec + first placement.
//! * [`JobStore::get`] / [`JobStore::list`] / [`JobStore::active_for_task`] /
//!   [`JobStore::lease_until`] / [`JobStore::expired_leases`].
//! * [`JobStore::set_placement`] — store a (re-)placement decision (Wait/Infeasible/Place).
//! * [`JobStore::assign`]`(job, node, epoch)` — fenced: `epoch` must be **greater**
//!   than the stored one; sets node/owner/state `Offered`, publishes `JobAssigned`.
//! * [`JobStore::bump_epoch`]`(job, expected)` — CAS `expected → expected+1`.
//! * [`JobStore::set_state`] / [`JobStore::record_checkpoint`] /
//!   [`JobStore::set_lease_until`] / [`JobStore::update_fenced`] — fenced: `epoch`
//!   must **equal** the stored one, else [`JobError::StaleEpoch`].
//! * [`check_epoch`] — the pure fencing rule (also for progress events).
//! * [`JobDispatcher`] — hook WP-C implements; `jobs.submit` calls it after a placement.

use std::collections::BTreeMap;

use orkestr_domain::{now, CheckpointId, ExecutorRef, JobId, LeaseEpoch, NodeId, Task, TaskId, Timestamp};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, NewEvent, TaskPayload};
use orkestr_protocol::methods::jobs::{JobAssignedPayload, JobSpec, JobState, JobView, Placement};
use orkestr_storage::rusqlite::{params, Connection, OptionalExtension};
use orkestr_storage::{Db, Migration};

use crate::node_scheduler::explanation_of;
use crate::{store, TaskError};

/// Migrations of the job table (`job_migrations()`, appended at the end of the core list).
pub const JOB_MIGRATIONS: &[Migration] = &[Migration {
    id: "task_0601_jobs",
    owner: "task-engine",
    up: "CREATE TABLE jobs (
            id           TEXT PRIMARY KEY NOT NULL,
            task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
            project_id   TEXT NOT NULL,
            node_id      TEXT,
            epoch        INTEGER NOT NULL DEFAULT 0,
            state        TEXT NOT NULL,
            lease_until  TEXT,
            created_at   TEXT NOT NULL,
            updated_at   TEXT NOT NULL,
            doc          TEXT NOT NULL
         );
         CREATE INDEX jobs_task ON jobs(task_id);
         CREATE INDEX jobs_node_state ON jobs(node_id, state);",
    down: "DROP TABLE IF EXISTS jobs;",
}];

/// Hands a placed job to the execution layer (WP-C: offer to the node, leases).
/// Called by `jobs.submit` after the job row is `Offered` with epoch ≥ 1.
/// Must not block: implementations spawn their work.
pub trait JobDispatcher: Send + Sync {
    /// Starts delivering `job_id` (read it back with [`JobStore::get`]).
    fn dispatch(&self, job_id: JobId);
}

/// Errors of the job table.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// Unknown job.
    #[error("job {0} not found")]
    NotFound(JobId),
    /// Fencing: the caller's epoch is not the current one.
    #[error("stale epoch {got} (current {current})")]
    StaleEpoch {
        /// Stored epoch.
        current: LeaseEpoch,
        /// Epoch presented.
        got: LeaseEpoch,
    },
    /// Task / storage failure.
    #[error(transparent)]
    Task(#[from] TaskError),
}

impl From<orkestr_storage::StorageError> for JobError {
    fn from(e: orkestr_storage::StorageError) -> Self {
        Self::Task(e.into())
    }
}

impl From<orkestr_storage::rusqlite::Error> for JobError {
    fn from(e: orkestr_storage::rusqlite::Error) -> Self {
        Self::Task(e.into())
    }
}

impl From<serde_json::Error> for JobError {
    fn from(e: serde_json::Error) -> Self {
        Self::Task(e.into())
    }
}

/// Fencing rule: a job-side message is accepted only with the current epoch.
pub fn check_epoch(current: LeaseEpoch, got: LeaseEpoch) -> Result<(), JobError> {
    if current == got {
        Ok(())
    } else {
        Err(JobError::StaleEpoch { current, got })
    }
}

/// Whether a job state is terminal.
#[must_use]
pub fn is_terminal(s: JobState) -> bool {
    matches!(s, JobState::Completed | JobState::Failed | JobState::Cancelled | JobState::Infeasible)
}

/// `jobs.list` filter (AND).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct JobFilter {
    /// Task.
    pub task_id: Option<TaskId>,
    /// Node.
    pub node_id: Option<NodeId>,
    /// Only non-terminal jobs.
    pub active_only: bool,
}

fn state_str(s: JobState) -> Result<String, JobError> {
    Ok(serde_json::to_value(s)?.as_str().unwrap_or_default().to_owned())
}

const TERMINAL_SQL: &str = "('completed','failed','cancelled','infeasible')";

fn load(c: &Connection, id: JobId) -> Result<(JobView, Option<String>), JobError> {
    let row: Option<(String, Option<String>)> = c
        .query_row("SELECT doc, lease_until FROM jobs WHERE id = ?1", [id.to_string()], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    let (doc, lease) = row.ok_or(JobError::NotFound(id))?;
    Ok((serde_json::from_str(&doc)?, lease))
}

fn save(c: &Connection, v: &JobView, lease_until: Option<Timestamp>) -> Result<(), JobError> {
    let node = match v.owner {
        Some(ExecutorRef::Node { node_id }) => Some(node_id.to_string()),
        _ => None,
    };
    c.execute(
        "INSERT INTO jobs (id, task_id, project_id, node_id, epoch, state, lease_until, created_at, updated_at, doc)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)
         ON CONFLICT(id) DO UPDATE SET node_id = ?4, epoch = ?5, state = ?6, lease_until = ?7, updated_at = ?9, doc = ?10",
        params![
            v.spec.job_id.to_string(),
            v.spec.task_id.to_string(),
            v.spec.project_id.to_string(),
            node,
            i64::try_from(v.epoch.0).unwrap_or(i64::MAX),
            state_str(v.state)?,
            lease_until.map(|t| t.to_rfc3339()),
            v.created_at.to_rfc3339(),
            v.updated_at.to_rfc3339(),
            serde_json::to_string(v)?,
        ],
    )?;
    Ok(())
}

fn parse_ts(s: Option<String>) -> Option<Timestamp> {
    s.and_then(|s| chrono::DateTime::parse_from_rfc3339(&s).ok()).map(|t| t.with_timezone(&chrono::Utc))
}

/// SQLite job table + `JobAssigned` events. Cheap to clone/construct.
#[derive(Clone)]
pub struct JobStore {
    db: Db,
    events: EventBus,
}

impl std::fmt::Debug for JobStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("JobStore").finish_non_exhaustive()
    }
}

impl JobStore {
    /// Over a database migrated with [`crate::job_migrations`].
    #[must_use]
    pub fn new(db: Db, events: EventBus) -> Self {
        Self { db, events }
    }

    async fn run<T, F>(&self, f: F) -> Result<T, JobError>
    where
        T: Send + 'static,
        F: FnOnce(&mut Connection) -> Result<T, JobError> + Send + 'static,
    {
        self.db.call(move |c| Ok(f(c))).await?
    }

    /// Read-modify-write of one job in a transaction; `check` runs on the stored row first.
    async fn modify<F>(&self, id: JobId, f: F) -> Result<JobView, JobError>
    where
        F: FnOnce(&mut JobView, &mut Option<Timestamp>) -> Result<(), JobError> + Send + 'static,
    {
        self.run(move |c| {
            let tx = c.transaction()?;
            let (mut v, lease) = load(&tx, id)?;
            let mut lease = parse_ts(lease);
            f(&mut v, &mut lease)?;
            v.updated_at = now();
            save(&tx, &v, lease)?;
            tx.commit()?;
            Ok(v)
        })
        .await
    }

    /// Inserts a `Pending` job (epoch 0, no owner) with its first placement decision.
    pub async fn create(&self, spec: JobSpec, placement: Option<Placement>) -> Result<JobView, JobError> {
        let t = now();
        let v = JobView {
            spec,
            state: JobState::Pending,
            owner: None,
            epoch: LeaseEpoch(0),
            placement,
            last_checkpoint: None,
            progress: None,
            exit_code: None,
            detail: None,
            artifacts: Vec::new(),
            created_at: t,
            updated_at: t,
        };
        let out = v.clone();
        self.run(move |c| save(c, &v, None)).await?;
        Ok(out)
    }

    /// Loads a job.
    pub async fn get(&self, id: JobId) -> Result<JobView, JobError> {
        self.run(move |c| Ok(load(c, id)?.0)).await
    }

    /// Lease end of a job (`None` when unleased).
    pub async fn lease_until(&self, id: JobId) -> Result<Option<Timestamp>, JobError> {
        self.run(move |c| Ok(parse_ts(load(c, id)?.1))).await
    }

    /// Jobs matching `filter`, newest first.
    pub async fn list(&self, filter: JobFilter) -> Result<Vec<JobView>, JobError> {
        self.run(move |c| {
            let mut sql = String::from("SELECT doc FROM jobs WHERE 1=1");
            let mut args: Vec<String> = Vec::new();
            if let Some(t) = filter.task_id {
                args.push(t.to_string());
                sql.push_str(&format!(" AND task_id = ?{}", args.len()));
            }
            if let Some(n) = filter.node_id {
                args.push(n.to_string());
                sql.push_str(&format!(" AND node_id = ?{}", args.len()));
            }
            if filter.active_only {
                sql.push_str(&format!(" AND state NOT IN {TERMINAL_SQL}"));
            }
            sql.push_str(" ORDER BY created_at DESC, id DESC");
            let mut stmt = c.prepare(&sql)?;
            let docs: Vec<String> = stmt
                .query_map(orkestr_storage::rusqlite::params_from_iter(args.iter()), |r| r.get(0))?
                .collect::<Result<_, _>>()?;
            docs.iter().map(|d| Ok(serde_json::from_str(d)?)).collect()
        })
        .await
    }

    /// The non-terminal job of a task, if any.
    pub async fn active_for_task(&self, task: TaskId) -> Result<Option<JobView>, JobError> {
        Ok(self.list(JobFilter { task_id: Some(task), node_id: None, active_only: true }).await?.into_iter().next())
    }

    /// Active (`Offered`/`Running`/`Migrating`) jobs whose lease ended before `at`.
    pub async fn expired_leases(&self, at: Timestamp) -> Result<Vec<JobView>, JobError> {
        self.run(move |c| {
            let mut stmt = c.prepare(
                "SELECT doc, lease_until FROM jobs WHERE state IN ('offered','running','migrating') AND lease_until IS NOT NULL",
            )?;
            let rows: Vec<(String, Option<String>)> =
                stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<Result<_, _>>()?;
            let mut out = Vec::new();
            for (doc, lease) in rows {
                if parse_ts(lease).is_some_and(|l| l < at) {
                    out.push(serde_json::from_str(&doc)?);
                }
            }
            Ok(out)
        })
        .await
    }

    /// Stores a (re-)placement decision; an `Infeasible` one also sets state `Infeasible`.
    pub async fn set_placement(&self, id: JobId, placement: Placement) -> Result<JobView, JobError> {
        self.modify(id, move |v, _| {
            if matches!(placement, Placement::Infeasible { .. }) && !is_terminal(v.state) {
                v.state = JobState::Infeasible;
            }
            v.placement = Some(placement);
            Ok(())
        })
        .await
    }

    /// Assigns the job to `node` under `epoch` (must be > stored epoch): owner,
    /// state `Offered`; publishes `JobAssigned` with the stored placement explanation.
    pub async fn assign(&self, id: JobId, node: NodeId, epoch: LeaseEpoch) -> Result<JobView, JobError> {
        let v = self
            .modify(id, move |v, _| {
                if epoch <= v.epoch {
                    return Err(JobError::StaleEpoch { current: v.epoch, got: epoch });
                }
                v.epoch = epoch;
                v.owner = Some(ExecutorRef::Node { node_id: node });
                v.state = JobState::Offered;
                Ok(())
            })
            .await?;
        let explanation = v.placement.as_ref().map(|p| explanation_of(p).to_vec()).unwrap_or_default();
        let ev = NewEvent::new(EventKind::JobAssigned(JobAssignedPayload {
            job_id: v.spec.job_id,
            task_id: v.spec.task_id,
            project_id: v.spec.project_id,
            node_id: node,
            epoch,
            explanation,
        }))
        .project(v.spec.project_id);
        if let Err(e) = self.events.publish(ev).await {
            tracing::error!(error = %e, "failed to publish JobAssigned");
        }
        Ok(v)
    }

    /// CAS `expected → expected.next()` (invalidates the old lease holder).
    pub async fn bump_epoch(&self, id: JobId, expected: LeaseEpoch) -> Result<LeaseEpoch, JobError> {
        let v = self
            .modify(id, move |v, lease| {
                check_epoch(v.epoch, expected)?;
                v.epoch = expected.next();
                *lease = None;
                Ok(())
            })
            .await?;
        Ok(v.epoch)
    }

    /// Fenced generic update (`epoch` must equal the stored one).
    pub async fn update_fenced<F>(&self, id: JobId, epoch: LeaseEpoch, f: F) -> Result<JobView, JobError>
    where
        F: FnOnce(&mut JobView) + Send + 'static,
    {
        self.modify(id, move |v, _| {
            check_epoch(v.epoch, epoch)?;
            f(v);
            Ok(())
        })
        .await
    }

    /// Fenced state change (+ optional detail).
    pub async fn set_state(
        &self,
        id: JobId,
        epoch: LeaseEpoch,
        state: JobState,
        detail: Option<String>,
    ) -> Result<JobView, JobError> {
        self.update_fenced(id, epoch, move |v| {
            v.state = state;
            if detail.is_some() {
                v.detail = detail;
            }
        })
        .await
    }

    /// Fenced: records the latest checkpoint id.
    pub async fn record_checkpoint(
        &self,
        id: JobId,
        epoch: LeaseEpoch,
        checkpoint: CheckpointId,
    ) -> Result<JobView, JobError> {
        self.update_fenced(id, epoch, move |v| v.last_checkpoint = Some(checkpoint)).await
    }

    /// Fenced: sets / renews the lease end.
    pub async fn set_lease_until(
        &self,
        id: JobId,
        epoch: LeaseEpoch,
        until: Option<Timestamp>,
    ) -> Result<JobView, JobError> {
        self.modify(id, move |v, lease| {
            check_epoch(v.epoch, epoch)?;
            *lease = until;
            Ok(())
        })
        .await
    }

    /// Sets `Task.owner` (never the status) and publishes `TaskUpdated`.
    pub async fn set_task_owner(&self, task: TaskId, owner: Option<ExecutorRef>) -> Result<Task, JobError> {
        let t = self
            .run(move |c| {
                let tx = c.transaction()?;
                let mut t = store::require_task(&tx, task)?;
                if t.owner != owner {
                    t.owner = owner;
                    t.updated_at = now();
                    store::save_task(&tx, &mut t)?;
                }
                tx.commit()?;
                Ok(t)
            })
            .await?;
        if let Err(e) =
            self.events.publish(NewEvent::new(EventKind::TaskUpdated(TaskPayload { task: t.clone() }))).await
        {
            tracing::error!(error = %e, "failed to publish TaskUpdated");
        }
        Ok(t)
    }

    /// Number of active jobs per node (local slot accounting).
    pub async fn active_per_node(&self) -> Result<BTreeMap<NodeId, u32>, JobError> {
        let jobs = self.list(JobFilter { active_only: true, ..JobFilter::default() }).await?;
        let mut m = BTreeMap::new();
        for j in jobs {
            if let Some(ExecutorRef::Node { node_id }) = j.owner {
                *m.entry(node_id).or_insert(0) += 1;
            }
        }
        Ok(m)
    }
}
