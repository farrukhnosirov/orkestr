//! The floating orb (ADR-0045, supersedes the per-agent robots of ADR-0016).
//!
//! One small transparent, frameless, always-on-top, skip-taskbar
//! `WebviewWindow` (`orb`) on the same frontend bundle as every other window.
//! The webview recognises it by its label and talks to the core over RPC; this
//! module only manages the window (§67) — no business data flows through it.
//!
//! * **Collapsed:** the window is a square box around the sphere (the sphere
//!   plus room for its glow and badge).
//! * **Hub open:** the window grows to hold the sphere *and* the Agent Hub
//!   flyout next to it. [`hub_layout`] picks the side with room (right of the
//!   sphere, else left) and keeps the flyout inside the monitor's work area;
//!   the sphere never moves on screen.
//! * **Click-through (RR-C4):** the window ignores cursor events except over
//!   the interactive shape (sphere circle + flyout / menu rectangles reported
//!   by the webview via [`orb_set_shape`]). A 30 Hz poller runs only while the
//!   orb is visible.
//! * **Drag / snap:** the webview calls `startDragging()`. Moves while the
//!   cursor is over the sphere are user drags; 400 ms without a move ends the
//!   drag, then the sphere snaps to work-area edges / corners
//!   ([`SNAP_THRESHOLD`]) and the webview gets [`EV_MOVED`] (position + monitor
//!   name) to remember it **per monitor** (local storage of this device).
//! * **Main window:** `orb_show` / `orb_hide` / `orb_toggle_hub` /
//!   `orb_reset_position` (main only); orb → main requests go through
//!   [`orb_to_main`] (whitelisted names), which also brings the main window
//!   forward.

use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindow,
    WebviewWindowBuilder,
};
use tokio::sync::Notify;

/// Label of the orb window (the webview renders the orb for this label).
pub const ORB_WINDOW: &str = "orb";
/// Quiet time after the last move that ends a drag.
pub const DRAG_END_QUIET: Duration = Duration::from_millis(400);
/// Snap distance to work-area edges, in logical px.
pub const SNAP_THRESHOLD: f64 = 28.0;
/// Margin from the work-area corner of the default position, in logical px.
pub const DEFAULT_MARGIN: f64 = 24.0;
/// Gap between the sphere and the hub flyout, in logical px.
pub const HUB_GAP: f64 = 8.0;
/// Cursor polling period (30 Hz).
const POLL: Duration = Duration::from_millis(33);

/// Shell → orb: the cursor entered / left the interactive shape (`bool`).
pub const EV_HOVER: &str = "orkestr://orb/hover";
/// Shell → orb: drag started / ended (`bool`).
pub const EV_DRAG: &str = "orkestr://orb/drag";
/// Shell → orb: the orb settled at a new position (`{x, y, monitor}`).
pub const EV_MOVED: &str = "orkestr://orb/moved";
/// Shell → orb: toggle the Agent Hub (hotkey in the main window).
pub const EV_HUB: &str = "orkestr://orb/hub";
/// Main window ← orb (via [`orb_to_main`]) prefix: `orkestr://orb/<name>`.
const EV_MAIN_PREFIX: &str = "orkestr://orb/";

/// An axis-aligned rectangle.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        px >= self.x && py >= self.y && px <= self.x + self.w && py <= self.y + self.h
    }
    #[must_use]
    pub fn center(&self) -> (f64, f64) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
    fn right(&self) -> f64 {
        self.x + self.w
    }
    fn bottom(&self) -> f64 {
        self.y + self.h
    }
}

/// A circle in window-local CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Circle {
    pub cx: f64,
    pub cy: f64,
    pub r: f64,
}

/// Interactive area of the orb window (window-local CSS px).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HitShape {
    #[serde(default)]
    pub circles: Vec<Circle>,
    #[serde(default)]
    pub rects: Vec<Rect>,
}

impl HitShape {
    /// Whether a window-local CSS px point is interactive.
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        self.circles.iter().any(|c| c.r > 0.0 && (px - c.cx).hypot(py - c.cy) <= c.r)
            || self.rects.iter().any(|r| r.contains(px, py))
    }
}

