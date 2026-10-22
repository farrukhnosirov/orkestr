//! Cloud adapters vs an in-test mock API: request shapes (paths, methods,
//! bodies, bearer auth, digests), status polling, rollback, and that the
//! token never appears in plans or logs.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use orkestr_deploy::adapters::{FlyAdapter, KubernetesAdapter, NetlifyAdapter, VercelAdapter};
use orkestr_deploy::{Credentials, DeployAdapter, DeployError, DeployRequest, LogSink, RollbackTarget};
use orkestr_domain::{
    now, Actor, Deployment, DeploymentId, DeploymentStatus, DeploymentTarget, Environment, EnvironmentId,
    EnvironmentKind, ProjectId,
};
use parking_lot::Mutex;
use serde_json::{json, Value};
use tokio_util::sync::CancellationToken;

const TOKEN: &str = "tok-SUPER-secret-0123456789";

#[derive(Debug, Clone)]
struct Rec {
    method: String,
    path: String,
    auth: Option<String>,
    headers: HeaderMap,
    body: Bytes,
}

#[derive(Clone, Default)]
struct Mock {
    log: Arc<Mutex<Vec<Rec>>>,
    polls: Arc<Mutex<std::collections::HashMap<String, u32>>>,
}

async fn handler(State(m): State<Mock>, method: Method, uri: Uri, headers: HeaderMap, body: Bytes) -> Response {
    let path = uri.path_and_query().map(|p| p.as_str().to_owned()).unwrap_or_default();
    let auth = headers.get("authorization").and_then(|v| v.to_str().ok()).map(str::to_owned);
    m.log.lock().push(Rec { method: method.to_string(), path: path.clone(), auth: auth.clone(), headers, body });
    if auth.as_deref() != Some(&format!("Bearer {TOKEN}")) {
        return (StatusCode::UNAUTHORIZED, Json(json!({ "error": "bad token" }))).into_response();
    }
    let first_poll = {
        let mut p = m.polls.lock();
        let n = p.entry(format!("{method} {}", uri.path())).or_insert(0);
        *n += 1;
        *n == 1
    };
    let p = uri.path();
    let v = match (method.as_str(), p) {
        ("POST", "/v2/files") => json!({}),
        ("POST", "/v13/deployments") => json!({ "id": "dpl_1", "url": "site-abc.vercel.app", "readyState": "QUEUED" }),
        ("GET", "/v13/deployments/dpl_1") => {
            json!({ "id": "dpl_1", "url": "site-abc.vercel.app", "readyState": if first_poll { "BUILDING" } else { "READY" } })
        }
        ("POST", "/v9/projects/site/rollback/dpl_0") => json!({}),
        ("GET", "/v13/deployments/dpl_0") => {
            json!({ "id": "dpl_0", "url": "site-old.vercel.app", "readyState": "READY" })
        }
        ("POST", "/api/v1/sites/site1/deploys") => {
            let b: Value = serde_json::from_slice(&m.log.lock().last().unwrap().body).unwrap_or_default();
            let first = b["files"].as_object().and_then(|o| o.values().next().cloned()).unwrap_or_default();
            json!({ "id": "nd1", "state": "uploading", "required": [first] })
        }
        ("PUT", _) if p.starts_with("/api/v1/deploys/nd1/files/") => json!({}),
        ("GET", "/api/v1/deploys/nd1") => {
            json!({ "id": "nd1", "state": "ready", "ssl_url": "https://site1.netlify.app" })
        }
        ("POST", "/api/v1/sites/site1/deploys/nd0/restore") => json!({ "id": "nd0", "state": "ready" }),
        ("GET", "/api/v1/deploys/nd0") => {
            json!({ "id": "nd0", "state": "ready", "ssl_url": "https://site1.netlify.app" })
        }
        ("GET", "/v1/apps/app1/machines") => json!([{ "id": "m1", "state": "started" }]),
        ("POST", "/v1/apps/app1/machines/m1") => json!({ "id": "m1" }),
        ("GET", "/v1/apps/app1/machines/m1/wait") => json!({ "ok": true }),
        ("PATCH", "/apis/apps/v1/namespaces/prod/deployments/web") => json!({ "metadata": { "generation": 7 } }),
        ("GET", "/apis/apps/v1/namespaces/prod/deployments/web") => json!({
            "metadata": { "generation": 7 },
            "spec": { "replicas": 2 },
            "status": { "observedGeneration": 7, "updatedReplicas": 2, "availableReplicas": if first_poll { 1 } else { 2 } }
        }),
        _ => {
            return (StatusCode::NOT_FOUND, Json(json!({ "error": format!("unexpected {method} {p}") })))
                .into_response()
        }
    };
    Json(v).into_response()
}

