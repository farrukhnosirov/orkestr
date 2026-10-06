//! Window management for detached panels and multi-monitor layouts
//! (§28 "detachable panels / floating windows", §34 multi-monitor, ADR-0013).
//!
//! Only window geometry flows through these commands — never business data
//! (§67): a detached panel window loads the same frontend at
//! `index.html#/panel/<panelId>?projectId=…` and talks to the core over RPC
//! like the main window.
//!
//! * Monitors are identified by a **monitor key**: the OS monitor name when
//!   it has one, otherwise a geometry key (`@x,y,wxh`).
//! * Window bounds in layouts are stored **relative to the monitor's work
//!   area**, so a layout survives monitors being rearranged.
//! * When the monitor a window was assigned to is gone (unplugged laptop
//!   dock), placement falls back to the primary monitor and the bounds are
//!   clamped into its work area ([`resolve_placement`], `fellBack: true`).
//!
//! Events to the main window: [`EVT_REDOCK`] (a detached panel asks to go
//! back into the dock), [`EVT_PANEL_CLOSED`] (a panel window was destroyed)
//! and [`EVT_WINDOWS_CHANGED`] (a panel window moved/resized/opened).

use std::collections::HashMap;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, PhysicalSize, WebviewUrl, WebviewWindowBuilder};

/// Label prefix of detached panel windows.
pub const PANEL_WINDOW_PREFIX: &str = "panel-";
/// A detached panel asks the main window to re-dock it (payload [`RedockPayload`]).
pub const EVT_REDOCK: &str = "orkestr://layout/redock";
/// A detached panel window was destroyed (payload [`RedockPayload`]).
pub const EVT_PANEL_CLOSED: &str = "orkestr://layout/panel-closed";
/// A detached panel window opened, moved or resized (no payload).
pub const EVT_WINDOWS_CHANGED: &str = "orkestr://layout/windows-changed";
/// Label of the main window.
const MAIN: &str = "main";
/// Smallest detached window.
const MIN_W: u32 = 360;
const MIN_H: u32 = 240;
/// A window must show at least this much of itself on some monitor to count as visible.
const VISIBLE_MIN: u32 = 64;

/// Axis-aligned rectangle in physical pixels.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rect {
    /// Left edge.
    pub x: i32,
    /// Top edge.
    pub y: i32,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

impl Rect {
    fn right(&self) -> i64 {
        i64::from(self.x) + i64::from(self.width)
    }
    fn bottom(&self) -> i64 {
        i64::from(self.y) + i64::from(self.height)
    }
    /// Width and height of the intersection with `other` (0 when disjoint).
    #[must_use]
    pub fn overlap_dims(&self, other: &Rect) -> (u64, u64) {
        let w = (self.right().min(other.right()) - i64::from(self.x.max(other.x))).max(0);
        let h = (self.bottom().min(other.bottom()) - i64::from(self.y.max(other.y))).max(0);
        (u64::try_from(w).unwrap_or(0), u64::try_from(h).unwrap_or(0))
    }
    /// Intersection area with `other` (0 when disjoint).
    #[must_use]
    pub fn overlap(&self, other: &Rect) -> u64 {
        let (w, h) = self.overlap_dims(other);
        w * h
    }
}

/// Geometry of one monitor (what placement needs; testable without Tauri).
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorGeom {
    /// Stable key (name or geometry key).
    pub key: String,
    /// OS name, if any.
    pub name: Option<String>,
    /// Full monitor bounds.
    pub bounds: Rect,
    /// Work area (without task bar / dock).
    pub work_area: Rect,
    /// DPI scale factor.
    pub scale_factor: f64,
    /// Primary monitor.
    pub primary: bool,
}

/// Where a window ends up.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Placement {
    /// Monitor key used.
    pub monitor: String,
    /// Absolute bounds (physical px).
    pub absolute: Rect,
    /// Bounds relative to the monitor's work area (what layouts store).
    pub relative: Rect,
    /// The requested monitor was not connected; the primary one was used.
    pub fell_back: bool,
}

