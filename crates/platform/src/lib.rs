//! # orkestr-platform
//!
//! The platform abstraction layer (MASTER_SPEC §46, contract §2): **all**
//! operating-system specific code of Orkestr lives in this crate
//! (`#[cfg(target_os = …)]` is not allowed anywhere else).
//!
//! The [`PlatformAdapter`] trait is the facade engines depend on; it exposes
//! one trait object per capability:
//!
//! | capability          | trait               | Windows | macOS | Linux |
//! |---------------------|---------------------|---------|-------|-------|
//! | paths               | [`paths::Paths`]    | ✔ | ✔ | ✔ |
//! | process control     | [`ProcessControl`]  | ✔ | ✔ | ✔ |
//! | shell detection     | [`ShellDetector`]   | ✔ PowerShell 7/5, cmd, Git Bash, WSL | ✔ /etc/shells | ✔ /etc/shells |
//! | credential store    | [`CredentialStore`] | ✔ Credential Manager | ✔ Keychain | ✔ kernel keyutils¹ |
//! | notifications       | [`Notifier`]        | ✖² | ✔ osascript | ✔ notify-send |
//! | autostart           | [`Autostart`]       | ✔ HKCU Run | ✔ LaunchAgent | ✔ XDG autostart |
//! | global hotkeys      | [`GlobalHotkeys`]   | ✖² | ✖² | ✖² |
//! | microphone          | [`Microphone`]      | ✖³ | ✖³ | ✖³ |
//! | computer control    | [`ComputerControl`] | ✖³ | ✖³ | ✖³ |
//! | power               | [`Power`]           | ✔ GetSystemPowerStatus | ✔ pmset | ✔ /sys |
//! | resources           | [`ResourceMonitor`] | ✔ sysinfo | ✔ | ✔ |
//! | probe commands      | [`run_with_timeout`] | ✔ no console window | ✔ | ✔ |
//! | GPUs                | [`detect_gpus`]     | ✔ nvidia-smi + WMI | ✔ system_profiler | ✔ nvidia-smi + lspci |
//! | web view runtime    | [`detect_webview`]  | ✔ WebView2 registry | ✔ WKWebView | ✔ WebKitGTK (pkg-config) |
//! | shutdown signals    | [`shutdown_signal`] | ✔ Ctrl-C, close, logoff, shutdown | ✔ INT/TERM/HUP | ✔ INT/TERM/HUP |
//!
//! ¹ keyutils is not persistent across reboots; the persistent Secret Service
//!   backend needs D-Bus and is a Phase 1 follow-up (see crate docs of
//!   `orkestr-security`).
//! ² provided by the Tauri desktop shell (plugins) — the headless core reports
//!   [`PlatformError::NotSupported`].
//! ³ arrives with its feature phase (voice §33, computer control §18); until
//!   then calls fail with an explicit [`PlatformError::NotSupported`].
//!
//! The only `unsafe` in the workspace is the documented Win32 FFI call in
//! [`power`] (`GetSystemPowerStatus`).
#![deny(unsafe_code)]
#![warn(missing_docs)]

pub mod autostart;
pub mod credentials;
pub mod error;
pub mod fs;
pub mod gpu;
pub mod hotkeys;
pub mod info;
pub mod microphone;
pub mod notifications;
pub mod paths;
pub mod power;
pub mod process;
pub mod resources;
pub mod shell;
pub mod signals;
pub mod webview;

use std::sync::Arc;

pub use autostart::{Autostart, AutostartEntry, MemoryAutostart, NativeAutostart};
pub use credentials::{CredentialStore, KeyringCredentialStore, MemoryCredentialStore};
pub use error::{PlatformError, PlatformResult};
pub use gpu::{detect_gpus, GpuDevice};
pub use hotkeys::{ComputerControl, GlobalHotkeys, UnsupportedComputerControl, UnsupportedHotkeys};
pub use info::PlatformInfo;
pub use microphone::{AudioDevice, Microphone, UnsupportedMicrophone};
pub use notifications::{DesktopNotification, NativeNotifier, Notifier};
pub use paths::Paths;
pub use power::{NativePower, Power, PowerStatus};
pub use process::{run_with_timeout, CommandOutput};
pub use process::{NativeProcessControl, ProcessControl};
pub use resources::{ResourceMonitor, ResourceSnapshot, SysinfoResourceMonitor};
pub use shell::{NativeShellDetector, ShellDetector, ShellInfo, ShellKind};
pub use signals::{shutdown_signal, ShutdownSignal};
pub use webview::{detect_webview, WebViewRuntime};

