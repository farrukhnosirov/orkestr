//! Environments and deployments (§26).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::capability::PermissionRule;
use crate::common::{Actor, Timestamp};
use crate::ids::{DeploymentId, EnvironmentId, ProjectId, SecretRefId, TaskId};

/// Environment kind.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum EnvironmentKind {
    /// Developer machine.
    Local,
    /// Dev.
    Dev,
    /// Test.
    Test,
    /// Staging.
    Staging,
    /// Production — always protected.
    Production,
    /// Custom.
    Custom(String),
}

/// A deployment environment with its configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Environment {
    /// Id.
    pub id: EnvironmentId,
    /// Project.
    pub project_id: ProjectId,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: EnvironmentKind,
    /// Non-secret variables.
    pub variables: BTreeMap<String, String>,
    /// Secret references (values live in the vault).
    pub secret_refs: Vec<SecretRefId>,
    /// Relevant URLs (app, admin, health).
    pub urls: Vec<String>,
    /// Protected environments require approval for writes/deploys.
    pub protected: bool,
    /// Environment-specific permission overrides.
    pub permissions: Vec<PermissionRule>,
    /// Creation time.
    pub created_at: Timestamp,
    /// Last modification.
    pub updated_at: Timestamp,
}

impl Environment {
    /// Production is always treated as protected regardless of the flag.
    #[must_use]
    pub fn is_protected(&self) -> bool {
        self.protected || self.kind == EnvironmentKind::Production
    }
}

/// Deployment target type.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DeploymentTarget {
    /// Docker image/container.
    Docker,
    /// Docker Compose.
    Compose,
    /// VPS over SSH.
    Ssh,
    /// Vercel.
    Vercel,
    /// Cloudflare.
    Cloudflare,
    /// AWS.
    Aws,
    /// GCP.
    Gcp,
    /// Azure.
    Azure,
    /// Custom script (name).
    CustomScript(String),
    /// Supervised local process with a health URL (Phase 7, appended).
    LocalProcess,
    /// Static site published to a served directory (Phase 7, appended).
    StaticSite,
    /// Netlify (Phase 7, appended).
    Netlify,
    /// Fly.io (Phase 7, appended).
    Fly,
    /// Kubernetes (Phase 7, appended).
    Kubernetes,
    /// Deployment provider contributed by a plugin (`<packageId>/<provider>`; Phase 7, appended).
    Plugin(String),
}

/// Deployment status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DeploymentStatus {
    /// Queued.
    Pending,
    /// Waiting for approval.
    AwaitingApproval,
    /// Running.
    Running,
    /// Succeeded.
    Succeeded,
    /// Failed.
    Failed,
    /// Rolled back.
    RolledBack,
    /// Cancelled.
    Cancelled,
}

/// A deployment run (with audit trail via events).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Deployment {
    /// Id.
    pub id: DeploymentId,
    /// Project.
    pub project_id: ProjectId,
    /// Environment.
    pub environment_id: EnvironmentId,
    /// Target.
    pub target: DeploymentTarget,
    /// Status.
    pub status: DeploymentStatus,
    /// Git commit / version deployed.
    pub version: Option<String>,
    /// Task that triggered it.
    pub task_id: Option<TaskId>,
    /// Who triggered it.
    pub triggered_by: Actor,
    /// Deployed URL.
    pub url: Option<String>,
    /// Log object id.
    pub log_object_id: Option<String>,
    /// Start.
    pub started_at: Option<Timestamp>,
    /// End.
    pub finished_at: Option<Timestamp>,
    /// Creation time.
    pub created_at: Timestamp,
}
