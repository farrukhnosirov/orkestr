//! Recording backend for tests (no OS side effects).

use std::path::Path;

use parking_lot::Mutex;

use orkestr_protocol::methods::computer::{
    AudioDeviceKind, MediaKey, MouseButton, ProcessInfo, ScrollDirection, ScrollUnit, SystemOp, WindowInfo, WindowOp,
};

use super::pc::{AudioDeviceInfo, AudioSessionInfo, BatteryInfo, MediaInfo, PcReply, PcRequest, Rect};
use super::{ComputerBackend, InstalledApp};
use crate::error::{ComputerError, ComputerResult};

/// A recorded call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockCall {
    /// `shell_open(target, args)`.
    Open(String, Option<String>),
    /// `window_op`.
    Window(Option<String>, WindowOp),
    /// `press`.
    Press(Vec<String>),
    /// `type_text`.
    Type(String),
    /// `scroll`.
    Scroll(ScrollDirection, u32, ScrollUnit),
    /// `mouse_click`.
    Click(MouseButton),
    /// `click_element`.
    ClickElement(String),
    /// `media`.
    Media(MediaKey),
    /// `volume_step`.
    Volume(i32, bool),
    /// `brightness`.
    Brightness(Option<u32>),
    /// `system`.
    System(SystemOp),
    /// `screenshot`.
    Screenshot(String),
    /// `clipboard_set`.
    ClipboardSet(String),
    /// `notify`.
    Notify(String, String),
    /// `kill`.
    Kill(u32),
    /// `recycle`.
    Recycle(String),
    /// `pc` (extended control).
    Pc(PcRequest),
}

/// Simulated machine state for [`ComputerBackend::pc`].
#[derive(Debug, Clone)]
pub struct MockPc {
    /// Output volume percent + muted.
    pub volume: (u32, bool),
    /// Microphone percent + muted.
    pub mic: (u32, bool),
    /// App sessions.
    pub sessions: Vec<AudioSessionInfo>,
    /// Output devices.
    pub outputs: Vec<AudioDeviceInfo>,
    /// Input devices.
    pub inputs: Vec<AudioDeviceInfo>,
    /// Media session.
    pub media: Option<MediaInfo>,
    /// Cursor.
    pub cursor: (i32, i32),
    /// Work area.
    pub work_area: Rect,
    /// Monitor bounds.
    pub monitor: Rect,
    /// Window rects by handle (default 100,100 800×600).
    pub rects: std::collections::HashMap<String, Rect>,
    /// Pointer speed.
    pub mouse_speed: u32,
    /// Keyboard layout.
    pub layout: String,
    /// Battery.
    pub battery: BatteryInfo,
    /// Buttons held.
    pub held: Vec<MouseButton>,
}

impl Default for MockPc {
    fn default() -> Self {
        let dev = |id: &str, name: &str, default: bool| AudioDeviceInfo { id: id.into(), name: name.into(), default };
        Self {
            volume: (40, false),
            mic: (70, false),
            sessions: vec![
                AudioSessionInfo { app: "spotify".into(), pid: 501, percent: 100, muted: false },
                AudioSessionInfo { app: "chrome".into(), pid: 502, percent: 80, muted: false },
            ],
            outputs: vec![dev("out-1", "Динамики (Realtek)", true), dev("out-2", "Наушники (USB Headset)", false)],
            inputs: vec![dev("in-1", "Микрофон (Realtek)", true), dev("in-2", "Микрофон (USB Headset)", false)],
            media: Some(MediaInfo {
                title: "Back in Black".into(),
                artist: "AC/DC".into(),
                app: "Spotify.exe".into(),
                playing: true,
            }),
            cursor: (500, 400),
            work_area: Rect::xywh(0, 0, 1920, 1040),
            monitor: Rect::xywh(0, 0, 1920, 1080),
            rects: std::collections::HashMap::new(),
            mouse_speed: 10,
            layout: "en-US".into(),
            battery: BatteryInfo { percent: Some(76), charging: true, plugged: true },
            held: Vec::new(),
        }
    }
}

/// Recording backend.
#[derive(Debug, Default)]
pub struct MockBackend {
    /// Calls so far.
    pub calls: Mutex<Vec<MockCall>>,
    /// Windows returned by `windows()`.
    pub windows: Mutex<Vec<WindowInfo>>,
    /// Processes returned by `processes()`.
    pub processes: Mutex<Vec<ProcessInfo>>,
    /// Start-menu apps.
    pub apps: Mutex<Vec<InstalledApp>>,
    /// Focused control is a password field.
    pub password_focused: Mutex<bool>,
    /// Clipboard.
    pub clipboard: Mutex<String>,
    /// `shell_open` fails for targets containing this text.
    pub fail_open: Mutex<Option<String>>,
    /// Extended state (`pc`).
    pub pc: Mutex<MockPc>,
}

