//! Architecture Map (§21, §68.48).
//!
//! [`ArchitectureService`] owns one incremental [`scan::Scanner`] per project,
//! builds the model ([`model::build`]), stores a snapshot whenever the model's
//! digest changes ([`store::ArchStore`]), diffs snapshots ([`diff::diff`]) and
//! publishes `ArchitectureMapUpdated`. [`spawn_auto_refresh`] keeps maps that
//! were opened at least once up to date after `FileChanged` events.

pub mod diff;
pub mod facts;
pub mod manifests;
pub mod model;
pub mod routes;
pub mod scan;
pub mod store;
pub mod yaml;

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{now, ProjectId};
use orkestr_events::{EventBus, ReplayQuery};
use orkestr_protocol::event::{ArchitectureMapUpdatedPayload, EventKind, NewEvent};
use orkestr_protocol::methods::architecture::{ArchDiff, ArchSnapshotInfo, ArchitectureMap};
use orkestr_storage::{Db, ProjectRepository};
use tokio::task::JoinHandle;
use tokio_util::sync::CancellationToken;

pub use scan::ScanOptions;
pub use store::insight_migrations;

use crate::error::{InsightError, InsightResult};

/// Outcome of a refresh.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RefreshOutcome {
    /// Latest map.
    pub map: ArchitectureMap,
    /// A new snapshot was stored.
    pub changed: bool,
    /// Diff to the previous snapshot (when one existed and the map changed).
    pub diff: Option<ArchDiff>,
}

type ScannerSlot = Arc<tokio::sync::Mutex<scan::Scanner>>;

struct Inner {
    store: store::ArchStore,
    events: Option<EventBus>,
    opts: ScanOptions,
    scanners: parking_lot::Mutex<HashMap<ProjectId, ScannerSlot>>,
}

/// The Architecture Map engine.
#[derive(Clone)]
pub struct ArchitectureService {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for ArchitectureService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ArchitectureService").field("opts", &self.inner.opts).finish_non_exhaustive()
    }
}

/// Model digest (blake3 over the canonical JSON of nodes + edges).
#[must_use]
pub fn digest(nodes: &[orkestr_protocol::methods::architecture::ArchNode], edges: &[orkestr_protocol::methods::architecture::ArchEdge]) -> String {
    let mut h = blake3::Hasher::new();
    h.update(serde_json::to_string(nodes).unwrap_or_default().as_bytes());
    h.update(b"\n");
    h.update(serde_json::to_string(edges).unwrap_or_default().as_bytes());
    h.finalize().to_hex().to_string()
}

impl ArchitectureService {
    /// New service over a migrated database (see [`insight_migrations`]).
    #[must_use]
    pub fn new(db: Db, events: Option<EventBus>, opts: ScanOptions) -> Self {
        Self {
            inner: Arc::new(Inner {
                store: store::ArchStore::new(db),
                events,
                opts,
                scanners: parking_lot::Mutex::new(HashMap::new()),
            }),
        }
    }

    /// Snapshot store.
    #[must_use]
    pub fn store(&self) -> &store::ArchStore {
        &self.inner.store
    }

    fn scanner(&self, project: ProjectId, root: &Path) -> ScannerSlot {
        let mut map = self.inner.scanners.lock();
        if let Some(slot) = map.get(&project) {
            // A root change (project moved) starts a fresh cache.
            if slot.try_lock().map_or(true, |s| s.root() == root) {
                return Arc::clone(slot);
            }
        }
        let slot = Arc::new(tokio::sync::Mutex::new(scan::Scanner::new(root, self.inner.opts.clone())));
        map.insert(project, Arc::clone(&slot));
        slot
    }

