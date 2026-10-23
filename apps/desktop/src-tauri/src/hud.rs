//! HUD / Ambient window (§34): a compact, translucent, always-on-top window
//! with the mini Manager input, the agent strip, run status, the voice slot
//! and approvals. It is created lazily and reuses the same frontend bundle;
//! the webview recognises it by its window label ([`HUD_WINDOW`]) and talks
//! to the core over RPC like every other window (no business data here).
//!
//! Window management only:
//! * `hud_show {x?, y?}` / `hud_hide` / `hud_toggle {x?, y?}` — the position
//!   is remembered **per device** by the webview (local storage) and passed
//!   in; positions that are not on any connected monitor fall back to the
//!   top-right corner of the primary monitor.
//! * `hud_collapse` hides the main window into the HUD; `hud_expand` brings
//!   the main window back and hides the HUD.

use tauri::{AppHandle, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

/// Label of the HUD window (the webview renders the HUD for this label).
pub const HUD_WINDOW: &str = "hud";
/// Logical size of the HUD.
pub const HUD_SIZE: (f64, f64) = (460.0, 148.0);

/// A remembered HUD position (physical pixels, outer top-left corner).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize, serde::Serialize)]
pub struct HudPosition {
    /// X.
    pub x: i32,
    /// Y.
    pub y: i32,
}

/// A monitor rectangle (physical pixels).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rect {
    /// Left.
    pub x: i32,
    /// Top.
    pub y: i32,
    /// Width.
    pub w: u32,
    /// Height.
    pub h: u32,
}

impl Rect {
    fn contains(&self, x: i32, y: i32) -> bool {
        let right = i64::from(self.x) + i64::from(self.w);
        let bottom = i64::from(self.y) + i64::from(self.h);
        i64::from(x) >= i64::from(self.x)
            && i64::from(x) < right
            && i64::from(y) >= i64::from(self.y)
            && i64::from(y) < bottom
    }
}

/// Where to put the HUD: the remembered position if a meaningful part of the
/// window (its top-left 48×48 px) is on a monitor, otherwise the top-right
/// corner of `primary` with a margin.
#[must_use]
pub fn resolve_position(
    saved: Option<HudPosition>,
    monitors: &[Rect],
    primary: Option<Rect>,
    size: (u32, u32),
) -> Option<HudPosition> {
    if let Some(p) = saved {
        if monitors.iter().any(|m| m.contains(p.x + 24, p.y + 24)) {
            return Some(p);
        }
    }
    let m = primary.or_else(|| monitors.first().copied())?;
    let margin = 24;
    let x = m.x + i32::try_from(m.w).unwrap_or(i32::MAX).saturating_sub(i32::try_from(size.0).unwrap_or(0) + margin);
    Some(HudPosition { x, y: m.y + margin + 32 })
}

fn rect_of(m: &tauri::Monitor) -> Rect {
    Rect { x: m.position().x, y: m.position().y, w: m.size().width, h: m.size().height }
}

fn ensure(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(w) = app.get_webview_window(HUD_WINDOW) {
        return Ok(w);
    }
    WebviewWindowBuilder::new(app, HUD_WINDOW, WebviewUrl::App("index.html".into()))
        .title("Orkestr HUD")
        .additional_browser_args(crate::webview_policy::BROWSER_ARGS)
        .inner_size(HUD_SIZE.0, HUD_SIZE.1)
        .min_inner_size(360.0, 96.0)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(true)
        .visible(false)
        .focused(false)
        .build()
}

fn place(app: &AppHandle, w: &WebviewWindow, saved: Option<HudPosition>) {
    let monitors: Vec<Rect> = app.available_monitors().map(|v| v.iter().map(rect_of).collect()).unwrap_or_default();
    let primary = app.primary_monitor().ok().flatten().map(|m| rect_of(&m));
    let size = w.outer_size().map(|s| (s.width, s.height)).unwrap_or((460, 148));
    if let Some(p) = resolve_position(saved, &monitors, primary, size) {
        let _ = w.set_position(PhysicalPosition::new(p.x, p.y));
    }
}

fn show(app: &AppHandle, position: Option<HudPosition>) -> Result<(), String> {
    let w = ensure(app).map_err(|e| e.to_string())?;
    place(app, &w, position);
    w.show().map_err(|e| e.to_string())?;
    let _ = w.set_always_on_top(true);
    tracing::info!("HUD shown");
    Ok(())
}

fn hide(app: &AppHandle) {
    if let Some(w) = app.get_webview_window(HUD_WINDOW) {
        let _ = w.hide();
    }
}

fn hud_visible(app: &AppHandle) -> bool {
    app.get_webview_window(HUD_WINDOW).and_then(|w| w.is_visible().ok()).unwrap_or(false)
}

// The commands are `async` on purpose: synchronous commands run on the main
// thread, and creating a webview window there deadlocks on Windows (WebView2).

/// Shows the HUD at the remembered position.
#[tauri::command]
pub async fn hud_show(app: AppHandle, position: Option<HudPosition>) -> Result<(), String> {
    show(&app, position)
}

/// Hides the HUD.
#[tauri::command]
pub async fn hud_hide(app: AppHandle) {
    hide(&app);
}

/// Toggles the HUD; returns whether it is visible afterwards.
#[tauri::command]
pub async fn hud_toggle(app: AppHandle, position: Option<HudPosition>) -> Result<bool, String> {
    if hud_visible(&app) {
        hide(&app);
        Ok(false)
    } else {
        show(&app, position)?;
        Ok(true)
    }
}

/// Collapses the main window into the HUD (main hidden, agents keep running).
#[tauri::command]
pub async fn hud_collapse(app: AppHandle, position: Option<HudPosition>) -> Result<(), String> {
    show(&app, position)?;
    if let Some(main) = app.get_webview_window(crate::MAIN_WINDOW) {
        let _ = main.hide();
    }
    Ok(())
}

/// Brings the main window back and hides the HUD.
#[tauri::command]
pub async fn hud_expand(app: AppHandle) {
    crate::show_main(&app);
    hide(&app);
}

#[cfg(test)]
mod tests {
    use super::*;

    const PRIMARY: Rect = Rect { x: 0, y: 0, w: 1920, h: 1080 };
    const SECOND: Rect = Rect { x: 1920, y: 0, w: 2560, h: 1440 };

    #[test]
    fn remembered_positions_are_kept_on_any_monitor() {
        let p = HudPosition { x: 2400, y: 300 };
        assert_eq!(resolve_position(Some(p), &[PRIMARY, SECOND], Some(PRIMARY), (460, 148)), Some(p));
    }

    #[test]
    fn off_screen_positions_fall_back_to_the_primary_corner() {
        let gone = HudPosition { x: 5000, y: 300 }; // the second monitor was unplugged
        let p = resolve_position(Some(gone), &[PRIMARY], Some(PRIMARY), (460, 148)).unwrap();
        assert_eq!(p, HudPosition { x: 1920 - 460 - 24, y: 56 });
        assert_eq!(resolve_position(None, &[], None, (460, 148)), None);
    }
}
