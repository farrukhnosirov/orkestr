//! Screen + pointer access for cursor mode (ADR-0050): capture, UI
//! Automation tree, absolute pointer input, the abort guard (low-level
//! hooks), the safety overlay and Orkestr's own windows. Synchronous — the
//! session calls it on blocking threads. Windows: `backend/cursor_win.rs`;
//! tests: [`MockScreen`].

use std::sync::Arc;

use parking_lot::Mutex;

use orkestr_protocol::methods::computer::{MouseButton, ScrollDirection};

use crate::error::{ComputerError, ComputerResult};

/// A rectangle in physical virtual-desktop pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScreenRect {
    /// Left.
    pub x: i32,
    /// Top.
    pub y: i32,
    /// Width.
    pub w: i32,
    /// Height.
    pub h: i32,
}

impl ScreenRect {
    /// Contains the point.
    #[must_use]
    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= self.x && y >= self.y && x < self.x + self.w && y < self.y + self.h
    }
    /// Centre.
    #[must_use]
    pub fn center(&self) -> (i32, i32) {
        (self.x + self.w / 2, self.y + self.h / 2)
    }
}

/// The foreground window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Foreground {
    /// Handle (`0x…`).
    pub handle: String,
    /// Title.
    pub title: String,
    /// Process name.
    pub process: String,
    /// Process id.
    pub pid: u32,
    /// Window rectangle.
    pub rect: ScreenRect,
}

/// A captured image (BGRA, top-down).
#[derive(Debug, Clone, Default)]
pub struct RawImage {
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
    /// `width * height * 4` bytes, BGRA.
    pub bgra: Vec<u8>,
}

/// A UI Automation element in screen coordinates.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RawElement {
    /// Control type (`button`, `edit`…).
    pub role: String,
    /// Name.
    pub name: String,
    /// Bounds.
    pub rect: ScreenRect,
    /// Enabled.
    pub enabled: bool,
    /// Password field.
    pub password: bool,
    /// Toggle / value state.
    pub value: Option<String>,
}

/// Why the guard tripped.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AbortCause {
    /// Physical Esc.
    Esc,
    /// Physical mouse movement / click / wheel.
    Mouse,
}

impl AbortCause {
    /// Code for `abortReason`.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Esc => "esc",
            Self::Mouse => "mouse",
        }
    }
}

/// Guard callback (called on the hook thread — must be quick).
pub type TripFn = Arc<dyn Fn(AbortCause) + Send + Sync>;

/// OS access for cursor mode.
pub trait CursorBackend: Send + Sync {
    /// `windows` / `mock` / `unsupported`.
    fn name(&self) -> &'static str;
    /// Ok when usable.
    ///
    /// # Errors
    /// `Unsupported`.
    fn available(&self) -> ComputerResult<()>;
    /// The foreground window (`None` on the secure desktop / lock screen).
    ///
    /// # Errors
    /// Backend failure.
    fn foreground(&self) -> ComputerResult<Option<Foreground>>;
    /// The monitor that shows `window` (or the cursor's monitor).
    ///
    /// # Errors
    /// Backend failure.
    fn monitor_of(&self, window: Option<&Foreground>) -> ComputerResult<ScreenRect>;
    /// Captures a rectangle (windows excluded from capture are not in it).
    ///
    /// # Errors
    /// Backend failure (e.g. secure desktop).
    fn capture(&self, rect: ScreenRect) -> ComputerResult<RawImage>;
    /// UI Automation elements of a window (on-screen, named or interactive; capped).
    ///
    /// # Errors
    /// Backend failure.
    fn elements(&self, window: &Foreground, max: usize) -> ComputerResult<Vec<RawElement>>;
    /// Cursor position.
    ///
    /// # Errors
    /// Backend failure.
    fn cursor_pos(&self) -> ComputerResult<(i32, i32)>;
    /// Process id of the top-level window under a point.
    ///
    /// # Errors
    /// Backend failure.
    fn pid_at(&self, x: i32, y: i32) -> ComputerResult<u32>;
    /// Moves the pointer (injected, marked as Orkestr's).
    ///
    /// # Errors
    /// Input blocked.
    fn move_to(&self, x: i32, y: i32) -> ComputerResult<()>;
    /// Clicks at a point.
    ///
    /// # Errors
    /// Input blocked.
    fn click_at(&self, x: i32, y: i32, button: MouseButton, double: bool) -> ComputerResult<()>;
    /// Left-button drag.
    ///
    /// # Errors
    /// Input blocked.
    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> ComputerResult<()>;
    /// Wheel at a point (notches).
    ///
    /// # Errors
    /// Input blocked.
    fn scroll_at(&self, x: i32, y: i32, direction: ScrollDirection, notches: u32) -> ComputerResult<()>;
    /// The focused control is a password field.
    ///
    /// # Errors
    /// Backend failure.
    fn focused_is_password(&self) -> ComputerResult<bool>;
    /// Arms the abort guard (Esc always; physical mouse when `mouse`).
    ///
    /// # Errors
    /// Hooks could not be installed — the session must not start.
    fn arm_guard(&self, mouse: bool, on_trip: TripFn) -> ComputerResult<()>;
    /// Disarms the guard.
    fn disarm_guard(&self);
    /// Shows / updates the click-through, capture-excluded overlay (border + badge).
    ///
    /// # Errors
    /// Backend failure.
    fn overlay(&self, badge: Option<&str>) -> ComputerResult<()>;
    /// Hides Orkestr's windows from capture (and minimizes the normal ones
    /// when `minimize`), or restores them (`hide = false`).
    ///
    /// # Errors
    /// Backend failure.
    fn own_windows(&self, pids: &[u32], hide: bool, minimize: bool) -> ComputerResult<()>;
}

