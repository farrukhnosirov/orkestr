//! Multi-node coordinator + worker plumbing (Phase 6, ADR-0033).
//!
//! Module ownership (Phase 6 work packages):
//! * WP-A: [`registry_store`] (SQLite registry + enrollments), [`connector`]
//!   (gateway core-to-core E2E links), [`heartbeat`] (pull heartbeat loop +
//!   health state machine), [`NodeHub`] (this file: links, enrollment,
//!   snapshots, worker-side state);
//! * WP-C: [`worker`] (worker-side job host), [`lease`] (epochs, fencing,
//!   expiry), [`coordinator_jobs`] (offer/accept, progress intake),
//!   [`migration`] (planned + unplanned migration, side-effect gate);
//! * shared test seam: [`loopback::LoopbackLink`] (a worker router in-process).
//!
//! Entry point for other packages: `Services.node_hub` ([`NodeHub`]):
//! * `node_hub.link(node_id).await` → `Arc<dyn NodeLink>` (cached; opens the
//!   gateway E2E session + `node.hello` on demand; `NodeError::Revoked` for
//!   revoked nodes);
//! * `node_hub.snapshots().await` → scheduler input (local node first);
//! * `node_hub.events()` → `(NodeId, Event)` from every worker (topic `jobs`);
//! * `node_hub.add_hook(..)` → lease renewals + health changes (coordinator);
//! * `node_hub.set_worker_host(..)` → leases / slots reported by this core
//!   when it is a worker;
//! * tests: `node_hub.insert_link(Arc::new(LoopbackLink::new(..)))`,
//!   `node_hub.set_connector(..)`.

pub mod connector;
pub mod coordinator_jobs;
pub mod heartbeat;
pub mod lease;
pub mod loopback;
pub mod migration;
pub mod registry_store;
pub mod worker;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use orkestr_domain::{Actor, AuditOutcome, DeviceId, JobId, Node, NodeId, NodeStatus, TrustLevel};
use orkestr_node::link::call_typed;
use orkestr_node::profile::{apply_profile, load_profile, profile_labels, EnvSource};
use orkestr_node::registry::{HealthConfig, NodeRegistry};
use orkestr_node::{NodeConnector, NodeError, NodeLink, NodeMonitor};
use orkestr_protocol::event::{EventKind, NodeOfflinePayload, NodeStatusPayload};
use orkestr_protocol::methods::nodes::{
    self as m, EnrollNodeParams, EnrollNodeResult, LeaseRenewal, LeaseState, NodeEnrollment, NodeHeartbeatParams,
    NodeHeartbeatResult, NodeHelloParams, NodeHelloResult, NodeProfile, NodeRegisteredPayload, NodeSlots, NodeSnapshot,
};
use orkestr_protocol::{Event, NewEvent, RpcError};
use orkestr_security::audit::NewAudit;
use parking_lot::{Mutex, RwLock};
use tokio::sync::broadcast;
use tokio_util::sync::CancellationToken;

pub use connector::{E2eLink, GatewayConnector};
pub use heartbeat::HeartbeatHook;
pub use loopback::LoopbackLink;
pub use registry_store::{SqlNodeRegistry, StoredEnrollment};

use crate::remote::client::{ClaimedPairing, ClientError, GatewayHttp};
use crate::remote::{e2e, pairing, RemoteService};
use crate::services::Services;

/// Node protocol version spoken by this core.
pub const NODE_PROTOCOL_VERSION: u32 = 1;
/// How long the first `nodes.enroll` call waits for the worker's approval
/// before answering "pending" with the SAS.
const ENROLL_FIRST_WAIT: Duration = Duration::from_secs(1);
/// Long-poll of later `nodes.enroll` calls for the same pairing.
const ENROLL_POLL_WAIT: Duration = Duration::from_secs(20);
/// Pending claims are forgotten after the pairing TTL.
const ENROLL_PENDING_TTL: Duration = Duration::from_secs(10 * 60);
/// Bound of the hardware inventory detection awaited by `node.hello`.
const DETECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Capacity of the merged worker event channel.
const EVENT_CAPACITY: usize = 4096;

