//! Phase 6 integration: plugs the work packages into one another at core
//! start ([`wire`]).
//!
//! * WP-A [`NodeHub`] ← WP-D model store ([`DataDirModels`] as
//!   `LocalModelSource`) and WP-C worker host ([`HubWorker`] as `WorkerHost`:
//!   lease renew / revoke inside `node.heartbeat`, slots);
//! * WP-C coordinator ([`CoordinatorJobs`]) over [`HubLinks`] (hub links for
//!   workers, an in-process [`LocalLink`] for the coordinator's own worker
//!   host), WP-B `DefaultNodeScheduler`, WP-D blob store + RPC blob transfer,
//!   CIK policy signer; [`CoordinatorHook`] feeds the hub's heartbeat loop
//!   (lease renewals, reconciliation, migration on drain / node loss, cancel
//!   on revoke);
//! * WP-B `jobs.submit` hooks: hub snapshots + a dispatcher resolving the
//!   installed coordinator;
//! * WP-D `models.distribute` links ([`HubLinks`]).
//!
//! Collaborators are resolved through the per-core registries
//! (`worker::host`, `coordinator_jobs::coordinator`) at call time, so tests
//! that install their own host / coordinator stay consistent.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Weak};

use async_trait::async_trait;
use orkestr_domain::{CorrelationId, DeviceId, JobId, NodeId, NodeStatus, SessionId};
use orkestr_node::scheduling::NodeScheduler;
use orkestr_node::{NodeError, NodeLink};
use orkestr_protocol::methods::jobs::{PlacementPolicy, PolicySnapshot};
use orkestr_protocol::methods::nodes::{
    LeaseRenewal, LeaseState, NodeHeartbeatParams, NodeHeartbeatResult, NodeSnapshot,
};
use orkestr_protocol::Event;
use orkestr_task_engine::jobs::JobDispatcher;
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

use super::coordinator_jobs::{
    self, CoordinatorConfig, CoordinatorDeps, CoordinatorJobs, RpcBlobTransfer, SchedulerPlacer,
};
use super::lease::{LeaseConfig, PolicySigner};
use super::{worker, HeartbeatHook, LocalModelSource, NodeHub, WorkerHost};
use crate::rpc::jobs_ui::{install_hooks, JobsUiHooks, NodeSnapshotSource};
use crate::rpc::{Principal, RpcContext, RpcRouter};
use crate::services::Services;

/// WP-D's model store of a data dir as the hub's [`LocalModelSource`].
#[derive(Debug, Clone)]
pub struct DataDirModels(pub PathBuf);

impl LocalModelSource for DataDirModels {
    fn model_names(&self) -> Vec<String> {
        orkestr_node::models::local_models(&self.0).into_iter().map(|m| m.name).collect()
    }

    fn installed_digests(&self) -> Vec<orkestr_domain::Digest> {
        orkestr_node::models::installed_digests(&self.0)
    }
}

/// The core's WP-C worker host as the hub's [`WorkerHost`].
pub struct HubWorker(pub Weak<Services>);

#[async_trait]
impl WorkerHost for HubWorker {
    async fn heartbeat(&self, coordinator: Option<DeviceId>, params: &NodeHeartbeatParams) -> Vec<LeaseState> {
        let Some(s) = self.0.upgrade() else { return Vec::new() };
        let host = worker::host(&s);
        // a coordinator only sees / revokes its own jobs (this core may serve
        // several coordinators and run its own jobs as a coordinator)
        let coord = coordinator.and_then(|d| host.coordinator_of_device(d));
        host.revoke_of(coord, &params.revoke);
        let _ = host.renew(&params.renew);
        host.leases_of(coord)
    }

    fn slots_used(&self) -> u32 {
        self.0.upgrade().map_or(0, |s| u32::try_from(worker::host(&s).active_jobs()).unwrap_or(u32::MAX))
    }
}

/// In-process link from the coordinator to its own worker host (jobs placed
/// on the coordinator node): calls run through the router as `Local`; events
/// are the host's own job events (never the coordinator's relays).
pub struct LocalLink {
    services: Arc<Services>,
    router: Arc<RpcRouter>,
    session_id: SessionId,
}

impl LocalLink {
    /// Link over `router` of the core behind `services`.
    #[must_use]
    pub fn new(services: Arc<Services>, router: Arc<RpcRouter>) -> Self {
        Self { services, router, session_id: SessionId::new() }
    }
}

