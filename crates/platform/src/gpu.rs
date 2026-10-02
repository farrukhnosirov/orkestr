//! GPU discovery (§27 node resources, §38 node capabilities).
//!
//! Sources, all through [`run_with_timeout`]:
//!
//! * every OS: `nvidia-smi --query-gpu=name,memory.total,driver_version` when
//!   the NVIDIA driver tools are on `PATH` (exact VRAM and driver version);
//! * Windows: WMI `Win32_VideoController` through PowerShell `Get-CimInstance`
//!   (all adapters; `AdapterRAM` is a 32-bit field, so ≥4 GiB is unknown);
//! * macOS: `system_profiler SPDisplaysDataType -json`;
//! * Linux: `lspci` (VGA / 3D / display controllers).
//!
//! Results are merged: NVIDIA entries from `nvidia-smi` win over the generic
//! source's entry for the same adapter. Parsers are pure and unit-tested.

use std::time::Duration;

use crate::process::{run_with_timeout, which};

/// A detected graphics adapter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GpuDevice {
    /// Marketing name.
    pub name: String,
    /// Vendor ("NVIDIA", "AMD", "Intel", "Apple", …) when recognisable.
    pub vendor: Option<String>,
    /// Dedicated memory in bytes, when known.
    pub vram_bytes: Option<u64>,
    /// Driver version, when known.
    pub driver_version: Option<String>,
    /// Detection source (`nvidia-smi`, `wmi`, `system_profiler`, `lspci`).
    pub source: &'static str,
}

/// Detects GPUs; every probe is bounded by `timeout`. Never fails: missing
/// tools simply contribute nothing.
#[must_use]
pub fn detect_gpus(timeout: Duration) -> Vec<GpuDevice> {
    let nvidia = which("nvidia-smi")
        .and_then(|p| {
            run_with_timeout(
                &p,
                &["--query-gpu=name,memory.total,driver_version", "--format=csv,noheader,nounits"],
                timeout,
            )
            .map_err(|e| tracing::debug!(error = %e, "nvidia-smi probe failed"))
            .ok()
        })
        .filter(|o| o.success)
        .map(|o| parse_nvidia_smi(&o.stdout))
        .unwrap_or_default();
    merge(nvidia, native_gpus(timeout))
}

#[cfg(target_os = "windows")]
fn native_gpus(timeout: Duration) -> Vec<GpuDevice> {
    let Some(ps) = which("powershell.exe").or_else(|| which("pwsh.exe")) else { return Vec::new() };
    let script = "Get-CimInstance Win32_VideoController | Select-Object Name,AdapterRAM,DriverVersion,AdapterCompatibility | ConvertTo-Json -Compress";
    match run_with_timeout(&ps, &["-NoLogo", "-NoProfile", "-NonInteractive", "-Command", script], timeout) {
        Ok(o) if o.success => parse_wmi_json(&o.stdout),
        Ok(o) => {
            tracing::debug!(stderr = %o.stderr.trim(), "WMI GPU query failed");
            Vec::new()
        }
        Err(e) => {
            tracing::debug!(error = %e, "WMI GPU query failed");
            Vec::new()
        }
    }
}

#[cfg(target_os = "macos")]
fn native_gpus(timeout: Duration) -> Vec<GpuDevice> {
    which("system_profiler")
        .and_then(|p| run_with_timeout(&p, &["SPDisplaysDataType", "-json"], timeout).ok())
        .filter(|o| o.success)
        .map(|o| parse_system_profiler(&o.stdout))
        .unwrap_or_default()
}

#[cfg(target_os = "linux")]
fn native_gpus(timeout: Duration) -> Vec<GpuDevice> {
    which("lspci")
        .and_then(|p| run_with_timeout(&p, &[], timeout).ok())
        .filter(|o| o.success)
        .map(|o| parse_lspci(&o.stdout))
        .unwrap_or_default()
}

#[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
fn native_gpus(_timeout: Duration) -> Vec<GpuDevice> {
    Vec::new()
}

/// Infers the vendor from an adapter name.
#[must_use]
pub fn vendor_of(name: &str) -> Option<String> {
    let n = name.to_ascii_lowercase();
    let v = if n.contains("nvidia") || n.contains("geforce") || n.contains("quadro") || n.contains("tesla") {
        "NVIDIA"
    } else if n.contains("amd") || n.contains("radeon") || n.contains("advanced micro devices") {
        "AMD"
    } else if n.contains("intel") {
        "Intel"
    } else if n.contains("apple") {
        "Apple"
    } else if n.contains("qualcomm") || n.contains("adreno") {
        "Qualcomm"
    } else if n.contains("microsoft") {
        "Microsoft"
    } else {
        return None;
    };
    Some(v.to_owned())
}

