//! Metric ring buffers fed by `NodeMonitor` (local) and `NodeHub` (nodes).
//!
//! Every node keeps a bounded ring of [`NodeMetricsSnapshot`]s (oldest
//! dropped first). Series are derived on read: `cpu`, `mem`, `disk` and
//! `gpuN` in percent, optionally averaged into fixed-width buckets.

use std::collections::{HashMap, VecDeque};

use async_trait::async_trait;
use orkestr_domain::{Node, NodeId, NodeStatus, Timestamp};
use orkestr_protocol::methods::monitor::{MetricPoint, MetricSeries, NodeMetricsSnapshot};
use orkestr_protocol::methods::nodes::ResourceSample;
use parking_lot::RwLock;

/// Remote nodes known to the coordinator. The integrator wires this to
/// `Services.node_hub` snapshots (`snapshots().await.into_iter().map(|s| s.node)`).
#[async_trait]
pub trait NodeMetricsSource: Send + Sync {
    /// Current view of every enrolled node (the local node may be included;
    /// it is skipped because the local sampler already covers it).
    async fn nodes(&self) -> Vec<Node>;
}

/// Metric names served by [`MetricsStore::series`].
pub const BASE_METRICS: [&str; 3] = ["cpu", "mem", "disk"];

/// Converts a local sample.
#[must_use]
pub fn snapshot_from_sample(name: &str, s: &ResourceSample) -> NodeMetricsSnapshot {
    NodeMetricsSnapshot {
        node_id: s.node_id,
        name: name.to_owned(),
        local: true,
        online: true,
        cpu_percent: s.cpu_percent,
        mem_used_bytes: s.memory_used_bytes,
        mem_total_bytes: s.memory_total_bytes,
        gpu_percent: Vec::new(),
        disk_used_bytes: s.disk_total_bytes.saturating_sub(s.disk_free_bytes),
        disk_total_bytes: s.disk_total_bytes,
        sampled_at: s.ts,
    }
}

/// Converts a node description (heartbeat data).
#[must_use]
pub fn snapshot_from_node(n: &Node, ts: Timestamp) -> NodeMetricsSnapshot {
    let r = &n.resources;
    NodeMetricsSnapshot {
        node_id: n.id,
        name: n.name.clone(),
        local: n.is_local,
        online: !matches!(n.status, NodeStatus::Offline | NodeStatus::Suspect),
        cpu_percent: r.cpu_usage_percent,
        mem_used_bytes: r.memory_used_bytes,
        mem_total_bytes: r.memory_total_bytes,
        gpu_percent: Vec::new(),
        disk_used_bytes: r.disk_total_bytes.saturating_sub(r.disk_free_bytes),
        disk_total_bytes: r.disk_total_bytes,
        sampled_at: n.last_heartbeat_at.unwrap_or(ts),
    }
}

#[allow(clippy::cast_precision_loss)]
fn pct(used: u64, total: u64) -> f64 {
    if total == 0 {
        0.0
    } else {
        used as f64 * 100.0 / total as f64
    }
}

/// Value of `metric` in a snapshot (percent), if present.
#[must_use]
pub fn metric_value(s: &NodeMetricsSnapshot, metric: &str) -> Option<f64> {
    match metric {
        "cpu" => Some(f64::from(s.cpu_percent)),
        "mem" => Some(pct(s.mem_used_bytes, s.mem_total_bytes)),
        "disk" => Some(pct(s.disk_used_bytes, s.disk_total_bytes)),
        m => m
            .strip_prefix("gpu")
            .and_then(|i| i.parse::<usize>().ok())
            .and_then(|i| s.gpu_percent.get(i))
            .map(|v| f64::from(*v)),
    }
}

struct Ring {
    latest: NodeMetricsSnapshot,
    samples: VecDeque<NodeMetricsSnapshot>,
}

/// Bounded per-node history.
pub struct MetricsStore {
    capacity: usize,
    nodes: RwLock<HashMap<NodeId, Ring>>,
}

impl std::fmt::Debug for MetricsStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MetricsStore")
            .field("capacity", &self.capacity)
            .field("nodes", &self.nodes.read().len())
            .finish()
    }
}

