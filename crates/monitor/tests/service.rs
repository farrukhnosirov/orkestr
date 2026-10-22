//! End-to-end behaviour of `MonitorService` with an in-memory database.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{DeploymentId, NodeId};
use orkestr_events::{EventBus, Received};
use orkestr_monitor::{DeploymentLogFeed, DeploymentRef, MonitorConfig, MonitorDeps, MonitorService};
use orkestr_platform::{NativePlatform, Paths, PlatformAdapter};
use orkestr_protocol::event::{EventKind, LogLevel};
use orkestr_protocol::methods::deploy::DeployLogsResult;
use orkestr_protocol::methods::monitor::{
    AlertState, CheckProbe, CheckState, IngestLine, IngestParams, LogQuery, LogSourceKind, UpsertCheckParams,
};

struct Fixture {
    svc: MonitorService,
    events: EventBus,
    _dir: tempfile::TempDir,
}

async fn fixture() -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    let db = orkestr_storage::Db::open_in_memory().unwrap();
    db.migrate(orkestr_monitor::monitor_migrations()).await.unwrap();
    let events = EventBus::in_memory(1024);
    let platform: Arc<dyn PlatformAdapter> = Arc::new(NativePlatform::new(Paths::from_root(dir.path())));
    let opts = orkestr_node::DetectOptions { ollama_endpoint: "127.0.0.1:9".into(), ..Default::default() };
    let nodes = Arc::new(orkestr_node::NodeMonitor::new(NodeId::new(), platform, "0.1.0", opts));
    let cfg = MonitorConfig {
        sample_interval: Duration::from_millis(200),
        scheduler_tick: Duration::from_millis(50),
        deploy_poll: Duration::from_millis(50),
        file_poll: Duration::from_millis(50),
        ..MonitorConfig::default()
    };
    let svc = MonitorService::with_config(
        MonitorDeps { db, events: events.clone(), nodes, data_dir: dir.path().join("monitor") },
        cfg,
    );
    Fixture { svc, events, _dir: dir }
}

async fn wait_for<F: FnMut() -> bool>(mut f: F, what: &str) {
    for _ in 0..200 {
        if f() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("timed out waiting for {what}");
}

#[tokio::test]
async fn check_down_then_up_emits_events_and_alerts() {
    let fx = fixture().await;
    let mut sub = fx.events.subscribe_all();
    fx.svc.start().await.unwrap();
    // Port that is open now and closed later.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let check = fx
        .svc
        .upsert_check(UpsertCheckParams {
            id: None,
            project_id: None,
            environment_id: None,
            name: "svc".into(),
            probe: CheckProbe::Tcp { host: "127.0.0.1".into(), port },
            interval_secs: Some(3600),
            timeout_ms: Some(500),
            failure_threshold: Some(2),
            enabled: Some(false),
        })
        .await
        .unwrap();
    let id = check.check.id;
    assert_eq!(fx.svc.run_check(id).await.unwrap().state, CheckState::Unknown);
    assert_eq!(fx.svc.run_check(id).await.unwrap().state, CheckState::Down);
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap();
    tokio::spawn(async move { while listener.accept().await.is_ok() {} });
    assert_eq!(fx.svc.run_check(id).await.unwrap().state, CheckState::Down, "hysteresis: one success is not enough");
    let up = fx.svc.run_check(id).await.unwrap();
    assert_eq!(up.state, CheckState::Up);
    assert_eq!(up.history.len(), 4);

    let mut kinds = Vec::new();
    while let Ok(Some(Received::Event(e))) = tokio::time::timeout(Duration::from_millis(300), sub.recv()).await {
        match &e.kind {
            EventKind::HealthCheckChanged(h) => kinds.push(format!("check:{:?}->{:?}", h.from, h.to)),
            EventKind::AlertRaised(_) => kinds.push("raised".into()),
            EventKind::AlertResolved(_) => kinds.push("resolved".into()),
            _ => {}
        }
    }
    assert_eq!(kinds, ["check:Unknown->Down", "raised", "check:Down->Up", "resolved"]);
    let alerts = fx.svc.alerts(None, true, None).await.unwrap();
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].state, AlertState::Resolved);
    assert!(fx.svc.active_alerts(None).await.unwrap().is_empty());
    let ov = fx.svc.overview(None).await.unwrap();
    assert_eq!(ov.checks.len(), 1);
    fx.svc.delete_check(id).await.unwrap();
    assert!(fx.svc.checks(None).is_empty());
    fx.svc.shutdown();
}

#[tokio::test]
async fn overview_and_metrics_from_local_sampler() {
    let fx = fixture().await;
    fx.svc.start().await.unwrap();
    wait_for(
        || fx.svc.metrics(None, &["cpu".into()], None, None).first().is_some_and(|s| s.points.len() >= 2),
        "samples",
    )
    .await;
    let ov = fx.svc.overview(None).await.unwrap();
    assert!(ov.nodes[0].local);
    assert!(ov.nodes[0].mem_total_bytes > 0);
    let series = fx.svc.recent_metrics(None, Duration::from_secs(60));
    assert!(series.iter().any(|s| s.metric == "disk" && !s.points.is_empty()));
    fx.svc.shutdown();
}

