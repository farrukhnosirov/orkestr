//! Built-in adapters. **Owner: WP4.**

use std::path::Path;
use std::sync::Arc;

use crate::DeployAdapter;

pub mod cloud;
pub mod common;
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

/// The built-in adapters with local state under `data_dir`.
#[derive(Clone)]
pub struct Builtin {
    /// Static sites (served locally).
    pub static_site: Arc<StaticSiteAdapter>,
    /// Supervised local processes.
    pub local_process: Arc<LocalProcessAdapter>,
    /// Every adapter, in display order.
    pub all: Vec<Arc<dyn DeployAdapter>>,
}

impl std::fmt::Debug for Builtin {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_list().entries(self.all.iter().map(|a| a.id())).finish()
    }
}

/// Every built-in adapter.
#[must_use]
pub fn builtin(data_dir: &Path) -> Builtin {
    let static_site = Arc::new(StaticSiteAdapter::new(data_dir.join("static")));
    let local_process = Arc::new(LocalProcessAdapter::new(data_dir.join("process")));
    let all: Vec<Arc<dyn DeployAdapter>> = vec![
        Arc::clone(&static_site) as Arc<dyn DeployAdapter>,
        Arc::clone(&local_process) as Arc<dyn DeployAdapter>,
        Arc::new(DockerAdapter::new(data_dir.join("docker"))),
        Arc::new(VercelAdapter::default()),
        Arc::new(NetlifyAdapter::default()),
        Arc::new(FlyAdapter::default()),
        Arc::new(KubernetesAdapter::default()),
    ];
    Builtin { static_site, local_process, all }
}
