//! Benchmark collection and router auto-scoring (§22, §68.38–39) plus
//! context-index invalidation (§20).
//!
//! [`spawn_collector`] runs one background task that
//!
//! * feeds benchmark-relevant events to the cost engine
//!   ([`orkestr_cost::CostEngine::apply_benchmark_events`]) — first catching
//!   up from the persistent event log (the cost engine stores the last applied
//!   `seq`, so nothing is counted twice and nothing is lost across restarts or
//!   a lagging subscription), then live;
//! * refreshes [`BenchmarkStats`] — the [`ModelStatsSource`] the provider
//!   registry's auto router blends into its scores — after executions finish
//!   (at most every [`MIN_REFRESH_INTERVAL`]) and every [`REFRESH_EVERY`];
//! * invalidates the context optimizer's file-index cache
//!   ([`orkestr_context::global_cache`]) on `FileChanged`, for the project
//!   root and, for agent changes, the task's worktree.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use orkestr_domain::{ModelRef, ProjectId, ProviderId};
use orkestr_events::{Received, ReplayQuery};
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::benchmarks::ModelBenchmarkRow;
use orkestr_protocol::Event;
use orkestr_providers::history::{ModelHistory, ModelStatsSource, MIN_SAMPLES};
use parking_lot::{Mutex, RwLock};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

use crate::services::Services;

/// Window of the routing statistics (days).
pub const ROUTING_WINDOW_DAYS: u32 = 30;
/// Periodic refresh of the routing statistics.
pub const REFRESH_EVERY: Duration = Duration::from_secs(5 * 60);
/// Min interval between event-triggered refreshes.
pub const MIN_REFRESH_INTERVAL: Duration = Duration::from_secs(20);
/// Replay page size.
const REPLAY_PAGE: u32 = 1_000;

/// Whether an event matters for benchmarks.
#[must_use]
pub fn is_benchmark_event(kind: &EventKind) -> bool {
    matches!(
        kind,
        EventKind::AgentSpawned(_)
            | EventKind::AgentStarted(_)
            | EventKind::AgentStopped(_)
            | EventKind::TaskStatusChanged(_)
            | EventKind::TaskCreated(_)
            | EventKind::ReviewFinished(_)
            | EventKind::BuildSucceeded(_)
            | EventKind::BuildFailed(_)
            | EventKind::TestRunFinished(_)
    )
}

/// Router history of a benchmark row.
#[must_use]
pub fn history_of(row: &ModelBenchmarkRow) -> ModelHistory {
    let samples = u32::try_from(row.executions.max(row.calls)).unwrap_or(u32::MAX);
    let first_pass = row.first_pass_success_rate.filter(|_| row.finished >= u64::from(MIN_SAMPLES));
    ModelHistory {
        samples,
        call_success_rate: row.call_success_rate.or(first_pass).unwrap_or(0.5),
        first_pass_rate: first_pass,
        avg_latency_ms: row.latency_avg_ms,
        avg_cost_usd: row.cost_avg_usd,
    }
}

/// Routing score (0..1) of a row, `None` below the sample minimum.
#[must_use]
pub fn routing_score(row: &ModelBenchmarkRow) -> Option<f64> {
    let h = history_of(row);
    (h.samples >= MIN_SAMPLES).then(|| h.score())
}

type Key = (ProviderId, String, Option<String>);

/// Snapshot of per-model (and per-model-and-role) history for the router.
#[derive(Debug, Default)]
pub struct BenchmarkStats {
    map: RwLock<HashMap<Key, ModelHistory>>,
    refreshed: Mutex<Option<Instant>>,
}

impl BenchmarkStats {
    /// Empty snapshot.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Recomputes the snapshot from the cost engine.
    pub async fn refresh(&self, cost: &orkestr_cost::CostEngine) -> orkestr_cost::CostResult<usize> {
        let rows = cost.model_routing_stats(ROUTING_WINDOW_DAYS).await?;
        let mut map = HashMap::with_capacity(rows.len());
        for s in rows {
            let role = s.role.map(|r| r.to_lowercase());
            map.insert((s.row.provider_id, s.row.model.clone(), role), history_of(&s.row));
        }
        let n = map.len();
        *self.map.write() = map;
        *self.refreshed.lock() = Some(Instant::now());
        Ok(n)
    }

