//! Windows implementation of the extended PC control requests
//! ([`super::pc`]): Core Audio (`IAudioEndpointVolume`,
//! `IAudioSessionManager2`, `IPolicyConfig` for the default device), the
//! media session (GSMTC), window geometry / topmost / opacity, the cursor,
//! pointer speed, keyboard layouts, displays (monitor power, DisplaySwitch,
//! DDC/CI brightness via dxva2), screen capture, radios
//! (`Windows.Devices.Radios`), battery and the recycle bin.
#![allow(unsafe_code, non_snake_case)]

use std::ffi::c_void;

use windows::core::{Interface as _, BOOL, HRESULT, HSTRING, PCWSTR, PWSTR};
use windows::Devices::Radios::{Radio, RadioAccessStatus, RadioKind as WinRadioKind, RadioState};
use windows::Media::Control::{
    GlobalSystemMediaTransportControlsSessionManager as MediaManager,
    GlobalSystemMediaTransportControlsSessionPlaybackStatus as PlaybackStatus,
};
use windows::Win32::Devices::Display::{
    DestroyPhysicalMonitors, GetMonitorBrightness, GetNumberOfPhysicalMonitorsFromHMONITOR,
    GetPhysicalMonitorsFromHMONITOR, SetMonitorBrightness, PHYSICAL_MONITOR,
};
use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_EXTENDED_FRAME_BOUNDS};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, MonitorFromPoint, MonitorFromWindow, HDC, HMONITOR, MONITORINFO,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{
    eCapture, eCommunications, eConsole, eMultimedia, eRender, EDataFlow, IAudioSessionControl2, IAudioSessionManager2,
    IMMDevice, IMMDeviceEnumerator, ISimpleAudioVolume, MMDeviceEnumerator, DEVICE_STATE_ACTIVE,
};
use windows::Win32::System::Com::StructuredStorage::{PropVariantClear, PropVariantToStringAlloc};
use windows::Win32::System::Com::{CoCreateInstance, CoTaskMemFree, CLSCTX_ALL, STGM_READ};
use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyboardLayout, GetKeyboardLayoutList, HKL};
use windows::Win32::UI::Shell::{SHEmptyRecycleBinW, SHERB_NOCONFIRMATION, SHERB_NOPROGRESSUI, SHERB_NOSOUND};
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetForegroundWindow, GetWindowLongW, GetWindowThreadProcessId, IsIconic, IsZoomed, PostMessageW,
    SetCursorPos, SetLayeredWindowAttributes, SetWindowLongW, SetWindowPos, ShowWindow, SystemParametersInfoW,
    GWL_EXSTYLE, HWND_BROADCAST, HWND_NOTOPMOST, HWND_TOPMOST, LWA_ALPHA, SC_MONITORPOWER, SPIF_SENDCHANGE,
    SPIF_UPDATEINIFILE, SPI_GETMOUSESPEED, SPI_SETMOUSESPEED, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_NOZORDER,
    SW_RESTORE, WM_INPUTLANGCHANGEREQUEST, WM_SYSCOMMAND, WS_EX_LAYERED,
};

use orkestr_protocol::methods::computer::{AudioDeviceKind, DisplayOp, MouseButton, RadioKind};

use super::pc::{AudioDeviceInfo, AudioSessionInfo, BatteryInfo, MediaInfo, PcReply, PcRequest, Rect};
use super::windows::{find_element, hwnd_of, mouse_input, process_name, send, uia, Com, WindowsBackend};
use super::ComputerBackend as _;
use crate::error::{ComputerError, ComputerResult};

fn be(what: &str, e: impl std::fmt::Display) -> ComputerError {
    ComputerError::Backend(format!("{what}: {e}"))
}

// ---------------------------------------------------------------------------
// Core Audio
// ---------------------------------------------------------------------------