/// Monitor key: the OS name, or `@x,y,wxh` when the OS reports none.
#[must_use]
pub fn monitor_key(name: Option<&str>, bounds: &Rect) -> String {
    match name {
        Some(n) if !n.trim().is_empty() => n.to_owned(),
        _ => format!("@{},{},{}x{}", bounds.x, bounds.y, bounds.width, bounds.height),
    }
}

fn primary_or_first(monitors: &[MonitorGeom]) -> Option<&MonitorGeom> {
    monitors.iter().find(|m| m.primary).or_else(|| monitors.first())
}

/// Resolves where a window goes: on `preferred` if it is connected, else on
/// the primary monitor (`fell_back`). `relative` bounds are clamped into the
/// work area; without them the window is centred at `default_frac` of it.
#[must_use]
pub fn resolve_placement(
    monitors: &[MonitorGeom],
    preferred: Option<&str>,
    relative: Option<Rect>,
    default_frac: f64,
) -> Option<Placement> {
    let wanted = preferred.and_then(|k| monitors.iter().find(|m| m.key == k));
    let fell_back = preferred.is_some() && wanted.is_none();
    let m = wanted.or_else(|| primary_or_first(monitors))?;
    let wa = m.work_area;
    let frac = default_frac.clamp(0.2, 1.0);
    let rel = relative.unwrap_or_else(|| {
        // Truncation is intended: a fraction of a pixel count.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (w, h) = ((f64::from(wa.width) * frac) as u32, (f64::from(wa.height) * frac) as u32);
        let x = i32::try_from((wa.width.saturating_sub(w)) / 2).unwrap_or(0);
        let y = i32::try_from((wa.height.saturating_sub(h)) / 2).unwrap_or(0);
        Rect { x, y, width: w, height: h }
    });
    let width = rel.width.clamp(MIN_W.min(wa.width), wa.width.max(1));
    let height = rel.height.clamp(MIN_H.min(wa.height), wa.height.max(1));
    let max_x = i32::try_from(wa.width - width).unwrap_or(0);
    let max_y = i32::try_from(wa.height - height).unwrap_or(0);
    let relative = Rect { x: rel.x.clamp(0, max_x), y: rel.y.clamp(0, max_y), width, height };
    let absolute = Rect { x: wa.x + relative.x, y: wa.y + relative.y, width, height };
    Some(Placement { monitor: m.key.clone(), absolute, relative, fell_back })
}

/// The monitor showing most of `rect`, if any part of it is on screen.
#[must_use]
pub fn monitor_for_rect<'a>(monitors: &'a [MonitorGeom], rect: &Rect) -> Option<&'a MonitorGeom> {
    monitors.iter().map(|m| (m, m.bounds.overlap(rect))).filter(|(_, a)| *a > 0).max_by_key(|(_, a)| *a).map(|(m, _)| m)
}

/// Whether enough of `rect` (at least 64×64 px) is visible on some monitor to grab it.
#[must_use]
pub fn is_reachable(monitors: &[MonitorGeom], rect: &Rect) -> bool {
    let min = u64::from(VISIBLE_MIN);
    monitors.iter().any(|m| {
        let (w, h) = m.work_area.overlap_dims(rect);
        w >= min && h >= min
    })
}

/// `rect` relative to a monitor's work area.
#[must_use]
pub fn relative_to(m: &MonitorGeom, rect: &Rect) -> Rect {
    Rect { x: rect.x - m.work_area.x, y: rect.y - m.work_area.y, width: rect.width, height: rect.height }
}

/// Monitor info for the webview (Layout Editor monitor map).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MonitorInfo {
    #[serde(flatten)]
    geom: MonitorGeom,
    /// 1-based display number in left-to-right order.
    index: usize,
}

