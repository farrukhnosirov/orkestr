//! [`MonitorService`]: facade used by `core/src/rpc/monitor.rs` and by the Bug
//! Detective (WP6) evidence collectors.
//!
//! Lifecycle: [`MonitorService::new`] is cheap and starts nothing.
//! [`MonitorService::start`] (idempotent; the RPC handlers also call it)
//! loads persisted checks / rules / alerts / sources and spawns the loops:
//! resource sampler, check scheduler, deployment-log poller, alert worker.
//! [`MonitorService::shutdown`] cancels them.
//!
//! # Evidence API (WP6)
//! * [`MonitorService::logs_around`] — redacted lines in `[at - before, at + after]`;
//! * [`MonitorService::recent_metrics`] — series of a node over a window;
//! * [`MonitorService::alert`] / [`MonitorService::active_alerts`];
//! * [`MonitorService::check_status`];
//! * [`MonitorService::sources`] / [`MonitorService::source`] (as [`LogSource`]) and
//!   [`MonitorService::subscribe_logs`] (live tail with drop accounting).

use std::collections::{BTreeMap, HashMap};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{AlertId, DeploymentId, HealthCheckId, NodeId, ProjectId, Timestamp};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, LogLevel};
use orkestr_protocol::methods::monitor::{
    Alert, CheckState, HealthCheck, HealthCheckChangedPayload, HealthCheckStatus, IngestParams, LogLine, LogQuery,
    LogSourceInfo, LogSourceKind, LogsResult, MetricSeries, MonitorOverview,
};
use orkestr_protocol::NewEvent;
use orkestr_storage::Db;
use parking_lot::{Mutex, RwLock};
use tokio::sync::{mpsc, OnceCell};
use tokio_util::sync::CancellationToken;

use crate::alerts::{default_rules, AlertEngine, AlertRule, Signal};
use crate::checks::{self, CheckRuntime};
use crate::logs::{self, BufferedSource, DeploymentLogFeed, LineHook, LogSource, TailReceiver};
use crate::metrics::{self, MetricsStore, NodeMetricsSource};
use crate::store::MonitorStore;
use crate::MonitorError;

/// Services the monitor needs.
#[derive(Clone)]
pub struct MonitorDeps {
    /// Database.
    pub db: Db,
    /// Event bus.
    pub events: EventBus,
    /// Local resource sampler.
    pub nodes: Arc<orkestr_node::NodeMonitor>,
    /// `<data_dir>/monitor`.
    pub data_dir: PathBuf,
}

/// Tunables (defaults suit production; tests shrink them).
#[derive(Debug, Clone)]
pub struct MonitorConfig {
    /// Local sampling period (also the remote-node poll period).
    pub sample_interval: Duration,
    /// Samples kept per node (default 720 = 1 h at 5 s).
    pub metrics_capacity: usize,
    /// Lines kept per log source.
    pub ring_lines: usize,
    /// Live-tail broadcast buffer per source.
    pub tail_buffer: usize,
    /// File tail poll period.
    pub file_poll: Duration,
    /// Deployment-log poll period.
    pub deploy_poll: Duration,
    /// Check scheduler tick.
    pub scheduler_tick: Duration,
}

impl Default for MonitorConfig {
    fn default() -> Self {
        Self {
            sample_interval: Duration::from_secs(5),
            metrics_capacity: 720,
            ring_lines: logs::DEFAULT_RING,
            tail_buffer: logs::DEFAULT_TAIL_BUFFER,
            file_poll: Duration::from_millis(500),
            deploy_poll: Duration::from_secs(2),
            scheduler_tick: Duration::from_secs(1),
        }
    }
}

/// Lines accepted per `monitor.ingest` call.
pub const MAX_INGEST_LINES: usize = 1000;
/// Deployments followed at once.
const MAX_DEPLOYMENTS: usize = 20;

struct DeployCursor {
    offset: u64,
    complete: bool,
}