/// Scales a rectangle from screen to screenshot pixels.
#[must_use]
pub fn to_shot(r: ScreenRect, origin: ScreenRect, sw: u32, sh: u32) -> [i32; 4] {
    let fx = f64::from(sw) / f64::from(origin.w.max(1));
    let fy = f64::from(sh) / f64::from(origin.h.max(1));
    #[allow(clippy::cast_possible_truncation)]
    let f = |v: i32, k: f64| (f64::from(v) * k).round() as i32;
    [f(r.x - origin.x, fx), f(r.y - origin.y, fy), f(r.w, fx).max(1), f(r.h, fy).max(1)]
}

/// Maps a screenshot point to screen pixels.
#[must_use]
pub fn to_screen(x: i32, y: i32, origin: ScreenRect, sw: u32, sh: u32) -> (i32, i32) {
    let fx = f64::from(origin.w) / f64::from(sw.max(1));
    let fy = f64::from(origin.h) / f64::from(sh.max(1));
    #[allow(clippy::cast_possible_truncation)]
    let f = |v: i32, k: f64| (f64::from(v) * k + k / 2.0).floor() as i32;
    (origin.x + f(x, fx), origin.y + f(y, fy))
}

/// Box-filter downscale so the longer side is ≤ `max_side` (BGRA → RGB).
#[must_use]
pub fn downscale_rgb(img: &RawImage, max_side: u32) -> (u32, u32, Vec<u8>) {
    let (w, h) = (img.width.max(1), img.height.max(1));
    let longest = w.max(h);
    let (tw, th) = if longest <= max_side {
        (w, h)
    } else {
        let k = f64::from(max_side) / f64::from(longest);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let s = |v: u32| ((f64::from(v) * k).round() as u32).max(1);
        (s(w), s(h))
    };
    let mut out = vec![0u8; (tw * th * 3) as usize];
    if img.bgra.len() < (w * h * 4) as usize {
        return (tw, th, out);
    }
    for ty in 0..th {
        let y0 = ty * h / th;
        let y1 = ((ty + 1) * h / th).max(y0 + 1).min(h);
        for tx in 0..tw {
            let x0 = tx * w / tw;
            let x1 = ((tx + 1) * w / tw).max(x0 + 1).min(w);
            let (mut r, mut g, mut b, mut n) = (0u32, 0u32, 0u32, 0u32);
            for y in y0..y1 {
                let row = (y * w) as usize * 4;
                for x in x0..x1 {
                    let p = row + x as usize * 4;
                    b += u32::from(img.bgra[p]);
                    g += u32::from(img.bgra[p + 1]);
                    r += u32::from(img.bgra[p + 2]);
                    n += 1;
                }
            }
            let o = ((ty * tw + tx) * 3) as usize;
            let n = n.max(1);
            #[allow(clippy::cast_possible_truncation)]
            {
                out[o] = (r / n) as u8;
                out[o + 1] = (g / n) as u8;
                out[o + 2] = (b / n) as u8;
            }
        }
    }
    (tw, th, out)
}

/// Encodes RGB as PNG.
///
/// # Errors
/// Encoder failure.
pub fn encode_png(width: u32, height: u32, rgb: &[u8]) -> ComputerResult<Vec<u8>> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, width, height);
        enc.set_color(png::ColorType::Rgb);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().map_err(|e| ComputerError::Backend(format!("png: {e}")))?;
        w.write_image_data(rgb).map_err(|e| ComputerError::Backend(format!("png: {e}")))?;
    }
    Ok(out)
}

