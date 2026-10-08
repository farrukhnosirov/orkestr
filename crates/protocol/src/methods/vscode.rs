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
//! * `vscode.listExtensions` / `vscode.installExtension {id}` — extensions
//!   in Orkestr's own VS Code extensions directory (Open VSX, via the
//!   installed server's CLI).
//! * `vscode.commands` / `vscode.execute` / `vscode.openView` /
//!   `vscode.openFile` / `vscode.tasks` / `vscode.runTask` — run extension
//!   commands through the Orkestr bridge extension (auto-installed, loopback,
//!   one-time token; every execution audited, dangerous ones confirmed).

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
    /// Extension commands pinned to the left rail / Mission Control quick actions.
    pub pinned_commands: Vec<String>,
}

impl Default for VsCodeConfig {
    fn default() -> Self {
        Self { auto_stop: true, idle_stop_secs: 120, auto_restart: true, pinned_commands: Vec::new() }
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
    /// See [`VsCodeConfig::pinned_commands`] (replaces the list; ≤ 24 ids).
    #[ts(optional)]
    pub pinned_commands: Option<Vec<String>>,
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
    /// A VS Code window is connected through the Orkestr bridge extension
    /// (extension commands can run).
    pub bridge_connected: bool,
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

/// An extension installed in Orkestr's VS Code extensions directory.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeExtension {
    /// Extension id `publisher.name` (lowercase).
    pub id: String,
    /// Installed version.
    pub version: Option<String>,
}

/// Result of `vscode.listExtensions` / `vscode.installExtension`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeExtensionsResult {
    /// Installed extensions, sorted by id.
    pub extensions: Vec<VsCodeExtension>,
}

/// Params of `vscode.installExtension`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeInstallExtensionParams {
    /// Open VSX id `publisher.name`, optionally `@version`
    /// (e.g. `anthropic.claude-code`).
    pub id: String,
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

/// A command of a VS Code extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeCommandInfo {
    /// Command id (`rainbow-csv.Align`).
    pub id: String,
    /// Title (localized from the extension's `package.nls*.json` when available).
    pub title: String,
    /// Category shown before the title in VS Code's palette.
    pub category: Option<String>,
    /// Hidden from VS Code's command palette (`when: false`) or registered
    /// only at runtime — the extension's "hidden" functions.
    pub hidden: bool,
    /// Not declared in the manifest; found through the bridge at runtime.
    pub runtime_only: bool,
    /// Looks destructive: `vscode.execute` requires `confirmed: true`.
    pub dangerous: bool,
    /// Default keybinding declared by the extension.
    pub keybinding: Option<String>,
}

/// A view container (activity bar / panel) of an extension.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeViewContainer {
    /// Container id.
    pub id: String,
    /// Title.
    pub title: String,
    /// `activitybar`, `panel`, …
    pub location: String,
    /// Command that focuses it (`workbench.view.extension.<id>`).
    pub command: String,
}

/// An installed extension with its commands (from its manifest on disk).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeExtensionCommands {
    /// `publisher.name` (lowercase).
    pub id: String,
    /// Display name.
    pub display_name: String,
    /// Description.
    pub description: Option<String>,
    /// Installed version.
    pub version: Option<String>,
    /// Publisher.
    pub publisher: String,
    /// Icon as a `data:` URL (≤ 256 KiB raster images only).
    pub icon: Option<String>,
    /// Marketplace categories.
    pub categories: Vec<String>,
    /// Commands (declared ones first, then runtime-only ones).
    pub commands: Vec<VsCodeCommandInfo>,
    /// View containers (open with `vscode.openView`).
    pub view_containers: Vec<VsCodeViewContainer>,
    /// The extension contributes settings.
    pub has_settings: bool,
    /// Install directory.
    pub dir: String,
}

/// Params of `vscode.commands`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VsCodeCommandsParams {
    /// Only this extension.
    #[ts(optional)]
    pub extension_id: Option<String>,
    /// UI locale for titles (`ru`, `en`).
    #[ts(optional)]
    pub locale: Option<String>,
}

/// Result of `vscode.commands`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeCommandsResult {
    /// Installed extensions with their commands (the Orkestr bridge is left out).
    pub extensions: Vec<VsCodeExtensionCommands>,
    /// A VS Code window is connected (commands can run right away).
    pub bridge_connected: bool,
    /// Whether the connected window trusts its folder (`null` = no window).
    /// In Restricted Mode VS Code disables most extensions, so their commands
    /// are not found until the folder is trusted (`workbench.trust.manage`).
    pub workspace_trusted: Option<bool>,
}