/// `IPolicyConfig` (Windows 7+, used by the Sound control panel) — the only
/// way to change the default endpoint; not in the SDK headers.
#[windows::core::interface("f8679f50-850a-41cf-9c72-430f290290c8")]
unsafe trait IPolicyConfig: windows::core::IUnknown {
    fn GetMixFormat(&self, id: PCWSTR, f: *mut *mut c_void) -> HRESULT;
    fn GetDeviceFormat(&self, id: PCWSTR, d: i32, f: *mut *mut c_void) -> HRESULT;
    fn ResetDeviceFormat(&self, id: PCWSTR) -> HRESULT;
    fn SetDeviceFormat(&self, id: PCWSTR, a: *mut c_void, b: *mut c_void) -> HRESULT;
    fn GetProcessingPeriod(&self, id: PCWSTR, d: i32, a: *mut i64, b: *mut i64) -> HRESULT;
    fn SetProcessingPeriod(&self, id: PCWSTR, a: *mut i64) -> HRESULT;
    fn GetShareMode(&self, id: PCWSTR, m: *mut c_void) -> HRESULT;
    fn SetShareMode(&self, id: PCWSTR, m: *mut c_void) -> HRESULT;
    fn GetPropertyValue(&self, id: PCWSTR, k: *const c_void, v: *mut c_void) -> HRESULT;
    fn SetPropertyValue(&self, id: PCWSTR, k: *const c_void, v: *mut c_void) -> HRESULT;
    fn SetDefaultEndpoint(&self, id: PCWSTR, role: i32) -> HRESULT;
    fn SetEndpointVisibility(&self, id: PCWSTR, visible: i32) -> HRESULT;
}

const CLSID_POLICY_CONFIG: windows::core::GUID = windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

fn flow(kind: AudioDeviceKind) -> EDataFlow {
    match kind {
        AudioDeviceKind::Output => eRender,
        AudioDeviceKind::Input => eCapture,
    }
}

fn enumerator() -> ComputerResult<IMMDeviceEnumerator> {
    // SAFETY: COM is initialized by the caller.
    unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }.map_err(|e| be("audio devices", e))
}

fn default_device(kind: AudioDeviceKind) -> ComputerResult<IMMDevice> {
    // SAFETY: live interface.
    unsafe { enumerator()?.GetDefaultAudioEndpoint(flow(kind), eConsole) }.map_err(|_| {
        ComputerError::NotFound(match kind {
            AudioDeviceKind::Output => "no audio output device".into(),
            AudioDeviceKind::Input => "no microphone".into(),
        })
    })
}

fn endpoint(kind: AudioDeviceKind) -> ComputerResult<IAudioEndpointVolume> {
    // SAFETY: live interface.
    unsafe { default_device(kind)?.Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None) }.map_err(|e| be("volume", e))
}

#[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
fn pct(f: f32) -> u32 {
    (f * 100.0).round().clamp(0.0, 100.0) as u32
}

#[allow(clippy::cast_precision_loss)]
fn scalar(p: u32) -> f32 {
    p.min(100) as f32 / 100.0
}

fn endpoint_level(ep: &IAudioEndpointVolume) -> ComputerResult<PcReply> {
    // SAFETY: live interface.
    unsafe {
        let v = ep.GetMasterVolumeLevelScalar().map_err(|e| be("volume", e))?;
        let m = ep.GetMute().map_err(|e| be("volume", e))?.as_bool();
        Ok(PcReply::Level(pct(v), m))
    }
}

fn endpoint_set(kind: AudioDeviceKind, level: Option<u32>, mute: Option<Option<bool>>) -> ComputerResult<PcReply> {
    let ep = endpoint(kind)?;
    // SAFETY: live interface; null event context.
    unsafe {
        if let Some(p) = level {
            ep.SetMasterVolumeLevelScalar(scalar(p), std::ptr::null()).map_err(|e| be("volume", e))?;
        }
        if let Some(m) = mute {
            let cur = ep.GetMute().map_err(|e| be("volume", e))?.as_bool();
            ep.SetMute(m.unwrap_or(!cur), std::ptr::null()).map_err(|e| be("mute", e))?;
        }
    }
    endpoint_level(&ep)
}

fn pwstr_take(p: PWSTR) -> String {
    // SAFETY: COM-allocated string, freed here.
    unsafe {
        let s = p.to_string().unwrap_or_default();
        CoTaskMemFree(Some(p.0 as *const c_void));
        s
    }
}

fn device_name(d: &IMMDevice) -> String {
    // SAFETY: live interfaces; the PROPVARIANT is cleared.
    unsafe {
        let Ok(store) = d.OpenPropertyStore(STGM_READ) else { return String::new() };
        let Ok(mut pv) = store.GetValue(&PKEY_Device_FriendlyName) else { return String::new() };
        let name = PropVariantToStringAlloc(&raw const pv).map(pwstr_take).unwrap_or_default();
        let _ = PropVariantClear(&raw mut pv);
        name
    }
}

fn device_id(d: &IMMDevice) -> String {
    // SAFETY: live interface.
    unsafe { d.GetId() }.map(pwstr_take).unwrap_or_default()
}

