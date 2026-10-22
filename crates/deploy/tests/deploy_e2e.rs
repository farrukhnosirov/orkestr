//! End-to-end deployment tests on localhost (static site, local process),
//! approvals, credentials, redaction, cancel, emergency cancel_all.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use orkestr_deploy::{AdapterAvailability, DeployDeps, DeployError, DeployService};
use orkestr_domain::{DeploymentStatus, DeploymentTarget, EnvironmentKind, ProjectId, SecretScope};
use orkestr_events::{EventBus, Received};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::deploy::{
    CreateEnvironmentParams, DeployParams, EnvironmentRecord, RollbackDeploymentParams,
};
use orkestr_security::approvals::ApprovalDecision;
use orkestr_security::{GuardConfig, SecretBackendChoice, Security, SecurityConfig};
use orkestr_storage::{Compression, Db, FsObjectStore};
use parking_lot::Mutex;

struct H {
    svc: DeployService,
    sec: Arc<Security>,
    events: Arc<Mutex<Vec<String>>>,
    project: ProjectId,
    root: tempfile::TempDir,
    _data: tempfile::TempDir,
}

async fn harness() -> H {
    let data = tempfile::tempdir().unwrap();
    let db = Db::open_in_memory().unwrap();
    let mut migs = orkestr_security::migrations();
    migs.extend(orkestr_deploy::deploy_migrations());
    db.migrate(migs).await.unwrap();
    let bus = EventBus::in_memory(4096);
    let sec = Arc::new(
        Security::open(SecurityConfig {
            db: db.clone(),
            events: bus.clone(),
            data_dir: data.path().join("sec"),
            credentials: Some(Arc::new(orkestr_platform::MemoryCredentialStore::default())),
            secret_backend: SecretBackendChoice::Memory,
            guard: GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let objects = Arc::new(FsObjectStore::open(data.path().join("objects"), Compression::None).unwrap());
    let svc = DeployService::new(DeployDeps {
        db,
        events: bus.clone(),
        security: Arc::clone(&sec),
        objects,
        data_dir: data.path().join("deploy"),
    });
    let events = Arc::new(Mutex::new(Vec::new()));
    let mut sub = bus.subscribe_all();
    let ev = Arc::clone(&events);
    tokio::spawn(async move {
        while let Some(r) = sub.recv().await {
            if let Received::Event(e) = r {
                let name = match &e.kind {
                    EventKind::DeploymentStarted(_) => "DeploymentStarted",
                    EventKind::DeploymentProgress(_) => "DeploymentProgress",
                    EventKind::DeploymentFinished(_) => "DeploymentFinished",
                    EventKind::ApprovalRequested(_) => "ApprovalRequested",
                    _ => continue,
                };
                ev.lock().push(name.to_owned());
            }
        }
    });
    H { svc, sec, events, project: ProjectId::new(), root: tempfile::tempdir().unwrap(), _data: data }
}

fn free_port() -> u16 {
    std::net::TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
}

fn write(root: &Path, rel: &str, text: &str) {
    let p = root.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, text).unwrap();
}

async fn env(
    h: &H,
    name: &str,
    kind: EnvironmentKind,
    target: DeploymentTarget,
    settings: &[(&str, &str)],
    secret_refs: Vec<orkestr_domain::SecretRefId>,
) -> EnvironmentRecord {
    h.svc
        .create_environment(
            CreateEnvironmentParams {
                project_id: h.project,
                name: name.into(),
                kind,
                protected: false,
                variables: BTreeMap::new(),
                secret_refs,
                urls: Vec::new(),
                target: Some(target),
                adapter_settings: settings.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect(),
            },
            h.sec.user(),
        )
        .await
        .unwrap()
}

async fn settle(h: &H, id: orkestr_domain::DeploymentId) -> orkestr_domain::Deployment {
    for _ in 0..1200 {
        let d = h.svc.get(id).await.unwrap().deployment;
        if matches!(
            d.status,
            DeploymentStatus::Succeeded
                | DeploymentStatus::Failed
                | DeploymentStatus::Cancelled
                | DeploymentStatus::RolledBack
        ) && !h.svc.active().contains(&id)
        {
            return d;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("deployment {id} did not finish");
}

async fn wait_status(h: &H, id: orkestr_domain::DeploymentId, s: DeploymentStatus) {
    for _ in 0..600 {
        if h.svc.get(id).await.unwrap().deployment.status == s {
            return;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("deployment {id} never reached {s:?}");
}

async fn all_logs(h: &H, id: orkestr_domain::DeploymentId) -> String {
    h.svc.logs(id, 0).await.unwrap().lines.join("\n")
}

async fn fetch(url: &str) -> String {
    reqwest::get(url).await.unwrap().text().await.unwrap()
}

fn params(env: &EnvironmentRecord, version: &str) -> DeployParams {
    DeployParams { environment_id: env.environment.id, version: Some(version.into()), source_dir: None }
}

#[tokio::test]
async fn static_site_deploys_serves_and_rolls_back() {
    let h = harness().await;
    let port = free_port();
    let e = env(
        &h,
        "local",
        EnvironmentKind::Local,
        DeploymentTarget::StaticSite,
        &[("port", &port.to_string()), ("outputDir", "dist"), ("healthPath", "/index.html")],
        Vec::new(),
    )
    .await;
    let root = h.root.path();
    write(root, "dist/index.html", "version one");
    write(root, "dist/assets/app.css", "body{}");

    let plan = h.svc.plan(&params(&e, "v1"), root).await.unwrap();
    assert!(!plan.requires_approval);
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);

    let r1 = h.svc.start(&params(&e, "v1"), root, h.sec.user()).await.unwrap();
    assert!(r1.approval_id.is_none());
    let d1 = settle(&h, r1.deployment.id).await;
    assert_eq!(d1.status, DeploymentStatus::Succeeded, "{}", all_logs(&h, d1.id).await);
    let url = d1.url.clone().unwrap();
    assert_eq!(fetch(&url).await, "version one");
    assert_eq!(fetch(&format!("{url}assets/app.css")).await, "body{}");
    assert_eq!(reqwest::get(format!("{url}../../secret")).await.unwrap().status().as_u16(), 404);
    assert!(d1.log_object_id.is_some(), "logs are stored in the object store");
    let logs = h.svc.logs(d1.id, 0).await.unwrap();
    assert!(logs.complete && !logs.lines.is_empty());

    write(root, "dist/index.html", "version two");
    let d2 = settle(&h, h.svc.start(&params(&e, "v2"), root, h.sec.user()).await.unwrap().deployment.id).await;
    assert_eq!(d2.status, DeploymentStatus::Succeeded);
    assert_eq!(fetch(&url).await, "version two");
    assert_eq!(h.svc.current_version(e.environment.id).await.unwrap().as_deref(), Some("v2"));
    assert_eq!(h.svc.previous_version(e.environment.id).await.unwrap().as_deref(), Some("v1"));
    let recent = h.svc.recent(10).await.unwrap();
    assert_eq!(recent[0].deployment_id, d2.id);
    assert_eq!(recent[0].name, "local v2");

    let rb = h
        .svc
        .rollback(&RollbackDeploymentParams { environment_id: e.environment.id, to_version: None }, root, h.sec.user())
        .await
        .unwrap();
    assert_eq!(rb.deployment.version.as_deref(), Some("v1"));
    let rb = settle(&h, rb.deployment.id).await;
    assert_eq!(rb.status, DeploymentStatus::Succeeded, "{}", all_logs(&h, rb.id).await);
    assert_eq!(fetch(&url).await, "version one", "rollback restores the previous version");
    assert_eq!(h.svc.get(d2.id).await.unwrap().deployment.status, DeploymentStatus::RolledBack);
    assert_eq!(h.svc.current_version(e.environment.id).await.unwrap().as_deref(), Some("v1"));
    assert_eq!(h.svc.list(h.project, Some(e.environment.id), None).await.unwrap().len(), 3);

    let names = h.events.lock().clone();
    assert!(names.iter().any(|n| n == "DeploymentStarted"));
    assert!(names.iter().filter(|n| *n == "DeploymentFinished").count() >= 3);

    // Environment listing carries the latest deployment.
    let envs = h.svc.list_environments(h.project).await.unwrap();
    assert_eq!(envs[0].last_deployment.as_ref().map(|d| d.id), Some(rb.id));
    h.svc.delete_environment(e.environment.id, h.sec.user()).await.unwrap();
}

fn node() -> bool {
    orkestr_platform::process::which("node").is_some()
}

const SERVER_JS: &str = r#"
const http = require('http');
const fs = require('fs');
const v = fs.readFileSync(__dirname + '/VERSION', 'utf8').trim();
console.log('starting ' + v + ' with secret ' + process.env.APP_SECRET);
http.createServer((q, s) => { s.end(v); }).listen(Number(process.env.PORT), '127.0.0.1');
"#;

#[tokio::test]
async fn local_process_deploys_redacts_and_rolls_back() {
    if !node() {
        eprintln!("skipped: node is not on PATH");
        return;
    }
    let h = harness().await;
    let secret = "app-secret-value-7f3a91";
    let sref = h
        .sec
        .secrets
        .set(
            "APP_SECRET",
            SecretScope::Project { project_id: h.project },
            zeroize::Zeroizing::new(secret.into()),
            h.sec.user(),
        )
        .await
        .unwrap();
    let port = free_port();
    let health = format!("http://127.0.0.1:{port}/");
    let e = env(
        &h,
        "staging",
        EnvironmentKind::Staging,
        DeploymentTarget::LocalProcess,
        &[("command", "node server.js"), ("workdir", "app"), ("port", &port.to_string()), ("healthUrl", &health)],
        vec![sref.id],
    )
    .await;
    let root = h.root.path();
    write(root, "app/server.js", SERVER_JS);
    write(root, "app/VERSION", "one");
    let d1 = settle(&h, h.svc.start(&params(&e, "v1"), root, h.sec.user()).await.unwrap().deployment.id).await;
    let logs = all_logs(&h, d1.id).await;
    assert_eq!(d1.status, DeploymentStatus::Succeeded, "{logs}");
    assert_eq!(fetch(&health).await, "one");
    assert!(!logs.contains(secret), "secret values never reach logs: {logs}");
    assert!(logs.contains("[REDACTED]"), "{logs}");

    write(root, "app/VERSION", "two");
    let d2 = settle(&h, h.svc.start(&params(&e, "v2"), root, h.sec.user()).await.unwrap().deployment.id).await;
    assert_eq!(d2.status, DeploymentStatus::Succeeded, "{}", all_logs(&h, d2.id).await);
    assert_eq!(fetch(&health).await, "two");

    let rb = h
        .svc
        .rollback(&RollbackDeploymentParams { environment_id: e.environment.id, to_version: None }, root, h.sec.user())
        .await
        .unwrap();
    let rb = settle(&h, rb.deployment.id).await;
    assert_eq!(rb.status, DeploymentStatus::Succeeded, "{}", all_logs(&h, rb.id).await);
    assert_eq!(fetch(&health).await, "one", "rollback restarts the previous version");
    assert_eq!(h.svc.local_process().running_version(e.environment.id).as_deref(), Some("v1"));
    h.svc.local_process().stop(e.environment.id).await;
}

#[tokio::test]
async fn production_deploy_waits_for_approval() {
    let h = harness().await;
    let port = free_port();
    let e = env(
        &h,
        "prod",
        EnvironmentKind::Production,
        DeploymentTarget::StaticSite,
        &[("port", &port.to_string())],
        Vec::new(),
    )
    .await;
    assert!(e.environment.protected, "production is always protected");
    let root = h.root.path();
    write(root, "dist/index.html", "prod");
    let plan = h.svc.plan(&params(&e, "p1"), root).await.unwrap();
    assert!(plan.requires_approval);

    // Rejected → cancelled, nothing deployed.
    let r = h.svc.start(&params(&e, "p0"), root, h.sec.user()).await.unwrap();
    assert_eq!(r.deployment.status, DeploymentStatus::AwaitingApproval);
    let a = r.approval_id.unwrap();
    h.sec.approvals.resolve(a, ApprovalDecision::Reject, None, h.sec.user()).await.unwrap();
    let d = settle(&h, r.deployment.id).await;
    assert_eq!(d.status, DeploymentStatus::Cancelled);
    assert!(h.svc.static_site().serving_port(e.environment.id).is_none());

    // Approved → runs.
    let r = h.svc.start(&params(&e, "p1"), root, h.sec.user()).await.unwrap();
    assert_eq!(r.deployment.status, DeploymentStatus::AwaitingApproval);
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(h.svc.get(r.deployment.id).await.unwrap().deployment.status, DeploymentStatus::AwaitingApproval);
    assert!(h.events.lock().iter().any(|n| n == "ApprovalRequested"));
    h.sec.approvals.resolve(r.approval_id.unwrap(), ApprovalDecision::Approve, None, h.sec.user()).await.unwrap();
    let d = settle(&h, r.deployment.id).await;
    assert_eq!(d.status, DeploymentStatus::Succeeded, "{}", all_logs(&h, d.id).await);
    assert_eq!(fetch(d.url.as_deref().unwrap()).await, "prod");
    h.svc.static_site().stop(e.environment.id);
}

#[tokio::test]
async fn cloud_adapters_need_credentials() {
    let h = harness().await;
    let infos = h.svc.adapter_infos().await.unwrap();
    for (id, secret) in [
        ("vercel", "VERCEL_TOKEN"),
        ("netlify", "NETLIFY_AUTH_TOKEN"),
        ("fly", "FLY_API_TOKEN"),
        ("kubernetes", "KUBE_TOKEN"),
    ] {
        let a = infos.iter().find(|a| a.id == id).unwrap();
        assert_eq!(a.availability, AdapterAvailability::NeedsCredential { secret: secret.into() });
        assert_eq!(a.verification, "requestShape");
    }
    for id in ["staticSite", "localProcess"] {
        let a = infos.iter().find(|a| a.id == id).unwrap();
        assert_eq!(a.availability, AdapterAvailability::Ready);
        assert_eq!(a.verification, "live");
    }
    // Docker is gated: Ready only when the CLI and daemon answer.
    let docker = infos.iter().find(|a| a.id == "docker").unwrap();
    assert!(matches!(docker.availability, AdapterAvailability::Ready | AdapterAvailability::Unavailable { .. }));

    let e =
        env(&h, "vercel", EnvironmentKind::Staging, DeploymentTarget::Vercel, &[("project", "site")], Vec::new()).await;
    let plan = h.svc.plan(&params(&e, "v1"), h.root.path()).await.unwrap();
    assert!(plan.warnings.iter().any(|w| w.contains("VERCEL_TOKEN")), "{:?}", plan.warnings);
    let err = h.svc.start(&params(&e, "v1"), h.root.path(), h.sec.user()).await.unwrap_err();
    assert!(matches!(err, DeployError::Unavailable(ref m) if m.contains("VERCEL_TOKEN")), "{err}");

    // Tokens never live in settings.
    let bad = h
        .svc
        .create_environment(
            CreateEnvironmentParams {
                project_id: h.project,
                name: "bad".into(),
                kind: EnvironmentKind::Staging,
                protected: false,
                variables: BTreeMap::new(),
                secret_refs: Vec::new(),
                urls: Vec::new(),
                target: Some(DeploymentTarget::Vercel),
                adapter_settings: [("token".to_owned(), "abc".to_owned())].into_iter().collect(),
            },
            h.sec.user(),
        )
        .await;
    assert!(matches!(bad, Err(DeployError::Invalid(_))));
}

#[tokio::test]
async fn cancel_running_and_cancel_all() {
    if !node() {
        eprintln!("skipped: node is not on PATH");
        return;
    }
    let h = harness().await;
    let root = h.root.path();
    write(root, "dist/index.html", "x");
    let port = free_port();
    let e = env(
        &h,
        "slow",
        EnvironmentKind::Dev,
        DeploymentTarget::StaticSite,
        &[("port", &port.to_string()), ("buildCommand", "node -e \"setTimeout(() => {}, 60000)\"")],
        Vec::new(),
    )
    .await;
    let r = h.svc.start(&params(&e, "s1"), root, h.sec.user()).await.unwrap();
    wait_status(&h, r.deployment.id, DeploymentStatus::Running).await;
    let started = std::time::Instant::now();
    let c = h.svc.cancel(r.deployment.id, h.sec.user()).await.unwrap();
    assert_eq!(c.deployment.status, DeploymentStatus::Cancelled);
    assert!(started.elapsed() < Duration::from_secs(20));

    // Emergency stop cancels deployments awaiting approval too.
    let p = env(
        &h,
        "prod",
        EnvironmentKind::Production,
        DeploymentTarget::StaticSite,
        &[("port", &free_port().to_string())],
        Vec::new(),
    )
    .await;
    let r = h.svc.start(&params(&p, "p1"), root, h.sec.user()).await.unwrap();
    assert_eq!(r.deployment.status, DeploymentStatus::AwaitingApproval);
    assert_eq!(h.svc.cancel_all().await, 1);
    let d = settle(&h, r.deployment.id).await;
    assert_eq!(d.status, DeploymentStatus::Cancelled);
    let a = h.sec.approvals.get(r.approval_id.unwrap()).await.unwrap().unwrap();
    assert_eq!(a.status, orkestr_domain::ApprovalStatus::Cancelled);
}
