//! Real-desktop smoke of cursor mode (ADR-0050) — **ignored by default**;
//! run explicitly on a Windows desktop:
//!
//! ```text
//! ORKESTR_DESKTOP_SMOKE=1 cargo test -p orkestr-computer --test cursor_desktop -- --ignored --nocapture --test-threads=1
//! ```
//!
//! Sandboxed: it opens its **own** throw-away window (a PowerShell WinForms
//! form "Orkestr Cursor Smoke", top-most, closes itself after 3 minutes), and
//! every model used here refuses to act unless that window is in front — so
//! no user window is ever typed into or closed. The clipboard is not used
//! (and is checked to be unchanged). Screenshots stay in memory.
//!
//! Covers: UI Automation elements → element click (DPI-correct), typing,
//! closing via a button, the overlay excluded from capture, Esc abort and
//! physical-mouse abort through the real low-level hooks (with the test
//! marker that makes `SendInput` events count as physical), injected Esc
//! NOT aborting, and — with `ORKESTR_SMOKE_CLAUDE=<claude.exe>` — one short
//! run of the real Claude Code CLI backend on the same window.
#![cfg(windows)]

use std::os::windows::process::CommandExt as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

use orkestr_computer::backend::{
    send_test_input, set_test_physical_marker, test_overlay_visible, WindowsBackend, WindowsCursor,
};
use orkestr_computer::cursor::model::{
    ComputerUseModel, DecideRequest, Decision, JsonVisionModel, ModelError, ModelErrorKind, VisionCompleter,
};
use orkestr_computer::cursor::select::CursorModelFactory;
use orkestr_computer::cursor::{CursorDeps, CursorService};
use orkestr_computer::{ComputerBackend, ComputerDeps, ComputerService};
use orkestr_events::EventBus;
use orkestr_protocol::methods::computer::MouseButton;
use orkestr_protocol::methods::cursor::{
    CursorAction, CursorBackendInfo, CursorSettings, CursorSource, CursorStartResult, CursorState, CursorStatus,
    CursorTarget,
};
use orkestr_security::{Security, SecurityConfig};
use orkestr_storage::Db;

const TITLE: &str = "Orkestr Cursor Smoke";
const MARKER: usize = 0x5E57_0001;
const TEXT: &str = "Привет от Orkestr cursor mode 42";

fn enabled() -> bool {
    std::env::var("ORKESTR_DESKTOP_SMOKE").as_deref() == Ok("1")
}

/// Refuses to act unless the smoke window is in front (protects user windows).
struct OnlySmoke(Arc<dyn ComputerUseModel>);