/// Worker-side job state reported in `node.heartbeat` (WP-C implements it in
/// `nodes::worker`; without one the worker reports no leases).
#[async_trait]
pub trait WorkerHost: Send + Sync {
    /// Applies the coordinator's renewals / revocations; returns the leases held.
    async fn heartbeat(&self, coordinator: Option<DeviceId>, params: &NodeHeartbeatParams) -> Vec<LeaseState>;
    /// Job slots in use.
    fn slots_used(&self) -> u32;
}

/// Local model store as seen by the node protocol (WP-D's model store; the
/// integrator plugs `orkestr_node::models::{local_models, installed_digests}`
/// of `services.paths.data_dir` in with [`NodeHub::set_model_source`]).
pub trait LocalModelSource: Send + Sync {
    /// Installed model names (`name:tag`), merged into `Node.local_models`.
    fn model_names(&self) -> Vec<String>;
    /// Installed manifest digests (`NodeSnapshot.model_digests`).
    fn installed_digests(&self) -> Vec<orkestr_domain::Digest>;
}

struct LinkEntry {
    link: Arc<dyn NodeLink>,
    stop: CancellationToken,
}

struct PendingEnroll {
    core_id: String,
    claimed: ClaimedPairing,
    at: Instant,
}

/// Coordinator + worker node state of this core (`Services.node_hub`).
pub struct NodeHub {
    registry: Arc<SqlNodeRegistry>,
    monitor: Arc<NodeMonitor>,
    remote: Arc<RemoteService>,
    health: HealthConfig,
    profile: Option<NodeProfile>,
    connector: RwLock<Arc<dyn NodeConnector>>,
    links: Mutex<HashMap<NodeId, LinkEntry>>,
    connecting: tokio::sync::Mutex<()>,
    hooks: RwLock<Vec<Arc<dyn HeartbeatHook>>>,
    worker: RwLock<Option<Arc<dyn WorkerHost>>>,
    models: RwLock<Option<Arc<dyn LocalModelSource>>>,
    pending: Mutex<HashMap<String, PendingEnroll>>,
    events: broadcast::Sender<(NodeId, Event)>,
    seq: AtomicU64,
}

impl std::fmt::Debug for NodeHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NodeHub").field("health", &self.health).field("profile", &self.profile).finish_non_exhaustive()
    }
}

fn internal(e: impl std::fmt::Display) -> RpcError {
    RpcError::internal(e.to_string())
}

/// Maps a node error onto an RPC error for UI methods.
#[must_use]
pub fn rpc_error(e: NodeError) -> RpcError {
    match e {
        NodeError::NotFound(id) => RpcError::not_found(format!("node {id} not found")),
        NodeError::Revoked(id) => RpcError::conflict(format!("node {id} is revoked")),
        NodeError::Rpc(r) => r,
        NodeError::Unimplemented(f) => RpcError::unimplemented_feature(f),
        other => RpcError::internal(other.to_string()),
    }
}

/// Canonical bytes signed in a [`NodeEnrollment`] (the record with `sig = ""`).
#[must_use]
pub fn enrollment_signing_bytes(e: &NodeEnrollment) -> Vec<u8> {
    serde_json::to_vec(&NodeEnrollment { sig: String::new(), ..e.clone() }).unwrap_or_default()
}

impl NodeHub {
    /// Hub over the core database, the local monitor and the remote service.
    /// Reads `ORKESTR_NODE_PROFILE`, `ORKESTR_NODE_HEARTBEAT_MS`,
    /// `ORKESTR_NODE_LEASE_TTL_MS` from `env`.
    #[must_use]
    pub fn new(
        db: orkestr_storage::Db,
        monitor: Arc<NodeMonitor>,
        remote: Arc<RemoteService>,
        env: &EnvSource,
    ) -> Self {
        let registry = Arc::new(SqlNodeRegistry::new(db));
        let connector: Arc<dyn NodeConnector> =
            Arc::new(GatewayConnector::new(Arc::clone(&remote), Arc::clone(&registry)));
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        Self {
            registry,
            monitor,
            remote,
            health: HealthConfig::from_env(env),
            profile: load_profile(env),
            connector: RwLock::new(connector),
            links: Mutex::new(HashMap::new()),
            connecting: tokio::sync::Mutex::new(()),
            hooks: RwLock::new(Vec::new()),
            worker: RwLock::new(None),
            models: RwLock::new(None),
            pending: Mutex::new(HashMap::new()),
            events,
            seq: AtomicU64::new(0),
        }
    }

