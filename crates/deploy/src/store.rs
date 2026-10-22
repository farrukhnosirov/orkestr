//! Persistence. Tables are prefixed `deploy_`; migrations are composed by
//! `orkestr-core::migrations::all()` (never reorder, never edit an applied one).

use orkestr_domain::{ApprovalId, Deployment, DeploymentId, DeploymentStatus, EnvironmentId, ProjectId};
use orkestr_protocol::methods::deploy::EnvironmentRecord;
use orkestr_storage::rusqlite::{params, OptionalExtension};
use orkestr_storage::{Db, Migration, StorageError};
use serde::{Deserialize, Serialize};

use crate::DeployError;

/// Migrations owned by this crate (owner `deploy`).
pub const MIGRATIONS: &[Migration] = &[Migration {
    id: "deploy_0001_environments_and_deployments",
    owner: "deploy",
    up: "
CREATE TABLE deploy_environments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    name TEXT NOT NULL,
    created_at TEXT NOT NULL,
    doc TEXT NOT NULL
);
CREATE INDEX deploy_environments_project ON deploy_environments(project_id, name);
CREATE TABLE deploy_deployments (
    id TEXT PRIMARY KEY,
    project_id TEXT NOT NULL,
    environment_id TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at TEXT NOT NULL,
    doc TEXT NOT NULL
);
CREATE INDEX deploy_deployments_project ON deploy_deployments(project_id, created_at);
CREATE INDEX deploy_deployments_env ON deploy_deployments(environment_id, created_at);
CREATE INDEX deploy_deployments_status ON deploy_deployments(status);
",
    down: "DROP TABLE IF EXISTS deploy_deployments; DROP TABLE IF EXISTS deploy_environments;",
}];

/// [`MIGRATIONS`] as a vector.
#[must_use]
pub fn migrations() -> Vec<Migration> {
    MIGRATIONS.to_vec()
}

/// What a deployment record does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum RunKind {
    /// A deployment of a new version.
    #[default]
    Deploy,
    /// A rollback to an earlier version.
    Rollback,
}

/// A deployment row: the domain record plus service bookkeeping.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredDeployment {
    /// Domain record.
    pub deployment: Deployment,
    /// Deploy or rollback.
    #[serde(default)]
    pub kind: RunKind,
    /// Pending / resolved approval.
    #[serde(default)]
    pub approval_id: Option<ApprovalId>,
    /// Provider deployment id (cloud adapters, docker image).
    #[serde(default)]
    pub provider_ref: Option<String>,
    /// Failure reason (redacted).
    #[serde(default)]
    pub error: Option<String>,
    /// Source directory (relative to the project root).
    #[serde(default)]
    pub source_dir: Option<String>,
    /// Rollback: the deployment whose version is restored.
    #[serde(default)]
    pub rollback_of: Option<DeploymentId>,
}

fn status_str(s: DeploymentStatus) -> String {
    serde_json::to_value(s).ok().and_then(|v| v.as_str().map(str::to_owned)).unwrap_or_default()
}

fn decode<T: for<'de> Deserialize<'de>>(doc: &str) -> Result<T, StorageError> {
    serde_json::from_str(doc).map_err(|e| StorageError::Corrupt(e.to_string()))
}

/// Typed access to the `deploy_` tables.
#[derive(Debug, Clone)]
pub struct Store {
    db: Db,
}

impl Store {
    /// Store on a migrated database.
    #[must_use]
    pub fn new(db: Db) -> Self {
        Self { db }
    }

    /// Inserts or replaces an environment (without `last_deployment`).
    pub async fn put_env(&self, rec: &EnvironmentRecord) -> Result<(), DeployError> {
        let mut rec = rec.clone();
        rec.last_deployment = None;
        let e = &rec.environment;
        let (id, project, name, created) =
            (e.id.to_string(), e.project_id.to_string(), e.name.clone(), e.created_at.to_rfc3339());
        let doc = serde_json::to_string(&rec).map_err(|e| DeployError::Internal(e.to_string()))?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO deploy_environments (id, project_id, name, created_at, doc) VALUES (?1, ?2, ?3, ?4, ?5)
                     ON CONFLICT(id) DO UPDATE SET name = excluded.name, doc = excluded.doc",
                    params![id, project, name, created, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One environment.
    pub async fn env(&self, id: EnvironmentId) -> Result<Option<EnvironmentRecord>, DeployError> {
        let id = id.to_string();
        Ok(self
            .db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM deploy_environments WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                doc.as_deref().map(decode).transpose()
            })
            .await?)
    }