    /// Time since the last refresh (`None` = never).
    #[must_use]
    pub fn age(&self) -> Option<Duration> {
        self.refreshed.lock().map(|t| t.elapsed())
    }

    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        self.map.read().len()
    }

    /// Whether the snapshot is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

impl ModelStatsSource for BenchmarkStats {
    fn history(&self, model: &ModelRef, purpose: Option<&str>) -> Option<ModelHistory> {
        let map = self.map.read();
        if let Some(p) = purpose.map(str::to_lowercase) {
            if let Some(h) = map.get(&(model.provider_id, model.model.clone(), Some(p))) {
                if h.samples >= MIN_SAMPLES {
                    return Some(*h);
                }
            }
        }
        map.get(&(model.provider_id, model.model.clone(), None)).copied()
    }
}

struct Collector {
    services: Arc<Services>,
    stats: Arc<BenchmarkStats>,
    roots: HashMap<ProjectId, Option<PathBuf>>,
    dirty: bool,
}

impl Collector {
    async fn apply(&mut self, events: Vec<Event>) {
        if events.is_empty() {
            return;
        }
        match self.services.cost.apply_benchmark_events(events).await {
            Ok(r) => self.dirty |= r.finished > 0,
            Err(e) => tracing::warn!(error = %e, "benchmark collection failed"),
        }
    }

    /// Replays the persistent log from the stored cursor up to `upto`.
    async fn catch_up(&mut self, upto: u64) {
        if !self.services.events.is_persistent() {
            return;
        }
        let mut from = match self.services.cost.benchmark_cursor().await {
            Ok(c) => c + 1,
            Err(e) => {
                tracing::warn!(error = %e, "benchmark cursor unavailable");
                return;
            }
        };
        while from <= upto {
            let q = ReplayQuery { from_seq: from, to_seq: Some(upto), limit: REPLAY_PAGE, ..ReplayQuery::default() };
            let page = match self.services.events.replay(&q).await {
                Ok(p) => p,
                Err(e) => {
                    tracing::warn!(error = %e, "benchmark replay failed");
                    return;
                }
            };
            let Some(last) = page.events.last().map(|e| e.seq) else { break };
            let relevant: Vec<Event> = page.events.into_iter().filter(|e| is_benchmark_event(&e.kind)).collect();
            self.apply(relevant).await;
            if !page.has_more {
                break;
            }
            from = last + 1;
        }
    }

    async fn project_root(&mut self, id: ProjectId) -> Option<PathBuf> {
        if let Some(r) = self.roots.get(&id) {
            return r.clone();
        }
        let root = self.services.projects.get(id).await.ok().flatten().and_then(|p| p.root_path).map(PathBuf::from);
        self.roots.insert(id, root.clone());
        root
    }

    async fn invalidate(&mut self, ev: &Event) {
        let EventKind::FileChanged(p) = &ev.kind else { return };
        let mut roots: Vec<PathBuf> = Vec::new();
        if let Some(pid) = ev.project_id {
            if let Some(r) = self.project_root(pid).await {
                roots.push(r);
            }
        }
        if let (Some(_), Some(task)) = (ev.agent_id, ev.task_id) {
            if let Ok(t) = self.services.tasks.get(task).await {
                if let Some(w) = t.worktree {
                    roots.push(PathBuf::from(w.path));
                }
            }
        }
        let cache = orkestr_context::global_cache();
        for root in &roots {
            cache.invalidate_path(root, &p.path);
            if let Some(old) = &p.old_path {
                cache.invalidate_path(root, old);
            }
        }
    }

    async fn refresh(&mut self, force: bool) {
        let due = force || (self.dirty && self.stats.age().is_none_or(|a| a >= MIN_REFRESH_INTERVAL));
        if !due {
            return;
        }
        match self.stats.refresh(&self.services.cost).await {
            Ok(n) => {
                self.dirty = false;
                tracing::debug!(entries = n, "router benchmark statistics refreshed");
            }
            Err(e) => tracing::warn!(error = %e, "router benchmark statistics refresh failed"),
        }
    }