/// What the mock records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScreenCall {
    /// `move_to`.
    Move(i32, i32),
    /// `click_at`.
    Click(i32, i32, MouseButton, bool),
    /// `drag`.
    Drag((i32, i32), (i32, i32)),
    /// `scroll_at`.
    Scroll(i32, i32, ScrollDirection, u32),
    /// `arm_guard(mouse)`.
    Arm(bool),
    /// `disarm_guard`.
    Disarm,
    /// `overlay(Some)` / `overlay(None)`.
    Overlay(Option<String>),
    /// `own_windows(hide)`.
    OwnWindows(bool),
}

/// Scriptable test backend (a 1920×1080 monitor).
#[derive(Default)]
pub struct MockScreen {
    /// Calls.
    pub calls: Mutex<Vec<ScreenCall>>,
    /// Foreground window.
    pub foreground: Mutex<Option<Foreground>>,
    /// Elements returned.
    pub elements: Mutex<Vec<RawElement>>,
    /// Process under every point (default: the foreground's).
    pub pid_at: Mutex<Option<u32>>,
    /// Focused control is a password field.
    pub password_focused: Mutex<bool>,
    /// The guard callback (tests trip it).
    pub trip: Mutex<Option<TripFn>>,
    /// Capture fails (secure desktop).
    pub capture_fails: Mutex<bool>,
}

impl std::fmt::Debug for MockScreen {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MockScreen").finish_non_exhaustive()
    }
}

impl MockScreen {
    /// Monitor of the mock.
    pub const MONITOR: ScreenRect = ScreenRect { x: 0, y: 0, w: 1920, h: 1080 };

    /// With a foreground window.
    #[must_use]
    pub fn with_window(title: &str, process: &str, pid: u32) -> Self {
        let s = Self::default();
        *s.foreground.lock() = Some(Foreground {
            handle: "0x10".into(),
            title: title.into(),
            process: process.into(),
            pid,
            rect: ScreenRect { x: 100, y: 100, w: 800, h: 600 },
        });
        s
    }
    /// Recorded calls (taken).
    pub fn take_calls(&self) -> Vec<ScreenCall> {
        std::mem::take(&mut *self.calls.lock())
    }
    /// Input calls only.
    #[must_use]
    pub fn input_calls(&self) -> Vec<ScreenCall> {
        self.calls
            .lock()
            .iter()
            .filter(|c| matches!(c, ScreenCall::Move(..) | ScreenCall::Click(..) | ScreenCall::Drag(..) | ScreenCall::Scroll(..)))
            .cloned()
            .collect()
    }
    /// Simulates Esc / a physical mouse movement.
    pub fn trip(&self, cause: AbortCause) {
        let f = self.trip.lock().clone();
        if let Some(f) = f {
            f(cause);
        }
    }
    fn rec(&self, c: ScreenCall) {
        self.calls.lock().push(c);
    }
}

impl CursorBackend for MockScreen {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn available(&self) -> ComputerResult<()> {
        Ok(())
    }
    fn foreground(&self) -> ComputerResult<Option<Foreground>> {
        Ok(self.foreground.lock().clone())
    }
    fn monitor_of(&self, _: Option<&Foreground>) -> ComputerResult<ScreenRect> {
        Ok(Self::MONITOR)
    }
    fn capture(&self, rect: ScreenRect) -> ComputerResult<RawImage> {
        if *self.capture_fails.lock() {
            return Err(ComputerError::Backend("secure desktop".into()));
        }
        #[allow(clippy::cast_sign_loss)]
        let (w, h) = (rect.w as u32, rect.h as u32);
        Ok(RawImage { width: w, height: h, bgra: vec![200; (w * h * 4) as usize] })
    }
    fn elements(&self, _: &Foreground, max: usize) -> ComputerResult<Vec<RawElement>> {
        Ok(self.elements.lock().iter().take(max).cloned().collect())
    }
    fn cursor_pos(&self) -> ComputerResult<(i32, i32)> {
        Ok((960, 540))
    }
    fn pid_at(&self, _: i32, _: i32) -> ComputerResult<u32> {
        let fixed = *self.pid_at.lock();
        Ok(fixed.or_else(|| self.foreground.lock().as_ref().map(|f| f.pid)).unwrap_or(0))
    }
    fn move_to(&self, x: i32, y: i32) -> ComputerResult<()> {
        self.rec(ScreenCall::Move(x, y));
        Ok(())
    }
    fn click_at(&self, x: i32, y: i32, button: MouseButton, double: bool) -> ComputerResult<()> {
        self.rec(ScreenCall::Click(x, y, button, double));
        Ok(())
    }
    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> ComputerResult<()> {
        self.rec(ScreenCall::Drag(from, to));
        Ok(())
    }
    fn scroll_at(&self, x: i32, y: i32, direction: ScrollDirection, notches: u32) -> ComputerResult<()> {
        self.rec(ScreenCall::Scroll(x, y, direction, notches));
        Ok(())
    }
    fn focused_is_password(&self) -> ComputerResult<bool> {
        Ok(*self.password_focused.lock())
    }
    fn arm_guard(&self, mouse: bool, on_trip: TripFn) -> ComputerResult<()> {
        *self.trip.lock() = Some(on_trip);
        self.rec(ScreenCall::Arm(mouse));
        Ok(())
    }
    fn disarm_guard(&self) {
        *self.trip.lock() = None;
        self.rec(ScreenCall::Disarm);
    }
    fn overlay(&self, badge: Option<&str>) -> ComputerResult<()> {
        self.rec(ScreenCall::Overlay(badge.map(str::to_owned)));
        Ok(())
    }
    fn own_windows(&self, _: &[u32], hide: bool, _: bool) -> ComputerResult<()> {
        self.rec(ScreenCall::OwnWindows(hide));
        Ok(())
    }
}