    /// The registry.
    #[must_use]
    pub fn registry(&self) -> &Arc<SqlNodeRegistry> {
        &self.registry
    }

    /// Health timings.
    #[must_use]
    pub fn health(&self) -> HealthConfig {
        self.health
    }

    /// The active `ORKESTR_NODE_PROFILE`, if any (tool shims for WP-C).
    #[must_use]
    pub fn profile(&self) -> Option<&NodeProfile> {
        self.profile.as_ref()
    }

    /// Replaces the connector (tests).
    pub fn set_connector(&self, c: Arc<dyn NodeConnector>) {
        *self.connector.write() = c;
    }

    /// Registers a coordinator-side heartbeat hook.
    pub fn add_hook(&self, hook: Arc<dyn HeartbeatHook>) {
        self.hooks.write().push(hook);
    }

    /// Sets the worker-side job host (leases + slots in `node.heartbeat`).
    pub fn set_worker_host(&self, host: Arc<dyn WorkerHost>) {
        *self.worker.write() = Some(host);
    }

    /// Sets the local model store view (WP-D).
    pub fn set_model_source(&self, src: Arc<dyn LocalModelSource>) {
        *self.models.write() = Some(src);
    }

    /// Digests of the locally installed models.
    #[must_use]
    pub fn local_model_digests(&self) -> Vec<orkestr_domain::Digest> {
        self.models.read().as_ref().map(|m| m.installed_digests()).unwrap_or_default()
    }

    /// Worker events (topic `jobs`) of every connected worker, tagged with the node.
    #[must_use]
    pub fn events(&self) -> broadcast::Receiver<(NodeId, Event)> {
        self.events.subscribe()
    }

    pub(crate) fn next_seq(&self) -> u64 {
        self.seq.fetch_add(1, Ordering::Relaxed) + 1
    }

    // ------------------------------------------------------------ local node

    /// This node with the profile applied (cheap; no detection).
    #[must_use]
    pub fn local_node(&self) -> Node {
        let n = self.monitor.local_node();
        let mut n = match &self.profile {
            Some(p) => apply_profile(n, p),
            None => n,
        };
        if let Some(src) = self.models.read().as_ref() {
            for name in src.model_names() {
                if !n.local_models.contains(&name) {
                    n.local_models.push(name);
                }
            }
        }
        n
    }

    /// [`Self::local_node`] after the hardware inventory (GPUs via
    /// `nvidia-smi`, runtimes, models) was detected once — cached; waits at
    /// most 10 s, detection continues in the background after that.
    pub async fn detected_local_node(&self) -> Node {
        if self.monitor.cached_info().is_none() {
            let monitor = Arc::clone(&self.monitor);
            let detect = tokio::task::spawn_blocking(move || {
                monitor.info(false);
            });
            if tokio::time::timeout(DETECT_TIMEOUT, detect).await.is_err() {
                tracing::warn!("node inventory detection is slow; answering with the quick description");
            }
        }
        self.local_node()
    }

    /// Maximum concurrent jobs of this node (profile, else cores / 4 in 1..=4).
    #[must_use]
    pub fn local_max_slots(&self) -> u32 {
        self.profile
            .as_ref()
            .and_then(|p| p.max_concurrent_jobs)
            .unwrap_or_else(|| (self.local_node().resources.cpu_cores / 4).clamp(1, 4))
    }