#[tokio::test]
async fn ingest_is_redacted_queryable_and_tails() {
    let fx = fixture().await;
    fx.svc.start().await.unwrap();
    let line = |m: &str, level| IngestLine { ts: None, level, message: m.into(), attrs: Default::default() };
    let mut attrs_line = line("payment failed", Some(LogLevel::Error));
    attrs_line.attrs.insert("password".into(), "s3cr3t-pass".into());
    attrs_line.attrs.insert("order".into(), "42".into());
    let accepted = fx
        .svc
        .ingest(IngestParams {
            stream: "web".into(),
            project_id: None,
            lines: vec![line("started api_key=AKIAABCDEFGHIJKLMNOP", None), attrs_line],
        })
        .await
        .unwrap();
    assert_eq!(accepted, 2);
    let mut tail = fx.svc.subscribe_logs("ingest:web").unwrap();
    fx.svc
        .ingest(IngestParams {
            stream: "web".into(),
            project_id: None,
            lines: vec![line("token=abcdef123456 later", None)],
        })
        .await
        .unwrap();
    let live = tail.recv().await.unwrap();
    assert!(!live.message.contains("abcdef123456"), "{}", live.message);
    let logs = fx.svc.logs(&LogQuery::default()).await.unwrap();
    assert_eq!(logs.lines.len(), 3);
    for l in &logs.lines {
        assert!(!l.message.contains("AKIAABCDEFGHIJKLMNOP") && !l.message.contains("s3cr3t-pass"), "{}", l.message);
    }
    assert!(logs.lines[1].message.contains("order=42"));
    let errors = fx.svc.logs(&LogQuery { min_level: Some(LogLevel::Error), ..LogQuery::default() }).await.unwrap();
    assert_eq!(errors.lines.len(), 1);
    let around = fx
        .svc
        .logs_around(orkestr_domain::now(), Duration::from_secs(60), Duration::from_secs(1), None, None, 100)
        .await
        .unwrap();
    assert_eq!(around.len(), 3);
    let old = fx
        .svc
        .logs_around(
            orkestr_domain::now() - chrono::Duration::hours(2),
            Duration::from_secs(60),
            Duration::from_secs(60),
            None,
            None,
            100,
        )
        .await
        .unwrap();
    assert!(old.is_empty());
    assert!(fx
        .svc
        .ingest(IngestParams { stream: "bad stream".into(), project_id: None, lines: vec![] })
        .await
        .is_err());
    let sources = fx.svc.log_sources(None);
    assert!(matches!(sources[0].kind, LogSourceKind::Ingest { .. }));
    fx.svc.shutdown();
}

struct FakeDeploy {
    dep: DeploymentId,
}

#[async_trait]
impl DeploymentLogFeed for FakeDeploy {
    async fn recent(&self, _limit: usize) -> Vec<DeploymentRef> {
        vec![DeploymentRef { deployment_id: self.dep, project_id: None, name: "prod v2".into() }]
    }
    async fn read(&self, _id: DeploymentId, offset: u64) -> Result<DeployLogsResult, String> {
        let all = ["build ok", "ERROR crash with Bearer abcdefghijklmnop12345", "done"];
        let lines: Vec<String> = all.iter().skip(usize::try_from(offset).unwrap()).map(|s| (*s).to_owned()).collect();
        Ok(DeployLogsResult { lines, next_offset: all.len() as u64, complete: true })
    }
}

#[tokio::test]
async fn deployment_and_file_sources() {
    let fx = fixture().await;
    let dep = DeploymentId::new();
    fx.svc.set_deployment_logs(Arc::new(FakeDeploy { dep }));
    let path = fx._dir.path().join("app.log");
    std::fs::write(&path, "boot\n").unwrap();
    let info = fx.svc.add_file_source("app", path.clone(), None).await.unwrap();
    fx.svc.start().await.unwrap();
    let id = format!("deploy:{dep}");
    wait_lines(&fx.svc, &id, 3).await;
    let lines = fx.svc.logs(&LogQuery { source_id: Some(id), ..LogQuery::default() }).await.unwrap().lines;
    assert!(lines.iter().all(|l| !l.message.contains("abcdefghijklmnop12345")));
    assert_eq!(lines[1].level, Some(LogLevel::Error));
    wait_lines(&fx.svc, &info.id, 1).await;
    fx.svc.remove_source(&info.id).await.unwrap();
    assert!(fx.svc.source(&info.id).is_none());
    fx.svc.shutdown();
}

async fn wait_lines(svc: &MonitorService, id: &str, n: usize) {
    for _ in 0..200 {
        if let Some(src) = svc.source(id) {
            if src.query(&LogQuery::default()).await.unwrap().len() == n {
                return;
            }
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("source {id} never reached {n} lines");
}

/// A deployment that appears after the loop's last refresh is visible (with
/// its lines, exactly once) as soon as `monitor.logs` / `logSources` ask.
#[tokio::test]
async fn deployment_sources_refresh_on_demand() {
    let fx = fixture().await;
    fx.svc.start().await.unwrap();
    // The loop's first (refresh) poll ran without a feed.
    tokio::time::sleep(Duration::from_millis(50)).await;
    let dep = DeploymentId::new();
    fx.svc.set_deployment_logs(Arc::new(FakeDeploy { dep }));
    let id = format!("deploy:{dep}");
    let q = LogQuery { source_id: Some(id.clone()), ..LogQuery::default() };
    let (first, ()) = tokio::join!(fx.svc.logs(&q), fx.svc.refresh_deployment_sources());
    first.unwrap();
    let lines = fx.svc.logs(&LogQuery { source_id: Some(id.clone()), ..LogQuery::default() }).await.unwrap().lines;
    assert_eq!(lines.len(), 3, "registered and read once: {lines:?}");
    assert!(fx.svc.log_sources(None).iter().any(|s| s.id == id));
    fx.svc.shutdown();
}