/// Facade over every platform capability (§46 PlatformAdapter).
pub trait PlatformAdapter: Send + Sync {
    /// Static information about the host.
    fn info(&self) -> &PlatformInfo;
    /// Resolved data/config/cache/log directories.
    fn paths(&self) -> &Paths;
    /// Process control.
    fn process(&self) -> &dyn ProcessControl;
    /// Shell detection.
    fn shells(&self) -> &dyn ShellDetector;
    /// OS credential store.
    fn credentials(&self) -> &dyn CredentialStore;
    /// Desktop notifications.
    fn notifications(&self) -> &dyn Notifier;
    /// Start-on-login registration.
    fn autostart(&self) -> &dyn Autostart;
    /// Global hotkeys.
    fn hotkeys(&self) -> &dyn GlobalHotkeys;
    /// Microphone access.
    fn microphone(&self) -> &dyn Microphone;
    /// Mouse/keyboard/screen control (never granted to agents by default).
    fn computer_control(&self) -> &dyn ComputerControl;
    /// Power / battery state.
    fn power(&self) -> &dyn Power;
    /// CPU/RAM/disk usage.
    fn resources(&self) -> &dyn ResourceMonitor;
}

/// The native adapter for the OS this binary was built for.
pub struct NativePlatform {
    info: PlatformInfo,
    paths: Paths,
    process: Arc<dyn ProcessControl>,
    shells: Arc<dyn ShellDetector>,
    credentials: Arc<dyn CredentialStore>,
    notifications: Arc<dyn Notifier>,
    autostart: Arc<dyn Autostart>,
    hotkeys: Arc<dyn GlobalHotkeys>,
    microphone: Arc<dyn Microphone>,
    computer_control: Arc<dyn ComputerControl>,
    power: Arc<dyn Power>,
    resources: Arc<dyn ResourceMonitor>,
}

impl std::fmt::Debug for NativePlatform {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativePlatform").field("info", &self.info).field("paths", &self.paths).finish_non_exhaustive()
    }
}

impl NativePlatform {
    /// Builds the native adapter for the given paths.
    #[must_use]
    pub fn new(paths: Paths) -> Self {
        let resources = Arc::new(SysinfoResourceMonitor::new(paths.data_dir.clone()));
        Self {
            info: PlatformInfo::detect(),
            process: Arc::new(NativeProcessControl),
            shells: Arc::new(NativeShellDetector::default()),
            credentials: Arc::new(KeyringCredentialStore::new()),
            notifications: Arc::new(NativeNotifier),
            autostart: Arc::new(NativeAutostart::default()),
            hotkeys: Arc::new(UnsupportedHotkeys),
            microphone: Arc::new(UnsupportedMicrophone),
            computer_control: Arc::new(UnsupportedComputerControl),
            power: Arc::new(NativePower),
            resources,
            paths,
        }
    }

    /// Replaces the credential store (tests use [`MemoryCredentialStore`]).
    #[must_use]
    pub fn with_credentials(mut self, store: Arc<dyn CredentialStore>) -> Self {
        self.credentials = store;
        self
    }

    /// Replaces the autostart implementation.
    #[must_use]
    pub fn with_autostart(mut self, autostart: Arc<dyn Autostart>) -> Self {
        self.autostart = autostart;
        self
    }
}

impl PlatformAdapter for NativePlatform {
    fn info(&self) -> &PlatformInfo {
        &self.info
    }
    fn paths(&self) -> &Paths {
        &self.paths
    }
    fn process(&self) -> &dyn ProcessControl {
        self.process.as_ref()
    }
    fn shells(&self) -> &dyn ShellDetector {
        self.shells.as_ref()
    }
    fn credentials(&self) -> &dyn CredentialStore {
        self.credentials.as_ref()
    }
    fn notifications(&self) -> &dyn Notifier {
        self.notifications.as_ref()
    }
    fn autostart(&self) -> &dyn Autostart {
        self.autostart.as_ref()
    }
    fn hotkeys(&self) -> &dyn GlobalHotkeys {
        self.hotkeys.as_ref()
    }
    fn microphone(&self) -> &dyn Microphone {
        self.microphone.as_ref()
    }
    fn computer_control(&self) -> &dyn ComputerControl {
        self.computer_control.as_ref()
    }
    fn power(&self) -> &dyn Power {
        self.power.as_ref()
    }
    fn resources(&self) -> &dyn ResourceMonitor {
        self.resources.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn native_platform_smoke() {
        let dir = tempfile::tempdir().unwrap();
        let p = NativePlatform::new(Paths::from_root(dir.path()))
            .with_credentials(Arc::new(MemoryCredentialStore::default()));
        assert!(!p.info().hostname.is_empty());
        assert!(p.computer_control().is_available().is_err());
        p.credentials().set("orkestr-test", "acct", "s3cret").unwrap();
        assert_eq!(p.credentials().get("orkestr-test", "acct").unwrap().as_deref(), Some("s3cret"));
        let snap = p.resources().snapshot();
        assert!(snap.cpu_cores > 0);
        assert!(snap.memory_total_bytes > 0);
    }
}