fn devices(kind: AudioDeviceKind) -> ComputerResult<Vec<AudioDeviceInfo>> {
    let e = enumerator()?;
    let default = default_device(kind).ok().map(|d| device_id(&d)).unwrap_or_default();
    let mut out = Vec::new();
    // SAFETY: live interfaces.
    unsafe {
        let all = e.EnumAudioEndpoints(flow(kind), DEVICE_STATE_ACTIVE).map_err(|e| be("audio devices", e))?;
        for i in 0..all.GetCount().unwrap_or(0) {
            let Ok(d) = all.Item(i) else { continue };
            let id = device_id(&d);
            out.push(AudioDeviceInfo { name: device_name(&d), default: id == default, id });
        }
    }
    Ok(out)
}

fn set_default_device(id: &str) -> ComputerResult<()> {
    // SAFETY: COM call on a live interface with a valid wide string.
    unsafe {
        let pc: IPolicyConfig =
            CoCreateInstance(&CLSID_POLICY_CONFIG, None, CLSCTX_ALL).map_err(|e| be("audio policy", e))?;
        let w = HSTRING::from(id);
        for role in [eConsole, eMultimedia, eCommunications] {
            pc.SetDefaultEndpoint(PCWSTR(w.as_ptr()), role.0).ok().map_err(|e| be("default device", e))?;
        }
    }
    Ok(())
}

/// `(name, pid, volume interface)` of every session of the default output.
fn sessions() -> ComputerResult<Vec<(String, u32, ISimpleAudioVolume)>> {
    let dev = default_device(AudioDeviceKind::Output)?;
    let mut out = Vec::new();
    // SAFETY: live interfaces.
    unsafe {
        let mgr: IAudioSessionManager2 = dev.Activate(CLSCTX_ALL, None).map_err(|e| be("audio sessions", e))?;
        let list = mgr.GetSessionEnumerator().map_err(|e| be("audio sessions", e))?;
        for i in 0..list.GetCount().unwrap_or(0) {
            let Ok(ctl) = list.GetSession(i) else { continue };
            let Ok(c2) = ctl.cast::<IAudioSessionControl2>() else { continue };
            let pid = c2.GetProcessId().unwrap_or(0);
            if pid == 0 {
                continue; // system sounds
            }
            let Ok(vol) = ctl.cast::<ISimpleAudioVolume>() else { continue };
            let name = crate::procs::norm(&process_name(pid));
            out.push((name, pid, vol));
        }
    }
    Ok(out)
}

fn app_sessions(app: &str) -> ComputerResult<Vec<ISimpleAudioVolume>> {
    let mut wanted: Vec<String> = crate::apps::process_names(app).iter().map(|n| crate::procs::norm(n)).collect();
    wanted.push(crate::procs::norm(app));
    let found: Vec<ISimpleAudioVolume> = sessions()?
        .into_iter()
        .filter(|(n, _, _)| !n.is_empty() && wanted.iter().any(|w| !w.is_empty() && (n == w || n.contains(w.as_str()))))
        .map(|(_, _, v)| v)
        .collect();
    if found.is_empty() {
        return Err(ComputerError::NotFound(format!("“{app}” is not playing sound right now")));
    }
    Ok(found)
}

fn app_level(list: &[ISimpleAudioVolume]) -> ComputerResult<PcReply> {
    // SAFETY: live interface.
    unsafe {
        let v = list[0].GetMasterVolume().map_err(|e| be("app volume", e))?;
        let m = list[0].GetMute().map_err(|e| be("app volume", e))?.as_bool();
        Ok(PcReply::Level(pct(v), m))
    }
}

// ---------------------------------------------------------------------------
// Media session (GSMTC)
// ---------------------------------------------------------------------------

fn media_session() -> ComputerResult<Option<windows::Media::Control::GlobalSystemMediaTransportControlsSession>> {
    let mgr = MediaManager::RequestAsync().and_then(|op| op.get()).map_err(|e| be("media session", e))?;
    Ok(mgr.GetCurrentSession().ok())
}

fn now_playing() -> ComputerResult<Option<MediaInfo>> {
    let Some(s) = media_session()? else { return Ok(None) };
    let props = s.TryGetMediaPropertiesAsync().and_then(|op| op.get()).map_err(|e| be("media session", e))?;
    let playing = s.GetPlaybackInfo().and_then(|i| i.PlaybackStatus()).is_ok_and(|st| st == PlaybackStatus::Playing);
    Ok(Some(MediaInfo {
        title: props.Title().map(|t| t.to_string()).unwrap_or_default(),
        artist: props.Artist().map(|t| t.to_string()).unwrap_or_default(),
        app: s.SourceAppUserModelId().map(|t| t.to_string()).unwrap_or_default(),
        playing,
    }))
}