impl MetricsStore {
    /// A store keeping at most `capacity` samples per node.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self { capacity: capacity.max(2), nodes: RwLock::new(HashMap::new()) }
    }

    /// Appends a sample. A sample not newer than the last one only refreshes
    /// `latest` (heartbeats may repeat between polls).
    pub fn push(&self, s: NodeMetricsSnapshot) {
        let mut nodes = self.nodes.write();
        let ring = nodes.entry(s.node_id).or_insert_with(|| Ring { latest: s.clone(), samples: VecDeque::new() });
        let newer = ring.samples.back().is_none_or(|b| s.sampled_at > b.sampled_at);
        ring.latest = s.clone();
        if newer {
            if ring.samples.len() == self.capacity {
                ring.samples.pop_front();
            }
            ring.samples.push_back(s);
        }
    }

    /// Marks nodes absent from `present` offline (keeps their history).
    pub fn mark_missing_offline(&self, present: &[NodeId], keep: NodeId) {
        for (id, ring) in self.nodes.write().iter_mut() {
            if *id != keep && !present.contains(id) {
                ring.latest.online = false;
            }
        }
    }

    /// Latest snapshot per node; local first then by name.
    #[must_use]
    pub fn latest(&self) -> Vec<NodeMetricsSnapshot> {
        let mut v: Vec<_> = self.nodes.read().values().map(|r| r.latest.clone()).collect();
        v.sort_by(|a, b| b.local.cmp(&a.local).then_with(|| a.name.cmp(&b.name)));
        v
    }

    /// Number of stored samples for `node`.
    #[must_use]
    pub fn len(&self, node: NodeId) -> usize {
        self.nodes.read().get(&node).map_or(0, |r| r.samples.len())
    }

    /// Last `n` values of `metric` for `node` (oldest first).
    #[must_use]
    pub fn last_values(&self, node: NodeId, metric: &str, n: usize) -> Vec<f64> {
        let nodes = self.nodes.read();
        let Some(ring) = nodes.get(&node) else { return Vec::new() };
        let skip = ring.samples.len().saturating_sub(n);
        ring.samples.iter().skip(skip).filter_map(|s| metric_value(s, metric)).collect()
    }

    /// Series of `node` since `since`, optionally bucketed by `resolution_secs`.
    /// Empty `metrics` = `cpu`, `mem`, `disk` plus every GPU.
    #[must_use]
    pub fn series(
        &self,
        node: NodeId,
        metrics: &[String],
        since: Timestamp,
        resolution_secs: Option<u32>,
    ) -> Vec<MetricSeries> {
        let nodes = self.nodes.read();
        let Some(ring) = nodes.get(&node) else { return Vec::new() };
        let names: Vec<String> = if metrics.is_empty() {
            let gpus = ring.latest.gpu_percent.len();
            BASE_METRICS.iter().map(|s| (*s).to_owned()).chain((0..gpus).map(|i| format!("gpu{i}"))).collect()
        } else {
            metrics.to_vec()
        };
        names
            .into_iter()
            .map(|metric| {
                let raw: Vec<MetricPoint> = ring
                    .samples
                    .iter()
                    .filter(|s| s.sampled_at >= since)
                    .filter_map(|s| metric_value(s, &metric).map(|value| MetricPoint { ts: s.sampled_at, value }))
                    .collect();
                let points = match resolution_secs {
                    Some(r) if r > 1 => bucket(&raw, i64::from(r)),
                    _ => raw,
                };
                MetricSeries { node_id: node, metric, unit: "percent".into(), points }
            })
            .collect()
    }
}

/// Averages points into `width`-second buckets (timestamp = bucket start).
#[allow(clippy::cast_precision_loss)]
fn bucket(points: &[MetricPoint], width: i64) -> Vec<MetricPoint> {
    let mut out: Vec<MetricPoint> = Vec::new();
    let mut cur: Option<(i64, f64, u32)> = None;
    let flush = |out: &mut Vec<MetricPoint>, (b, sum, n): (i64, f64, u32)| {
        if let Some(ts) = chrono::DateTime::from_timestamp(b * width, 0) {
            out.push(MetricPoint { ts, value: sum / f64::from(n) });
        }
    };
    for p in points {
        let b = p.ts.timestamp().div_euclid(width);
        cur = match cur {
            Some((cb, sum, n)) if cb == b => Some((cb, sum + p.value, n + 1)),
            Some(prev) => {
                flush(&mut out, prev);
                Some((b, p.value, 1))
            }
            None => Some((b, p.value, 1)),
        };
    }
    if let Some(c) = cur {
        flush(&mut out, c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snap(node: NodeId, secs: i64, cpu: f32) -> NodeMetricsSnapshot {
        NodeMetricsSnapshot {
            node_id: node,
            name: "n".into(),
            local: true,
            online: true,
            cpu_percent: cpu,
            mem_used_bytes: 50,
            mem_total_bytes: 100,
            gpu_percent: vec![10.0],
            disk_used_bytes: 1,
            disk_total_bytes: 4,
            sampled_at: chrono::DateTime::from_timestamp(1_700_000_000 + secs, 0).unwrap(),
        }
    }

    #[test]
    fn ring_keeps_only_capacity_newest_samples() {
        let store = MetricsStore::new(10);
        let n = NodeId::new();
        for i in 0..25 {
            store.push(snap(n, i, i as f32));
        }
        assert_eq!(store.len(n), 10);
        assert_eq!(store.last_values(n, "cpu", 3), vec![22.0, 23.0, 24.0]);
        // Duplicate timestamp refreshes latest only.
        store.push(snap(n, 24, 99.0));
        assert_eq!(store.len(n), 10);
        assert!((store.latest()[0].cpu_percent - 99.0).abs() < f32::EPSILON);
        let all = store.series(n, &[], chrono::DateTime::from_timestamp(0, 0).unwrap(), None);
        let names: Vec<_> = all.iter().map(|s| s.metric.as_str()).collect();
        assert_eq!(names, ["cpu", "mem", "disk", "gpu0"]);
        assert_eq!(all[0].points.len(), 10);
        assert!((all[1].points[0].value - 50.0).abs() < 1e-9);
        assert!((all[2].points[0].value - 25.0).abs() < 1e-9);
    }

    #[test]
    fn series_bucketing_averages() {
        let store = MetricsStore::new(100);
        let n = NodeId::new();
        // 1_700_000_000 is divisible by 10 → buckets [0..10), [10..20).
        for i in 0..20 {
            store.push(snap(n, i, if i < 10 { 10.0 } else { 30.0 }));
        }
        let s = store.series(n, &["cpu".into()], chrono::DateTime::from_timestamp(0, 0).unwrap(), Some(10));
        let v: Vec<f64> = s[0].points.iter().map(|p| p.value).collect();
        assert_eq!(v, vec![10.0, 30.0]);
        let since = chrono::DateTime::from_timestamp(1_700_000_015, 0).unwrap();
        assert_eq!(store.series(n, &["cpu".into()], since, None)[0].points.len(), 5);
    }
}
