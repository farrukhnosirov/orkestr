//! Windows backend: ShellExecuteW, SendInput, EnumWindows, UI Automation,
//! LockWorkStation / SetSuspendState; PowerShell (no window, script on
//! stdin, data as base64) for Start-menu apps, clipboard, toast
//! notifications, WMI brightness, screenshots and the recycle bin.
//!
//! This is the only module of the crate with `unsafe` (Win32 / COM FFI).
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::io::Write as _;
use std::os::windows::process::CommandExt as _;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::Duration;

use windows::core::{BOOL, HSTRING, PCWSTR, PWSTR};
use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoUninitialize, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
};
use windows::Win32::System::Power::SetSuspendState;
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::System::Threading::{
    OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION,
};
use windows::Win32::UI::Accessibility::{
    CUIAutomation, IUIAutomation, IUIAutomationElement, IUIAutomationInvokePattern, IUIAutomationSelectionItemPattern,
    IUIAutomationTogglePattern, TreeScope_Descendants, UIA_InvokePatternId, UIA_SelectionItemPatternId,
    UIA_TogglePatternId,
};
use windows::Win32::UI::HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetKeyState, GetKeyboardLayout, MapVirtualKeyW, SendInput, VkKeyScanExW, INPUT, INPUT_0, INPUT_KEYBOARD,
    INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP, KEYEVENTF_UNICODE,
    MAPVK_VK_TO_VSC, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
    VIRTUAL_KEY,
};
use windows::Win32::UI::Shell::ShellExecuteW;
use windows::Win32::UI::WindowsAndMessaging::{
    EnumChildWindows, EnumWindows, GetCursorPos, GetForegroundWindow, GetWindow, GetWindowLongW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, IsZoomed,
    PostMessageW, SetCursorPos, SetForegroundWindow, ShowWindow, GWL_EXSTYLE, GW_OWNER, SW_MAXIMIZE, SW_MINIMIZE,
    SW_RESTORE, SW_SHOWNORMAL, WM_CLOSE, WS_EX_TOOLWINDOW,
};

use orkestr_protocol::methods::computer::{
    MediaKey, MouseButton, ProcessInfo, ScrollDirection, ScrollUnit, SystemOp, WindowInfo, WindowOp,
};

use super::{ComputerBackend, InstalledApp};
use crate::error::{ComputerError, ComputerResult};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// The Windows backend.
#[derive(Debug)]
pub struct WindowsBackend;

impl Default for WindowsBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl WindowsBackend {
    /// Creates the backend (makes the process per-monitor DPI aware so
    /// UI Automation rectangles and cursor positions agree; a no-op when the
    /// shell already did).
    #[must_use]
    pub fn new() -> Self {
        // SAFETY: plain Win32 call without pointers; failure (already set) is fine.
        unsafe {
            let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        }
        Self
    }
}

pub(super) fn hwnd_of(handle: &str) -> ComputerResult<HWND> {
    let v = handle
        .strip_prefix("0x")
        .map_or_else(|| handle.parse::<isize>().ok(), |h| isize::from_str_radix(h, 16).ok())
        .ok_or_else(|| ComputerError::Invalid(format!("bad window handle `{handle}`")))?;
    let h = HWND(v as *mut core::ffi::c_void);
    // SAFETY: IsWindow accepts any value.
    if unsafe { IsWindow(Some(h)) }.as_bool() {
        Ok(h)
    } else {
        Err(ComputerError::NotFound(format!("window {handle} is gone")))
    }
}

pub(super) fn handle_str(h: HWND) -> String {
    format!("0x{:x}", h.0 as usize)
}

pub(super) fn window_title(h: HWND) -> String {
    // SAFETY: buffer sized from GetWindowTextLengthW (+1 for the terminator).
    unsafe {
        let len = GetWindowTextLengthW(h);
        if len <= 0 {
            return String::new();
        }
        let mut buf = vec![0u16; usize::try_from(len).unwrap_or(0) + 1];
        let n = GetWindowTextW(h, &mut buf);
        String::from_utf16_lossy(&buf[..usize::try_from(n).unwrap_or(0)])
    }
}

pub(super) fn window_pid(h: HWND) -> u32 {
    let mut pid = 0u32;
    // SAFETY: pid is a valid out pointer.
    unsafe {
        GetWindowThreadProcessId(h, Some(&raw mut pid));
    }
    pid
}