fn media_seek(delta_secs: i64) -> ComputerResult<()> {
    let s = media_session()?.ok_or_else(|| ComputerError::NotFound("nothing is playing".into()))?;
    let tl = s.GetTimelineProperties().map_err(|e| be("media timeline", e))?;
    let pos = tl.Position().map_err(|e| be("media timeline", e))?.Duration;
    let end = tl.EndTime().map(|t| t.Duration).unwrap_or(i64::MAX);
    let target = pos.saturating_add(delta_secs.saturating_mul(10_000_000)).clamp(0, end.max(0));
    let ok = s.TryChangePlaybackPositionAsync(target).and_then(|op| op.get()).map_err(|e| be("media seek", e))?;
    if ok {
        Ok(())
    } else {
        Err(ComputerError::Unsupported("the player does not allow seeking".into()))
    }
}

// ---------------------------------------------------------------------------
// Windows / monitors / cursor
// ---------------------------------------------------------------------------

fn rect_of(r: RECT) -> Rect {
    Rect { left: r.left, top: r.top, right: r.right, bottom: r.bottom }
}

fn monitor_info(m: HMONITOR) -> ComputerResult<MONITORINFO> {
    let mut mi =
        MONITORINFO { cbSize: u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(40), ..Default::default() };
    // SAFETY: valid out pointer with cbSize set.
    if unsafe { GetMonitorInfoW(m, &raw mut mi) }.as_bool() {
        Ok(mi)
    } else {
        Err(ComputerError::Backend("monitor info unavailable".into()))
    }
}

fn cursor_monitor() -> HMONITOR {
    let mut p = POINT::default();
    // SAFETY: valid out pointer; plain queries.
    unsafe {
        let _ = GetCursorPos(&raw mut p);
        MonitorFromPoint(p, MONITOR_DEFAULTTONEAREST)
    }
}

/// Visible frame (without the invisible resize borders).
fn frame_rect(h: HWND) -> Option<RECT> {
    let mut r = RECT::default();
    // SAFETY: r is a valid out buffer of the given size.
    unsafe {
        DwmGetWindowAttribute(
            h,
            DWMWA_EXTENDED_FRAME_BOUNDS,
            (&raw mut r).cast(),
            u32::try_from(std::mem::size_of::<RECT>()).unwrap_or(16),
        )
        .ok()
        .map(|()| r)
    }
    .or_else(|| super::windows::window_rect(h))
}

fn place(h: HWND, r: Rect) -> ComputerResult<()> {
    // SAFETY: Win32 calls on a valid window handle.
    unsafe {
        if IsZoomed(h).as_bool() || IsIconic(h).as_bool() {
            let _ = ShowWindow(h, SW_RESTORE);
        }
        // compensate the invisible borders so the visible frame lands on `r`
        let (outer, inner) = (super::windows::window_rect(h), frame_rect(h));
        let (dl, dt, dr, db) = match (outer, inner) {
            (Some(o), Some(i)) => (i.left - o.left, i.top - o.top, o.right - i.right, o.bottom - i.bottom),
            _ => (0, 0, 0, 0),
        };
        SetWindowPos(
            h,
            None,
            r.left - dl,
            r.top - dt,
            r.width() + dl + dr,
            r.height() + dt + db,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )
        .map_err(|e| be("move window", e))
    }
}

fn set_opacity(h: HWND, percent: u32) -> ComputerResult<()> {
    let p = percent.clamp(20, 100);
    // SAFETY: Win32 calls on a valid window handle.
    unsafe {
        let ex = GetWindowLongW(h, GWL_EXSTYLE);
        #[allow(clippy::cast_possible_wrap)]
        let layered = WS_EX_LAYERED.0 as i32;
        if p >= 100 {
            let _ = SetWindowLongW(h, GWL_EXSTYLE, ex & !layered);
            return Ok(());
        }
        let _ = SetWindowLongW(h, GWL_EXSTYLE, ex | layered);
        #[allow(clippy::cast_possible_truncation)]
        let alpha = (p * 255 / 100) as u8;
        SetLayeredWindowAttributes(h, COLORREF(0), alpha, LWA_ALPHA).map_err(|e| be("transparency", e))
    }
}

fn mouse_button(b: MouseButton, down: bool) -> ComputerResult<()> {
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_RIGHTDOWN,
        MOUSEEVENTF_RIGHTUP,
    };
    let f = match (b, down) {
        (MouseButton::Left | MouseButton::Double, true) => MOUSEEVENTF_LEFTDOWN,
        (MouseButton::Left | MouseButton::Double, false) => MOUSEEVENTF_LEFTUP,
        (MouseButton::Right, true) => MOUSEEVENTF_RIGHTDOWN,
        (MouseButton::Right, false) => MOUSEEVENTF_RIGHTUP,
        (MouseButton::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
        (MouseButton::Middle, false) => MOUSEEVENTF_MIDDLEUP,
    };
    send(&[mouse_input(f, 0)])
}

