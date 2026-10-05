//! RPC method routing (contract §4).
//!
//! Every area registers its handlers in `rpc/<area>.rs` through a
//! `pub fn register(router: &mut RpcRouter)` function; [`build_router`] wires
//! all areas. Handlers are typed by `orkestr_protocol::methods::*` markers.

pub mod agents;
pub mod approvals;
pub mod audit;
pub mod cost;
pub mod emergency;
pub mod events;
pub mod files;
pub mod manager;
pub mod memory;
pub mod merge;
pub mod nodes;
pub mod permissions;
pub mod projects;
pub mod providers;
pub mod secrets;
pub mod settings;
pub mod snapshots;
pub mod system;
pub mod tasks;
pub mod templates;
pub mod terminal;
pub mod vcs;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use orkestr_domain::{CorrelationId, SessionId};
use orkestr_protocol::methods::Method;
use orkestr_protocol::RpcError;
use serde_json::Value;

use crate::services::Services;

/// Per-request context.
#[derive(Clone)]
pub struct RpcContext {
    /// Shared services.
    pub services: Arc<Services>,
    /// Connection session.
    pub session_id: SessionId,
    /// Correlation id of this request (ambient for published events).
    pub correlation_id: CorrelationId,
}

impl std::fmt::Debug for RpcContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcContext")
            .field("session_id", &self.session_id)
            .field("correlation_id", &self.correlation_id)
            .finish_non_exhaustive()
    }
}

type BoxFuture = Pin<Box<dyn Future<Output = Result<Value, RpcError>> + Send>>;
type Handler = Arc<dyn Fn(RpcContext, Value) -> BoxFuture + Send + Sync>;

/// Method name → handler table.
#[derive(Default, Clone)]
pub struct RpcRouter {
    handlers: BTreeMap<&'static str, Handler>,
}

impl std::fmt::Debug for RpcRouter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcRouter").field("methods", &self.methods()).finish()
    }
}

impl RpcRouter {
    /// Empty router.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Registers an area (`router.register(projects::register)`).
    pub fn register(&mut self, area: fn(&mut RpcRouter)) -> &mut Self {
        area(self);
        self
    }

    /// Registers a typed handler for method `M`.
    ///
    /// # Panics
    /// Panics if `M::NAME` is already registered (a wiring bug caught at startup/tests).
    pub fn method<M, F, Fut>(&mut self, handler: F) -> &mut Self
    where
        M: Method,
        F: Fn(RpcContext, M::Params) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<M::Result, RpcError>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let erased: Handler = Arc::new(move |ctx: RpcContext, params: Value| {
            let handler = Arc::clone(&handler);
            Box::pin(async move {
                let params = if params.is_null() { Value::Object(serde_json::Map::new()) } else { params };
                let typed: M::Params = serde_json::from_value(params)
                    .map_err(|e| RpcError::bad_request(format!("invalid params for {}: {e}", M::NAME)))?;
                let result = handler(ctx, typed).await?;
                serde_json::to_value(result).map_err(|e| {
                    tracing::error!(method = M::NAME, error = %e, "failed to serialize result");
                    RpcError::internal("failed to serialize result")
                })
            }) as BoxFuture
        });
        assert!(self.handlers.insert(M::NAME, erased).is_none(), "method {} registered twice", M::NAME);
        self
    }

    /// Registered method names (sorted).
    #[must_use]
    pub fn methods(&self) -> Vec<&'static str> {
        self.handlers.keys().copied().collect()
    }

    /// Dispatches a request. Unknown methods → `bad_request`.
    pub async fn dispatch(&self, ctx: RpcContext, method: &str, params: Value) -> Result<Value, RpcError> {
        let Some(handler) = self.handlers.get(method).cloned() else {
            return Err(RpcError::bad_request(format!("unknown method `{method}`")));
        };
        let correlation = ctx.correlation_id;
        orkestr_events::with_correlation(correlation, handler(ctx, params)).await
    }
}

/// Builds the router with every implemented area.
#[must_use]
pub fn build_router() -> RpcRouter {
    let mut router = RpcRouter::new();
    router.register(system::register).register(projects::register).register(events::register);
    // security (Phase 1)
    router
        .register(permissions::register)
        .register(approvals::register)
        .register(secrets::register)
        .register(emergency::register)
        .register(audit::register);
    router.register(providers::register);
    router.register(tasks::register);
    router.register(vcs::register).register(snapshots::register);
    router.register(terminal::register);
    router.register(files::register);
    router.register(memory::register).register(cost::register);
    router.register(settings::register).register(nodes::register);
    router.register(templates::register).register(agents::register);
    router.register(manager::register);
    router.register(merge::register);
    router
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn router_covers_protocol_registry_exactly() {
        let router = build_router();
        let mut registered = router.methods();
        let mut declared: Vec<&str> = orkestr_protocol::methods::registry().iter().map(|m| m.name).collect();
        registered.sort_unstable();
        declared.sort_unstable();
        assert_eq!(registered, declared, "protocol registry and core router must list the same methods");
    }
}
