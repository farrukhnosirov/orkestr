//! Windows cursor-mode backend (ADR-0050): GDI screen capture, UI
//! Automation element tree, absolute DPI-correct `SendInput` pointer input,
//! the abort guard (`WH_KEYBOARD_LL` / `WH_MOUSE_LL` on a dedicated thread,
//! physical vs injected via `LLKHF_INJECTED` / `LLMHF_INJECTED`) and the
//! safety overlay (click-through layered top-most window, excluded from
//! capture with `WDA_EXCLUDEFROMCAPTURE`, on the same thread).
#![allow(unsafe_code)]

use std::sync::atomic::{AtomicBool, AtomicI32, AtomicIsize, AtomicU32, AtomicUsize, Ordering};
use std::sync::mpsc;
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::Duration;

use parking_lot::Mutex;
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{
    BeginPaint, BitBlt, CreateCompatibleBitmap, CreateCompatibleDC, CreateFontW, CreateSolidBrush, DeleteDC,
    DeleteObject, DrawTextW, EndPaint, EnumDisplayMonitors, FillRect, GetDC, GetDIBits, GetMonitorInfoW,
    InvalidateRect, MonitorFromPoint, MonitorFromWindow, ReleaseDC, SelectObject, SetBkMode, SetTextColor, BITMAPINFO,
    BITMAPINFOHEADER, BI_RGB, CAPTUREBLT, CLEARTYPE_QUALITY, CLIP_DEFAULT_PRECIS, DEFAULT_CHARSET, DIB_RGB_COLORS,
    DT_CALCRECT, DT_CENTER, DT_NOPREFIX, DT_WORDBREAK, FF_DONTCARE, FW_SEMIBOLD, HDC, HGDIOBJ, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST, OUT_DEFAULT_PRECIS, PAINTSTRUCT, SRCCOPY, TRANSPARENT,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentThreadId;
use windows::Win32::System::Variant::{VariantToInt32, VariantToStringAlloc};
use windows::Win32::UI::Accessibility::{
    IUIAutomationElement, TreeScope_Descendants, UIA_BoundingRectanglePropertyId, UIA_ControlTypePropertyId,
    UIA_IsEnabledPropertyId, UIA_IsOffscreenPropertyId, UIA_IsPasswordPropertyId, UIA_NamePropertyId,
    UIA_ToggleToggleStatePropertyId, UIA_ValueValuePropertyId,
};
use windows::Win32::UI::HiDpi::{SetThreadDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    INPUT, INPUT_0, INPUT_MOUSE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK, MOUSEEVENTF_WHEEL, MOUSEINPUT, MOUSE_EVENT_FLAGS,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, CreateWindowExW, DefWindowProcW, DispatchMessageW, GetAncestor, GetCursorPos, GetForegroundWindow,
    GetMessageW, GetSystemMetrics, GetWindowLongW, IsIconic, IsWindowVisible, PostThreadMessageW, RegisterClassExW,
    SetLayeredWindowAttributes, SetWindowDisplayAffinity, SetWindowPos, SetWindowsHookExW, ShowWindow,
    TranslateMessage, UnhookWindowsHookEx, WindowFromPoint, GA_ROOT, GWL_EXSTYLE, HHOOK, HWND_TOPMOST, KBDLLHOOKSTRUCT,
    LLKHF_INJECTED, LLMHF_INJECTED, LWA_COLORKEY, MSG, MSLLHOOKSTRUCT, SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
    SM_XVIRTUALSCREEN, SM_YVIRTUALSCREEN, SWP_NOACTIVATE, SW_HIDE, SW_MINIMIZE, SW_RESTORE, SW_SHOWNOACTIVATE,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_APP, WM_KEYDOWN, WM_LBUTTONDOWN, WM_MBUTTONDOWN,
    WM_MOUSEHWHEEL, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WM_SYSKEYDOWN, WM_XBUTTONDOWN, WNDCLASSEXW,
    WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_EX_TRANSPARENT, WS_POPUP,
};

use orkestr_protocol::methods::computer::{MouseButton, ScrollDirection};

use super::windows::{
    enum_top_windows, is_cloaked, process_name, send, uia, window_pid, window_rect, window_title, Com,
};
use crate::cursor::screen::{AbortCause, CursorBackend, Foreground, RawElement, RawImage, ScreenRect, TripFn};
use crate::error::{ComputerError, ComputerResult};

/// `dwExtraInfo` of Orkestr's own injected pointer events.
const MAGIC: usize = 0x4F52_4B43; // "ORKC"
/// Physical movement (px) that aborts.
const MOUSE_TOLERANCE: i32 = 6;

const MSG_ARM: u32 = WM_APP + 1;
const MSG_DISARM: u32 = WM_APP + 2;
const MSG_OVERLAY: u32 = WM_APP + 3;
const MSG_OVERLAY_HIDE: u32 = WM_APP + 4;

const KEY: COLORREF = COLORREF(0x00FF_00FF); // magenta = transparent
const ACCENT: COLORREF = COLORREF(0x00FF_B03A); // #3AB0FF (BGR)
const BADGE_BG: COLORREF = COLORREF(0x0020_1810);
const BADGE_FG: COLORREF = COLORREF(0x00FF_FFFF);
const BORDER: i32 = 4;

// ── shared state (hook procs are plain functions) ─────────────────────────

struct Guard {
    armed: AtomicBool,
    mouse: AtomicBool,
    anchor_set: AtomicBool,
    ax: AtomicI32,
    ay: AtomicI32,
    trip: StdMutex<Option<TripFn>>,
    /// Smoke tests only: injected events with this `dwExtraInfo` count as physical.
    test_marker: AtomicUsize,
}

static GUARD: LazyLock<Guard> = LazyLock::new(|| Guard {
    armed: AtomicBool::new(false),
    mouse: AtomicBool::new(false),
    anchor_set: AtomicBool::new(false),
    ax: AtomicI32::new(0),
    ay: AtomicI32::new(0),
    trip: StdMutex::new(None),
    test_marker: AtomicUsize::new(0),
});

static OVERLAY_HWND: AtomicIsize = AtomicIsize::new(0);
static BADGE: StdMutex<String> = StdMutex::new(String::new());

fn fire(cause: AbortCause) {
    if GUARD.armed.swap(false, Ordering::SeqCst) {
        let f = GUARD.trip.lock().ok().and_then(|g| g.clone());
        if let Some(f) = f {
            f(cause);
        }
    }
}

fn is_injected(flag: bool, extra: usize) -> bool {
    let marker = GUARD.test_marker.load(Ordering::Relaxed);
    flag && !(marker != 0 && extra == marker)
}

unsafe extern "system" fn keyboard_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && GUARD.armed.load(Ordering::Relaxed) {
        // SAFETY: for WH_KEYBOARD_LL, lparam points to a KBDLLHOOKSTRUCT.
        let k = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        let down = matches!(u32::try_from(wparam.0).unwrap_or(0), WM_KEYDOWN | WM_SYSKEYDOWN);
        if down && k.vkCode == 0x1B && !is_injected(k.flags.0 & LLKHF_INJECTED.0 != 0, k.dwExtraInfo) {
            fire(AbortCause::Esc);
        }
    }
    // SAFETY: passes the event on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_proc(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 && GUARD.armed.load(Ordering::Relaxed) && GUARD.mouse.load(Ordering::Relaxed) {
        // SAFETY: for WH_MOUSE_LL, lparam points to an MSLLHOOKSTRUCT.
        let m = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        let injected = is_injected((m.flags & LLMHF_INJECTED) != 0, m.dwExtraInfo);
        let msg = u32::try_from(wparam.0).unwrap_or(0);
        if injected {
            // our own pointer moved: physical movement is measured from the next physical event
            GUARD.anchor_set.store(false, Ordering::Relaxed);
        } else {
            match msg {
                WM_MOUSEMOVE => {
                    if GUARD.anchor_set.swap(true, Ordering::Relaxed) {
                        let dx = (m.pt.x - GUARD.ax.load(Ordering::Relaxed)).abs();
                        let dy = (m.pt.y - GUARD.ay.load(Ordering::Relaxed)).abs();
                        if dx.max(dy) > MOUSE_TOLERANCE {
                            fire(AbortCause::Mouse);
                        }
                    } else {
                        GUARD.ax.store(m.pt.x, Ordering::Relaxed);
                        GUARD.ay.store(m.pt.y, Ordering::Relaxed);
                    }
                }
                WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN | WM_XBUTTONDOWN | WM_MOUSEWHEEL | WM_MOUSEHWHEEL => {
                    fire(AbortCause::Mouse);
                }
                _ => {}
            }
        }
    }
    // SAFETY: passes the event on unchanged.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