fn element_center(window: Option<&str>, name: &str) -> ComputerResult<PcReply> {
    let h = match window {
        Some(s) => hwnd_of(s)?,
        // SAFETY: plain query.
        None => unsafe { GetForegroundWindow() },
    };
    let _com = Com::init();
    let auto = uia()?;
    let (el, label) = find_element(&auto, h, name)?;
    // SAFETY: live interface.
    let r = unsafe { el.CurrentBoundingRectangle() }.map_err(|e| be("UI Automation", e))?;
    if r.right <= r.left || r.bottom <= r.top {
        return Err(ComputerError::NotFound(format!("“{label}” is not on the screen")));
    }
    Ok(PcReply::Element((r.left + r.right) / 2, (r.top + r.bottom) / 2, label))
}

fn mouse_speed(set: Option<u32>) -> ComputerResult<PcReply> {
    // SAFETY: documented SPI calls with a valid out pointer / value.
    unsafe {
        if let Some(v) = set {
            let v = v.clamp(1, 20) as usize;
            SystemParametersInfoW(SPI_SETMOUSESPEED, 0, Some(v as *mut c_void), SPIF_UPDATEINIFILE | SPIF_SENDCHANGE)
                .map_err(|e| be("pointer speed", e))?;
        }
        let mut cur: u32 = 0;
        SystemParametersInfoW(SPI_GETMOUSESPEED, 0, Some((&raw mut cur).cast()), Default::default())
            .map_err(|e| be("pointer speed", e))?;
        Ok(PcReply::Level(cur, false))
    }
}

// ---------------------------------------------------------------------------
// Keyboard layouts
// ---------------------------------------------------------------------------

/// Primary language ids by name (lowercase, en + ru).
const LANGS: &[(u16, &str, &[&str])] = &[
    (0x09, "en", &["en", "eng", "english", "английский", "английская", "англ", "инглиш"]),
    (0x19, "ru", &["ru", "rus", "russian", "русский", "русская", "рус"]),
    (0x22, "uk", &["uk", "ua", "ukrainian", "украинский", "украинская"]),
    (0x07, "de", &["de", "german", "немецкий", "немецкая"]),
    (0x0c, "fr", &["fr", "french", "французский"]),
    (0x0a, "es", &["es", "spanish", "испанский"]),
    (0x10, "it", &["it", "italian", "итальянский"]),
    (0x43, "uz", &["uz", "uzbek", "узбекский"]),
    (0x3f, "kk", &["kk", "kazakh", "казахский"]),
    (0x23, "be", &["be", "belarusian", "белорусский"]),
    (0x1f, "tr", &["tr", "turkish", "турецкий"]),
    (0x15, "pl", &["pl", "polish", "польский"]),
    (0x04, "zh", &["zh", "chinese", "китайский"]),
    (0x11, "ja", &["ja", "japanese", "японский"]),
    (0x01, "ar", &["ar", "arabic", "арабский"]),
];

fn hkl_lang(h: HKL) -> u16 {
    #[allow(clippy::cast_possible_truncation)]
    let langid = (h.0 as usize & 0xffff) as u16;
    langid
}

fn lang_name(langid: u16) -> String {
    let primary = langid & 0x3ff;
    LANGS
        .iter()
        .find(|(p, _, _)| *p == primary)
        .map_or_else(|| format!("0x{langid:04x}"), |(_, code, _)| (*code).to_owned())
}

fn foreground_layout() -> HKL {
    // SAFETY: plain queries.
    unsafe {
        let fg = GetForegroundWindow();
        let tid = GetWindowThreadProcessId(fg, None);
        GetKeyboardLayout(tid)
    }
}

