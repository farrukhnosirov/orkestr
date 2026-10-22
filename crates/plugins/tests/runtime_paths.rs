//! Plugin runtime paths (ADR-0034 §12-13): `subscribes` event delivery
//! (grant, bounded queue, kill switch) and `DeploymentProvider` plugins as
//! deploy adapters (register / plan / deploy / rollback / logs / cancel /
//! unregister).

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use orkestr_deploy::{AdapterAvailability, DeployDeps, DeployService};
use orkestr_domain::{
    now, Capability, DeploymentStatus, DeploymentTarget, EnvironmentKind, PluginId, PluginManifest, PluginState,
    ProjectId, SecretScope, Task,
};
use orkestr_events::EventBus;
use orkestr_plugins::fixtures::{self, Fixture};
use orkestr_plugins::{package, wat, PluginDeps, PluginService};
use orkestr_protocol::event::{EventKind, ProjectPayload, TaskPayload};
use orkestr_protocol::methods::deploy::{
    CreateEnvironmentParams, DeployParams, EnvironmentRecord, RollbackDeploymentParams,
};
use orkestr_protocol::methods::plugins::{GrantDecision, GrantPluginParams, InstallPluginParams, InvokePluginParams};
use orkestr_protocol::NewEvent;
use orkestr_security::Security;
use orkestr_storage::{Compression, Db, FsObjectStore};
use serde_json::{json, Value};

struct H {
    _dir: tempfile::TempDir,
    root: tempfile::TempDir,
    events: EventBus,
    sec: Arc<Security>,
    svc: Arc<PluginService>,
    deploy: DeployService,
    project: ProjectId,
    fixtures: BTreeMap<&'static str, Fixture>,
}