    /// Environments of a project (by name), or of every project.
    pub async fn envs(&self, project: Option<ProjectId>) -> Result<Vec<EnvironmentRecord>, DeployError> {
        Ok(self
            .db
            .call(move |c| {
                let mut out = Vec::new();
                match project {
                    Some(p) => {
                        let mut st =
                            c.prepare("SELECT doc FROM deploy_environments WHERE project_id = ?1 ORDER BY name")?;
                        for d in st.query_map([p.to_string()], |r| r.get::<_, String>(0))? {
                            out.push(decode(&d?)?);
                        }
                    }
                    None => {
                        let mut st = c.prepare("SELECT doc FROM deploy_environments ORDER BY name")?;
                        for d in st.query_map([], |r| r.get::<_, String>(0))? {
                            out.push(decode(&d?)?);
                        }
                    }
                }
                Ok(out)
            })
            .await?)
    }

    /// Deletes an environment and its deployment history.
    pub async fn delete_env(&self, id: EnvironmentId) -> Result<bool, DeployError> {
        let id = id.to_string();
        Ok(self
            .db
            .call(move |c| {
                let tx = c.transaction()?;
                tx.execute("DELETE FROM deploy_deployments WHERE environment_id = ?1", [&id])?;
                let n = tx.execute("DELETE FROM deploy_environments WHERE id = ?1", [&id])?;
                tx.commit()?;
                Ok(n > 0)
            })
            .await?)
    }

    /// Inserts or replaces a deployment.
    pub async fn put_dep(&self, d: &StoredDeployment) -> Result<(), DeployError> {
        let dep = &d.deployment;
        let (id, project, env, status, created) = (
            dep.id.to_string(),
            dep.project_id.to_string(),
            dep.environment_id.to_string(),
            status_str(dep.status),
            dep.created_at.to_rfc3339(),
        );
        let doc = serde_json::to_string(d).map_err(|e| DeployError::Internal(e.to_string()))?;
        self.db
            .call(move |c| {
                c.execute(
                    "INSERT INTO deploy_deployments (id, project_id, environment_id, status, created_at, doc)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                     ON CONFLICT(id) DO UPDATE SET status = excluded.status, doc = excluded.doc",
                    params![id, project, env, status, created, doc],
                )?;
                Ok(())
            })
            .await?;
        Ok(())
    }

    /// One deployment.
    pub async fn dep(&self, id: DeploymentId) -> Result<Option<StoredDeployment>, DeployError> {
        let id = id.to_string();
        Ok(self
            .db
            .call(move |c| {
                let doc: Option<String> =
                    c.query_row("SELECT doc FROM deploy_deployments WHERE id = ?1", [id], |r| r.get(0)).optional()?;
                doc.as_deref().map(decode).transpose()
            })
            .await?)
    }

    /// Deployments newest first, by project and/or environment.
    pub async fn deps(
        &self,
        project: Option<ProjectId>,
        env: Option<EnvironmentId>,
        limit: u32,
    ) -> Result<Vec<StoredDeployment>, DeployError> {
        Ok(self
            .db
            .call(move |c| {
                let mut st = c.prepare(
                    "SELECT doc FROM deploy_deployments
                     WHERE (?1 IS NULL OR project_id = ?1) AND (?2 IS NULL OR environment_id = ?2)
                     ORDER BY created_at DESC, rowid DESC LIMIT ?3",
                )?;
                let rows = st.query_map(
                    params![project.map(|p| p.to_string()), env.map(|e| e.to_string()), i64::from(limit)],
                    |r| r.get::<_, String>(0),
                )?;
                let mut out = Vec::new();
                for d in rows {
                    out.push(decode(&d?)?);
                }
                Ok(out)
            })
            .await?)
    }

    /// Deployments in a non-final state (startup recovery).
    pub async fn unfinished(&self) -> Result<Vec<StoredDeployment>, DeployError> {
        let states: Vec<String> =
            [DeploymentStatus::Pending, DeploymentStatus::AwaitingApproval, DeploymentStatus::Running]
                .into_iter()
                .map(status_str)
                .collect();
        Ok(self
            .db
            .call(move |c| {
                let mut st = c.prepare("SELECT doc FROM deploy_deployments WHERE status IN (?1, ?2, ?3)")?;
                let rows = st.query_map(params![states[0], states[1], states[2]], |r| r.get::<_, String>(0))?;
                let mut out = Vec::new();
                for d in rows {
                    out.push(decode(&d?)?);
                }
                Ok(out)
            })
            .await?)
    }
}
