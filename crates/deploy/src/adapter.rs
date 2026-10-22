//! The [`DeployAdapter`] trait (binding for WP4 adapters and plugin providers).

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentId, DeploymentTarget, Environment, ProjectId};
use orkestr_protocol::methods::deploy::{AdapterAvailability, DeployPlan};
use tokio_util::sync::CancellationToken;

use crate::DeployError;

/// Resolved credentials (vault values). `Debug` never prints values.
#[derive(Clone, Default)]
pub struct Credentials(pub BTreeMap<String, String>);

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.0.keys()).finish()
    }
}

/// Everything an adapter needs for one deployment.
#[derive(Debug, Clone)]
pub struct DeployRequest {
    /// Deployment.
    pub deployment_id: DeploymentId,
    /// Project.
    pub project_id: ProjectId,
    /// Project root.
    pub project_root: PathBuf,
    /// Source directory (absolute, confined to the project root).
    pub source_dir: PathBuf,
    /// Environment.
    pub environment: Environment,
    /// Target.
    pub target: DeploymentTarget,
    /// Adapter settings.
    pub settings: BTreeMap<String, String>,
    /// Version (git sha / tag).
    pub version: String,
    /// Credentials (resolved from the environment's secret refs).
    pub credentials: Credentials,
}

/// Result of a deploy / rollback.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DeployOutcome {
    /// Deployed URL.
    pub url: Option<String>,
    /// Version now live.
    pub version: String,
    /// Provider deployment id, if any.
    pub provider_ref: Option<String>,
}

/// Line sink for deployment logs (the service redacts and stores lines).
#[derive(Clone)]
pub struct LogSink(Arc<dyn Fn(&str) + Send + Sync>);

impl std::fmt::Debug for LogSink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("LogSink")
    }
}

impl LogSink {
    /// Sink calling `f` per line.
    pub fn new(f: impl Fn(&str) + Send + Sync + 'static) -> Self {
        Self(Arc::new(f))
    }

    /// Sink dropping every line.
    #[must_use]
    pub fn discard() -> Self {
        Self::new(|_| {})
    }

    /// Writes one line.
    pub fn line(&self, line: &str) {
        (self.0)(line);
    }
}

/// A deployment adapter.
#[async_trait]
pub trait DeployAdapter: Send + Sync {
    /// Adapter id (`staticSite`, `localProcess`, `docker`, `vercel`, ...).
    fn id(&self) -> &str;
    /// Target.
    fn target(&self) -> DeploymentTarget;
    /// Whether it can run here.
    fn availability(&self) -> AdapterAvailability;
    /// What a deployment would do (redacted request previews, no side effects).
    async fn plan(&self, req: &DeployRequest) -> Result<DeployPlan, DeployError>;
    /// Deploys.
    async fn deploy(
        &self,
        req: &DeployRequest,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError>;
    /// Restores `version`.
    async fn rollback(
        &self,
        dep: &Deployment,
        version: &str,
        log: LogSink,
        cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError>;
}