struct Inner {
    deps: MonitorDeps,
    cfg: MonitorConfig,
    store: MonitorStore,
    metrics: MetricsStore,
    alerts: Arc<AlertEngine>,
    checks: Mutex<HashMap<HealthCheckId, CheckRuntime>>,
    sources: RwLock<BTreeMap<String, Arc<BufferedSource>>>,
    file_tails: Mutex<HashMap<String, CancellationToken>>,
    node_source: RwLock<Option<Arc<dyn NodeMetricsSource>>>,
    deploy_feed: RwLock<Option<Arc<dyn DeploymentLogFeed>>>,
    deploy_cursors: Mutex<HashMap<DeploymentId, DeployCursor>>,
    signal_tx: mpsc::UnboundedSender<Vec<Signal>>,
    signal_rx: Mutex<Option<mpsc::UnboundedReceiver<Vec<Signal>>>>,
    client: reqwest::Client,
    cancel: CancellationToken,
    init: OnceCell<()>,
    started: OnceCell<()>,
}

/// Infrastructure monitor.
#[derive(Clone)]
pub struct MonitorService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MonitorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MonitorService").field("data_dir", &self.inner.deps.data_dir).finish_non_exhaustive()
    }
}

fn valid_name(s: &str, max: usize) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
}

impl MonitorService {
    /// Creates the service (no background loops are started here).
    #[must_use]
    pub fn new(deps: MonitorDeps) -> Self {
        Self::with_config(deps, MonitorConfig::default())
    }

    /// Creates the service with explicit tunables.
    #[must_use]
    pub fn with_config(deps: MonitorDeps, cfg: MonitorConfig) -> Self {
        let store = MonitorStore::new(deps.db.clone());
        let alerts = Arc::new(AlertEngine::new(store.clone(), deps.events.clone(), Vec::new(), Vec::new()));
        let (signal_tx, signal_rx) = mpsc::unbounded_channel();
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .user_agent(concat!("orkestr-monitor/", env!("CARGO_PKG_VERSION")))
            .build()
            .unwrap_or_default();
        Self {
            inner: Arc::new(Inner {
                metrics: MetricsStore::new(cfg.metrics_capacity),
                deps,
                cfg,
                store,
                alerts,
                checks: Mutex::new(HashMap::new()),
                sources: RwLock::new(BTreeMap::new()),
                file_tails: Mutex::new(HashMap::new()),
                node_source: RwLock::new(None),
                deploy_feed: RwLock::new(None),
                deploy_cursors: Mutex::new(HashMap::new()),
                signal_tx,
                signal_rx: Mutex::new(Some(signal_rx)),
                client,
                cancel: CancellationToken::new(),
                init: OnceCell::new(),
                started: OnceCell::new(),
            }),
        }
    }

    /// Wires per-node metrics (integrator: `NodeHub` snapshots).
    pub fn set_node_source(&self, src: Arc<dyn NodeMetricsSource>) {
        *self.inner.node_source.write() = Some(src);
    }

    /// Wires deployment logs (integrator: `DeployService`, WP4).
    pub fn set_deployment_logs(&self, feed: Arc<dyn DeploymentLogFeed>) {
        *self.inner.deploy_feed.write() = Some(feed);
    }

    /// Loads persisted state (idempotent; no loops).
    pub async fn init(&self) -> Result<(), MonitorError> {
        let inner = &self.inner;
        inner
            .init
            .get_or_try_init(|| async {
                let mut rules = inner.store.load_rules().await?;
                if rules.is_empty() {
                    rules = default_rules();
                    for r in &rules {
                        inner.store.save_rule(r).await?;
                    }
                }
                inner.alerts.set_rules(rules);
                inner.alerts.restore_open(inner.store.load_open_alerts().await?).await;
                let stored = inner.store.load_checks().await?;
                {
                    let mut checks = inner.checks.lock();
                    for c in stored {
                        checks.insert(c.check.id, CheckRuntime::new(c.check, c.state, c.since));
                    }
                }
                for info in inner.store.load_sources().await? {
                    self.register_source(info);
                }
                Ok::<(), MonitorError>(())
            })
            .await?;
        Ok(())
    }