// ── the safety thread: hooks + overlay + message loop ─────────────────────

struct SafetyThread {
    thread_id: u32,
    acks: mpsc::Receiver<Result<(), String>>,
}

fn start_safety_thread() -> ComputerResult<SafetyThread> {
    let (tid_tx, tid_rx) = mpsc::channel();
    let (ack_tx, ack_rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("orkestr-cursor-guard".into())
        .spawn(move || safety_loop(&tid_tx, &ack_tx))
        .map_err(|e| ComputerError::Backend(format!("guard thread: {e}")))?;
    let thread_id = tid_rx
        .recv_timeout(Duration::from_secs(3))
        .map_err(|_| ComputerError::Backend("guard thread did not start".into()))?;
    Ok(SafetyThread { thread_id, acks: ack_rx })
}

#[allow(clippy::too_many_lines)]
fn safety_loop(tid_tx: &mpsc::Sender<u32>, ack: &mpsc::Sender<Result<(), String>>) {
    // SAFETY: Win32 calls on this thread's own objects; hooks are removed before the thread ends.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        let mut msg = MSG::default();
        // create the message queue before reporting the thread id
        let _ = windows::Win32::UI::WindowsAndMessaging::PeekMessageW(
            &raw mut msg,
            None,
            0,
            0,
            windows::Win32::UI::WindowsAndMessaging::PM_NOREMOVE,
        );
        let _ = tid_tx.send(GetCurrentThreadId());
        let mut kb: Option<HHOOK> = None;
        let mut ms: Option<HHOOK> = None;
        let mut overlay: Option<HWND> = None;
        while GetMessageW(&raw mut msg, None, 0, 0).as_bool() {
            if msg.hwnd.0.is_null() {
                match msg.message {
                    MSG_ARM => {
                        let mouse = msg.wParam.0 != 0;
                        let hmod = GetModuleHandleW(PCWSTR::null()).ok().map(Into::into);
                        let mut res = Ok(());
                        if kb.is_none() {
                            match SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_proc), hmod, 0) {
                                Ok(h) => kb = Some(h),
                                Err(e) => res = Err(format!("keyboard hook: {e}")),
                            }
                        }
                        if res.is_ok() && mouse && ms.is_none() {
                            match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_proc), hmod, 0) {
                                Ok(h) => ms = Some(h),
                                Err(e) => res = Err(format!("mouse hook: {e}")),
                            }
                        }
                        if res.is_ok() {
                            GUARD.mouse.store(mouse, Ordering::SeqCst);
                            GUARD.anchor_set.store(false, Ordering::SeqCst);
                            GUARD.armed.store(true, Ordering::SeqCst);
                        }
                        let _ = ack.send(res);
                        continue;
                    }
                    MSG_DISARM => {
                        GUARD.armed.store(false, Ordering::SeqCst);
                        if let Some(h) = kb.take() {
                            let _ = UnhookWindowsHookEx(h);
                        }
                        if let Some(h) = ms.take() {
                            let _ = UnhookWindowsHookEx(h);
                        }
                        let _ = ack.send(Ok(()));
                        continue;
                    }
                    MSG_OVERLAY => {
                        if overlay.is_none() {
                            overlay = create_overlay();
                        }
                        if let Some(h) = overlay {
                            let (x, y, w, h2) = virtual_screen();
                            let _ = SetWindowPos(h, Some(HWND_TOPMOST), x, y, w, h2, SWP_NOACTIVATE);
                            let _ = ShowWindow(h, SW_SHOWNOACTIVATE);
                            let _ = InvalidateRect(Some(h), None, true);
                        }
                        let _ = ack.send(if overlay.is_some() { Ok(()) } else { Err("overlay window".into()) });
                        continue;
                    }
                    MSG_OVERLAY_HIDE => {
                        if let Some(h) = overlay {
                            let _ = ShowWindow(h, SW_HIDE);
                        }
                        let _ = ack.send(Ok(()));
                        continue;
                    }
                    _ => {}
                }
            }
            let _ = TranslateMessage(&raw const msg);
            DispatchMessageW(&raw const msg);
        }
    }
}