pub(super) fn process_name(pid: u32) -> String {
    // SAFETY: handle closed below; buffer length passed in/out.
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = vec![0u16; 1024];
        let mut size = u32::try_from(buf.len()).unwrap_or(1024);
        let ok = QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &raw mut size).is_ok();
        let _ = CloseHandle(h);
        if !ok {
            return String::new();
        }
        let full = String::from_utf16_lossy(&buf[..size as usize]);
        full.rsplit(['\\', '/']).next().unwrap_or_default().to_owned()
    }
}

pub(super) fn is_cloaked(h: HWND) -> bool {
    let mut cloaked = 0u32;
    // SAFETY: out buffer of the attribute's size.
    unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_CLOAKED,
            (&raw mut cloaked).cast(),
            u32::try_from(std::mem::size_of::<u32>()).unwrap_or(4),
        )
        .is_ok()
            && cloaked != 0
    }
}

/// UWP windows are hosted by ApplicationFrameHost: report the app's process.
fn hosted_app_pid(h: HWND, host_pid: u32) -> Option<u32> {
    struct Ctx {
        host: u32,
        found: Option<u32>,
    }
    unsafe extern "system" fn cb(child: HWND, lp: LPARAM) -> BOOL {
        // SAFETY: lp is the &mut Ctx passed below, alive for the enumeration.
        let ctx = unsafe { &mut *(lp.0 as *mut Ctx) };
        let pid = window_pid(child);
        if pid != ctx.host && pid != 0 {
            ctx.found = Some(pid);
            return BOOL(0);
        }
        BOOL(1)
    }
    let mut ctx = Ctx { host: host_pid, found: None };
    // SAFETY: callback only touches ctx during the call.
    unsafe {
        let _ = EnumChildWindows(Some(h), Some(cb), LPARAM(&raw mut ctx as isize));
    }
    ctx.found
}

pub(super) fn enum_top_windows() -> Vec<HWND> {
    unsafe extern "system" fn cb(h: HWND, lp: LPARAM) -> BOOL {
        // SAFETY: lp is the &mut Vec passed below.
        let v = unsafe { &mut *(lp.0 as *mut Vec<HWND>) };
        v.push(h);
        BOOL(1)
    }
    let mut v: Vec<HWND> = Vec::new();
    // SAFETY: callback only pushes into v during the call.
    unsafe {
        let _ = EnumWindows(Some(cb), LPARAM(&raw mut v as isize));
    }
    v
}

fn list_windows() -> Vec<WindowInfo> {
    // SAFETY: plain queries on handles returned by EnumWindows.
    let fg = unsafe { GetForegroundWindow() };
    let mut names: HashMap<u32, String> = HashMap::new();
    let mut out = Vec::new();
    for h in enum_top_windows() {
        unsafe {
            if !IsWindowVisible(h).as_bool() {
                continue;
            }
            if GetWindow(h, GW_OWNER).is_ok_and(|o| !o.is_invalid()) {
                continue;
            }
            #[allow(clippy::cast_sign_loss)]
            let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
            if ex & WS_EX_TOOLWINDOW.0 != 0 {
                continue;
            }
        }
        if is_cloaked(h) {
            continue;
        }
        let title = window_title(h);
        if title.is_empty() {
            continue;
        }
        let mut pid = window_pid(h);
        let mut process = names.entry(pid).or_insert_with(|| process_name(pid)).clone();
        if process.eq_ignore_ascii_case("ApplicationFrameHost.exe") {
            if let Some(app) = hosted_app_pid(h, pid) {
                pid = app;
                process = names.entry(app).or_insert_with(|| process_name(app)).clone();
            }
        }
        // SAFETY: valid handle.
        let minimized = unsafe { IsIconic(h) }.as_bool();
        out.push(WindowInfo { handle: handle_str(h), title, process, pid, active: h == fg, minimized });
    }
    out
}

// ---------------------------------------------------------------------------
// input
// ---------------------------------------------------------------------------

