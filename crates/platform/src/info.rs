//! Static host information.

use orkestr_domain::{Arch, OsKind};

/// Host description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlatformInfo {
    /// OS family.
    pub os: OsKind,
    /// Architecture.
    pub arch: Arch,
    /// Host name.
    pub hostname: String,
    /// Human-readable OS version ("Windows 11 Pro 26200").
    pub os_version: String,
}

impl PlatformInfo {
    /// Detects the current host.
    #[must_use]
    pub fn detect() -> Self {
        Self {
            os: current_os(),
            arch: current_arch(),
            hostname: sysinfo::System::host_name().unwrap_or_else(|| "localhost".into()),
            os_version: sysinfo::System::long_os_version()
                .or_else(sysinfo::System::os_version)
                .unwrap_or_else(|| std::env::consts::OS.into()),
        }
    }
}

/// OS family of this build.
#[must_use]
pub fn current_os() -> OsKind {
    #[cfg(target_os = "windows")]
    {
        OsKind::Windows
    }
    #[cfg(target_os = "macos")]
    {
        OsKind::MacOs
    }
    #[cfg(target_os = "linux")]
    {
        OsKind::Linux
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        OsKind::Other
    }
}

/// Architecture of this build.
#[must_use]
pub fn current_arch() -> Arch {
    match std::env::consts::ARCH {
        "x86_64" => Arch::X64,
        "aarch64" => Arch::Arm64,
        _ => Arch::Other,
    }
}