/// Params of `vscode.execute`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeExecuteParams {
    /// Command id.
    pub command_id: String,
    /// Arguments (JSON; `{"$uri": "/abs/path"}` becomes a `vscode.Uri`).
    #[serde(default)]
    #[ts(optional, type = "Array<unknown>")]
    pub args: Option<Vec<serde_json::Value>>,
    /// The user confirmed a dangerous command.
    #[serde(default)]
    #[ts(optional)]
    pub confirmed: Option<bool>,
    /// Wait this long (ms, ≤ 120000) for a VS Code window to connect.
    #[serde(default)]
    #[ts(optional)]
    pub wait_ms: Option<u32>,
}

/// Result of `vscode.execute` / `vscode.openView` / `vscode.openFile` / `vscode.runTask`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeExecuteResult {
    /// What the command returned (JSON, truncated at 64 KiB).
    #[ts(type = "unknown")]
    pub result: serde_json::Value,
    /// Duration in ms.
    pub duration_ms: u64,
}

/// Params of `vscode.openView`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeOpenViewParams {
    /// Extension whose view container to focus.
    pub extension_id: String,
    /// Container id; the extension's first one when omitted.
    #[serde(default)]
    #[ts(optional)]
    pub container_id: Option<String>,
    /// Open the extension's settings instead of its view.
    #[serde(default)]
    #[ts(optional)]
    pub settings: Option<bool>,
    /// See [`VsCodeExecuteParams::wait_ms`].
    #[serde(default)]
    #[ts(optional)]
    pub wait_ms: Option<u32>,
}

/// Params of `vscode.openFile`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeOpenFileParams {
    /// Absolute path.
    pub path: String,
    /// 1-based line.
    #[serde(default)]
    #[ts(optional)]
    pub line: Option<u32>,
    /// 1-based column.
    #[serde(default)]
    #[ts(optional)]
    pub column: Option<u32>,
    /// See [`VsCodeExecuteParams::wait_ms`].
    #[serde(default)]
    #[ts(optional)]
    pub wait_ms: Option<u32>,
}

/// A VS Code task.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeTaskInfo {
    /// Task name.
    pub name: String,
    /// Source (`npm`, `Workspace`, …).
    pub source: String,
    /// Detail line.
    #[serde(default)]
    pub detail: Option<String>,
}

/// Result of `vscode.tasks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeTasksResult {
    /// Tasks of the open workspace.
    pub tasks: Vec<VsCodeTaskInfo>,
}

/// Params of `vscode.runTask`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeRunTaskParams {
    /// Task name.
    pub name: String,
    /// Task source, to disambiguate.
    #[serde(default)]
    #[ts(optional)]
    pub source: Option<String>,
    /// Running a task executes a shell command: the user must confirm.
    #[serde(default)]
    #[ts(optional)]
    pub confirmed: Option<bool>,
    /// See [`VsCodeExecuteParams::wait_ms`].
    #[serde(default)]
    #[ts(optional)]
    pub wait_ms: Option<u32>,
}

/// Params of `vscode.tasks`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct VsCodeTasksParams {
    /// See [`VsCodeExecuteParams::wait_ms`].
    #[ts(optional)]
    pub wait_ms: Option<u32>,
}

/// `VsCodeCommandExecuted` payload: an extension command ran through the bridge.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct VsCodeCommandExecutedPayload {
    /// Command id (or `openFile` / `runTask:<name>`).
    pub command_id: String,
    /// Whether it succeeded.
    pub ok: bool,
    /// Error message (redacted).
    pub error: Option<String>,
    /// Duration in ms.
    pub duration_ms: u64,
}

method!(
    /// `vscode.commands` — installed extensions with their commands, view containers and settings.
    Commands, "vscode.commands", VsCodeCommandsParams, VsCodeCommandsResult
);
method!(
    /// `vscode.execute` — runs a command in the VS Code window (audited; dangerous ones need `confirmed`).
    Execute, "vscode.execute", VsCodeExecuteParams, VsCodeExecuteResult
);
method!(
    /// `vscode.openView` — focuses an extension's view container (or its settings).
    OpenView, "vscode.openView", VsCodeOpenViewParams, VsCodeExecuteResult
);
method!(
    /// `vscode.openFile` — opens a file at a line in the VS Code window.
    OpenFile, "vscode.openFile", VsCodeOpenFileParams, VsCodeExecuteResult
);
method!(
    /// `vscode.tasks` — tasks of the workspace open in VS Code.
    Tasks, "vscode.tasks", VsCodeTasksParams, VsCodeTasksResult
);
method!(
    /// `vscode.runTask` — runs a VS Code task (needs `confirmed`).
    RunTask, "vscode.runTask", VsCodeRunTaskParams, VsCodeExecuteResult
);

method!(
    /// `vscode.listExtensions` — extensions installed in Orkestr's VS Code.
    ListExtensions, "vscode.listExtensions", Empty, VsCodeExtensionsResult
);
method!(
    /// `vscode.installExtension` — installs an Open VSX extension (server CLI).
    InstallExtension, "vscode.installExtension", VsCodeInstallExtensionParams, VsCodeExtensionsResult
);

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