/// The work area that holds a rect's centre (else the nearest one).
#[must_use]
pub fn area_of(rect: Rect, areas: &[Rect]) -> Option<Rect> {
    let (cx, cy) = rect.center();
    areas
        .iter()
        .find(|a| a.contains(cx, cy))
        .or_else(|| {
            areas.iter().min_by(|a, b| {
                let da = (a.center().0 - cx).hypot(a.center().1 - cy);
                let db = (b.center().0 - cx).hypot(b.center().1 - cy);
                da.total_cmp(&db)
            })
        })
        .copied()
}

/// Snaps a window rect to the edges / corners of its work area and keeps it
/// on screen. Returns the new top-left (same unit as the inputs).
#[must_use]
pub fn snap_rect(rect: Rect, areas: &[Rect], threshold: f64) -> (f64, f64) {
    let Some(a) = area_of(rect, areas) else { return (rect.x, rect.y) };
    let pick = |current: f64, candidates: [f64; 2]| {
        candidates
            .iter()
            .copied()
            .filter(|c| (c - current).abs() <= threshold)
            .min_by(|x, y| (x - current).abs().total_cmp(&(y - current).abs()))
            .unwrap_or(current)
    };
    let x = pick(rect.x, [a.x, a.right() - rect.w]).clamp(a.x, (a.right() - rect.w).max(a.x));
    let y = pick(rect.y, [a.y, a.bottom() - rect.h]).clamp(a.y, (a.bottom() - rect.h).max(a.y));
    (x, y)
}

/// Default orb position: the bottom-right corner of the work area, inset by `margin`.
#[must_use]
pub fn default_position(area: Rect, w: f64, h: f64, margin: f64) -> (f64, f64) {
    ((area.right() - margin - w).max(area.x), (area.bottom() - margin - h).max(area.y))
}

/// Which side of the sphere the hub opens on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum HubSide {
    Left,
    Right,
}

/// Window geometry for the open hub (all in the unit of the inputs).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HubGeometry {
    /// The window rect (bounding box of the sphere box and the flyout).
    pub window: Rect,
    /// Sphere box top-left relative to the window.
    pub orb: (f64, f64),
    /// Flyout top-left relative to the window.
    pub hub: (f64, f64),
    /// Side the flyout opens on.
    pub side: HubSide,
}

/// Places the hub flyout next to the sphere box `orb` inside `area`: on the
/// right when it fits, else on the left when that fits, else on the roomier
/// side clamped to the area; vertically centred on the sphere and clamped.
/// The sphere box keeps its screen position.
#[must_use]
pub fn hub_layout(orb: Rect, area: Rect, hub: (f64, f64), gap: f64) -> HubGeometry {
    let (hw, hh) = hub;
    let room_right = area.right() - (orb.right() + gap);
    let room_left = (orb.x - gap) - area.x;
    let side = if room_right >= hw {
        HubSide::Right
    } else if room_left >= hw || room_left > room_right {
        HubSide::Left
    } else {
        HubSide::Right
    };
    let hx = match side {
        HubSide::Right => (orb.right() + gap).min(area.right() - hw),
        HubSide::Left => (orb.x - gap - hw).max(area.x),
    };
    let hy = (orb.center().1 - hh / 2.0).clamp(area.y, (area.bottom() - hh).max(area.y));
    let left = orb.x.min(hx);
    let top = orb.y.min(hy);
    let right = orb.right().max(hx + hw);
    let bottom = orb.bottom().max(hy + hh);
    HubGeometry {
        window: Rect { x: left, y: top, w: right - left, h: bottom - top },
        orb: (orb.x - left, orb.y - top),
        hub: (hx - left, hy - top),
        side,
    }
}

/// A remembered position (physical px, outer top-left of the sphere box) on a monitor.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbPosition {
    /// Monitor name (`None` = unknown).
    pub monitor: Option<String>,
    pub x: f64,
    pub y: f64,
}

/// A connected monitor (physical px).
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorArea {
    pub name: Option<String>,
    pub work: Rect,
}