fn virtual_screen() -> (i32, i32, i32, i32) {
    // SAFETY: plain metrics queries.
    unsafe {
        (
            GetSystemMetrics(SM_XVIRTUALSCREEN),
            GetSystemMetrics(SM_YVIRTUALSCREEN),
            GetSystemMetrics(SM_CXVIRTUALSCREEN),
            GetSystemMetrics(SM_CYVIRTUALSCREEN),
        )
    }
}

unsafe fn create_overlay() -> Option<HWND> {
    // SAFETY: class registration and window creation on the calling (message-loop) thread.
    unsafe {
        let hinst = GetModuleHandleW(PCWSTR::null()).ok()?;
        let class = w!("OrkestrCursorOverlay");
        let wc = WNDCLASSEXW {
            cbSize: u32::try_from(std::mem::size_of::<WNDCLASSEXW>()).unwrap_or(80),
            lpfnWndProc: Some(overlay_proc),
            hInstance: hinst.into(),
            lpszClassName: class,
            ..Default::default()
        };
        let _ = RegisterClassExW(&raw const wc);
        let (x, y, w, h) = virtual_screen();
        let hwnd = CreateWindowExW(
            WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE,
            class,
            w!("Orkestr cursor mode"),
            WS_POPUP,
            x,
            y,
            w,
            h,
            None,
            None,
            Some(hinst.into()),
            None,
        )
        .ok()?;
        let _ = SetLayeredWindowAttributes(hwnd, KEY, 255, LWA_COLORKEY);
        // never in screenshots (ours or anybody's)
        let _ = SetWindowDisplayAffinity(hwnd, WDA_EXCLUDEFROMCAPTURE);
        OVERLAY_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        Some(hwnd)
    }
}

