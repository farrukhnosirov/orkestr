//! Built-in adapters. **Owner: WP4.**

use std::sync::Arc;

use crate::DeployAdapter;

pub mod docker;
pub mod fly;
pub mod kubernetes;
pub mod local_process;
pub mod netlify;
pub mod static_site;
pub mod vercel;

pub use docker::DockerAdapter;
pub use fly::FlyAdapter;
pub use kubernetes::KubernetesAdapter;
pub use local_process::LocalProcessAdapter;
pub use netlify::NetlifyAdapter;
pub use static_site::StaticSiteAdapter;
pub use vercel::VercelAdapter;

/// Every built-in adapter.
#[must_use]
pub fn builtin() -> Vec<Arc<dyn DeployAdapter>> {
    vec![
        Arc::new(StaticSiteAdapter),
        Arc::new(LocalProcessAdapter),
        Arc::new(DockerAdapter),
        Arc::new(VercelAdapter),
        Arc::new(NetlifyAdapter),
        Arc::new(FlyAdapter),
        Arc::new(KubernetesAdapter),
    ]
}
