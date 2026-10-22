//! Supervised local process with a health URL. **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use tokio_util::sync::CancellationToken;

use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Supervised local process with a health URL.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalProcessAdapter;

#[async_trait]
impl DeployAdapter for LocalProcessAdapter {
    fn id(&self) -> &str {
        "localProcess"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::LocalProcess
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Unavailable { reason: "not implemented yet (unimplemented-feature)".into() }
    }

    async fn plan(&self, _req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.localProcess"))
    }

    async fn deploy(
        &self,
        _req: &DeployRequest,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.localProcess"))
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.localProcess"))
    }
}
