//! In-process [`NodeLink`] to a worker core's router — the test seam for the
//! scheduler, job execution, migration and blob transfer (Phase 6).
//!
//! Calls go through [`RpcRouter::dispatch`] (so remote authorization applies
//! when the principal is `Remote`); worker events on topic `jobs` of the
//! worker's bus are forwarded to [`NodeLink::events`].

use std::sync::Arc;

use async_trait::async_trait;
use orkestr_domain::{CorrelationId, NodeId, SessionId};
use orkestr_events::Received;
use orkestr_node::{NodeError, NodeLink};
use orkestr_protocol::{topic, Event};
use tokio::sync::broadcast;

use crate::rpc::{Principal, RpcContext, RpcRouter};
use crate::services::Services;

/// Capacity of the forwarded event channel.
const EVENT_CAPACITY: usize = 1024;

/// A worker core reached in-process.
pub struct LoopbackLink {
    node_id: NodeId,
    router: Arc<RpcRouter>,
    services: Arc<Services>,
    principal: Principal,
    session_id: SessionId,
    events: broadcast::Sender<Event>,
}

impl std::fmt::Debug for LoopbackLink {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoopbackLink").field("node_id", &self.node_id).finish_non_exhaustive()
    }
}

impl LoopbackLink {
    /// Wraps the worker's `router` + `services`. Calls run as `principal`
    /// (`Principal::Local` skips remote authorization; pass a coordinator
    /// `Principal::Remote` to exercise it). Spawns the event forwarder, so it
    /// must be called inside a Tokio runtime.
    #[must_use]
    pub fn new(node_id: NodeId, router: Arc<RpcRouter>, services: Arc<Services>, principal: Principal) -> Self {
        let (events, _) = broadcast::channel(EVENT_CAPACITY);
        if let Ok(mut sub) = services.events.subscribe([topic::JOBS]) {
            let tx = events.clone();
            tokio::spawn(async move {
                while let Some(r) = sub.recv().await {
                    if let Received::Event(ev) = r {
                        let _ = tx.send((*ev).clone());
                    }
                }
            });
        }
        Self { node_id, router, services, principal, session_id: SessionId::new(), events }
    }

    /// Sender of the forwarded events (tests may inject events directly).
    #[must_use]
    pub fn event_sender(&self) -> broadcast::Sender<Event> {
        self.events.clone()
    }
}

#[async_trait]
impl NodeLink for LoopbackLink {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    async fn call(&self, method: &str, params: serde_json::Value) -> Result<serde_json::Value, NodeError> {
        let ctx = RpcContext {
            services: Arc::clone(&self.services),
            session_id: self.session_id,
            correlation_id: CorrelationId::new(),
            principal: self.principal.clone(),
        };
        Ok(self.router.dispatch(ctx, method, params).await?)
    }

    fn events(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }
}
