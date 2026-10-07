//! `vscode.*` — the embedded VS Code panel (VSCodium REH web server).
//!
//! Orkestr can download the open-source VSCodium "remote extension host —
//! web" server (MIT, extensions from Open VSX — never the Microsoft
//! Marketplace) on demand, verify its published sha256, and run it on
//! `127.0.0.1` with a random connection token. The desktop shows its UI in a
//! dedicated window; extensions run inside that server **as the user**
//! (untrusted code) — the install call therefore requires explicit consent.
//!
//! * `vscode.status` — install / run state (never contains the token).
//! * `vscode.checkUpdate` — the latest release for this OS/arch (size, URLs).
//! * `vscode.install {accept: true}` — download + sha256 verify + extract
//!   (background; progress via `VsCodeProgress` events).
//! * `vscode.cancelInstall`, `vscode.uninstall`.
//! * `vscode.start {projectId}` — start (or reuse) the server and return the
//!   UI URL **with** the connection token for that project's folder.
//! * `vscode.stop`, `vscode.keepAlive` (idle auto-stop), `vscode.setConfig`,
//!   `vscode.logs` (redacted server output).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ProjectId, Timestamp};

use super::{method, Empty};

/// Lifecycle state of the VS Code server.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VsCodeState {
    /// The server is not downloaded yet.
    NotInstalled,
    /// Downloading / verifying / extracting the server package.
    Installing,
    /// Installed and not running.
    Stopped,
    /// The server process is starting (waiting for its port).
    Starting,
    /// Running and healthy.
    Running,
    /// Crashed; waiting for the restart back-off.
    Restarting,
    /// Stopping.
    Stopping,
    /// Failed (install or start); see `error` and `vscode.logs`.
    Error,
}

/// Phase of an install.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum VsCodeInstallPhase {
    /// Resolving the release.
    Resolving,
    /// Downloading the archive.
    Downloading,
    /// Checking the sha256.
    Verifying,
    /// Unpacking.
    Extracting,
}

/// Install progress.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeProgress {
    /// Phase.
    pub phase: VsCodeInstallPhase,
    /// Bytes downloaded (or files extracted while extracting).
    pub done: u64,
    /// Total bytes (or files), when known.
    pub total: Option<u64>,
    /// Version being installed.
    pub version: Option<String>,
}

/// An installed server package.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeInstallation {
    /// VSCodium version, e.g. `1.135.06055`.
    pub version: String,
    /// Asset file name it was installed from.
    pub asset_name: String,
    /// Verified sha256 (hex) of the archive.
    pub sha256: String,
    /// Archive size in bytes.
    pub size_bytes: u64,
    /// Install time.
    pub installed_at: Timestamp,
    /// Install directory.
    pub dir: String,
}

/// A running server (no token: see `vscode.start`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeRunning {
    /// Loopback port.
    pub port: u16,
    /// Server process id.
    pub pid: Option<u32>,
    /// Start time of the current process.
    pub started_at: Timestamp,
    /// Automatic restarts after crashes since the user started it.
    pub restarts: u32,
}

/// User configuration of the VS Code server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VsCodeConfig {
    /// Stop the server when no VS Code panel/window has been open for
    /// `idleStopSecs` (default on: extensions run only while VS Code is used).
    pub auto_stop: bool,
    /// Grace period of the auto-stop, seconds (30..=3600).
    pub idle_stop_secs: u32,
    /// Restart the server automatically after a crash (with back-off).
    pub auto_restart: bool,
}

impl Default for VsCodeConfig {
    fn default() -> Self {
        Self { auto_stop: true, idle_stop_secs: 120, auto_restart: true }
    }
}

/// Partial update of [`VsCodeConfig`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VsCodeConfigPatch {
    /// See [`VsCodeConfig::auto_stop`].
    #[ts(optional)]
    pub auto_stop: Option<bool>,
    /// See [`VsCodeConfig::idle_stop_secs`].
    #[ts(optional)]
    pub idle_stop_secs: Option<u32>,
    /// See [`VsCodeConfig::auto_restart`].
    #[ts(optional)]
    pub auto_restart: Option<bool>,
}

