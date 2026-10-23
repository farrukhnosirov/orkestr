//! OS backends. [`ComputerBackend`] is synchronous (the executor calls it on
//! a blocking thread). Windows: `windows.rs` (ShellExecute, SendInput,
//! EnumWindows, UI Automation, PowerShell for brightness / toast /
//! screenshots / clipboard). Other OSes: [`UnsupportedBackend`] (every call
//! fails with an explicit `Unsupported`). [`MockBackend`] records calls for tests.

use std::path::Path;

use orkestr_protocol::methods::computer::{
    MediaKey, MouseButton, ProcessInfo, ScrollDirection, ScrollUnit, SystemOp, WindowInfo, WindowOp,
};

use crate::error::{ComputerError, ComputerResult};

#[cfg(windows)]
mod windows;
#[cfg(windows)]
pub use self::windows::WindowsBackend;

mod mock;
pub use mock::{MockBackend, MockCall};

/// An installed app from the Start menu (name + launch target).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledApp {
    /// Display name.
    pub name: String,
    /// Launch target (`shell:AppsFolder\<AUMID>`).
    pub target: String,
}

/// OS access used by the executor.
pub trait ComputerBackend: Send + Sync {
    /// Backend id (`windows`, `unsupported`, `mock`).
    fn name(&self) -> &'static str;
    /// Ok when usable.
    ///
    /// # Errors
    /// `Unsupported` with the reason.
    fn available(&self) -> ComputerResult<()>;
    /// Opens a program / file / folder / URI (`ShellExecute "open"`).
    ///
    /// # Errors
    /// Backend failure.
    fn shell_open(&self, target: &str, args: Option<&str>) -> ComputerResult<()>;
    /// Start-menu apps (may be slow; cached by the caller).
    ///
    /// # Errors
    /// Backend failure.
    fn installed_apps(&self) -> ComputerResult<Vec<InstalledApp>>;
    /// Visible top-level windows (z-order, foreground first).
    ///
    /// # Errors
    /// Backend failure.
    fn windows(&self) -> ComputerResult<Vec<WindowInfo>>;
    /// The foreground window.
    ///
    /// # Errors
    /// Backend failure.
    fn active_window(&self) -> ComputerResult<Option<WindowInfo>> {
        Ok(self.windows()?.into_iter().find(|w| w.active))
    }
    /// Window operation on `handle` (`None` = foreground window).
    ///
    /// # Errors
    /// Backend failure / unknown window.
    fn window_op(&self, handle: Option<&str>, op: WindowOp) -> ComputerResult<()>;
    /// Presses a combination (canonical key names, see `keys`).
    ///
    /// # Errors
    /// Backend failure.
    fn press(&self, keys: &[String]) -> ComputerResult<()>;
    /// Types Unicode text.
    ///
    /// # Errors
    /// Backend failure.
    fn type_text(&self, text: &str) -> ComputerResult<()>;
    /// Scrolls.
    ///
    /// # Errors
    /// Backend failure.
    fn scroll(&self, direction: ScrollDirection, amount: u32, unit: ScrollUnit) -> ComputerResult<()>;
    /// Mouse click.
    ///
    /// # Errors
    /// Backend failure.
    fn mouse_click(&self, button: MouseButton, at: Option<(i32, i32)>) -> ComputerResult<()>;
    /// Clicks (invokes) an element of the foreground window by accessible name.
    ///
    /// # Errors
    /// `NotFound` when no element has that name.
    fn click_element(&self, window: Option<&str>, name: &str) -> ComputerResult<String>;
    /// True when the focused control is a password field.
    ///
    /// # Errors
    /// Backend failure.
    fn focused_is_password(&self) -> ComputerResult<bool>;
    /// Media key.
    ///
    /// # Errors
    /// Backend failure.
    fn media(&self, key: MediaKey) -> ComputerResult<()>;
    /// Volume up / down in percent steps (positive / negative), or mute toggle.
    ///
    /// # Errors
    /// Backend failure.
    fn volume_step(&self, delta_percent: i32, mute_toggle: bool) -> ComputerResult<()>;
    /// Brightness: `Some(percent)` sets, `None` reads.
    ///
    /// # Errors
    /// `Unsupported` on external monitors.
    fn brightness(&self, set: Option<u32>) -> ComputerResult<u32>;
    /// Lock / sleep / shutdown / restart / sign out.
    ///
    /// # Errors
    /// Backend failure.
    fn system(&self, op: SystemOp) -> ComputerResult<()>;
    /// Screenshot of all screens to a PNG file.
    ///
    /// # Errors
    /// Backend failure.
    fn screenshot(&self, path: &Path) -> ComputerResult<()>;
    /// Clipboard text.
    ///
    /// # Errors
    /// Backend failure.
    fn clipboard_get(&self) -> ComputerResult<String>;
    /// Sets the clipboard.
    ///
    /// # Errors
    /// Backend failure.
    fn clipboard_set(&self, text: &str) -> ComputerResult<()>;
    /// Desktop notification.
    ///
    /// # Errors
    /// Backend failure.
    fn notify(&self, title: &str, body: &str) -> ComputerResult<()>;
    /// Running processes.
    ///
    /// # Errors
    /// Backend failure.
    fn processes(&self) -> ComputerResult<Vec<ProcessInfo>>;
    /// Kills a process.
    ///
    /// # Errors
    /// Backend failure.
    fn kill(&self, pid: u32) -> ComputerResult<()>;
    /// Moves a file / folder to the recycle bin.
    ///
    /// # Errors
    /// Backend failure.
    fn recycle(&self, path: &Path) -> ComputerResult<()>;
}