/// Where to show the sphere box of physical size `size`: the first remembered
/// position whose monitor is connected and that lies on it, else the default
/// corner of `primary`.
#[must_use]
pub fn resolve_position(
    remembered: &[OrbPosition],
    monitors: &[MonitorArea],
    primary: Option<Rect>,
    size: f64,
    margin: f64,
) -> (f64, f64) {
    for p in remembered {
        let on = monitors.iter().find(|m| match &p.monitor {
            Some(name) => m.name.as_deref() == Some(name.as_str()),
            None => true,
        });
        if let Some(m) = on {
            if m.work.contains(p.x + size / 2.0, p.y + size / 2.0) {
                return (p.x, p.y);
            }
        }
    }
    let area =
        primary.or_else(|| monitors.first().map(|m| m.work)).unwrap_or(Rect { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 });
    default_position(area, size, size, margin)
}

#[derive(Debug)]
struct OrbWin {
    shape: HitShape,
    /// Window top-left (physical).
    pos: (f64, f64),
    /// Window size (physical).
    size: (f64, f64),
    /// Sphere box top-left relative to the window (physical).
    orb_offset: (f64, f64),
    /// Sphere box edge (logical px).
    box_size: f64,
    scale: f64,
    expanded: bool,
    inside: bool,
    last_user_move: Option<Instant>,
    expected_pos: Option<(i32, i32)>,
}

impl OrbWin {
    fn orb_rect(&self) -> Rect {
        let edge = self.box_size * self.scale;
        Rect { x: self.pos.0 + self.orb_offset.0, y: self.pos.1 + self.orb_offset.1, w: edge, h: edge }
    }
}

#[derive(Debug, Default)]
struct Registry {
    orb: Option<OrbWin>,
    poller: bool,
}

/// Shared state of the orb window (managed by Tauri).
#[derive(Debug, Default)]
pub struct OrbState {
    inner: std::sync::Mutex<Registry>,
    wake: Arc<Notify>,
}

fn lock(state: &OrbState) -> Result<std::sync::MutexGuard<'_, Registry>, String> {
    state.inner.lock().map_err(|_| "orb state poisoned".to_owned())
}

fn monitor_rect(m: &tauri::Monitor) -> Rect {
    let a = m.work_area();
    Rect {
        x: f64::from(a.position.x),
        y: f64::from(a.position.y),
        w: f64::from(a.size.width),
        h: f64::from(a.size.height),
    }
}

fn monitors(app: &AppHandle) -> Vec<MonitorArea> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| MonitorArea { name: m.name().cloned(), work: monitor_rect(m) })
        .collect()
}

fn work_areas(app: &AppHandle) -> Vec<Rect> {
    monitors(app).into_iter().map(|m| m.work).collect()
}

/// Arguments of [`orb_show`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbShowArgs {
    /// Edge of the sphere box in logical px (sphere + glow / badge room).
    pub box_size: f64,
    /// Remembered positions, most recently used first.
    #[serde(default)]
    pub positions: Vec<OrbPosition>,
}

fn clamp_box(v: f64) -> f64 {
    if v.is_finite() {
        v.clamp(48.0, 160.0)
    } else {
        88.0
    }
}

fn build(app: &AppHandle, box_size: f64) -> Result<WebviewWindow, String> {
    // `#/orb` keeps the URL distinct from the main window (DevTools targets, smoke scripts).
    WebviewWindowBuilder::new(app, ORB_WINDOW, WebviewUrl::App("index.html#/orb".into()))
        .title("Orkestr")
        .additional_browser_args(crate::webview_policy::BROWSER_ARGS)
        .transparent(true)
        .decorations(false)
        .shadow(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .resizable(false)
        .maximizable(false)
        .minimizable(false)
        .focused(false)
        .visible_on_all_workspaces(true)
        .inner_size(box_size, box_size)
        .position(0.0, 0.0)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())
}

// The commands are `async` on purpose: synchronous commands run on the main
// thread, and creating a webview window there deadlocks on Windows (WebView2).