fn is_virtual_adapter(name: &str) -> bool {
    let n = name.to_ascii_lowercase();
    n.contains("remote display") || n.contains("basic render") || n.contains("indirect display")
}

/// Parses `nvidia-smi --query-gpu=name,memory.total,driver_version --format=csv,noheader,nounits`.
#[must_use]
pub fn parse_nvidia_smi(text: &str) -> Vec<GpuDevice> {
    text.lines()
        .filter_map(|line| {
            let mut parts = line.split(',').map(str::trim);
            let name = parts.next().filter(|n| !n.is_empty())?.to_owned();
            let vram_mib = parts.next().and_then(|m| m.parse::<u64>().ok());
            let driver = parts.next().filter(|d| !d.is_empty()).map(str::to_owned);
            Some(GpuDevice {
                vendor: Some("NVIDIA".into()),
                name,
                vram_bytes: vram_mib.map(|m| m * 1024 * 1024),
                driver_version: driver,
                source: "nvidia-smi",
            })
        })
        .collect()
}

/// Parses PowerShell `ConvertTo-Json -Compress` output of `Win32_VideoController`
/// (a single object or an array).
#[must_use]
pub fn parse_wmi_json(text: &str) -> Vec<GpuDevice> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text.trim()) else { return Vec::new() };
    let items = match value {
        serde_json::Value::Array(a) => a,
        obj @ serde_json::Value::Object(_) => vec![obj],
        _ => return Vec::new(),
    };
    items
        .iter()
        .filter_map(|it| {
            let name = it.get("Name")?.as_str()?.trim().to_owned();
            if name.is_empty() || is_virtual_adapter(&name) {
                return None;
            }
            // AdapterRAM is a uint32: values at the cap mean "4 GiB or more".
            let ram = it.get("AdapterRAM").and_then(serde_json::Value::as_u64).filter(|r| *r > 0 && *r < 0xFFF0_0000);
            let vendor = vendor_of(&name)
                .or_else(|| it.get("AdapterCompatibility").and_then(|v| v.as_str()).and_then(vendor_of));
            Some(GpuDevice {
                vendor,
                vram_bytes: ram,
                driver_version: it.get("DriverVersion").and_then(|v| v.as_str()).map(str::to_owned),
                name,
                source: "wmi",
            })
        })
        .collect()
}

/// Parses sizes like `"16 GB"`, `"1536 MB"`.
fn parse_size(s: &str) -> Option<u64> {
    let mut it = s.split_whitespace();
    let n: f64 = it.next()?.parse().ok()?;
    let mult: f64 = match it.next()?.to_ascii_uppercase().as_str() {
        "KB" => 1024.0,
        "MB" => 1024.0 * 1024.0,
        "GB" => 1024.0 * 1024.0 * 1024.0,
        "TB" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return None,
    };
    let bytes = n * mult;
    (bytes.is_finite() && bytes >= 0.0).then_some(bytes as u64)
}

/// Parses `system_profiler SPDisplaysDataType -json`.
#[must_use]
pub fn parse_system_profiler(text: &str) -> Vec<GpuDevice> {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(text) else { return Vec::new() };
    let Some(items) = value.get("SPDisplaysDataType").and_then(|v| v.as_array()) else { return Vec::new() };
    items
        .iter()
        .filter_map(|it| {
            let name = it.get("sppci_model").or_else(|| it.get("_name")).and_then(|v| v.as_str())?.to_owned();
            let vram = ["spdisplays_vram", "spdisplays_vram_shared"]
                .iter()
                .find_map(|k| it.get(*k).and_then(|v| v.as_str()).and_then(parse_size));
            Some(GpuDevice {
                vendor: vendor_of(&name),
                name,
                vram_bytes: vram,
                driver_version: None,
                source: "system_profiler",
            })
        })
        .collect()
}

/// Parses plain `lspci` output.
#[must_use]
pub fn parse_lspci(text: &str) -> Vec<GpuDevice> {
    text.lines()
        .filter_map(|line| {
            let (_, rest) = line.split_once(' ')?;
            let (class, desc) = rest.split_once(": ")?;
            let class = class.to_ascii_lowercase();
            if !(class.contains("vga") || class.contains("3d controller") || class.contains("display controller")) {
                return None;
            }
            let name = desc.split(" (rev ").next().unwrap_or(desc).trim().to_owned();
            Some(GpuDevice { vendor: vendor_of(&name), name, vram_bytes: None, driver_version: None, source: "lspci" })
        })
        .collect()
}