fn set_layout(lang: Option<&str>) -> ComputerResult<String> {
    let mut list = vec![HKL::default(); 32];
    // SAFETY: buffer of 32 entries.
    let n = usize::try_from(unsafe { GetKeyboardLayoutList(Some(&mut list)) }).unwrap_or(0);
    list.truncate(n);
    if list.is_empty() {
        return Err(ComputerError::Backend("no keyboard layouts".into()));
    }
    let cur = foreground_layout();
    let target = match lang {
        None => {
            let i = list.iter().position(|h| h.0 == cur.0).map_or(0, |i| (i + 1) % list.len());
            list[i]
        }
        Some(l) => {
            let l = l.trim().to_lowercase();
            let primary = LANGS
                .iter()
                .find(|(_, code, names)| *code == l || names.iter().any(|n| l.starts_with(n) || n.starts_with(&l)))
                .map(|(p, _, _)| *p)
                .ok_or_else(|| ComputerError::NotFound(format!("unknown language “{l}”")))?;
            *list.iter().find(|h| hkl_lang(**h) & 0x3ff == primary).ok_or_else(|| {
                ComputerError::NotFound(format!(
                    "the “{l}” keyboard layout is not installed (Settings → Time & language)"
                ))
            })?
        }
    };
    // SAFETY: posting a documented message to the foreground window.
    unsafe {
        let fg = GetForegroundWindow();
        PostMessageW(Some(fg), WM_INPUTLANGCHANGEREQUEST, WPARAM(0), LPARAM(target.0 as isize))
            .map_err(|e| be("keyboard layout", e))?;
    }
    Ok(lang_name(hkl_lang(target)))
}

// ---------------------------------------------------------------------------
// Displays: power, projection, DDC/CI brightness
// ---------------------------------------------------------------------------

fn monitors() -> Vec<HMONITOR> {
    unsafe extern "system" fn cb(m: HMONITOR, _: HDC, _: *mut RECT, lp: LPARAM) -> BOOL {
        // SAFETY: lp is the &mut Vec passed below.
        let v = unsafe { &mut *(lp.0 as *mut Vec<HMONITOR>) };
        v.push(m);
        BOOL(1)
    }
    let mut v: Vec<HMONITOR> = Vec::new();
    // SAFETY: the callback only runs during this call.
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(cb), LPARAM(&raw mut v as isize));
    }
    v
}

/// DDC/CI brightness of external monitors (dxva2): `Some` sets every
/// monitor that answers, returns the first one's level.
pub(super) fn ddc_brightness(set: Option<u32>) -> ComputerResult<u32> {
    let mut first: Option<u32> = None;
    for m in monitors() {
        let mut n = 0u32;
        // SAFETY: documented dxva2 calls; the array is destroyed below.
        unsafe {
            if GetNumberOfPhysicalMonitorsFromHMONITOR(m, &raw mut n).is_err() || n == 0 {
                continue;
            }
            let mut phys = vec![PHYSICAL_MONITOR::default(); n as usize];
            if GetPhysicalMonitorsFromHMONITOR(m, &mut phys).is_err() {
                continue;
            }
            for p in &phys {
                let (mut lo, mut cur, mut hi) = (0u32, 0u32, 0u32);
                if GetMonitorBrightness(p.hPhysicalMonitor, &raw mut lo, &raw mut cur, &raw mut hi) == 0 || hi <= lo {
                    continue;
                }
                let level = if let Some(v) = set {
                    let raw = lo + (hi - lo) * v.min(100) / 100;
                    if SetMonitorBrightness(p.hPhysicalMonitor, raw) == 0 {
                        continue;
                    }
                    v.min(100)
                } else {
                    (cur.saturating_sub(lo)) * 100 / (hi - lo)
                };
                first.get_or_insert(level);
            }
            let _ = DestroyPhysicalMonitors(&phys);
        }
    }
    first.ok_or_else(|| {
        ComputerError::Unsupported(
            "this display does not support brightness control (no laptop panel and no DDC/CI monitor)".into(),
        )
    })
}

fn display(b: &WindowsBackend, op: DisplayOp) -> ComputerResult<()> {
    let arg = match op {
        DisplayOp::ScreenOff => {
            // SAFETY: documented broadcast (posted, never blocks).
            return unsafe {
                PostMessageW(Some(HWND_BROADCAST), WM_SYSCOMMAND, WPARAM(SC_MONITORPOWER as usize), LPARAM(2))
            }
            .map_err(|e| be("screen off", e));
        }
        DisplayOp::PcScreenOnly => "/internal",
        DisplayOp::Duplicate => "/clone",
        DisplayOp::Extend => "/extend",
        DisplayOp::SecondScreenOnly => "/external",
    };
    b.shell_open("DisplaySwitch.exe", Some(arg))
}

