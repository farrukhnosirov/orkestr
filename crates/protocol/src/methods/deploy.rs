//! `environments.*` and `deploy.*` — environments CRUD, deployment adapters,
//! plan / start / cancel / logs / rollback (Phase 7, ADR-0041). **Owner: WP4**
//! (crate `orkestr-deploy`, handlers `core/src/rpc/deploy.rs`).
//!
//! Deploys to protected environments (Production is always protected) go
//! through the Dangerous Action Guard: `deploy.start` returns the deployment
//! in `AwaitingApproval` with the approval id; it runs after approval.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{
    ApprovalId, Deployment, DeploymentId, DeploymentStatus, DeploymentTarget, Environment, EnvironmentId,
    EnvironmentKind, ProjectId, SecretRefId,
};

use super::{method, Empty};

/// An environment with its deployment configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvironmentRecord {
    /// The environment.
    pub environment: Environment,
    /// Deployment target (adapter) of this environment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<DeploymentTarget>,
    /// Adapter settings (non-secret: output dir, command, health URL, site/app name, namespace...).
    #[serde(default)]
    pub adapter_settings: BTreeMap<String, String>,
    /// Latest deployment.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_deployment: Option<Deployment>,
}

/// `environments.list` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEnvironmentsParams {
    /// Project.
    pub project_id: ProjectId,
}

/// `environments.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListEnvironmentsResult {
    /// Environments.
    pub environments: Vec<EnvironmentRecord>,
}

/// `environments.create` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateEnvironmentParams {
    /// Project.
    pub project_id: ProjectId,
    /// Name.
    pub name: String,
    /// Kind.
    pub kind: EnvironmentKind,
    /// Protected (Production is always protected).
    #[serde(default)]
    pub protected: bool,
    /// Non-secret variables.
    #[serde(default)]
    pub variables: BTreeMap<String, String>,
    /// Secret references (tokens: `VERCEL_TOKEN`, ...).
    #[serde(default)]
    pub secret_refs: Vec<SecretRefId>,
    /// URLs (app, health).
    #[serde(default)]
    pub urls: Vec<String>,
    /// Target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<DeploymentTarget>,
    /// Adapter settings.
    #[serde(default)]
    pub adapter_settings: BTreeMap<String, String>,
}

/// `environments.update` params (absent fields stay unchanged).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct UpdateEnvironmentParams {
    /// Environment.
    pub environment_id: EnvironmentId,
    /// Name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    /// Kind.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kind: Option<EnvironmentKind>,
    /// Protected.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub protected: Option<bool>,
    /// Variables (replaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub variables: Option<BTreeMap<String, String>>,
    /// Secret references (replaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub secret_refs: Option<Vec<SecretRefId>>,
    /// URLs (replaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub urls: Option<Vec<String>>,
    /// Target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<DeploymentTarget>,
    /// Adapter settings (replaces).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub adapter_settings: Option<BTreeMap<String, String>>,
}

/// Params naming one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvironmentIdParams {
    /// Environment.
    pub environment_id: EnvironmentId,
}

/// Result carrying one environment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EnvironmentResult {
    /// The environment.
    pub environment: EnvironmentRecord,
}

/// Whether an adapter can run here.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "state", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum AdapterAvailability {
    /// Ready to deploy.
    Ready,
    /// A vault secret is missing (e.g. `VERCEL_TOKEN`).
    NeedsCredential {
        /// Secret name expected in the environment's secret refs / vault.
        secret: String,
    },
    /// Cannot run on this machine (e.g. `docker` not on PATH).
    Unavailable {
        /// Reason.
        reason: String,
    },
}

/// A deployment adapter.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdapterInfo {
    /// Adapter id (`staticSite`, `localProcess`, `docker`, `vercel`, ...).
    pub id: String,
    /// Target.
    pub target: DeploymentTarget,
    /// Availability.
    pub availability: AdapterAvailability,
    /// `live` (exercised end to end) or `requestShape` (verified against a fake API only).
    pub verification: String,
    /// JSON schema of `adapterSettings`.
    #[ts(type = "unknown")]
    pub settings_schema: serde_json::Value,
}

/// `deploy.adapters` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdaptersResult {
    /// Adapters.
    pub adapters: Vec<AdapterInfo>,
}

