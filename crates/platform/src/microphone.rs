//! Microphone access (§33 Voice Command Center).
//!
//! Voice capture lives in the desktop webview (getUserMedia) for Phase 3;
//! native capture for wake-word detection (local, no cloud streaming without
//! consent) is a later addition behind this trait.

use crate::error::{PlatformError, PlatformResult};

/// An audio input device.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDevice {
    /// Device id.
    pub id: String,
    /// Display name.
    pub name: String,
    /// Default input.
    pub is_default: bool,
}

/// Microphone enumeration/capture.
pub trait Microphone: Send + Sync {
    /// Lists input devices.
    fn list_devices(&self) -> PlatformResult<Vec<AudioDevice>>;
}

/// Placeholder implementation.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedMicrophone;

impl Microphone for UnsupportedMicrophone {
    fn list_devices(&self) -> PlatformResult<Vec<AudioDevice>> {
        Err(PlatformError::not_supported(
            "native microphone capture",
            "voice input uses the desktop webview for now; native capture arrives with wake-word support (§33)",
        ))
    }
}
