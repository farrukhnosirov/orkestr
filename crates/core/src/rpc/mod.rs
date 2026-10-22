//! RPC method routing (contract §4).
//!
//! Every area registers its handlers in `rpc/<area>.rs` through a
//! `pub fn register(router: &mut RpcRouter)` function; [`build_router`] wires
//! all areas. Handlers are typed by `orkestr_protocol::methods::*` markers.

pub mod agents;
pub mod approvals;
pub mod architecture;
pub mod audit;
pub mod benchmarks;
pub mod claude_code;
pub mod claude_engine;
pub mod computer;
pub mod consensus;
pub mod context;
pub mod cost;
pub mod dashboard;
pub mod dashboards;
pub mod deploy;
pub mod detective;
pub mod emergency;
pub mod events;
pub mod extensions;
pub mod files;
pub mod jobs_ctl;
pub mod jobs_ui;
pub mod layouts;
pub mod lsp;
pub mod manager;
pub mod marketplace;
pub mod memory;
pub mod merge;
pub mod models;
pub mod monitor;
pub mod node_worker;
pub mod nodes;
pub mod permissions;
pub mod pipeline;
pub mod plugins;
pub mod pm;
pub mod projects;
pub mod providers;
pub mod push;
pub mod quality;
pub mod remote;
pub mod replay;
pub mod reviews;
pub mod robots;
pub mod scheduler;
pub mod secrets;
pub mod settings;
pub mod snapshots;
pub mod sync;
pub mod system;
pub mod tasks;
pub mod teams;
pub mod templates;
pub mod terminal;
pub mod ui;
pub mod vcs;
pub mod voice;
pub mod vscode;
pub mod widgets;

use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use orkestr_domain::{CorrelationId, DeviceId, ProjectId, SessionId};
use orkestr_protocol::methods::Method;
use orkestr_protocol::policy::Role;
use orkestr_protocol::RpcError;
use serde_json::Value;

use crate::services::Services;

/// A remote caller (paired device over the E2E tunnel, Phase 4).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemotePrincipal {
    /// Paired device.
    pub device_id: DeviceId,
    /// Granted role.
    pub role: Role,
    /// Remote terminal access granted.
    pub remote_terminal: bool,
    /// Project restriction (empty = all projects).
    pub project_ids: Vec<ProjectId>,
    /// The peer is a coordinator core that enrolled this core as a worker node
    /// (Phase 6): may call the coordinator-only `node.*` / `jobs.offer|drain` methods.
    pub coordinator: bool,
    /// Trust level this worker granted the coordinator (Phase 6); `None` for devices.
    pub trust_level: Option<orkestr_domain::TrustLevel>,
}

/// Who is calling. Local callers (desktop, CLI over loopback) are not
/// restricted by `MethodPolicy`; remote callers are (enforced in dispatch by
/// the remote-access package).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum Principal {
    /// The local machine (loopback + core token).
    #[default]
    Local,
    /// A paired remote device.
    Remote(RemotePrincipal),
}

impl Principal {
    /// True for [`Principal::Local`].
    #[must_use]
    pub fn is_local(&self) -> bool {
        matches!(self, Self::Local)
    }
}

/// Per-request context.
#[derive(Clone)]
pub struct RpcContext {
    /// Shared services.
    pub services: Arc<Services>,
    /// Connection session.
    pub session_id: SessionId,
    /// Correlation id of this request (ambient for published events).
    pub correlation_id: CorrelationId,
    /// Caller identity (default [`Principal::Local`]).
    pub principal: Principal,
}

impl std::fmt::Debug for RpcContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RpcContext")
            .field("session_id", &self.session_id)
            .field("correlation_id", &self.correlation_id)
            .field("principal", &self.principal)
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

    /// Dispatches a request. Unknown methods → `bad_request` (local) /
    /// `forbidden` (remote: fail closed before revealing anything).
    ///
    /// Local callers are unrestricted. Remote callers go through
    /// `remote::authz::authorize` (MethodPolicy: local-only, role, remote
    /// terminal, project scope, per-device rate limit, step-up); every remote
    /// decision is audited.
    pub async fn dispatch(&self, ctx: RpcContext, method: &str, mut params: Value) -> Result<Value, RpcError> {
        if let Principal::Remote(rp) = &ctx.principal {
            let rp = rp.clone();
            let audited = params.clone();
            let decision = crate::remote::authz::authorize(&ctx, &rp, method, &mut params).await;
            ctx.services.remote.audit_call(rp.device_id, method, &audited, decision.as_ref().err()).await;
            decision?;
        }
        let Some(handler) = self.handlers.get(method).cloned() else {
            return Err(RpcError::bad_request(format!("unknown method `{method}`")));
        };
        let correlation = ctx.correlation_id;
        orkestr_events::with_correlation(correlation, handler(ctx, params)).await
    }
}

/// Registers `M` with a handler that always fails with the explicit
/// `unimplementedFeature` error (RR-G14) — for skeleton areas.
pub(crate) fn stub<M: Method>(router: &mut RpcRouter) {
    router.method::<M, _, _>(|_ctx: RpcContext, _params: M::Params| async {
        Err::<M::Result, RpcError>(RpcError::unimplemented_feature(M::NAME))
    });
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
    router.register(quality::register);
    // review-consensus (Phase 2)
    router.register(reviews::register).register(consensus::register);
    // architecture map + execution replay (Phase 2)
    router.register(architecture::register).register(replay::register);
    router.register(merge::register);
    router.register(pipeline::register);
    // Phase 2: context optimizer + benchmarks.
    router.register(context::register).register(benchmarks::register);
    router.register(scheduler::register);
    router.register(teams::register).register(pm::register);
    // theme engine + AI-editable UI patches (Phase 3)
    router.register(ui::register);
    // layout-multimonitor (Phase 3)
    router.register(layouts::register);
    // mission-control (Phase 3)
    router.register(dashboard::register);
    router.register(robots::register);
    // voice-sound (Phase 3)
    router.register(voice::register);
    // embedded VS Code (VSCodium REH web server)
    router.register(vscode::register);
    // VS Code extensions in the native editor + language servers (ADR-0025)
    router.register(extensions::register).register(lsp::register);
    // Claude Code panel (official `claude` CLI, stream-json)
    router.register(claude_code::register);
    router.register(claude_engine::register);
    // remote access, sync, Web Push (Phase 4)
    router.register(remote::register).register(sync::register).register(push::register);
    // multi-node (Phase 6): one registration per work-package file
    router
        .register(node_worker::register)
        .register(jobs_ui::register)
        .register(jobs_ctl::register)
        .register(models::register);
    // ecosystem (Phase 7): WP1 plugins, WP2 marketplace, WP3 dashboards + widgets,
    // WP4 environments + deploy, WP5 monitor, WP6 detective
    router.register(plugins::register).register(marketplace::register);
    router.register(dashboards::register).register(widgets::register);
    router.register(deploy::register).register(monitor::register).register(detective::register);
    // Wave 8: computer assistant (ADR-0035), local only
    router.register(computer::register);
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

    #[test]
    fn principal_defaults_to_local() {
        assert!(Principal::default().is_local());
    }
}
