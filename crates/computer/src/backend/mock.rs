//! Recording backend for tests (no OS side effects).

use std::path::Path;

use parking_lot::Mutex;

use orkestr_protocol::methods::computer::{
    MediaKey, MouseButton, ProcessInfo, ScrollDirection, ScrollUnit, SystemOp, WindowInfo, WindowOp,
};

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
}