/// A detached panel window (or the main window) and its placement.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WindowInfo {
    /// Window label.
    pub label: String,
    /// Panel id (`None` for the main window).
    pub panel_id: Option<String>,
    /// Project the panel window is bound to.
    pub project_id: Option<String>,
    /// Monitor key the window is (mostly) on.
    pub monitor: Option<String>,
    /// Bounds relative to that monitor's work area.
    pub bounds: Option<Rect>,
    /// Maximized.
    pub maximized: bool,
    /// Placement fell back to the primary monitor (only on open/move).
    pub fell_back: bool,
}

/// Request to open (or focus) a detached panel window.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPanelRequest {
    /// Panel registry id.
    pub panel_id: String,
    /// Project to bind the window to.
    pub project_id: Option<String>,
    /// Window title (localized by the webview).
    pub title: Option<String>,
    /// Monitor key to open on.
    pub monitor: Option<String>,
    /// Bounds relative to that monitor's work area.
    pub bounds: Option<Rect>,
    /// Open maximized.
    #[serde(default)]
    pub maximized: bool,
}

/// Payload of [`EVT_REDOCK`] / [`EVT_PANEL_CLOSED`].
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RedockPayload {
    /// Window label.
    pub label: String,
    /// Panel id.
    pub panel_id: String,
    /// Project id the window was bound to.
    pub project_id: Option<String>,
}

#[derive(Debug, Clone)]
struct PanelMeta {
    panel_id: String,
    project_id: Option<String>,
}

/// Registry of open panel windows (label → panel/project).
#[derive(Debug, Default)]
pub struct PanelWindows {
    open: Mutex<HashMap<String, PanelMeta>>,
}

fn is_id(s: &str, max: usize, extra: &[char]) -> bool {
    !s.is_empty() && s.len() <= max && s.chars().all(|c| c.is_ascii_alphanumeric() || extra.contains(&c))
}

/// Label of a panel window (one window per panel).
#[must_use]
pub fn panel_label(panel_id: &str) -> String {
    format!("{PANEL_WINDOW_PREFIX}{panel_id}")
}

/// Hash route a panel window loads (`/panel/<id>[?projectId=<id>]`); ids are
/// validated by the caller.
#[must_use]
pub fn panel_route(panel_id: &str, project_id: Option<&str>) -> String {
    match project_id {
        Some(p) => format!("/panel/{panel_id}?projectId={p}"),
        None => format!("/panel/{panel_id}"),
    }
}

/// Whether re-opening an existing panel window must re-point its webview at
/// another project (the window keeps the project of the route it loaded).
fn needs_rebind(previous: Option<&PanelMeta>, requested: &PanelMeta) -> bool {
    previous.is_none_or(|p| p.project_id != requested.project_id)
}

fn to_rect(pos: PhysicalPosition<i32>, size: PhysicalSize<u32>) -> Rect {
    Rect { x: pos.x, y: pos.y, width: size.width, height: size.height }
}

/// Connected monitors with keys, sorted left-to-right.
fn monitors(app: &AppHandle) -> Vec<MonitorGeom> {
    let primary = app.primary_monitor().ok().flatten().map(|m| to_rect(*m.position(), *m.size()));
    let mut list: Vec<MonitorGeom> = app
        .available_monitors()
        .unwrap_or_default()
        .into_iter()
        .map(|m| {
            let bounds = to_rect(*m.position(), *m.size());
            let wa = m.work_area();
            let work_area = to_rect(wa.position, wa.size);
            MonitorGeom {
                key: monitor_key(m.name().map(String::as_str), &bounds),
                name: m.name().cloned(),
                bounds,
                work_area: if work_area.width == 0 || work_area.height == 0 { bounds } else { work_area },
                scale_factor: m.scale_factor(),
                primary: primary == Some(bounds),
            }
        })
        .collect();
    list.sort_by_key(|m| (m.bounds.x, m.bounds.y));
    if !list.iter().any(|m| m.primary) {
        if let Some(first) = list.first_mut() {
            first.primary = true;
        }
    }
    list
}