fn capture(r: Rect, path: Option<&std::path::Path>, clipboard: bool) -> ComputerResult<()> {
    use crate::cursor::screen::{encode_png, CursorBackend as _, ScreenRect};
    let img = super::cursor_win::WindowsCursor::new().capture(ScreenRect {
        x: r.left,
        y: r.top,
        w: r.width(),
        h: r.height(),
    })?;
    let rgb: Vec<u8> = img.bgra.as_chunks::<4>().0.iter().flat_map(|p| [p[2], p[1], p[0]]).collect();
    let png = encode_png(img.width, img.height, &rgb)?;
    if let Some(p) = path {
        if let Some(dir) = p.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(p, &png)?;
    }
    if clipboard {
        let tmp = std::env::temp_dir().join(format!("orkestr-shot-{}.png", std::process::id()));
        std::fs::write(&tmp, &png)?;
        let r = super::windows::powershell(&format!(
            "Add-Type -AssemblyName System.Windows.Forms, System.Drawing\n\
             $i = [System.Drawing.Image]::FromFile({})\n\
             [System.Windows.Forms.Clipboard]::SetImage($i)\n$i.Dispose()",
            super::windows::ps_str(&tmp.display().to_string())
        ));
        let _ = std::fs::remove_file(&tmp);
        r?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Radios, battery, recycle bin
// ---------------------------------------------------------------------------

fn radio(kind: RadioKind, on: bool) -> ComputerResult<String> {
    let access = Radio::RequestAccessAsync().and_then(|op| op.get()).map_err(|e| be("radios", e))?;
    if access != RadioAccessStatus::Allowed {
        return Err(ComputerError::Forbidden(
            "Windows denies radio control to apps (Settings → Privacy → Radios)".into(),
        ));
    }
    let radios = Radio::GetRadiosAsync().and_then(|op| op.get()).map_err(|e| be("radios", e))?;
    let mut names = Vec::new();
    for i in 0..radios.Size().unwrap_or(0) {
        let Ok(r) = radios.GetAt(i) else { continue };
        let k = r.Kind().unwrap_or(WinRadioKind::Other);
        let wanted = match kind {
            RadioKind::Wifi => k == WinRadioKind::WiFi,
            RadioKind::Bluetooth => k == WinRadioKind::Bluetooth,
            RadioKind::All => true,
        };
        if !wanted {
            continue;
        }
        let state = if on { RadioState::On } else { RadioState::Off };
        r.SetStateAsync(state).and_then(|op| op.get()).map_err(|e| be("radio", e))?;
        names.push(r.Name().map(|n| n.to_string()).unwrap_or_default());
    }
    if names.is_empty() {
        return Err(ComputerError::NotFound(format!("no {kind:?} radio on this computer")));
    }
    Ok(names.join(", "))
}

fn battery() -> ComputerResult<BatteryInfo> {
    let mut s = SYSTEM_POWER_STATUS::default();
    // SAFETY: valid out pointer.
    unsafe { GetSystemPowerStatus(&raw mut s) }.map_err(|e| be("battery", e))?;
    let none = s.BatteryFlag & 128 != 0 || s.BatteryLifePercent == 255;
    Ok(BatteryInfo {
        percent: (!none).then_some(u32::from(s.BatteryLifePercent)),
        charging: s.BatteryFlag & 8 != 0,
        plugged: s.ACLineStatus == 1,
    })
}

fn empty_recycle_bin() -> ComputerResult<()> {
    // SAFETY: documented call; null root = every drive.
    match unsafe { SHEmptyRecycleBinW(None, PCWSTR::null(), SHERB_NOCONFIRMATION | SHERB_NOPROGRESSUI | SHERB_NOSOUND) }
    {
        Ok(()) => Ok(()),
        // E_UNEXPECTED: the bin is already empty
        Err(e) if e.code().0 as u32 == 0x8000_FFFF => Ok(()),
        Err(e) => Err(be("recycle bin", e)),
    }
}

// ---------------------------------------------------------------------------
// Dispatcher
// ---------------------------------------------------------------------------

/// Handles one request.
pub(super) fn handle(b: &WindowsBackend, req: &PcRequest) -> ComputerResult<PcReply> {
    let _com = Com::init();
    let fg = || -> HWND {
        // SAFETY: plain query.
        unsafe { GetForegroundWindow() }
    };
    let window = |h: &str| hwnd_of(h);
    match req {
        PcRequest::GetVolume => endpoint_level(&endpoint(AudioDeviceKind::Output)?),
        PcRequest::SetVolume(v) => endpoint_set(AudioDeviceKind::Output, Some(*v), None),
        PcRequest::SetMute(m) => endpoint_set(AudioDeviceKind::Output, None, Some(*m)),
        PcRequest::GetMic => endpoint_level(&endpoint(AudioDeviceKind::Input)?),
        PcRequest::SetMic(v) => endpoint_set(AudioDeviceKind::Input, Some(*v), None),
        PcRequest::SetMicMute(m) => endpoint_set(AudioDeviceKind::Input, None, Some(*m)),
        PcRequest::GetAppVolume(app) => app_level(&app_sessions(app)?),
        PcRequest::SetAppVolume(app, v) => {
            let list = app_sessions(app)?;
            for s in &list {
                // SAFETY: live interface.
                unsafe { s.SetMasterVolume(scalar(*v), std::ptr::null()) }.map_err(|e| be("app volume", e))?;
            }
            app_level(&list)
        }
        PcRequest::SetAppMute(app, m) => {
            let list = app_sessions(app)?;
            // SAFETY: live interfaces.
            unsafe {
                let cur = list[0].GetMute().map_err(|e| be("app mute", e))?.as_bool();
                for s in &list {
                    s.SetMute(m.unwrap_or(!cur), std::ptr::null()).map_err(|e| be("app mute", e))?;
                }
            }
            app_level(&list)
        }
        PcRequest::AudioDevices(k) => Ok(PcReply::Devices(devices(*k)?)),
        PcRequest::SetDefaultAudioDevice(_, id) => set_default_device(id).map(|()| PcReply::Unit),
        PcRequest::AudioSessions => {
            let mut out = Vec::new();
            for (app, pid, vol) in sessions()? {
                // SAFETY: live interface.
                let (v, m) =
                    unsafe { (vol.GetMasterVolume().unwrap_or(1.0), vol.GetMute().is_ok_and(|b| b.as_bool())) };
                out.push(AudioSessionInfo { app, pid, percent: pct(v), muted: m });
            }
            Ok(PcReply::Sessions(out))
        }
        PcRequest::NowPlaying => Ok(PcReply::Media(now_playing()?)),
        PcRequest::MediaSeek(d) => media_seek(*d).map(|()| PcReply::Unit),
        PcRequest::WorkArea(h) => {
            let m = match h {
                // SAFETY: plain query on a valid handle.
                Some(h) => unsafe { MonitorFromWindow(window(h)?, MONITOR_DEFAULTTONEAREST) },
                None => cursor_monitor(),
            };
            Ok(PcReply::Rect(rect_of(monitor_info(m)?.rcWork)))
        }
        PcRequest::MonitorBounds => Ok(PcReply::Rect(rect_of(monitor_info(cursor_monitor())?.rcMonitor))),
        PcRequest::WindowRect(h) => frame_rect(window(h)?)
            .map(|r| PcReply::Rect(rect_of(r)))
            .ok_or_else(|| ComputerError::Backend("window rectangle unavailable".into())),
        PcRequest::PlaceWindow(h, r) => place(window(h)?, *r).map(|()| PcReply::Unit),
        PcRequest::SetTopmost(h, on) => {
            let h = window(h)?;
            let after = if *on { HWND_TOPMOST } else { HWND_NOTOPMOST };
            // SAFETY: valid handle.
            unsafe { SetWindowPos(h, Some(after), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE) }
                .map_err(|e| be("always on top", e))?;
            Ok(PcReply::Unit)
        }
        PcRequest::SetOpacity(h, p) => set_opacity(window(h)?, *p).map(|()| PcReply::Unit),
        PcRequest::CursorPos => {
            let mut p = POINT::default();
            // SAFETY: valid out pointer.
            unsafe { GetCursorPos(&raw mut p) }.map_err(|e| be("cursor", e))?;
            Ok(PcReply::Point(p.x, p.y))
        }
        PcRequest::SetCursor(x, y) => {
            // SAFETY: plain cursor move.
            unsafe { SetCursorPos(*x, *y) }.map_err(|e| be("cursor", e))?;
            Ok(PcReply::Unit)
        }
        PcRequest::MouseButton(btn, down) => mouse_button(*btn, *down).map(|()| PcReply::Unit),
        PcRequest::ElementCenter(w, name) => element_center(w.as_deref(), name),
        PcRequest::GetMouseSpeed => mouse_speed(None),
        PcRequest::SetMouseSpeed(v) => mouse_speed(Some(*v)),
        PcRequest::GetKeyboardLayout => Ok(PcReply::Text(lang_name(hkl_lang(foreground_layout())))),
        PcRequest::SetKeyboardLayout(l) => set_layout(l.as_deref()).map(PcReply::Text),
        PcRequest::Display(op) => display(b, *op).map(|()| PcReply::Unit),
        PcRequest::CaptureRect(r, p, clip) => capture(*r, p.as_deref(), *clip).map(|()| PcReply::Unit),
        PcRequest::Radio(k, on) => radio(*k, *on).map(PcReply::Text),
        PcRequest::Battery => Ok(PcReply::Battery(battery()?)),
        PcRequest::EmptyRecycleBin => {
            let _ = fg();
            empty_recycle_bin().map(|()| PcReply::Unit)
        }
    }
}