    async fn run(mut self, shutdown: CancellationToken) {
        let mut sub = self.services.events.subscribe_all();
        let upto = self.services.events.last_seq();
        self.catch_up(upto).await;
        self.refresh(true).await;
        let mut tick = tokio::time::interval(REFRESH_EVERY);
        tick.tick().await;
        let mut throttle = tokio::time::interval(MIN_REFRESH_INTERVAL);
        throttle.tick().await;
        loop {
            tokio::select! {
                () = shutdown.cancelled() => break,
                _ = tick.tick() => self.refresh(true).await,
                _ = throttle.tick() => self.refresh(false).await,
                r = sub.recv() => match r {
                    Some(Received::Event(ev)) => {
                        match &ev.kind {
                            EventKind::FileChanged(_) => self.invalidate(&ev).await,
                            EventKind::ProjectUpdated(p) => {
                                self.roots.remove(&p.project.id);
                                if let Some(r) = &p.project.root_path {
                                    orkestr_context::global_cache().invalidate_root(std::path::Path::new(r));
                                }
                            }
                            k if is_benchmark_event(k) => self.apply(vec![(*ev).clone()]).await,
                            _ => {}
                        }
                    }
                    Some(Received::Lagged(n)) => {
                        tracing::warn!(dropped = n, "benchmark collector lagged; replaying");
                        let upto = self.services.events.last_seq();
                        self.catch_up(upto).await;
                    }
                    None => break,
                },
            }
        }
    }
}

/// Starts the collector (wires `stats` into the router first).
pub fn spawn_collector(services: Arc<Services>, shutdown: CancellationToken) -> JoinHandle<()> {
    let stats = Arc::clone(&services.benchmarks);
    services.providers.set_stats_source(Arc::clone(&stats) as Arc<dyn ModelStatsSource>);
    let collector = Collector { services, stats, roots: HashMap::new(), dirty: false };
    tokio::spawn(collector.run(shutdown))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(executions: u64, calls: u64, finished: u64, fp: Option<f64>) -> ModelBenchmarkRow {
        ModelBenchmarkRow {
            provider_id: ProviderId::new(),
            model: "m".into(),
            roles: vec![],
            executions,
            finished,
            calls,
            call_success_rate: (calls > 0).then_some(0.9),
            first_pass_success_rate: fp,
            build_success_rate: None,
            defects_found_avg: None,
            retries_avg: None,
            latency_avg_ms: Some(800.0),
            latency_p50_ms: None,
            latency_p95_ms: None,
            duration_avg_ms: None,
            input_tokens: 0,
            output_tokens: 0,
            tokens_avg: None,
            cost_total_usd: 0.0,
            cost_avg_usd: Some(0.02),
            routing_score: None,
        }
    }

    #[test]
    fn history_and_lookup() {
        let r = row(10, 40, 2, Some(1.0));
        let h = history_of(&r);
        assert_eq!(h.samples, 40);
        assert_eq!(h.first_pass_rate, None, "too few finished executions");
        assert!(routing_score(&r).is_some());
        assert!(routing_score(&row(1, 1, 0, None)).is_none());
        let finished = history_of(&row(5, 0, 5, Some(0.8)));
        assert_eq!(finished.first_pass_rate, Some(0.8));
        assert!((finished.call_success_rate - 0.8).abs() < 1e-9);

        let stats = BenchmarkStats::new();
        let p = ProviderId::new();
        let mr = ModelRef { provider_id: p, model: "m".into() };
        let overall = ModelHistory { samples: 30, ..h };
        let backend = ModelHistory { samples: 5, call_success_rate: 0.1, ..h };
        let few = ModelHistory { samples: 1, ..h };
        stats.map.write().insert((p, "m".into(), None), overall);
        stats.map.write().insert((p, "m".into(), Some("backend".into())), backend);
        stats.map.write().insert((p, "m".into(), Some("qa".into())), few);
        assert_eq!(stats.history(&mr, Some("Backend")), Some(backend), "role-specific when confident");
        assert_eq!(stats.history(&mr, Some("QA")), Some(overall), "too few role samples → overall");
        assert_eq!(stats.history(&mr, None), Some(overall));
        assert!(stats.history(&ModelRef { provider_id: p, model: "x".into() }, None).is_none());
    }
}