use axum::Json;

async fn mock() -> (String, Mock) {
    let m = Mock::default();
    let app = axum::Router::new().fallback(handler).with_state(m.clone());
    let l = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", l.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(l, app).await.unwrap() });
    (base, m)
}

fn request(
    root: &Path,
    target: DeploymentTarget,
    settings: &[(&str, &str)],
    secret: &str,
    with_token: bool,
) -> DeployRequest {
    let t = now();
    let mut creds = BTreeMap::new();
    if with_token {
        creds.insert(secret.to_owned(), TOKEN.to_owned());
    }
    DeployRequest {
        deployment_id: DeploymentId::new(),
        project_id: ProjectId::new(),
        project_root: root.to_path_buf(),
        source_dir: root.to_path_buf(),
        environment: Environment {
            id: EnvironmentId::new(),
            project_id: ProjectId::new(),
            name: "production".into(),
            kind: EnvironmentKind::Production,
            variables: [("MODE".to_owned(), "prod".to_owned())].into_iter().collect(),
            secret_refs: Vec::new(),
            urls: Vec::new(),
            protected: true,
            permissions: Vec::new(),
            created_at: t,
            updated_at: t,
        },
        target,
        settings: settings
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .chain([("pollIntervalMs".to_owned(), "10".to_owned())])
            .collect(),
        version: "abc123".into(),
        credentials: Credentials(creds),
    }
}

fn site(root: &Path) {
    std::fs::create_dir_all(root.join("dist/assets")).unwrap();
    std::fs::write(root.join("dist/index.html"), "<h1>hi</h1>").unwrap();
    std::fs::write(root.join("dist/assets/a b.js"), "console.log(1)").unwrap();
    std::fs::write(root.join("dist/.env"), "SECRET=1").unwrap();
}

fn sink() -> (LogSink, Arc<Mutex<Vec<String>>>) {
    let lines = Arc::new(Mutex::new(Vec::new()));
    let l = Arc::clone(&lines);
    (LogSink::new(move |s| l.lock().push(s.to_owned())), lines)
}

fn assert_no_token(what: &str, text: &str) {
    assert!(!text.contains(TOKEN), "{what} leaks the token: {text}");
}

fn earlier(target: DeploymentTarget, provider_ref: &str) -> RollbackTarget {
    RollbackTarget {
        deployment: Deployment {
            id: DeploymentId::new(),
            project_id: ProjectId::new(),
            environment_id: EnvironmentId::new(),
            target,
            status: DeploymentStatus::Succeeded,
            version: Some("old".into()),
            task_id: None,
            triggered_by: Actor::System,
            url: None,
            log_object_id: None,
            started_at: None,
            finished_at: None,
            created_at: now(),
        },
        provider_ref: Some(provider_ref.into()),
    }
}

#[tokio::test]
async fn vercel_request_shapes() {
    let (base, m) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    site(dir.path());
    let a = VercelAdapter::with_base(Some(base.clone()));
    let req =
        request(dir.path(), DeploymentTarget::Vercel, &[("project", "site"), ("dir", "dist")], "VERCEL_TOKEN", true);
    let plan = a.plan(&req).await.unwrap();
    assert_no_token("plan", &serde_json::to_string(&plan).unwrap());
    assert!(plan.requests.iter().all(|r| r.body.as_deref().unwrap_or("").contains("Bearer [REDACTED]")));

    let (log, lines) = sink();
    let out = a.deploy(&req, log, CancellationToken::new()).await.unwrap();
    assert_eq!(out.url.as_deref(), Some("https://site-abc.vercel.app"));
    assert_eq!(out.provider_ref.as_deref(), Some("dpl_1"));
    let recs = m.log.lock().clone();
    assert!(recs.iter().all(|r| r.auth.as_deref() == Some(&format!("Bearer {TOKEN}"))));
    let uploads: Vec<&Rec> = recs.iter().filter(|r| r.path == "/v2/files").collect();
    assert_eq!(uploads.len(), 2, ".env files are never uploaded");
    for u in &uploads {
        use sha1::Digest;
        let digest = u.headers.get("x-vercel-digest").unwrap().to_str().unwrap();
        assert_eq!(digest, hex::encode(sha1::Sha1::digest(&u.body)));
    }
    let create = recs.iter().find(|r| r.path == "/v13/deployments").unwrap();
    let body: Value = serde_json::from_slice(&create.body).unwrap();
    assert_eq!(body["name"], "site");
    assert_eq!(body["target"], "production");
    let files = body["files"].as_array().unwrap();
    assert!(files
        .iter()
        .any(|f| f["file"] == "assets/a b.js" && f["sha"].as_str().unwrap().len() == 40 && f["size"] == 14));
    assert!(recs.iter().filter(|r| r.path == "/v13/deployments/dpl_1").count() >= 2, "polls readyState");
    assert_no_token("logs", &lines.lock().join("\n"));

    let rb = a
        .rollback_to(&req, &earlier(DeploymentTarget::Vercel, "dpl_0"), sink().0, CancellationToken::new())
        .await
        .unwrap();
    assert_eq!(rb.provider_ref.as_deref(), Some("dpl_0"));
    assert!(m.log.lock().iter().any(|r| r.method == "POST" && r.path == "/v9/projects/site/rollback/dpl_0"));
}