/// `deploy.plan` / `deploy.start` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeployParams {
    /// Environment.
    pub environment_id: EnvironmentId,
    /// Version / git ref (default `HEAD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub version: Option<String>,
    /// Source directory relative to the project root (adapter default otherwise).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_dir: Option<String>,
}

/// A redacted preview of one outgoing request / command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RequestPreview {
    /// HTTP method or `exec`.
    pub method: String,
    /// URL or command line (redacted).
    pub target: String,
    /// Body summary (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub body: Option<String>,
}

/// What a deployment would do.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeployPlan {
    /// Adapter id.
    pub adapter_id: String,
    /// Target.
    pub target: DeploymentTarget,
    /// Environment.
    pub environment_id: EnvironmentId,
    /// Version to deploy.
    pub version: String,
    /// Needs Dangerous Action Guard approval (protected environment).
    pub requires_approval: bool,
    /// Human-readable steps.
    pub steps: Vec<String>,
    /// Redacted requests / commands.
    pub requests: Vec<RequestPreview>,
    /// Warnings (missing health URL, unavailable adapter, ...).
    pub warnings: Vec<String>,
}

/// `deploy.plan` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeployPlanResult {
    /// The plan.
    pub plan: DeployPlan,
}

/// Result carrying one deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeploymentResult {
    /// The deployment.
    pub deployment: Deployment,
    /// Pending approval (protected environments).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
}

/// Params naming one deployment.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeploymentIdParams {
    /// Deployment.
    pub deployment_id: DeploymentId,
}

/// `deploy.list` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListDeploymentsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Environment filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub environment_id: Option<EnvironmentId>,
    /// Max rows (newest first; default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `deploy.list` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListDeploymentsResult {
    /// Deployments.
    pub deployments: Vec<Deployment>,
}

/// `deploy.logs` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeployLogsParams {
    /// Deployment.
    pub deployment_id: DeploymentId,
    /// Line offset (default 0).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub offset: Option<u64>,
}

/// `deploy.logs` result (redacted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeployLogsResult {
    /// Lines from `offset`.
    pub lines: Vec<String>,
    /// Offset for the next call.
    pub next_offset: u64,
    /// The deployment finished (no more lines).
    pub complete: bool,
}

/// `deploy.rollback` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RollbackDeploymentParams {
    /// Environment to roll back.
    pub environment_id: EnvironmentId,
    /// Version to restore (default: the previous successful one).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub to_version: Option<String>,
}

/// `DeploymentStarted`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeploymentStartedPayload {
    /// The deployment.
    pub deployment: Deployment,
}

/// `DeploymentProgress` (ephemeral).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeploymentProgressPayload {
    /// Deployment.
    pub deployment_id: DeploymentId,
    /// Project.
    pub project_id: ProjectId,
    /// Status.
    pub status: DeploymentStatus,
    /// Current step.
    pub step: String,
    /// Last log line (redacted).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}

method!(/// Environments of a project.
    EnvironmentsList, "environments.list", ListEnvironmentsParams, ListEnvironmentsResult);
method!(/// Creates an environment.
    EnvironmentsCreate, "environments.create", CreateEnvironmentParams, EnvironmentResult);
method!(/// Updates an environment (admin + step-up).
    EnvironmentsUpdate, "environments.update", UpdateEnvironmentParams, EnvironmentResult);
method!(/// Deletes an environment (admin + step-up).
    EnvironmentsDelete, "environments.delete", EnvironmentIdParams, Empty);
method!(/// Deployment adapters and their availability.
    Adapters, "deploy.adapters", Empty, AdaptersResult);
method!(/// Plans a deployment (redacted previews, no side effects).
    Plan, "deploy.plan", DeployParams, DeployPlanResult);
method!(/// Starts a deployment (protected → approval first).
    Start, "deploy.start", DeployParams, DeploymentResult);
method!(/// Cancels a running deployment (emergency class).
    Cancel, "deploy.cancel", DeploymentIdParams, DeploymentResult);
method!(/// Deployments of a project.
    List, "deploy.list", ListDeploymentsParams, ListDeploymentsResult);
method!(/// One deployment.
    Get, "deploy.get", DeploymentIdParams, DeploymentResult);
method!(/// Deployment log lines.
    Logs, "deploy.logs", DeployLogsParams, DeployLogsResult);
method!(/// Rolls an environment back to a previous version.
    Rollback, "deploy.rollback", RollbackDeploymentParams, DeploymentResult);