    /// Scans the project (incrementally unless `full`) and stores a snapshot
    /// when the model changed. Refreshes of one project are serialised.
    pub async fn refresh(&self, project: ProjectId, root: PathBuf, full: bool, automatic: bool) -> InsightResult<RefreshOutcome> {
        let slot = self.scanner(project, &root);
        let mut guard = slot.lock_owned().await;
        let root_name = root.file_name().map_or_else(|| "project".to_owned(), |n| n.to_string_lossy().into_owned());
        let (output, guard_back) = tokio::task::spawn_blocking(move || {
            let out = guard.scan(full);
            let model = out.as_ref().ok().map(|o| model::build(&o.files, &o.facts, &root_name));
            (out.map(|o| (o, model)), guard)
        })
        .await
        .map_err(|e| InsightError::Aborted(e.to_string()))?;
        let (scan_out, model) = output?;
        let model = model.unwrap_or_default();
        let digest = digest(&model.nodes, &model.edges);
        let mut stats = model.stats();
        stats.files_scanned = u32::try_from(scan_out.files.len()).unwrap_or(u32::MAX);
        stats.files_parsed = scan_out.parsed;
        stats.scan_ms = scan_out.elapsed_ms;
        let previous = self.inner.store.latest(project).await?;
        if let Some(prev) = previous.as_ref().filter(|p| p.digest == digest) {
            drop(guard_back);
            return Ok(RefreshOutcome { map: prev.clone(), changed: false, diff: None });
        }
        let mut warnings = model.warnings;
        warnings.extend(scan_out.warnings);
        let map = ArchitectureMap {
            snapshot_id: uuid::Uuid::now_v7().to_string(),
            project_id: project,
            generated_at: now(),
            digest,
            nodes: model.nodes,
            edges: model.edges,
            stats,
            warnings,
            truncated: scan_out.truncated,
        };
        self.inner.store.insert(&map).await?;
        drop(guard_back);
        let diff = previous.as_ref().map(|p| diff::diff(Some(p), &map));
        if let Some(bus) = &self.inner.events {
            let d = diff.clone().unwrap_or_else(|| diff::diff(None, &map));
            let payload = ArchitectureMapUpdatedPayload {
                project_id: project,
                snapshot_id: map.snapshot_id.clone(),
                stats: map.stats.clone(),
                added_nodes: u32::try_from(d.added_nodes.len()).unwrap_or(u32::MAX),
                removed_nodes: u32::try_from(d.removed_nodes.len()).unwrap_or(u32::MAX),
                changed_nodes: u32::try_from(d.changed_nodes.len()).unwrap_or(u32::MAX),
                changed_edges: u32::try_from(d.added_edges.len() + d.removed_edges.len()).unwrap_or(u32::MAX),
                automatic,
            };
            if let Err(e) = bus.publish(NewEvent::new(EventKind::ArchitectureMapUpdated(payload)).project(project)).await {
                tracing::warn!(error = %e, "failed to publish ArchitectureMapUpdated");
            }
        }
        tracing::info!(%project, nodes = map.nodes.len(), edges = map.edges.len(), automatic, "architecture snapshot stored");
        Ok(RefreshOutcome { map, changed: true, diff })
    }

    /// Latest snapshot, or a stored one.
    pub async fn get(&self, project: ProjectId, snapshot: Option<&str>) -> InsightResult<Option<ArchitectureMap>> {
        match snapshot {
            Some(id) => self.inner.store.get(project, id).await.map(Some),
            None => self.inner.store.latest(project).await,
        }
    }

    /// Snapshot history, newest first.
    pub async fn snapshots(&self, project: ProjectId) -> InsightResult<Vec<ArchSnapshotInfo>> {
        self.inner.store.list(project).await
    }

    /// Diff between two snapshots. Defaults: `to` = latest, `from` = the one
    /// before `to` (or the empty model).
    pub async fn diff(&self, project: ProjectId, from: Option<&str>, to: Option<&str>) -> InsightResult<ArchDiff> {
        let list = self.inner.store.list(project).await?;
        let to_id = match to {
            Some(t) => t.to_owned(),
            None => list
                .first()
                .map(|s| s.snapshot_id.clone())
                .ok_or_else(|| InsightError::NotFound("architecture snapshot (run architecture.refresh first)".into()))?,
        };
        let to_map = self.inner.store.get(project, &to_id).await?;
        let from_map = match from {
            Some(f) => Some(self.inner.store.get(project, f).await?),
            None => {
                let pos = list.iter().position(|s| s.snapshot_id == to_id);
                match pos.and_then(|p| list.get(p + 1)) {
                    Some(prev) => Some(self.inner.store.get(project, &prev.snapshot_id).await?),
                    None => None,
                }
            }
        };
        Ok(diff::diff(from_map.as_ref(), &to_map))
    }

    /// Drops the in-memory cache of a project.
    pub fn forget(&self, project: ProjectId) {
        self.inner.scanners.lock().remove(&project);
    }
}

/// Auto-refresh tuning.
#[derive(Debug, Clone)]
pub struct AutoRefreshConfig {
    /// Quiet period after the last `FileChanged` of a project.
    pub debounce: Duration,
    /// Minimum time between two automatic scans of one project.
    pub min_interval: Duration,
    /// How often the persisted event log is checked for new `FileChanged`
    /// events (a cheap in-memory `last_seq` comparison when nothing happened).
    pub poll: Duration,
}

