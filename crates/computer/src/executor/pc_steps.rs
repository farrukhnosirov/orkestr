//! Wave 10 steps (ADR-0055): sound (endpoint, apps, microphone, devices,
//! media seek), window geometry, virtual desktops, the cursor and mouse,
//! keyboard layouts, displays, radios, reports (`Query`), alarms and files.
//! The arithmetic (levels, snap rectangles) is plain code here so the mock
//! backend tests it; the backend only answers [`PcRequest`]s.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use serde_json::json;

use orkestr_protocol::methods::computer::{
    AudioDeviceKind, ClientEffect, DesktopOp, DisplayOp, LevelOp, MouseButton, QueryKind, RadioKind, ScreenPoint,
    ScreenshotScope, ScrollDirection, SlotNum, WindowInfo, WindowOp,
};

use super::{num, Runner, StepOut};
use crate::backend::{ComputerBackend, PcReply, PcRequest, Rect};
use crate::error::{ComputerError, ComputerResult};

/// A mouse button is held by a `MouseHold` step (released by the global stop).
static HELD: AtomicBool = AtomicBool::new(false);

/// Releases buttons a `MouseHold` left pressed (global stop / emergency).
pub fn release_held_buttons(b: &dyn ComputerBackend) {
    if HELD.swap(false, Ordering::SeqCst) {
        for btn in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
            let _ = b.pc(&PcRequest::MouseButton(btn, false));
        }
    }
}

/// The level after `op` (`None` for the mute operations). `v`: percent for
/// Up / Down / Set, percent of the current level for Scale.
#[must_use]
pub fn level_after(op: LevelOp, cur: u32, v: u32, max: u32) -> Option<u32> {
    Some(match op {
        LevelOp::Up => cur.saturating_add(v).min(max),
        LevelOp::Down => cur.saturating_sub(v),
        LevelOp::Set => v.min(max),
        LevelOp::Scale => {
            let s = u64::from(cur) * u64::from(v) / 100;
            // "twice as loud" from 0 still makes it audible
            let s = if cur == 0 && v > 100 { u64::from(max / 10) } else { s };
            u32::try_from(s).unwrap_or(max).min(max)
        }
        LevelOp::Mute | LevelOp::MuteOn | LevelOp::MuteOff => return None,
    })
}

fn mute_of(op: LevelOp) -> Option<Option<bool>> {
    match op {
        LevelOp::Mute => Some(None),
        LevelOp::MuteOn => Some(Some(true)),
        LevelOp::MuteOff => Some(Some(false)),
        _ => None,
    }
}

fn default_value(op: LevelOp) -> u32 {
    if op == LevelOp::Scale {
        50
    } else {
        10
    }
}

/// Window ops the executor does itself (geometry / z-order / hotkeys).
#[must_use]
pub fn is_geometry(op: WindowOp) -> bool {
    matches!(
        op,
        WindowOp::SnapTop
            | WindowOp::SnapBottom
            | WindowOp::SnapTopLeft
            | WindowOp::SnapTopRight
            | WindowOp::SnapBottomLeft
            | WindowOp::SnapBottomRight
            | WindowOp::Center
            | WindowOp::AlwaysOnTop
            | WindowOp::NotOnTop
            | WindowOp::MinimizeAll
            | WindowOp::RestoreAll
    )
}

/// Target rectangle of a snap / center op inside `work` (`cur` = the
/// window's rectangle, kept in size by `Center`).
#[must_use]
pub fn snap_rect(op: WindowOp, work: Rect, cur: Rect) -> Option<Rect> {
    let (x, y, w, h) = (work.left, work.top, work.width(), work.height());
    let (hw, hh) = (w / 2, h / 2);
    Some(match op {
        WindowOp::SnapLeft => Rect::xywh(x, y, hw, h),
        WindowOp::SnapRight => Rect::xywh(x + hw, y, w - hw, h),
        WindowOp::SnapTop => Rect::xywh(x, y, w, hh),
        WindowOp::SnapBottom => Rect::xywh(x, y + hh, w, h - hh),
        WindowOp::SnapTopLeft => Rect::xywh(x, y, hw, hh),
        WindowOp::SnapTopRight => Rect::xywh(x + hw, y, w - hw, hh),
        WindowOp::SnapBottomLeft => Rect::xywh(x, y + hh, hw, h - hh),
        WindowOp::SnapBottomRight => Rect::xywh(x + hw, y + hh, w - hw, h - hh),
        WindowOp::Center => {
            let (cw, ch) = (cur.width().min(w), cur.height().min(h));
            Rect::xywh(x + (w - cw) / 2, y + (h - ch) / 2, cw, ch)
        }
        _ => return None,
    })
}

/// A size centered in `work` (clamped to it, at least 200×150).
#[must_use]
pub fn centered(work: Rect, w: i32, h: i32) -> Rect {
    let (w, h) = (w.clamp(200, work.width().max(200)), h.clamp(150, work.height().max(150)));
    Rect::xywh(work.left + (work.width() - w) / 2, work.top + (work.height() - h) / 2, w, h)
}