fn vk_of(name: &str) -> Option<(u16, bool)> {
    // (vk, extended)
    let v = match name {
        "ctrl" => (0x11, false),
        "shift" => (0x10, false),
        "alt" => (0x12, false),
        "win" => (0x5B, true),
        "enter" => (0x0D, false),
        "esc" => (0x1B, false),
        "tab" => (0x09, false),
        "space" => (0x20, false),
        "backspace" => (0x08, false),
        "delete" => (0x2E, true),
        "insert" => (0x2D, true),
        "home" => (0x24, true),
        "end" => (0x23, true),
        "pageup" => (0x21, true),
        "pagedown" => (0x22, true),
        "up" => (0x26, true),
        "down" => (0x28, true),
        "left" => (0x25, true),
        "right" => (0x27, true),
        "printscreen" => (0x2C, true),
        "capslock" => (0x14, false),
        "numlock" => (0x90, true),
        "scrolllock" => (0x91, false),
        "pause" => (0x13, false),
        "menu" => (0x5D, true),
        "volumeup" => (0xAF, true),
        "volumedown" => (0xAE, true),
        "volumemute" => (0xAD, true),
        "playpause" => (0xB3, true),
        "nexttrack" => (0xB0, true),
        "prevtrack" => (0xB1, true),
        "mediastop" => (0xB2, true),
        "browserback" => (0xA6, true),
        "browserforward" => (0xA7, true),
        "browserrefresh" => (0xA8, true),
        "browserhome" => (0xAC, true),
        "plus" => (0xBB, false),
        "equals" => (0xBB, false),
        "minus" => (0xBD, false),
        "comma" => (0xBC, false),
        "period" => (0xBE, false),
        "slash" => (0xBF, false),
        "backslash" => (0xDC, false),
        "semicolon" => (0xBA, false),
        "quote" => (0xDE, false),
        "bracketleft" => (0xDB, false),
        "bracketright" => (0xDD, false),
        "backquote" => (0xC0, false),
        _ => {
            let c: Vec<char> = name.chars().collect();
            if c.len() == 1 && c[0].is_ascii_alphanumeric() {
                return Some((u16::from(c[0].to_ascii_uppercase() as u8), false));
            }
            if let Some(n) = name.strip_prefix('f').and_then(|n| n.parse::<u16>().ok()) {
                if (1..=24).contains(&n) {
                    return Some((0x70 + n - 1, false));
                }
            }
            return None;
        }
    };
    Some(v)
}

pub(super) fn key_input(vk: u16, extended: bool, up: bool) -> INPUT {
    // SAFETY: MapVirtualKeyW is a pure lookup.
    let scan = u16::try_from(unsafe { MapVirtualKeyW(u32::from(vk), MAPVK_VK_TO_VSC) }).unwrap_or(0);
    let mut flags = KEYBD_EVENT_FLAGS(0);
    if extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(vk), wScan: scan, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

fn unicode_input(unit: u16, up: bool) -> INPUT {
    let mut flags = KEYEVENTF_UNICODE;
    if up {
        flags |= KEYEVENTF_KEYUP;
    }
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT { wVk: VIRTUAL_KEY(0), wScan: unit, dwFlags: flags, time: 0, dwExtraInfo: 0 },
        },
    }
}

pub(super) fn mouse_input(flags: MOUSE_EVENT_FLAGS, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx: 0,
                dy: 0,
                #[allow(clippy::cast_sign_loss)]
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: 0,
            },
        },
    }
}

pub(super) fn send(inputs: &[INPUT]) -> ComputerResult<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: slice of initialized INPUT structs with the right cbsize.
    let n = unsafe { SendInput(inputs, i32::try_from(std::mem::size_of::<INPUT>()).unwrap_or(40)) };
    if n as usize == inputs.len() {
        Ok(())
    } else {
        Err(ComputerError::Backend(
            "input was blocked (another window has higher integrity, e.g. an elevated app or the lock screen)".into(),
        ))
    }
}

pub(super) fn press_keys(keys: &[String]) -> ComputerResult<()> {
    let mut vks = Vec::new();
    for k in keys {
        vks.push(vk_of(k).ok_or_else(|| ComputerError::Invalid(format!("unknown key `{k}`")))?);
    }
    let mut inputs: Vec<INPUT> = vks.iter().map(|(vk, ext)| key_input(*vk, *ext, false)).collect();
    inputs.extend(vks.iter().rev().map(|(vk, ext)| key_input(*vk, *ext, true)));
    send(&inputs)
}

/// Lets SetForegroundWindow succeed from a background process: a synthetic
/// Alt press makes this process the last input source.
pub(super) fn force_foreground(h: HWND) -> bool {
    // SAFETY: plain Win32 calls on a valid handle.
    unsafe {
        if IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        if SetForegroundWindow(h).as_bool() && GetForegroundWindow() == h {
            return true;
        }
        let _ = send(&[key_input(0x12, false, false), key_input(0x12, false, true)]);
        let ok = SetForegroundWindow(h).as_bool();
        std::thread::sleep(Duration::from_millis(60));
        ok || GetForegroundWindow() == h
    }
}

pub(super) fn window_rect(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out pointer.
    unsafe { GetWindowRect(h, &raw mut r).ok().map(|()| r) }
}

// ---------------------------------------------------------------------------
// UI Automation
// ---------------------------------------------------------------------------

pub(super) struct Com;
impl Com {
    pub(super) fn init() -> Self {
        // SAFETY: balanced by Drop (CoUninitialize) on this thread.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        Self
    }
}
impl Drop for Com {
    fn drop(&mut self) {
        // SAFETY: paired with CoInitializeEx above.
        unsafe { CoUninitialize() };
    }
}