/// Backend for hosts without cursor mode.
#[derive(Debug, Default, Clone, Copy)]
pub struct UnsupportedScreen;

fn unsupported<T>() -> ComputerResult<T> {
    Err(ComputerError::Unsupported(format!(
        "cursor mode is implemented for Windows only (this host: {})",
        std::env::consts::OS
    )))
}

impl CursorBackend for UnsupportedScreen {
    fn name(&self) -> &'static str {
        "unsupported"
    }
    fn available(&self) -> ComputerResult<()> {
        unsupported()
    }
    fn foreground(&self) -> ComputerResult<Option<Foreground>> {
        unsupported()
    }
    fn monitor_of(&self, _: Option<&Foreground>) -> ComputerResult<ScreenRect> {
        unsupported()
    }
    fn capture(&self, _: ScreenRect) -> ComputerResult<RawImage> {
        unsupported()
    }
    fn elements(&self, _: &Foreground, _: usize) -> ComputerResult<Vec<RawElement>> {
        unsupported()
    }
    fn cursor_pos(&self) -> ComputerResult<(i32, i32)> {
        unsupported()
    }
    fn pid_at(&self, _: i32, _: i32) -> ComputerResult<u32> {
        unsupported()
    }
    fn move_to(&self, _: i32, _: i32) -> ComputerResult<()> {
        unsupported()
    }
    fn click_at(&self, _: i32, _: i32, _: MouseButton, _: bool) -> ComputerResult<()> {
        unsupported()
    }
    fn drag(&self, _: (i32, i32), _: (i32, i32)) -> ComputerResult<()> {
        unsupported()
    }
    fn scroll_at(&self, _: i32, _: i32, _: ScrollDirection, _: u32) -> ComputerResult<()> {
        unsupported()
    }
    fn focused_is_password(&self) -> ComputerResult<bool> {
        Ok(false)
    }
    fn arm_guard(&self, _: bool, _: TripFn) -> ComputerResult<()> {
        unsupported()
    }
    fn disarm_guard(&self) {}
    fn overlay(&self, _: Option<&str>) -> ComputerResult<()> {
        Ok(())
    }
    fn own_windows(&self, _: &[u32], _: bool, _: bool) -> ComputerResult<()> {
        Ok(())
    }
}

/// The cursor backend for this host.
#[must_use]
pub fn default_cursor_backend() -> Arc<dyn CursorBackend> {
    #[cfg(windows)]
    {
        Arc::new(crate::backend::WindowsCursor::new())
    }
    #[cfg(not(windows))]
    {
        Arc::new(UnsupportedScreen)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coordinate_mapping_round_trips_on_a_scaled_secondary_monitor() {
        // a 2560×1440 monitor left of the primary, screenshot downscaled to 1280×720
        let mon = ScreenRect { x: -2560, y: 0, w: 2560, h: 1440 };
        let (x, y) = to_screen(640, 360, mon, 1280, 720);
        assert_eq!((x, y), (-2560 + 1281, 721));
        let r = to_shot(ScreenRect { x: -2560 + 200, y: 100, w: 400, h: 60 }, mon, 1280, 720);
        assert_eq!(r, [100, 50, 200, 30]);
        let (x, y) = to_screen(0, 0, mon, 1280, 720);
        assert!(mon.contains(x, y));
    }

    #[test]
    fn downscale_keeps_aspect_and_averages() {
        let img = RawImage { width: 4, height: 2, bgra: [[0u8, 0, 255, 255], [255, 0, 0, 255]].repeat(4).concat() };
        let (w, h, rgb) = downscale_rgb(&img, 2);
        assert_eq!((w, h), (2, 1));
        assert_eq!(rgb.len(), 6);
        assert_eq!(rgb[0], 127, "red averaged with blue");
        let png = encode_png(w, h, &rgb).unwrap();
        assert_eq!(&png[1..4], b"PNG");
        let (w, h, _) = downscale_rgb(&img, 1600);
        assert_eq!((w, h), (4, 2), "no upscaling");
    }
}