    /// Initialises and spawns the background loops (idempotent).
    pub async fn start(&self) -> Result<(), MonitorError> {
        self.init().await?;
        self.inner
            .started
            .get_or_init(|| async {
                self.spawn_loops();
            })
            .await;
        Ok(())
    }

    /// Stops every loop and file tail.
    pub fn shutdown(&self) {
        self.inner.cancel.cancel();
    }

    fn spawn_loops(&self) {
        let cancel = self.inner.cancel.clone();
        // Alert worker.
        if let Some(mut rx) = self.inner.signal_rx.lock().take() {
            let alerts = Arc::clone(&self.inner.alerts);
            let c = cancel.clone();
            tokio::spawn(async move {
                loop {
                    let batch = tokio::select! {
                        () = c.cancelled() => break,
                        b = rx.recv() => match b { Some(b) => b, None => break },
                    };
                    for s in batch {
                        if let Err(e) = alerts.apply(s).await {
                            tracing::warn!(error = %e, "alert update failed");
                        }
                    }
                }
            });
        }
        // Sampler.
        let me = self.clone();
        let c = cancel.clone();
        tokio::spawn(async move {
            loop {
                me.sample_once().await;
                tokio::select! {
                    () = c.cancelled() => break,
                    () = tokio::time::sleep(me.inner.cfg.sample_interval) => {}
                }
            }
        });
        // Check scheduler + log-rule resolution tick.
        let me = self.clone();
        let c = cancel.clone();
        tokio::spawn(async move {
            loop {
                me.schedule_due_checks();
                let sig = me.inner.alerts.tick(orkestr_domain::now());
                if !sig.is_empty() {
                    let _ = me.inner.signal_tx.send(sig);
                }
                tokio::select! {
                    () = c.cancelled() => break,
                    () = tokio::time::sleep(me.inner.cfg.scheduler_tick) => {}
                }
            }
        });
        // Deployment logs.
        let me = self.clone();
        tokio::spawn(async move {
            let mut n: u64 = 0;
            loop {
                me.poll_deployments(n.is_multiple_of(5)).await;
                n += 1;
                tokio::select! {
                    () = cancel.cancelled() => break,
                    () = tokio::time::sleep(me.inner.cfg.deploy_poll) => {}
                }
            }
        });
        // File tails registered before start.
        let files: Vec<(String, String)> = self
            .inner
            .sources
            .read()
            .values()
            .filter_map(|s| match &s.info().kind {
                LogSourceKind::File { path } => Some((s.info().id.clone(), path.clone())),
                _ => None,
            })
            .collect();
        for (id, path) in files {
            self.start_file_tail(&id, PathBuf::from(path));
        }
    }

    // ---- metrics -------------------------------------------------------

    /// Takes one local sample and polls remote nodes (the sampler loop calls this).
    pub async fn sample_once(&self) {
        let nodes = Arc::clone(&self.inner.deps.nodes);
        let local_id = nodes.node_id();
        if let Ok(sample) = tokio::task::spawn_blocking(move || {
            let name = nodes.local_node().name;
            (nodes.sample(), name)
        })
        .await
        {
            let snap = metrics::snapshot_from_sample(&sample.1, &sample.0);
            self.ingest_snapshot(snap);
        }
        let src = self.inner.node_source.read().clone();
        if let Some(src) = src {
            let remote = src.nodes().await;
            let now = orkestr_domain::now();
            let mut present = Vec::with_capacity(remote.len());
            for n in remote.iter().filter(|n| n.id != local_id && !n.is_local) {
                present.push(n.id);
                self.ingest_snapshot(metrics::snapshot_from_node(n, now));
            }
            self.inner.metrics.mark_missing_offline(&present, local_id);
        }
    }

