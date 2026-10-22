//! Widget catalogue + sandboxed bundles (`widgets.*`, ADR-0034). **Owner: WP3.**
//!
//! * `widgets.list` — built-in widget descriptors (native desktop widgets,
//!   ids `builtin:<id>`) plus widgets of enabled plugins (ids
//!   `<packageId>/<widgetId>`). Plugin widgets whose data binding violates the
//!   allowlist are left out (logged).
//! * `widgets.bundle` — the plugin widget's self-contained HTML plus the CSP
//!   the host injects. The binding allowlist is enforced here (core) and again
//!   in the desktop bridge: every bound method must be Read-class and not
//!   `local_only` per [`policy_for`]; event kinds must be known.
//!
//! Plugin widgets come from a [`PluginWidgetSource`]. The default source is
//! the plugin host (`PluginService::widgets` / `widget_html`); tests may
//! override it with [`set_plugin_widget_source`].

use std::collections::HashMap;
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use orkestr_domain::{DataBinding, PluginId, WidgetDecl, WidgetSize};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::widgets::{
    self as m, ListWidgetsResult, WidgetBundleParams, WidgetBundleResult, WidgetDescriptor, WidgetSource,
};
use orkestr_protocol::policy::{policy_for, RateClass};
use orkestr_protocol::RpcError;
use parking_lot::RwLock;
use serde_json::json;
use sha2::{Digest, Sha256};

use super::{RpcContext, RpcRouter};
use crate::services::Services;

/// CSP injected into every plugin widget document (no network, inline only).
pub const WIDGET_CSP: &str = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; \
img-src data:; font-src data:; connect-src 'none'; media-src 'none'; object-src 'none'; frame-src 'none'; \
worker-src 'none'; form-action 'none'; base-uri 'none'";

/// Max size of a widget HTML bundle.
pub const MAX_WIDGET_HTML_BYTES: usize = 512 * 1024;
/// Max methods / events in one binding.
pub const MAX_BINDING_ENTRIES: usize = 32;

/// Prefix of built-in widget ids.
pub const BUILTIN_PREFIX: &str = "builtin:";

/// A widget declared by an enabled plugin.
#[derive(Debug, Clone)]
pub struct PluginWidget {
    /// Install id.
    pub plugin_id: PluginId,
    /// Manifest package id.
    pub package_id: String,
    /// Declaration.
    pub decl: WidgetDecl,
}

/// Where plugin widgets come from (implemented over the plugin host).
#[async_trait]
pub trait PluginWidgetSource: Send + Sync {
    /// Widgets of enabled plugins.
    async fn widgets(&self) -> Result<Vec<PluginWidget>, RpcError>;
    /// HTML of `widget.decl.entry_html` inside the plugin package.
    async fn html(&self, widget: &PluginWidget) -> Result<String, RpcError>;
}

/// Default source over `Services.plugins` (the plugin host): widgets of
/// enabled `Widget` plugins and their entry HTML read from the verified,
/// extracted package (refused in safe mode).
struct ServicePluginWidgets(Arc<orkestr_plugins::PluginService>);

#[async_trait]
impl PluginWidgetSource for ServicePluginWidgets {
    async fn widgets(&self) -> Result<Vec<PluginWidget>, RpcError> {
        let widgets = match self.0.widgets().await {
            Ok(w) => w,
            Err(e) => {
                tracing::debug!(error = %e, "plugin widgets unavailable");
                return Ok(Vec::new());
            }
        };
        Ok(widgets
            .into_iter()
            .map(|w| PluginWidget { plugin_id: w.plugin_id, package_id: w.package_id, decl: w.widget })
            .collect())
    }

    async fn html(&self, widget: &PluginWidget) -> Result<String, RpcError> {
        Ok(self.0.widget_html(&widget.plugin_id.to_string(), &widget.decl.id).await?)
    }
}

fn source_slot() -> &'static RwLock<Option<Arc<dyn PluginWidgetSource>>> {
    static SLOT: OnceLock<RwLock<Option<Arc<dyn PluginWidgetSource>>>> = OnceLock::new();
    SLOT.get_or_init(|| RwLock::new(None))
}

/// Installs the plugin widget source (integration: the plugin host).
pub fn set_plugin_widget_source(source: Arc<dyn PluginWidgetSource>) {
    *source_slot().write() = Some(source);
}

fn source(services: &Services) -> Arc<dyn PluginWidgetSource> {
    source_slot().read().clone().unwrap_or_else(|| Arc::new(ServicePluginWidgets(Arc::clone(&services.plugins))))
}