#[async_trait]
impl NodeLink for LocalLink {
    fn node_id(&self) -> NodeId {
        self.services.node_id
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, NodeError> {
        let ctx = RpcContext {
            services: Arc::clone(&self.services),
            session_id: self.session_id,
            correlation_id: CorrelationId::new(),
            principal: Principal::Local,
        };
        Ok(self.router.dispatch(ctx, method, params).await?)
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        worker::host(&self.services).subscribe()
    }
}

/// Node links + snapshots over the [`NodeHub`] (WP-C coordinator, WP-D
/// `models.distribute`, WP-B placement input).
pub struct HubLinks {
    services: Weak<Services>,
    hub: Arc<NodeHub>,
    own: NodeId,
    local: Arc<dyn NodeLink>,
}

impl HubLinks {
    /// Links of the core behind `services` (`router` serves its own jobs).
    #[must_use]
    pub fn new(services: &Arc<Services>, router: Arc<RpcRouter>) -> Self {
        Self {
            services: Arc::downgrade(services),
            hub: Arc::clone(&services.node_hub),
            own: services.node_id,
            local: Arc::new(LocalLink::new(Arc::clone(services), router)),
        }
    }

    /// Hub snapshots; slot usage also counts the jobs this coordinator
    /// already assigned (heartbeats report them only up to one interval later).
    async fn all_snapshots(&self) -> Vec<NodeSnapshot> {
        let mut snaps = match self.hub.snapshots().await {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "node registry unavailable; placing on the local node only");
                vec![self.hub.local_snapshot()]
            }
        };
        if let Some(s) = self.services.upgrade() {
            if let Ok(active) = crate::rpc::jobs_ui::job_store(&s).active_per_node().await {
                for snap in &mut snaps {
                    if let Some(n) = active.get(&snap.node.id) {
                        snap.slots.used = snap.slots.used.max(*n);
                    }
                }
            }
        }
        snaps
    }
}

#[async_trait]
impl coordinator_jobs::LinkProvider for HubLinks {
    async fn link(&self, node: NodeId) -> Option<Arc<dyn NodeLink>> {
        if node == self.own {
            return Some(Arc::clone(&self.local));
        }
        match self.hub.link(node).await {
            Ok(l) => Some(l),
            Err(e) => {
                tracing::debug!(%node, error = %e, "no link to node");
                None
            }
        }
    }

    async fn snapshots(&self) -> Vec<NodeSnapshot> {
        self.all_snapshots().await
    }

    async fn node_ids(&self) -> Vec<NodeId> {
        self.all_snapshots()
            .await
            .into_iter()
            .filter(|s| s.node.id != self.own && s.status != NodeStatus::Revoked)
            .map(|s| s.node.id)
            .collect()
    }
}

#[async_trait]
impl crate::rpc::models::LinkProvider for HubLinks {
    async fn link(&self, node_id: NodeId) -> Result<Arc<dyn NodeLink>, NodeError> {
        self.hub.link(node_id).await
    }
}

#[async_trait]
impl NodeSnapshotSource for HubLinks {
    async fn snapshots(&self) -> Vec<NodeSnapshot> {
        self.all_snapshots().await
    }
}

/// `jobs.submit` dispatcher resolving the coordinator installed for `node`.
pub struct InstalledDispatcher(pub NodeId);

impl JobDispatcher for InstalledDispatcher {
    fn dispatch(&self, job_id: JobId) {
        match coordinator_jobs::coordinator(self.0) {
            Some(c) => c.dispatcher().dispatch(job_id),
            None => tracing::warn!(job_id = %job_id, "no jobs coordinator installed; job stays offered"),
        }
    }
}

type SentLeases = (Vec<LeaseRenewal>, Vec<JobId>);

/// Coordinator side of the hub's heartbeat loop (WP-A ↔ WP-C).
pub struct CoordinatorHook {
    own: NodeId,
    sent: Mutex<HashMap<NodeId, SentLeases>>,
}

impl CoordinatorHook {
    /// Hook of the coordinator installed for `own`.
    #[must_use]
    pub fn new(own: NodeId) -> Self {
        Self { own, sent: Mutex::new(HashMap::new()) }
    }
}

#[async_trait]
impl HeartbeatHook for CoordinatorHook {
    async fn lease_params(&self, node: NodeId) -> (Vec<LeaseRenewal>, Vec<JobId>) {
        let Some(c) = coordinator_jobs::coordinator(self.own) else { return (Vec::new(), Vec::new()) };
        let p = c.heartbeat_params(node);
        self.sent.lock().insert(node, p.clone());
        p
    }

    async fn on_heartbeat(&self, node: NodeId, result: &NodeHeartbeatResult) {
        let Some(c) = coordinator_jobs::coordinator(self.own) else { return };
        let (renew, revoke) = self.sent.lock().remove(&node).unwrap_or_default();
        c.heartbeat_ok(node, &renew, &revoke, &result.leases);
        c.fence_reported(node, &result.leases).await;
    }