    /// Stores a node snapshot and evaluates metric rules.
    pub fn ingest_snapshot(&self, snap: orkestr_protocol::methods::monitor::NodeMetricsSnapshot) {
        let sig = self.inner.alerts.on_metrics(snap.node_id, &snap.name, |m| metrics::metric_value(&snap, m));
        self.inner.metrics.push(snap);
        if !sig.is_empty() {
            let _ = self.inner.signal_tx.send(sig);
        }
    }

    /// `monitor.overview`.
    pub async fn overview(&self, project: Option<ProjectId>) -> Result<MonitorOverview, MonitorError> {
        self.init().await?;
        let mut nodes = self.inner.metrics.latest();
        if nodes.is_empty() {
            if let Some(s) = self.inner.deps.nodes.latest() {
                let name = self.inner.deps.nodes.local_node().name;
                nodes.push(metrics::snapshot_from_sample(&name, &s));
            }
        }
        let checks = self.checks(project);
        let open_alerts = self
            .inner
            .alerts
            .open_alerts()
            .await
            .iter()
            .filter(|a| project.is_none() || a.project_id == project)
            .count();
        let log_sources = self.log_sources(project).len();
        Ok(MonitorOverview {
            nodes,
            checks,
            open_alerts: u32::try_from(open_alerts).unwrap_or(u32::MAX),
            log_sources: u32::try_from(log_sources).unwrap_or(u32::MAX),
        })
    }

    /// `monitor.metrics` (default node: local; default window: 15 min).
    #[must_use]
    pub fn metrics(
        &self,
        node: Option<NodeId>,
        metrics: &[String],
        since: Option<Timestamp>,
        resolution_secs: Option<u32>,
    ) -> Vec<MetricSeries> {
        let node = node.unwrap_or_else(|| self.inner.deps.nodes.node_id());
        let since = since.unwrap_or_else(|| orkestr_domain::now() - chrono::Duration::minutes(15));
        self.inner.metrics.series(node, metrics, since, resolution_secs)
    }

    /// Evidence: series of `node` (default local) over the last `window`.
    #[must_use]
    pub fn recent_metrics(&self, node: Option<NodeId>, window: Duration) -> Vec<MetricSeries> {
        let since = orkestr_domain::now() - chrono::Duration::from_std(window).unwrap_or(chrono::Duration::minutes(15));
        self.metrics(node, &[], Some(since), None)
    }

    // ---- checks --------------------------------------------------------

    /// Checks with status (`project` filter; `None` = all).
    #[must_use]
    pub fn checks(&self, project: Option<ProjectId>) -> Vec<HealthCheckStatus> {
        let mut v: Vec<HealthCheckStatus> = self
            .inner
            .checks
            .lock()
            .values()
            .filter(|r| project.is_none() || r.check.project_id == project)
            .map(CheckRuntime::status)
            .collect();
        v.sort_by_key(|a| a.check.name.to_lowercase());
        v
    }

    /// One check.
    pub async fn check_status(&self, id: HealthCheckId) -> Result<HealthCheckStatus, MonitorError> {
        self.init().await?;
        self.inner
            .checks
            .lock()
            .get(&id)
            .map(CheckRuntime::status)
            .ok_or_else(|| MonitorError::NotFound(format!("check {id}")))
    }

    /// `monitor.checkUpsert`.
    pub async fn upsert_check(
        &self,
        p: orkestr_protocol::methods::monitor::UpsertCheckParams,
    ) -> Result<HealthCheckStatus, MonitorError> {
        self.init().await?;
        let name = p.name.trim().to_owned();
        if name.is_empty() || name.len() > 120 {
            return Err(MonitorError::Invalid("name must be 1..120 characters".into()));
        }
        checks::validate(&p.probe).map_err(MonitorError::Invalid)?;
        let id = p.id.unwrap_or_default();
        if p.id.is_some() && !self.inner.checks.lock().contains_key(&id) {
            return Err(MonitorError::NotFound(format!("check {id}")));
        }
        let check = HealthCheck {
            id,
            project_id: p.project_id,
            environment_id: p.environment_id,
            name,
            probe: p.probe,
            interval_secs: p.interval_secs.unwrap_or(30).clamp(5, 86_400),
            timeout_ms: p.timeout_ms.unwrap_or(5_000).clamp(100, 60_000),
            failure_threshold: p.failure_threshold.unwrap_or(2).clamp(1, 20),
            enabled: p.enabled.unwrap_or(true),
        };
        self.inner.store.save_check(&check).await?;
        let status = {
            let mut checks = self.inner.checks.lock();
            let rt = checks.entry(id).or_insert_with(|| CheckRuntime::new(check.clone(), CheckState::Unknown, None));
            if rt.check.probe != check.probe {
                rt.fails = 0;
                rt.oks = 0;
            }
            rt.check = check;
            rt.next_due = Instant::now();
            rt.status()
        };
        Ok(status)
    }