/// Shows the orb (creating its window) at the remembered position, or
/// resizes the existing sphere box. Main window only.
#[tauri::command]
pub async fn orb_show(app: AppHandle, state: tauri::State<'_, OrbState>, args: OrbShowArgs) -> Result<(), String> {
    let box_size = clamp_box(args.box_size);
    if let Some(window) = app.get_webview_window(ORB_WINDOW) {
        // Already there: apply a new size (collapsed only — the hub relayouts itself).
        let collapsed = {
            let mut reg = lock(&state)?;
            match reg.orb.as_mut() {
                Some(o) => {
                    o.box_size = box_size;
                    !o.expanded
                }
                None => true,
            }
        };
        if collapsed {
            let _ = window.set_size(LogicalSize::new(box_size, box_size));
            if let Ok(mut reg) = lock(&state) {
                if let Some(o) = reg.orb.as_mut() {
                    o.size = (box_size * o.scale, box_size * o.scale);
                }
            }
        }
        let _ = window.show();
        let _ = window.set_always_on_top(true);
        return Ok(());
    }
    let window = build(&app, box_size)?;
    let scale = window.scale_factor().unwrap_or(1.0);
    let primary = app.primary_monitor().ok().flatten().map(|m| monitor_rect(&m));
    let edge = box_size * scale;
    let (x, y) = resolve_position(&args.positions, &monitors(&app), primary, edge, DEFAULT_MARGIN * scale);
    let target = (x.round() as i32, y.round() as i32);
    let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
    let _ = window.set_ignore_cursor_events(true);
    {
        let mut reg = lock(&state)?;
        reg.orb = Some(OrbWin {
            shape: HitShape::default(),
            pos: (f64::from(target.0), f64::from(target.1)),
            size: (edge, edge),
            orb_offset: (0.0, 0.0),
            box_size,
            scale,
            expanded: false,
            inside: false,
            last_user_move: None,
            expected_pos: Some(target),
        });
        if !reg.poller {
            reg.poller = true;
            spawn_poller(app.clone());
        }
    }
    let _ = window.show();
    state.wake.notify_one();
    tracing::info!("orb shown");
    Ok(())
}

/// Hides and closes the orb window. Main window only.
#[tauri::command]
pub async fn orb_hide(app: AppHandle, state: tauri::State<'_, OrbState>) -> Result<(), String> {
    lock(&state)?.orb = None;
    if let Some(w) = app.get_webview_window(ORB_WINDOW) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Toggles the Agent Hub (hotkey in the main window): focuses the orb window
/// so the hub is keyboard-ready and tells it to open / close.
#[tauri::command]
pub async fn orb_toggle_hub(app: AppHandle) -> Result<bool, String> {
    let Some(w) = app.get_webview_window(ORB_WINDOW) else { return Ok(false) };
    let _ = w.set_ignore_cursor_events(false);
    let _ = w.set_focus();
    app.emit_to(ORB_WINDOW, EV_HUB, ()).map_err(|e| e.to_string())?;
    Ok(true)
}

/// Moves the orb back to the default corner of the primary monitor. Main window only.
#[tauri::command]
pub async fn orb_reset_position(app: AppHandle, state: tauri::State<'_, OrbState>) -> Result<(), String> {
    let Some(window) = app.get_webview_window(ORB_WINDOW) else { return Ok(()) };
    let (rect, offset, scale) = {
        let reg = lock(&state)?;
        let Some(o) = reg.orb.as_ref() else { return Ok(()) };
        (o.orb_rect(), o.orb_offset, o.scale)
    };
    let primary = app.primary_monitor().ok().flatten().map(|m| monitor_rect(&m));
    let (x, y) = resolve_position(&[], &monitors(&app), primary, rect.w, DEFAULT_MARGIN * scale);
    move_window(&app, &window, x - offset.0, y - offset.1);
    emit_moved(&app, &window, x, y);
    Ok(())
}

/// Orb webview → shell: its interactive shape.
#[tauri::command]
pub fn orb_set_shape(window: WebviewWindow, state: tauri::State<'_, OrbState>, shape: HitShape) -> Result<(), String> {
    if window.label() != ORB_WINDOW {
        return Err("not the orb window".to_owned());
    }
    if shape.circles.len() + shape.rects.len() > 16 {
        return Err("hit shape too complex".to_owned());
    }
    if let Some(o) = lock(&state)?.orb.as_mut() {
        o.shape = shape;
    }
    Ok(())
}

/// Layout of the orb window, in window-local CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OrbLayout {
    pub orb_x: f64,
    pub orb_y: f64,
    pub hub_x: f64,
    pub hub_y: f64,
    pub side: HubSide,
}

