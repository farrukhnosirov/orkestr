//! Floating robot windows (§30, ADR-0016).
//!
//! Every visible robot is its own small transparent, frameless, always-on-top,
//! skip-taskbar `WebviewWindow` (`robot-<agentId>`, plus `robot-cluster` for the
//! overflow stack) loading the separate `robot.html` entry. The main window's
//! robot host decides **which** robots are visible (from agent events over the
//! RPC protocol) and calls [`robot_open`] / [`robot_close`]; this module only
//! manages windows — no business data flows through it (§67).
//!
//! * **Click-through (RR-C4):** windows start with `set_ignore_cursor_events(true)`.
//!   A poller reads the global cursor position at 30 Hz (only while robots are
//!   open) and hit-tests it against each robot's shape (ellipses + rectangles
//!   in CSS px, reported by the robot webview via [`robot_set_shape`]). Cursor
//!   events are re-enabled only while the cursor is over the robot; in
//!   pass-through mode (idle robot) only after a 600 ms hover.
//! * **Drag:** the webview calls `startDragging()`. Moves while the cursor is
//!   over the robot are user drags; 400 ms without a move ends the drag. Then
//!   the cursor is hit-tested against the main window's drop zones (project
//!   rail tiles, reported by the main window via [`robot_set_drop_zones`]): a hit
//!   emits `orkestr://robot/drop` to the main window and puts the robot back.
//!   Otherwise the robot snaps to monitor work-area edges/corners and other
//!   robots (16 px) and its window gets `orkestr://robot/moved`.
//! * **Inter-window:** robot → main requests go through [`robot_to_main`]
//!   (whitelisted names), which also brings the main window forward for
//!   "open agent".

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};
use tokio::sync::Notify;

/// Label prefix of robot windows.
pub const ROBOT_LABEL_PREFIX: &str = "robot-";
/// Label of the overflow cluster window.
pub const CLUSTER_LABEL: &str = "robot-cluster";
/// Hover time before a pass-through (idle) robot becomes interactive.
pub const PASS_THROUGH_DWELL: Duration = Duration::from_millis(600);
/// Quiet time after the last move that ends a drag.
pub const DRAG_END_QUIET: Duration = Duration::from_millis(400);
/// Snap distance in logical px.
pub const SNAP_THRESHOLD: f64 = 16.0;
/// Cursor polling period (30 Hz).
const POLL: Duration = Duration::from_millis(33);

/// Event names (robot window ← shell).
pub const EV_HOVER: &str = "orkestr://robot/hover";
/// Drag started / ended.
pub const EV_DRAG: &str = "orkestr://robot/drag";
/// The robot window settled at a new position (after snapping).
pub const EV_MOVED: &str = "orkestr://robot/moved";
/// Main window ← shell: a robot was dropped on a drop zone.
pub const EV_DROP: &str = "orkestr://robot/drop";
/// Main window ← robot (via [`robot_to_main`]) prefix: `orkestr://robot/<name>`.
const EV_MAIN_PREFIX: &str = "orkestr://robot/";

/// An axis-aligned rectangle (CSS px or physical px depending on context).
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
}

/// An ellipse in window-local CSS px.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Ellipse {
    pub cx: f64,
    pub cy: f64,
    pub rx: f64,
    pub ry: f64,
}

impl Ellipse {
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        if self.rx <= 0.0 || self.ry <= 0.0 {
            return false;
        }
        let dx = (px - self.cx) / self.rx;
        let dy = (py - self.cy) / self.ry;
        dx * dx + dy * dy <= 1.0
    }
}

/// Interactive area of a robot window (window-local CSS px).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct HitShape {
    #[serde(default)]
    pub ellipses: Vec<Ellipse>,
    #[serde(default)]
    pub rects: Vec<Rect>,
}

impl HitShape {
    /// Whether a window-local CSS px point is on the robot.
    #[must_use]
    pub fn contains(&self, px: f64, py: f64) -> bool {
        self.ellipses.iter().any(|e| e.contains(px, py)) || self.rects.iter().any(|r| r.contains(px, py))
    }
}

