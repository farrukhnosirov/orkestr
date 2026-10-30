//! Extended PC control (wave 10, ADR-0055): one request / reply pair per
//! low-level capability the executor composes into actions — endpoint and
//! per-app volume, microphone, audio devices, the media session, window
//! geometry / topmost / opacity, the cursor and mouse buttons, pointer
//! speed, keyboard layouts, displays, screenshots of a rectangle, radios,
//! battery, the recycle bin. Backends answer [`ComputerBackend::pc`]
//! (default: `Unsupported`); the geometry and arithmetic stay in the
//! executor so the mock backend tests them.
//!
//! [`ComputerBackend::pc`]: super::ComputerBackend::pc

use std::path::PathBuf;

use orkestr_protocol::methods::computer::{AudioDeviceKind, DisplayOp, MouseButton, RadioKind};

use crate::error::{ComputerError, ComputerResult};

/// A screen rectangle (px, virtual-screen coordinates).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    /// Left.
    pub left: i32,
    /// Top.
    pub top: i32,
    /// Right (exclusive).
    pub right: i32,
    /// Bottom (exclusive).
    pub bottom: i32,
}

impl Rect {
    /// New rect from position and size.
    #[must_use]
    pub const fn xywh(x: i32, y: i32, w: i32, h: i32) -> Self {
        Self { left: x, top: y, right: x + w, bottom: y + h }
    }
    /// Width.
    #[must_use]
    pub const fn width(&self) -> i32 {
        self.right - self.left
    }
    /// Height.
    #[must_use]
    pub const fn height(&self) -> i32 {
        self.bottom - self.top
    }
    /// Center point.
    #[must_use]
    pub const fn center(&self) -> (i32, i32) {
        (self.left + self.width() / 2, self.top + self.height() / 2)
    }
}

/// An audio endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioDeviceInfo {
    /// Endpoint id.
    pub id: String,
    /// Friendly name.
    pub name: String,
    /// The default device of its kind.
    pub default: bool,
}

/// An app's audio session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioSessionInfo {
    /// Process name (no `.exe`).
    pub app: String,
    /// Process id.
    pub pid: u32,
    /// Session volume percent.
    pub percent: u32,
    /// Muted.
    pub muted: bool,
}

/// The current media session.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MediaInfo {
    /// Title.
    pub title: String,
    /// Artist.
    pub artist: String,
    /// Source app id.
    pub app: String,
    /// Playing (not paused).
    pub playing: bool,
}

/// Battery state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BatteryInfo {
    /// Percent (absent = no battery).
    pub percent: Option<u32>,
    /// Charging.
    pub charging: bool,
    /// On AC power.
    pub plugged: bool,
}

/// A request to the backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcRequest {
    /// Master volume of the default output → `Level`.
    GetVolume,
    /// Sets the master volume percent → `Level`.
    SetVolume(u32),
    /// Mutes (`Some(true)`), unmutes or toggles (`None`) the output → `Level`.
    SetMute(Option<bool>),
    /// Volume of an app's audio sessions → `Level`.
    GetAppVolume(String),
    /// Sets an app's session volume → `Level`.
    SetAppVolume(String, u32),
    /// Mutes / unmutes / toggles an app → `Level`.
    SetAppMute(String, Option<bool>),
    /// Default microphone level → `Level`.
    GetMic,
    /// Sets the microphone level → `Level`.
    SetMic(u32),
    /// Mutes / unmutes / toggles the microphone → `Level`.
    SetMicMute(Option<bool>),
    /// Active endpoints → `Devices`.
    AudioDevices(AudioDeviceKind),
    /// Makes an endpoint the default (all roles) → `Unit`.
    SetDefaultAudioDevice(AudioDeviceKind, String),
    /// Audio sessions of the default output → `Sessions`.
    AudioSessions,
    /// Current media session → `Media`.
    NowPlaying,
    /// Seeks the current media session by seconds → `Unit`.
    MediaSeek(i64),
    /// Work area of the monitor of a window (`None` = under the cursor) → `Rect`.
    WorkArea(Option<String>),
    /// Full bounds of the monitor under the cursor → `Rect`.
    MonitorBounds,
    /// Window rectangle → `Rect`.
    WindowRect(String),
    /// Moves / resizes a window (restored first) → `Unit`.
    PlaceWindow(String, Rect),
    /// Always on top on / off → `Unit`.
    SetTopmost(String, bool),
    /// Window opacity percent → `Unit`.
    SetOpacity(String, u32),
    /// Cursor position → `Point`.
    CursorPos,
    /// Moves the cursor → `Unit`.
    SetCursor(i32, i32),
    /// Presses (`true`) or releases a mouse button → `Unit`.
    MouseButton(MouseButton, bool),
    /// Center of a named UI element of a window (`None` = foreground) → `Element`.
    ElementCenter(Option<String>, String),
    /// Pointer speed 1..20 → `Level`.
    GetMouseSpeed,
    /// Sets the pointer speed 1..20 → `Level`.
    SetMouseSpeed(u32),
    /// Keyboard layout of the foreground window → `Text` (`ru-RU`).
    GetKeyboardLayout,
    /// Switches the foreground window's layout (`None` = next) → `Text`.
    SetKeyboardLayout(Option<String>),
    /// Display operation → `Unit`.
    Display(DisplayOp),
    /// Captures a rectangle to a PNG and / or the clipboard → `Unit`.
    CaptureRect(Rect, Option<PathBuf>, bool),
    /// Radio on / off → `Text` (names of the radios switched).
    Radio(RadioKind, bool),
    /// Battery → `Battery`.
    Battery,
    /// Empties the recycle bin → `Unit`.
    EmptyRecycleBin,
}