#[async_trait]
impl ComputerUseModel for OnlySmoke {
    fn id(&self) -> &str {
        self.0.id()
    }
    fn label(&self) -> String {
        self.0.label()
    }
    async fn decide(&self, req: &DecideRequest, cancel: &CancellationToken) -> Result<Decision, ModelError> {
        if req.observation.window_title != TITLE {
            // never show the user's screen to the model, never act on it:
            // the smoke window is gone (closed by the last click) or not in front
            let closed = req.history.iter().any(|h| h.action.contains(r#""type":"type""#) && h.outcome == "ok");
            return Ok(Decision::act(CursorAction::Done {
                summary: format!("smoke window not in front (“{}”)", req.observation.window_title),
                success: closed,
            }));
        }
        self.0.decide(req, cancel).await
    }
}

/// Finds elements by name in the observation (exercises the UIA tree + ids).
#[derive(Default)]
struct SmokeModel {
    step: Mutex<u32>,
    waits: bool,
    shots: Mutex<Vec<(u32, u32, String)>>,
}

#[async_trait]
impl ComputerUseModel for SmokeModel {
    fn id(&self) -> &str {
        "mock"
    }
    fn label(&self) -> String {
        "smoke".into()
    }
    async fn decide(&self, req: &DecideRequest, _: &CancellationToken) -> Result<Decision, ModelError> {
        let o = &req.observation;
        self.shots.lock().push((o.width, o.height, o.image_base64.clone()));
        if self.waits {
            return Ok(Decision::act(CursorAction::Wait { ms: 3000 }));
        }
        let find = |name: &str| o.elements.iter().find(|e| e.name == name).map(|e| e.id);
        let mut step = self.step.lock();
        *step += 1;
        let click = |id: u32| CursorAction::Click {
            target: CursorTarget::element(id),
            button: MouseButton::Left,
            double: false,
        };
        let a = match *step {
            1 => click(
                find("Smoke input")
                    .ok_or_else(|| ModelError::new(ModelErrorKind::Other, format!("no input in {:?}", o.elements)))?,
            ),
            2 => CursorAction::Type { text: TEXT.into() },
            3 => click(find("Close smoke").ok_or_else(|| ModelError::new(ModelErrorKind::Other, "no close button"))?),
            _ => CursorAction::Done { summary: "typed and closed".into(), success: true },
        };
        Ok(Decision::act(a))
    }
}

struct OneFactory(Mutex<Option<Arc<dyn ComputerUseModel>>>);

#[async_trait]
impl CursorModelFactory for OneFactory {
    async fn backends(&self, _: &CursorSettings) -> Vec<CursorBackendInfo> {
        vec![CursorBackendInfo {
            id: "mock".into(),
            label: "smoke".into(),
            source: CursorSource::Subscriptions,
            available: true,
            detail: String::new(),
            model: String::new(),
            consented: true,
        }]
    }
    fn build(&self, _: &str, _: &CursorSettings) -> Option<Arc<dyn ComputerUseModel>> {
        self.0.lock().clone()
    }
}

struct Fx {
    svc: Arc<CursorService>,
    factory: Arc<OneFactory>,
    backend: Arc<WindowsBackend>,
    _dir: tempfile::TempDir,
}

async fn fx() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(dir.path().join("t.db")).unwrap();
    let mut migs = orkestr_storage::storage_migrations();
    migs.extend(orkestr_events::events_migrations());
    migs.extend(orkestr_security::migrations());
    db.migrate(migs).await.unwrap();
    let events = EventBus::in_memory(4096);
    let security = Arc::new(
        Security::open(SecurityConfig {
            db,
            events: events.clone(),
            data_dir: dir.path().join("data"),
            credentials: None,
            secret_backend: orkestr_security::SecretBackendChoice::Memory,
            guard: orkestr_security::GuardConfig::default(),
        })
        .await
        .unwrap(),
    );
    let backend = Arc::new(WindowsBackend::new());
    let computer = ComputerService::new(ComputerDeps {
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: dir.path().join("computer"),
        backend: backend.clone(),
    });
    let factory = Arc::new(OneFactory(Mutex::new(None)));
    let svc = CursorService::new(CursorDeps {
        events,
        security,
        data_dir: dir.path().join("computer"),
        computer,
        screen: Arc::new(WindowsCursor::new()),
        factory: factory.clone(),
    });
    svc.set_settings(CursorSettings {
        max_steps: 12,
        max_seconds: 120,
        minimize_orkestr: false,
        abort_on_mouse: false,
        ..CursorSettings::default()
    })
    .await
    .unwrap();
    Fx { svc, factory, backend, _dir: dir }
}

const FORM: &str = r#"
param([string]$out)
Add-Type -AssemblyName System.Windows.Forms
$f = New-Object Windows.Forms.Form
$f.Text = 'Orkestr Cursor Smoke'
$f.Width = 640; $f.Height = 360; $f.StartPosition = 'CenterScreen'; $f.TopMost = $true
$tb = New-Object Windows.Forms.TextBox
$tb.Multiline = $true; $tb.AccessibleName = 'Smoke input'; $tb.SetBounds(20, 20, 580, 180)
$b = New-Object Windows.Forms.Button
$b.Text = 'Close smoke'; $b.AccessibleName = 'Close smoke'; $b.SetBounds(20, 220, 200, 44)
$b.Add_Click({ [IO.File]::WriteAllText($out, $tb.Text, [Text.Encoding]::UTF8); $f.Close() })
$t = New-Object Windows.Forms.Timer
$t.Interval = 180000; $t.Add_Tick({ $f.Close() }); $t.Start()
$f.Controls.Add($tb); $f.Controls.Add($b)
$f.Add_Shown({ $f.Activate() })
[void]$f.ShowDialog()
"#;

fn open_form(dir: &Path) -> (std::process::Child, PathBuf) {
    let script = dir.join("smoke.ps1");
    let out = dir.join("smoke-out.txt");
    std::fs::write(&script, FORM).unwrap();
    // no console window (CREATE_NO_WINDOW); the WinForms test form itself still shows
    let child = std::process::Command::new("powershell")
        .creation_flags(CREATE_NO_WINDOW)
        .args(["-NoProfile", "-ExecutionPolicy", "Bypass", "-WindowStyle", "Hidden", "-File"])
        .arg(&script)
        .arg(&out)
        .spawn()
        .unwrap();
    (child, out)
}

/// Input only ever goes to the smoke window.
fn assert_front(b: &WindowsBackend) {
    let fg = b.active_window().ok().flatten().map(|w| w.title).unwrap_or_default();
    assert_eq!(fg, TITLE, "refusing to send input: the foreground window is not the smoke window");
}

fn smoke_window(b: &WindowsBackend) -> Option<String> {
    b.windows().ok()?.into_iter().find(|w| w.title == TITLE).map(|w| w.handle)
}

async fn front(b: &Arc<WindowsBackend>) -> bool {
    for _ in 0..80 {
        if let Some(h) = smoke_window(b) {
            let _ = b.window_op(Some(&h), orkestr_protocol::methods::computer::WindowOp::Focus);
            tokio::time::sleep(Duration::from_millis(250)).await;
            if b.active_window().ok().flatten().is_some_and(|w| w.title == TITLE) {
                return true;
            }
        }
        tokio::time::sleep(Duration::from_millis(250)).await;
    }
    false
}

async fn wait_state(svc: &CursorService, pred: impl Fn(&CursorStatus) -> bool, secs: u64) -> CursorStatus {
    let t = Instant::now();
    loop {
        let s = svc.status().await;
        if pred(&s) || t.elapsed() > Duration::from_secs(secs) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

fn st(s: &CursorStatus) -> Option<CursorState> {
    s.session.as_ref().map(|x| x.state)
}

fn started(r: CursorStartResult) {
    assert!(matches!(r, CursorStartResult::Started { .. }), "{r:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "drives the real desktop; run with ORKESTR_DESKTOP_SMOKE=1 -- --ignored"]
async fn sandboxed_desktop_smoke() {
    if !enabled() {
        eprintln!("ORKESTR_DESKTOP_SMOKE != 1 — skipped");
        return;
    }
    let f = fx().await;
    let clip_before = f.backend.clipboard_get().ok();
    let work = tempfile::tempdir().unwrap();
    let (mut child, out) = open_form(work.path());
    assert!(front(&f.backend).await, "the smoke window did not come to the front");

    // 1. Esc: injected Esc is ignored, a "physical" Esc aborts at once.
    set_test_physical_marker(MARKER);
    let waits = Arc::new(SmokeModel { waits: true, ..SmokeModel::default() });
    *f.factory.0.lock() = Some(Arc::new(OnlySmoke(waits.clone())));
    started(f.svc.start("wait (esc test)", "en", "ui").await.unwrap());
    let s = wait_state(&f.svc, |s| st(s) == Some(CursorState::Acting), 20).await;
    assert_eq!(st(&s), Some(CursorState::Acting), "{s:?}");
    assert!(test_overlay_visible(), "the border overlay + badge must be on screen");
    assert_front(&f.backend);
    f.backend.press(&["esc".to_owned()]).unwrap(); // injected, not ours to obey
    tokio::time::sleep(Duration::from_millis(400)).await;
    assert!(!st(&f.svc.status().await).unwrap().is_final(), "injected Esc must not abort");
    let t = Instant::now();
    assert_front(&f.backend);
    send_test_input(true, MARKER).unwrap();
    let s = wait_state(&f.svc, |s| st(s).is_some_and(CursorState::is_final), 5).await;
    let sess = s.session.unwrap();
    assert_eq!((sess.state, sess.abort_reason.as_deref()), (CursorState::Aborted, Some("esc")));
    eprintln!("esc abort in {:?}", t.elapsed());
    assert!(!test_overlay_visible(), "the overlay is hidden after the session");

    // 2. physical mouse movement aborts (the user's own mouse may beat us to it: retry)
    let mut set = f.svc.settings();
    set.abort_on_mouse = true;
    f.svc.set_settings(set.clone()).await.unwrap();
    let mut ok = false;
    for attempt in 0..3 {
        assert!(front(&f.backend).await);
        started(f.svc.start("wait (mouse test)", "en", "ui").await.unwrap());
        let s =
            wait_state(&f.svc, |s| st(s) == Some(CursorState::Acting) || st(s).is_some_and(CursorState::is_final), 20)
                .await;
        if st(&s) != Some(CursorState::Acting) {
            eprintln!(
                "attempt {attempt}: aborted before our movement ({:?}) — the user moved the mouse",
                s.session.and_then(|x| x.abort_reason)
            );
            continue;
        }
        let t = Instant::now();
        assert_front(&f.backend);
        send_test_input(false, MARKER).unwrap();
        let s = wait_state(&f.svc, |s| st(s).is_some_and(CursorState::is_final), 5).await;
        assert_eq!(s.session.unwrap().abort_reason.as_deref(), Some("mouse"));
        eprintln!("mouse abort in {:?}", t.elapsed());
        ok = true;
        break;
    }
    assert!(ok, "mouse abort could not be tested (the mouse kept moving)");
    set.abort_on_mouse = false;
    f.svc.set_settings(set).await.unwrap();
    set_test_physical_marker(0);

    // 3. element click → type → close button, verified by the form's output file
    assert!(front(&f.backend).await);
    let model = Arc::new(SmokeModel::default());
    *f.factory.0.lock() = Some(Arc::new(OnlySmoke(model.clone())));
    started(f.svc.start("type a line and close", "ru", "ui").await.unwrap());
    let s = wait_state(&f.svc, |s| st(s).is_some_and(CursorState::is_final), 90).await;
    let sess = s.session.unwrap();
    for step in &sess.steps {
        eprintln!("step {} {:?} → {} {}", step.index, step.action, step.outcome, step.detail);
    }
    assert_eq!(sess.state, CursorState::Done, "{:?} {:?}", sess.summary, sess.abort_reason);
    let mut typed = String::new();
    for _ in 0..40 {
        if let Ok(t) = std::fs::read_to_string(&out) {
            typed = t.trim_start_matches('\u{feff}').to_owned();
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    assert_eq!(typed, TEXT);
    // the overlay border (accent #3AB0FF) is not in what the model saw
    let shots = model.shots.lock().clone();
    let (w, h, b64) = shots.first().cloned().unwrap();
    assert!(!overlay_visible(w, h, &b64), "the overlay leaked into the screenshot");

    // 4. optional: one short real run of the Claude Code CLI on a fresh smoke window
    if let Ok(claude) = std::env::var("ORKESTR_SMOKE_CLAUDE") {
        let _ = std::fs::remove_file(&out);
        let (mut child2, out2) = open_form(work.path());
        assert!(front(&f.backend).await);
        let real: Arc<dyn ComputerUseModel> = Arc::new(JsonVisionModel::new(Arc::new(ClaudeCli {
            program: PathBuf::from(claude),
            cwd: work.path().to_owned(),
        })));
        *f.factory.0.lock() = Some(Arc::new(OnlySmoke(real)));
        let task = "In the window \"Orkestr Cursor Smoke\": click the text box named \"Smoke input\", type exactly: Hello from Orkestr, then click the button \"Close smoke\". Then finish.";
        let t = Instant::now();
        started(f.svc.start(task, "en", "ui").await.unwrap());
        let s = wait_state(&f.svc, |s| st(s).is_some_and(CursorState::is_final), 150).await;
        let sess = s.session.unwrap();
        for step in &sess.steps {
            eprintln!("real step {} {:?} → {} ({} ms)", step.index, step.action, step.outcome, step.think_ms);
        }
        eprintln!("real run: {:?} in {:?}, summary {:?}", sess.state, t.elapsed(), sess.summary);
        let typed2 = std::fs::read_to_string(&out2).unwrap_or_default();
        eprintln!("real run typed: {typed2:?}");
        if sess.state != CursorState::Done {
            let _ = f.svc.stop("stop").await;
        }
        let _ = child2.kill();
        assert!(typed2.contains("Hello from Orkestr"), "the real model did not complete the task");
    }

    let _ = child.kill();
    assert_eq!(f.backend.clipboard_get().ok(), clip_before, "clipboard must stay untouched");
}

fn overlay_visible(w: u32, h: u32, b64: &str) -> bool {
    use base64::Engine as _;
    let png = base64::engine::general_purpose::STANDARD.decode(b64).unwrap();
    let dec = png::Decoder::new(std::io::Cursor::new(png));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size()];
    let info = r.next_frame(&mut buf).unwrap();
    assert_eq!((info.width, info.height), (w, h));
    // left border column, middle third: accent would be ≈ (58,176,255)
    let accent = (h / 3..2 * h / 3)
        .filter(|y| {
            let p = (*y * w * 3) as usize;
            let (r, g, b) = (buf[p], buf[p + 1], buf[p + 2]);
            r.abs_diff(58) < 40 && g.abs_diff(176) < 40 && b > 215
        })
        .count();
    accent > (h / 3) as usize * 9 / 10
}

/// Minimal Claude Code CLI transport for the optional real run.
struct ClaudeCli {
    program: PathBuf,
    cwd: PathBuf,
}

#[async_trait]
impl VisionCompleter for ClaudeCli {
    fn id(&self) -> &str {
        "claudeCode"
    }
    fn label(&self) -> String {
        "Claude Code · sonnet".into()
    }
    async fn complete(&self, system: &str, user: &str, png: &str, _: &CancellationToken) -> Result<String, ModelError> {
        use std::io::Write as _;
        let line = serde_json::json!({"type":"user","message":{"role":"user","content":[
            {"type":"image","source":{"type":"base64","media_type":"image/png","data":png}},
            {"type":"text","text":user}]}})
        .to_string();
        let (program, cwd, system) = (self.program.clone(), self.cwd.clone(), system.to_owned());
        tokio::task::spawn_blocking(move || {
            // no console window (CREATE_NO_WINDOW)
            let mut c = std::process::Command::new(program)
                .creation_flags(CREATE_NO_WINDOW)
                .args([
                    "-p",
                    "--input-format",
                    "stream-json",
                    "--output-format",
                    "stream-json",
                    "--verbose",
                    "--tools",
                    "",
                    "--no-session-persistence",
                    "--setting-sources",
                    "",
                    "--strict-mcp-config",
                    "--model",
                    "sonnet",
                    "--system-prompt",
                    &system,
                ])
                .current_dir(cwd)
                .stdin(std::process::Stdio::piped())
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
                .map_err(|e| ModelError::new(ModelErrorKind::Unavailable, e.to_string()))?;
            let mut i = c.stdin.take().unwrap();
            i.write_all(line.as_bytes()).ok();
            i.write_all(b"\n").ok();
            drop(i);
            let o = c.wait_with_output().map_err(|e| ModelError::new(ModelErrorKind::Network, e.to_string()))?;
            let out = String::from_utf8_lossy(&o.stdout);
            for l in out.lines().rev() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(l) {
                    if v["type"] == "result" {
                        if v["is_error"].as_bool().unwrap_or(false) {
                            return Err(ModelError::classified(v["result"].as_str().unwrap_or("error").to_owned()));
                        }
                        return Ok(v["result"].as_str().unwrap_or_default().to_owned());
                    }
                }
            }
            Err(ModelError::new(ModelErrorKind::Unavailable, String::from_utf8_lossy(&o.stderr).into_owned()))
        })
        .await
        .map_err(|e| ModelError::new(ModelErrorKind::Other, e.to_string()))?
    }
}