/// Hover gate: geometric hit → interactive, with a dwell in pass-through mode.
#[must_use]
pub fn gate_hover(geom_inside: bool, pass_through: bool, hover_since: &mut Option<Instant>, now: Instant) -> bool {
    if !geom_inside {
        *hover_since = None;
        return false;
    }
    let since = *hover_since.get_or_insert(now);
    !pass_through || now.duration_since(since) >= PASS_THROUGH_DWELL
}

/// Snaps a window rect to work-area edges/corners and to other robots.
/// Returns the new top-left. All values in the same (physical) unit.
#[must_use]
pub fn snap_rect(rect: Rect, areas: &[Rect], others: &[Rect], threshold: f64) -> (f64, f64) {
    let (cx, cy) = rect.center();
    let area = areas
        .iter()
        .find(|a| a.contains(cx, cy))
        .or_else(|| {
            areas.iter().min_by(|a, b| {
                let da = (a.center().0 - cx).hypot(a.center().1 - cy);
                let db = (b.center().0 - cx).hypot(b.center().1 - cy);
                da.total_cmp(&db)
            })
        })
        .copied();
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    if let Some(a) = area {
        xs.extend([a.x, a.x + a.w - rect.w]);
        ys.extend([a.y, a.y + a.h - rect.h]);
    }
    for o in others {
        // Only robots that are vertically / horizontally near enough to be neighbours.
        let v_overlap = rect.y < o.y + o.h + threshold && o.y < rect.y + rect.h + threshold;
        let h_overlap = rect.x < o.x + o.w + threshold && o.x < rect.x + rect.w + threshold;
        if v_overlap {
            xs.extend([o.x + o.w, o.x - rect.w, o.x]);
        }
        if h_overlap {
            ys.extend([o.y + o.h, o.y - rect.h, o.y]);
        }
    }
    let pick = |current: f64, candidates: &[f64]| {
        candidates
            .iter()
            .copied()
            .filter(|c| (c - current).abs() <= threshold)
            .min_by(|a, b| (a - current).abs().total_cmp(&(b - current).abs()))
            .unwrap_or(current)
    };
    let mut x = pick(rect.x, &xs);
    let mut y = pick(rect.y, &ys);
    // Never leave the robot (mostly) off-screen.
    if let Some(a) = area {
        x = x.clamp(a.x, (a.x + a.w - rect.w).max(a.x));
        y = y.clamp(a.y, (a.y + a.h - rect.h).max(a.y));
    }
    (x, y)
}

/// A drop target in the main window (CSS px relative to its client area).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DropZone {
    pub id: String,
    pub rect: Rect,
}

/// The drop zone under a physical cursor, given the main window's client
/// origin (physical) and scale factor.
#[must_use]
pub fn drop_zone_at(
    cursor: (f64, f64),
    main_origin: (f64, f64),
    main_scale: f64,
    zones: &[DropZone],
) -> Option<&DropZone> {
    if main_scale <= 0.0 {
        return None;
    }
    let lx = (cursor.0 - main_origin.0) / main_scale;
    let ly = (cursor.1 - main_origin.1) / main_scale;
    zones.iter().find(|z| z.rect.contains(lx, ly))
}

/// Default position of the n-th robot: a row along the bottom-right corner of
/// the work area, wrapping upwards.
#[must_use]
pub fn default_slot(area: Rect, slot: u32, w: f64, h: f64, margin: f64) -> (f64, f64) {
    let per_row = (((area.w - margin) / (w + margin)).floor() as u32).max(1);
    let col = f64::from(slot % per_row);
    let row = f64::from(slot / per_row);
    let x = area.x + area.w - margin - w - col * (w + margin);
    let y = area.y + area.h - margin - h - row * (h + margin);
    (x.max(area.x), y.max(area.y))
}

/// Whether an agent id is safe to embed in a window label / URL.
#[must_use]
pub fn is_safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 64 && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

