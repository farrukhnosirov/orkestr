//! Cursor capture mode (ADR-0050) with the mock screen, the recording
//! computer backend and scripted models: observe → decide → act loop,
//! Esc / mouse aborts, limits, confirmations through the approvals service,
//! refusals (own windows, secrets), questions, pause, consent and failover.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;

use orkestr_computer::backend::{MockBackend, MockCall};
use orkestr_computer::cursor::model::{ComputerUseModel, Decision, ModelError, ScriptedModel};
use orkestr_computer::cursor::screen::{AbortCause, MockScreen, RawElement, ScreenCall, ScreenRect};
use orkestr_computer::cursor::select::CursorModelFactory;
use orkestr_computer::cursor::{CursorDeps, CursorService, Timing};
use orkestr_computer::{ComputerDeps, ComputerService};
use orkestr_events::EventBus;
use orkestr_protocol::event::EventKind;
use orkestr_protocol::methods::computer::MouseButton;
use orkestr_protocol::methods::cursor::{
    CursorAction, CursorBackendInfo, CursorEventPayload, CursorSettings, CursorSource, CursorStartResult, CursorState,
    CursorStatus, CursorTarget,
};
use orkestr_security::{AuditQuery, Security, SecurityConfig};
use orkestr_storage::Db;

struct Factory {
    infos: Vec<CursorBackendInfo>,
    models: HashMap<String, Arc<ScriptedModel>>,
}

#[async_trait]
impl CursorModelFactory for Factory {
    async fn backends(&self, _: &CursorSettings) -> Vec<CursorBackendInfo> {
        self.infos.clone()
    }
    fn build(&self, id: &str, _: &CursorSettings) -> Option<Arc<dyn ComputerUseModel>> {
        self.models.get(id).map(|m| Arc::clone(m) as Arc<dyn ComputerUseModel>)
    }
}

fn info(id: &str) -> CursorBackendInfo {
    CursorBackendInfo {
        id: id.into(),
        label: id.into(),
        source: if id == "anthropic" { CursorSource::Api } else { CursorSource::Subscriptions },
        available: true,
        detail: String::new(),
        model: "m".into(),
        consented: false,
    }
}

struct Fx {
    svc: Arc<CursorService>,
    screen: Arc<MockScreen>,
    backend: Arc<MockBackend>,
    security: Arc<Security>,
    events: EventBus,
    _dir: tempfile::TempDir,
}

async fn fx_with(
    models: Vec<(&str, ScriptedModel)>,
    edit: impl FnOnce(&MockScreen),
) -> (Fx, HashMap<String, Arc<ScriptedModel>>) {
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
    let backend = Arc::new(MockBackend::default());
    let computer = ComputerService::new(ComputerDeps {
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: dir.path().join("computer"),
        backend: backend.clone(),
    });
    let screen = Arc::new(MockScreen::with_window("Без имени — Блокнот", "notepad.exe", 500));
    *screen.elements.lock() = vec![
        RawElement {
            role: "edit".into(),
            name: "Text editor".into(),
            rect: ScreenRect { x: 100, y: 200, w: 800, h: 400 },
            enabled: true,
            password: false,
            value: None,
        },
        RawElement {
            role: "button".into(),
            name: "Купить".into(),
            rect: ScreenRect { x: 1000, y: 100, w: 100, h: 40 },
            enabled: true,
            password: false,
            value: None,
        },
    ];
    edit(&screen);
    let models: HashMap<String, Arc<ScriptedModel>> =
        models.into_iter().map(|(id, m)| (id.to_owned(), Arc::new(m))).collect();
    let mut infos: Vec<CursorBackendInfo> = models.keys().map(|k| info(k)).collect();
    infos.sort_by(|a, b| a.id.cmp(&b.id));
    let svc = CursorService::new(CursorDeps {
        events: events.clone(),
        security: Arc::clone(&security),
        data_dir: dir.path().join("computer"),
        computer,
        screen: screen.clone(),
        factory: Arc::new(Factory { infos, models: models.clone() }),
    });
    svc.set_timing(Timing { take_control: Duration::from_millis(1), settle: Duration::from_millis(1) });
    for id in models.keys() {
        svc.consent(id, true).await.unwrap();
    }
    (Fx { svc, screen, backend, security, events, _dir: dir }, models)
}