    async fn on_health(&self, node: NodeId, from: NodeStatus, to: NodeStatus) {
        let Some(c) = coordinator_jobs::coordinator(self.own) else { return };
        let held = c.leases.held_by(node);
        if held.is_empty() {
            return;
        }
        tracing::info!(%node, ?from, ?to, jobs = held.len(), "node health changed with jobs on it");
        for (job, lease) in held {
            let c = Arc::clone(&c);
            match to {
                // node lost: unplanned migration (side-effect gate applies)
                NodeStatus::Offline => {
                    tokio::spawn(async move { c.recover_unplanned(job, lease, "node_lost: offline").await });
                }
                // revoked: its output is no longer trusted — cancel, never resume from it
                NodeStatus::Revoked => {
                    tokio::spawn(async move {
                        if let Err(e) = c.cancel(job, true).await {
                            tracing::warn!(job_id = %job, error = %e, "cancel after revoke failed");
                        }
                    });
                }
                // drain: planned migration off the node
                NodeStatus::Draining => {
                    tokio::spawn(async move {
                        if let Err(e) = c.migrate(job, None).await {
                            tracing::warn!(job_id = %job, error = %e, "migration off a draining node failed");
                        }
                    });
                }
                _ => {}
            }
        }
    }
}

/// Wires the Phase 6 packages of a started core (call once, after the remote
/// service loaded the CIK). Without a CIK the coordinator part is skipped
/// (no signed policy snapshots → no offers); the worker part always runs.
pub fn wire(services: &Arc<Services>, router: Arc<RpcRouter>, shutdown: &CancellationToken) {
    let hub = &services.node_hub;
    let own = services.node_id;
    hub.set_model_source(Arc::new(DataDirModels(services.paths.data_dir.clone())));
    hub.set_worker_host(Arc::new(HubWorker(Arc::downgrade(services))));
    let host = worker::host(services);

    let links = Arc::new(HubLinks::new(services, router));
    if let Err(e) = crate::rpc::models::set_link_provider(services, links.clone()) {
        tracing::warn!(error = %e.message, "models.distribute links not wired");
    }

    let Some(cik) = services.remote.cik().cloned() else {
        tracing::warn!("no core identity key: this core cannot coordinate worker nodes");
        return;
    };
    let blobs = match crate::rpc::models::blob_store(services) {
        Ok(b) => b,
        Err(e) => {
            tracing::error!(error = %e.message, "blob store unavailable: jobs coordinator disabled");
            return;
        }
    };
    let signer = PolicySigner::new(cik);
    // the coordinator's own worker host accepts the coordinator's offers
    host.trust_coordinator(own, signer.verifying_key());
    let policy = PlacementPolicy { coordinator_node_id: Some(own), ..PlacementPolicy::default() };
    let lease = LeaseConfig::from_env(&orkestr_node::EnvSource::process());
    let scheduler: Arc<dyn NodeScheduler> = Arc::new(orkestr_task_engine::node_scheduler::DefaultNodeScheduler);
    let coord = CoordinatorJobs::new(CoordinatorDeps {
        services: Arc::clone(services),
        cfg: CoordinatorConfig {
            node_id: own,
            lease,
            policy: PolicySnapshot { approval_mode: "ask".into(), ..PolicySnapshot::default() },
        },
        links: links.clone(),
        placer: Arc::new(SchedulerPlacer { scheduler, links: links.clone(), policy }),
        transfer: Arc::new(RpcBlobTransfer),
        blobs,
        signer,
    });
    coordinator_jobs::install(own, Arc::clone(&coord));
    coord.spawn_hub_intake(hub.events(), shutdown.clone());
    coord.spawn_lease_loop(shutdown.clone());
    hub.add_hook(Arc::new(CoordinatorHook::new(own)));
    install_hooks(
        own,
        JobsUiHooks { snapshots: Some(links), dispatcher: Some(Arc::new(InstalledDispatcher(own))), weights: None },
    );
    spawn_local_leases(Arc::downgrade(services), lease.heartbeat, shutdown.clone());
}

/// Renews the leases of jobs running on the coordinator's own worker host
/// (the hub's heartbeat loop only visits enrolled workers).
fn spawn_local_leases(services: Weak<Services>, every: std::time::Duration, shutdown: CancellationToken) {
    tokio::spawn(async move {
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep(every) => {}
            }
            let Some(s) = services.upgrade() else { break };
            let Some(c) = coordinator_jobs::coordinator(s.node_id) else { continue };
            let (renew, revoke) = c.heartbeat_params(s.node_id);
            let host = worker::host(&s);
            host.revoke_of(Some(s.node_id), &revoke);
            let _ = host.renew(&renew);
            let states = host.leases_of(Some(s.node_id));
            c.heartbeat_ok(s.node_id, &renew, &revoke, &states);
            c.fence_reported(s.node_id, &states).await;
        }
    });
}