#[derive(Debug)]
struct RobotWin {
    shape: HitShape,
    pass_through: bool,
    snap: bool,
    agent_id: Option<String>,
    pos: (f64, f64),
    size: (f64, f64),
    scale: f64,
    inside: bool,
    hover_since: Option<Instant>,
    last_user_move: Option<Instant>,
    drag_start: Option<(f64, f64)>,
    expected_pos: Option<(i32, i32)>,
}

#[derive(Debug, Default)]
struct Registry {
    robots: HashMap<String, RobotWin>,
    drop_zones: Vec<DropZone>,
    poller: bool,
}

/// Shared state of the robot windows (managed by Tauri).
#[derive(Debug, Default)]
pub struct RobotsState {
    inner: std::sync::Mutex<Registry>,
    wake: Arc<Notify>,
}

fn lock(state: &RobotsState) -> Result<std::sync::MutexGuard<'_, Registry>, String> {
    state.inner.lock().map_err(|_| "robot state poisoned".to_owned())
}

/// Arguments of [`robot_open`].
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RobotOpenArgs {
    /// Agent id (`None` = the overflow cluster window).
    pub agent_id: Option<String>,
    /// Logical window size.
    pub width: f64,
    pub height: f64,
    /// Remembered physical position (validated against the monitors).
    pub x: Option<f64>,
    pub y: Option<f64>,
    /// Slot for the default placement.
    pub slot: u32,
    /// Snap after drags.
    pub snap: bool,
}

fn label_for(agent_id: Option<&str>) -> Result<String, String> {
    match agent_id {
        None => Ok(CLUSTER_LABEL.to_owned()),
        Some(id) if is_safe_id(id) && id != "cluster" => Ok(format!("{ROBOT_LABEL_PREFIX}{id}")),
        Some(_) => Err("invalid agent id".to_owned()),
    }
}

fn work_areas(app: &AppHandle) -> Vec<Rect> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let a = m.work_area();
            Rect {
                x: f64::from(a.position.x),
                y: f64::from(a.position.y),
                w: f64::from(a.size.width),
                h: f64::from(a.size.height),
            }
        })
        .collect()
}

/// Opens (or keeps) a robot window. Window management only (§67).
#[tauri::command]
pub async fn robot_open(
    app: AppHandle,
    state: tauri::State<'_, RobotsState>,
    args: RobotOpenArgs,
) -> Result<String, String> {
    let label = label_for(args.agent_id.as_deref())?;
    if app.get_webview_window(&label).is_some() {
        return Ok(label);
    }
    let width = args.width.clamp(80.0, 600.0);
    let height = args.height.clamp(60.0, 700.0);
    let route = match &args.agent_id {
        Some(id) => format!("robot.html#/agent/{id}"),
        None => "robot.html#/cluster".to_owned(),
    };
    let primary = app.primary_monitor().ok().flatten();
    let scale = primary.as_ref().map_or(1.0, tauri::Monitor::scale_factor);
    let areas = work_areas(&app);
    let phys_w = width * scale;
    let phys_h = height * scale;
    let remembered = match (args.x, args.y) {
        (Some(x), Some(y)) if areas.iter().any(|a| a.contains(x + phys_w / 2.0, y + phys_h / 2.0)) => Some((x, y)),
        _ => None,
    };
    let (x, y) = remembered.unwrap_or_else(|| {
        let area = primary
            .as_ref()
            .map(|m| {
                let a = m.work_area();
                Rect {
                    x: f64::from(a.position.x),
                    y: f64::from(a.position.y),
                    w: f64::from(a.size.width),
                    h: f64::from(a.size.height),
                }
            })
            .unwrap_or(Rect { x: 0.0, y: 0.0, w: 1280.0, h: 720.0 });
        default_slot(area, args.slot, phys_w, phys_h, 12.0 * scale)
    });
    let window = WebviewWindowBuilder::new(&app, &label, WebviewUrl::App(route.into()))
        .title("Orkestr robot")
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
        .inner_size(width, height)
        .position(0.0, 0.0)
        .visible(false)
        .build()
        .map_err(|e| e.to_string())?;
    let _ = window.set_position(PhysicalPosition::new(x.round() as i32, y.round() as i32));
    let _ = window.set_ignore_cursor_events(true);
    let _ = window.show();
    let win_scale = window.scale_factor().unwrap_or(scale);
    {
        let mut reg = lock(&state)?;
        reg.robots.insert(
            label.clone(),
            RobotWin {
                shape: HitShape::default(),
                pass_through: true,
                snap: args.snap,
                agent_id: args.agent_id.clone(),
                pos: (x.round(), y.round()),
                size: (width * win_scale, height * win_scale),
                scale: win_scale,
                inside: false,
                hover_since: None,
                last_user_move: None,
                drag_start: None,
                expected_pos: Some((x.round() as i32, y.round() as i32)),
            },
        );
        if !reg.poller {
            reg.poller = true;
            spawn_poller(app.clone());
        }
    }
    state.wake.notify_one();
    tracing::info!(%label, "robot window opened");
    Ok(label)
}