impl MockBackend {
    /// Calls so far (clone).
    #[must_use]
    pub fn take_calls(&self) -> Vec<MockCall> {
        std::mem::take(&mut *self.calls.lock())
    }
    fn rec(&self, c: MockCall) {
        self.calls.lock().push(c);
    }
    /// Adds a window.
    pub fn add_window(&self, handle: &str, title: &str, process: &str, active: bool) {
        let mut w = self.windows.lock();
        if active {
            for x in w.iter_mut() {
                x.active = false;
            }
        }
        let pid = 100 + u32::try_from(w.len()).unwrap_or(0);
        w.push(WindowInfo {
            handle: handle.into(),
            title: title.into(),
            process: process.into(),
            pid,
            active,
            minimized: false,
        });
    }
}

impl ComputerBackend for MockBackend {
    fn name(&self) -> &'static str {
        "mock"
    }
    fn available(&self) -> ComputerResult<()> {
        Ok(())
    }
    fn shell_open(&self, target: &str, args: Option<&str>) -> ComputerResult<()> {
        if self.fail_open.lock().as_deref().is_some_and(|f| target.contains(f)) {
            return Err(ComputerError::Backend(format!("cannot open {target}")));
        }
        self.rec(MockCall::Open(target.into(), args.map(str::to_owned)));
        Ok(())
    }
    fn installed_apps(&self) -> ComputerResult<Vec<InstalledApp>> {
        Ok(self.apps.lock().clone())
    }
    fn windows(&self) -> ComputerResult<Vec<WindowInfo>> {
        Ok(self.windows.lock().clone())
    }
    fn window_op(&self, handle: Option<&str>, op: WindowOp) -> ComputerResult<()> {
        if op == WindowOp::Close {
            if let Some(h) = handle {
                self.windows.lock().retain(|w| w.handle != h);
            }
        }
        if op == WindowOp::Focus {
            if let Some(h) = handle {
                for w in self.windows.lock().iter_mut() {
                    w.active = w.handle == h;
                }
            }
        }
        self.rec(MockCall::Window(handle.map(str::to_owned), op));
        Ok(())
    }
    fn press(&self, keys: &[String]) -> ComputerResult<()> {
        self.rec(MockCall::Press(keys.to_vec()));
        Ok(())
    }
    fn type_text(&self, text: &str) -> ComputerResult<()> {
        self.rec(MockCall::Type(text.into()));
        Ok(())
    }
    fn scroll(&self, d: ScrollDirection, amount: u32, unit: ScrollUnit) -> ComputerResult<()> {
        self.rec(MockCall::Scroll(d, amount, unit));
        Ok(())
    }
    fn mouse_click(&self, button: MouseButton, _: Option<(i32, i32)>) -> ComputerResult<()> {
        self.rec(MockCall::Click(button));
        Ok(())
    }
    fn click_element(&self, _: Option<&str>, name: &str) -> ComputerResult<String> {
        self.rec(MockCall::ClickElement(name.into()));
        Ok(name.into())
    }
    fn focused_is_password(&self) -> ComputerResult<bool> {
        Ok(*self.password_focused.lock())
    }
    fn media(&self, key: MediaKey) -> ComputerResult<()> {
        self.rec(MockCall::Media(key));
        Ok(())
    }
    fn volume_step(&self, d: i32, mute: bool) -> ComputerResult<()> {
        self.rec(MockCall::Volume(d, mute));
        Ok(())
    }
    fn brightness(&self, set: Option<u32>) -> ComputerResult<u32> {
        self.rec(MockCall::Brightness(set));
        Ok(set.unwrap_or(50))
    }
    fn system(&self, op: SystemOp) -> ComputerResult<()> {
        self.rec(MockCall::System(op));
        Ok(())
    }
    fn screenshot(&self, path: &Path) -> ComputerResult<()> {
        self.rec(MockCall::Screenshot(path.display().to_string()));
        Ok(())
    }
    fn clipboard_get(&self) -> ComputerResult<String> {
        Ok(self.clipboard.lock().clone())
    }
    fn clipboard_set(&self, text: &str) -> ComputerResult<()> {
        text.clone_into(&mut self.clipboard.lock());
        self.rec(MockCall::ClipboardSet(text.into()));
        Ok(())
    }
    fn notify(&self, title: &str, body: &str) -> ComputerResult<()> {
        self.rec(MockCall::Notify(title.into(), body.into()));
        Ok(())
    }
    fn processes(&self) -> ComputerResult<Vec<ProcessInfo>> {
        Ok(self.processes.lock().clone())
    }
    fn kill(&self, pid: u32) -> ComputerResult<()> {
        self.processes.lock().retain(|p| p.pid != pid);
        self.rec(MockCall::Kill(pid));
        Ok(())
    }
    fn recycle(&self, path: &Path) -> ComputerResult<()> {
        self.rec(MockCall::Recycle(path.display().to_string()));
        Ok(())
    }
    #[allow(clippy::too_many_lines)]
    fn pc(&self, req: &PcRequest) -> ComputerResult<PcReply> {
        self.rec(MockCall::Pc(req.clone()));
        let mut st = self.pc.lock();
        let toggle = |cur: bool, m: Option<bool>| m.unwrap_or(!cur);
        let session = |st: &MockPc, app: &str| -> ComputerResult<usize> {
            let a = app.to_lowercase();
            st.sessions
                .iter()
                .position(|s| s.app.contains(&a) || a.contains(&s.app))
                .ok_or_else(|| ComputerError::NotFound(format!("no app “{app}” is playing sound")))
        };
        Ok(match req {
            PcRequest::GetVolume => PcReply::Level(st.volume.0, st.volume.1),
            PcRequest::SetVolume(v) => {
                st.volume.0 = (*v).min(100);
                PcReply::Level(st.volume.0, st.volume.1)
            }
            PcRequest::SetMute(m) => {
                st.volume.1 = toggle(st.volume.1, *m);
                PcReply::Level(st.volume.0, st.volume.1)
            }
            PcRequest::GetAppVolume(app) => {
                let i = session(&st, app)?;
                PcReply::Level(st.sessions[i].percent, st.sessions[i].muted)
            }
            PcRequest::SetAppVolume(app, v) => {
                let i = session(&st, app)?;
                st.sessions[i].percent = (*v).min(100);
                PcReply::Level(st.sessions[i].percent, st.sessions[i].muted)
            }
            PcRequest::SetAppMute(app, m) => {
                let i = session(&st, app)?;
                st.sessions[i].muted = toggle(st.sessions[i].muted, *m);
                PcReply::Level(st.sessions[i].percent, st.sessions[i].muted)
            }
            PcRequest::GetMic => PcReply::Level(st.mic.0, st.mic.1),
            PcRequest::SetMic(v) => {
                st.mic.0 = (*v).min(100);
                PcReply::Level(st.mic.0, st.mic.1)
            }
            PcRequest::SetMicMute(m) => {
                st.mic.1 = toggle(st.mic.1, *m);
                PcReply::Level(st.mic.0, st.mic.1)
            }
            PcRequest::AudioDevices(k) => PcReply::Devices(match k {
                AudioDeviceKind::Output => st.outputs.clone(),
                AudioDeviceKind::Input => st.inputs.clone(),
            }),
            PcRequest::SetDefaultAudioDevice(k, id) => {
                let list = match k {
                    AudioDeviceKind::Output => &mut st.outputs,
                    AudioDeviceKind::Input => &mut st.inputs,
                };
                if !list.iter().any(|d| &d.id == id) {
                    return Err(ComputerError::NotFound(format!("no audio device {id}")));
                }
                for d in list.iter_mut() {
                    d.default = &d.id == id;
                }
                PcReply::Unit
            }
            PcRequest::AudioSessions => PcReply::Sessions(st.sessions.clone()),
            PcRequest::NowPlaying => PcReply::Media(st.media.clone()),
            PcRequest::MediaSeek(_) => {
                if st.media.is_none() {
                    return Err(ComputerError::NotFound("nothing is playing".into()));
                }
                PcReply::Unit
            }
            PcRequest::WorkArea(_) => PcReply::Rect(st.work_area),
            PcRequest::MonitorBounds => PcReply::Rect(st.monitor),
            PcRequest::WindowRect(h) => {
                PcReply::Rect(st.rects.get(h).copied().unwrap_or(Rect::xywh(100, 100, 800, 600)))
            }
            PcRequest::PlaceWindow(h, r) => {
                st.rects.insert(h.clone(), *r);
                PcReply::Unit
            }
            PcRequest::SetTopmost(..)
            | PcRequest::SetOpacity(..)
            | PcRequest::Display(_)
            | PcRequest::CaptureRect(..)
            | PcRequest::EmptyRecycleBin => PcReply::Unit,
            PcRequest::CursorPos => PcReply::Point(st.cursor.0, st.cursor.1),
            PcRequest::SetCursor(x, y) => {
                st.cursor = (*x, *y);
                PcReply::Unit
            }
            PcRequest::MouseButton(b, down) => {
                if *down {
                    st.held.push(*b);
                } else {
                    st.held.retain(|x| x != b);
                }
                PcReply::Unit
            }
            PcRequest::ElementCenter(_, name) => {
                if name.to_lowercase().contains("missing") {
                    return Err(ComputerError::NotFound(format!("no element “{name}”")));
                }
                PcReply::Element(300, 200, name.clone())
            }
            PcRequest::GetMouseSpeed => PcReply::Level(st.mouse_speed, false),
            PcRequest::SetMouseSpeed(v) => {
                st.mouse_speed = (*v).clamp(1, 20);
                PcReply::Level(st.mouse_speed, false)
            }
            PcRequest::GetKeyboardLayout => PcReply::Text(st.layout.clone()),
            PcRequest::SetKeyboardLayout(l) => {
                st.layout = match l.as_deref() {
                    Some(x) => x.to_owned(),
                    None if st.layout == "en-US" => "ru-RU".into(),
                    None => "en-US".into(),
                };
                PcReply::Text(st.layout.clone())
            }
            PcRequest::Radio(k, on) => PcReply::Text(format!("{k:?} {}", if *on { "on" } else { "off" })),
            PcRequest::Battery => PcReply::Battery(st.battery),
        })
    }
}