/// The sphere box grown about its centre to `edge` (the Jarvis circle while a
/// voice command runs); never smaller than the box itself.
#[must_use]
pub fn grown_rect(orb: Rect, edge: f64) -> Rect {
    let e = edge.max(orb.w);
    let (cx, cy) = orb.center();
    Rect { x: cx - e / 2.0, y: cy - e / 2.0, w: e, h: e }
}

/// `r` moved (not resized) to lie inside `area` where it fits.
#[must_use]
pub fn keep_inside(r: Rect, area: Rect) -> Rect {
    let x = r.x.clamp(area.x, (area.right() - r.w).max(area.x));
    let y = r.y.clamp(area.y, (area.bottom() - r.h).max(area.y));
    Rect { x, y, ..r }
}

/// Orb webview → shell: opens (`hub = Some([w, h])`, logical px) or closes the
/// hub area. The sphere keeps its screen position; returns where the webview
/// must draw the sphere and the flyout. `grow` (logical px) reserves room for
/// the orb growing about its centre (the voice circle): the flyout is placed
/// next to the grown circle and the window holds all of it.
#[tauri::command]
pub async fn orb_set_layout(
    app: AppHandle,
    window: WebviewWindow,
    state: tauri::State<'_, OrbState>,
    hub: Option<(f64, f64)>,
    grow: Option<f64>,
) -> Result<OrbLayout, String> {
    if window.label() != ORB_WINDOW {
        return Err("not the orb window".to_owned());
    }
    let (rect, scale) = {
        let reg = lock(&state)?;
        let o = reg.orb.as_ref().ok_or("the orb is not shown")?;
        (o.orb_rect(), o.scale)
    };
    // `draw_off`: where the webview centres the circle (the grown circle is
    // kept on screen, so near an edge it shifts a little off the sphere box).
    let (win, orb_off, draw_off, hub_off, side) = match hub {
        Some((w, h)) => {
            let w = if w.is_finite() { w.clamp(200.0, 640.0) } else { 360.0 };
            let h = if h.is_finite() { h.clamp(160.0, 900.0) } else { 520.0 };
            let area = area_of(rect, &work_areas(&app)).unwrap_or(rect);
            let edge = grow.filter(|g| g.is_finite()).map_or(0.0, |g| g.clamp(0.0, 240.0) * scale);
            let big = keep_inside(grown_rect(rect, edge), area);
            let g = hub_layout(big, area, (w * scale, h * scale), HUB_GAP * scale);
            // The sphere box inside the window, and the box centred in the grown circle.
            let orb_off = (g.orb.0 + (rect.x - big.x), g.orb.1 + (rect.y - big.y));
            let draw_off = (g.orb.0 + (big.w - rect.w) / 2.0, g.orb.1 + (big.h - rect.h) / 2.0);
            (g.window, orb_off, draw_off, g.hub, g.side)
        }
        None => (rect, (0.0, 0.0), (0.0, 0.0), (0.0, 0.0), HubSide::Right),
    };
    {
        let mut reg = lock(&state)?;
        if let Some(o) = reg.orb.as_mut() {
            o.expanded = hub.is_some();
            o.orb_offset = orb_off;
            o.size = (win.w, win.h);
            o.last_user_move = None;
        }
    }
    move_window(&app, &window, win.x, win.y);
    let _ = window.set_size(PhysicalSize::new(win.w.round() as u32, win.h.round() as u32));
    Ok(OrbLayout {
        orb_x: draw_off.0 / scale,
        orb_y: draw_off.1 / scale,
        hub_x: hub_off.0 / scale,
        hub_y: hub_off.1 / scale,
        side,
    })
}

/// Requests the orb window may forward to the main window.
const MAIN_REQUESTS: &[&str] =
    &["open-main", "open-agent", "open-claude", "open-settings", "open-approvals", "open-cursor"];

