//! Live resource sampler (§27 monitor, §11 resource-aware scheduling).
//!
//! Keeps one `sysinfo::System` alive between samples (CPU usage is a delta
//! between two refreshes, so the first sample reports 0 %). Each sample
//! refreshes CPU, memory, the data volume and process CPU/RAM.

use std::path::{Path, PathBuf};

use orkestr_domain::NodeId;
use orkestr_protocol::methods::nodes::{ProcessUsage, ResourceSample};
use sysinfo::{Disks, Pid, ProcessRefreshKind, ProcessesToUpdate, System};

/// Number of busiest processes reported.
pub const TOP_PROCESSES: usize = 5;

/// Stateful sampler; not `Sync` on purpose — wrap it in a mutex.
pub struct ResourceSampler {
    node_id: NodeId,
    system: System,
    disks: Disks,
    disk_anchor: PathBuf,
    own_pid: Pid,
    samples: u64,
}

impl std::fmt::Debug for ResourceSampler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ResourceSampler").field("node_id", &self.node_id).field("samples", &self.samples).finish()
    }
}

/// Canonical path without the Windows verbatim prefix (`\\?\C:\x` → `C:\x`),
/// so it can be compared with mount points.
#[must_use]
pub fn comparable_path(p: &Path) -> PathBuf {
    let canon = p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    let s = canon.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) if !rest.starts_with("UNC\\") => PathBuf::from(rest),
        _ => canon,
    }
}

/// Picks the disk whose mount point is the longest prefix of `anchor`.
fn data_volume(disks: &Disks, anchor: &Path) -> (u64, u64) {
    let anchor = comparable_path(anchor);
    disks
        .list()
        .iter()
        .filter(|d| anchor.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .or_else(|| disks.list().first())
        .map_or((0, 0), |d| (d.total_space(), d.available_space()))
}

fn round1(v: f32) -> f32 {
    if v.is_finite() {
        (v * 10.0).round() / 10.0
    } else {
        0.0
    }
}

impl ResourceSampler {
    /// Creates a sampler reporting the volume holding `disk_anchor`.
    #[must_use]
    pub fn new(node_id: NodeId, disk_anchor: PathBuf) -> Self {
        let mut system = System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self {
            node_id,
            system,
            disks: Disks::new_with_refreshed_list(),
            disk_anchor,
            own_pid: Pid::from_u32(std::process::id()),
            samples: 0,
        }
    }

    fn usage(&self, pid: Pid, p: &sysinfo::Process, cores: f32) -> ProcessUsage {
        // sysinfo reports 100 % per fully used core.
        let raw = p.cpu_usage();
        let raw = if raw.is_finite() && raw > 0.0 { raw } else { 0.0 };
        ProcessUsage {
            pid: pid.as_u32(),
            name: p.name().to_string_lossy().into_owned(),
            cpu_percent: round1((raw / cores).clamp(0.0, 100.0)),
            cpu_cores: raw.round() / 100.0,
            memory_bytes: p.memory(),
        }
    }

    /// Takes a sample.
    pub fn sample(&mut self) -> ResourceSample {
        self.system.refresh_cpu_usage();
        self.system.refresh_memory();
        self.system.refresh_processes_specifics(
            ProcessesToUpdate::All,
            true,
            ProcessRefreshKind::nothing().with_cpu().with_memory(),
        );
        // Mount points rarely change; refresh the list every ~5 minutes.
        if self.samples.is_multiple_of(150) {
            self.disks.refresh(true);
        } else {
            self.disks.refresh(false);
        }
        self.samples += 1;
        let cores = self.system.cpus().len().max(1) as f32;
        let (disk_total_bytes, disk_free_bytes) = data_volume(&self.disks, &self.disk_anchor);
        let core_process = self.system.process(self.own_pid).map(|p| self.usage(self.own_pid, p, cores));
        let mut top: Vec<ProcessUsage> = self
            .system
            .processes()
            .iter()
            // pid 0 is the idle/kernel pseudo process on Windows/macOS.
            .filter(|(pid, _)| pid.as_u32() != 0)
            .map(|(pid, p)| self.usage(*pid, p, cores))
            .collect();
        top.sort_by(|a, b| b.cpu_percent.total_cmp(&a.cpu_percent).then(b.memory_bytes.cmp(&a.memory_bytes)));
        top.truncate(TOP_PROCESSES);
        ResourceSample {
            node_id: self.node_id,
            ts: orkestr_domain::now(),
            cpu_percent: round1(self.system.global_cpu_usage()),
            per_core_percent: self.system.cpus().iter().map(|c| round1(c.cpu_usage())).collect(),
            memory_total_bytes: self.system.total_memory(),
            memory_used_bytes: self.system.used_memory(),
            swap_total_bytes: self.system.total_swap(),
            swap_used_bytes: self.system.used_swap(),
            disk_total_bytes,
            disk_free_bytes,
            core_process,
            top_processes: top,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn samples_are_sane() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = ResourceSampler::new(NodeId::new(), dir.path().to_path_buf());
        let _ = s.sample();
        std::thread::sleep(std::time::Duration::from_millis(250));
        let r = s.sample();
        assert!(!r.per_core_percent.is_empty());
        assert!((0.0..=100.0).contains(&r.cpu_percent));
        assert!(r.per_core_percent.iter().all(|c| (0.0..=100.0).contains(c)));
        assert!(r.memory_total_bytes >= r.memory_used_bytes && r.memory_total_bytes > 0);
        assert!(r.disk_total_bytes >= r.disk_free_bytes && r.disk_total_bytes > 0);
        let me = r.core_process.expect("own process is sampled");
        assert_eq!(me.pid, std::process::id());
        assert!(me.memory_bytes > 0);
        assert!(r.top_processes.len() <= TOP_PROCESSES);
        assert!(r.top_processes.windows(2).all(|w| w[0].cpu_percent >= w[1].cpu_percent));
    }
}
