//! Docker (available only when `docker` is on PATH). **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use tokio_util::sync::CancellationToken;

use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Docker (available only when `docker` is on PATH).
#[derive(Debug, Default, Clone, Copy)]
pub struct DockerAdapter;

#[async_trait]
impl DeployAdapter for DockerAdapter {
    fn id(&self) -> &str {
        "docker"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Docker
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Unavailable { reason: "not implemented yet (unimplemented-feature)".into() }
    }

    async fn plan(&self, _req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.docker"))
    }

    async fn deploy(
        &self,
        _req: &DeployRequest,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.docker"))
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.docker"))
    }
}