    /// `monitor.checkDelete` (resolves its alerts).
    pub async fn delete_check(&self, id: HealthCheckId) -> Result<(), MonitorError> {
        self.init().await?;
        let removed = self.inner.checks.lock().remove(&id);
        let in_db = self.inner.store.delete_check(id).await?;
        let Some(rt) = removed else {
            return if in_db { Ok(()) } else { Err(MonitorError::NotFound(format!("check {id}"))) };
        };
        for s in self.inner.alerts.on_check(&rt.check, false, None) {
            self.inner.alerts.apply(s).await?;
        }
        Ok(())
    }

    /// `monitor.checkRun`: probes now and applies the result.
    pub async fn run_check(&self, id: HealthCheckId) -> Result<HealthCheckStatus, MonitorError> {
        self.init().await?;
        if !self.inner.checks.lock().contains_key(&id) {
            return Err(MonitorError::NotFound(format!("check {id}")));
        }
        self.execute_check(id).await;
        self.check_status(id).await
    }

    fn schedule_due_checks(&self) {
        let now = Instant::now();
        let due: Vec<HealthCheckId> = {
            let mut checks = self.inner.checks.lock();
            checks
                .values_mut()
                .filter(|r| r.check.enabled && !r.running && r.next_due <= now)
                .map(|r| {
                    r.running = true;
                    r.next_due = now + Duration::from_secs(u64::from(r.check.interval_secs.max(1)));
                    r.check.id
                })
                .collect()
        };
        for id in due {
            let me = self.clone();
            tokio::spawn(async move {
                me.execute_check(id).await;
                if let Some(r) = me.inner.checks.lock().get_mut(&id) {
                    r.running = false;
                }
            });
        }
    }

    async fn execute_check(&self, id: HealthCheckId) {
        let Some(check) = self.inner.checks.lock().get(&id).map(|r| r.check.clone()) else { return };
        let outcome =
            checks::probe(&self.inner.client, &check.probe, Duration::from_millis(u64::from(check.timeout_ms))).await;
        let at = orkestr_domain::now();
        let (transition, current) = {
            let mut checks = self.inner.checks.lock();
            let Some(rt) = checks.get_mut(&id) else { return };
            (rt.record(at, &outcome), rt.check.clone())
        };
        let Some(t) = transition else { return };
        if let Err(e) = self.inner.store.save_check_state(id, t.to, Some(at)).await {
            tracing::warn!(error = %e, "failed to persist check state");
        }
        let ev = EventKind::HealthCheckChanged(HealthCheckChangedPayload {
            check_id: id,
            project_id: current.project_id,
            name: current.name.clone(),
            from: t.from,
            to: t.to,
            error: if t.to == CheckState::Down { outcome.error.clone() } else { None },
        });
        if let Err(e) = self.inner.deps.events.publish(NewEvent::new(ev)).await {
            tracing::warn!(error = %e, "HealthCheckChanged publication failed");
        }
        if t.to == CheckState::Unknown {
            return;
        }
        let down = t.to == CheckState::Down;
        // Unknown → Up needs no resolve; everything else goes through the engine.
        for s in self.inner.alerts.on_check(&current, down, outcome.error.as_deref()) {
            if let Err(e) = self.inner.alerts.apply(s).await {
                tracing::warn!(error = %e, "alert update failed");
            }
        }
    }