fn window_info(app: &AppHandle, label: &str, meta: Option<&PanelMeta>) -> Option<WindowInfo> {
    let w = app.get_webview_window(label)?;
    let rect = to_rect(w.outer_position().ok()?, w.outer_size().ok()?);
    let mons = monitors(app);
    let m = monitor_for_rect(&mons, &rect);
    Some(WindowInfo {
        label: label.to_owned(),
        panel_id: meta.map(|m| m.panel_id.clone()),
        project_id: meta.and_then(|m| m.project_id.clone()),
        monitor: m.map(|m| m.key.clone()),
        bounds: m.map(|m| relative_to(m, &rect)),
        maximized: w.is_maximized().unwrap_or(false),
        fell_back: false,
    })
}

fn place(w: &tauri::WebviewWindow, p: &Placement, maximized: bool) -> Result<(), String> {
    if w.is_maximized().unwrap_or(false) {
        w.unmaximize().map_err(|e| e.to_string())?;
    }
    w.set_position(PhysicalPosition::new(p.absolute.x, p.absolute.y)).map_err(|e| e.to_string())?;
    w.set_size(PhysicalSize::new(p.absolute.width, p.absolute.height)).map_err(|e| e.to_string())?;
    if maximized {
        w.maximize().map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn notify_changed(app: &AppHandle) {
    let _ = app.emit_to(MAIN, EVT_WINDOWS_CHANGED, ());
}

/// Lists connected monitors (left-to-right, 1-based `index`).
#[tauri::command]
pub fn window_list_monitors(app: AppHandle) -> Vec<MonitorInfo> {
    monitors(&app).into_iter().enumerate().map(|(i, geom)| MonitorInfo { geom, index: i + 1 }).collect()
}

/// Opens (or focuses) the detached window of a panel on the requested
/// monitor (primary when that monitor is gone).
#[tauri::command]
pub async fn window_open_panel(
    app: AppHandle,
    req: OpenPanelRequest,
    state: tauri::State<'_, PanelWindows>,
) -> Result<WindowInfo, String> {
    if !is_id(&req.panel_id, 48, &['-', '_']) {
        return Err("invalid panel id".into());
    }
    if let Some(p) = &req.project_id {
        if !is_id(p, 64, &['-']) {
            return Err("invalid project id".into());
        }
    }
    let label = panel_label(&req.panel_id);
    let meta = PanelMeta { panel_id: req.panel_id.clone(), project_id: req.project_id.clone() };
    let mons = monitors(&app);
    let placement = resolve_placement(&mons, req.monitor.as_deref(), req.bounds, 0.6);
    if let Some(existing) = app.get_webview_window(&label) {
        if let (Some(p), true) = (&placement, req.monitor.is_some()) {
            place(&existing, p, req.maximized)?;
        }
        let previous = state.open.lock().map_err(|_| "window state poisoned".to_owned())?.get(&label).cloned();
        if needs_rebind(previous.as_ref(), &meta) {
            // The webview reads its project from the route once at load: point it
            // at the new project and reload, so the window shows what the registry
            // (and the saved layout) says it shows.
            let route = serde_json::to_string(&format!("#{}", panel_route(&req.panel_id, req.project_id.as_deref())))
                .map_err(|e| e.to_string())?;
            existing
                .eval(format!("window.location.replace({route});window.location.reload();"))
                .map_err(|e| e.to_string())?;
            tracing::info!(panel = %req.panel_id, project = ?req.project_id, "panel window rebound to another project");
        }
        let _ = existing.show();
        let _ = existing.unminimize();
        let _ = existing.set_focus();
    } else {
        let url = format!("index.html#{}", panel_route(&req.panel_id, req.project_id.as_deref()));
        let title = req.title.clone().unwrap_or_else(|| "Orkestr".to_owned());
        let w = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(url.into()))
            .title(title.chars().take(120).collect::<String>())
            .decorations(false)
            .transparent(true)
            .shadow(true)
            .resizable(true)
            .min_inner_size(f64::from(MIN_W), f64::from(MIN_H))
            .visible(false)
            .disable_drag_drop_handler()
            .build()
            .map_err(|e| e.to_string())?;
        if let Some(p) = &placement {
            place(&w, p, req.maximized)?;
        }
        w.show().map_err(|e| e.to_string())?;
        let _ = w.set_focus();
    }
    state.open.lock().map_err(|_| "window state poisoned".to_owned())?.insert(label.clone(), meta.clone());
    notify_changed(&app);
    let mut info = window_info(&app, &label, Some(&meta)).ok_or("window vanished")?;
    info.fell_back = placement.is_some_and(|p| p.fell_back);
    tracing::info!(panel = %req.panel_id, monitor = ?info.monitor, fell_back = info.fell_back, "panel window opened");
    Ok(info)
}

/// Closes a detached panel window (no re-dock).
#[tauri::command]
pub fn window_close_panel(app: AppHandle, label: String) -> Result<(), String> {
    if !label.starts_with(PANEL_WINDOW_PREFIX) {
        return Err("not a panel window".into());
    }
    if let Some(w) = app.get_webview_window(&label) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Called by a detached panel window: asks the main window to re-dock the
/// panel, then closes the calling window.
#[tauri::command]
pub fn window_redock_panel(
    app: AppHandle,
    window: tauri::WebviewWindow,
    state: tauri::State<'_, PanelWindows>,
) -> Result<(), String> {
    let label = window.label().to_owned();
    if !label.starts_with(PANEL_WINDOW_PREFIX) {
        return Err("not a panel window".into());
    }
    let meta = state.open.lock().map_err(|_| "window state poisoned".to_owned())?.remove(&label);
    let panel_id = meta.as_ref().map_or_else(|| label[PANEL_WINDOW_PREFIX.len()..].to_owned(), |m| m.panel_id.clone());
    let payload = RedockPayload { label, panel_id, project_id: meta.and_then(|m| m.project_id) };
    app.emit_to(MAIN, EVT_REDOCK, payload).map_err(|e| e.to_string())?;
    window.close().map_err(|e| e.to_string())
}

/// Open panel windows and their placement (for saving the arrangement).
#[tauri::command]
pub fn window_list_panels(app: AppHandle, state: tauri::State<'_, PanelWindows>) -> Result<Vec<WindowInfo>, String> {
    let open = state.open.lock().map_err(|_| "window state poisoned".to_owned())?.clone();
    let mut list: Vec<WindowInfo> =
        open.iter().filter_map(|(label, meta)| window_info(&app, label, Some(meta))).collect();
    list.sort_by(|a, b| a.label.cmp(&b.label));
    Ok(list)
}

/// Placement of the main window.
#[tauri::command]
pub fn window_main_placement(app: AppHandle) -> Option<WindowInfo> {
    window_info(&app, MAIN, None)
}

/// Moves a window (`main` or a panel window) to a monitor; primary when it is gone.
#[tauri::command]
pub fn window_move_to_monitor(
    app: AppHandle,
    label: String,
    monitor: String,
    bounds: Option<Rect>,
    maximized: Option<bool>,
) -> Result<WindowInfo, String> {
    if label != MAIN && !label.starts_with(PANEL_WINDOW_PREFIX) {
        return Err("unknown window".into());
    }
    let w = app.get_webview_window(&label).ok_or("window not open")?;
    let mons = monitors(&app);
    let frac = if label == MAIN { 0.85 } else { 0.6 };
    let p = resolve_placement(&mons, Some(&monitor), bounds, frac).ok_or("no monitors")?;
    let max = maximized.unwrap_or_else(|| w.is_maximized().unwrap_or(false));
    place(&w, &p, max)?;
    let state = app.state::<PanelWindows>();
    let meta = state.open.lock().ok().and_then(|m| m.get(&label).cloned());
    let mut info = window_info(&app, &label, meta.as_ref()).ok_or("window vanished")?;
    info.fell_back = p.fell_back;
    notify_changed(&app);
    Ok(info)
}

/// Brings every managed window that is no longer reachable (its monitor was
/// disconnected) back onto the primary monitor. Returns the moved labels.
#[tauri::command]
pub fn window_reconcile(app: AppHandle) -> Vec<String> {
    let mons = monitors(&app);
    if mons.is_empty() {
        return Vec::new();
    }
    let mut moved = Vec::new();
    for (label, w) in app.webview_windows() {
        if label != MAIN && !label.starts_with(PANEL_WINDOW_PREFIX) {
            continue;
        }
        let (Ok(pos), Ok(size)) = (w.outer_position(), w.outer_size()) else { continue };
        let rect = to_rect(pos, size);
        if w.is_minimized().unwrap_or(false) || is_reachable(&mons, &rect) {
            continue;
        }
        let frac = if label == MAIN { 0.85 } else { 0.6 };
        let rel = Rect { x: 0, y: 0, width: rect.width, height: rect.height };
        if let Some(mut p) = resolve_placement(&mons, None, Some(rel), frac) {
            // Centre on the fallback monitor.
            let wa = primary_or_first(&mons).map(|m| m.work_area).unwrap_or_default();
            p.absolute.x = wa.x + i32::try_from((wa.width - p.absolute.width) / 2).unwrap_or(0);
            p.absolute.y = wa.y + i32::try_from((wa.height - p.absolute.height) / 2).unwrap_or(0);
            if place(&w, &p, false).is_ok() {
                tracing::warn!(%label, "window was off-screen (monitor disconnected); moved to the primary monitor");
                moved.push(label);
            }
        }
    }
    if !moved.is_empty() {
        notify_changed(&app);
    }
    moved
}

/// Window-event hook for panel windows (called from `on_window_event`).
pub fn on_panel_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    let label = window.label();
    if !label.starts_with(PANEL_WINDOW_PREFIX) {
        return;
    }
    let app = window.app_handle();
    match event {
        tauri::WindowEvent::Destroyed => {
            let state = app.state::<PanelWindows>();
            let meta = state.open.lock().ok().and_then(|mut m| m.remove(label));
            // A re-docked window was already removed (and announced) by `window_redock_panel`.
            if let Some(meta) = meta {
                let payload =
                    RedockPayload { label: label.to_owned(), panel_id: meta.panel_id, project_id: meta.project_id };
                let _ = app.emit_to(MAIN, EVT_PANEL_CLOSED, payload);
            }
            notify_changed(app);
        }
        tauri::WindowEvent::Moved(_) | tauri::WindowEvent::Resized(_) => notify_changed(app),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mon(key: &str, x: i32, w: u32, primary: bool) -> MonitorGeom {
        let bounds = Rect { x, y: 0, width: w, height: 1080 };
        MonitorGeom {
            key: key.into(),
            name: Some(key.into()),
            bounds,
            work_area: Rect { x, y: 0, width: w, height: 1040 },
            scale_factor: 1.0,
            primary,
        }
    }

    fn setup() -> Vec<MonitorGeom> {
        vec![mon("DISPLAY1", 0, 1920, true), mon("DISPLAY2", 1920, 2560, false)]
    }

    #[test]
    fn places_on_the_requested_monitor() {
        let p =
            resolve_placement(&setup(), Some("DISPLAY2"), Some(Rect { x: 100, y: 50, width: 800, height: 600 }), 0.6)
                .unwrap();
        assert_eq!(p.monitor, "DISPLAY2");
        assert!(!p.fell_back);
        assert_eq!(p.absolute, Rect { x: 2020, y: 50, width: 800, height: 600 });
        assert_eq!(p.relative, Rect { x: 100, y: 50, width: 800, height: 600 });
    }

    #[test]
    fn falls_back_to_primary_when_monitor_is_gone_and_clamps() {
        let p = resolve_placement(
            &setup(),
            Some("DISPLAY3"),
            Some(Rect { x: 2000, y: 900, width: 2500, height: 600 }),
            0.6,
        )
        .unwrap();
        assert_eq!(p.monitor, "DISPLAY1");
        assert!(p.fell_back);
        // Width clamped to the work area, position clamped inside it.
        assert_eq!(p.absolute.width, 1920);
        assert_eq!(p.absolute.x, 0);
        assert_eq!(p.absolute.y + i32::try_from(p.absolute.height).unwrap(), 1040);
    }

    #[test]
    fn default_bounds_are_centred() {
        let p = resolve_placement(&setup(), None, None, 0.5).unwrap();
        assert_eq!(p.monitor, "DISPLAY1");
        assert!(!p.fell_back);
        assert_eq!(p.relative, Rect { x: 480, y: 260, width: 960, height: 520 });
        assert!(resolve_placement(&[], None, None, 0.5).is_none());
    }

    #[test]
    fn tiny_bounds_grow_to_the_minimum() {
        let p = resolve_placement(&setup(), Some("DISPLAY1"), Some(Rect { x: 5, y: 5, width: 10, height: 10 }), 0.6)
            .unwrap();
        assert_eq!((p.absolute.width, p.absolute.height), (MIN_W, MIN_H));
    }

    #[test]
    fn monitor_lookup_and_reachability() {
        let mons = setup();
        let on_second = Rect { x: 1800, y: 100, width: 800, height: 600 };
        assert_eq!(monitor_for_rect(&mons, &on_second).unwrap().key, "DISPLAY2");
        let gone = Rect { x: 5000, y: 100, width: 800, height: 600 };
        assert!(monitor_for_rect(&mons, &gone).is_none());
        assert!(!is_reachable(&mons, &gone));
        assert!(is_reachable(&mons, &on_second));
        let sliver = Rect { x: 4470, y: 0, width: 800, height: 600 };
        assert!(!is_reachable(&mons, &sliver), "10 px on screen cannot be grabbed");
        assert_eq!(relative_to(&mons[1], &on_second), Rect { x: -120, y: 100, width: 800, height: 600 });
    }

    #[test]
    fn keys_and_labels() {
        let r = Rect { x: -1920, y: 0, width: 1920, height: 1080 };
        assert_eq!(monitor_key(None, &r), "@-1920,0,1920x1080");
        assert_eq!(monitor_key(Some("  "), &r), "@-1920,0,1920x1080");
        assert_eq!(monitor_key(Some("\\\\.\\DISPLAY1"), &r), "\\\\.\\DISPLAY1");
        assert_eq!(panel_label("missionControl"), "panel-missionControl");
        assert!(is_id("missionControl", 48, &['-', '_']));
        assert!(!is_id("../x", 48, &['-', '_']));
        assert!(!is_id("a?b=c", 64, &['-']));
    }

    #[test]
    fn reopening_a_panel_for_another_project_rebinds_the_window() {
        assert_eq!(panel_route("terminal", Some("p-a")), "/panel/terminal?projectId=p-a");
        assert_eq!(panel_route("terminal", None), "/panel/terminal");
        let meta = |p: Option<&str>| PanelMeta { panel_id: "terminal".into(), project_id: p.map(str::to_owned) };
        assert!(needs_rebind(Some(&meta(Some("a"))), &meta(Some("b"))), "A → B must re-point the webview");
        assert!(needs_rebind(Some(&meta(Some("a"))), &meta(None)), "A → active project");
        assert!(needs_rebind(Some(&meta(None)), &meta(Some("b"))));
        assert!(!needs_rebind(Some(&meta(Some("a"))), &meta(Some("a"))), "same project: just focus");
        assert!(!needs_rebind(Some(&meta(None)), &meta(None)));
        assert!(needs_rebind(None, &meta(Some("a"))), "unknown window content: re-point it");
    }
}