async fn wait_for(svc: &CursorService, pred: impl Fn(&CursorStatus) -> bool) -> CursorStatus {
    for _ in 0..400 {
        let s = svc.status().await;
        if pred(&s) {
            return s;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("timed out; last: {:?}", svc.status().await.session.map(|s| (s.state, s.steps.len())));
}

fn state(s: &CursorStatus) -> CursorState {
    s.session.as_ref().map_or(CursorState::Failed, |x| x.state)
}

async fn finished(svc: &CursorService) -> CursorStatus {
    wait_for(svc, |s| state(s).is_final()).await
}

fn started(r: CursorStartResult) {
    assert!(matches!(r, CursorStartResult::Started { .. }), "{r:?}");
}

async fn drain(sub: &mut orkestr_events::Subscription) -> Vec<CursorEventPayload> {
    let mut out = Vec::new();
    while let Ok(Some(r)) = tokio::time::timeout(Duration::from_millis(100), sub.recv()).await {
        if let orkestr_events::Received::Event(ev) = r {
            if let EventKind::ComputerCursor(p) = &ev.kind {
                out.push(p.clone());
            }
        }
    }
    out
}

fn click(id: u32) -> CursorAction {
    CursorAction::Click { target: CursorTarget::element(id), button: MouseButton::Left, double: false }
}

#[tokio::test]
async fn notepad_task_runs_end_to_end_and_hands_control_back() {
    let script = ScriptedModel::new(
        "mock",
        vec![
            Decision {
                thought: "focus the editor".into(),
                plan: vec!["click".into(), "type".into()],
                say: None,
                risk: None,
                action: click(1),
            },
            Decision::act(CursorAction::Type { text: "Привет от Orkestr".into() }),
            Decision::act(CursorAction::Keys { keys: vec!["ctrl".into(), "a".into()] }),
            Decision::act(CursorAction::Done { summary: "Строка напечатана.".into(), success: true }),
        ],
    );
    let (f, models) = fx_with(vec![("mock", script)], |_| {}).await;
    let mut rx = f.events.subscribe_all();
    started(f.svc.start("напечатай строку в блокноте", "ru", "ui").await.unwrap());
    let s = finished(&f.svc).await;
    let sess = s.session.unwrap();
    assert_eq!(sess.state, CursorState::Done, "{:?}", sess.steps);
    assert_eq!(sess.steps.len(), 4);
    assert_eq!(sess.summary.as_deref(), Some("Строка напечатана."));
    assert_eq!(sess.plan, ["click", "type"]);
    // element 1 = the editor, clicked in its centre (screen pixels)
    assert_eq!(
        f.screen.input_calls(),
        vec![ScreenCall::Move(500, 400), ScreenCall::Click(500, 400, MouseButton::Left, false)]
    );
    let typed: String = f
        .backend
        .take_calls()
        .into_iter()
        .filter_map(|c| match c {
            MockCall::Type(t) => Some(t),
            _ => None,
        })
        .collect();
    assert_eq!(typed, "Привет от Orkestr");
    let calls = f.screen.take_calls();
    assert_eq!(calls.first(), Some(&ScreenCall::OwnWindows(true)));
    assert!(calls.contains(&ScreenCall::Arm(true)));
    let tail: Vec<&ScreenCall> = calls.iter().rev().take(3).collect();
    assert_eq!(tail, [&ScreenCall::OwnWindows(false), &ScreenCall::Overlay(None), &ScreenCall::Disarm]);
    // the model saw the elements in screenshot pixels (1920×1080 → 1280×720)
    let seen = models["mock"].seen();
    assert_eq!(seen[0].observation.width, 1280);
    assert_eq!(seen[0].observation.elements[0].rect, [67, 133, 533, 267]);
    assert_eq!(seen[1].history.len(), 1);
    // a screenshot is fetchable while the session is the last one
    let shot = sess.last_shot.unwrap();
    assert!(!f.svc.screenshot(&shot.id).unwrap().data_base64.is_empty());
    // spoken phrases
    let says: Vec<String> = drain(&mut rx).await.into_iter().filter_map(|p| p.say).collect();
    assert_eq!(says.first().map(String::as_str), Some("Беру управление, сэр."));
    assert!(says.last().unwrap().ends_with("Готово, возвращаю управление."), "{says:?}");
    // audit: start, steps (typed text reduced to its length), finish
    let audit = f.security.audit.query(AuditQuery::default()).await.unwrap();
    let actions: Vec<&str> = audit.iter().map(|r| r.event.action.as_str()).collect();
    assert!(actions.contains(&"computer.cursorStart") && actions.contains(&"computer.cursorFinish"), "{actions:?}");
    assert_eq!(actions.iter().filter(|a| **a == "computer.cursorStep").count(), 4);
    let dump = serde_json::to_string(&audit.iter().map(|r| r.event.details.clone()).collect::<Vec<_>>()).unwrap();
    assert!(!dump.contains("Привет от Orkestr"), "typed text must not be in the audit");
}

fn waits(n: usize) -> ScriptedModel {
    ScriptedModel::actions("mock", (0..n).map(|_| CursorAction::Wait { ms: 5000 }).collect())
}

#[tokio::test]
async fn esc_aborts_instantly() {
    let (f, _) = fx_with(vec![("mock", waits(10))], |_| {}).await;
    started(f.svc.start("wait", "en", "voice").await.unwrap());
    wait_for(&f.svc, |s| s.session.as_ref().is_some_and(|x| x.state == CursorState::Acting)).await;
    let t = std::time::Instant::now();
    f.screen.trip(AbortCause::Esc);
    let s = finished(&f.svc).await.session.unwrap();
    assert!(t.elapsed() < Duration::from_secs(2));
    assert_eq!((s.state, s.abort_reason.as_deref()), (CursorState::Aborted, Some("esc")));
    assert_eq!(f.screen.take_calls().last(), Some(&ScreenCall::OwnWindows(false)));
}

#[tokio::test]
async fn physical_mouse_aborts_and_stop_works() {
    let (f, _) = fx_with(vec![("mock", waits(10))], |_| {}).await;
    started(f.svc.start("wait", "en", "ui").await.unwrap());
    wait_for(&f.svc, |s| state(s) == CursorState::Acting).await;
    f.screen.trip(AbortCause::Mouse);
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!(s.abort_reason.as_deref(), Some("mouse"));

    started(f.svc.start("wait again", "en", "ui").await.unwrap());
    wait_for(&f.svc, |s| state(s) == CursorState::Acting).await;
    let s = f.svc.stop("stop").await;
    assert_eq!(s.session.unwrap().abort_reason.as_deref(), Some("stop"));
}

#[tokio::test]
async fn step_limit_ends_the_session() {
    let (f, _) =
        fx_with(vec![("mock", ScriptedModel::actions("mock", vec![CursorAction::Wait { ms: 50 }; 5]))], |_| {}).await;
    f.svc.set_settings(CursorSettings { max_steps: 2, ..CursorSettings::default() }).await.unwrap();
    started(f.svc.start("loop", "ru", "ui").await.unwrap());
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!((s.state, s.abort_reason.as_deref(), s.steps.len()), (CursorState::Aborted, Some("stepLimit"), 2));
}

#[tokio::test]
async fn risky_click_waits_for_confirmation() {
    let script = ScriptedModel::actions(
        "mock",
        vec![click(2), click(2), CursorAction::Done { summary: String::new(), success: true }],
    );
    let (f, models) = fx_with(vec![("mock", script)], |_| {}).await;
    started(f.svc.start("buy it", "ru", "ui").await.unwrap());
    let s = wait_for(&f.svc, |s| state(s) == CursorState::AwaitingConfirmation).await;
    let c = s.session.unwrap().confirmation.unwrap();
    assert_eq!(c.code, "purchase");
    assert!(c.shot.is_some() && c.target_rect.is_some() && c.approval_id.is_some());
    assert!(f.screen.input_calls().is_empty(), "nothing clicked before the confirmation");
    assert_eq!(f.screen.calls.lock().iter().filter(|c| **c == ScreenCall::Disarm).count(), 1, "input handed back");
    f.svc.confirm(false).await.unwrap();
    // second attempt: approved
    wait_for(&f.svc, |s| state(s) == CursorState::AwaitingConfirmation && s.session.as_ref().unwrap().steps.len() == 1)
        .await;
    f.svc.confirm(true).await.unwrap();
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!(s.state, CursorState::Done);
    assert_eq!(s.steps[0].outcome, "rejected");
    assert_eq!(s.steps[1].outcome, "ok");
    assert_eq!(f.screen.input_calls().len(), 2, "one move + one click");
    assert!(models["mock"].seen()[1].notes.iter().any(|n| n.contains("rejected")));
}

#[tokio::test]
async fn own_windows_and_secrets_are_refused() {
    let own = std::process::id();
    let script = ScriptedModel::actions(
        "mock",
        vec![
            click(1),
            CursorAction::Type { text: "sk-ant-api03-AbCdEfGhIjKlMnOpQrStUvWxYz0123456789".into() },
            CursorAction::Done { summary: String::new(), success: false },
        ],
    );
    let (f, models) = fx_with(vec![("mock", script)], |s| *s.pid_at.lock() = Some(own)).await;
    started(f.svc.start("x", "en", "ui").await.unwrap());
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!(s.steps[0].outcome, "refused");
    assert_eq!(s.steps[1].outcome, "refused");
    assert!(f.screen.input_calls().is_empty());
    assert!(!f.backend.take_calls().iter().any(|c| matches!(c, MockCall::Type(_))));
    assert!(models["mock"].seen()[1].notes.iter().any(|n| n.contains("Orkestr")));
    assert_eq!(s.state, CursorState::Failed, "done(success=false)");
}

#[tokio::test]
async fn questions_reach_the_model() {
    let script = ScriptedModel::actions(
        "mock",
        vec![
            CursorAction::AskUser { question: "Какой город?".into() },
            CursorAction::Done { summary: String::new(), success: true },
        ],
    );
    let (f, models) = fx_with(vec![("mock", script)], |_| {}).await;
    started(f.svc.start("tickets", "ru", "voice").await.unwrap());
    let s = wait_for(&f.svc, |s| state(s) == CursorState::AwaitingUser).await;
    assert_eq!(s.session.unwrap().question.as_deref(), Some("Какой город?"));
    f.svc.answer("Ташкент").await.unwrap();
    finished(&f.svc).await;
    assert_eq!(models["mock"].seen()[1].answers, vec![("Какой город?".to_owned(), "Ташкент".to_owned())]);
}

#[tokio::test]
async fn pause_hands_input_back_and_resume_continues() {
    let script = ScriptedModel::actions(
        "mock",
        vec![CursorAction::Wait { ms: 300 }, CursorAction::Done { summary: String::new(), success: true }],
    );
    let (f, _) = fx_with(vec![("mock", script)], |_| {}).await;
    started(f.svc.start("p", "en", "ui").await.unwrap());
    wait_for(&f.svc, |s| state(s) == CursorState::Acting).await;
    f.svc.pause().await;
    wait_for(&f.svc, |s| state(s) == CursorState::Paused).await;
    f.svc.resume().await;
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!(s.state, CursorState::Done);
    let arms = f.screen.calls.lock().iter().filter(|c| matches!(c, ScreenCall::Arm(_))).count();
    assert_eq!(arms, 2, "guard re-armed after resume");
}

#[tokio::test]
async fn consent_is_required_once_per_provider() {
    let (f, _) = fx_with(vec![("claudeCode", waits(0))], |_| {}).await;
    f.svc.consent("claudeCode", false).await.unwrap();
    let r = f.svc.start("x", "ru", "ui").await.unwrap();
    assert!(matches!(&r, CursorStartResult::NeedsConsent { backend } if backend.id == "claudeCode"), "{r:?}");
    f.svc.consent("claudeCode", true).await.unwrap();
    started(f.svc.start("x", "ru", "ui").await.unwrap());
    finished(&f.svc).await;
    // persisted
    assert!(f.svc.settings().consents.contains_key("claudeCode"));
}

#[tokio::test]
async fn failover_moves_to_the_next_subscription() {
    let a =
        ScriptedModel::with_results("claudeCode", vec![Err(ModelError::classified("Claude AI usage limit reached"))]);
    let b = ScriptedModel::actions("codex", vec![CursorAction::Done { summary: "ok".into(), success: true }]);
    let (f, _) = fx_with(vec![("claudeCode", a), ("codex", b)], |_| {}).await;
    let mut rx = f.events.subscribe_all();
    started(f.svc.start("x", "ru", "ui").await.unwrap());
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!((s.state, s.backend.as_str()), (CursorState::Done, "codex"));
    let p = drain(&mut rx).await.into_iter().find(|p| p.event == "failover").expect("failover event");
    assert_eq!(p.say.as_deref(), Some("Переключаюсь на codex."));
    assert!(p.message.unwrap().starts_with("claudeCode → codex"));
}

#[tokio::test]
async fn secure_desktop_aborts() {
    let (f, _) = fx_with(vec![("mock", waits(3))], |s| *s.capture_fails.lock() = true).await;
    started(f.svc.start("x", "ru", "ui").await.unwrap());
    let s = finished(&f.svc).await.session.unwrap();
    assert_eq!(s.abort_reason.as_deref(), Some("secureDesktop"));
}