    // ---- logs ----------------------------------------------------------

    fn hook(&self) -> LineHook {
        let alerts = Arc::clone(&self.inner.alerts);
        let tx = self.inner.signal_tx.clone();
        Arc::new(move |line: &LogLine, project: Option<ProjectId>| {
            let sig = alerts.on_log(line, project);
            if !sig.is_empty() {
                let _ = tx.send(sig);
            }
        })
    }

    fn register_source(&self, info: LogSourceInfo) -> Arc<BufferedSource> {
        let mut sources = self.inner.sources.write();
        if let Some(s) = sources.get(&info.id) {
            return Arc::clone(s);
        }
        let src = Arc::new(BufferedSource::new(
            info,
            self.inner.cfg.ring_lines,
            self.inner.cfg.tail_buffer,
            Some(self.hook()),
        ));
        sources.insert(src.info().id.clone(), Arc::clone(&src));
        src
    }

    fn start_file_tail(&self, id: &str, path: PathBuf) {
        let Some(src) = self.inner.sources.read().get(id).cloned() else { return };
        let token = self.inner.cancel.child_token();
        if let Some(old) = self.inner.file_tails.lock().insert(id.to_owned(), token.clone()) {
            old.cancel();
        }
        logs::spawn_file_tail(src, path, self.inner.cfg.file_poll, token);
    }

    /// Adds (and persists) a tailed file source.
    pub async fn add_file_source(
        &self,
        name: &str,
        path: PathBuf,
        project: Option<ProjectId>,
    ) -> Result<LogSourceInfo, MonitorError> {
        self.init().await?;
        if !path.is_absolute() {
            return Err(MonitorError::Invalid("log file path must be absolute".into()));
        }
        let path_s = path.to_string_lossy().into_owned();
        let id = format!("file:{}", short_hash(&path_s));
        let info = LogSourceInfo {
            id: id.clone(),
            name: if name.trim().is_empty() { path_s.clone() } else { name.trim().to_owned() },
            kind: LogSourceKind::File { path: path_s },
            project_id: project,
        };
        self.inner.store.save_source(&info).await?;
        self.register_source(info.clone());
        if self.inner.started.initialized() {
            self.start_file_tail(&id, path);
        }
        Ok(info)
    }

    /// Removes a source (persisted or not) and stops its tail.
    pub async fn remove_source(&self, id: &str) -> Result<(), MonitorError> {
        self.init().await?;
        let removed = self.inner.sources.write().remove(id);
        if let Some(t) = self.inner.file_tails.lock().remove(id) {
            t.cancel();
        }
        let in_db = self.inner.store.delete_source(id).await?;
        if removed.is_none() && !in_db {
            return Err(MonitorError::NotFound(format!("log source {id}")));
        }
        Ok(())
    }

    /// `monitor.logSources`.
    #[must_use]
    pub fn log_sources(&self, project: Option<ProjectId>) -> Vec<LogSourceInfo> {
        self.inner
            .sources
            .read()
            .values()
            .map(|s| s.info().clone())
            .filter(|i| project.is_none() || i.project_id.is_none() || i.project_id == project)
            .collect()
    }

    /// Every source as a [`LogSource`].
    #[must_use]
    pub fn sources(&self) -> Vec<Arc<dyn LogSource>> {
        self.inner.sources.read().values().map(|s| Arc::clone(s) as Arc<dyn LogSource>).collect()
    }

    /// One source as a [`LogSource`].
    #[must_use]
    pub fn source(&self, id: &str) -> Option<Arc<dyn LogSource>> {
        self.inner.sources.read().get(id).map(|s| Arc::clone(s) as Arc<dyn LogSource>)
    }

    /// Live tail of one source (lagging consumers drop lines and count them).
    #[must_use]
    pub fn subscribe_logs(&self, id: &str) -> Option<TailReceiver> {
        self.inner.sources.read().get(id).map(|s| s.subscribe())
    }