/// Screen point inside `bounds` (5 px from the edges).
#[must_use]
pub fn screen_point(p: ScreenPoint, b: Rect) -> (i32, i32) {
    let m = 5;
    let (cx, cy) = b.center();
    let (l, t, r, btm) = (b.left + m, b.top + m, b.right - 1 - m, b.bottom - 1 - m);
    match p {
        ScreenPoint::Center => (cx, cy),
        ScreenPoint::TopLeft => (l, t),
        ScreenPoint::TopRight => (r, t),
        ScreenPoint::BottomLeft => (l, btm),
        ScreenPoint::BottomRight => (r, btm),
        ScreenPoint::Top => (cx, t),
        ScreenPoint::Bottom => (cx, btm),
        ScreenPoint::Left => (l, cy),
        ScreenPoint::Right => (r, cy),
    }
}

fn offset(d: ScrollDirection, n: i32) -> (i32, i32) {
    match d {
        ScrollDirection::Up => (0, -n),
        ScrollDirection::Down => (0, n),
        ScrollDirection::Left => (-n, 0),
        ScrollDirection::Right => (n, 0),
    }
}

/// Spoken device words → name fragments.
const DEVICE_WORDS: &[(&str, &[&str])] = &[
    ("наушник", &["headphone", "headset", "наушник", "гарнитур", "airpods", "buds"]),
    ("headphone", &["headphone", "headset", "наушник", "гарнитур"]),
    ("гарнитур", &["headset", "гарнитур", "headphone"]),
    ("динамик", &["speaker", "динамик", "realtek"]),
    ("колонк", &["speaker", "динамик", "колонк"]),
    ("speaker", &["speaker", "динамик"]),
    ("монитор", &["hdmi", "displayport", "monitor", "монитор", "nvidia", "amd high definition"]),
    ("телевизор", &["hdmi", "tv", "телевизор"]),
    ("hdmi", &["hdmi", "nvidia", "amd high definition"]),
    ("блютуз", &["bluetooth", "hands-free", "airpods", "buds"]),
    ("bluetooth", &["bluetooth", "hands-free", "airpods", "buds"]),
    ("веб", &["webcam", "camera", "камер"]),
    ("камер", &["webcam", "camera", "камер"]),
    ("usb", &["usb"]),
];

/// The device a spoken name means (exact fragment, then the word table).
#[must_use]
pub fn pick_device<'a>(
    list: &'a [crate::backend::AudioDeviceInfo],
    name: &str,
) -> Option<&'a crate::backend::AudioDeviceInfo> {
    let n = name.trim().to_lowercase().replace('ё', "е");
    if n.is_empty() {
        return None;
    }
    if let Some(d) = list.iter().find(|d| d.name.to_lowercase().contains(&n)) {
        return Some(d);
    }
    for word in n.split_whitespace() {
        let w: String = word.chars().take(9).collect();
        for (key, frags) in DEVICE_WORDS {
            if w.starts_with(key) || key.starts_with(w.as_str()) && w.chars().count() >= 4 {
                if let Some(d) = list.iter().find(|d| {
                    let dn = d.name.to_lowercase();
                    frags.iter().any(|f| dn.contains(f))
                }) {
                    return Some(d);
                }
            }
        }
        if word.chars().count() >= 4 {
            if let Some(d) = list.iter().find(|d| d.name.to_lowercase().contains(word)) {
                return Some(d);
            }
        }
    }
    None
}

/// Next local occurrence of `h:m` in seconds from `now`.
#[must_use]
pub fn seconds_until(now: chrono::NaiveDateTime, h: u32, m: u32) -> u32 {
    let today = now.date().and_hms_opt(h, m, 0).unwrap_or(now);
    let at = if today > now { today } else { today + chrono::Duration::days(1) };
    u32::try_from((at - now).num_seconds()).unwrap_or(60).max(1)
}

fn gb(bytes: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)]
    let v = bytes as f64 / 1_073_741_824.0;
    (v * 10.0).round() / 10.0
}

/// Local IPv4 used for outgoing traffic (no packet is sent: UDP connect only).
fn local_ip() -> Option<String> {
    let s = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    s.connect("192.0.2.1:9").ok()?;
    s.local_addr().ok().map(|a| a.ip().to_string())
}

/// System facts (`DiskSpace`, `Network`, `SystemInfo`, `Uptime`, `IpAddress`)
/// as JSON + display lines; blocking (CPU sampling waits ~250 ms).
fn system_query(what: QueryKind) -> (serde_json::Value, Vec<String>) {
    use sysinfo::{Disks, Networks, System};
    match what {
        QueryKind::DiskSpace => {
            let disks = Disks::new_with_refreshed_list();
            let mut list = Vec::new();
            let mut lines = Vec::new();
            for d in disks.list() {
                let mount = d.mount_point().display().to_string();
                if list.iter().any(|v: &serde_json::Value| v["mount"] == mount.as_str()) {
                    continue;
                }
                let (free, total) = (gb(d.available_space()), gb(d.total_space()));
                lines.push(format!("{mount} {free} / {total} GB"));
                list.push(json!({ "mount": mount, "freeGb": free, "totalGb": total }));
            }
            (json!({ "disks": list }), lines)
        }
        QueryKind::Network => {
            let nets = Networks::new_with_refreshed_list();
            let mut list = Vec::new();
            let mut lines = Vec::new();
            for (name, data) in &nets {
                let ips: Vec<String> = data
                    .ip_networks()
                    .iter()
                    .map(|n| n.addr.to_string())
                    .filter(|a| !a.starts_with("fe80") && a != "127.0.0.1" && a != "::1")
                    .collect();
                if ips.is_empty() {
                    continue;
                }
                lines.push(format!("{name}: {}", ips.join(", ")));
                list.push(json!({ "name": name, "ips": ips }));
            }
            (json!({ "adapters": list, "ip": local_ip() }), lines)
        }
        QueryKind::IpAddress => {
            let ip = local_ip();
            (json!({ "ip": ip }), ip.into_iter().collect())
        }
        QueryKind::Uptime => {
            let s = System::uptime();
            (json!({ "seconds": s }), vec![format!("{} h {} min", s / 3600, (s % 3600) / 60)])
        }
        _ => {
            let mut sys = System::new();
            sys.refresh_cpu_usage();
            std::thread::sleep(sysinfo::MINIMUM_CPU_UPDATE_INTERVAL.max(Duration::from_millis(250)));
            sys.refresh_cpu_usage();
            sys.refresh_memory();
            let cpu = (f64::from(sys.global_cpu_usage()) * 10.0).round() / 10.0;
            let (used, total) = (gb(sys.used_memory()), gb(sys.total_memory()));
            #[allow(clippy::cast_precision_loss)]
            let ram = if sys.total_memory() == 0 {
                0.0
            } else {
                (sys.used_memory() as f64 * 1000.0 / sys.total_memory() as f64).round() / 10.0
            };
            (
                json!({ "cpu": cpu, "ramUsedGb": used, "ramTotalGb": total, "ramPercent": ram, "gpu": null }),
                vec![format!("CPU {cpu}%"), format!("RAM {used} / {total} GB ({ram}%)")],
            )
        }
    }
}