/// Status of the VS Code integration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeStatus {
    /// Lifecycle state.
    pub state: VsCodeState,
    /// Release target of this host (`win32-x64`, `darwin-arm64`, …); `null`
    /// when no server build exists for this OS/architecture.
    pub target: Option<String>,
    /// Why this host is unsupported (when `target` is null).
    pub unsupported_reason: Option<String>,
    /// Installed package.
    pub installed: Option<VsCodeInstallation>,
    /// Running process.
    pub running: Option<VsCodeRunning>,
    /// Install progress (while `installing`).
    pub progress: Option<VsCodeProgress>,
    /// Last error (redacted).
    pub error: Option<String>,
    /// Root of the server data (`<data_dir>/vscode-server`).
    pub data_dir: String,
    /// Configuration.
    pub config: VsCodeConfig,
}

/// A downloadable server release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeRelease {
    /// Version (release tag).
    pub version: String,
    /// Asset file name.
    pub asset_name: String,
    /// Download size in bytes.
    pub size_bytes: u64,
    /// Archive URL (GitHub release asset).
    pub download_url: String,
    /// URL of the published `.sha256` file.
    pub sha256_url: String,
    /// Publish time, when known.
    pub published_at: Option<String>,
    /// Release page.
    pub release_page: String,
    /// License of the package (`MIT`).
    pub license: String,
    /// Extension registry the server uses (`https://open-vsx.org`).
    pub extension_registry: String,
    /// Whether this is newer than the installed version.
    pub update_available: bool,
}

/// Params of `vscode.install`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeInstallParams {
    /// Must be `true`: the user accepted that extensions run as their OS user.
    pub accept: bool,
    /// Expected version (from `vscode.checkUpdate`); latest when omitted.
    #[serde(default)]
    #[ts(optional)]
    pub version: Option<String>,
}

/// Params of `vscode.start`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeStartParams {
    /// Project whose root folder is opened; none = empty window.
    #[serde(default)]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
}

/// Result of `vscode.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeStartResult {
    /// UI URL including the connection token (`http://127.0.0.1:<port>/?tkn=…&folder=…`).
    /// Secret: never log or persist it.
    pub url: String,
    /// Origin (`http://127.0.0.1:<port>`), for window / navigation checks.
    pub origin: String,
    /// Opened folder.
    pub folder: Option<String>,
    /// Status after the start.
    pub status: VsCodeStatus,
}

/// Result of `vscode.logs`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeLogsResult {
    /// Recent server output lines (redacted, oldest first).
    pub lines: Vec<String>,
}

method!(
    /// `vscode.status`.
    Status, "vscode.status", Empty, VsCodeStatus
);
method!(
    /// `vscode.checkUpdate` — latest release for this host (network).
    CheckUpdate, "vscode.checkUpdate", Empty, VsCodeRelease
);
method!(
    /// `vscode.install` — download, verify and unpack (runs in the background).
    Install, "vscode.install", VsCodeInstallParams, VsCodeStatus
);
method!(
    /// `vscode.cancelInstall`.
    CancelInstall, "vscode.cancelInstall", Empty, VsCodeStatus
);
method!(
    /// `vscode.uninstall` — stops the server and removes the package (keeps settings and extensions unless `purge`).
    Uninstall, "vscode.uninstall", VsCodeUninstallParams, VsCodeStatus
);
method!(
    /// `vscode.start`.
    Start, "vscode.start", VsCodeStartParams, VsCodeStartResult
);
method!(
    /// `vscode.stop`.
    Stop, "vscode.stop", Empty, VsCodeStatus
);
method!(
    /// `vscode.keepAlive` — a VS Code view is open (resets the idle auto-stop timer).
    KeepAlive, "vscode.keepAlive", Empty, Empty
);
method!(
    /// `vscode.setConfig`.
    SetConfig, "vscode.setConfig", VsCodeConfigPatch, VsCodeStatus
);
method!(
    /// `vscode.logs`.
    Logs, "vscode.logs", Empty, VsCodeLogsResult
);

/// Params of `vscode.uninstall`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeUninstallParams {
    /// Also delete VS Code user settings and installed extensions.
    #[serde(default)]
    #[ts(optional)]
    pub purge: Option<bool>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_defaults_are_safe() {
        let c: VsCodeConfig = serde_json::from_str("{}").unwrap();
        assert!(c.auto_stop, "extensions run only while VS Code is used by default");
        assert!(c.auto_restart);
        assert_eq!(c.idle_stop_secs, 120);
    }

    #[test]
    fn install_requires_explicit_field() {
        assert!(serde_json::from_str::<VsCodeInstallParams>("{}").is_err(), "accept must be sent explicitly");
        let p: VsCodeInstallParams = serde_json::from_str(r#"{"accept":true}"#).unwrap();
        assert!(p.accept);
    }
}