    fn query_lines(&self, q: &LogQuery, until: Option<Timestamp>) -> Result<LogsResult, MonitorError> {
        let limit = q.limit.unwrap_or(500).clamp(1, 10_000) as usize;
        let selected: Vec<Arc<BufferedSource>> = {
            let sources = self.inner.sources.read();
            match &q.source_id {
                Some(id) => {
                    vec![sources.get(id).cloned().ok_or_else(|| MonitorError::NotFound(format!("log source {id}")))?]
                }
                None => sources
                    .values()
                    .filter(|s| {
                        q.project_id.is_none() || s.info().project_id.is_none() || s.info().project_id == q.project_id
                    })
                    .cloned()
                    .collect(),
            }
        };
        let mut truncated = false;
        let mut lines: Vec<LogLine> = Vec::new();
        for s in selected {
            let (l, t) = s.query_range(q, until);
            truncated |= t;
            lines.extend(l);
        }
        lines.sort_by_key(|a| a.ts);
        if lines.len() > limit {
            truncated = true;
            lines.drain(..lines.len() - limit);
        }
        Ok(LogsResult { lines, truncated })
    }

    /// `monitor.logs`.
    pub async fn logs(&self, q: &LogQuery) -> Result<LogsResult, MonitorError> {
        self.init().await?;
        self.query_lines(q, None)
    }

    /// Evidence: redacted lines within `[at - before, at + after]` (oldest first).
    pub async fn logs_around(
        &self,
        at: Timestamp,
        before: Duration,
        after: Duration,
        project: Option<ProjectId>,
        min_level: Option<LogLevel>,
        limit: u32,
    ) -> Result<Vec<LogLine>, MonitorError> {
        self.init().await?;
        let d = |x: Duration| chrono::Duration::from_std(x).unwrap_or(chrono::Duration::hours(1));
        let q = LogQuery {
            project_id: project,
            min_level,
            since: Some(at - d(before)),
            limit: Some(limit),
            ..LogQuery::default()
        };
        Ok(self.query_lines(&q, Some(at + d(after)))?.lines)
    }

    /// `monitor.ingest`: stores (redacted) lines under `ingest:<stream>`.
    pub async fn ingest(&self, p: IngestParams) -> Result<u32, MonitorError> {
        self.init().await?;
        let stream = p.stream.trim();
        if !valid_name(stream, 64) {
            return Err(MonitorError::Invalid("stream must be 1..64 of [A-Za-z0-9._-]".into()));
        }
        if p.lines.len() > MAX_INGEST_LINES {
            return Err(MonitorError::Invalid(format!("at most {MAX_INGEST_LINES} lines per call")));
        }
        let id = format!("ingest:{stream}");
        let existing = self.inner.sources.read().get(&id).cloned();
        let src = match existing {
            Some(s) => {
                if s.info().project_id.is_some() && p.project_id.is_some() && s.info().project_id != p.project_id {
                    return Err(MonitorError::Conflict(format!("stream {stream} belongs to another project")));
                }
                s
            }
            None => {
                let info = LogSourceInfo {
                    id: id.clone(),
                    name: stream.to_owned(),
                    kind: LogSourceKind::Ingest { stream: stream.to_owned() },
                    project_id: p.project_id,
                };
                self.inner.store.save_source(&info).await?;
                self.register_source(info)
            }
        };
        let mut accepted = 0u32;
        for l in p.lines {
            let text = format!("{}{}", l.message, logs::render_attrs(&l.attrs));
            if text.trim().is_empty() {
                continue;
            }
            src.push_raw(l.ts, l.level, &text);
            accepted += 1;
        }
        Ok(accepted)
    }