pub(super) fn uia() -> ComputerResult<IUIAutomation> {
    // SAFETY: COM initialized by the caller (Com guard).
    unsafe { CoCreateInstance(&CUIAutomation, None, CLSCTX_INPROC_SERVER) }
        .map_err(|e| ComputerError::Backend(format!("UI Automation unavailable: {e}")))
}

fn norm_name(s: &str) -> String {
    s.to_lowercase().replace('ё', "е").replace('&', "").trim().to_owned()
}

/// Best element of `window` named like `name` (exact > prefix > contains, on screen).
pub(super) fn find_element(
    auto: &IUIAutomation,
    window: HWND,
    name: &str,
) -> ComputerResult<(IUIAutomationElement, String)> {
    let want = norm_name(name);
    // SAFETY: COM calls on live interfaces; BSTRs are owned.
    unsafe {
        let root = auto.ElementFromHandle(window).map_err(|e| ComputerError::Backend(format!("UI Automation: {e}")))?;
        let cond = auto.CreateTrueCondition().map_err(|e| ComputerError::Backend(e.to_string()))?;
        let all = root.FindAll(TreeScope_Descendants, &cond).map_err(|e| ComputerError::Backend(e.to_string()))?;
        let len = all.Length().unwrap_or(0);
        let mut best: Option<(u8, IUIAutomationElement, String)> = None;
        for i in 0..len.min(5000) {
            let Ok(el) = all.GetElement(i) else { continue };
            let Ok(n) = el.CurrentName() else { continue };
            let n = n.to_string();
            if n.is_empty() {
                continue;
            }
            let nn = norm_name(&n);
            let rank = if nn == want {
                3
            } else if nn.starts_with(&want) {
                2
            } else if nn.contains(&want) {
                1
            } else {
                0
            };
            if rank == 0 || el.CurrentIsOffscreen().is_ok_and(|b| b.as_bool()) {
                continue;
            }
            if best.as_ref().is_none_or(|(r, _, _)| rank > *r) {
                best = Some((rank, el, n));
                if rank == 3 {
                    break;
                }
            }
        }
        let (_, el, label) =
            best.ok_or_else(|| ComputerError::NotFound(format!("no element named “{name}” in the active window")))?;
        Ok((el, label))
    }
}

