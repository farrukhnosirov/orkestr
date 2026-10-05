//! Persistence of merge proposals and per-project merge policies.

use orkestr_domain::{ManagerRunId, MergeProposalId, ProjectId};
use orkestr_protocol::methods::merge::{MergeManagerProposal, MergePolicy, MergeProposalStatus};
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};

use crate::error::{OrchResult, OrchestratorError};

/// Tables of the Merge Manager (applied after the orchestrator's own).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "orchestrator_0101_merge_manager",
    owner: "orchestrator",
    up: "CREATE TABLE merge_proposals (
            id          TEXT PRIMARY KEY NOT NULL,
            project_id  TEXT NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            run_id      TEXT,
            status      TEXT NOT NULL,
            created_at  TEXT NOT NULL,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );
         CREATE INDEX merge_proposals_project ON merge_proposals(project_id, created_at);
         CREATE INDEX merge_proposals_status ON merge_proposals(status);
         CREATE TABLE merge_policies (
            project_id  TEXT PRIMARY KEY NOT NULL REFERENCES projects(id) ON DELETE CASCADE,
            updated_at  TEXT NOT NULL,
            doc         TEXT NOT NULL
         );",
    down: "DROP TABLE IF EXISTS merge_policies;
           DROP INDEX IF EXISTS merge_proposals_status;
           DROP INDEX IF EXISTS merge_proposals_project;
           DROP TABLE IF EXISTS merge_proposals;",
}];

fn status_str(s: MergeProposalStatus) -> Result<String, StorageError> {
    match serde_json::to_value(s)? {
        serde_json::Value::String(s) => Ok(s),
        other => Ok(other.to_string()),
    }
}

/// Persistence facade (cheap to clone).
#[derive(Debug, Clone)]
pub struct MergeStore {
    db: Db,
}

impl MergeStore {
    /// Wraps a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces a proposal.
    pub async fn save(&self, p: &MergeManagerProposal) -> OrchResult<()> {
        let doc = serde_json::to_string(p)?;
        let status = status_str(p.status)?;
        let (id, project, run, created, updated) = (
            p.id.to_string(),
            p.project_id.to_string(),
            p.run_id.map(|r| r.to_string()),
            p.created_at.to_rfc3339(),
            p.updated_at.to_rfc3339(),
        );
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO merge_proposals (id, project_id, run_id, status, created_at, updated_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, updated_at = excluded.updated_at,
                        doc = excluded.doc",
                    params![id, project, run, status, created, updated, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One proposal.
    pub async fn get(&self, id: MergeProposalId) -> OrchResult<MergeManagerProposal> {
        let key = id.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM merge_proposals WHERE id = ?1", [key], |r| r.get(0)).optional()?)
            })
            .await?;
        let doc = doc.ok_or_else(|| OrchestratorError::NotFound(format!("merge proposal {id}")))?;
        Ok(serde_json::from_str(&doc)?)
    }

    /// Proposals of a project, newest first.
    pub async fn list(
        &self,
        project: ProjectId,
        run: Option<ManagerRunId>,
        limit: u32,
    ) -> OrchResult<Vec<MergeManagerProposal>> {
        let (pid, rid) = (project.to_string(), run.map(|r| r.to_string()));
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare(
                    "SELECT doc FROM merge_proposals WHERE project_id = ?1 AND (?2 IS NULL OR run_id = ?2)
                     ORDER BY created_at DESC LIMIT ?3",
                )?;
                let rows = stmt.query_map(params![pid, rid, i64::from(limit)], |r| r.get::<_, String>(0))?;
                Ok(rows.collect::<Result<Vec<_>, _>>()?)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(Into::into)).collect()
    }

    /// Proposals in any of `statuses` (all projects).
    pub async fn with_status(&self, statuses: &[MergeProposalStatus]) -> OrchResult<Vec<MergeManagerProposal>> {
        let wanted: Vec<String> = statuses.iter().map(|s| status_str(*s)).collect::<Result<_, _>>()?;
        let docs: Vec<String> = self
            .db
            .call(move |c| {
                let mut stmt = c.prepare("SELECT status, doc FROM merge_proposals ORDER BY created_at")?;
                let rows = stmt.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
                let mut out = Vec::new();
                for row in rows {
                    let (status, doc) = row?;
                    if wanted.contains(&status) {
                        out.push(doc);
                    }
                }
                Ok(out)
            })
            .await?;
        docs.iter().map(|d| serde_json::from_str(d).map_err(Into::into)).collect()
    }

    /// The project's policy (default when never set).
    pub async fn policy(&self, project: ProjectId) -> OrchResult<MergePolicy> {
        let pid = project.to_string();
        let doc: Option<String> = self
            .db
            .call(move |c| {
                Ok(c.query_row("SELECT doc FROM merge_policies WHERE project_id = ?1", [pid], |r| r.get(0))
                    .optional()?)
            })
            .await?;
        match doc {
            Some(d) => Ok(serde_json::from_str(&d)?),
            None => Ok(MergePolicy::default()),
        }
    }

    /// Replaces the project's policy.
    pub async fn set_policy(&self, project: ProjectId, policy: &MergePolicy) -> OrchResult<()> {
        let doc = serde_json::to_string(policy)?;
        let (pid, at) = (project.to_string(), orkestr_domain::now().to_rfc3339());
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO merge_policies (project_id, updated_at, doc) VALUES (?1, ?2, ?3)
                     ON CONFLICT(project_id) DO UPDATE SET updated_at = excluded.updated_at, doc = excluded.doc",
                    params![pid, at, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }
}
