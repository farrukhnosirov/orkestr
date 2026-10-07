//! Which VSCodium REH-web build runs on this host.
//!
//! OS / architecture detection lives in `orkestr-platform`
//! ([`orkestr_platform::PlatformInfo`]); this module only maps those values
//! to VSCodium's release asset names (no `cfg` code here).

use orkestr_domain::{Arch, OsKind};

/// A VSCodium REH-web release target (`win32-x64`, `darwin-arm64`, …).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServerTarget {
    /// VS Code platform name: `win32`, `darwin` or `linux`.
    pub platform: &'static str,
    /// VS Code architecture name: `x64` or `arm64`.
    pub arch: &'static str,
    /// The build runs under emulation (Windows on ARM runs the x64 build:
    /// VSCodium publishes no `win32-arm64` REH-web package).
    pub emulated: bool,
}

impl ServerTarget {
    /// `<platform>-<arch>`.
    #[must_use]
    pub fn id(&self) -> String {
        format!("{}-{}", self.platform, self.arch)
    }

    /// Release asset file name for `version`.
    #[must_use]
    pub fn asset_name(&self, version: &str) -> String {
        format!("vscodium-reh-web-{}-{}-{version}.tar.gz", self.platform, self.arch)
    }

    /// File name of the bundled Node.js runtime inside the package.
    #[must_use]
    pub fn node_executable(&self) -> &'static str {
        if self.platform == "win32" {
            "node.exe"
        } else {
            "node"
        }
    }
}

/// Why no server build exists for a host.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("VS Code server builds are not available for {os}/{arch}")]
pub struct UnsupportedTarget {
    /// OS name.
    pub os: String,
    /// Architecture name.
    pub arch: String,
}

/// Picks the release target for an OS / architecture.
pub fn select_target(os: OsKind, arch: Arch) -> Result<ServerTarget, UnsupportedTarget> {
    let t = |platform, arch, emulated| ServerTarget { platform, arch, emulated };
    match (os, arch) {
        (OsKind::Windows, Arch::X64) => Ok(t("win32", "x64", false)),
        // Windows 11 on ARM emulates x64 binaries; no native REH-web build exists.
        (OsKind::Windows, Arch::Arm64) => Ok(t("win32", "x64", true)),
        (OsKind::MacOs, Arch::X64) => Ok(t("darwin", "x64", false)),
        (OsKind::MacOs, Arch::Arm64) => Ok(t("darwin", "arm64", false)),
        (OsKind::Linux, Arch::X64) => Ok(t("linux", "x64", false)),
        (OsKind::Linux, Arch::Arm64) => Ok(t("linux", "arm64", false)),
        (os, arch) => Err(UnsupportedTarget { os: format!("{os:?}"), arch: format!("{arch:?}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_every_supported_host_to_a_published_asset() {
        let cases = [
            (OsKind::Windows, Arch::X64, "win32-x64", false),
            (OsKind::Windows, Arch::Arm64, "win32-x64", true),
            (OsKind::MacOs, Arch::X64, "darwin-x64", false),
            (OsKind::MacOs, Arch::Arm64, "darwin-arm64", false),
            (OsKind::Linux, Arch::X64, "linux-x64", false),
            (OsKind::Linux, Arch::Arm64, "linux-arm64", false),
        ];
        for (os, arch, id, emulated) in cases {
            let t = select_target(os, arch).unwrap();
            assert_eq!(t.id(), id);
            assert_eq!(t.emulated, emulated);
        }
    }

    #[test]
    fn unknown_hosts_are_rejected_explicitly() {
        assert!(select_target(OsKind::Other, Arch::X64).is_err());
        assert!(select_target(OsKind::Linux, Arch::Other).is_err());
        let e = select_target(OsKind::Windows, Arch::Other).unwrap_err();
        assert!(e.to_string().contains("not available"));
    }

    #[test]
    fn asset_names_match_vscodium_release_naming() {
        let win = select_target(OsKind::Windows, Arch::X64).unwrap();
        assert_eq!(win.asset_name("1.135.06055"), "vscodium-reh-web-win32-x64-1.135.06055.tar.gz");
        assert_eq!(win.node_executable(), "node.exe");
        let mac = select_target(OsKind::MacOs, Arch::Arm64).unwrap();
        assert_eq!(mac.asset_name("1.2.3"), "vscodium-reh-web-darwin-arm64-1.2.3.tar.gz");
        assert_eq!(mac.node_executable(), "node");
    }
}
