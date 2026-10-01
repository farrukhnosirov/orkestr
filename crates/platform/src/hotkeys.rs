//! Global hotkeys and computer control.
//!
//! * Global hotkeys are registered by the desktop shell (Tauri global-shortcut
//!   plugin); the headless core has no message loop, so it reports
//!   `NotSupported`.
//! * Computer control (§18) is a separate, never-default capability. The real
//!   implementation (input injection + screen capture per OS) arrives with its
//!   phase and will always be gated by `Capability::ComputerControl`.

use crate::error::{PlatformError, PlatformResult};

/// A hotkey binding, e.g. `Ctrl+Shift+Space`.
pub trait GlobalHotkeys: Send + Sync {
    /// Registers a hotkey under an action id.
    fn register(&self, action: &str, accelerator: &str) -> PlatformResult<()>;
    /// Unregisters an action.
    fn unregister(&self, action: &str) -> PlatformResult<()>;
}

/// Headless implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedHotkeys;

impl GlobalHotkeys for UnsupportedHotkeys {
    fn register(&self, _action: &str, _accelerator: &str) -> PlatformResult<()> {
        Err(PlatformError::not_supported("global hotkeys", "registered by the desktop shell"))
    }
    fn unregister(&self, _action: &str) -> PlatformResult<()> {
        Err(PlatformError::not_supported("global hotkeys", "registered by the desktop shell"))
    }
}

/// Mouse/keyboard/screen control.
pub trait ComputerControl: Send + Sync {
    /// Ok when the capability can be used on this host.
    fn is_available(&self) -> PlatformResult<()>;
    /// Captures the primary screen as PNG bytes.
    fn screenshot_png(&self) -> PlatformResult<Vec<u8>>;
    /// Moves the pointer and clicks.
    fn click(&self, x: i32, y: i32) -> PlatformResult<()>;
    /// Types text.
    fn type_text(&self, text: &str) -> PlatformResult<()>;
}

/// Placeholder until the computer-control phase.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedComputerControl;

const CC_HINT: &str = "computer control arrives in a later phase (§18) and is never enabled by default";

impl ComputerControl for UnsupportedComputerControl {
    fn is_available(&self) -> PlatformResult<()> {
        Err(PlatformError::not_supported("computer control", CC_HINT))
    }
    fn screenshot_png(&self) -> PlatformResult<Vec<u8>> {
        Err(PlatformError::not_supported("computer control", CC_HINT))
    }
    fn click(&self, _x: i32, _y: i32) -> PlatformResult<()> {
        Err(PlatformError::not_supported("computer control", CC_HINT))
    }
    fn type_text(&self, _text: &str) -> PlatformResult<()> {
        Err(PlatformError::not_supported("computer control", CC_HINT))
    }
}