unsafe extern "system" fn monitor_rects(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
    // SAFETY: lp is the &mut Vec<RECT> passed by `paint`.
    let v = unsafe { &mut *(lp.0 as *mut Vec<(RECT, bool)>) };
    let mut mi =
        MONITORINFO { cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(40), ..Default::default() };
    // SAFETY: mi is a valid out struct.
    if unsafe { GetMonitorInfoW(m, &raw mut mi) }.as_bool() {
        v.push((mi.rcMonitor, mi.dwFlags & 1 != 0));
    }
    BOOL(1)
}

unsafe fn paint(hwnd: HWND) {
    // SAFETY: standard WM_PAINT handling; GDI objects are deleted below.
    unsafe {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &raw mut ps);
        let (vx, vy, vw, vh) = virtual_screen();
        let key = CreateSolidBrush(KEY);
        let accent = CreateSolidBrush(ACCENT);
        let bg = CreateSolidBrush(BADGE_BG);
        FillRect(hdc, &RECT { left: 0, top: 0, right: vw, bottom: vh }, key);
        let mut mons: Vec<(RECT, bool)> = Vec::new();
        let _ = EnumDisplayMonitors(None, None, Some(monitor_rects), LPARAM(&raw mut mons as isize));
        let font = CreateFontW(
            22,
            0,
            0,
            0,
            FW_SEMIBOLD.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            FF_DONTCARE.0 as u32,
            w!("Segoe UI"),
        );
        let old_font = SelectObject(hdc, HGDIOBJ(font.0));
        let badge = BADGE.lock().map(|b| b.clone()).unwrap_or_default();
        for (r, primary) in &mons {
            let r = RECT { left: r.left - vx, top: r.top - vy, right: r.right - vx, bottom: r.bottom - vy };
            for edge in [
                RECT { left: r.left, top: r.top, right: r.right, bottom: r.top + BORDER },
                RECT { left: r.left, top: r.bottom - BORDER, right: r.right, bottom: r.bottom },
                RECT { left: r.left, top: r.top, right: r.left + BORDER, bottom: r.bottom },
                RECT { left: r.right - BORDER, top: r.top, right: r.right, bottom: r.bottom },
            ] {
                FillRect(hdc, &raw const edge, accent);
            }
            if !*primary && mons.len() > 1 {
                continue;
            }
            // the floating Jarvis badge, top centre of the primary monitor
            let mut text: Vec<u16> = badge.encode_utf16().collect();
            let width = 720.min(r.right - r.left - 40);
            let mut calc = RECT { left: 0, top: 0, right: width - 40, bottom: 0 };
            DrawTextW(hdc, &mut text, &raw mut calc, DT_CALCRECT | DT_WORDBREAK | DT_CENTER | DT_NOPREFIX);
            let cx = (r.left + r.right) / 2;
            let top = r.top + 18;
            let b = RECT { left: cx - width / 2, top, right: cx + width / 2, bottom: top + calc.bottom + 24 };
            FillRect(hdc, &raw const b, accent);
            FillRect(hdc, &RECT { left: b.left + 2, top: b.top + 2, right: b.right - 2, bottom: b.bottom - 2 }, bg);
            SetBkMode(hdc, TRANSPARENT);
            SetTextColor(hdc, BADGE_FG);
            let mut tr = RECT { left: b.left + 20, top: b.top + 12, right: b.right - 20, bottom: b.bottom - 10 };
            DrawTextW(hdc, &mut text, &raw mut tr, DT_WORDBREAK | DT_CENTER | DT_NOPREFIX);
            // accent dot (the "orb")
            FillRect(hdc, &RECT { left: b.left + 10, top: b.top + 10, right: b.left + 16, bottom: b.top + 16 }, accent);
        }
        SelectObject(hdc, old_font);
        let _ = DeleteObject(HGDIOBJ(font.0));
        let _ = DeleteObject(HGDIOBJ(key.0));
        let _ = DeleteObject(HGDIOBJ(accent.0));
        let _ = DeleteObject(HGDIOBJ(bg.0));
        let _ = EndPaint(hwnd, &raw const ps);
    }
}

unsafe extern "system" fn overlay_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_PAINT {
        // SAFETY: called by the window manager for our window.
        unsafe { paint(hwnd) };
        return LRESULT(0);
    }
    // SAFETY: default handling.
    unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
}

// ── the backend ───────────────────────────────────────────────────────────

/// Windows cursor-mode backend.
pub struct WindowsCursor {
    thread: Mutex<Option<SafetyThread>>,
    /// Own windows we changed: (hwnd, minimized by us).
    changed: Mutex<Vec<(isize, bool)>>,
    armed_with: AtomicU32,
}