    /// Slots of this node.
    #[must_use]
    pub fn local_slots(&self) -> NodeSlots {
        let used = self.worker.read().as_ref().map_or(0, |w| w.slots_used());
        NodeSlots { max: self.local_max_slots(), used }
    }

    /// Labels of this node (profile).
    #[must_use]
    pub fn local_labels(&self) -> std::collections::BTreeMap<String, String> {
        profile_labels(self.profile.as_ref())
    }

    /// The local (coordinator) node as a snapshot.
    #[must_use]
    pub fn local_snapshot(&self) -> NodeSnapshot {
        let node = self.local_node();
        NodeSnapshot {
            status: node.status,
            last_heartbeat_at: node.last_heartbeat_at,
            node,
            trust_level: TrustLevel::Trusted,
            allowed_projects: vec!["*".into()],
            slots: self.local_slots(),
            leases: Vec::new(),
            is_coordinator: true,
            labels: self.local_labels(),
            model_digests: self.local_model_digests(),
        }
    }

    /// Every node (local first), the scheduler's input.
    ///
    /// # Errors
    /// Registry failure.
    pub async fn snapshots(&self) -> Result<Vec<NodeSnapshot>, NodeError> {
        let mut out = vec![self.local_snapshot()];
        out.extend(self.registry.list().await?);
        Ok(out)
    }

    /// Worker side of `node.hello`.
    pub async fn hello(&self, trust_level: TrustLevel) -> NodeHelloResult {
        NodeHelloResult {
            node: self.detected_local_node().await,
            trust_level,
            slots: self.local_slots(),
            labels: self.local_labels(),
            protocol_version: NODE_PROTOCOL_VERSION,
            model_digests: self.local_model_digests(),
        }
    }

    /// Worker side of `node.heartbeat`.
    pub async fn worker_heartbeat(
        &self,
        coordinator: Option<DeviceId>,
        p: &NodeHeartbeatParams,
    ) -> NodeHeartbeatResult {
        let host = self.worker.read().clone();
        let leases = match host {
            Some(h) => h.heartbeat(coordinator, p).await,
            None => Vec::new(),
        };
        let fresh = self.monitor.latest().filter(|s| chrono::Utc::now() - s.ts < chrono::TimeDelta::seconds(5));
        let sample = match fresh {
            Some(s) => Some(s),
            None => {
                let monitor = Arc::clone(&self.monitor);
                tokio::task::spawn_blocking(move || monitor.sample()).await.ok()
            }
        };
        NodeHeartbeatResult {
            node: self.local_node(),
            sample,
            slots: self.local_slots(),
            leases,
            model_digests: self.local_model_digests(),
        }
    }

    // ------------------------------------------------------------ links

    /// Link to an enrolled worker: the cached one, else connects (gateway E2E)
    /// and runs `node.hello`, which refreshes the node descriptor.
    ///
    /// # Errors
    /// `NotFound` (not enrolled), `Revoked`, `Transport`, `Rpc`.
    pub async fn link(&self, id: NodeId) -> Result<Arc<dyn NodeLink>, NodeError> {
        if let Some(l) = self.cached_link(id) {
            return Ok(l);
        }
        let _guard = self.connecting.lock().await;
        if let Some(l) = self.cached_link(id) {
            return Ok(l);
        }
        let stored = self.registry.stored(id).await?.ok_or(NodeError::NotFound(id))?;
        if stored.enrollment.revoked_at.is_some() {
            return Err(NodeError::Revoked(id));
        }
        let connector = self.connector.read().clone();
        let link = connector.connect(&stored.enrollment).await?;
        let hello = call_typed::<m::Hello>(
            link.as_ref(),
            &NodeHelloParams { coordinator_node_id: self.monitor.node_id(), protocol_version: NODE_PROTOCOL_VERSION },
        )
        .await?;
        if hello.node.id != id {
            return Err(NodeError::Internal(format!("worker answered as node {} instead of {id}", hello.node.id)));
        }
        self.apply_hello(&stored, hello).await?;
        self.insert_link(Arc::clone(&link));
        Ok(link)
    }