/// Registers the area.
pub fn register(router: &mut RpcRouter) {
    router
        .method::<m::List, _, _>(|ctx: RpcContext, _| async move {
            let widgets = catalog_with(source(&ctx.services).as_ref()).await?;
            Ok(ListWidgetsResult { widgets })
        })
        .method::<m::Bundle, _, _>(|ctx: RpcContext, p: WidgetBundleParams| async move {
            bundle_with(source(&ctx.services).as_ref(), &p).await
        });
}

// ── binding allowlist ────────────────────────────────────────────────────

/// True when a widget may bind `method`: a known method whose policy is
/// Read-class and not `local_only` (fail closed for unknown methods).
#[must_use]
pub fn is_bindable_method(method: &str) -> bool {
    policy_for(method).is_some_and(|p| p.rate_class == RateClass::Read && !p.local_only && !p.coordinator_only)
}

fn known_event_kinds() -> &'static std::collections::HashSet<String> {
    static KINDS: OnceLock<std::collections::HashSet<String>> = OnceLock::new();
    KINDS.get_or_init(|| EventKind::NAMES.iter().map(|s| (*s).to_owned()).collect())
}

/// Validates a widget data binding.
///
/// # Errors
/// `forbidden` naming the first method that is not Read-class / is local-only
/// / unknown, or the first unknown event kind; `bad_request` when too large.
pub fn validate_binding(binding: &DataBinding) -> Result<(), RpcError> {
    if binding.methods.len() > MAX_BINDING_ENTRIES || binding.events.len() > MAX_BINDING_ENTRIES {
        return Err(RpcError::bad_request(format!("data binding exceeds {MAX_BINDING_ENTRIES} entries")));
    }
    for method in &binding.methods {
        if !is_bindable_method(method) {
            return Err(RpcError::forbidden(format!(
                "widget data binding may only use Read-class, non-local methods; {method:?} is not allowed"
            )));
        }
    }
    let kinds = known_event_kinds();
    for kind in &binding.events {
        if !kinds.contains(kind) {
            return Err(RpcError::forbidden(format!("widget data binding: unknown event kind {kind:?}")));
        }
    }
    Ok(())
}

// ── catalogue ────────────────────────────────────────────────────────────

fn sizes(list: &[(u32, u32)]) -> Vec<WidgetSize> {
    list.iter().map(|&(w, h)| WidgetSize { w, h }).collect()
}

fn no_config() -> serde_json::Value {
    json!({ "type": "object", "properties": {}, "additionalProperties": false })
}

fn builtin(id: &str, title: &str, size_list: &[(u32, u32)], methods: &[&str], events: &[&str]) -> WidgetDescriptor {
    WidgetDescriptor {
        id: format!("{BUILTIN_PREFIX}{id}"),
        title: title.to_owned(),
        source: WidgetSource::BuiltIn,
        sizes: sizes(size_list),
        data_binding: DataBinding {
            methods: methods.iter().map(|s| (*s).to_owned()).collect(),
            events: events.iter().map(|s| (*s).to_owned()).collect(),
        },
        config_schema: no_config(),
    }
}

/// Built-in (native desktop) widgets. Mirrors the Mission Control widget
/// registry plus the dashboard-only `note` widget; titles are fallbacks (the
/// desktop localizes them).
#[must_use]
pub fn builtin_widgets() -> Vec<WidgetDescriptor> {
    const S: (u32, u32) = (3, 3);
    const M: (u32, u32) = (4, 4);
    const L: (u32, u32) = (6, 4);
    const XL: (u32, u32) = (8, 4);
    const FULL: (u32, u32) = (12, 4);
    let mut out = vec![
        builtin(
            "overview",
            "Overview",
            &[FULL, XL],
            &["dashboard.snapshot"],
            &["AgentStateChanged", "TaskStatusChanged"],
        ),
        builtin(
            "agents",
            "Agents",
            &[XL, L, FULL],
            &["agents.list"],
            &["AgentSpawned", "AgentStateChanged", "AgentUpdated", "AgentStopped"],
        ),
        builtin("approvals", "Approvals", &[M, L], &["approvals.list"], &["ApprovalRequested", "ApprovalResolved"]),
        builtin("projects", "Projects", &[L, M, XL], &["dashboard.snapshot"], &["ProjectCreated", "ProjectUpdated"]),
        builtin(
            "taskGraph",
            "Task graph",
            &[L, XL, FULL],
            &["tasks.graph", "manager.status"],
            &["TaskStatusChanged", "ManagerRunUpdated"],
        ),
        builtin("nodes", "Nodes", &[L, XL, FULL], &["nodes.list", "nodes.resources"], &["NodeStatus"]),
        builtin(
            "providers",
            "Providers",
            &[L, M],
            &["dashboard.snapshot"],
            &["ProviderStatusChanged", "ProviderRateLimited"],
        ),
        builtin("budgets", "Budgets", &[L, M, XL], &["dashboard.snapshot", "budgets.list"], &["BudgetThreshold"]),
        builtin(
            "alerts",
            "Alerts",
            &[L, M],
            &["events.replay"],
            &["BuildFailed", "ProviderRateLimited", "BudgetThreshold"],
        ),
        builtin(
            "quality",
            "Quality",
            &[M, L],
            &["quality.runs", "quality.getRun"],
            &["BuildStarted", "BuildFailed", "BuildSucceeded", "TestRunFinished"],
        ),
        builtin(
            "reviews",
            "Reviews",
            &[M, L],
            &["reviews.list", "merge.list"],
            &["ReviewUpdated", "MergeProposalUpdated"],
        ),
        builtin("deployments", "Deployments", &[M, S], &[], &["DeploymentFinished"]),
    ];
    let mut note = builtin("note", "Note", &[M, S, L, XL, FULL], &[], &[]);
    note.config_schema = json!({
        "type": "object",
        "properties": {
            "text": { "type": "string", "title": "Text", "maxLength": 4000 },
            "tone": { "type": "string", "title": "Tone", "enum": ["neutral", "info", "success", "warning", "danger"], "default": "neutral" }
        },
        "additionalProperties": false
    });
    out.push(note);
    out
}