impl std::fmt::Debug for WindowsCursor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WindowsCursor").finish_non_exhaustive()
    }
}

impl Default for WindowsCursor {
    fn default() -> Self {
        Self::new()
    }
}

fn dpi() {
    // SAFETY: affects only the calling thread.
    unsafe {
        let _ = SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }
}

fn hwnd_from(v: isize) -> HWND {
    HWND(v as *mut core::ffi::c_void)
}

fn rect_of(r: RECT) -> ScreenRect {
    ScreenRect { x: r.left, y: r.top, w: r.right - r.left, h: r.bottom - r.top }
}

fn mouse(flags: MOUSE_EVENT_FLAGS, dx: i32, dy: i32, data: i32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                #[allow(clippy::cast_sign_loss)]
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: MAGIC,
            },
        },
    }
}

fn abs_move(x: i32, y: i32) -> INPUT {
    let (vx, vy, vw, vh) = virtual_screen();
    let norm = |v: i32, o: i32, size: i32| -> i32 {
        let size = i64::from(size.max(2));
        i32::try_from(((i64::from(v - o)) * 65535 + (size - 1) / 2) / (size - 1)).unwrap_or(0)
    };
    mouse(MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK, norm(x, vx, vw), norm(y, vy, vh), 0)
}

fn buttons(b: MouseButton) -> (MOUSE_EVENT_FLAGS, MOUSE_EVENT_FLAGS) {
    match b {
        MouseButton::Right => (MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
        MouseButton::Middle => (MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        MouseButton::Left | MouseButton::Double => (MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
    }
}

fn role_of(id: i32) -> &'static str {
    match id {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "edit",
        50005 => "hyperlink",
        50006 => "image",
        50007 => "listitem",
        50008 => "list",
        50009 => "menu",
        50010 => "menubar",
        50011 => "menuitem",
        50012 => "progressbar",
        50013 => "radiobutton",
        50014 => "scrollbar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "statusbar",
        50018 => "tab",
        50019 => "tabitem",
        50020 => "text",
        50021 => "toolbar",
        50023 => "tree",
        50024 => "treeitem",
        50026 => "group",
        50029 => "dataitem",
        50030 => "document",
        50031 => "splitbutton",
        50032 => "window",
        50033 => "pane",
        50035 => "headeritem",
        50037 => "titlebar",
        _ => "custom",
    }
}

fn interactive(role: &str) -> bool {
    matches!(
        role,
        "button"
            | "checkbox"
            | "combobox"
            | "edit"
            | "hyperlink"
            | "listitem"
            | "menuitem"
            | "radiobutton"
            | "slider"
            | "spinner"
            | "tabitem"
            | "treeitem"
            | "dataitem"
            | "splitbutton"
            | "headeritem"
            | "document"
    )
}

unsafe fn cached_string(
    el: &IUIAutomationElement,
    prop: windows::Win32::UI::Accessibility::UIA_PROPERTY_ID,
) -> Option<String> {
    // SAFETY: VARIANT owned by the call; the PWSTR is freed with CoTaskMemFree.
    unsafe {
        let v = el.GetCachedPropertyValue(prop).ok()?;
        let p = VariantToStringAlloc(&raw const v).ok()?;
        let s = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const core::ffi::c_void));
        s
    }
}

/// Smoke tests only: injected input carrying `marker` in `dwExtraInfo` is
/// treated as physical by the guard (0 = off), so the Esc / mouse abort path
/// can be exercised end to end with `SendInput`.
#[doc(hidden)]
pub fn set_test_physical_marker(marker: usize) {
    GUARD.test_marker.store(marker, Ordering::SeqCst);
}

/// Smoke tests only: the overlay window exists and is visible.
#[doc(hidden)]
#[must_use]
pub fn test_overlay_visible() -> bool {
    let h = OVERLAY_HWND.load(Ordering::SeqCst);
    // SAFETY: plain query on our own window handle (0 = none).
    h != 0 && unsafe { IsWindowVisible(hwnd_from(h)) }.as_bool()
}

/// Smoke tests only: sends an Esc press (`esc = true`) or a 40 px relative
/// mouse movement carrying `marker` in `dwExtraInfo`.
///
/// # Errors
/// Input blocked.
#[doc(hidden)]
pub fn send_test_input(esc: bool, marker: usize) -> ComputerResult<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    };
    if esc {
        let key = |up: bool| INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(0x1B),
                    wScan: 1,
                    dwFlags: if up { KEYEVENTF_KEYUP } else { KEYBD_EVENT_FLAGS(0) },
                    time: 0,
                    dwExtraInfo: marker,
                },
            },
        };
        send(&[key(false), key(true)])
    } else {
        let mut m = mouse(MOUSEEVENTF_MOVE, 40, 0, 0);
        m.Anonymous.mi.dwExtraInfo = marker;
        send(&[m])?;
        std::thread::sleep(Duration::from_millis(30));
        let mut back = mouse(MOUSEEVENTF_MOVE, -40, 0, 0);
        back.Anonymous.mi.dwExtraInfo = marker;
        send(&[back])
    }
}