fn click_element_impl(window: HWND, name: &str) -> ComputerResult<String> {
    let _com = Com::init();
    let auto = uia()?;
    let (el, label) = find_element(&auto, window, name)?;
    // SAFETY: COM calls on live interfaces.
    unsafe {
        if el.CurrentIsPassword().is_ok_and(|b| b.as_bool()) {
            return Err(ComputerError::Forbidden("refusing to act on a password field".into()));
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationInvokePattern>(UIA_InvokePatternId) {
            if p.Invoke().is_ok() {
                return Ok(label);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationTogglePattern>(UIA_TogglePatternId) {
            if p.Toggle().is_ok() {
                return Ok(label);
            }
        }
        if let Ok(p) = el.GetCurrentPatternAs::<IUIAutomationSelectionItemPattern>(UIA_SelectionItemPatternId) {
            if p.Select().is_ok() {
                return Ok(label);
            }
        }
        // fall back to a real click in the middle of the element
        let r = el.CurrentBoundingRectangle().map_err(|e| ComputerError::Backend(e.to_string()))?;
        if r.right <= r.left || r.bottom <= r.top {
            return Err(ComputerError::Backend(format!("“{label}” cannot be clicked")));
        }
        SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2)
            .map_err(|e| ComputerError::Backend(e.to_string()))?;
        send(&[mouse_input(MOUSEEVENTF_LEFTDOWN, 0), mouse_input(MOUSEEVENTF_LEFTUP, 0)])?;
        Ok(label)
    }
}

fn focused_password_impl() -> ComputerResult<bool> {
    let _com = Com::init();
    let auto = uia()?;
    // SAFETY: COM calls on live interfaces.
    unsafe {
        match auto.GetFocusedElement() {
            Ok(el) => Ok(el.CurrentIsPassword().is_ok_and(|b| b.as_bool())),
            Err(_) => Ok(false),
        }
    }
}

// ---------------------------------------------------------------------------
// PowerShell helpers
// ---------------------------------------------------------------------------

fn b64(s: &str) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let bytes = s.as_bytes();
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b = [chunk[0], *chunk.get(1).unwrap_or(&0), *chunk.get(2).unwrap_or(&0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                out.push(T[((n >> (18 - 6 * i)) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// PowerShell string literal decoding a base64 UTF-8 value (no escaping issues).
pub(super) fn ps_str(s: &str) -> String {
    format!("([Text.Encoding]::UTF8.GetString([Convert]::FromBase64String('{}')))", b64(s))
}

pub(super) fn powershell(script: &str) -> ComputerResult<String> {
    let mut child = Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-ExecutionPolicy", "Bypass", "-Command", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .creation_flags(CREATE_NO_WINDOW)
        .spawn()
        .map_err(|e| ComputerError::Backend(format!("powershell: {e}")))?;
    {
        let mut stdin = child.stdin.take().ok_or_else(|| ComputerError::Backend("powershell stdin".into()))?;
        let full = format!(
            "$ErrorActionPreference='Stop'\n[Console]::OutputEncoding=[Text.Encoding]::UTF8\n{script}\nexit 0\n"
        );
        stdin.write_all(full.as_bytes())?;
    }
    let out = child.wait_with_output()?;
    let stdout = String::from_utf8_lossy(&out.stdout).trim().to_owned();
    if out.status.success() {
        Ok(stdout)
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err.lines().find(|l| !l.trim().is_empty()).unwrap_or("failed").trim().to_owned();
        Err(ComputerError::Backend(format!("powershell: {first}")))
    }
}

impl ComputerBackend for WindowsBackend {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn available(&self) -> ComputerResult<()> {
        Ok(())
    }

    fn shell_open(&self, target: &str, args: Option<&str>) -> ComputerResult<()> {
        let file = HSTRING::from(target);
        let params = args.map(HSTRING::from);
        // SAFETY: HSTRINGs outlive the call.
        let r = unsafe {
            ShellExecuteW(
                None,
                &HSTRING::from("open"),
                &file,
                params.as_ref().map_or(PCWSTR::null(), |p| PCWSTR(p.as_ptr())),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        let code = r.0 as isize;
        if code > 32 {
            Ok(())
        } else {
            Err(ComputerError::Backend(format!(
                "cannot open “{target}” ({})",
                match code {
                    2 | 3 => "not found".to_owned(),
                    5 => "access denied".to_owned(),
                    31 => "no app is associated".to_owned(),
                    c => format!("error {c}"),
                }
            )))
        }
    }

    fn installed_apps(&self) -> ComputerResult<Vec<InstalledApp>> {
        let raw = powershell("Get-StartApps | Select-Object Name, AppID | ConvertTo-Json -Compress")?;
        #[derive(serde::Deserialize)]
        struct Row {
            #[serde(rename = "Name")]
            name: Option<String>,
            #[serde(rename = "AppID")]
            app_id: Option<String>,
        }
        let rows: Vec<Row> = if raw.starts_with('[') {
            serde_json::from_str(&raw).unwrap_or_default()
        } else {
            serde_json::from_str::<Row>(&raw).map(|r| vec![r]).unwrap_or_default()
        };
        Ok(rows
            .into_iter()
            .filter_map(|r| {
                let (name, id) = (r.name?, r.app_id?);
                (!name.is_empty() && !id.is_empty())
                    .then(|| InstalledApp { name, target: format!("shell:AppsFolder\\{id}") })
            })
            .collect())
    }

    fn windows(&self) -> ComputerResult<Vec<WindowInfo>> {
        Ok(list_windows())
    }

    fn window_op(&self, handle: Option<&str>, op: WindowOp) -> ComputerResult<()> {
        let h = match handle {
            Some(s) => hwnd_of(s)?,
            // SAFETY: plain query.
            None => unsafe { GetForegroundWindow() },
        };
        // SAFETY: Win32 calls on a valid window handle.
        unsafe {
            match op {
                WindowOp::Focus => {
                    if !force_foreground(h) {
                        return Err(ComputerError::Backend("Windows refused to switch the foreground window".into()));
                    }
                }
                WindowOp::Minimize => {
                    let _ = ShowWindow(h, SW_MINIMIZE);
                }
                WindowOp::Maximize => {
                    let _ = ShowWindow(h, SW_MAXIMIZE);
                }
                WindowOp::Restore => {
                    let _ = ShowWindow(h, SW_RESTORE);
                }
                WindowOp::Close => {
                    PostMessageW(Some(h), WM_CLOSE, WPARAM(0), LPARAM(0))
                        .map_err(|e| ComputerError::Backend(format!("close: {e}")))?;
                }
                WindowOp::SnapLeft | WindowOp::SnapRight | WindowOp::NextMonitor | WindowOp::PreviousMonitor => {
                    force_foreground(h);
                    if IsZoomed(h).as_bool() && matches!(op, WindowOp::SnapLeft | WindowOp::SnapRight) {
                        let _ = ShowWindow(h, SW_RESTORE);
                    }
                    let keys: &[&str] = match op {
                        WindowOp::SnapLeft => &["win", "left"],
                        WindowOp::SnapRight => &["win", "right"],
                        WindowOp::NextMonitor => &["win", "shift", "right"],
                        _ => &["win", "shift", "left"],
                    };
                    press_keys(&keys.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>())?;
                }
                WindowOp::SwitchNext => {
                    press_keys(&["alt".into(), "tab".into()])?;
                }
                WindowOp::ShowDesktop => {
                    press_keys(&["win".into(), "d".into()])?;
                }
                WindowOp::MinimizeAll => press_keys(&["win".into(), "m".into()])?,
                WindowOp::RestoreAll => press_keys(&["win".into(), "shift".into(), "m".into()])?,
                // geometry / z-order: the executor computes the rectangle (`pc` requests)
                _ => return Err(ComputerError::Invalid(format!("{op:?} is handled by the executor"))),
            }
        }
        Ok(())
    }

    fn press(&self, keys: &[String]) -> ComputerResult<()> {
        press_keys(keys)
    }

    fn type_text(&self, text: &str) -> ComputerResult<()> {
        // Characters the foreground keyboard layout has a key for are typed
        // as real key strokes (with Shift); everything else as VK_PACKET.
        // One character per SendInput call with a pause: WinUI text boxes
        // (Windows 11 Notepad) mangle bursts of VK_PACKET input.
        // SAFETY: plain queries.
        let layout = unsafe { GetKeyboardLayout(GetWindowThreadProcessId(GetForegroundWindow(), None)) };
        // Caps Lock would invert letters typed as key strokes: send those as VK_PACKET.
        // SAFETY: plain query.
        let caps = unsafe { GetKeyState(0x14) } & 1 != 0;
        for ch in text.chars() {
            let batch: Vec<INPUT> = match ch {
                '\r' => continue,
                '\n' => vec![key_input(0x0D, false, false), key_input(0x0D, false, true)],
                '\t' => vec![key_input(0x09, false, false), key_input(0x09, false, true)],
                c => {
                    let mut buf = [0u16; 2];
                    let units = c.encode_utf16(&mut buf);
                    // SAFETY: pure lookup.
                    let scan = if units.len() == 1 && !(caps && c.is_alphabetic()) {
                        unsafe { VkKeyScanExW(units[0], layout) }
                    } else {
                        -1
                    };
                    #[allow(clippy::cast_sign_loss)]
                    let (vk, mods) = ((scan & 0xFF) as u16, ((scan >> 8) & 0xFF) as u8);
                    if scan != -1 && mods <= 1 && vk != 0 {
                        let mut v = Vec::new();
                        if mods == 1 {
                            v.push(key_input(0x10, false, false));
                        }
                        v.push(key_input(vk, false, false));
                        v.push(key_input(vk, false, true));
                        if mods == 1 {
                            v.push(key_input(0x10, false, true));
                        }
                        v
                    } else {
                        let mut v: Vec<INPUT> = units.iter().map(|u| unicode_input(*u, false)).collect();
                        v.extend(units.iter().map(|u| unicode_input(*u, true)));
                        v
                    }
                }
            };
            send(&batch)?;
            std::thread::sleep(Duration::from_millis(if ch == '\n' { 60 } else { 8 }));
        }
        Ok(())
    }

    fn scroll(&self, direction: ScrollDirection, amount: u32, unit: ScrollUnit) -> ComputerResult<()> {
        match unit {
            ScrollUnit::Lines => {
                // the wheel goes to the window under the cursor: put it over the foreground window
                // SAFETY: plain queries / cursor move.
                unsafe {
                    let fg = GetForegroundWindow();
                    if let Some(r) = window_rect(fg) {
                        let mut p = POINT::default();
                        let _ = GetCursorPos(&raw mut p);
                        let inside = p.x >= r.left && p.x < r.right && p.y >= r.top && p.y < r.bottom;
                        if !inside {
                            let _ = SetCursorPos((r.left + r.right) / 2, (r.top + r.bottom) / 2);
                        }
                    }
                }
                let (flags, sign) = match direction {
                    ScrollDirection::Up => (MOUSEEVENTF_WHEEL, 1),
                    ScrollDirection::Down => (MOUSEEVENTF_WHEEL, -1),
                    ScrollDirection::Right => (MOUSEEVENTF_HWHEEL, 1),
                    ScrollDirection::Left => (MOUSEEVENTF_HWHEEL, -1),
                };
                for _ in 0..amount.clamp(1, 100) {
                    send(&[mouse_input(flags, 120 * sign)])?;
                    std::thread::sleep(Duration::from_millis(15));
                }
                Ok(())
            }
            ScrollUnit::Pages => {
                let key = match direction {
                    ScrollDirection::Up | ScrollDirection::Left => "pageup",
                    ScrollDirection::Down | ScrollDirection::Right => "pagedown",
                };
                for _ in 0..amount.clamp(1, 50) {
                    press_keys(&[key.to_owned()])?;
                    std::thread::sleep(Duration::from_millis(30));
                }
                Ok(())
            }
            ScrollUnit::Edge => {
                let key = match direction {
                    ScrollDirection::Up | ScrollDirection::Left => "home",
                    ScrollDirection::Down | ScrollDirection::Right => "end",
                };
                press_keys(&["ctrl".to_owned(), key.to_owned()])
            }
        }
    }

    fn mouse_click(&self, button: MouseButton, at: Option<(i32, i32)>) -> ComputerResult<()> {
        if let Some((x, y)) = at {
            // SAFETY: plain cursor move.
            unsafe { SetCursorPos(x, y) }.map_err(|e| ComputerError::Backend(e.to_string()))?;
        }
        let (down, up) = match button {
            MouseButton::Left | MouseButton::Double => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        };
        send(&[mouse_input(down, 0), mouse_input(up, 0)])?;
        if button == MouseButton::Double {
            send(&[mouse_input(down, 0), mouse_input(up, 0)])?;
        }
        Ok(())
    }

    fn click_element(&self, window: Option<&str>, name: &str) -> ComputerResult<String> {
        let h = match window {
            Some(s) => hwnd_of(s)?,
            // SAFETY: plain query.
            None => unsafe { GetForegroundWindow() },
        };
        click_element_impl(h, name)
    }

    fn focused_is_password(&self) -> ComputerResult<bool> {
        focused_password_impl()
    }

    fn media(&self, key: MediaKey) -> ComputerResult<()> {
        let k = match key {
            MediaKey::PlayPause => "playpause",
            MediaKey::Next => "nexttrack",
            MediaKey::Previous => "prevtrack",
            MediaKey::Stop => "mediastop",
        };
        press_keys(&[k.to_owned()])
    }

    fn volume_step(&self, delta_percent: i32, mute_toggle: bool) -> ComputerResult<()> {
        if mute_toggle {
            return press_keys(&["volumemute".to_owned()]);
        }
        // one volume key press = 2 %
        let key = if delta_percent >= 0 { "volumeup" } else { "volumedown" };
        let presses = (delta_percent.unsigned_abs() / 2).clamp(1, 50);
        for _ in 0..presses {
            press_keys(&[key.to_owned()])?;
            std::thread::sleep(Duration::from_millis(10));
        }
        Ok(())
    }

    fn brightness(&self, set: Option<u32>) -> ComputerResult<u32> {
        let script = match set {
            Some(v) => format!(
                "$m = Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightnessMethods\n\
                 Invoke-CimMethod -InputObject $m -MethodName WmiSetBrightness -Arguments @{{Timeout=1; Brightness=[byte]{}}} | Out-Null\n\
                 {}",
                v.min(100),
                v.min(100)
            ),
            None => "(Get-CimInstance -Namespace root/WMI -ClassName WmiMonitorBrightness).CurrentBrightness".to_owned(),
        };
        match powershell(&script) {
            Ok(out) => Ok(out.lines().last().and_then(|l| l.trim().parse().ok()).unwrap_or(set.unwrap_or(0))),
            // no laptop panel: external monitors over DDC/CI
            Err(_) => super::pc_win::ddc_brightness(set),
        }
    }

    fn system(&self, op: SystemOp) -> ComputerResult<()> {
        match op {
            // SAFETY: plain Win32 calls.
            SystemOp::Lock => unsafe { LockWorkStation() }.map_err(|e| ComputerError::Backend(e.to_string())),
            SystemOp::Sleep => {
                // SAFETY: plain Win32 call.
                if unsafe { SetSuspendState(false, false, false) } {
                    Ok(())
                } else {
                    Err(ComputerError::Backend("sleep was refused".into()))
                }
            }
            SystemOp::Shutdown | SystemOp::Restart | SystemOp::SignOut => {
                let args: &[&str] = match op {
                    SystemOp::Shutdown => &["/s", "/t", "5"],
                    SystemOp::Restart => &["/r", "/t", "5"],
                    _ => &["/l"],
                };
                let st = Command::new("shutdown.exe").args(args).creation_flags(CREATE_NO_WINDOW).status()?;
                if st.success() {
                    Ok(())
                } else {
                    Err(ComputerError::Backend(format!("shutdown.exe failed ({st})")))
                }
            }
        }
    }

    fn screenshot(&self, path: &Path) -> ComputerResult<()> {
        let script = format!(
            "Add-Type -AssemblyName System.Windows.Forms, System.Drawing\n\
             Add-Type -Namespace W -Name D -MemberDefinition '[DllImport(\"user32.dll\")] public static extern bool SetProcessDPIAware();'\n\
             [W.D]::SetProcessDPIAware() | Out-Null\n\
             $b = [System.Windows.Forms.SystemInformation]::VirtualScreen\n\
             $bmp = New-Object System.Drawing.Bitmap $b.Width, $b.Height\n\
             $g = [System.Drawing.Graphics]::FromImage($bmp)\n\
             $g.CopyFromScreen($b.Left, $b.Top, 0, 0, $bmp.Size)\n\
             $p = {}\n\
             New-Item -ItemType Directory -Force -Path (Split-Path $p) | Out-Null\n\
             $bmp.Save($p, [System.Drawing.Imaging.ImageFormat]::Png)\n\
             $g.Dispose(); $bmp.Dispose()",
            ps_str(&path.display().to_string())
        );
        powershell(&script).map(|_| ())
    }

    fn clipboard_get(&self) -> ComputerResult<String> {
        powershell("$t = Get-Clipboard -Raw\nif ($t) { $t } else { '' }")
    }

    fn clipboard_set(&self, text: &str) -> ComputerResult<()> {
        powershell(&format!("Set-Clipboard -Value {}", ps_str(text))).map(|_| ())
    }

    fn notify(&self, title: &str, body: &str) -> ComputerResult<()> {
        let script = format!(
            "[Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null\n\
             [Windows.Data.Xml.Dom.XmlDocument, Windows.Data.Xml.Dom.XmlDocument, ContentType = WindowsRuntime] | Out-Null\n\
             $x = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02)\n\
             $t = $x.GetElementsByTagName('text')\n\
             $t.Item(0).AppendChild($x.CreateTextNode({})) | Out-Null\n\
             $t.Item(1).AppendChild($x.CreateTextNode({})) | Out-Null\n\
             $app = '{{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}}\\WindowsPowerShell\\v1.0\\powershell.exe'\n\
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($app).Show([Windows.UI.Notifications.ToastNotification]::new($x))",
            ps_str(title),
            ps_str(body)
        );
        powershell(&script).map(|_| ())
    }

    fn processes(&self) -> ComputerResult<Vec<ProcessInfo>> {
        let with_windows: std::collections::HashSet<u32> = list_windows().into_iter().map(|w| w.pid).collect();
        Ok(crate::procs::list()
            .into_iter()
            .map(|mut p| {
                p.has_window = with_windows.contains(&p.pid);
                p
            })
            .collect())
    }

    fn kill(&self, pid: u32) -> ComputerResult<()> {
        crate::procs::kill(pid)
    }

    fn pc(&self, req: &super::PcRequest) -> ComputerResult<super::PcReply> {
        super::pc_win::handle(self, req)
    }

    fn recycle(&self, path: &Path) -> ComputerResult<()> {
        let p = ps_str(&path.display().to_string());
        let script = format!(
            "Add-Type -AssemblyName Microsoft.VisualBasic\n$p = {p}\n\
             if (Test-Path -LiteralPath $p -PathType Container) {{ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteDirectory($p, 'OnlyErrorDialogs', 'SendToRecycleBin') }}\n\
             elseif (Test-Path -LiteralPath $p) {{ [Microsoft.VisualBasic.FileIO.FileSystem]::DeleteFile($p, 'OnlyErrorDialogs', 'SendToRecycleBin') }}\n\
             else {{ throw 'not found' }}"
        );
        powershell(&script).map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_reference() {
        assert_eq!(b64(""), "");
        assert_eq!(b64("f"), "Zg==");
        assert_eq!(b64("fo"), "Zm8=");
        assert_eq!(b64("foo"), "Zm9v");
        assert_eq!(b64("Привет"), "0J/RgNC40LLQtdGC");
    }

    #[test]
    fn every_canonical_key_has_a_virtual_key() {
        for k in crate::keys::KEY_NAMES {
            assert!(vk_of(k).is_some(), "{k}");
        }
        assert_eq!(vk_of("a"), Some((0x41, false)));
        assert_eq!(vk_of("f5"), Some((0x74, false)));
    }

    #[test]
    fn enumerates_windows_without_panicking() {
        // headless CI may have none; the call itself must work
        let _ = list_windows();
    }
}
