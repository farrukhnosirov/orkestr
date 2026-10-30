//! Windows backend, read-only on the real machine (ADR-0055; desktop
//! automation safety): volume / devices / sessions / media / battery /
//! pointer speed / layout / monitors are only READ. The one write — the
//! master volume — sets the value it just read (no audible change) and
//! restores it. No window, key, clipboard or cursor of the user is touched.
#![cfg(windows)]

use orkestr_computer::backend::WindowsBackend;
use orkestr_computer::backend::{ComputerBackend, PcReply, PcRequest};
use orkestr_computer::ComputerError;
use orkestr_protocol::methods::computer::AudioDeviceKind;

fn b() -> WindowsBackend {
    WindowsBackend::new()
}

/// Ok, or a machine without that hardware (CI, VMs).
fn tolerate(r: Result<PcReply, ComputerError>) -> Option<PcReply> {
    match r {
        Ok(v) => Some(v),
        Err(ComputerError::NotFound(_) | ComputerError::Unsupported(_)) => None,
        Err(e) => panic!("backend error: {e}"),
    }
}

#[test]
fn reads_volume_and_restores_it_exactly() {
    let b = b();
    let Some(r) = tolerate(b.pc(&PcRequest::GetVolume)) else { return };
    let (before, muted) = r.level().unwrap();
    assert!(before <= 100);
    // write the same value back: proves the write path, changes nothing
    let (after, m2) = b.pc(&PcRequest::SetVolume(before)).unwrap().level().unwrap();
    assert_eq!((after, m2), (before, muted));
    println!("volume {before}% muted={muted}");
}

#[test]
fn lists_audio_devices_sessions_and_media() {
    let b = b();
    if let Some(r) = tolerate(b.pc(&PcRequest::AudioDevices(AudioDeviceKind::Output))) {
        let d = r.devices().unwrap();
        assert!(d.iter().filter(|x| x.default).count() <= 1);
        println!("outputs: {:?}", d.iter().map(|x| (&x.name, x.default)).collect::<Vec<_>>());
    }
    if let Some(r) = tolerate(b.pc(&PcRequest::AudioDevices(AudioDeviceKind::Input))) {
        println!("inputs: {}", r.devices().unwrap().len());
    }
    if let Some(r) = tolerate(b.pc(&PcRequest::GetMic)) {
        assert!(r.level().unwrap().0 <= 100);
    }
    if let Some(r) = tolerate(b.pc(&PcRequest::AudioSessions)) {
        println!("sessions: {:?}", r.sessions().unwrap().iter().map(|s| (&s.app, s.percent)).collect::<Vec<_>>());
    }
    if let Some(r) = tolerate(b.pc(&PcRequest::NowPlaying)) {
        println!("now playing: {:?}", r.media().unwrap().map(|m| (m.app, m.playing)));
    }
}

#[test]
fn reads_system_state() {
    let b = b();
    let bat = b.pc(&PcRequest::Battery).unwrap().battery().unwrap();
    assert!(bat.percent.is_none_or(|p| p <= 100));
    let (speed, _) = b.pc(&PcRequest::GetMouseSpeed).unwrap().level().unwrap();
    assert!((1..=20).contains(&speed), "{speed}");
    let layout = b.pc(&PcRequest::GetKeyboardLayout).unwrap().text().unwrap();
    assert!(!layout.is_empty());
    let mon = b.pc(&PcRequest::MonitorBounds).unwrap().rect().unwrap();
    let work = b.pc(&PcRequest::WorkArea(None)).unwrap().rect().unwrap();
    assert!(mon.width() > 0 && work.width() > 0 && work.width() <= mon.width());
    let (x, y) = b.pc(&PcRequest::CursorPos).unwrap().point().unwrap();
    println!("battery {bat:?}, pointer speed {speed}, layout {layout}, monitor {mon:?}, cursor {x},{y}");
    // brightness: laptop panel or DDC/CI, else a clear Unsupported
    match b.brightness(None) {
        Ok(v) => assert!(v <= 100),
        Err(e) => assert!(matches!(e, ComputerError::Unsupported(_)), "{e}"),
    }
}

#[test]
fn window_list_is_readable() {
    let ws = b().windows().unwrap();
    assert!(ws.iter().filter(|w| w.active).count() <= 1);
}
