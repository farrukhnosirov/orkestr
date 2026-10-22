//! Plugin tools for agents: the `orkestr_agent_runtime::tools::external::ExternalTools`
//! implementation backed by `orkestr_plugins::PluginService` (Phase 7,
//! ADR-0034). **Owner: WP1.** Installed into the runtime with
//! `AgentRuntime::set_external_tools` by [`wire`], which also installs the
//! [`CoreRpcReader`] used by the plugins' `RpcRead` host calls and loads the
//! plugin state (safe mode, installed plugins).
//!
//! Tool names: `plugin_<package slug>__<tool>` (`orkestr-dev.hello-tool` +
//! `hello` → `plugin_orkestr_dev_hello_tool__hello`); names longer than 64
//! chars use an 8-hex hash of the package id instead of the slug.

use std::sync::{Arc, Weak};

use async_trait::async_trait;
use orkestr_agent_runtime::tools::external::{ExternalToolDecl, ExternalTools, ToolCallCtx, EXTERNAL_TOOL_PREFIX};
use orkestr_domain::{CorrelationId, ProjectId, SessionId};
use orkestr_plugins::{PluginService, PluginTool, RpcReader};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::rpc::{Principal, RpcContext, RpcRouter};
use crate::services::Services;

/// Agent-facing name of a plugin tool.
#[must_use]
pub fn tool_name(package_id: &str, tool: &str) -> String {
    let slug: String =
        package_id.chars().map(|c| if c.is_ascii_alphanumeric() { c.to_ascii_lowercase() } else { '_' }).collect();
    let name = format!("{EXTERNAL_TOOL_PREFIX}{slug}__{tool}");
    if name.len() <= 64 {
        return name;
    }
    let h = hex::encode(&Sha256::digest(package_id.as_bytes())[..4]);
    format!("{EXTERNAL_TOOL_PREFIX}{h}__{tool}").chars().take(64).collect()
}

/// [`ExternalTools`] over the enabled `Tool` plugins.
#[derive(Debug, Clone)]
pub struct PluginTools {
    plugins: Arc<PluginService>,
}

impl PluginTools {
    /// New provider.
    #[must_use]
    pub fn new(plugins: Arc<PluginService>) -> Self {
        Self { plugins }
    }

    fn resolve(&self, name: &str) -> Option<PluginTool> {
        self.plugins.tool_catalog().into_iter().find(|t| tool_name(&t.package_id, &t.tool.name) == name)
    }
}

#[async_trait]
impl ExternalTools for PluginTools {
    fn list(&self, _project: &ProjectId) -> Vec<ExternalToolDecl> {
        self.plugins
            .tool_catalog()
            .into_iter()
            .map(|t| ExternalToolDecl {
                name: tool_name(&t.package_id, &t.tool.name),
                description: format!("[plugin {} ({})] {}", t.plugin_name, t.package_id, t.tool.description),
                input_schema: t.tool.input_schema.clone(),
                capability: t.capability,
            })
            .collect()
    }

    async fn call(&self, tool: &str, input: Value, ctx: ToolCallCtx) -> Result<Value, String> {
        let t = self
            .resolve(tool)
            .ok_or_else(|| format!("plugin tool `{tool}` is not available (disabled or uninstalled)"))?;
        self.plugins
            .invoke_tool(t.plugin_id, &t.tool.name, input, Some(ctx.project_id), ctx.cancel)
            .await
            .map(|r| r.output)
            .map_err(|e| e.to_string())
    }
}

/// Executes plugin `RpcRead` host calls through the core router as a local
/// call. The plugin host already checked the grant and that the method is
/// Read-class, Viewer-role and not local-only; this re-checks the policy.
pub struct CoreRpcReader {
    services: Weak<Services>,
    router: Arc<RpcRouter>,
}

impl std::fmt::Debug for CoreRpcReader {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CoreRpcReader").finish_non_exhaustive()
    }
}

#[async_trait]
impl RpcReader for CoreRpcReader {
    async fn read(&self, method: &str, params: Value, _project: Option<ProjectId>) -> Result<Value, String> {
        if !orkestr_plugins::host::rpc_read_allowed(method) {
            return Err(format!("`{method}` may not be read by plugins"));
        }
        let services = self.services.upgrade().ok_or("the core is shutting down")?;
        let ctx = RpcContext {
            services,
            session_id: SessionId::new(),
            correlation_id: CorrelationId::new(),
            principal: Principal::Local,
        };
        self.router.dispatch(ctx, method, params).await.map_err(|e| e.message)
    }
}