/// Orb webview → main window request (`open-main`, `open-agent`,
/// `open-claude`, `open-settings`, `open-approvals`). Every request brings the
/// main window forward.
#[tauri::command]
pub fn orb_to_main(
    app: AppHandle,
    window: WebviewWindow,
    request: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    if window.label() != ORB_WINDOW {
        return Err("not the orb window".to_owned());
    }
    if !MAIN_REQUESTS.contains(&request.as_str()) {
        return Err("unknown orb request".to_owned());
    }
    crate::show_main(&app);
    app.emit_to(crate::MAIN_WINDOW, &format!("{EV_MAIN_PREFIX}{request}"), payload).map_err(|e| e.to_string())
}

/// Window events of the orb window (called from the app's `on_window_event`).
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    if window.label() != ORB_WINDOW {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<OrbState>();
    let Ok(mut reg) = state.inner.lock() else { return };
    match event {
        tauri::WindowEvent::Moved(p) => {
            let Some(o) = reg.orb.as_mut() else { return };
            o.pos = (f64::from(p.x), f64::from(p.y));
            if o.expected_pos == Some((p.x, p.y)) {
                o.expected_pos = None;
                return;
            }
            // A user drag (the sphere, or the Jarvis voice card that moves the
            // orb along with it): snap and remember the position when it ends.
            if o.inside {
                if o.last_user_move.is_none() {
                    let _ = app.emit_to(ORB_WINDOW, EV_DRAG, true);
                }
                o.last_user_move = Some(Instant::now());
            }
        }
        tauri::WindowEvent::ScaleFactorChanged { scale_factor, new_inner_size, .. } => {
            if let Some(o) = reg.orb.as_mut() {
                let ratio = if o.scale > 0.0 { *scale_factor / o.scale } else { 1.0 };
                o.orb_offset = (o.orb_offset.0 * ratio, o.orb_offset.1 * ratio);
                o.scale = *scale_factor;
                o.size = (f64::from(new_inner_size.width), f64::from(new_inner_size.height));
            }
        }
        tauri::WindowEvent::Destroyed => {
            reg.orb = None;
        }
        _ => {}
    }
}

fn spawn_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let wake = Arc::clone(&app.state::<OrbState>().wake);
        loop {
            let present = app.state::<OrbState>().inner.lock().map(|r| r.orb.is_some()).unwrap_or(false);
            if !present {
                wake.notified().await;
                continue;
            }
            tokio::time::sleep(POLL).await;
            let Ok(cursor) = app.cursor_position() else { continue };
            let now = Instant::now();
            let (hover, drag_end) = {
                let state = app.state::<OrbState>();
                let Ok(mut reg) = state.inner.lock() else { return };
                let Some(o) = reg.orb.as_mut() else { continue };
                let lx = (cursor.x - o.pos.0) / o.scale;
                let ly = (cursor.y - o.pos.1) / o.scale;
                // While dragging the cursor is on the orb by definition.
                let inside = o.last_user_move.is_some() || o.shape.contains(lx, ly);
                let hover = (inside != o.inside).then_some(inside);
                o.inside = inside;
                let drag_end = o.last_user_move.is_some_and(|t| now.duration_since(t) >= DRAG_END_QUIET);
                if drag_end {
                    o.last_user_move = None;
                }
                (hover, drag_end)
            };
            if let Some(inside) = hover {
                if let Some(w) = app.get_webview_window(ORB_WINDOW) {
                    let _ = w.set_ignore_cursor_events(!inside);
                    let _ = app.emit_to(ORB_WINDOW, EV_HOVER, inside);
                }
            }
            if drag_end {
                finish_drag(&app);
            }
        }
    });
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct MovedPayload {
    x: f64,
    y: f64,
    monitor: Option<String>,
}

fn finish_drag(app: &AppHandle) {
    let Some(window) = app.get_webview_window(ORB_WINDOW) else { return };
    let _ = app.emit_to(ORB_WINDOW, EV_DRAG, false);
    let (rect, offset, scale) = {
        let state = app.state::<OrbState>();
        let Ok(reg) = state.inner.lock() else { return };
        let Some(o) = reg.orb.as_ref() else { return };
        (o.orb_rect(), o.orb_offset, o.scale)
    };
    let (x, y) = snap_rect(rect, &work_areas(app), SNAP_THRESHOLD * scale);
    move_window(app, &window, x - offset.0, y - offset.1);
    emit_moved(app, &window, x, y);
}