/// Spoken folder names → paths for file search.
fn search_root(folder: Option<&str>) -> PathBuf {
    match folder.map(str::trim).filter(|f| !f.is_empty()) {
        Some(f) => crate::apps::known_folder(f)
            .filter(|p| !crate::apps::is_uri(p))
            .map_or_else(|| crate::apps::file_path(f), PathBuf::from),
        None => directories::UserDirs::new().map_or_else(|| PathBuf::from("."), |u| u.home_dir().to_path_buf()),
    }
}

const SKIP_DIRS: &[&str] =
    &["appdata", "node_modules", ".git", "target", "$recycle.bin", "windows", ".cache", ".cargo", ".rustup"];

/// Files / folders under `root` whose name contains `needle` (≤ 20, depth ≤ 6, ≤ 4 s).
#[must_use]
pub fn search_files(root: &Path, needle: &str, budget: Duration) -> Vec<PathBuf> {
    let needle = needle.trim().to_lowercase().replace('ё', "е");
    let started = Instant::now();
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        if started.elapsed() > budget || out.len() >= 20 {
            break;
        }
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().to_lowercase().replace('ё', "е");
            let is_dir = e.file_type().is_ok_and(|t| t.is_dir());
            if name.contains(&needle) {
                out.push(e.path());
                if out.len() >= 20 {
                    break;
                }
            }
            if is_dir && depth < 6 && !name.starts_with('.') && !SKIP_DIRS.contains(&name.as_str()) {
                stack.push((e.path(), depth + 1));
            }
        }
    }
    out
}

fn copy_tree(from: &Path, to: &Path) -> std::io::Result<u64> {
    if from.is_dir() {
        std::fs::create_dir_all(to)?;
        let mut n = 0;
        for e in std::fs::read_dir(from)?.flatten() {
            n += copy_tree(&e.path(), &to.join(e.file_name()))?;
        }
        Ok(n)
    } else {
        std::fs::copy(from, to).map(|_| 1)
    }
}