#[tokio::test]
async fn netlify_request_shapes() {
    let (base, m) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    site(dir.path());
    let a = NetlifyAdapter::with_base(Some(base));
    let req = request(dir.path(), DeploymentTarget::Netlify, &[("siteId", "site1")], "NETLIFY_AUTH_TOKEN", true);
    let out = a.deploy(&req, sink().0, CancellationToken::new()).await.unwrap();
    assert_eq!(out.url.as_deref(), Some("https://site1.netlify.app"));
    let recs = m.log.lock().clone();
    let create = recs.iter().find(|r| r.path == "/api/v1/sites/site1/deploys").unwrap();
    let body: Value = serde_json::from_slice(&create.body).unwrap();
    let files = body["files"].as_object().unwrap();
    assert!(files.contains_key("/index.html") && files.contains_key("/assets/a b.js") && !files.contains_key("/.env"));
    assert_eq!(body["draft"], false);
    let puts: Vec<&Rec> = recs.iter().filter(|r| r.method == "PUT").collect();
    assert_eq!(puts.len(), 1, "only the files Netlify requires are uploaded");
    assert_eq!(puts[0].headers.get("content-type").unwrap(), "application/octet-stream");
    a.rollback_to(&req, &earlier(DeploymentTarget::Netlify, "nd0"), sink().0, CancellationToken::new()).await.unwrap();
    assert!(m.log.lock().iter().any(|r| r.path == "/api/v1/sites/site1/deploys/nd0/restore"));
}

#[tokio::test]
async fn fly_request_shapes() {
    let (base, m) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    let a = FlyAdapter::with_base(Some(base));
    let req = request(
        dir.path(),
        DeploymentTarget::Fly,
        &[("app", "app1"), ("image", "registry.fly.io/app1:{version}"), ("internalPort", "3000")],
        "FLY_API_TOKEN",
        true,
    );
    let out = a.deploy(&req, sink().0, CancellationToken::new()).await.unwrap();
    assert_eq!(out.url.as_deref(), Some("https://app1.fly.dev"));
    let recs = m.log.lock().clone();
    let upd = recs.iter().find(|r| r.method == "POST" && r.path == "/v1/apps/app1/machines/m1").unwrap();
    let body: Value = serde_json::from_slice(&upd.body).unwrap();
    assert_eq!(body["config"]["image"], "registry.fly.io/app1:abc123");
    assert_eq!(body["config"]["services"][0]["internal_port"], 3000);
    assert_eq!(body["config"]["env"]["MODE"], "prod");
    assert!(recs.iter().any(|r| r.path.starts_with("/v1/apps/app1/machines/m1/wait?state=started")));
}

#[tokio::test]
async fn kubernetes_request_shapes() {
    let (base, m) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    let a = KubernetesAdapter::with_base(Some(base));
    let req = request(
        dir.path(),
        DeploymentTarget::Kubernetes,
        &[("namespace", "prod"), ("deployment", "web"), ("container", "app"), ("image", "ghcr.io/acme/web:{version}")],
        "KUBE_TOKEN",
        true,
    );
    a.deploy(&req, sink().0, CancellationToken::new()).await.unwrap();
    let recs = m.log.lock().clone();
    let patch = recs.iter().find(|r| r.method == "PATCH").unwrap();
    assert_eq!(patch.headers.get("content-type").unwrap(), "application/strategic-merge-patch+json");
    let body: Value = serde_json::from_slice(&patch.body).unwrap();
    assert_eq!(body["spec"]["template"]["spec"]["containers"][0]["name"], "app");
    assert_eq!(body["spec"]["template"]["spec"]["containers"][0]["image"], "ghcr.io/acme/web:abc123");
    assert!(recs.iter().filter(|r| r.method == "GET").count() >= 2, "polls the rollout until available");
}