async fn harness() -> H {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("o.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    migs.extend(orkestr_plugins::plugin_migrations());
    migs.extend(orkestr_deploy::deploy_migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(8192);
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
    let svc = Arc::new(PluginService::new(PluginDeps {
        db: db.clone(),
        events: events.clone(),
        security: Arc::clone(&sec),
        data_dir: dir.path().join("data").join("plugins"),
    }));
    let objects = Arc::new(FsObjectStore::open(dir.path().join("objects"), Compression::None).unwrap());
    let deploy = DeployService::new(DeployDeps {
        db,
        events: events.clone(),
        security: Arc::clone(&sec),
        objects,
        data_dir: dir.path().join("data").join("deploy"),
    });
    let key = fixtures::dev_key();
    svc.trust_store().trust(&key.public_key_b64(), "Orkestr Dev", "test").await.unwrap();
    svc.start_event_delivery();
    svc.attach_deploy(deploy.clone()).await.unwrap();
    let fixtures = fixtures::all(&key).unwrap().into_iter().map(|f| (f.file, f)).collect();
    H { _dir: dir, root: tempfile::tempdir().unwrap(), events, sec, svc, deploy, project: ProjectId::new(), fixtures }
}

fn b64(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn custom(mut m: PluginManifest, id: &str, wat_src: &str, timeout_ms: u32) -> Vec<u8> {
    m.id = format!("orkestr-dev.{id}");
    m.limits.timeout_ms = timeout_ms;
    let files = BTreeMap::from([("plugin.wasm".to_owned(), wat::to_wasm(wat_src).unwrap())]);
    package::build(&m, &files, Some(&fixtures::dev_key())).unwrap()
}

impl H {
    async fn install_bytes(&self, bytes: &[u8], allow: bool) -> PluginId {
        let r = self
            .svc
            .install(InstallPluginParams { path: None, package_base64: Some(b64(bytes)), source: Some("test".into()) })
            .await
            .unwrap();
        let decisions = r
            .pending_grants
            .iter()
            .map(|p| GrantDecision { capability: p.capability, scope: p.scope.clone(), allow })
            .collect();
        let id = r.plugin.plugin.id;
        let info = self.svc.grant(GrantPluginParams { plugin_id: id, decisions, enable: true }).await.unwrap();
        assert_eq!(info.plugin.state, PluginState::Enabled);
        id
    }

    async fn install(&self, file: &str, allow: bool) -> PluginId {
        let bytes = self.fixtures[file].bytes.clone();
        self.install_bytes(&bytes, allow).await
    }

    async fn task_created(&self) {
        let task = Task::new(self.project, "t", now());
        self.events.publish(NewEvent::new(EventKind::TaskCreated(TaskPayload { task }))).await.unwrap();
    }

    async fn count(&self, id: PluginId) -> Value {
        self.svc
            .invoke(InvokePluginParams { plugin_id: id, tool: "count".into(), input: json!({}), project_id: None })
            .await
            .unwrap()
            .output
    }

    fn has_target(&self, t: &DeploymentTarget) -> bool {
        self.deploy.adapters().iter().any(|a| &a.target() == t)
    }

    async fn env(&self, target: DeploymentTarget, secret_refs: Vec<orkestr_domain::SecretRefId>) -> EnvironmentRecord {
        self.deploy
            .create_environment(
                CreateEnvironmentParams {
                    project_id: self.project,
                    name: "plugin-env".into(),
                    kind: EnvironmentKind::Staging,
                    protected: false,
                    variables: BTreeMap::new(),
                    secret_refs,
                    urls: Vec::new(),
                    target: Some(target),
                    adapter_settings: BTreeMap::from([("site".to_owned(), "demo".to_owned())]),
                },
                self.sec.user(),
            )
            .await
            .unwrap()
    }

    async fn settle(&self, id: orkestr_domain::DeploymentId) -> orkestr_domain::Deployment {
        for _ in 0..400 {
            let d = self.deploy.get(id).await.unwrap().deployment;
            if matches!(
                d.status,
                DeploymentStatus::Succeeded
                    | DeploymentStatus::Failed
                    | DeploymentStatus::Cancelled
                    | DeploymentStatus::RolledBack
            ) && !self.deploy.active().contains(&id)
            {
                return d;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        panic!("deployment {id} did not finish");
    }

    async fn logs(&self, id: orkestr_domain::DeploymentId) -> String {
        self.deploy.logs(id, 0).await.unwrap().lines.join("\n")
    }
}

async fn wait_until(what: &str, mut f: impl FnMut() -> bool) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn manifests_are_validated() {
    let files = BTreeMap::from([("plugin.wasm".to_owned(), b"\0asm".to_vec())]);
    let mut m = fixtures::subscriber_manifest();
    assert!(package::validate_manifest(&m, &files).is_ok());
    m.subscribes = vec!["TerminalOutput".into()];
    assert!(package::validate_manifest(&m, &files).is_err(), "sensitive kinds cannot be subscribed");
    m.subscribes = vec!["NoSuchKind".into()];
    assert!(package::validate_manifest(&m, &files).is_err());
    let mut d = fixtures::deployer_manifest();
    assert!(package::validate_manifest(&d, &files).is_ok());
    d.kinds = vec![orkestr_domain::PluginKind::Tool];
    d.tools = fixtures::hello_manifest().tools;
    assert!(package::validate_manifest(&d, &files).is_err(), "deployProviders need kind DeploymentProvider");
}

#[tokio::test(flavor = "multi_thread")]
async fn subscribe_needs_an_export() {
    let h = harness().await;
    // `subscribes` on a module without `orkestr_on_event` is refused at install.
    let bytes = custom(fixtures::subscriber_manifest(), "no-export", &wat::hello(), 5_000);
    let err = h
        .svc
        .install(InstallPluginParams { path: None, package_base64: Some(b64(&bytes)), source: None })
        .await
        .unwrap_err();
    assert!(err.to_string().contains("orkestr_on_event"), "{err}");
}

#[tokio::test(flavor = "multi_thread")]
async fn events_are_delivered_only_with_the_grant() {
    let h = harness().await;
    // Denied grant: enabled, but nothing is delivered.
    let id = h.install("hello-subscriber.orkp", false).await;
    h.task_created().await;
    wait_until("denied delivery", || h.svc.event_stats(id).denied == 1).await;
    assert_eq!(h.count(id).await, Value::Null);
    assert_eq!(h.svc.event_stats(id).delivered, 0);

    // Allow it: TaskCreated is delivered, other kinds are not.
    let grant = GrantDecision {
        capability: Capability::Read,
        scope: Some(orkestr_plugins::events::event_scope("TaskCreated")),
        allow: true,
    };
    h.svc.grant(GrantPluginParams { plugin_id: id, decisions: vec![grant], enable: false }).await.unwrap();
    for _ in 0..3 {
        h.task_created().await;
    }
    let project = orkestr_domain::Project::new("p", now());
    h.events.publish(NewEvent::new(EventKind::ProjectCreated(ProjectPayload { project }))).await.unwrap();
    wait_until("3 deliveries", || h.svc.event_stats(id).delivered == 3).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(h.count(id).await, json!(3));
    let s = h.svc.event_stats(id);
    assert_eq!((s.delivered, s.failed, s.denied, s.dropped), (3, 0, 1, 0), "{s:?}");

    // Audit summaries are written.
    let audits = h
        .sec
        .audit
        .query(orkestr_security::audit::AuditQuery::default())
        .await
        .unwrap()
        .into_iter()
        .filter(|a| a.event.action == "plugins.events.deliver")
        .count();
    assert!(audits >= 2, "{audits}");

    // Disable: no more delivery.
    h.svc.set_enabled(id, false, None).await.unwrap();
    h.task_created().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.svc.event_stats(id).delivered, 3);
}

#[tokio::test(flavor = "multi_thread")]
async fn queue_overflow_drops_oldest_and_kill_stops_delivery() {
    let h = harness().await;
    let bytes = custom(fixtures::subscriber_manifest(), "spin-subscriber", &wat::subscriber(true), 10_000);
    let id = h.install_bytes(&bytes, true).await;
    h.svc.set_event_queue_capacity(8);
    for _ in 0..40 {
        h.task_created().await;
    }
    // The first delivery spins; 39 queue up behind it, capacity 8.
    wait_until("drops", || h.svc.event_stats(id).dropped >= 31).await;
    wait_until("spinning delivery", || h.svc.running_calls() == 1).await;
    let s = h.svc.event_stats(id);
    assert!((7..=8).contains(&s.queued) && s.dropped + s.queued == 39, "{s:?}");

    let t = std::time::Instant::now();
    let r = h.svc.kill(Some(id), "test").await;
    assert_eq!(r.calls_interrupted, 1);
    wait_until("interrupted", || h.svc.running_calls() == 0).await;
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!(h.svc.event_stats(id).queued, 0, "kill clears the queue");
    for _ in 0..5 {
        h.task_created().await;
    }
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.svc.running_calls(), 0, "a killed plugin gets no events");
    assert_eq!(h.svc.event_stats(id).queued, 0);
    assert_eq!(h.svc.get(id).await.unwrap().plugin.state, PluginState::Killed);

    // Safe mode stops delivery too.
    h.svc.set_enabled(id, true, None).await.unwrap();
    h.svc.set_safe_mode(true).await.unwrap();
    h.task_created().await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.svc.running_calls(), 0);
    h.svc.set_safe_mode(false).await.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_deploy_provider_end_to_end() {
    let h = harness().await;
    let target = DeploymentTarget::Plugin("orkestr-dev.hello-deployer/hello".into());
    let r = h
        .svc
        .install(InstallPluginParams {
            path: None,
            package_base64: Some(b64(&h.fixtures["hello-deployer.orkp"].bytes)),
            source: None,
        })
        .await
        .unwrap();
    assert!(!h.has_target(&target), "not registered before enable");
    let id = r.plugin.plugin.id;
    let decisions =
        vec![GrantDecision { capability: Capability::Secrets, scope: Some(wat::DEPLOYER_SECRET.into()), allow: true }];
    h.svc.grant(GrantPluginParams { plugin_id: id, decisions, enable: true }).await.unwrap();
    assert!(h.has_target(&target));
    assert_eq!(h.svc.deploy_targets(), vec![target.clone()]);
    let mut ready = false;
    for _ in 0..100 {
        let infos = h.deploy.adapter_infos().await.unwrap();
        let info = infos.iter().find(|a| a.target == target).unwrap();
        assert_eq!(info.id, "plugin:orkestr-dev.hello-deployer/hello");
        if info.availability == AdapterAvailability::Ready {
            ready = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(ready);

    let secret = "hd-token-value-93c1f7";
    let sref = h
        .sec
        .secrets
        .set(
            wat::DEPLOYER_SECRET,
            SecretScope::Project { project_id: h.project },
            zeroize::Zeroizing::new(secret.into()),
            h.sec.user(),
        )
        .await
        .unwrap();
    let env = h.env(target.clone(), vec![sref.id]).await;
    let params =
        |v: &str| DeployParams { environment_id: env.environment.id, version: Some(v.into()), source_dir: None };
    let plan = h.deploy.plan(&params("v1"), h.root.path()).await.unwrap();
    assert_eq!(plan.adapter_id, "plugin:orkestr-dev.hello-deployer/hello");
    assert_eq!(plan.steps.len(), 3, "{plan:?}");
    assert!(!format!("{plan:?}").contains(secret));

    let d1 = h.settle(h.deploy.start(&params("v1"), h.root.path(), h.sec.user()).await.unwrap().deployment.id).await;
    let logs = h.logs(d1.id).await;
    assert_eq!(d1.status, DeploymentStatus::Succeeded, "{logs}");
    assert_eq!(d1.url.as_deref(), Some(wat::DEPLOYER_URL));
    assert!(logs.contains("step 1/3") && logs.contains("step 3/3"), "{logs}");
    assert!(logs.contains("using token [REDACTED]"), "the granted secret is read and redacted: {logs}");
    assert!(!logs.contains(secret), "{logs}");
    let d2 = h.settle(h.deploy.start(&params("v2"), h.root.path(), h.sec.user()).await.unwrap().deployment.id).await;
    assert_eq!(d2.status, DeploymentStatus::Succeeded);
    let rb = h
        .deploy
        .rollback(
            &RollbackDeploymentParams { environment_id: env.environment.id, to_version: None },
            h.root.path(),
            h.sec.user(),
        )
        .await
        .unwrap();
    let rb = h.settle(rb.deployment.id).await;
    let logs = h.logs(rb.id).await;
    assert_eq!(rb.status, DeploymentStatus::Succeeded, "{logs}");
    assert_eq!(rb.version.as_deref(), Some("v1"));
    assert!(logs.contains("rolling back"), "{logs}");

    // Lifecycle: disable / kill / safe mode / uninstall unregister.
    h.svc.set_enabled(id, false, None).await.unwrap();
    assert!(!h.has_target(&target));
    h.svc.set_enabled(id, true, None).await.unwrap();
    assert!(h.has_target(&target));
    h.svc.kill(Some(id), "test").await;
    assert!(!h.has_target(&target));
    h.svc.set_enabled(id, true, None).await.unwrap();
    h.svc.set_safe_mode(true).await.unwrap();
    assert!(!h.has_target(&target));
    h.svc.set_safe_mode(false).await.unwrap();
    assert!(h.has_target(&target));
    h.svc.uninstall(id).await.unwrap();
    assert!(!h.has_target(&target));
    assert!(h.svc.deploy_targets().is_empty());
}

#[tokio::test(flavor = "multi_thread")]
async fn plugin_deploy_cancel_interrupts_the_guest() {
    let h = harness().await;
    let mut m = fixtures::deployer_manifest();
    m.deploy_providers = vec!["slow".into()];
    m.permissions.clear();
    let bytes = custom(m, "slow-deployer", &wat::deployer(true), 60_000);
    let id = h.install_bytes(&bytes, true).await;
    let target = DeploymentTarget::Plugin("orkestr-dev.slow-deployer/slow".into());
    assert!(h.has_target(&target));
    let env = h.env(target, Vec::new()).await;
    let p = DeployParams { environment_id: env.environment.id, version: Some("v1".into()), source_dir: None };
    let d = h.deploy.start(&p, h.root.path(), h.sec.user()).await.unwrap().deployment;
    wait_until("guest running", || h.svc.running_calls() == 1).await;
    let t = std::time::Instant::now();
    let r = h.deploy.cancel(d.id, h.sec.user()).await.unwrap();
    assert_eq!(r.deployment.status, DeploymentStatus::Cancelled, "{}", h.logs(d.id).await);
    assert!(t.elapsed() < Duration::from_secs(5), "{:?}", t.elapsed());
    assert_eq!(h.svc.running_calls(), 0);
    assert!(h.logs(d.id).await.contains("step 1/3"));
    // A cancel is not a trap: the plugin stays enabled.
    assert_eq!(h.svc.get(id).await.unwrap().plugin.state, PluginState::Enabled);
}