/// Closes a robot window (`agentId: null` = the cluster).
#[tauri::command]
pub async fn robot_close(
    app: AppHandle,
    state: tauri::State<'_, RobotsState>,
    agent_id: Option<String>,
) -> Result<(), String> {
    let label = label_for(agent_id.as_deref())?;
    lock(&state)?.robots.remove(&label);
    if let Some(w) = app.get_webview_window(&label) {
        w.close().map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Closes every robot window (robots switched off).
#[tauri::command]
pub async fn robot_close_all(app: AppHandle, state: tauri::State<'_, RobotsState>) -> Result<(), String> {
    let labels: Vec<String> = lock(&state)?.robots.drain().map(|(k, _)| k).collect();
    for label in labels {
        if let Some(w) = app.get_webview_window(&label) {
            let _ = w.close();
        }
    }
    Ok(())
}

/// Robot webview → shell: its interactive shape and behaviour flags.
#[tauri::command]
pub fn robot_set_shape(
    window: WebviewWindow,
    state: tauri::State<'_, RobotsState>,
    shape: HitShape,
    pass_through: bool,
    snap: bool,
) -> Result<(), String> {
    if shape.ellipses.len() + shape.rects.len() > 16 {
        return Err("hit shape too complex".to_owned());
    }
    let mut reg = lock(&state)?;
    if let Some(r) = reg.robots.get_mut(window.label()) {
        r.shape = shape;
        r.pass_through = pass_through;
        r.snap = snap;
    }
    Ok(())
}

/// Main window → shell: drop zones (project rail tiles) in CSS px.
#[tauri::command]
pub fn robot_set_drop_zones(
    window: WebviewWindow,
    state: tauri::State<'_, RobotsState>,
    zones: Vec<DropZone>,
) -> Result<(), String> {
    if window.label() != crate::MAIN_WINDOW {
        return Err("only the main window reports drop zones".to_owned());
    }
    let mut zones = zones;
    zones.truncate(256);
    lock(&state)?.drop_zones = zones;
    Ok(())
}

/// Requests a robot window may forward to the main window.
const MAIN_REQUESTS: &[&str] = &["open-agent", "hide", "assign", "open-cluster", "voice", "cluster-ready"];

/// Robot webview → main window request (`open-agent`, `hide`, `assign`,
/// `open-cluster`, `voice`, `cluster-ready`). `open-agent` / `open-cluster` /
/// `assign` bring the main window forward.
#[tauri::command]
pub fn robot_to_main(
    app: AppHandle,
    window: WebviewWindow,
    request: String,
    payload: serde_json::Value,
) -> Result<(), String> {
    if !window.label().starts_with(ROBOT_LABEL_PREFIX) {
        return Err("not a robot window".to_owned());
    }
    if !MAIN_REQUESTS.contains(&request.as_str()) {
        return Err("unknown robot request".to_owned());
    }
    if matches!(request.as_str(), "open-agent" | "open-cluster" | "assign") {
        crate::show_main(&app);
    }
    app.emit_to(crate::MAIN_WINDOW, &format!("{EV_MAIN_PREFIX}{request}"), payload).map_err(|e| e.to_string())
}

/// Window events of robot windows (called from the app's `on_window_event`).
pub fn on_window_event(window: &tauri::Window, event: &tauri::WindowEvent) {
    let label = window.label();
    if !label.starts_with(ROBOT_LABEL_PREFIX) {
        return;
    }
    let app = window.app_handle();
    let state = app.state::<RobotsState>();
    let Ok(mut reg) = state.inner.lock() else { return };
    match event {
        tauri::WindowEvent::Moved(p) => {
            let Some(r) = reg.robots.get_mut(label) else { return };
            let prev = r.pos;
            r.pos = (f64::from(p.x), f64::from(p.y));
            if r.expected_pos == Some((p.x, p.y)) {
                r.expected_pos = None;
                return;
            }
            if r.inside {
                if r.last_user_move.is_none() {
                    r.drag_start = Some(prev);
                    let _ = app.emit_to(label, EV_DRAG, true);
                }
                r.last_user_move = Some(Instant::now());
            }
        }
        tauri::WindowEvent::ScaleFactorChanged { scale_factor, new_inner_size, .. } => {
            if let Some(r) = reg.robots.get_mut(label) {
                r.scale = *scale_factor;
                r.size = (f64::from(new_inner_size.width), f64::from(new_inner_size.height));
            }
        }
        tauri::WindowEvent::Destroyed => {
            reg.robots.remove(label);
        }
        _ => {}
    }
}

enum Action {
    Hover { label: String, inside: bool },
    DragEnd { label: String },
}

fn spawn_poller(app: AppHandle) {
    tauri::async_runtime::spawn(async move {
        let wake = Arc::clone(&app.state::<RobotsState>().wake);
        loop {
            let empty = app.state::<RobotsState>().inner.lock().map(|r| r.robots.is_empty()).unwrap_or(true);
            if empty {
                wake.notified().await;
                continue;
            }
            tokio::time::sleep(POLL).await;
            let Ok(cursor) = app.cursor_position() else { continue };
            let now = Instant::now();
            let mut actions = Vec::new();
            {
                let state = app.state::<RobotsState>();
                let Ok(mut reg) = state.inner.lock() else { return };
                for (label, r) in &mut reg.robots {
                    let lx = (cursor.x - r.pos.0) / r.scale;
                    let ly = (cursor.y - r.pos.1) / r.scale;
                    let geom = r.shape.contains(lx, ly);
                    // While dragging the cursor is on the robot by definition.
                    let inside =
                        r.last_user_move.is_some() || gate_hover(geom, r.pass_through, &mut r.hover_since, now);
                    if inside != r.inside {
                        r.inside = inside;
                        actions.push(Action::Hover { label: label.clone(), inside });
                    }
                    if r.last_user_move.is_some_and(|t| now.duration_since(t) >= DRAG_END_QUIET) {
                        r.last_user_move = None;
                        actions.push(Action::DragEnd { label: label.clone() });
                    }
                }
            }
            for action in actions {
                match action {
                    Action::Hover { label, inside } => {
                        if let Some(w) = app.get_webview_window(&label) {
                            let _ = w.set_ignore_cursor_events(!inside);
                            let _ = app.emit_to(label.as_str(), EV_HOVER, inside);
                        }
                    }
                    Action::DragEnd { label } => finish_drag(&app, &label, (cursor.x, cursor.y)),
                }
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

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DropPayload {
    agent_id: String,
    project_id: String,
}

fn finish_drag(app: &AppHandle, label: &str, cursor: (f64, f64)) {
    let Some(window) = app.get_webview_window(label) else { return };
    let _ = app.emit_to(label, EV_DRAG, false);
    // 1. Dropped on a project in the main window?
    let main_geom = app.get_webview_window(crate::MAIN_WINDOW).and_then(|m| {
        let visible = m.is_visible().unwrap_or(false) && !m.is_minimized().unwrap_or(false);
        let origin = m.inner_position().ok()?;
        let scale = m.scale_factor().ok()?;
        visible.then_some(((f64::from(origin.x), f64::from(origin.y)), scale))
    });
    let state = app.state::<RobotsState>();
    let (snap, agent_id, drag_start, rect, others, hit) = {
        let Ok(mut reg) = state.inner.lock() else { return };
        let hit = main_geom
            .and_then(|(origin, scale)| drop_zone_at(cursor, origin, scale, &reg.drop_zones).map(|z| z.id.clone()));
        let others: Vec<Rect> = reg
            .robots
            .iter()
            .filter(|(l, _)| l.as_str() != label)
            .map(|(_, o)| Rect { x: o.pos.0, y: o.pos.1, w: o.size.0, h: o.size.1 })
            .collect();
        let Some(r) = reg.robots.get_mut(label) else { return };
        let rect = Rect { x: r.pos.0, y: r.pos.1, w: r.size.0, h: r.size.1 };
        (r.snap, r.agent_id.clone(), r.drag_start.take(), rect, others, hit)
    };
    if let (Some(project_id), Some(agent_id)) = (hit, agent_id) {
        tracing::info!(%agent_id, %project_id, "robot dropped on a project");
        let _ = app.emit_to(crate::MAIN_WINDOW, EV_DROP, DropPayload { agent_id, project_id });
        if let Some((x, y)) = drag_start {
            move_to(app, &window, label, x, y);
        }
        return;
    }
    // 2. Snap.
    let (x, y) = if snap {
        let scale = window.scale_factor().unwrap_or(1.0);
        snap_rect(rect, &work_areas(app), &others, SNAP_THRESHOLD * scale)
    } else {
        (rect.x, rect.y)
    };
    move_to(app, &window, label, x, y);
}

fn move_to(app: &AppHandle, window: &WebviewWindow, label: &str, x: f64, y: f64) {
    let target = (x.round() as i32, y.round() as i32);
    let current = window.outer_position().ok().map(|p| (p.x, p.y));
    if current != Some(target) {
        if let Ok(mut reg) = app.state::<RobotsState>().inner.lock() {
            if let Some(r) = reg.robots.get_mut(label) {
                r.expected_pos = Some(target);
                r.pos = (f64::from(target.0), f64::from(target.1));
            }
        }
        let _ = window.set_position(PhysicalPosition::new(target.0, target.1));
    }
    let monitor = window.current_monitor().ok().flatten().and_then(|m| m.name().cloned());
    let _ = app.emit_to(label, EV_MOVED, MovedPayload { x: f64::from(target.0), y: f64::from(target.1), monitor });
}

#[cfg(test)]
mod tests {
    use super::*;

    const SCREEN: Rect = Rect { x: 0.0, y: 0.0, w: 1920.0, h: 1040.0 };

    #[test]
    fn hit_shape_ellipse_and_rect() {
        let shape = HitShape {
            ellipses: vec![Ellipse { cx: 100.0, cy: 200.0, rx: 60.0, ry: 70.0 }],
            rects: vec![Rect { x: 10.0, y: 10.0, w: 200.0, h: 50.0 }],
        };
        assert!(shape.contains(100.0, 200.0));
        assert!(shape.contains(159.0, 200.0));
        assert!(!shape.contains(161.0, 200.0));
        assert!(!shape.contains(150.0, 255.0), "corner of the bounding box is outside the ellipse");
        assert!(shape.contains(205.0, 55.0), "bubble rect");
        assert!(!shape.contains(5.0, 5.0));
        assert!(!HitShape::default().contains(1.0, 1.0));
        assert!(!Ellipse { cx: 0.0, cy: 0.0, rx: 0.0, ry: 5.0 }.contains(0.0, 0.0));
    }

    #[test]
    fn pass_through_requires_dwell() {
        let t0 = Instant::now();
        let mut since = None;
        assert!(!gate_hover(true, true, &mut since, t0));
        assert!(!gate_hover(true, true, &mut since, t0 + Duration::from_millis(500)));
        assert!(gate_hover(true, true, &mut since, t0 + Duration::from_millis(650)));
        assert!(!gate_hover(false, true, &mut since, t0 + Duration::from_millis(700)));
        assert!(since.is_none(), "leaving resets the dwell");
        let mut since = None;
        assert!(gate_hover(true, false, &mut since, t0), "interactive robots react immediately");
    }

    #[test]
    fn snaps_to_screen_corner_and_edges() {
        let rect = Rect { x: 1920.0 - 200.0 - 10.0, y: 1040.0 - 260.0 - 7.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(rect, &[SCREEN], &[], 16.0), (1720.0, 780.0));
        let far = Rect { x: 600.0, y: 400.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(far, &[SCREEN], &[], 16.0), (600.0, 400.0), "nothing within the threshold");
        let left = Rect { x: 12.0, y: 400.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(left, &[SCREEN], &[], 16.0), (0.0, 400.0));
    }

    #[test]
    fn snaps_next_to_other_robots_and_stays_on_screen() {
        let other = Rect { x: 1000.0, y: 500.0, w: 200.0, h: 260.0 };
        let rect = Rect { x: 1210.0, y: 505.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(rect, &[SCREEN], &[other], 16.0), (1200.0, 500.0), "right of + top-aligned");
        let above = Rect { x: 1004.0, y: 230.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(above, &[SCREEN], &[other], 16.0), (1000.0, 240.0), "stacked above");
        let off = Rect { x: 1900.0, y: 1000.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(off, &[SCREEN], &[], 16.0), (1720.0, 780.0), "clamped back on screen");
        let second = Rect { x: 1920.0, y: 0.0, w: 1280.0, h: 1024.0 };
        let on_second = Rect { x: 1925.0, y: 300.0, w: 200.0, h: 260.0 };
        assert_eq!(snap_rect(on_second, &[SCREEN, second], &[], 16.0), (1920.0, 300.0), "uses the robot's monitor");
    }

    #[test]
    fn drop_zone_hit_test_uses_main_window_origin_and_scale() {
        let zones = vec![
            DropZone { id: "p1".into(), rect: Rect { x: 10.0, y: 50.0, w: 48.0, h: 48.0 } },
            DropZone { id: "p2".into(), rect: Rect { x: 10.0, y: 110.0, w: 48.0, h: 48.0 } },
        ];
        // Main window client area at (100, 200) physical, 150% scale.
        let at = |x: f64, y: f64| drop_zone_at((x, y), (100.0, 200.0), 1.5, &zones).map(|z| z.id.as_str());
        assert_eq!(at(100.0 + 30.0 * 1.5, 200.0 + 70.0 * 1.5), Some("p1"));
        assert_eq!(at(100.0 + 30.0 * 1.5, 200.0 + 130.0 * 1.5), Some("p2"));
        assert_eq!(at(100.0 + 300.0 * 1.5, 200.0 + 70.0 * 1.5), None);
        assert_eq!(at(50.0, 50.0), None, "outside the main window");
        assert!(drop_zone_at((0.0, 0.0), (0.0, 0.0), 0.0, &zones).is_none());
    }

    #[test]
    fn default_slots_fill_bottom_right_row_then_wrap() {
        let area = Rect { x: 0.0, y: 0.0, w: 700.0, h: 1000.0 };
        assert_eq!(default_slot(area, 0, 200.0, 260.0, 12.0), (488.0, 728.0));
        assert_eq!(default_slot(area, 1, 200.0, 260.0, 12.0), (276.0, 728.0));
        assert_eq!(default_slot(area, 3, 200.0, 260.0, 12.0), (488.0, 456.0), "3 per row → wraps");
    }

    #[test]
    fn labels_are_safe() {
        assert_eq!(
            label_for(Some("0190a0b0-0000-7000-8000-000000000001")).unwrap(),
            "robot-0190a0b0-0000-7000-8000-000000000001"
        );
        assert_eq!(label_for(None).unwrap(), CLUSTER_LABEL);
        assert!(label_for(Some("../evil")).is_err());
        assert!(label_for(Some("")).is_err());
    }
}