fn same_adapter(a: &str, b: &str) -> bool {
    let (a, b) = (a.to_ascii_lowercase(), b.to_ascii_lowercase());
    a == b || a.contains(&b) || b.contains(&a)
}

/// `preferred` first, then every `other` entry not describing the same adapter.
#[must_use]
pub fn merge(preferred: Vec<GpuDevice>, other: Vec<GpuDevice>) -> Vec<GpuDevice> {
    let mut out = preferred;
    for g in other {
        let dup = out.iter().any(|p| {
            same_adapter(&p.name, &g.name)
                || (p.source == "nvidia-smi" && g.vendor.as_deref() == Some("NVIDIA") && g.source == "lspci")
        });
        if !dup {
            out.push(g);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nvidia_smi_csv() {
        let g = parse_nvidia_smi("NVIDIA GeForce RTX 4090, 24564, 551.86\nNVIDIA RTX A2000, 6138, 551.86\n\n");
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "NVIDIA GeForce RTX 4090");
        assert_eq!(g[0].vram_bytes, Some(24564 * 1024 * 1024));
        assert_eq!(g[0].driver_version.as_deref(), Some("551.86"));
        assert_eq!(g[1].vendor.as_deref(), Some("NVIDIA"));
    }

    #[test]
    fn wmi_object_and_array() {
        let one = r#"{"Name":"Intel(R) UHD Graphics 770","AdapterRAM":1073741824,"DriverVersion":"31.0.101.4502","AdapterCompatibility":"Intel Corporation"}"#;
        let g = parse_wmi_json(one);
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].vendor.as_deref(), Some("Intel"));
        assert_eq!(g[0].vram_bytes, Some(1_073_741_824));
        let many = r#"[{"Name":"NVIDIA GeForce RTX 3080","AdapterRAM":4293918720,"DriverVersion":"31.0.15.5186"},
                      {"Name":"Microsoft Remote Display Adapter","AdapterRAM":0,"DriverVersion":"10.0"},
                      {"Name":"AMD Radeon(TM) Graphics","AdapterRAM":536870912,"DriverVersion":"31.0.12027.9001"}]"#;
        let g = parse_wmi_json(many);
        assert_eq!(g.len(), 2, "virtual adapters are skipped: {g:?}");
        assert_eq!(g[0].vram_bytes, None, "capped AdapterRAM is unknown");
        assert_eq!(g[1].vendor.as_deref(), Some("AMD"));
        assert!(parse_wmi_json("not json").is_empty());
        assert!(parse_wmi_json("").is_empty());
    }

    #[test]
    fn system_profiler_json() {
        let j = r#"{"SPDisplaysDataType":[{"_name":"kHW_AppleM1ProItem","sppci_model":"Apple M1 Pro","sppci_cores":"16"},
                    {"_name":"Radeon Pro 5500M","sppci_model":"AMD Radeon Pro 5500M","spdisplays_vram":"8 GB"}]}"#;
        let g = parse_system_profiler(j);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Apple M1 Pro");
        assert_eq!(g[0].vendor.as_deref(), Some("Apple"));
        assert_eq!(g[1].vram_bytes, Some(8 * 1024 * 1024 * 1024));
    }

    #[test]
    fn lspci_lines() {
        let t = "00:02.0 VGA compatible controller: Intel Corporation Alder Lake-P GT2 [Iris Xe Graphics] (rev 0c)\n\
                 01:00.0 3D controller: NVIDIA Corporation GA107M [GeForce RTX 3050 Mobile] (rev a1)\n\
                 00:1f.3 Audio device: Intel Corporation Alder Lake PCH-P High Definition Audio Controller (rev 01)\n";
        let g = parse_lspci(t);
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].name, "Intel Corporation Alder Lake-P GT2 [Iris Xe Graphics]");
        assert_eq!(g[1].vendor.as_deref(), Some("NVIDIA"));
    }

    #[test]
    fn merge_prefers_nvidia_smi() {
        let smi = parse_nvidia_smi("NVIDIA GeForce RTX 3080, 10240, 551.86");
        let wmi = parse_wmi_json(
            r#"[{"Name":"NVIDIA GeForce RTX 3080","AdapterRAM":4293918720},{"Name":"Intel(R) UHD Graphics","AdapterRAM":1073741824}]"#,
        );
        let m = merge(smi, wmi);
        assert_eq!(m.len(), 2);
        assert_eq!(m[0].source, "nvidia-smi");
        assert_eq!(m[1].vendor.as_deref(), Some("Intel"));
    }

    #[test]
    fn live_detection_does_not_panic() {
        let gpus = detect_gpus(Duration::from_secs(20));
        for g in gpus {
            assert!(!g.name.is_empty());
        }
    }
}
