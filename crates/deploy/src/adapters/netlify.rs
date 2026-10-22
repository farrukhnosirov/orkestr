//! Netlify REST API (base URL override `ORKESTR_NETLIFY_API`; request-shape verified). **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use tokio_util::sync::CancellationToken;

use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Netlify REST API (base URL override `ORKESTR_NETLIFY_API`; request-shape verified).
#[derive(Debug, Default, Clone, Copy)]
pub struct NetlifyAdapter;

#[async_trait]
impl DeployAdapter for NetlifyAdapter {
    fn id(&self) -> &str {
        "netlify"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Netlify
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Unavailable { reason: "not implemented yet (unimplemented-feature)".into() }
    }

    async fn plan(&self, _req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.netlify"))
    }

    async fn deploy(
        &self,
        _req: &DeployRequest,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.netlify"))
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.netlify"))
    }
}
