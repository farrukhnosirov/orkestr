//! Desktop notifications from the headless core.
//!
//! The desktop app shows notifications through the Tauri shell; this adapter
//! is for headless cores (`orkestrd`). On Windows toast notifications require
//! an AppUserModelID registered by an installer, so the core reports
//! `NotSupported` and relies on the desktop shell / mobile push.

use crate::error::{PlatformError, PlatformResult};

/// A notification to display.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopNotification {
    /// Title.
    pub title: String,
    /// Body.
    pub body: String,
}

/// Shows desktop notifications.
pub trait Notifier: Send + Sync {
    /// Displays a notification.
    fn notify(&self, notification: &DesktopNotification) -> PlatformResult<()>;
}

/// Native implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct NativeNotifier;

#[cfg(target_os = "macos")]
fn applescript_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

impl Notifier for NativeNotifier {
    #[cfg(target_os = "windows")]
    fn notify(&self, _notification: &DesktopNotification) -> PlatformResult<()> {
        Err(PlatformError::not_supported(
            "notifications",
            "the headless core cannot raise toasts without an installed AppUserModelID; the desktop shell shows them",
        ))
    }

    #[cfg(target_os = "macos")]
    fn notify(&self, n: &DesktopNotification) -> PlatformResult<()> {
        let script =
            format!("display notification {} with title {}", applescript_string(&n.body), applescript_string(&n.title));
        crate::error::run_command("osascript", &["-e", &script]).map(|_| ())
    }

    #[cfg(target_os = "linux")]
    fn notify(&self, n: &DesktopNotification) -> PlatformResult<()> {
        if crate::process::which("notify-send").is_none() {
            return Err(PlatformError::not_supported("notifications", "install libnotify (notify-send)"));
        }
        crate::error::run_command("notify-send", &["--app-name=Orkestr", "--", &n.title, &n.body]).map(|_| ())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    fn notify(&self, _n: &DesktopNotification) -> PlatformResult<()> {
        Err(PlatformError::not_supported("notifications", "unsupported OS"))
    }
}