impl WindowsCursor {
    /// New backend (the guard thread starts on first use).
    #[must_use]
    pub fn new() -> Self {
        Self { thread: Mutex::new(None), changed: Mutex::new(Vec::new()), armed_with: AtomicU32::new(0) }
    }

    fn post(&self, msg: u32, wparam: usize) -> ComputerResult<()> {
        let mut g = self.thread.lock();
        if g.is_none() {
            *g = Some(start_safety_thread()?);
        }
        let t = g.as_ref().expect("started above");
        while t.acks.try_recv().is_ok() {}
        // SAFETY: posting to our own live thread.
        unsafe { PostThreadMessageW(t.thread_id, msg, WPARAM(wparam), LPARAM(0)) }
            .map_err(|e| ComputerError::Backend(format!("guard thread: {e}")))?;
        match t.acks.recv_timeout(Duration::from_secs(3)) {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(ComputerError::Backend(e)),
            Err(_) => Err(ComputerError::Backend("guard thread did not answer".into())),
        }
    }

    fn foreground_info(h: HWND) -> Foreground {
        let pid = window_pid(h);
        Foreground {
            handle: format!("0x{:x}", h.0 as usize),
            title: window_title(h),
            process: process_name(pid),
            pid,
            rect: window_rect(h).map(rect_of).unwrap_or_default(),
        }
    }
}

