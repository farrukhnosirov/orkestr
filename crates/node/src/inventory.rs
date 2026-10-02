//! Hardware + runtime inventory of the local node (`nodes.info`).

use std::path::Path;
use std::time::{Duration, Instant};

use orkestr_domain::NodeId;
use orkestr_platform::PlatformAdapter;
use orkestr_protocol::methods::nodes::{CpuInfo, DiskInfo, GpuDevice, NodeInfo};
use sysinfo::{CpuRefreshKind, DiskKind, Disks, RefreshKind, System};

use crate::{ollama, runtimes};

/// Timeouts used by [`detect_node_info`].
#[derive(Debug, Clone)]
pub struct DetectOptions {
    /// Per runtime version probe.
    pub runtime_timeout: Duration,
    /// Per GPU probe (`nvidia-smi`, PowerShell WMI, …).
    pub gpu_timeout: Duration,
    /// Ollama HTTP probe.
    pub ollama_timeout: Duration,
    /// Ollama endpoint (`host:port`).
    pub ollama_endpoint: String,
}

impl Default for DetectOptions {
    fn default() -> Self {
        Self {
            runtime_timeout: Duration::from_secs(8),
            gpu_timeout: Duration::from_secs(10),
            ollama_timeout: Duration::from_millis(1500),
            ollama_endpoint: ollama::endpoint_from_env(std::env::var("OLLAMA_HOST").ok().as_deref()),
        }
    }
}

fn cpu_info() -> CpuInfo {
    let sys = System::new_with_specifics(RefreshKind::nothing().with_cpu(CpuRefreshKind::everything()));
    let first = sys.cpus().first();
    CpuInfo {
        model: first
            .map(|c| c.brand().trim().to_owned())
            .filter(|b| !b.is_empty())
            .unwrap_or_else(|| "Unknown CPU".into()),
        vendor: first.map(|c| c.vendor_id().trim().to_owned()).unwrap_or_default(),
        logical_cores: u32::try_from(sys.cpus().len()).unwrap_or(u32::MAX).max(1),
        physical_cores: System::physical_core_count().and_then(|n| u32::try_from(n).ok()),
        frequency_mhz: first.map_or(0, sysinfo::Cpu::frequency),
    }
}

/// Lists mounted volumes, flagging the one holding `data_dir`.
#[must_use]
pub fn disks(data_dir: &Path) -> Vec<DiskInfo> {
    let disks = Disks::new_with_refreshed_list();
    let anchor = crate::sampler::comparable_path(data_dir);
    let data_mount = disks
        .list()
        .iter()
        .filter(|d| anchor.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len())
        .map(|d| d.mount_point().to_path_buf());
    let mut out: Vec<DiskInfo> = Vec::new();
    for d in disks.list() {
        let mount = d.mount_point().display().to_string();
        if out.iter().any(|x| x.mount_point == mount) || d.total_space() == 0 {
            continue;
        }
        out.push(DiskInfo {
            name: d.name().to_string_lossy().into_owned(),
            mount_point: mount,
            file_system: d.file_system().to_string_lossy().into_owned(),
            kind: match d.kind() {
                DiskKind::SSD => "ssd",
                DiskKind::HDD => "hdd",
                DiskKind::Unknown(_) => "unknown",
            }
            .to_owned(),
            removable: d.is_removable(),
            total_bytes: d.total_space(),
            available_bytes: d.available_space(),
            is_data_volume: data_mount.as_deref() == Some(d.mount_point()),
        });
    }
    out
}

fn gpus(timeout: Duration) -> Vec<GpuDevice> {
    orkestr_platform::detect_gpus(timeout)
        .into_iter()
        .map(|g| GpuDevice {
            name: g.name,
            vendor: g.vendor,
            vram_bytes: g.vram_bytes,
            driver_version: g.driver_version,
            source: g.source.to_owned(),
        })
        .collect()
}

/// Detects the full inventory. Blocking (runs external probes in parallel,
/// bounded by the timeouts in `opts`); call from a blocking thread.
#[must_use]
pub fn detect_node_info(
    node_id: NodeId,
    platform: &dyn PlatformAdapter,
    core_version: &str,
    opts: &DetectOptions,
) -> NodeInfo {
    let started = Instant::now();
    let info = platform.info();
    let data_dir = platform.paths().data_dir.clone();
    let (cpu, gpus, runtimes, local_models, disks) = std::thread::scope(|s| {
        let gpu = s.spawn(|| gpus(opts.gpu_timeout));
        let rt = s.spawn(|| runtimes::detect_runtimes(platform.process(), opts.runtime_timeout));
        let models = s.spawn(|| ollama::discover(&opts.ollama_endpoint, opts.ollama_timeout));
        let cpu = cpu_info();
        let disks = disks(&data_dir);
        (
            cpu,
            gpu.join().unwrap_or_default(),
            rt.join().unwrap_or_default(),
            models.join().unwrap_or_else(|_| orkestr_protocol::methods::nodes::LocalModelsInfo {
                ollama_endpoint: opts.ollama_endpoint.clone(),
                ollama_reachable: false,
                models: Vec::new(),
            }),
            disks,
        )
    });
    let mut sys = System::new();
    sys.refresh_memory();
    let detection_ms = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    tracing::info!(
        detection_ms,
        gpus = gpus.len(),
        runtimes = runtimes.iter().filter(|r| r.available).count(),
        "node inventory detected"
    );
    NodeInfo {
        node_id,
        hostname: info.hostname.clone(),
        os: info.os,
        os_version: info.os_version.clone(),
        kernel_version: System::kernel_version(),
        arch: info.arch,
        core_version: core_version.to_owned(),
        cpu,
        memory_total_bytes: sys.total_memory(),
        swap_total_bytes: sys.total_swap(),
        gpus,
        disks,
        runtimes,
        local_models,
        detected_at: orkestr_domain::now(),
        detection_ms,
    }
}