impl Default for AutoRefreshConfig {
    fn default() -> Self {
        Self { debounce: Duration::from_secs(3), min_interval: Duration::from_secs(10), poll: Duration::from_secs(1) }
    }
}

/// Max events read from the log per poll (the rest is read on the next poll).
const TAIL_PAGE: u32 = 1000;
const TAIL_MAX_PAGES: usize = 20;

/// Projects with new `FileChanged` events in `(cursor, last]`; returns the new cursor.
async fn tail_file_changes(bus: &EventBus, cursor: u64, out: &mut Vec<ProjectId>) -> InsightResult<u64> {
    let last_before = bus.last_seq();
    let mut from = cursor + 1;
    let mut finished = false;
    for _ in 0..TAIL_MAX_PAGES {
        let page = bus
            .replay(&ReplayQuery { from_seq: from, to_seq: None, limit: TAIL_PAGE, topics: None, newest_first: false })
            .await?;
        for ev in &page.events {
            from = ev.seq + 1;
            if let (EventKind::FileChanged(_), Some(pid)) = (&ev.kind, ev.project_id) {
                if !out.contains(&pid) {
                    out.push(pid);
                }
            }
        }
        if !page.has_more || page.events.is_empty() {
            finished = true;
            break;
        }
    }
    let read_up_to = from.saturating_sub(1).max(cursor);
    // Ephemeral events are never persisted: once the log is drained, skip
    // their seqs too (everything up to `last_before` was either read or not stored).
    Ok(if finished { read_up_to.max(last_before) } else { read_up_to })
}

/// Refreshes maps after `FileChanged` events (debounced, rate-limited). Only
/// projects that already have a snapshot are refreshed — nothing is scanned
/// for projects whose map was never opened.
///
/// The task tails the persisted event log (it does not hold a live bus
/// subscription, so it never counts as a connected listener — the core skips
/// work such as node heartbeats while nobody listens).
pub fn spawn_auto_refresh(
    service: ArchitectureService,
    bus: EventBus,
    projects: Arc<dyn ProjectRepository>,
    cfg: AutoRefreshConfig,
    shutdown: CancellationToken,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        use tokio::time::Instant;
        if !bus.is_persistent() {
            tracing::warn!("architecture auto-refresh disabled: the event bus has no persistent log");
            return;
        }
        let mut cursor = bus.last_seq();
        let mut due: HashMap<ProjectId, Instant> = HashMap::new();
        let mut last_run: HashMap<ProjectId, Instant> = HashMap::new();
        loop {
            let next_poll = Instant::now() + cfg.poll;
            let wake = due.values().min().copied().map_or(next_poll, |t| t.min(next_poll));
            tokio::select! {
                () = shutdown.cancelled() => break,
                () = tokio::time::sleep_until(wake) => {}
            }
            let last = bus.last_seq();
            if last > cursor {
                let mut changed = Vec::new();
                match tail_file_changes(&bus, cursor, &mut changed).await {
                    Ok(c) => cursor = c.max(cursor),
                    Err(e) => {
                        tracing::warn!(error = %e, "architecture auto-refresh: reading the event log failed");
                        cursor = last;
                    }
                }
                for pid in changed {
                    let mut at = Instant::now() + cfg.debounce;
                    if let Some(prev) = last_run.get(&pid) {
                        at = at.max(*prev + cfg.min_interval);
                    }
                    due.insert(pid, at);
                }
            }
            let now_i = Instant::now();
            let ready: Vec<ProjectId> = due.iter().filter(|(_, t)| **t <= now_i).map(|(p, _)| *p).collect();
            for pid in ready {
                due.remove(&pid);
                last_run.insert(pid, now_i);
                match service.store().has_any(pid).await {
                    Ok(true) => {}
                    Ok(false) => continue,
                    Err(e) => {
                        tracing::warn!(error = %e, "architecture auto-refresh: store check failed");
                        continue;
                    }
                }
                let root = match projects.get(pid).await {
                    Ok(Some(p)) => p.root_path.filter(|r| !r.trim().is_empty()).map(PathBuf::from),
                    _ => None,
                };
                let Some(root) = root.filter(|r| r.is_dir()) else { continue };
                match service.refresh(pid, root, false, true).await {
                    Ok(o) if o.changed => tracing::debug!(project = %pid, "architecture map auto-refreshed"),
                    Ok(_) => {}
                    Err(e) => tracing::warn!(project = %pid, error = %e, "architecture auto-refresh failed"),
                }
            }
        }
    })
}