impl CursorBackend for WindowsCursor {
    fn name(&self) -> &'static str {
        "windows"
    }

    fn available(&self) -> ComputerResult<()> {
        Ok(())
    }

    fn foreground(&self) -> ComputerResult<Option<Foreground>> {
        dpi();
        // SAFETY: plain query.
        let h = unsafe { GetForegroundWindow() };
        if h.0.is_null() {
            return Ok(None);
        }
        Ok(Some(Self::foreground_info(h)))
    }

    fn monitor_of(&self, window: Option<&Foreground>) -> ComputerResult<ScreenRect> {
        dpi();
        // SAFETY: monitor queries with valid out structs.
        unsafe {
            let m = match window.and_then(|w| usize::from_str_radix(w.handle.trim_start_matches("0x"), 16).ok()) {
                Some(h) if h != 0 => MonitorFromWindow(hwnd_from(h as isize), MONITOR_DEFAULTTONEAREST),
                _ => {
                    let mut p = POINT::default();
                    let _ = GetCursorPos(&raw mut p);
                    MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST)
                }
            };
            let mut mi = MONITORINFO {
                cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(40),
                ..Default::default()
            };
            if !GetMonitorInfoW(m, &raw mut mi).as_bool() {
                return Err(ComputerError::Backend("no monitor".into()));
            }
            Ok(rect_of(mi.rcMonitor))
        }
    }

    fn capture(&self, r: ScreenRect) -> ComputerResult<RawImage> {
        dpi();
        if r.w <= 0 || r.h <= 0 {
            return Err(ComputerError::Invalid("empty capture rectangle".into()));
        }
        // SAFETY: GDI objects are created and released in this block.
        unsafe {
            let screen = GetDC(None);
            if screen.is_invalid() {
                return Err(ComputerError::Backend("screen DC unavailable (secure desktop?)".into()));
            }
            let mem = CreateCompatibleDC(Some(screen));
            let bmp = CreateCompatibleBitmap(screen, r.w, r.h);
            let old = SelectObject(mem, HGDIOBJ(bmp.0));
            let ok = BitBlt(mem, 0, 0, r.w, r.h, Some(screen), r.x, r.y, SRCCOPY | CAPTUREBLT).is_ok();
            SelectObject(mem, old);
            let mut bi = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: u32::try_from(std::mem::size_of::<BITMAPINFOHEADER>()).unwrap_or(40),
                    biWidth: r.w,
                    biHeight: -r.h,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            #[allow(clippy::cast_sign_loss)]
            let (w, h) = (r.w as u32, r.h as u32);
            let mut buf = vec![0u8; (w * h * 4) as usize];
            let lines = if ok {
                GetDIBits(mem, bmp, 0, h, Some(buf.as_mut_ptr().cast()), &raw mut bi, DIB_RGB_COLORS)
            } else {
                0
            };
            let _ = DeleteObject(HGDIOBJ(bmp.0));
            let _ = DeleteDC(mem);
            ReleaseDC(None, screen);
            if lines <= 0 {
                return Err(ComputerError::Backend("screen capture failed (secure desktop or locked)".into()));
            }
            Ok(RawImage { width: w, height: h, bgra: buf })
        }
    }

    fn elements(&self, window: &Foreground, max: usize) -> ComputerResult<Vec<RawElement>> {
        dpi();
        let h = usize::from_str_radix(window.handle.trim_start_matches("0x"), 16)
            .map_err(|_| ComputerError::Invalid("bad window handle".into()))?;
        let _com = Com::init();
        let auto = uia()?;
        let be = |e: windows::core::Error| ComputerError::Backend(format!("UI Automation: {e}"));
        // SAFETY: COM calls on live interfaces; VARIANTs / BSTRs are owned.
        unsafe {
            let root = auto.ElementFromHandle(hwnd_from(h as isize)).map_err(be)?;
            let cache = auto.CreateCacheRequest().map_err(be)?;
            for p in [
                UIA_NamePropertyId,
                UIA_ControlTypePropertyId,
                UIA_BoundingRectanglePropertyId,
                UIA_IsEnabledPropertyId,
                UIA_IsOffscreenPropertyId,
                UIA_IsPasswordPropertyId,
                UIA_ToggleToggleStatePropertyId,
                UIA_ValueValuePropertyId,
            ] {
                let _ = cache.AddProperty(p);
            }
            let cond = auto.ControlViewCondition().map_err(be)?;
            let all = root.FindAllBuildCache(TreeScope_Descendants, &cond, &cache).map_err(be)?;
            let len = all.Length().unwrap_or(0).min(3000);
            let mut first = Vec::new();
            let mut rest = Vec::new();
            for i in 0..len {
                let Ok(el) = all.GetElement(i) else { continue };
                if el.CachedIsOffscreen().is_ok_and(|b| b.as_bool()) {
                    continue;
                }
                let Ok(rc) = el.CachedBoundingRectangle() else { continue };
                if rc.right - rc.left < 2 || rc.bottom - rc.top < 2 {
                    continue;
                }
                let role = role_of(el.CachedControlType().map(|c| c.0).unwrap_or(0));
                let name: String =
                    el.CachedName().map(|b| b.to_string()).unwrap_or_default().trim().chars().take(120).collect();
                let inter = interactive(role);
                if name.is_empty() && !inter
                    || matches!(role, "pane" | "window" | "titlebar" | "scrollbar") && name.is_empty()
                {
                    continue;
                }
                let password = el.CachedIsPassword().is_ok_and(|b| b.as_bool());
                let toggle = el
                    .GetCachedPropertyValue(UIA_ToggleToggleStatePropertyId)
                    .ok()
                    .and_then(|v| VariantToInt32(&raw const v).ok())
                    .and_then(|t| match t {
                        0 => Some("off".to_owned()),
                        1 => Some("on".to_owned()),
                        _ => None,
                    });
                let value = toggle.or_else(|| {
                    if password {
                        None
                    } else {
                        cached_string(&el, UIA_ValueValuePropertyId)
                            .map(|s| s.chars().take(40).collect::<String>())
                            .filter(|s| !s.is_empty())
                    }
                });
                let e = RawElement {
                    role: role.to_owned(),
                    name,
                    rect: rect_of(rc),
                    enabled: el.CachedIsEnabled().is_ok_and(|b| b.as_bool()),
                    password,
                    value,
                };
                if inter {
                    first.push(e);
                } else {
                    rest.push(e);
                }
                if first.len() >= max {
                    break;
                }
            }
            first.extend(rest);
            first.truncate(max);
            Ok(first)
        }
    }

    fn cursor_pos(&self) -> ComputerResult<(i32, i32)> {
        dpi();
        let mut p = POINT::default();
        // SAFETY: valid out pointer.
        unsafe { GetCursorPos(&raw mut p) }.map_err(|e| ComputerError::Backend(e.to_string()))?;
        Ok((p.x, p.y))
    }

    fn pid_at(&self, x: i32, y: i32) -> ComputerResult<u32> {
        dpi();
        let overlay = OVERLAY_HWND.load(Ordering::SeqCst);
        // SAFETY: plain window queries.
        unsafe {
            let h = WindowFromPoint(POINT { x, y });
            let root = if h.0.is_null() { h } else { GetAncestor(h, GA_ROOT) };
            if !root.0.is_null() && root.0 as isize != overlay {
                return Ok(window_pid(root));
            }
            // fall back to z-order (the overlay is never the answer)
            for w in enum_top_windows() {
                if w.0 as isize == overlay || !IsWindowVisible(w).as_bool() || IsIconic(w).as_bool() || is_cloaked(w) {
                    continue;
                }
                if window_rect(w).is_some_and(|r| x >= r.left && x < r.right && y >= r.top && y < r.bottom) {
                    return Ok(window_pid(w));
                }
            }
        }
        Ok(0)
    }

    fn move_to(&self, x: i32, y: i32) -> ComputerResult<()> {
        dpi();
        send(&[abs_move(x, y)])
    }

    fn click_at(&self, x: i32, y: i32, button: MouseButton, double: bool) -> ComputerResult<()> {
        dpi();
        let (down, up) = buttons(button);
        send(&[abs_move(x, y), mouse(down, 0, 0, 0), mouse(up, 0, 0, 0)])?;
        if double || button == MouseButton::Double {
            std::thread::sleep(Duration::from_millis(60));
            send(&[mouse(down, 0, 0, 0), mouse(up, 0, 0, 0)])?;
        }
        Ok(())
    }

    fn drag(&self, from: (i32, i32), to: (i32, i32)) -> ComputerResult<()> {
        dpi();
        send(&[abs_move(from.0, from.1), mouse(MOUSEEVENTF_LEFTDOWN, 0, 0, 0)])?;
        for i in 1..=12 {
            std::thread::sleep(Duration::from_millis(15));
            let x = from.0 + (to.0 - from.0) * i / 12;
            let y = from.1 + (to.1 - from.1) * i / 12;
            send(&[abs_move(x, y)])?;
        }
        std::thread::sleep(Duration::from_millis(40));
        send(&[mouse(MOUSEEVENTF_LEFTUP, 0, 0, 0)])
    }

    fn scroll_at(&self, x: i32, y: i32, direction: ScrollDirection, notches: u32) -> ComputerResult<()> {
        dpi();
        send(&[abs_move(x, y)])?;
        let (flags, sign) = match direction {
            ScrollDirection::Up => (MOUSEEVENTF_WHEEL, 1),
            ScrollDirection::Down => (MOUSEEVENTF_WHEEL, -1),
            ScrollDirection::Right => (MOUSEEVENTF_HWHEEL, 1),
            ScrollDirection::Left => (MOUSEEVENTF_HWHEEL, -1),
        };
        for _ in 0..notches.clamp(1, 15) {
            std::thread::sleep(Duration::from_millis(30));
            send(&[mouse(flags, 0, 0, 120 * sign)])?;
        }
        Ok(())
    }

    fn focused_is_password(&self) -> ComputerResult<bool> {
        let _com = Com::init();
        let auto = uia()?;
        // SAFETY: COM calls on live interfaces.
        unsafe { Ok(auto.GetFocusedElement().is_ok_and(|el| el.CurrentIsPassword().is_ok_and(|b| b.as_bool()))) }
    }

    fn arm_guard(&self, mouse: bool, on_trip: TripFn) -> ComputerResult<()> {
        if let Ok(mut g) = GUARD.trip.lock() {
            *g = Some(on_trip);
        }
        self.armed_with.store(u32::from(mouse), Ordering::SeqCst);
        self.post(MSG_ARM, usize::from(mouse))
    }

    fn disarm_guard(&self) {
        GUARD.armed.store(false, Ordering::SeqCst);
        let _ = self.post(MSG_DISARM, 0);
        if let Ok(mut g) = GUARD.trip.lock() {
            *g = None;
        }
    }

    fn overlay(&self, badge: Option<&str>) -> ComputerResult<()> {
        match badge {
            Some(text) => {
                if let Ok(mut b) = BADGE.lock() {
                    text.clone_into(&mut b);
                }
                self.post(MSG_OVERLAY, 0)
            }
            None => self.post(MSG_OVERLAY_HIDE, 0),
        }
    }

    fn own_windows(&self, pids: &[u32], hide: bool, minimize: bool) -> ComputerResult<()> {
        let overlay = OVERLAY_HWND.load(Ordering::SeqCst);
        // SAFETY: window calls on handles owned by this process.
        unsafe {
            if hide {
                let mut changed = self.changed.lock();
                for h in enum_top_windows() {
                    if h.0 as isize == overlay || !pids.contains(&window_pid(h)) || !IsWindowVisible(h).as_bool() {
                        continue;
                    }
                    if changed.iter().any(|(c, _)| *c == h.0 as isize) {
                        continue;
                    }
                    // Only windows of this very process accept a display affinity.
                    let _ = SetWindowDisplayAffinity(h, WDA_EXCLUDEFROMCAPTURE);
                    let ex = GetWindowLongW(h, GWL_EXSTYLE) as u32;
                    let tool_or_topmost = ex & (WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0) != 0;
                    let minimized = minimize && !tool_or_topmost && !IsIconic(h).as_bool();
                    if minimized {
                        let _ = ShowWindow(h, SW_MINIMIZE);
                    }
                    changed.push((h.0 as isize, minimized));
                }
            } else {
                let changed = std::mem::take(&mut *self.changed.lock());
                for (h, minimized) in changed {
                    let h = hwnd_from(h);
                    let _ = SetWindowDisplayAffinity(h, WDA_NONE);
                    if minimized {
                        let _ = ShowWindow(h, SW_RESTORE);
                    }
                }
            }
        }
        Ok(())
    }
}
