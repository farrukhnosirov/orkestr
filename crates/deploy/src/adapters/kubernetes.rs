//! Kubernetes API (base URL override `ORKESTR_KUBERNETES_API`; request-shape verified). **Owner: WP4.**

use async_trait::async_trait;
use orkestr_domain::{Deployment, DeploymentTarget};
use tokio_util::sync::CancellationToken;

use crate::{AdapterAvailability, DeployAdapter, DeployError, DeployOutcome, DeployPlan, DeployRequest, LogSink};

/// Kubernetes API (base URL override `ORKESTR_KUBERNETES_API`; request-shape verified).
#[derive(Debug, Default, Clone, Copy)]
pub struct KubernetesAdapter;

#[async_trait]
impl DeployAdapter for KubernetesAdapter {
    fn id(&self) -> &str {
        "kubernetes"
    }

    fn target(&self) -> DeploymentTarget {
        DeploymentTarget::Kubernetes
    }

    fn availability(&self) -> AdapterAvailability {
        AdapterAvailability::Unavailable { reason: "not implemented yet (unimplemented-feature)".into() }
    }

    async fn plan(&self, _req: &DeployRequest) -> Result<DeployPlan, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.kubernetes"))
    }

    async fn deploy(
        &self,
        _req: &DeployRequest,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.kubernetes"))
    }

    async fn rollback(
        &self,
        _dep: &Deployment,
        _version: &str,
        _log: LogSink,
        _cancel: CancellationToken,
    ) -> Result<DeployOutcome, DeployError> {
        Err(DeployError::Unimplemented("deploy.adapter.kubernetes"))
    }
}