/// Wires the plugin host into the core (call once at boot, after the router
/// is built): agent tools, RPC reader, plugin state load (safe mode honoured).
pub async fn wire(services: &Arc<Services>, router: Arc<RpcRouter>) {
    services.plugins.set_rpc_reader(Arc::new(CoreRpcReader { services: Arc::downgrade(services), router }));
    services.agents.set_external_tools(Arc::new(PluginTools::new(Arc::clone(&services.plugins))));
    if let Err(e) = services.plugins.init().await {
        tracing::error!(error = %e, "plugin host state could not be loaded");
    }
}

#[cfg(test)]
mod tests {
    use super::tool_name;

    #[test]
    fn tool_names_are_valid() {
        let n = tool_name("orkestr-dev.hello-tool", "hello");
        assert_eq!(n, "plugin_orkestr_dev_hello_tool__hello");
        let long = tool_name(&format!("acme.{}", "x".repeat(80)), "do_it");
        assert!(long.len() <= 64 && long.starts_with("plugin_") && long.ends_with("__do_it"), "{long}");
        for n in [n, long] {
            assert!(n.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_'));
        }
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn plugin_tools_call_a_real_plugin() {
        use std::sync::Arc;

        use base64::Engine as _;
        use orkestr_agent_runtime::tools::external::{ExternalTools, ToolCallCtx};
        use orkestr_domain::{AgentId, Capability, ProjectId};
        use orkestr_plugins::{fixtures, PluginDeps, PluginService};
        use orkestr_protocol::methods::plugins::{GrantDecision, GrantPluginParams, InstallPluginParams};
        use serde_json::json;

        let dir = tempfile::tempdir().unwrap();
        let db = orkestr_storage::Db::open(dir.path().join("o.db")).unwrap();
        let mut migs = orkestr_storage::storage_migrations();
        migs.extend(orkestr_events::events_migrations());
        migs.extend(orkestr_security::migrations());
        migs.extend(orkestr_plugins::plugin_migrations());
        db.migrate(migs).await.unwrap();
        let events = orkestr_events::EventBus::in_memory(1024);
        let sec = Arc::new(
            orkestr_security::Security::open(orkestr_security::SecurityConfig {
                db: db.clone(),
                events: events.clone(),
                data_dir: dir.path().join("data"),
                credentials: None,
                secret_backend: orkestr_security::SecretBackendChoice::Memory,
                guard: orkestr_security::GuardConfig::default(),
            })
            .await
            .unwrap(),
        );
        let svc = Arc::new(PluginService::new(PluginDeps {
            db,
            events,
            security: sec,
            data_dir: dir.path().join("data").join("plugins"),
        }));
        let key = fixtures::dev_key();
        svc.trust_store().trust(&key.public_key_b64(), "dev", "test").await.unwrap();
        let bytes = fixtures::hello_tool(&key).unwrap();
        let r = svc
            .install(InstallPluginParams {
                package_base64: Some(base64::engine::general_purpose::STANDARD.encode(bytes)),
                ..Default::default()
            })
            .await
            .unwrap();
        let tools = super::PluginTools::new(Arc::clone(&svc));
        assert!(tools.list(&ProjectId::new()).is_empty(), "not enabled yet");
        let decisions = r
            .pending_grants
            .iter()
            .map(|p| GrantDecision { capability: p.capability, scope: p.scope.clone(), allow: true })
            .collect();
        svc.grant(GrantPluginParams { plugin_id: r.plugin.plugin.id, decisions, enable: true }).await.unwrap();
        let decls = tools.list(&ProjectId::new());
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "plugin_orkestr_dev_hello_tool__hello");
        assert_eq!(decls[0].capability, Some(Capability::Network));
        let ctx = || ToolCallCtx {
            project_id: ProjectId::new(),
            agent_id: AgentId::new(),
            task_id: None,
            cancel: tokio_util::sync::CancellationToken::new(),
        };
        let out = tools.call(&decls[0].name, json!({"name": "Ada"}), ctx()).await.unwrap();
        assert_eq!(out["echo"]["input"]["name"], "Ada");
        svc.kill(None, "test").await;
        assert!(tools.list(&ProjectId::new()).is_empty());
        assert!(tools.call(&decls[0].name, json!({"name": "Ada"}), ctx()).await.is_err());
    }
}