fn descriptor(w: &PluginWidget) -> WidgetDescriptor {
    WidgetDescriptor {
        id: format!("{}/{}", w.package_id, w.decl.id),
        title: w.decl.title.clone(),
        source: WidgetSource::Plugin { plugin_id: w.plugin_id, package_id: w.package_id.clone() },
        sizes: w.decl.sizes.clone(),
        data_binding: w.decl.data_binding.clone(),
        config_schema: w.decl.config_schema.clone(),
    }
}

/// Built-ins plus valid plugin widgets (invalid bindings are skipped).
///
/// # Errors
/// Source failure.
pub async fn catalog_with(source: &dyn PluginWidgetSource) -> Result<Vec<WidgetDescriptor>, RpcError> {
    let mut out = builtin_widgets();
    for w in source.widgets().await? {
        let d = descriptor(&w);
        if let Err(e) = validate_binding(&d.data_binding) {
            tracing::warn!(widget = %d.id, error = %e.message, "plugin widget hidden: invalid data binding");
            continue;
        }
        if out.iter().any(|x| x.id == d.id) {
            tracing::warn!(widget = %d.id, "duplicate plugin widget id ignored");
            continue;
        }
        out.push(d);
    }
    Ok(out)
}

/// Catalogue keyed by widget id (used by `dashboards.save`).
///
/// # Errors
/// Source failure.
pub async fn catalog_map(services: &Services) -> Result<HashMap<String, WidgetDescriptor>, RpcError> {
    Ok(catalog_with(source(services).as_ref()).await?.into_iter().map(|d| (d.id.clone(), d)).collect())
}

/// `widgets.bundle` against an explicit source.
///
/// # Errors
/// `not_found`, `forbidden` (binding allowlist), `bad_request` (bundle too large).
pub async fn bundle_with(
    source: &dyn PluginWidgetSource,
    p: &WidgetBundleParams,
) -> Result<WidgetBundleResult, RpcError> {
    if p.widget_id.starts_with(BUILTIN_PREFIX) {
        return Err(RpcError::bad_request("built-in widgets render natively and have no bundle"));
    }
    let widgets = source.widgets().await?;
    let widget = widgets
        .into_iter()
        .find(|w| format!("{}/{}", w.package_id, w.decl.id) == p.widget_id)
        .ok_or_else(|| RpcError::not_found(format!("widget {} not found (plugin missing or disabled)", p.widget_id)))?;
    validate_binding(&widget.decl.data_binding)?;
    let html = source.html(&widget).await?;
    build_bundle(&p.widget_id, html)
}