    async fn poll_deployments(&self, refresh: bool) {
        let Some(feed) = self.inner.deploy_feed.read().clone() else { return };
        if refresh {
            for d in feed.recent(MAX_DEPLOYMENTS).await {
                let id = format!("deploy:{}", d.deployment_id);
                if self.inner.sources.read().contains_key(&id) {
                    continue;
                }
                self.register_source(LogSourceInfo {
                    id,
                    name: d.name,
                    kind: LogSourceKind::Deployment { deployment_id: d.deployment_id },
                    project_id: d.project_id,
                });
                self.inner
                    .deploy_cursors
                    .lock()
                    .entry(d.deployment_id)
                    .or_insert(DeployCursor { offset: 0, complete: false });
            }
        }
        let pending: Vec<(DeploymentId, u64)> = self
            .inner
            .deploy_cursors
            .lock()
            .iter()
            .filter(|(_, c)| !c.complete)
            .map(|(id, c)| (*id, c.offset))
            .collect();
        for (dep, offset) in pending {
            let Some(src) = self.inner.sources.read().get(&format!("deploy:{dep}")).cloned() else { continue };
            match feed.read(dep, offset).await {
                Ok(r) => {
                    for l in &r.lines {
                        src.push_raw(None, None, l);
                    }
                    if let Some(c) = self.inner.deploy_cursors.lock().get_mut(&dep) {
                        c.offset = r.next_offset.max(offset);
                        c.complete = r.complete;
                    }
                }
                Err(e) => tracing::debug!(deployment = %dep, error = %e, "deployment log read failed"),
            }
        }
    }

    // ---- alerts --------------------------------------------------------

    /// `monitor.alerts` (newest first).
    pub async fn alerts(
        &self,
        project: Option<ProjectId>,
        include_resolved: bool,
        limit: Option<u32>,
    ) -> Result<Vec<Alert>, MonitorError> {
        self.init().await?;
        self.inner.store.list_alerts(project, include_resolved, limit.unwrap_or(100).clamp(1, 1000)).await
    }

    /// Evidence: unresolved alerts.
    pub async fn active_alerts(&self, project: Option<ProjectId>) -> Result<Vec<Alert>, MonitorError> {
        self.init().await?;
        Ok(self
            .inner
            .alerts
            .open_alerts()
            .await
            .into_iter()
            .filter(|a| project.is_none() || a.project_id == project)
            .collect())
    }

    /// Evidence: one alert by id.
    pub async fn alert(&self, id: AlertId) -> Result<Alert, MonitorError> {
        self.init().await?;
        self.inner.store.get_alert(id).await?.ok_or_else(|| MonitorError::NotFound(format!("alert {id}")))
    }

    /// `monitor.ackAlert`.
    pub async fn ack_alert(&self, id: AlertId) -> Result<Alert, MonitorError> {
        self.init().await?;
        self.inner.alerts.ack(id).await
    }

    /// Alert rules.
    pub async fn rules(&self) -> Result<Vec<AlertRule>, MonitorError> {
        self.init().await?;
        Ok(self.inner.alerts.rules())
    }

    /// Creates / replaces a rule.
    pub async fn upsert_rule(&self, rule: AlertRule) -> Result<(), MonitorError> {
        self.init().await?;
        if !valid_name(&rule.id, 64) {
            return Err(MonitorError::Invalid("rule id must be 1..64 of [A-Za-z0-9._-]".into()));
        }
        self.inner.store.save_rule(&rule).await?;
        let mut rules = self.inner.alerts.rules();
        match rules.iter_mut().find(|r| r.id == rule.id) {
            Some(r) => *r = rule,
            None => rules.push(rule),
        }
        self.inner.alerts.set_rules(rules);
        Ok(())
    }

    /// Deletes a rule (its open alerts stay until acknowledged/resolved).
    pub async fn delete_rule(&self, id: &str) -> Result<(), MonitorError> {
        self.init().await?;
        if !self.inner.store.delete_rule(id).await? {
            return Err(MonitorError::NotFound(format!("rule {id}")));
        }
        let rules = self.inner.alerts.rules().into_iter().filter(|r| r.id != id).collect();
        self.inner.alerts.set_rules(rules);
        Ok(())
    }
}

fn short_hash(s: &str) -> String {
    // FNV-1a: stable across runs (std's hasher is randomly seeded).
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in s.to_lowercase().bytes() {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}