impl Runner<'_> {
    pub(super) async fn pc(&self, req: PcRequest) -> ComputerResult<PcReply> {
        self.call(move |b| b.pc(&req)).await
    }

    async fn target_window(&self, app: Option<&str>) -> ComputerResult<WindowInfo> {
        match app.filter(|a| !a.trim().is_empty()) {
            Some(a) => self
                .app_windows(a)
                .await?
                .into_iter()
                .next()
                .ok_or_else(|| ComputerError::NotFound(format!("no open window of “{a}”"))),
            None => self.user_window().await?.ok_or_else(|| ComputerError::NotFound("no window".into())),
        }
    }

    // ── sound ────────────────────────────────────────────────────────────

    pub(super) async fn volume(
        &mut self,
        op: LevelOp,
        value: Option<&SlotNum>,
        app: Option<&str>,
    ) -> ComputerResult<StepOut> {
        let v = num(value, default_value(op))?.min(400);
        if let Some(app) = app.filter(|a| !a.trim().is_empty()) {
            let a = app.to_owned();
            let (p, m) = match mute_of(op) {
                Some(m) => self.pc(PcRequest::SetAppMute(a, m)).await?.level()?,
                None => {
                    let (cur, _) = self.pc(PcRequest::GetAppVolume(a.clone())).await?.level()?;
                    let target = level_after(op, cur, v, 100).unwrap_or(cur);
                    self.pc(PcRequest::SetAppVolume(a, target)).await?.level()?
                }
            };
            return Ok(StepOut::Done(format!("{app} volume {p}%{}", if m { " (muted)" } else { "" })));
        }
        let cur = match self.pc(PcRequest::GetVolume).await {
            Ok(r) => r.level()?,
            // no Core Audio: the volume keys (2 % a press)
            Err(ComputerError::Unsupported(_)) => return self.volume_keys(op, v).await,
            Err(e) => return Err(e),
        };
        let (p, m) = match mute_of(op) {
            Some(m) => self.pc(PcRequest::SetMute(m)).await?.level()?,
            None => {
                let target = level_after(op, cur.0, v, 100).unwrap_or(cur.0);
                let r = self.pc(PcRequest::SetVolume(target)).await?.level()?;
                // "громкость 10%" while muted means: make it audible
                if r.1 && target > 0 {
                    self.pc(PcRequest::SetMute(Some(false))).await?.level()?
                } else {
                    r
                }
            }
        };
        Ok(StepOut::Done(format!("volume {p}%{}", if m { " (muted)" } else { "" })))
    }

    async fn volume_keys(&mut self, op: LevelOp, v: u32) -> ComputerResult<StepOut> {
        #[allow(clippy::cast_possible_wrap)]
        let v = v.min(100) as i32;
        self.call(move |b| match op {
            LevelOp::Up => b.volume_step(v, false),
            LevelOp::Down => b.volume_step(-v, false),
            LevelOp::Mute | LevelOp::MuteOn | LevelOp::MuteOff => b.volume_step(0, true),
            LevelOp::Set | LevelOp::Scale => {
                b.volume_step(-100, false)?;
                if v > 0 && op == LevelOp::Set {
                    b.volume_step(v, false)?;
                }
                Ok(())
            }
        })
        .await?;
        Ok(StepOut::Done(format!("{op:?} {v}%")))
    }

    pub(super) async fn microphone(&mut self, op: LevelOp, value: Option<&SlotNum>) -> ComputerResult<StepOut> {
        let v = num(value, default_value(op))?.min(400);
        let (p, m) = match mute_of(op) {
            Some(m) => self.pc(PcRequest::SetMicMute(m)).await?.level()?,
            None => {
                let (cur, _) = self.pc(PcRequest::GetMic).await?.level()?;
                self.pc(PcRequest::SetMic(level_after(op, cur, v, 100).unwrap_or(cur))).await?.level()?
            }
        };
        Ok(StepOut::Done(format!("microphone {p}%{}", if m { " (muted)" } else { "" })))
    }

    pub(super) async fn audio_device(&mut self, kind: AudioDeviceKind, name: Option<&str>) -> ComputerResult<StepOut> {
        let list = self.pc(PcRequest::AudioDevices(kind)).await?.devices()?;
        if list.is_empty() {
            return Err(ComputerError::NotFound("no audio devices".into()));
        }
        let target = match name.filter(|n| !n.trim().is_empty()) {
            Some(n) => pick_device(&list, n).cloned().ok_or_else(|| {
                ComputerError::NotFound(format!(
                    "no audio device like “{n}” (available: {})",
                    list.iter().map(|d| d.name.as_str()).collect::<Vec<_>>().join(", ")
                ))
            })?,
            None => {
                let i = list.iter().position(|d| d.default).map_or(0, |i| (i + 1) % list.len());
                list[i].clone()
            }
        };
        self.pc(PcRequest::SetDefaultAudioDevice(kind, target.id.clone())).await?;
        Ok(StepOut::Done(target.name))
    }

    pub(super) async fn media_seek(&mut self, forward: bool, seconds: Option<&SlotNum>) -> ComputerResult<StepOut> {
        let s = i64::from(num(seconds, 10)?.clamp(1, 3600));
        self.pc(PcRequest::MediaSeek(if forward { s } else { -s })).await?;
        Ok(StepOut::Done(format!("{}{s} s", if forward { "+" } else { "-" })))
    }

    // ── windows / desktops ───────────────────────────────────────────────

    pub(super) async fn window_geometry(&mut self, op: WindowOp, app: Option<&str>) -> ComputerResult<StepOut> {
        if matches!(op, WindowOp::MinimizeAll | WindowOp::RestoreAll) {
            let keys: Vec<String> = if op == WindowOp::MinimizeAll {
                vec!["win".into(), "m".into()]
            } else {
                vec!["win".into(), "shift".into(), "m".into()]
            };
            self.call(move |b| b.press(&keys)).await?;
            return Ok(StepOut::Done(format!("{op:?}")));
        }
        let w = self.target_window(app).await?;
        let h = w.handle.clone();
        if matches!(op, WindowOp::AlwaysOnTop | WindowOp::NotOnTop) {
            self.pc(PcRequest::SetTopmost(h, op == WindowOp::AlwaysOnTop)).await?;
            return Ok(StepOut::Done(format!("{op:?}: {}", w.title)));
        }
        let work = self.pc(PcRequest::WorkArea(Some(h.clone()))).await?.rect()?;
        let cur = self.pc(PcRequest::WindowRect(h.clone())).await?.rect()?;
        let r = snap_rect(op, work, cur).ok_or_else(|| ComputerError::Invalid(format!("{op:?}")))?;
        self.pc(PcRequest::PlaceWindow(h, r)).await?;
        Ok(StepOut::Done(format!("{op:?}: {}", w.title)))
    }

    pub(super) async fn window_resize(
        &mut self,
        app: Option<&str>,
        percent: Option<&SlotNum>,
        width: Option<&SlotNum>,
        height: Option<&SlotNum>,
    ) -> ComputerResult<StepOut> {
        let w = self.target_window(app).await?;
        let h = w.handle.clone();
        let work = self.pc(PcRequest::WorkArea(Some(h.clone()))).await?.rect()?;
        let cur = self.pc(PcRequest::WindowRect(h.clone())).await?.rect()?;
        let r = if let Some(p) = percent {
            let p = i32::try_from(num(Some(p), 70)?.clamp(10, 100)).unwrap_or(70);
            centered(work, work.width() * p / 100, work.height() * p / 100)
        } else {
            let ww =
                width.map(|v| num(Some(v), 800)).transpose()?.map_or(cur.width(), |v| i32::try_from(v).unwrap_or(800));
            let hh = height
                .map(|v| num(Some(v), 600))
                .transpose()?
                .map_or(cur.height(), |v| i32::try_from(v).unwrap_or(600));
            centered(work, ww, hh)
        };
        self.pc(PcRequest::PlaceWindow(h, r)).await?;
        Ok(StepOut::Done(format!("{}×{}: {}", r.width(), r.height(), w.title)))
    }

    pub(super) async fn window_opacity(&mut self, app: Option<&str>, percent: &SlotNum) -> ComputerResult<StepOut> {
        let p = num(Some(percent), 100)?.clamp(20, 100);
        let w = self.target_window(app).await?;
        self.pc(PcRequest::SetOpacity(w.handle.clone(), p)).await?;
        Ok(StepOut::Done(format!("opacity {p}%: {}", w.title)))
    }

    pub(super) async fn desktop(&mut self, op: DesktopOp) -> ComputerResult<StepOut> {
        let combo: &[&str] = match op {
            DesktopOp::Next => &["win", "ctrl", "right"],
            DesktopOp::Previous => &["win", "ctrl", "left"],
            DesktopOp::New => &["win", "ctrl", "d"],
            DesktopOp::Close => &["win", "ctrl", "f4"],
            DesktopOp::TaskView => &["win", "tab"],
        };
        let keys: Vec<String> = combo.iter().map(|s| (*s).to_owned()).collect();
        self.call(move |b| b.press(&keys)).await?;
        Ok(StepOut::Done(format!("{op:?}")))
    }

    // ── cursor & mouse ───────────────────────────────────────────────────

    pub(super) async fn mouse_move(
        &mut self,
        d: ScrollDirection,
        distance: Option<&SlotNum>,
    ) -> ComputerResult<StepOut> {
        let n = i32::try_from(num(distance, 100)?.clamp(1, 4000)).unwrap_or(100);
        let (x, y) = self.pc(PcRequest::CursorPos).await?.point()?;
        let (dx, dy) = offset(d, n);
        self.pc(PcRequest::SetCursor(x + dx, y + dy)).await?;
        Ok(StepOut::Done(json!({ "x": x + dx, "y": y + dy }).to_string()))
    }

    pub(super) async fn mouse_to(&mut self, p: ScreenPoint) -> ComputerResult<StepOut> {
        let b = self.pc(PcRequest::MonitorBounds).await?.rect()?;
        let (x, y) = screen_point(p, b);
        self.pc(PcRequest::SetCursor(x, y)).await?;
        Ok(StepOut::Done(json!({ "x": x, "y": y }).to_string()))
    }

    async fn element(&mut self, name: &str) -> ComputerResult<(i32, i32, String)> {
        let w = self.ensure_target().await?;
        let r = self.pc(PcRequest::ElementCenter(w.map(|w| w.handle), name.to_owned())).await?;
        match r {
            PcReply::Element(x, y, label) => Ok((x, y, label)),
            other => {
                let (x, y) = other.point()?;
                Ok((x, y, name.to_owned()))
            }
        }
    }

    pub(super) async fn mouse_to_element(&mut self, name: &str) -> ComputerResult<StepOut> {
        let (x, y, label) = self.element(name).await?;
        self.pc(PcRequest::SetCursor(x, y)).await?;
        Ok(StepOut::Done(label))
    }

    pub(super) async fn mouse_hold(&mut self, button: MouseButton, release: bool) -> ComputerResult<StepOut> {
        if !release {
            self.ensure_target().await?;
        }
        self.pc(PcRequest::MouseButton(button, !release)).await?;
        HELD.store(!release, Ordering::SeqCst);
        Ok(StepOut::Done(format!("{button:?} {}", if release { "released" } else { "held" })))
    }

    pub(super) async fn mouse_drag(
        &mut self,
        direction: Option<ScrollDirection>,
        distance: Option<&SlotNum>,
        element: Option<&str>,
    ) -> ComputerResult<StepOut> {
        let (x0, y0) = self.pc(PcRequest::CursorPos).await?.point()?;
        let (x1, y1) = match (element.filter(|e| !e.trim().is_empty()), direction) {
            (Some(e), _) => {
                let (x, y, _) = self.element(e).await?;
                (x, y)
            }
            (None, Some(d)) => {
                let n = i32::try_from(num(distance, 200)?.clamp(1, 4000)).unwrap_or(200);
                let (dx, dy) = offset(d, n);
                (x0 + dx, y0 + dy)
            }
            (None, None) => return Err(ComputerError::Invalid("drag needs a direction or a target element".into())),
        };
        self.ensure_target().await?;
        self.pc(PcRequest::MouseButton(MouseButton::Left, true)).await?;
        HELD.store(true, Ordering::SeqCst);
        let mut result = Ok(());
        for i in 1..=12 {
            let (x, y) = (x0 + (x1 - x0) * i / 12, y0 + (y1 - y0) * i / 12);
            if let Err(e) = self.pc(PcRequest::SetCursor(x, y)).await {
                result = Err(e);
                break;
            }
            if let Err(e) = self.sleep(15).await {
                result = Err(e);
                break;
            }
        }
        // always release, even when stopped half-way
        let b = std::sync::Arc::clone(&self.env.backend);
        let _ = tokio::task::spawn_blocking(move || b.pc(&PcRequest::MouseButton(MouseButton::Left, false))).await;
        HELD.store(false, Ordering::SeqCst);
        result?;
        Ok(StepOut::Done(json!({ "x": x1, "y": y1 }).to_string()))
    }

    pub(super) async fn mouse_speed(&mut self, op: LevelOp, value: Option<&SlotNum>) -> ComputerResult<StepOut> {
        if mute_of(op).is_some() {
            return Err(ComputerError::Invalid("pointer speed cannot be muted".into()));
        }
        let (cur, _) = self.pc(PcRequest::GetMouseSpeed).await?.level()?;
        let v = num(value, if op == LevelOp::Scale { 50 } else { 2 })?;
        let target = level_after(op, cur, v, 20).unwrap_or(cur).clamp(1, 20);
        let (now, _) = self.pc(PcRequest::SetMouseSpeed(target)).await?.level()?;
        Ok(StepOut::Done(format!("pointer speed {now}/20")))
    }

    pub(super) async fn locate_cursor(&mut self) -> ComputerResult<StepOut> {
        let (x, y) = self.pc(PcRequest::CursorPos).await?.point()?;
        // a small circle around the pointer, back to where it was
        for i in 0..=24 {
            let a = f64::from(i) * std::f64::consts::TAU / 24.0;
            #[allow(clippy::cast_possible_truncation)]
            let (dx, dy) = ((a.cos() * 40.0) as i32, (a.sin() * 40.0) as i32);
            self.pc(PcRequest::SetCursor(x + dx, y + dy)).await?;
            self.sleep(12).await?;
        }
        self.pc(PcRequest::SetCursor(x, y)).await?;
        self.effects.push(ClientEffect::Show { title: "cursor".into(), lines: vec![format!("x {x}, y {y}")] });
        Ok(StepOut::Done(json!({ "x": x, "y": y }).to_string()))
    }

    // ── keyboard / display / radios ──────────────────────────────────────

    pub(super) async fn keyboard_layout(&mut self, lang: Option<&str>) -> ComputerResult<StepOut> {
        self.ensure_target().await?;
        let now = self.pc(PcRequest::SetKeyboardLayout(lang.map(str::to_owned))).await?.text()?;
        Ok(StepOut::Done(now))
    }

    pub(super) async fn display(&mut self, op: DisplayOp) -> ComputerResult<StepOut> {
        self.pc(PcRequest::Display(op)).await?;
        Ok(StepOut::Done(format!("{op:?}")))
    }

    pub(super) async fn radio(&mut self, kind: RadioKind, on: bool) -> ComputerResult<StepOut> {
        let names = self.pc(PcRequest::Radio(kind, on)).await?.text()?;
        Ok(StepOut::Done(names))
    }

    // ── screenshots ──────────────────────────────────────────────────────

    pub(super) async fn screenshot_ex(
        &mut self,
        path: Option<&str>,
        scope: ScreenshotScope,
        clipboard: bool,
    ) -> ComputerResult<StepOut> {
        if scope == ScreenshotScope::Region {
            // the Snipping Tool overlay: the user selects, the image lands on the clipboard
            self.call(|b| b.press(&["win".into(), "shift".into(), "s".into()])).await?;
            return Ok(StepOut::Done("region".into()));
        }
        let file = match (path, clipboard) {
            (Some(p), _) => Some(crate::apps::file_path(p)),
            (None, true) => None,
            (None, false) => Some(
                self.env.screenshot_dir.join(format!("orkestr-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S"))),
            ),
        };
        let rect = match scope {
            ScreenshotScope::Window => {
                let w = self.user_window().await?.ok_or_else(|| ComputerError::NotFound("no window".into()))?;
                self.pc(PcRequest::WindowRect(w.handle)).await?.rect()?
            }
            _ => self.pc(PcRequest::MonitorBounds).await?.rect()?,
        };
        self.pc(PcRequest::CaptureRect(rect, file.clone(), clipboard)).await?;
        if let Some(f) = &file {
            self.effects.push(ClientEffect::Screenshot { path: f.display().to_string() });
        }
        Ok(StepOut::Done(file.map_or_else(|| "clipboard".to_owned(), |f| f.display().to_string())))
    }

    // ── reports ──────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    pub(super) async fn query(&mut self, what: QueryKind) -> ComputerResult<StepOut> {
        let (value, lines): (serde_json::Value, Vec<String>) = match what {
            QueryKind::DateTime | QueryKind::Date | QueryKind::Time => {
                let now = chrono::Local::now();
                (
                    json!({
                        "iso": now.to_rfc3339(),
                        "date": now.format("%Y-%m-%d").to_string(),
                        "time": now.format("%H:%M").to_string(),
                        "weekday": now.format("%A").to_string(),
                    }),
                    vec![now.format("%Y-%m-%d %H:%M").to_string()],
                )
            }
            QueryKind::Battery => {
                let b = self.pc(PcRequest::Battery).await?.battery()?;
                (
                    json!({ "percent": b.percent, "charging": b.charging, "plugged": b.plugged }),
                    vec![b.percent.map_or_else(|| "—".to_owned(), |p| format!("{p}%"))],
                )
            }
            QueryKind::Volume => {
                let (p, m) = self.pc(PcRequest::GetVolume).await?.level()?;
                (json!({ "percent": p, "muted": m }), vec![format!("{p}%{}", if m { " (mute)" } else { "" })])
            }
            QueryKind::Brightness => {
                let p = self.call(|b| b.brightness(None)).await?;
                (json!({ "percent": p }), vec![format!("{p}%")])
            }
            QueryKind::NowPlaying => match self.pc(PcRequest::NowPlaying).await?.media()? {
                Some(m) => {
                    let line =
                        if m.artist.is_empty() { m.title.clone() } else { format!("{} — {}", m.artist, m.title) };
                    (json!({ "title": m.title, "artist": m.artist, "app": m.app, "playing": m.playing }), vec![line])
                }
                None => (json!({ "none": true }), Vec::new()),
            },
            QueryKind::ActiveWindow => {
                let w = self.user_window().await?.ok_or_else(|| ComputerError::NotFound("no window".into()))?;
                (json!({ "title": w.title, "process": w.process }), vec![format!("{} ({})", w.title, w.process)])
            }
            QueryKind::CursorPosition => {
                let (x, y) = self.pc(PcRequest::CursorPos).await?.point()?;
                (json!({ "x": x, "y": y }), vec![format!("x {x}, y {y}")])
            }
            QueryKind::AudioDevices => {
                let outs = self.pc(PcRequest::AudioDevices(AudioDeviceKind::Output)).await?.devices()?;
                let ins = self.pc(PcRequest::AudioDevices(AudioDeviceKind::Input)).await?.devices()?;
                let def = |l: &[crate::backend::AudioDeviceInfo]| l.iter().find(|d| d.default).map(|d| d.name.clone());
                let mut lines: Vec<String> =
                    outs.iter().map(|d| format!("🔈 {}{}", d.name, if d.default { " ✓" } else { "" })).collect();
                lines.extend(ins.iter().map(|d| format!("🎙 {}{}", d.name, if d.default { " ✓" } else { "" })));
                (
                    json!({
                        "outputs": outs.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
                        "inputs": ins.iter().map(|d| d.name.clone()).collect::<Vec<_>>(),
                        "defaultOutput": def(&outs),
                        "defaultInput": def(&ins),
                    }),
                    lines,
                )
            }
            QueryKind::AudioSessions => {
                let s = self.pc(PcRequest::AudioSessions).await?.sessions()?;
                (
                    json!({ "apps": s.iter().map(|x| json!({ "app": x.app, "percent": x.percent, "muted": x.muted })).collect::<Vec<_>>() }),
                    s.iter()
                        .map(|x| format!("{} {}%{}", x.app, x.percent, if x.muted { " (mute)" } else { "" }))
                        .collect(),
                )
            }
            QueryKind::KeyboardLayout => {
                let l = self.pc(PcRequest::GetKeyboardLayout).await?.text()?;
                (json!({ "layout": l }), vec![l])
            }
            QueryKind::DiskSpace
            | QueryKind::IpAddress
            | QueryKind::Network
            | QueryKind::SystemInfo
            | QueryKind::Uptime => {
                let task = tokio::task::spawn_blocking(move || system_query(what));
                tokio::select! {
                    r = task => r.map_err(|e| ComputerError::Backend(format!("system info: {e}")))?,
                    () = self.env.cancel.cancelled() => return Err(ComputerError::Stopped),
                }
            }
        };
        if !lines.is_empty() {
            self.effects.push(ClientEffect::Show { title: format!("query.{what:?}"), lines });
        }
        Ok(StepOut::Done(value.to_string()))
    }

    // ── alarms / files ───────────────────────────────────────────────────

    pub(super) fn alarm(&mut self, at: &str, message: &str) -> ComputerResult<StepOut> {
        let (h, m) = crate::text::parse_clock(at)
            .ok_or_else(|| ComputerError::Invalid(format!("“{at}” is not a time of day")))?;
        let secs = seconds_until(chrono::Local::now().naive_local(), h, m);
        let text = if message.trim().is_empty() { format!("{h:02}:{m:02}") } else { message.trim().to_owned() };
        (self.env.timers)(secs, text);
        Ok(StepOut::Done(json!({ "at": format!("{h:02}:{m:02}"), "seconds": secs }).to_string()))
    }

    pub(super) async fn file_copy(&mut self, from: &str, to: &str) -> ComputerResult<StepOut> {
        let (a, mut b) = (crate::apps::file_path(from), crate::apps::file_path(to));
        if !a.exists() {
            return Err(ComputerError::NotFound(format!("{} does not exist", a.display())));
        }
        if b.is_dir() {
            b = b.join(a.file_name().unwrap_or_default());
        }
        if b.exists() {
            return Err(ComputerError::Conflict(format!("{} already exists", b.display())));
        }
        if b.starts_with(&a) {
            return Err(ComputerError::Invalid("cannot copy a folder into itself".into()));
        }
        let (aa, bb) = (a.clone(), b.clone());
        let n = tokio::task::spawn_blocking(move || copy_tree(&aa, &bb))
            .await
            .map_err(|e| ComputerError::Backend(format!("copy: {e}")))??;
        Ok(StepOut::Done(format!("{} → {} ({n} file(s))", a.display(), b.display())))
    }

    pub(super) async fn file_search(&mut self, name: &str, folder: Option<&str>) -> ComputerResult<StepOut> {
        if name.trim().chars().count() < 2 {
            return Err(ComputerError::Invalid("the file name to search is too short".into()));
        }
        let root = search_root(folder);
        let needle = name.to_owned();
        let found = tokio::task::spawn_blocking(move || search_files(&root, &needle, Duration::from_secs(4)))
            .await
            .map_err(|e| ComputerError::Backend(format!("search: {e}")))?;
        let lines: Vec<String> = found.iter().map(|p| p.display().to_string()).collect();
        let n = lines.len();
        self.effects.push(ClientEffect::Show { title: "files".into(), lines });
        Ok(StepOut::Done(json!({ "count": n, "name": name }).to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn levels() {
        assert_eq!(level_after(LevelOp::Up, 40, 10, 100), Some(50));
        assert_eq!(level_after(LevelOp::Up, 95, 10, 100), Some(100));
        assert_eq!(level_after(LevelOp::Down, 5, 10, 100), Some(0));
        assert_eq!(level_after(LevelOp::Set, 40, 10, 100), Some(10));
        assert_eq!(level_after(LevelOp::Set, 40, 250, 100), Some(100));
        assert_eq!(level_after(LevelOp::Scale, 40, 50, 100), Some(20));
        assert_eq!(level_after(LevelOp::Scale, 40, 200, 100), Some(80));
        assert_eq!(level_after(LevelOp::Scale, 0, 200, 100), Some(10));
        assert_eq!(level_after(LevelOp::Mute, 40, 0, 100), None);
        assert_eq!(level_after(LevelOp::Up, 18, 2, 20), Some(20));
    }

    #[test]
    fn snaps_fill_the_work_area() {
        let work = Rect::xywh(0, 0, 1920, 1040);
        let cur = Rect::xywh(10, 10, 800, 600);
        assert_eq!(snap_rect(WindowOp::SnapLeft, work, cur), Some(Rect::xywh(0, 0, 960, 1040)));
        assert_eq!(snap_rect(WindowOp::SnapTopRight, work, cur), Some(Rect::xywh(960, 0, 960, 520)));
        assert_eq!(snap_rect(WindowOp::SnapBottomLeft, work, cur), Some(Rect::xywh(0, 520, 960, 520)));
        assert_eq!(snap_rect(WindowOp::Center, work, cur), Some(Rect::xywh(560, 220, 800, 600)));
        assert_eq!(snap_rect(WindowOp::Focus, work, cur), None);
        // second monitor at the right
        let w2 = Rect::xywh(1920, 0, 2560, 1400);
        assert_eq!(snap_rect(WindowOp::SnapBottom, w2, cur), Some(Rect::xywh(1920, 700, 2560, 700)));
        assert_eq!(centered(work, 50, 50), Rect::xywh(860, 445, 200, 150));
    }

    #[test]
    fn screen_points_stay_inside() {
        let b = Rect::xywh(0, 0, 1920, 1080);
        assert_eq!(screen_point(ScreenPoint::Center, b), (960, 540));
        assert_eq!(screen_point(ScreenPoint::TopLeft, b), (5, 5));
        assert_eq!(screen_point(ScreenPoint::BottomRight, b), (1914, 1074));
        assert_eq!(screen_point(ScreenPoint::Right, b), (1914, 540));
    }

    #[test]
    fn devices_by_spoken_name() {
        let dev =
            |id: &str, name: &str| crate::backend::AudioDeviceInfo { id: id.into(), name: name.into(), default: false };
        let list = vec![
            dev("1", "Speakers (Realtek High Definition Audio)"),
            dev("2", "Headphones (WH-1000XM4)"),
            dev("3", "LG ULTRAGEAR (NVIDIA High Definition Audio)"),
        ];
        assert_eq!(pick_device(&list, "наушники").map(|d| d.id.as_str()), Some("2"));
        assert_eq!(pick_device(&list, "на наушники").map(|d| d.id.as_str()), Some("2"));
        assert_eq!(pick_device(&list, "колонки").map(|d| d.id.as_str()), Some("1"));
        assert_eq!(pick_device(&list, "монитор").map(|d| d.id.as_str()), Some("3"));
        assert_eq!(pick_device(&list, "realtek").map(|d| d.id.as_str()), Some("1"));
        assert_eq!(pick_device(&list, "телефон"), None);
    }

    #[test]
    fn alarm_times_roll_over_midnight() {
        let now = chrono::NaiveDate::from_ymd_opt(2026, 10, 30).unwrap().and_hms_opt(22, 0, 0).unwrap();
        assert_eq!(seconds_until(now, 23, 30), 5400);
        assert_eq!(seconds_until(now, 7, 0), 9 * 3600);
        assert_eq!(seconds_until(now, 22, 0), 24 * 3600);
    }

    #[test]
    fn file_search_finds_by_fragment() {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join("a/b")).unwrap();
        std::fs::write(d.path().join("a/b/Отчёт-2026.txt"), "x").unwrap();
        std::fs::write(d.path().join("a/other.txt"), "x").unwrap();
        std::fs::create_dir_all(d.path().join("node_modules")).unwrap();
        std::fs::write(d.path().join("node_modules/отчет.js"), "x").unwrap();
        let r = search_files(d.path(), "отчет", Duration::from_secs(2));
        assert_eq!(r.len(), 1, "{r:?}");
        assert!(r[0].ends_with("Отчёт-2026.txt"));
    }

    #[test]
    fn system_queries_answer() {
        let (v, _) = system_query(QueryKind::DiskSpace);
        assert!(v["disks"].is_array());
        let (v, lines) = system_query(QueryKind::SystemInfo);
        assert!(v["ramTotalGb"].as_f64().unwrap_or(0.0) > 0.0, "{v}");
        assert_eq!(lines.len(), 2);
        let (v, _) = system_query(QueryKind::Uptime);
        assert!(v["seconds"].as_u64().is_some());
    }
}
