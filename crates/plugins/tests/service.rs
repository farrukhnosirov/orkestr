//! End-to-end tests of the plugin host: install → grant → enable → invoke,
//! signature refusals, sandbox limits, host enforcement + audit, kill switch,
//! safe mode across restarts.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{Capability, PluginLimits, PluginState};
use orkestr_events::{EventBus, Received};
use orkestr_plugins::fixtures::{self, Fixture};
use orkestr_plugins::host::HostApi;
use orkestr_plugins::{package, wat, HostOp, PluginDeps, PluginError, PluginService};
use orkestr_protocol::methods::plugins::{GrantDecision, GrantPluginParams, InstallPluginParams, InvokePluginParams};
use orkestr_security::Security;
use orkestr_storage::Db;
use serde_json::{json, Value};

struct H {
    _dir: tempfile::TempDir,
    db: Db,
    events: EventBus,
    sec: Arc<Security>,
    data: std::path::PathBuf,
    svc: Arc<PluginService>,
    fixtures: BTreeMap<&'static str, Fixture>,
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("o.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_plugins::plugin_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(4096);
    let sec = Arc::new(
        Security::open(orkestr_security::SecurityConfig {
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
    let data = dir.path().join("data").join("plugins");
    let svc = Arc::new(PluginService::new(PluginDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&sec),
        data_dir: data.clone(),
    }));
    let key = fixtures::dev_key();
    svc.trust_store().trust(&key.public_key_b64(), "Orkestr Dev", "test").await.unwrap();
    let fixtures = fixtures::all(&key).unwrap().into_iter().map(|f| (f.file, f)).collect();
    H { _dir: dir, db, events, sec, data, svc, fixtures }
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

impl H {
    async fn install(
        &self,
        file: &str,
    ) -> Result<orkestr_protocol::methods::plugins::InstallPluginResult, PluginError> {
        self.svc
            .install(InstallPluginParams {
                path: None,
                package_base64: Some(b64(&self.fixtures[file].bytes)),
                source: Some("test".into()),
            })
            .await
    }

    async fn install_enabled(&self, file: &str) -> orkestr_domain::PluginId {
        let r = self.install(file).await.unwrap();
        let id = r.plugin.plugin.id;
        let decisions = r
            .pending_grants
            .iter()
            .map(|p| GrantDecision { capability: p.capability, scope: p.scope.clone(), allow: true })
            .collect();
        let info = self.svc.grant(GrantPluginParams { plugin_id: id, decisions, enable: true }).await.unwrap();
        assert_eq!(info.plugin.state, PluginState::Enabled);
        id
    }

    async fn invoke(&self, id: orkestr_domain::PluginId, tool: &str) -> Result<Value, PluginError> {
        self.svc
            .invoke(InvokePluginParams {
                plugin_id: id,
                tool: tool.into(),
                input: json!({"name": "Ada"}),
                project_id: None,
            })
            .await
            .map(|r| r.output)
    }
}

fn kinds(sub: &mut orkestr_events::Subscription) -> Vec<String> {
    use futures::FutureExt;
    let mut out = Vec::new();
    while let Some(Some(Received::Event(e))) = sub.recv().now_or_never() {
        out.push(e.kind_name().to_owned());
    }
    out
}

#[tokio::test(flavor = "multi_thread")]
async fn install_grant_enable_invoke_hello() {
    let h = harness().await;
    let mut sub = h.events.subscribe_all();
    let r = h.install("hello-tool.orkp").await.unwrap();
    assert_eq!(r.plugin.plugin.state, PluginState::Installed);
    assert_eq!(r.pending_grants.len(), 1);
    assert_eq!(r.pending_grants[0].capability, Capability::Network);
    assert!(r.plugin.signer_fingerprint.is_some());
    let id = r.plugin.plugin.id;
    assert!(h.data.join(id.to_string()).join("widgets/hello.html").is_file());
    // No code before grants + enable.
    assert!(matches!(h.invoke(id, "hello").await, Err(PluginError::Conflict(_))));
    assert!(matches!(h.svc.set_enabled(id, true, None).await, Err(PluginError::Conflict(_))));
    let info = h
        .svc
        .grant(GrantPluginParams {
            plugin_id: id,
            decisions: vec![GrantDecision {
                capability: Capability::Network,
                scope: Some("example.com".into()),
                allow: true,
            }],
            enable: true,
        })
        .await
        .unwrap();
    assert_eq!(info.plugin.state, PluginState::Enabled);
    assert!(info.pending_grants.is_empty());
    // Grants live in the permission store under principal plugin:<id>.
    let rules = h.sec.permissions.list(&Default::default()).await.unwrap();
    assert!(rules.iter().any(|r| r.rule.resource.as_deref() == Some(&format!("plugin:{id}/example.com"))));
    let out = h.invoke(id, "hello").await.unwrap();
    assert_eq!(out["echo"]["tool"], "hello");
    assert_eq!(out["echo"]["input"]["name"], "Ada");
    let logs = h.svc.logs(id, 50).await.unwrap();
    assert!(logs.iter().any(|l| l.message == "hello called"), "{logs:?}");
    // Tool + widget catalogues; widget asset readable, path escapes refused.
    let tools = h.svc.tool_catalog();
    assert_eq!(tools.len(), 1);
    assert_eq!(tools[0].capability, Some(Capability::Network));
    assert_eq!(h.svc.widgets().await.unwrap().len(), 1);
    assert!(h.svc.package_file(id, "widgets/hello.html").await.unwrap().starts_with(b"<!doctype"));
    assert!(h.svc.package_file(id, "../o.db").await.is_err());
    let k = kinds(&mut sub);
    assert!(k.contains(&"PluginInstalled".to_owned()) && k.contains(&"PluginStateChanged".to_owned()), "{k:?}");
    // Uninstall revokes the grants.
    h.svc.uninstall(id).await.unwrap();
    let rules = h.sec.permissions.list(&Default::default()).await.unwrap();
    assert!(!rules.iter().any(|r| r.rule.resource.as_deref().is_some_and(|s| s.starts_with("plugin:"))));
    assert!(!h.data.join(id.to_string()).exists());
}

#[tokio::test(flavor = "multi_thread")]
async fn tampered_unsigned_traversal_and_untrusted_are_refused() {
    let h = harness().await;
    assert!(matches!(h.install("evil-tampered.orkp").await, Err(PluginError::Signature(m)) if m.contains("tampered")));
    assert!(
        matches!(h.install("evil-unsigned.orkp").await, Err(PluginError::Signature(m)) if m.contains("ORKESTR_PLUGINS_DEV"))
    );
    assert!(matches!(h.install("evil-traversal.orkp").await, Err(PluginError::Package(m)) if m.contains("unsafe")));
    let stranger = orkestr_plugins::PublisherKeyPair::generate();
    let bytes = fixtures::hello_tool(&stranger).unwrap();
    let r = h.svc.install(InstallPluginParams { package_base64: Some(b64(&bytes)), ..Default::default() }).await;
    assert!(matches!(r, Err(PluginError::Signature(m)) if m.contains("not trusted")));
    assert!(h.svc.list().await.unwrap().is_empty());
    // Update with another (trusted) key is refused: key continuity.
    h.install("hello-tool.orkp").await.unwrap();
    h.svc.trust_store().trust(&stranger.public_key_b64(), "Stranger", "test").await.unwrap();
    let r = h.svc.install(InstallPluginParams { package_base64: Some(b64(&bytes)), ..Default::default() }).await;
    assert!(matches!(r, Err(PluginError::Signature(m)) if m.contains("another publisher key")));
    let audits = h
        .sec
        .audit
        .query(orkestr_security::AuditQuery { action_prefix: Some("plugins.install".into()), ..Default::default() })
        .await
        .unwrap();
    assert!(audits.iter().any(|a| a.event.outcome == orkestr_domain::AuditOutcome::Denied));
}

#[tokio::test(flavor = "multi_thread")]
async fn loop_traps_fast_and_repeated_traps_quarantine() {
    let h = harness().await;
    let id = h.install_enabled("evil-loop.orkp").await;
    for i in 0..3 {
        let t = Instant::now();
        let e = h.invoke(id, "spin").await.unwrap_err();
        assert!(t.elapsed() < Duration::from_secs(1), "took {:?}", t.elapsed());
        if i < 3 {
            assert!(matches!(&e, PluginError::Trap(m) if m.contains("timed out")), "{e}");
        }
    }
    let info = h.svc.get(id).await.unwrap();
    assert_eq!(info.plugin.state, PluginState::Quarantined);
    assert!(info.last_error.unwrap().contains("timed out"));
    assert!(matches!(h.invoke(id, "spin").await, Err(PluginError::Conflict(_))));
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_bomb_is_capped() {
    let h = harness().await;
    let id = h.install_enabled("evil-memory.orkp").await;
    let e = h.invoke(id, "bomb").await.unwrap_err();
    assert!(matches!(&e, PluginError::Trap(m) if m.contains("memory limit of 16 MiB")), "{e}");
}

#[tokio::test(flavor = "multi_thread")]
async fn undeclared_network_is_denied_and_audited() {
    let h = harness().await;
    let mut sub = h.events.subscribe_all();
    let id = h.install_enabled("evil-network.orkp").await;
    let e = h.invoke(id, "steal").await.unwrap_err();
    assert!(
        matches!(&e, PluginError::Invalid(m) if m.contains("evil.example.com") && m.contains("not granted")),
        "{e}"
    );
    let k = kinds(&mut sub);
    assert!(k.contains(&"PluginCallDenied".to_owned()), "{k:?}");
    let audits = h
        .sec
        .audit
        .query(orkestr_security::AuditQuery {
            action_prefix: Some("plugins.host.HttpFetch".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert!(audits
        .iter()
        .any(|a| a.event.outcome == orkestr_domain::AuditOutcome::Denied && a.event.resource == "evil.example.com"));
}

#[tokio::test(flavor = "multi_thread")]
async fn host_enforces_grants_for_fetch_kv_secrets_and_rpc() {
    let h = harness().await;
    // Tiny HTTP server.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        while let Ok((mut s, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = s.read(&mut buf).await;
            let _ = s.write_all(b"HTTP/1.1 200 OK\r\ncontent-length: 2\r\nconnection: close\r\n\r\nhi").await;
        }
    });
    let mut m = fixtures::hello_manifest();
    m.id = "orkestr-dev.fetcher".into();
    m.kinds = vec![orkestr_domain::PluginKind::Tool];
    m.widgets.clear();
    m.permissions = vec![
        orkestr_domain::PermissionRequest {
            capability: Capability::Network,
            scope: Some("127.0.0.1".into()),
            reason: "t".into(),
        },
        orkestr_domain::PermissionRequest {
            capability: Capability::Read,
            scope: Some("projects.list".into()),
            reason: "t".into(),
        },
        orkestr_domain::PermissionRequest {
            capability: Capability::Secrets,
            scope: Some("API_TOKEN".into()),
            reason: "t".into(),
        },
    ];
    let files = BTreeMap::from([("plugin.wasm".to_owned(), wat::to_wasm(&wat::hello()).unwrap())]);
    let bytes = package::build(&m, &files, Some(&fixtures::dev_key())).unwrap();
    let r =
        h.svc.install(InstallPluginParams { package_base64: Some(b64(&bytes)), ..Default::default() }).await.unwrap();
    let id = r.plugin.plugin.id;
    // Allow network + rpc, deny the secret.
    let decisions = r
        .pending_grants
        .iter()
        .map(|p| GrantDecision {
            capability: p.capability,
            scope: p.scope.clone(),
            allow: p.capability != Capability::Secrets,
        })
        .collect();
    h.svc.grant(GrantPluginParams { plugin_id: id, decisions, enable: true }).await.unwrap();
    let host = h.svc.host_for(None);
    let ok = host.call(&id, HostOp::HttpFetch, json!({"url": format!("http://127.0.0.1:{port}/x")})).await.unwrap();
    assert_eq!(ok["status"], 200);
    assert_eq!(ok["body"], "hi");
    assert!(host.call(&id, HostOp::HttpFetch, json!({"url": "http://localhost:1/x"})).await.is_err());
    assert!(host.call(&id, HostOp::HttpFetch, json!({"url": "file:///etc/passwd"})).await.is_err());
    // KV is private per plugin.
    host.call(&id, HostOp::KvSet, json!({"key": "k", "value": {"n": 1}})).await.unwrap();
    assert_eq!(host.call(&id, HostOp::KvGet, json!({"key": "k"})).await.unwrap(), json!({"n": 1}));
    // Denied secret; ungranted / non-read / local-only RPC refused.
    assert!(matches!(
        host.call(&id, HostOp::SecretGet, json!({"name": "API_TOKEN"})).await,
        Err(PluginError::Denied(_))
    ));
    assert!(matches!(host.call(&id, HostOp::SecretGet, json!({"name": "OTHER"})).await, Err(PluginError::Denied(_))));
    assert!(matches!(
        host.call(&id, HostOp::RpcRead, json!({"method": "tasks.list"})).await,
        Err(PluginError::Denied(_))
    ));
    assert!(matches!(
        host.call(&id, HostOp::RpcRead, json!({"method": "plugins.install"})).await,
        Err(PluginError::Denied(_))
    ));
    // Granted read without an installed reader: explicit unimplemented.
    assert!(matches!(
        host.call(&id, HostOp::RpcRead, json!({"method": "projects.list"})).await,
        Err(PluginError::Unimplemented(_))
    ));
    struct Reader;
    #[async_trait::async_trait]
    impl orkestr_plugins::RpcReader for Reader {
        async fn read(
            &self,
            method: &str,
            _p: Value,
            _project: Option<orkestr_domain::ProjectId>,
        ) -> Result<Value, String> {
            Ok(json!({ "method": method }))
        }
    }
    h.svc.set_rpc_reader(Arc::new(Reader));
    assert_eq!(
        host.call(&id, HostOp::RpcRead, json!({"method": "projects.list"})).await.unwrap()["method"],
        "projects.list"
    );
    // Unknown plugin.
    assert!(host.call(&orkestr_domain::PluginId::new(), HostOp::Log, json!({})).await.is_err());
}

#[tokio::test(flavor = "multi_thread")]
async fn kill_all_stops_running_calls() {
    let h = harness().await;
    let mut m = orkestr_plugins::package::parse(&h.fixtures["evil-loop.orkp"].bytes).unwrap().manifest;
    m.id = "orkestr-dev.long-loop".into();
    m.limits = PluginLimits { timeout_ms: 30_000, fuel_per_call: package::MAX_FUEL, ..PluginLimits::default() };
    let files = BTreeMap::from([("plugin.wasm".to_owned(), wat::to_wasm(&wat::infinite_loop()).unwrap())]);
    let bytes = package::build(&m, &files, Some(&fixtures::dev_key())).unwrap();
    let r =
        h.svc.install(InstallPluginParams { package_base64: Some(b64(&bytes)), ..Default::default() }).await.unwrap();
    let id = r.plugin.plugin.id;
    h.svc.set_enabled(id, true, None).await.unwrap();
    let svc = Arc::clone(&h.svc);
    let call = tokio::spawn(async move {
        svc.invoke(InvokePluginParams { plugin_id: id, tool: "spin".into(), input: json!({}), project_id: None }).await
    });
    let t = Instant::now();
    while h.svc.running_calls() == 0 {
        assert!(t.elapsed() < Duration::from_secs(10), "call never started");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let mut sub = h.events.subscribe_all();
    let res = h.svc.kill(None, "test").await;
    assert_eq!(res.calls_interrupted, 1);
    assert_eq!(res.killed, 1);
    let out = tokio::time::timeout(Duration::from_secs(2), call).await.expect("call not interrupted").unwrap();
    assert!(matches!(out, Err(PluginError::Killed)), "{out:?}");
    assert_eq!(h.svc.get(id).await.unwrap().plugin.state, PluginState::Killed);
    assert!(matches!(h.invoke(id, "spin").await, Err(PluginError::Killed)));
    assert!(kinds(&mut sub).contains(&"PluginStateChanged".to_owned()));
    assert!(h.svc.tool_catalog().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn safe_mode_survives_restart() {
    let h = harness().await;
    let id = h.install_enabled("hello-tool.orkp").await;
    h.invoke(id, "hello").await.unwrap();
    h.svc.set_safe_mode(true).await.unwrap();
    assert!(matches!(h.invoke(id, "hello").await, Err(PluginError::SafeMode)));
    // "Restart": a new service over the same database.
    let svc2 = PluginService::new(PluginDeps {
        db: h.db.clone(),
        events: h.events.clone(),
        security: Arc::clone(&h.sec),
        data_dir: h.data.clone(),
    });
    svc2.init().await.unwrap();
    assert!(svc2.safe_mode());
    assert!(svc2.tool_catalog().is_empty());
    assert!(svc2.widgets().await.unwrap().is_empty());
    let r = svc2
        .invoke(InvokePluginParams { plugin_id: id, tool: "hello".into(), input: json!({}), project_id: None })
        .await;
    assert!(matches!(r, Err(PluginError::SafeMode)));
    svc2.set_safe_mode(false).await.unwrap();
    let r = svc2
        .invoke(InvokePluginParams { plugin_id: id, tool: "hello".into(), input: json!({}), project_id: None })
        .await;
    assert!(r.is_ok(), "{r:?}");
}

#[test]
fn fixtures_write_to_disk() {
    let dir = tempfile::tempdir().unwrap();
    let files = fixtures::write_all(dir.path()).unwrap();
    assert!(files.iter().any(|f| f.ends_with("hello-tool.orkp")));
    let idx: Value = serde_json::from_slice(&std::fs::read(dir.path().join("fixtures.json")).unwrap()).unwrap();
    assert_eq!(idx["publicKey"], fixtures::dev_key().public_key_b64());
}

#[tokio::test(flavor = "multi_thread")]
async fn widget_plugin_serves_its_html() {
    let h = harness().await;
    let r = h.install("hello-widget.orkp").await.unwrap();
    assert!(r.pending_grants.is_empty());
    let id = r.plugin.plugin.id;
    assert!(matches!(
        h.svc.widget_html("orkestr-dev.hello-widget", "hello-widget").await,
        Err(PluginError::Conflict(_))
    ));
    h.svc.set_enabled(id, true, None).await.unwrap();
    let html = h.svc.widget_html("orkestr-dev.hello-widget", "hello-widget").await.unwrap();
    assert!(html.contains("Hello widget"));
    assert_eq!(h.svc.widget_html(&id.to_string(), "hello-widget").await.unwrap(), html);
    assert!(matches!(h.svc.widget_html(&id.to_string(), "nope").await, Err(PluginError::NotFound(_))));
    let w = h.svc.widgets().await.unwrap();
    assert_eq!(w[0].widget.data_binding.methods, vec!["projects.list".to_owned()]);
    h.svc.set_safe_mode(true).await.unwrap();
    assert!(matches!(h.svc.widget_html(&id.to_string(), "hello-widget").await, Err(PluginError::SafeMode)));
}