    fn cached_link(&self, id: NodeId) -> Option<Arc<dyn NodeLink>> {
        self.links.lock().get(&id).map(|e| Arc::clone(&e.link))
    }

    /// Installs a link (replacing any previous one) and forwards its events.
    /// Tests use it to plug a `LoopbackLink`.
    pub fn insert_link(&self, link: Arc<dyn NodeLink>) {
        let id = link.node_id();
        let stop = CancellationToken::new();
        let mut rx = link.events();
        let tx = self.events.clone();
        let s = stop.clone();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    () = s.cancelled() => break,
                    r = rx.recv() => match r {
                        Ok(ev) => { let _ = tx.send((id, ev)); }
                        Err(broadcast::error::RecvError::Lagged(n)) => tracing::warn!(%id, n, "node events lagged"),
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        });
        if let Some(old) = self.links.lock().insert(id, LinkEntry { link, stop }) {
            old.stop.cancel();
        }
    }

    /// Drops a link (closes a gateway session); the next `link()` reconnects.
    pub fn drop_link(&self, id: NodeId) {
        if let Some(old) = self.links.lock().remove(&id) {
            old.stop.cancel();
        }
    }

    async fn apply_hello(&self, stored: &StoredEnrollment, hello: NodeHelloResult) -> Result<NodeSnapshot, NodeError> {
        let id = stored.enrollment.node_id;
        let mut node = hello.node;
        node.is_local = false;
        node.last_heartbeat_at = Some(chrono::Utc::now());
        let granted = hello.trust_level;
        let max = stored.max_concurrent_jobs.map_or(hello.slots.max, |o| o.min(hello.slots.max));
        let snap = match self.registry.get(id).await? {
            Some(mut s) => {
                s.node = node;
                s.trust_level = s.trust_level.min(granted);
                s.slots = NodeSlots { max, used: hello.slots.used };
                s.labels = hello.labels;
                s.model_digests = hello.model_digests;
                s.last_heartbeat_at = s.node.last_heartbeat_at;
                if !matches!(s.status, NodeStatus::Draining | NodeStatus::Revoked) {
                    s.status = NodeStatus::Online;
                }
                s.node.status = s.status;
                s
            }
            None => NodeSnapshot {
                last_heartbeat_at: node.last_heartbeat_at,
                node,
                status: NodeStatus::Online,
                trust_level: granted.min(stored.enrollment.trust_level),
                allowed_projects: stored.enrollment.allowed_projects.clone(),
                slots: NodeSlots { max, used: hello.slots.used },
                leases: Vec::new(),
                is_coordinator: false,
                labels: hello.labels,
                model_digests: hello.model_digests,
            },
        };
        self.registry.put_snapshot(snap.clone()).await?;
        Ok(snap)
    }

    // ------------------------------------------------------------ hooks

    pub(crate) async fn lease_params(&self, node: NodeId) -> (Vec<LeaseRenewal>, Vec<JobId>) {
        let hooks = self.hooks.read().clone();
        let mut renew = Vec::new();
        let mut revoke = Vec::new();
        for h in hooks {
            let (r, v) = h.lease_params(node).await;
            renew.extend(r);
            revoke.extend(v);
        }
        (renew, revoke)
    }

    pub(crate) async fn on_heartbeat(&self, node: NodeId, hb: &NodeHeartbeatResult) {
        let hooks = self.hooks.read().clone();
        for h in hooks {
            h.on_heartbeat(node, hb).await;
        }
    }

    /// Publishes a health change (`NodeStatus`, `NodeOffline`) and tells the hooks.
    pub(crate) async fn health_changed(&self, services: &Services, node: NodeId, from: NodeStatus, to: NodeStatus) {
        tracing::info!(%node, ?from, ?to, "node health changed");
        if let Ok(Some(s)) = self.registry.get(node).await {
            let _ = services
                .events
                .publish(NewEvent::new(EventKind::NodeStatus(NodeStatusPayload { node: s.node.clone(), sample: None })))
                .await;
            if to == NodeStatus::Offline {
                let _ = services
                    .events
                    .publish(NewEvent::new(EventKind::NodeOffline(NodeOfflinePayload {
                        node_id: node,
                        last_seen_at: s.last_heartbeat_at,
                    })))
                    .await;
            }
        }
        let hooks = self.hooks.read().clone();
        for h in hooks {
            h.on_health(node, from, to).await;
        }
    }

    // ------------------------------------------------------------ enrollment

    fn sign(&self, mut e: NodeEnrollment) -> Result<NodeEnrollment, RpcError> {
        let cik = self.remote.cik().ok_or_else(|| RpcError::conflict("no core identity key"))?;
        let sig: p256::ecdsa::Signature = p256::ecdsa::signature::Signer::sign(cik, &enrollment_signing_bytes(&e));
        e.sig = e2e::b64(&sig.to_bytes());
        Ok(e)
    }

    /// Verifies an enrollment signature against this core's CIK.
    #[must_use]
    pub fn verify_enrollment(&self, e: &NodeEnrollment) -> bool {
        let Some(cik) = self.remote.cik() else { return false };
        let msg = enrollment_signing_bytes(e);
        e2e::verify(cik.verifying_key(), &msg, &e.sig).is_ok()
    }

    /// `nodes.enroll`: claims the worker's `kind: "node"` pairing with this
    /// core's CIK, waits for the worker's approval (SAS compared on both
    /// screens), connects, runs `node.hello`, stores a CIK-signed enrollment
    /// and publishes `NodeRegistered`.
    ///
    /// While the worker has not approved yet, answers `conflict` with
    /// `data: {pendingApproval: true, sas, pairingId, coreId}`; call again with
    /// the same params to finish (long-poll up to 20 s).
    ///
    /// # Errors
    /// Remote access not enrolled, wrong code (`forbidden`), unknown pairing /
    /// core (`not_found`), pending approval (`conflict`), transport failures.
    pub async fn enroll(
        &self,
        services: &Services,
        actor: Actor,
        p: EnrollNodeParams,
    ) -> Result<EnrollNodeResult, RpcError> {
        let (gateway, token) = self
            .remote
            .sync_credentials()
            .ok_or_else(|| RpcError::conflict("enable remote access (remote.enable) before enrolling nodes"))?;
        let cik = self.remote.cik().ok_or_else(|| RpcError::conflict("no core identity key"))?.clone();
        let http = GatewayHttp::new(&gateway, &token);
        let existing = {
            let mut pending = self.pending.lock();
            pending.retain(|_, e| e.at.elapsed() < ENROLL_PENDING_TTL);
            pending.get(&p.pairing_id).map(|e| (e.core_id.clone(), e.claimed.clone()))
        };
        let (claimed, wait) = match existing {
            Some((core_id, claimed)) => {
                if core_id != p.core_id {
                    return Err(RpcError::bad_request("pairing belongs to another core"));
                }
                (claimed, ENROLL_POLL_WAIT)
            }
            None => {
                let name = p.name.clone().unwrap_or_else(|| self.local_node().name);
                let claimed = http
                    .claim(&p.core_id, &p.pairing_id, &p.code, &cik, &name, pairing::NODE_PLATFORM)
                    .await
                    .map_err(|e| {
                        let err = claim_error(e);
                        tracing::warn!(pairing = %p.pairing_id, error = %err.message, "node enrollment claim rejected");
                        err
                    });
                let claimed = match claimed {
                    Ok(c) => c,
                    Err(e) => {
                        self.audit(
                            services,
                            actor,
                            "nodes.enroll",
                            &p.core_id,
                            AuditOutcome::Denied,
                            serde_json::json!({
                                "pairingId": p.pairing_id, "reason": e.message,
                            }),
                        )
                        .await;
                        return Err(e);
                    }
                };
                self.pending.lock().insert(
                    p.pairing_id.clone(),
                    PendingEnroll { core_id: p.core_id.clone(), claimed: claimed.clone(), at: Instant::now() },
                );
                (claimed, ENROLL_FIRST_WAIT)
            }
        };
        // wait for the worker's approval (completion signature verified)
        let started = Instant::now();
        loop {
            match http.approval(&p.pairing_id, &claimed, &cik).await {
                Ok(Some(_role)) => break,
                Ok(None) => {}
                Err(e) => {
                    self.pending.lock().remove(&p.pairing_id);
                    return Err(claim_error(e));
                }
            }
            if started.elapsed() >= wait {
                return Err(RpcError::conflict(format!(
                    "waiting for approval on the worker; compare the code {} on both screens",
                    claimed.sas
                ))
                .with_data(serde_json::json!({
                    "pendingApproval": true, "sas": claimed.sas, "pairingId": p.pairing_id, "coreId": p.core_id,
                })));
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.pending.lock().remove(&p.pairing_id);
        // connect + hello
        let gc = GatewayConnector::new(Arc::clone(&self.remote), Arc::clone(&self.registry));
        let client = gc
            .open(&p.core_id, &claimed.device_id, &claimed.core_public_key)
            .await
            .map_err(|e| RpcError::conflict(format!("approved, but the worker core is unreachable: {e}")))?;
        let hello: NodeHelloResult = client
            .call_typed::<m::Hello>(&NodeHelloParams {
                coordinator_node_id: self.monitor.node_id(),
                protocol_version: NODE_PROTOCOL_VERSION,
            })
            .await
            .map_err(|e| match e {
                ClientError::Rpc(r) => r,
                other => RpcError::conflict(other.to_string()),
            })?;
        let node_id = hello.node.id;
        if node_id == self.monitor.node_id() {
            return Err(RpcError::bad_request("a core cannot enroll itself"));
        }
        let enrollment = self.sign(NodeEnrollment {
            node_id,
            core_id: p.core_id.clone(),
            name: p.name.clone().unwrap_or_else(|| hello.node.name.clone()),
            public_key: e2e::b64(&claimed.core_public_key),
            trust_level: hello.trust_level,
            allowed_projects: vec!["*".into()],
            enrolled_at: chrono::Utc::now(),
            revoked_at: None,
            sig: String::new(),
        })?;
        let stored = StoredEnrollment {
            enrollment: enrollment.clone(),
            device_id: claimed.device_id.clone(),
            max_concurrent_jobs: None,
        };
        self.registry.put_stored(stored.clone()).await.map_err(internal)?;
        // a re-enrollment starts from a clean snapshot
        if let Ok(Some(mut old)) = self.registry.get(node_id).await {
            old.status = NodeStatus::Online;
            old.trust_level = enrollment.trust_level;
            self.registry.put_snapshot(old).await.map_err(internal)?;
        }
        let mut snap = self.apply_hello(&stored, hello).await.map_err(internal)?;
        if let Some(name) = &p.name {
            snap.node.name.clone_from(name);
            self.registry.put_snapshot(snap.clone()).await.map_err(internal)?;
        }
        self.insert_link(Arc::new(E2eLink::new(node_id, client)));
        self.audit(
            services,
            actor,
            "nodes.enroll",
            &node_id.to_string(),
            AuditOutcome::Allowed,
            serde_json::json!({
                "coreId": p.core_id, "trustLevel": enrollment.trust_level, "deviceId": claimed.device_id,
            }),
        )
        .await;
        let _ = services
            .events
            .publish(NewEvent::new(EventKind::NodeRegistered(NodeRegisteredPayload { node: Box::new(snap) })))
            .await;
        Ok(EnrollNodeResult { enrollment, sas: claimed.sas })
    }

    /// Marks a node revoked (explicit `nodes.revoke` or the worker refused us).
    ///
    /// # Errors
    /// Unknown node, storage failure.
    pub async fn mark_revoked(&self, services: &Services, id: NodeId, reason: &str) -> Result<NodeSnapshot, RpcError> {
        let mut stored =
            self.registry.stored(id).await.map_err(internal)?.ok_or_else(|| rpc_error(NodeError::NotFound(id)))?;
        if stored.enrollment.revoked_at.is_none() {
            stored.enrollment.revoked_at = Some(chrono::Utc::now());
            stored.enrollment = self.sign(stored.enrollment)?;
            self.registry.put_stored(stored).await.map_err(internal)?;
        }
        self.drop_link(id);
        let before = self.registry.get(id).await.map_err(internal)?.map(|s| s.status);
        let snap = self.registry.set_status(id, NodeStatus::Revoked).await.map_err(rpc_error)?;
        if let Some(from) = before.filter(|f| *f != NodeStatus::Revoked) {
            self.health_changed(services, id, from, NodeStatus::Revoked).await;
        }
        self.audit(
            services,
            Actor::System,
            "nodes.revoked",
            &id.to_string(),
            AuditOutcome::Allowed,
            serde_json::json!({
                "reason": reason,
            }),
        )
        .await;
        Ok(snap)
    }

    /// `nodes.drain`: `Draining` (excluded from placement; WP-C migrates its
    /// jobs via the health hook) or back to `Online`.
    ///
    /// # Errors
    /// Unknown or revoked node.
    pub async fn set_draining(&self, services: &Services, id: NodeId, drain: bool) -> Result<NodeSnapshot, RpcError> {
        let cur = self.registry.get(id).await.map_err(internal)?.ok_or_else(|| rpc_error(NodeError::NotFound(id)))?;
        if cur.status == NodeStatus::Revoked {
            return Err(rpc_error(NodeError::Revoked(id)));
        }
        let to = if drain { NodeStatus::Draining } else { NodeStatus::Online };
        let snap = self.registry.set_status(id, to).await.map_err(rpc_error)?;
        if cur.status != to {
            self.health_changed(services, id, cur.status, to).await;
        }
        Ok(snap)
    }

    /// `nodes.setPolicy`.
    ///
    /// # Errors
    /// Unknown node; trust above what the worker granted.
    pub async fn set_policy(&self, p: m::SetNodePolicyParams) -> Result<NodeSnapshot, RpcError> {
        let id = p.node_id;
        let mut stored =
            self.registry.stored(id).await.map_err(internal)?.ok_or_else(|| rpc_error(NodeError::NotFound(id)))?;
        if let Some(t) = p.trust_level {
            if t > stored.enrollment.trust_level {
                return Err(RpcError::bad_request(format!(
                    "the worker granted trust level {:?}; a higher level must be granted on the worker",
                    stored.enrollment.trust_level
                )));
            }
        }
        if let Some(max) = p.max_concurrent_jobs {
            stored.max_concurrent_jobs = Some(max);
        }
        if let Some(ap) = &p.allowed_projects {
            stored.enrollment.allowed_projects.clone_from(ap);
            stored.enrollment = self.sign(stored.enrollment)?;
        }
        self.registry.put_stored(stored).await.map_err(internal)?;
        self.registry
            .update_snapshot(id, move |s| {
                if let Some(t) = p.trust_level {
                    s.trust_level = t;
                }
                if let Some(ap) = p.allowed_projects {
                    s.allowed_projects = ap;
                }
                if let Some(max) = p.max_concurrent_jobs {
                    s.slots.max = max;
                }
            })
            .await
            .map_err(rpc_error)
    }

    async fn audit(
        &self,
        services: &Services,
        actor: Actor,
        action: &str,
        resource: &str,
        outcome: AuditOutcome,
        details: serde_json::Value,
    ) {
        let mut entry = NewAudit::new(actor, action, resource.to_owned(), outcome);
        entry.details = details;
        services.security.audit.record(entry).await;
    }
}

fn claim_error(e: ClientError) -> RpcError {
    match e {
        ClientError::Gateway { status: 403, message } => {
            RpcError::forbidden(format!("the worker rejected the pairing: {message}"))
        }
        ClientError::Gateway { status: 404, message } => RpcError::not_found(message),
        ClientError::Refused(code) => RpcError::forbidden(format!("pairing {code}")),
        ClientError::Rpc(r) => r,
        other => RpcError::conflict(other.to_string()),
    }
}