/// Backend for hosts without computer control.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedBackend;

fn unsupported<T>() -> ComputerResult<T> {
    Err(ComputerError::Unsupported(format!(
        "computer control is implemented for Windows only (this host: {})",
        std::env::consts::OS
    )))
}

impl ComputerBackend for UnsupportedBackend {
    fn name(&self) -> &'static str {
        "unsupported"
    }
    fn available(&self) -> ComputerResult<()> {
        unsupported()
    }
    fn shell_open(&self, _: &str, _: Option<&str>) -> ComputerResult<()> {
        unsupported()
    }
    fn installed_apps(&self) -> ComputerResult<Vec<InstalledApp>> {
        Ok(Vec::new())
    }
    fn windows(&self) -> ComputerResult<Vec<WindowInfo>> {
        unsupported()
    }
    fn window_op(&self, _: Option<&str>, _: WindowOp) -> ComputerResult<()> {
        unsupported()
    }
    fn press(&self, _: &[String]) -> ComputerResult<()> {
        unsupported()
    }
    fn type_text(&self, _: &str) -> ComputerResult<()> {
        unsupported()
    }
    fn scroll(&self, _: ScrollDirection, _: u32, _: ScrollUnit) -> ComputerResult<()> {
        unsupported()
    }
    fn mouse_click(&self, _: MouseButton, _: Option<(i32, i32)>) -> ComputerResult<()> {
        unsupported()
    }
    fn click_element(&self, _: Option<&str>, _: &str) -> ComputerResult<String> {
        unsupported()
    }
    fn focused_is_password(&self) -> ComputerResult<bool> {
        Ok(false)
    }
    fn media(&self, _: MediaKey) -> ComputerResult<()> {
        unsupported()
    }
    fn volume_step(&self, _: i32, _: bool) -> ComputerResult<()> {
        unsupported()
    }
    fn brightness(&self, _: Option<u32>) -> ComputerResult<u32> {
        unsupported()
    }
    fn system(&self, _: SystemOp) -> ComputerResult<()> {
        unsupported()
    }
    fn screenshot(&self, _: &Path) -> ComputerResult<()> {
        unsupported()
    }
    fn clipboard_get(&self) -> ComputerResult<String> {
        unsupported()
    }
    fn clipboard_set(&self, _: &str) -> ComputerResult<()> {
        unsupported()
    }
    fn notify(&self, _: &str, _: &str) -> ComputerResult<()> {
        unsupported()
    }
    fn processes(&self) -> ComputerResult<Vec<ProcessInfo>> {
        Ok(crate::procs::list())
    }
    fn kill(&self, pid: u32) -> ComputerResult<()> {
        crate::procs::kill(pid)
    }
    fn recycle(&self, _: &Path) -> ComputerResult<()> {
        unsupported()
    }
}

/// The backend for this host.
#[must_use]
pub fn default_backend() -> std::sync::Arc<dyn ComputerBackend> {
    #[cfg(windows)]
    {
        std::sync::Arc::new(WindowsBackend::new())
    }
    #[cfg(not(windows))]
    {
        std::sync::Arc::new(UnsupportedBackend)
    }
}