/// A reply.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PcReply {
    /// Done.
    Unit,
    /// Level percent + muted.
    Level(u32, bool),
    /// Audio endpoints.
    Devices(Vec<AudioDeviceInfo>),
    /// Audio sessions.
    Sessions(Vec<AudioSessionInfo>),
    /// Media session.
    Media(Option<MediaInfo>),
    /// Rectangle.
    Rect(Rect),
    /// Point.
    Point(i32, i32),
    /// UI element center + its name.
    Element(i32, i32, String),
    /// Text.
    Text(String),
    /// Battery.
    Battery(BatteryInfo),
}

fn wrong(r: &PcReply) -> ComputerError {
    ComputerError::Backend(format!("unexpected backend reply {r:?}"))
}

impl PcReply {
    /// `(percent, muted)`.
    ///
    /// # Errors
    /// Another reply.
    pub fn level(self) -> ComputerResult<(u32, bool)> {
        match self {
            Self::Level(p, m) => Ok((p, m)),
            r => Err(wrong(&r)),
        }
    }
    /// Rectangle.
    ///
    /// # Errors
    /// Another reply.
    pub fn rect(self) -> ComputerResult<Rect> {
        match self {
            Self::Rect(r) => Ok(r),
            r => Err(wrong(&r)),
        }
    }
    /// Point.
    ///
    /// # Errors
    /// Another reply.
    pub fn point(self) -> ComputerResult<(i32, i32)> {
        match self {
            Self::Point(x, y) | Self::Element(x, y, _) => Ok((x, y)),
            r => Err(wrong(&r)),
        }
    }
    /// Text.
    ///
    /// # Errors
    /// Another reply.
    pub fn text(self) -> ComputerResult<String> {
        match self {
            Self::Text(t) | Self::Element(_, _, t) => Ok(t),
            r => Err(wrong(&r)),
        }
    }
    /// Devices.
    ///
    /// # Errors
    /// Another reply.
    pub fn devices(self) -> ComputerResult<Vec<AudioDeviceInfo>> {
        match self {
            Self::Devices(d) => Ok(d),
            r => Err(wrong(&r)),
        }
    }
    /// Sessions.
    ///
    /// # Errors
    /// Another reply.
    pub fn sessions(self) -> ComputerResult<Vec<AudioSessionInfo>> {
        match self {
            Self::Sessions(s) => Ok(s),
            r => Err(wrong(&r)),
        }
    }
    /// Media.
    ///
    /// # Errors
    /// Another reply.
    pub fn media(self) -> ComputerResult<Option<MediaInfo>> {
        match self {
            Self::Media(m) => Ok(m),
            r => Err(wrong(&r)),
        }
    }
    /// Battery.
    ///
    /// # Errors
    /// Another reply.
    pub fn battery(self) -> ComputerResult<BatteryInfo> {
        match self {
            Self::Battery(b) => Ok(b),
            r => Err(wrong(&r)),
        }
    }
}

/// Default answer of backends without the capability.
///
/// # Errors
/// Always `Unsupported`.
pub fn unsupported_pc(req: &PcRequest) -> ComputerResult<PcReply> {
    Err(ComputerError::Unsupported(format!("{req:?} is not supported on this host ({})", std::env::consts::OS)))
}
