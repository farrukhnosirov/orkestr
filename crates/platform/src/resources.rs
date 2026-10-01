//! CPU / RAM / disk usage (§11 resource-aware scheduling, §27 monitor).

use std::path::PathBuf;

use parking_lot::Mutex;

/// A point-in-time resource snapshot.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceSnapshot {
    /// Logical cores.
    pub cpu_cores: u32,
    /// Global CPU usage 0..=100 (0 on the very first sample).
    pub cpu_usage_percent: f32,
    /// Total RAM bytes.
    pub memory_total_bytes: u64,
    /// Used RAM bytes.
    pub memory_used_bytes: u64,
    /// Total bytes of the disk holding the data dir.
    pub disk_total_bytes: u64,
    /// Free bytes of that disk.
    pub disk_free_bytes: u64,
}

/// Resource monitor.
pub trait ResourceMonitor: Send + Sync {
    /// Takes a snapshot (refreshing counters).
    fn snapshot(&self) -> ResourceSnapshot;
}

/// sysinfo-based implementation.
pub struct SysinfoResourceMonitor {
    system: Mutex<sysinfo::System>,
    disk_anchor: PathBuf,
}

impl std::fmt::Debug for SysinfoResourceMonitor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SysinfoResourceMonitor").field("disk_anchor", &self.disk_anchor).finish_non_exhaustive()
    }
}

impl SysinfoResourceMonitor {
    /// `disk_anchor` selects which volume is reported (usually the data dir).
    #[must_use]
    pub fn new(disk_anchor: PathBuf) -> Self {
        let mut system = sysinfo::System::new();
        system.refresh_cpu_usage();
        system.refresh_memory();
        Self { system: Mutex::new(system), disk_anchor }
    }
}

fn disk_for(anchor: &std::path::Path) -> (u64, u64) {
    let disks = sysinfo::Disks::new_with_refreshed_list();
    let anchor = anchor.canonicalize().unwrap_or_else(|_| anchor.to_path_buf());
    disks
        .list()
        .iter()
        .filter(|d| anchor.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .or_else(|| disks.list().first())
        .map_or((0, 0), |d| (d.total_space(), d.available_space()))
}

impl ResourceMonitor for SysinfoResourceMonitor {
    fn snapshot(&self) -> ResourceSnapshot {
        let mut s = self.system.lock();
        s.refresh_cpu_usage();
        s.refresh_memory();
        let (disk_total_bytes, disk_free_bytes) = disk_for(&self.disk_anchor);
        ResourceSnapshot {
            cpu_cores: u32::try_from(s.cpus().len()).unwrap_or(u32::MAX).max(1),
            cpu_usage_percent: s.global_cpu_usage(),
            memory_total_bytes: s.total_memory(),
            memory_used_bytes: s.used_memory(),
            disk_total_bytes,
            disk_free_bytes,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_is_sane() {
        let dir = tempfile::tempdir().unwrap();
        let m = SysinfoResourceMonitor::new(dir.path().to_path_buf());
        let s = m.snapshot();
        assert!(s.cpu_cores >= 1);
        assert!(s.memory_total_bytes >= s.memory_used_bytes);
        assert!(s.disk_total_bytes >= s.disk_free_bytes);
        assert!(s.disk_total_bytes > 0, "data dir disk not found");
    }
}