#[tokio::test]
async fn missing_token_and_api_errors_are_redacted() {
    let (base, _m) = mock().await;
    let dir = tempfile::tempdir().unwrap();
    site(dir.path());
    let a = VercelAdapter::with_base(Some(base.clone()));
    let req =
        request(dir.path(), DeploymentTarget::Vercel, &[("project", "site"), ("dir", "dist")], "VERCEL_TOKEN", false);
    let err = a.deploy(&req, sink().0, CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, DeployError::Unavailable(ref m) if m.contains("VERCEL_TOKEN")));
    let plan = a.plan(&req).await.unwrap();
    assert!(plan.warnings.iter().any(|w| w.contains("VERCEL_TOKEN")));

    // Unknown site → API error; the message never carries the token.
    let req = request(dir.path(), DeploymentTarget::Netlify, &[("siteId", "nope")], "NETLIFY_AUTH_TOKEN", true);
    let err = NetlifyAdapter::with_base(Some(base)).deploy(&req, sink().0, CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, DeployError::Failed(_)));
    assert_no_token("error", &err.to_string());
}

/// The smoke fixture `scripts/smoke/fixtures/fake-cloud-api.mjs` accepts every
/// request the adapters send (auth + shapes), so acceptance runs can rely on it.
#[tokio::test]
async fn fake_cloud_api_fixture_accepts_every_adapter() {
    use tokio::io::AsyncBufReadExt;
    let Some(node) = orkestr_platform::process::which("node") else {
        eprintln!("skipped: node is not on PATH");
        return;
    };
    let script = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../scripts/smoke/fixtures/fake-cloud-api.mjs");
    let mut child = tokio::process::Command::new(node)
        .arg(&script)
        .args(["--port", "0", "--token", TOKEN])
        .stdout(std::process::Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut lines = tokio::io::BufReader::new(child.stdout.take().unwrap()).lines();
    let first = lines.next_line().await.unwrap().unwrap();
    let base = serde_json::from_str::<Value>(&first).unwrap()["base"].as_str().unwrap().to_owned();
    let dir = tempfile::tempdir().unwrap();
    site(dir.path());
    let b = Some(base.clone());
    let cases: Vec<(Box<dyn DeployAdapter>, DeployRequest)> = vec![
        (
            Box::new(VercelAdapter::with_base(b.clone())),
            request(
                dir.path(),
                DeploymentTarget::Vercel,
                &[("project", "site"), ("dir", "dist")],
                "VERCEL_TOKEN",
                true,
            ),
        ),
        (
            Box::new(NetlifyAdapter::with_base(b.clone())),
            request(dir.path(), DeploymentTarget::Netlify, &[("siteId", "site1")], "NETLIFY_AUTH_TOKEN", true),
        ),
        (
            Box::new(FlyAdapter::with_base(b.clone())),
            request(
                dir.path(),
                DeploymentTarget::Fly,
                &[("app", "app1"), ("image", "img:{version}")],
                "FLY_API_TOKEN",
                true,
            ),
        ),
        (
            Box::new(KubernetesAdapter::with_base(b.clone())),
            request(
                dir.path(),
                DeploymentTarget::Kubernetes,
                &[("deployment", "web"), ("image", "img:{version}")],
                "KUBE_TOKEN",
                true,
            ),
        ),
    ];
    for (a, req) in &cases {
        let out = a.deploy(req, sink().0, CancellationToken::new()).await;
        assert!(out.is_ok(), "{} vs fake API: {:?}", a.id(), out.err());
    }
    // Vercel + Netlify rollbacks against the ids the fake handed out.
    let (v, vr) = &cases[0];
    let first_vercel = reqwest::get(format!("{base}/__requests")).await.unwrap().json::<Vec<Value>>().await.unwrap();
    assert!(first_vercel.iter().any(|r| r["path"] == "/v13/deployments"));
    v.rollback_to(vr, &earlier(DeploymentTarget::Vercel, "dpl_1"), sink().0, CancellationToken::new()).await.unwrap();
    let recs = reqwest::get(format!("{base}/__requests")).await.unwrap().json::<Vec<Value>>().await.unwrap();
    assert!(recs.len() > 10);
    for r in &recs {
        assert_eq!(r["authOk"], true, "{r}");
        assert_eq!(r["shapeOk"], true, "{r}");
    }
    assert_no_token("fake API log", &serde_json::to_string(&recs).unwrap());
    let _ = child.kill().await;
}