fn move_window(app: &AppHandle, window: &WebviewWindow, x: f64, y: f64) {
    let target = (x.round() as i32, y.round() as i32);
    let current = window.outer_position().ok().map(|p| (p.x, p.y));
    if current == Some(target) {
        return;
    }
    if let Ok(mut reg) = app.state::<OrbState>().inner.lock() {
        if let Some(o) = reg.orb.as_mut() {
            o.expected_pos = Some(target);
            o.pos = (f64::from(target.0), f64::from(target.1));
        }
    }
    let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
}

fn emit_moved(app: &AppHandle, window: &WebviewWindow, x: f64, y: f64) {
    let monitor = window.current_monitor().ok().flatten().and_then(|m| m.name().cloned());
    let _ = app.emit_to(ORB_WINDOW, EV_MOVED, MovedPayload { x: x.round(), y: y.round(), monitor });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };

    #[test]
    fn hit_shape_circle_and_rect() {
        let shape = HitShape {
            circles: vec![Circle { cx: 44.0, cy: 44.0, r: 32.0 }],
            rects: vec![Rect { x: 100.0, y: 0.0, w: 360.0, h: 500.0 }],
        };
        assert!(shape.contains(44.0, 44.0));
        assert!(shape.contains(44.0 + 31.0, 44.0));
        assert!(!shape.contains(44.0 + 33.0, 44.0));
        assert!(!shape.contains(2.0, 2.0), "corners of the sphere box pass clicks through");
        assert!(shape.contains(300.0, 250.0), "hub flyout");
        assert!(!HitShape::default().contains(1.0, 1.0));
        assert!(!HitShape { circles: vec![Circle { cx: 0.0, cy: 0.0, r: 0.0 }], rects: vec![] }.contains(0.0, 0.0));
    }

    #[test]
    fn snaps_to_edges_and_corners_and_stays_on_screen() {
        let near_corner = Rect { x: 1920.0 - 88.0 - 20.0, y: 1040.0 - 88.0 - 9.0, w: 88.0, h: 88.0 };
        assert_eq!(snap_rect(near_corner, &[SCREEN], 28.0), (1832.0, 952.0));
        let middle = Rect { x: 600.0, y: 400.0, w: 88.0, h: 88.0 };
        assert_eq!(snap_rect(middle, &[SCREEN], 28.0), (600.0, 400.0), "nothing within the threshold");
        let left = Rect { x: 12.0, y: 400.0, w: 88.0, h: 88.0 };
        assert_eq!(snap_rect(left, &[SCREEN], 28.0), (0.0, 400.0));
        let off = Rect { x: 1900.0, y: 1030.0, w: 88.0, h: 88.0 };
        assert_eq!(snap_rect(off, &[SCREEN], 28.0), (1832.0, 952.0), "clamped back on screen");
        let second = Rect { x: 1920.0, y: 0.0, w: 1280.0, h: 1024.0 };
        let on_second = Rect { x: 1930.0, y: 300.0, w: 88.0, h: 88.0 };
        assert_eq!(snap_rect(on_second, &[SCREEN, second], 28.0), (1920.0, 300.0), "uses the orb's monitor");
        assert_eq!(snap_rect(middle, &[], 28.0), (600.0, 400.0));
    }

    #[test]
    fn hub_opens_right_when_it_fits_else_left_and_stays_on_screen() {
        let hub = (360.0, 520.0);
        // Room on the right.
        let orb = Rect { x: 100.0, y: 300.0, w: 88.0, h: 88.0 };
        let g = hub_layout(orb, SCREEN, hub, 8.0);
        assert_eq!(g.side, HubSide::Right);
        assert_eq!(g.window.x, 100.0, "the sphere does not move");
        assert_eq!(g.orb.0, 0.0);
        assert_eq!(g.hub.0, 88.0 + 8.0);
        assert_eq!(g.window.w, 88.0 + 8.0 + 360.0);
        // The sphere's screen position is preserved.
        assert_eq!(g.window.x + g.orb.0, orb.x);
        assert_eq!(g.window.y + g.orb.1, orb.y);
        // Docked bottom-right: the hub opens left and is clamped above the taskbar.
        let corner = Rect { x: 1832.0, y: 952.0, w: 88.0, h: 88.0 };
        let g = hub_layout(corner, SCREEN, hub, 8.0);
        assert_eq!(g.side, HubSide::Left);
        assert_eq!(g.window.x + g.hub.0, 1832.0 - 8.0 - 360.0);
        assert_eq!(g.window.y + g.hub.1 + 520.0, 1040.0, "flyout bottom on the work-area edge");
        assert_eq!(g.window.x + g.orb.0, corner.x);
        assert_eq!(g.window.y + g.orb.1, corner.y);
        assert!(g.window.y >= 0.0 && g.window.y + g.window.h <= 1040.0);
        // Top-left corner: opens right, top clamped to the area.
        let top = Rect { x: 0.0, y: 0.0, w: 88.0, h: 88.0 };
        let g = hub_layout(top, SCREEN, hub, 8.0);
        assert_eq!(g.side, HubSide::Right);
        assert_eq!(g.window.y, 0.0);
        // A screen too narrow on both sides: roomier side, clamped inside.
        let narrow = Rect { x: 0.0, y: 0.0, w: 500.0, h: 800.0 };
        let g = hub_layout(Rect { x: 300.0, y: 300.0, w: 88.0, h: 88.0 }, narrow, hub, 8.0);
        assert_eq!(g.side, HubSide::Left);
        assert!(g.window.x + g.hub.0 >= 0.0);
    }

    #[test]
    fn remembered_positions_are_used_per_monitor_else_the_default_corner() {
        let monitors = vec![
            MonitorArea { name: Some("A".into()), work: SCREEN },
            MonitorArea { name: Some("B".into()), work: Rect { x: 1920.0, y: 0.0, w: 1280.0, h: 1024.0 } },
        ];
        let on_b = OrbPosition { monitor: Some("B".into()), x: 2000.0, y: 100.0 };
        let on_c = OrbPosition { monitor: Some("C".into()), x: 4000.0, y: 100.0 };
        assert_eq!(
            resolve_position(&[on_c.clone(), on_b.clone()], &monitors, Some(SCREEN), 88.0, 24.0),
            (2000.0, 100.0)
        );
        // The monitor is gone: default corner of the primary monitor.
        assert_eq!(
            resolve_position(&[on_c], &monitors, Some(SCREEN), 88.0, 24.0),
            (1920.0 - 24.0 - 88.0, 1040.0 - 24.0 - 88.0)
        );
        // A remembered position that is not on its monitor any more (resolution change).
        let stale = OrbPosition { monitor: Some("A".into()), x: 5000.0, y: 5000.0 };
        assert_eq!(resolve_position(&[stale], &monitors, Some(SCREEN), 88.0, 24.0), (1808.0, 928.0));
        assert_eq!(resolve_position(&[], &[], None, 88.0, 24.0), (1280.0 - 24.0 - 88.0, 720.0 - 24.0 - 88.0));
    }

    #[test]
    fn grown_rect_is_centred_and_never_smaller() {
        let orb = Rect { x: 100.0, y: 100.0, w: 88.0, h: 88.0 };
        let g = grown_rect(orb, 150.0);
        assert_eq!(g.center(), orb.center());
        assert!((g.w - 150.0).abs() < 1e-9);
        assert_eq!(grown_rect(orb, 10.0), orb);
        // Near the screen edge the grown circle shifts back on screen.
        let screen = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let corner = Rect { x: 1808.0, y: 968.0, w: 88.0, h: 88.0 };
        let k = keep_inside(grown_rect(corner, 150.0), screen);
        assert!(k.x + k.w <= 1920.0 && k.y + k.h <= 1080.0);
        // The flyout goes next to the grown circle, and the window holds both.
        let area = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1080.0 };
        let h = hub_layout(g, area, (300.0, 176.0), 8.0);
        assert!(h.window.x <= g.x && h.window.x + h.window.w >= g.x + g.w);
        assert!(h.hub.0 >= g.w + 8.0 - 1e-9);
    }

    #[test]
    fn box_size_is_clamped() {
        assert_eq!(clamp_box(10.0), 48.0);
        assert_eq!(clamp_box(1000.0), 160.0);
        assert_eq!(clamp_box(f64::NAN), 88.0);
        assert_eq!(clamp_box(88.0), 88.0);
    }
}