/// Wraps widget HTML into a bundle result (size-checked, hashed, CSP attached).
///
/// # Errors
/// `bad_request` when the HTML is larger than [`MAX_WIDGET_HTML_BYTES`].
pub fn build_bundle(widget_id: &str, html: String) -> Result<WidgetBundleResult, RpcError> {
    if html.len() > MAX_WIDGET_HTML_BYTES {
        return Err(RpcError::bad_request(format!("widget bundle larger than {MAX_WIDGET_HTML_BYTES} bytes")));
    }
    let sha256 = hex::encode(Sha256::digest(html.as_bytes()));
    Ok(WidgetBundleResult { widget_id: widget_id.to_owned(), html, csp: WIDGET_CSP.to_owned(), sha256 })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn binding(methods: &[&str], events: &[&str]) -> DataBinding {
        DataBinding {
            methods: methods.iter().map(|s| (*s).to_owned()).collect(),
            events: events.iter().map(|s| (*s).to_owned()).collect(),
        }
    }

    #[test]
    fn allowlist_accepts_reads_only() {
        assert!(validate_binding(&binding(
            &["monitor.overview", "agents.list", "dashboard.snapshot"],
            &["AlertRaised"]
        ))
        .is_ok());
        // write-class
        let e = validate_binding(&binding(&["agents.spawn"], &[])).unwrap_err();
        assert!(e.message.contains("agents.spawn"));
        assert!(validate_binding(&binding(&["tasks.create"], &[])).is_err());
        assert!(validate_binding(&binding(&["approvals.resolve"], &[])).is_err());
        // local-only (secrets area, vscode area)
        assert!(validate_binding(&binding(&["secrets.list"], &[])).is_err());
        assert!(validate_binding(&binding(&["vscode.status"], &[])).is_err());
        // unknown method / event
        assert!(validate_binding(&binding(&["nope.list"], &[])).is_err());
        assert!(validate_binding(&binding(&[], &["NotAnEvent"])).is_err());
        assert!(validate_binding(&binding(&[], &["DashboardChanged", "AgentSpawned"])).is_ok());
    }

    #[test]
    fn builtins_are_wellformed() {
        let list = builtin_widgets();
        let mut ids = std::collections::HashSet::new();
        for d in &list {
            assert!(ids.insert(d.id.clone()), "duplicate {}", d.id);
            assert!(d.id.starts_with(BUILTIN_PREFIX));
            assert!(!d.sizes.is_empty());
            for e in &d.data_binding.events {
                assert!(known_event_kinds().contains(e), "{} binds unknown event {e}", d.id);
            }
            assert!(crate::dashboards_store::config_schema::validate(&d.config_schema, &json!({})).is_ok());
        }
    }

    struct Fake(Vec<PluginWidget>);

    #[async_trait]
    impl PluginWidgetSource for Fake {
        async fn widgets(&self) -> Result<Vec<PluginWidget>, RpcError> {
            Ok(self.0.clone())
        }
        async fn html(&self, _w: &PluginWidget) -> Result<String, RpcError> {
            Ok("<!doctype html><p>hi</p>".into())
        }
    }

    fn pw(id: &str, b: DataBinding) -> PluginWidget {
        PluginWidget {
            plugin_id: PluginId::new(),
            package_id: "acme.stats".into(),
            decl: WidgetDecl {
                id: id.into(),
                title: id.into(),
                entry_html: "w.html".into(),
                sizes: vec![WidgetSize { w: 4, h: 3 }],
                data_binding: b,
                config_schema: serde_json::Value::Null,
            },
        }
    }

    #[tokio::test]
    async fn bundle_enforces_allowlist_and_csp() {
        let fake = Fake(vec![
            pw("good", binding(&["monitor.overview"], &["AlertRaised"])),
            pw("writer", binding(&["deploy.start"], &[])),
            pw("local", binding(&["secrets.list"], &[])),
        ]);
        let ok = bundle_with(&fake, &WidgetBundleParams { widget_id: "acme.stats/good".into() }).await.unwrap();
        assert_eq!(ok.csp, WIDGET_CSP);
        assert!(ok.csp.contains("default-src 'none'") && ok.csp.contains("connect-src 'none'"));
        assert_eq!(ok.sha256.len(), 64);
        for bad in ["acme.stats/writer", "acme.stats/local"] {
            let e = bundle_with(&fake, &WidgetBundleParams { widget_id: bad.into() }).await.unwrap_err();
            assert_eq!(e.code, orkestr_protocol::RpcErrorCode::Forbidden, "{bad}");
        }
        let e = bundle_with(&fake, &WidgetBundleParams { widget_id: "acme.stats/none".into() }).await.unwrap_err();
        assert_eq!(e.code, orkestr_protocol::RpcErrorCode::NotFound);
        // catalogue hides invalid plugin widgets
        let cat = catalog_with(&fake).await.unwrap();
        assert!(cat.iter().any(|d| d.id == "acme.stats/good"));
        assert!(!cat.iter().any(|d| d.id == "acme.stats/writer" || d.id == "acme.stats/local"));
        assert!(build_bundle("x", "a".repeat(MAX_WIDGET_HTML_BYTES + 1)).is_err());
    }
}
