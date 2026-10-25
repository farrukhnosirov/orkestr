//! Cursor capture mode (ADR-0050): the AI drives the mouse and keyboard to
//! perform a task, then hands control back.
//!
//! One session at a time runs the loop **observe → decide → act → verify**:
//!
//! 1. *Observe* — screenshot of the monitor that shows the foreground
//!    window (downscaled, Orkestr's windows and the overlay are excluded
//!    from capture), its UI Automation elements (capped), the cursor and the
//!    active window.
//! 2. *Decide* — a [`ComputerUseModel`] (vision model behind a
//!    [`select::FailoverModel`]) returns one [`CursorAction`].
//! 3. *Check* — [`guardrails`] refuse (own windows, secrets) or ask for a
//!    confirmation (approvals inbox + panel with the screenshot).
//! 4. *Act* — absolute, DPI-correct input marked as injected; the next
//!    observation verifies the effect.
//!
//! Esc or a physical mouse movement trips the low-level hook guard and
//! cancels everything at once; step / time limits, Stop, the global
//! computer stop and the emergency stop end the session too. Every step is
//! audited; screenshots live in memory only and go only to the chosen model.

pub mod guardrails;
pub mod model;
pub mod phrases;
pub mod screen;
pub mod select;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use base64::Engine as _;
use parking_lot::{Mutex, RwLock};
use tokio::sync::{oneshot, watch};
use tokio_util::sync::CancellationToken;

use orkestr_domain::{ApprovalStatus, AuditOutcome, Capability, RiskLevel};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, NewEvent};
use orkestr_protocol::methods::computer::ComputerAction;
use orkestr_protocol::methods::cursor::{
    CursorAction, CursorBackendInfo, CursorConfirmation, CursorElement, CursorEventPayload, CursorSession,
    CursorSettings, CursorShot, CursorShotResult, CursorStartResult, CursorState, CursorStatus, CursorStep,
    CursorTarget,
};
use orkestr_security::{ApprovalDecision, ApprovalRequest, NewAudit, Security};

use crate::error::{ComputerError, ComputerResult};
use crate::service::ComputerService;
use guardrails::{StepContext, Verdict};
use model::{ComputerUseModel, DecideRequest, Decision, HistoryItem, ModelErrorKind, Observation};
use phrases::{Moment, PhraseHook};
use screen::{downscale_rgb, encode_png, to_screen, to_shot, CursorBackend, Foreground, RawElement, ScreenRect};
use select::{CursorModelFactory, FailoverModel};

pub use model::{JsonVisionModel, ModelError, ScriptedModel, VisionCompleter};
pub use screen::{default_cursor_backend, MockScreen, UnsupportedScreen};

const MAX_ELEMENTS: usize = 120;
const HISTORY_TO_MODEL: usize = 12;
const SHOTS_KEPT: usize = 30;
const MODEL_TIMEOUT: Duration = Duration::from_secs(120);
const CONFIRM_TTL: Duration = Duration::from_secs(5 * 60);
const MAX_ERRORS: u32 = 3;

/// Dependencies.
pub struct CursorDeps {
    /// Event bus.
    pub events: EventBus,
    /// Security (approvals, audit).
    pub security: Arc<Security>,
    /// `<data>/computer` (settings file).
    pub data_dir: PathBuf,
    /// The computer assistant (keys, typing, launching, focusing, own pids).
    pub computer: Arc<ComputerService>,
    /// Screen / pointer / guard / overlay.
    pub screen: Arc<dyn CursorBackend>,
    /// Model backends.
    pub factory: Arc<dyn CursorModelFactory>,
}

/// Timing knobs (tests shorten them).
#[derive(Debug, Clone, Copy)]
pub struct Timing {
    /// Pause after taking control (minimize animations).
    pub take_control: Duration,
    /// Pause after each action before the next observation.
    pub settle: Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self { take_control: Duration::from_millis(450), settle: Duration::from_millis(700) }
    }
}

struct Shot {
    png: Vec<u8>,
    width: u32,
    height: u32,
}

#[derive(Default)]
struct Ctrl {
    cancel: CancellationToken,
    reason: Mutex<Option<String>>,
    paused: Option<watch::Sender<bool>>,
    answer: Mutex<Option<oneshot::Sender<String>>>,
    confirm: Mutex<Option<oneshot::Sender<bool>>>,
}

impl Ctrl {
    fn abort(&self, reason: &str) {
        {
            let mut r = self.reason.lock();
            if r.is_none() {
                *r = Some(reason.to_owned());
            }
        }
        self.cancel.cancel();
    }
    fn is_paused(&self) -> bool {
        self.paused.as_ref().is_some_and(|p| *p.borrow())
    }
}

/// The cursor capture mode service.
pub struct CursorService {
    deps: CursorDeps,
    settings: RwLock<CursorSettings>,
    session: Arc<Mutex<Option<CursorSession>>>,
    ctrl: Mutex<Option<Arc<Ctrl>>>,
    shots: Mutex<(HashMap<String, Shot>, VecDeque<String>)>,
    hook: RwLock<Option<PhraseHook>>,
    timing: RwLock<Timing>,
}

impl std::fmt::Debug for CursorService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CursorService").field("screen", &self.deps.screen.name()).finish_non_exhaustive()
    }
}

fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::now_v7().simple())
}

fn clamp_settings(mut s: CursorSettings) -> CursorSettings {
    s.max_steps = s.max_steps.clamp(1, 100);
    s.max_seconds = s.max_seconds.clamp(30, 1800);
    s.max_image_side = s.max_image_side.clamp(640, 1600);
    for list in [&mut s.subscription_order, &mut s.api_order] {
        let mut seen = Vec::new();
        list.retain(|x| !seen.contains(x) && {
            seen.push(x.clone());
            true
        });
    }
    s
}

/// Everything one observation produced (screen coordinates kept for acting).
struct Observed {
    fg: Option<Foreground>,
    monitor: ScreenRect,
    width: u32,
    height: u32,
    shot: CursorShot,
    png_b64: String,
    elements: Vec<CursorElement>,
    raw: Vec<RawElement>,
    cursor: Option<(i32, i32)>,
    fg_is_own: bool,
}

impl CursorService {
    /// Loads the settings.
    #[must_use]
    pub fn new(deps: CursorDeps) -> Arc<Self> {
        let path = deps.data_dir.join("cursor.json");
        let settings = std::fs::read_to_string(&path)
            .ok()
            .and_then(|t| serde_json::from_str::<CursorSettings>(&t).ok())
            .map(clamp_settings)
            .unwrap_or_default();
        Arc::new(Self {
            deps,
            settings: RwLock::new(settings),
            session: Arc::new(Mutex::new(None)),
            ctrl: Mutex::new(None),
            shots: Mutex::new((HashMap::new(), VecDeque::new())),
            hook: RwLock::new(None),
            timing: RwLock::new(Timing::default()),
        })
    }

    /// Replaces the spoken phrases (Jarvis phrasing, package B).
    pub fn set_phrase_hook(&self, hook: PhraseHook) {
        *self.hook.write() = Some(hook);
    }

    /// Shorter waits (tests).
    pub fn set_timing(&self, t: Timing) {
        *self.timing.write() = t;
    }

    fn say(&self, m: &Moment<'_>, lang: &str) -> Option<String> {
        let hook = self.hook.read().clone();
        hook.and_then(|h| h(m, lang)).or_else(|| phrases::phrase(m, lang))
    }

    // ── settings / status ────────────────────────────────────────────────

    /// Settings.
    #[must_use]
    pub fn settings(&self) -> CursorSettings {
        self.settings.read().clone()
    }

    fn save_settings(&self) -> ComputerResult<()> {
        let s = self.settings.read().clone();
        std::fs::create_dir_all(&self.deps.data_dir)?;
        let tmp = self.deps.data_dir.join("cursor.json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(&s).unwrap_or_default())?;
        std::fs::rename(&tmp, self.deps.data_dir.join("cursor.json"))?;
        Ok(())
    }

    /// Saves settings (consents are kept as they are).
    ///
    /// # Errors
    /// IO.
    pub async fn set_settings(&self, new: CursorSettings) -> ComputerResult<CursorStatus> {
        {
            let mut s = self.settings.write();
            let consents = std::mem::take(&mut s.consents);
            *s = clamp_settings(CursorSettings { consents, ..new });
        }
        self.save_settings()?;
        Ok(self.status().await)
    }

    /// Grants / revokes the privacy consent of a backend.
    ///
    /// # Errors
    /// IO.
    pub async fn consent(&self, backend: &str, granted: bool) -> ComputerResult<CursorStatus> {
        if backend.is_empty() || backend.len() > 40 {
            return Err(ComputerError::Invalid("bad backend id".into()));
        }
        {
            let mut s = self.settings.write();
            if granted {
                s.consents.insert(backend.to_owned(), chrono::Utc::now());
            } else {
                s.consents.remove(backend);
            }
        }
        self.save_settings()?;
        let sec = &self.deps.security;
        sec.audit
            .record(
                NewAudit::new(sec.user(), "computer.cursorConsent", backend, AuditOutcome::Executed)
                    .capability(Capability::ComputerControl)
                    .details(serde_json::json!({ "granted": granted })),
            )
            .await;
        Ok(self.status().await)
    }

    async fn backends(&self) -> Vec<CursorBackendInfo> {
        let s = self.settings();
        select::ordered(self.deps.factory.backends(&s).await, &s)
    }

    /// Status.
    pub async fn status(&self) -> CursorStatus {
        let settings = self.settings();
        let backends = self.backends().await;
        let active_backend = select::candidates(&backends, &settings).first().map(|b| b.id.clone());
        let avail = self.deps.screen.available();
        CursorStatus {
            session: self.session.lock().clone(),
            backends,
            active_backend,
            settings,
            supported: avail.is_ok(),
            unsupported_reason: avail.err().map(|e| e.to_string()),
        }
    }

    /// A screenshot of the current / last session.
    ///
    /// # Errors
    /// `NotFound`.
    pub fn screenshot(&self, id: &str) -> ComputerResult<CursorShotResult> {
        let g = self.shots.lock();
        let s = g.0.get(id).ok_or_else(|| ComputerError::NotFound(format!("screenshot {id}")))?;
        Ok(CursorShotResult {
            media_type: "image/png".into(),
            data_base64: base64::engine::general_purpose::STANDARD.encode(&s.png),
            width: s.width,
            height: s.height,
        })
    }

    fn keep_shot(&self, png: Vec<u8>, width: u32, height: u32) -> CursorShot {
        let id = new_id("shot");
        let mut g = self.shots.lock();
        g.0.insert(id.clone(), Shot { png, width, height });
        g.1.push_back(id.clone());
        while g.1.len() > SHOTS_KEPT {
            if let Some(old) = g.1.pop_front() {
                g.0.remove(&old);
            }
        }
        CursorShot { id, width, height }
    }

    // ── control ──────────────────────────────────────────────────────────

    fn running(&self) -> bool {
        self.session.lock().as_ref().is_some_and(|s| !s.state.is_final())
    }

    /// Starts a session.
    ///
    /// # Errors
    /// `Conflict` (already running), `Unsupported`, `Invalid`.
    pub async fn start(self: &Arc<Self>, task: &str, lang: &str, source: &str) -> ComputerResult<CursorStartResult> {
        let task = task.trim();
        if task.is_empty() {
            return Err(ComputerError::Invalid("empty task".into()));
        }
        if task.chars().count() > 1000 {
            return Err(ComputerError::Invalid("task too long".into()));
        }
        if self.running() {
            return Err(ComputerError::Conflict("the AI is already controlling the cursor".into()));
        }
        self.deps.screen.available()?;
        let settings = self.settings();
        let backends = self.backends().await;
        let candidates = select::candidates(&backends, &settings);
        let Some(first) = candidates.first() else {
            let reason = backends
                .iter()
                .filter(|b| b.source == settings.source)
                .map(|b| format!("{}: {}", b.label, b.detail))
                .collect::<Vec<_>>()
                .join("; ");
            return Ok(CursorStartResult::NoBackend { reason });
        };
        if !first.consented {
            return Ok(CursorStartResult::NeedsConsent { backend: first.clone() });
        }
        let chain: Vec<Arc<dyn ComputerUseModel>> = candidates
            .iter()
            .filter(|b| b.consented)
            .filter_map(|b| self.deps.factory.build(&b.id, &settings))
            .collect();
        if chain.is_empty() {
            return Ok(CursorStartResult::NoBackend { reason: format!("{} could not be started", first.label) });
        }
        let lang = if lang.is_empty() { "ru" } else { lang };
        let ctrl = Arc::new(Ctrl { paused: Some(watch::channel(false).0), ..Ctrl::default() });
        let session_id = new_id("cursor");
        let labels: HashMap<String, String> = backends.iter().map(|b| (b.id.clone(), b.label.clone())).collect();
        let model = {
            let me = Arc::downgrade(self);
            let sid = session_id.clone();
            let lang = lang.to_owned();
            Arc::new(FailoverModel::new(
                chain,
                Arc::new(move |from: &str, to: &str, why: &str| {
                    let Some(me) = me.upgrade() else { return };
                    let label = labels.get(to).cloned().unwrap_or_else(|| to.to_owned());
                    if let Some(s) = me.session.lock().as_mut() {
                        s.backend = to.to_owned();
                    }
                    let say = me.say(&Moment::Failover(&label), &lang);
                    let msg = format!("{from} → {to}: {}", why.chars().take(160).collect::<String>());
                    let me2 = Arc::clone(&me);
                    let sid = sid.clone();
                    tokio::spawn(async move { me2.publish(&sid, "failover", say, Some(msg)).await });
                }),
            ))
        };
        let session = CursorSession {
            session_id: session_id.clone(),
            task: task.to_owned(),
            source: if source.is_empty() { "ui".into() } else { source.chars().take(20).collect() },
            state: CursorState::Starting,
            backend: model.current_id(),
            model: model.current_label(),
            plan: Vec::new(),
            current: String::new(),
            steps: Vec::new(),
            last_shot: None,
            confirmation: None,
            question: None,
            summary: None,
            abort_reason: None,
            max_steps: settings.max_steps,
            max_seconds: settings.max_seconds,
            started_at: chrono::Utc::now(),
            finished_at: None,
            lang: lang.to_owned(),
        };
        {
            let mut g = self.shots.lock();
            g.0.clear();
            g.1.clear();
        }
        *self.session.lock() = Some(session.clone());
        *self.ctrl.lock() = Some(Arc::clone(&ctrl));
        let me = Arc::clone(self);
        tokio::spawn(async move { me.run(ctrl, model, settings).await });
        Ok(CursorStartResult::Started { session })
    }

    fn current_ctrl(&self) -> Option<Arc<Ctrl>> {
        self.ctrl.lock().clone()
    }

    /// Stops the session (`reason`: `stop`, `emergency`…).
    pub async fn stop(&self, reason: &str) -> CursorStatus {
        if let Some(c) = self.current_ctrl() {
            if self.running() {
                c.abort(reason);
            }
        }
        // the loop finishes asynchronously; give it a moment so the status is final
        for _ in 0..40 {
            if !self.running() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        self.status().await
    }

    /// Pauses.
    pub async fn pause(&self) -> CursorStatus {
        if let Some(p) = self.current_ctrl().and_then(|c| c.paused.clone()) {
            if self.running() {
                p.send_replace(true);
            }
        }
        self.status().await
    }

    /// Resumes.
    pub async fn resume(&self) -> CursorStatus {
        if let Some(p) = self.current_ctrl().and_then(|c| c.paused.clone()) {
            p.send_replace(false);
        }
        self.status().await
    }

    /// Answers the model's question.
    ///
    /// # Errors
    /// `Conflict` when nothing was asked.
    pub async fn answer(&self, text: &str) -> ComputerResult<CursorStatus> {
        let tx = self.current_ctrl().and_then(|c| c.answer.lock().take());
        let tx = tx.ok_or_else(|| ComputerError::Conflict("no question is pending".into()))?;
        let _ = tx.send(text.chars().take(1000).collect());
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(self.status().await)
    }

    /// Approves / rejects the pending step.
    ///
    /// # Errors
    /// `Conflict` when nothing waits.
    pub async fn confirm(&self, approve: bool) -> ComputerResult<CursorStatus> {
        let aid = self.session.lock().as_ref().and_then(|s| s.confirmation.as_ref().and_then(|c| c.approval_id));
        let tx = self.current_ctrl().and_then(|c| c.confirm.lock().take());
        let tx = tx.ok_or_else(|| ComputerError::Conflict("no step waits for a confirmation".into()))?;
        if let Some(aid) = aid {
            let decision = if approve { ApprovalDecision::Approve } else { ApprovalDecision::Reject };
            let user = self.deps.security.user();
            let _ = self.deps.security.approvals.resolve(aid, decision, None, user).await;
        }
        let _ = tx.send(approve);
        tokio::time::sleep(Duration::from_millis(30)).await;
        Ok(self.status().await)
    }

    // ── events / audit ───────────────────────────────────────────────────

    fn patch(&self, f: impl FnOnce(&mut CursorSession)) -> Option<CursorSession> {
        let mut g = self.session.lock();
        g.as_mut().map(|s| {
            f(s);
            s.clone()
        })
    }

    async fn publish(&self, session_id: &str, event: &str, say: Option<String>, message: Option<String>) {
        let (state, steps) = self
            .session
            .lock()
            .as_ref()
            .map_or((CursorState::Failed, 0), |s| (s.state, u32::try_from(s.steps.len()).unwrap_or(0)));
        let ev = NewEvent::new(EventKind::ComputerCursor(CursorEventPayload {
            session_id: session_id.to_owned(),
            event: event.to_owned(),
            state,
            say,
            message,
            steps,
        }));
        if let Err(e) = self.deps.events.publish(ev).await {
            tracing::warn!(error = %e, "could not publish ComputerCursor");
        }
    }

    async fn set_state(&self, sid: &str, state: CursorState, current: Option<String>, say: Option<String>) {
        self.patch(|s| {
            s.state = state;
            if let Some(c) = current {
                s.current = c;
            }
        });
        self.publish(sid, "state", say, None).await;
    }

    async fn audit(&self, action: &str, sid: &str, outcome: AuditOutcome, details: serde_json::Value) {
        let s = &self.deps.security;
        s.audit
            .record(NewAudit::new(s.user(), action, sid, outcome).capability(Capability::ComputerControl).details(details))
            .await;
    }

    // ── the loop ─────────────────────────────────────────────────────────

    #[allow(clippy::too_many_lines)]
    async fn run(self: Arc<Self>, ctrl: Arc<Ctrl>, model: Arc<FailoverModel>, settings: CursorSettings) {
        let (sid, task, lang) = {
            let g = self.session.lock();
            let s = g.as_ref().expect("session set by start");
            (s.session_id.clone(), s.task.clone(), s.lang.clone())
        };
        let timing = *self.timing.read();
        let started = Instant::now();
        self.audit(
            "computer.cursorStart",
            &sid,
            AuditOutcome::Executed,
            serde_json::json!({ "task": task, "backend": model.current_id(), "maxSteps": settings.max_steps }),
        )
        .await;
        let say = self.say(&Moment::Start, &lang);
        self.set_state(&sid, CursorState::Starting, None, say).await;

        let mut outcome: Result<(CursorState, Option<String>), String> = match self.take_control(&ctrl, &settings, &lang).await {
            Ok(()) => Err(String::new()),
            Err(e) => Ok((CursorState::Failed, Some(e.to_string()))),
        };
        if outcome.is_err() {
            sleep_or(&ctrl.cancel, timing.take_control).await;
        }

        let mut history: Vec<HistoryItem> = Vec::new();
        let mut answers: Vec<(String, String)> = Vec::new();
        let mut notes: Vec<String> = Vec::new();
        let mut errors = 0u32;
        let mut blind = 0u32;
        let mut step = 0u32;
        while outcome.is_err() {
            if ctrl.cancel.is_cancelled() {
                outcome = Ok((CursorState::Aborted, None));
                break;
            }
            if step >= settings.max_steps {
                ctrl.abort("stepLimit");
                continue;
            }
            if started.elapsed() > Duration::from_secs(u64::from(settings.max_seconds)) {
                ctrl.abort("timeLimit");
                continue;
            }
            if ctrl.is_paused() {
                self.wait_paused(&ctrl, &sid, &settings, &lang).await;
                continue;
            }
            // observe
            self.set_state(&sid, CursorState::Observing, None, None).await;
            let obs = match self.observe(&settings).await {
                Ok(o) => {
                    blind = 0;
                    o
                }
                Err(e) => {
                    blind += 1;
                    tracing::warn!(error = %e, "cursor observation failed");
                    if blind >= 3 {
                        ctrl.abort("secureDesktop");
                    } else {
                        sleep_or(&ctrl.cancel, Duration::from_millis(500)).await;
                    }
                    continue;
                }
            };
            self.patch(|s| s.last_shot = Some(obs.shot.clone()));
            // decide
            step += 1;
            self.set_state(&sid, CursorState::Thinking, None, None).await;
            let req = DecideRequest {
                task: task.clone(),
                lang: lang.clone(),
                step,
                max_steps: settings.max_steps,
                history: history.iter().rev().take(HISTORY_TO_MODEL).rev().cloned().collect(),
                answers: answers.clone(),
                notes: std::mem::take(&mut notes),
                observation: Observation {
                    image_base64: obs.png_b64.clone(),
                    width: obs.width,
                    height: obs.height,
                    elements: obs.elements.clone(),
                    cursor: obs.cursor,
                    window_title: obs.fg.as_ref().map(|f| f.title.clone()).unwrap_or_default(),
                    window_process: obs.fg.as_ref().map(|f| f.process.clone()).unwrap_or_default(),
                },
            };
            let t0 = Instant::now();
            let decided = tokio::select! {
                r = tokio::time::timeout(MODEL_TIMEOUT, model.decide(&req, &ctrl.cancel)) => {
                    r.unwrap_or_else(|_| Err(ModelError::new(ModelErrorKind::Network, "the model did not answer in time")))
                }
                () = ctrl.cancel.cancelled() => Err(ModelError::cancelled()),
            };
            let think_ms = u64::try_from(t0.elapsed().as_millis()).unwrap_or(u64::MAX);
            self.patch(|s| {
                s.backend = model.current_id();
                s.model = model.current_label();
            });
            let decision = match decided {
                Ok(d) => d,
                Err(e) if e.kind == ModelErrorKind::Cancelled || ctrl.cancel.is_cancelled() => continue,
                Err(e) => {
                    errors += 1;
                    if e.kind.fails_over() || errors >= MAX_ERRORS {
                        outcome = Ok((CursorState::Failed, Some(e.message)));
                    } else {
                        notes.push(format!("Your last answer could not be used: {}", e.message));
                    }
                    continue;
                }
            };
            if ctrl.is_paused() {
                // the screen may change while paused: decide again afterwards
                step -= 1;
                continue;
            }
            let (target_screen, target_el) = resolve_target(&decision.action, &obs);
            let target_rect = target_screen.map(|(x, y)| {
                target_el.map_or_else(|| to_shot(ScreenRect { x: x - 8, y: y - 8, w: 16, h: 16 }, obs.monitor, obs.width, obs.height), |e| e.rect)
            });
            let current = decision.say.clone().unwrap_or_else(|| decision.thought.clone());
            self.patch(|s| {
                if !decision.plan.is_empty() {
                    s.plan.clone_from(&decision.plan);
                }
                s.current.clone_from(&current);
            });
            if let Some(err) = target_error(&decision.action, target_screen, &obs) {
                errors += 1;
                notes.push(err.clone());
                self.record(&sid, step, &decision, "error", &err, target_rect, &obs, &model, think_ms, &mut history).await;
                if errors >= MAX_ERRORS {
                    outcome = Ok((CursorState::Failed, Some(err)));
                }
                continue;
            }
            match &decision.action {
                CursorAction::Done { summary, success } => {
                    self.record(&sid, step, &decision, "ok", summary, None, &obs, &model, think_ms, &mut history).await;
                    outcome = Ok((if *success { CursorState::Done } else { CursorState::Failed }, Some(summary.clone())));
                    continue;
                }
                CursorAction::AskUser { question } => {
                    self.record(&sid, step, &decision, "ok", question, None, &obs, &model, think_ms, &mut history).await;
                    match self.ask(&ctrl, &sid, question, &settings, &lang).await {
                        Some(a) => answers.push((question.clone(), a)),
                        None => continue,
                    }
                    continue;
                }
                _ => {}
            }
            // safety
            let screen = Arc::clone(&self.deps.screen);
            let own = self.deps.computer.own_pids();
            let needs_focus = matches!(decision.action, CursorAction::Type { .. });
            let (target_is_own, focused_password) = tokio::task::spawn_blocking(move || {
                let own_t = target_screen.is_some_and(|(x, y)| screen.pid_at(x, y).is_ok_and(|p| own.contains(&p)));
                let pw = needs_focus && screen.focused_is_password().unwrap_or(false);
                (own_t, pw)
            })
            .await
            .unwrap_or((true, false));
            let cx = StepContext {
                window_title: obs.fg.as_ref().map_or("", |f| f.title.as_str()),
                window_process: obs.fg.as_ref().map_or("", |f| f.process.as_str()),
                foreground_is_own: obs.fg_is_own,
                target_is_own,
                element: target_el,
                focused_password,
                model_risk: decision.risk.as_deref(),
            };
            match guardrails::check(&decision.action, &cx) {
                Verdict::Allow => {}
                Verdict::Refuse { code, detail } => {
                    let msg = format!("refused ({code}): {detail}");
                    notes.push(match code.as_str() {
                        "ownWindow" => "That action targets Orkestr's own window, which is never allowed. Work only in the user's apps.".to_owned(),
                        _ => "Typing secrets is never allowed. Ask the user to type it themselves (ask_user).".to_owned(),
                    });
                    self.record(&sid, step, &decision, "refused", &msg, target_rect, &obs, &model, think_ms, &mut history).await;
                    self.audit("computer.cursorStep", &sid, AuditOutcome::Denied, serde_json::json!({ "code": code, "action": audit_action(&decision.action) })).await;
                    continue;
                }
                Verdict::Confirm { code, detail } => {
                    let conf = CursorConfirmation {
                        code: code.clone(),
                        detail: detail.clone(),
                        action: decision.action.clone(),
                        shot: Some(obs.shot.clone()),
                        target_rect,
                        approval_id: None,
                    };
                    match self.confirm_step(&ctrl, &sid, conf, &settings, &lang).await {
                        Some(true) => {}
                        Some(false) => {
                            notes.push("The user rejected that action. Do not repeat it; find another way or finish.".into());
                            self.record(&sid, step, &decision, "rejected", &code, target_rect, &obs, &model, think_ms, &mut history).await;
                            continue;
                        }
                        None => continue,
                    }
                }
            }
            // act
            let say = decision.say.clone().or_else(|| self.say(&Moment::Act(&decision.action), &lang));
            self.set_state(&sid, CursorState::Acting, Some(current.clone()), say).await;
            let _ = self.deps.screen.overlay(Some(&phrases::badge(&lang, &current)));
            let result = self.act(&decision.action, target_screen, &obs, &ctrl).await;
            let (out, detail) = match &result {
                Ok(d) => ("ok", d.clone()),
                Err(ComputerError::Stopped) => continue,
                Err(e) => {
                    errors += 1;
                    ("error", e.to_string())
                }
            };
            if out == "ok" {
                errors = 0;
            }
            self.record(&sid, step, &decision, out, &detail, target_rect, &obs, &model, think_ms, &mut history).await;
            if errors >= MAX_ERRORS {
                outcome = Ok((CursorState::Failed, Some(detail)));
                continue;
            }
            sleep_or(&ctrl.cancel, timing.settle).await;
        }

        // hand control back
        self.release().await;
        let (state, summary) = outcome.unwrap_or((CursorState::Aborted, None));
        let reason = ctrl.reason.lock().clone();
        let state = if reason.is_some() && state != CursorState::Done { CursorState::Aborted } else { state };
        let say = match state {
            CursorState::Done => self.say(&Moment::Done(summary.as_deref().unwrap_or("")), &lang),
            CursorState::Aborted => self.say(&Moment::Aborted(reason.as_deref().unwrap_or("stop")), &lang),
            _ if summary.is_some() && history.last().is_some_and(|h| h.action.contains("\"done\"")) => {
                self.say(&Moment::Impossible(summary.as_deref().unwrap_or("")), &lang)
            }
            _ => self.say(&Moment::Failed, &lang),
        };
        // a pending approval is moot now
        if let Some(aid) = self.session.lock().as_ref().and_then(|s| s.confirmation.as_ref().and_then(|c| c.approval_id)) {
            let sec = Arc::clone(&self.deps.security);
            tokio::spawn(async move {
                let _ = sec.approvals.cancel(aid, "cursor session ended").await;
            });
        }
        self.patch(|s| {
            s.state = state;
            s.summary.clone_from(&summary);
            s.abort_reason.clone_from(&reason);
            s.finished_at = Some(chrono::Utc::now());
            s.confirmation = None;
            s.question = None;
            s.current = String::new();
        });
        let steps = self.session.lock().as_ref().map_or(0, |s| s.steps.len());
        self.audit(
            "computer.cursorFinish",
            &sid,
            if state == CursorState::Done { AuditOutcome::Executed } else { AuditOutcome::Failed },
            serde_json::json!({ "state": format!("{state:?}"), "reason": reason, "steps": steps, "summary": summary }),
        )
        .await;
        self.publish(&sid, "finished", say, summary).await;
    }

    async fn take_control(&self, ctrl: &Arc<Ctrl>, settings: &CursorSettings, lang: &str) -> ComputerResult<()> {
        let screen = Arc::clone(&self.deps.screen);
        let own = self.deps.computer.own_pids();
        let minimize = settings.minimize_orkestr;
        let mouse = settings.abort_on_mouse;
        let badge = phrases::badge(lang, "");
        let c = Arc::clone(ctrl);
        tokio::task::spawn_blocking(move || {
            screen.own_windows(&own, true, minimize)?;
            screen.overlay(Some(&badge))?;
            let c2 = Arc::clone(&c);
            screen.arm_guard(mouse, Arc::new(move |cause| c2.abort(cause.code())))
        })
        .await
        .map_err(|e| ComputerError::Backend(e.to_string()))?
    }

    async fn release(&self) {
        let screen = Arc::clone(&self.deps.screen);
        let own = self.deps.computer.own_pids();
        let _ = tokio::task::spawn_blocking(move || {
            screen.disarm_guard();
            let _ = screen.overlay(None);
            let _ = screen.own_windows(&own, false, false);
        })
        .await;
    }

    /// Hands input back while the user decides (pause / question / confirmation).
    async fn hand_back(&self, waiting: &str) {
        let screen = Arc::clone(&self.deps.screen);
        let own = self.deps.computer.own_pids();
        let text = waiting.to_owned();
        let _ = tokio::task::spawn_blocking(move || {
            screen.disarm_guard();
            let _ = screen.overlay(Some(&text));
            let _ = screen.own_windows(&own, false, false);
        })
        .await;
    }

    async fn retake(&self, ctrl: &Arc<Ctrl>, settings: &CursorSettings, lang: &str) {
        if let Err(e) = self.take_control(ctrl, settings, lang).await {
            tracing::warn!(error = %e, "could not retake control");
            ctrl.abort("error");
        }
        let pause = self.timing.read().take_control;
        sleep_or(&ctrl.cancel, pause).await;
    }

    async fn wait_paused(&self, ctrl: &Arc<Ctrl>, sid: &str, settings: &CursorSettings, lang: &str) {
        let say = self.say(&Moment::Paused, lang);
        self.set_state(sid, CursorState::Paused, None, say).await;
        self.hand_back(if lang.starts_with("ru") { "ИИ на паузе — курсор ваш" } else { "AI paused — the cursor is yours" }).await;
        let Some(mut rx) = ctrl.paused.as_ref().map(watch::Sender::subscribe) else { return };
        loop {
            if !*rx.borrow() {
                break;
            }
            tokio::select! {
                r = rx.changed() => if r.is_err() { break },
                () = ctrl.cancel.cancelled() => return,
            }
        }
        let say = self.say(&Moment::Resumed, lang);
        self.set_state(sid, CursorState::Observing, None, say).await;
        self.retake(ctrl, settings, lang).await;
    }

    async fn ask(&self, ctrl: &Arc<Ctrl>, sid: &str, question: &str, settings: &CursorSettings, lang: &str) -> Option<String> {
        let (tx, rx) = oneshot::channel();
        *ctrl.answer.lock() = Some(tx);
        self.patch(|s| s.question = Some(question.to_owned()));
        let say = self.say(&Moment::Ask(question), lang);
        self.set_state(sid, CursorState::AwaitingUser, None, say).await;
        self.publish(sid, "question", None, Some(question.to_owned())).await;
        self.hand_back(question).await;
        let r = tokio::select! {
            a = rx => a.ok(),
            () = ctrl.cancel.cancelled() => None,
        };
        ctrl.answer.lock().take();
        self.patch(|s| s.question = None);
        if r.is_some() {
            self.retake(ctrl, settings, lang).await;
        }
        r
    }

    async fn confirm_step(
        &self,
        ctrl: &Arc<Ctrl>,
        sid: &str,
        mut conf: CursorConfirmation,
        settings: &CursorSettings,
        lang: &str,
    ) -> Option<bool> {
        let mut req = ApprovalRequest::new(
            Capability::ComputerControl,
            format!("Cursor mode: {} — {}", conf.code, conf.detail).chars().take(300).collect::<String>(),
            "The AI controlling the cursor needs your confirmation for a risky step",
            if matches!(conf.code.as_str(), "purchase" | "delete" | "send") { RiskLevel::High } else { RiskLevel::Medium },
        );
        req.command_preview = Some(serde_json::to_string_pretty(&audit_action(&conf.action)).unwrap_or_default());
        req.ttl = Some(CONFIRM_TTL);
        let aid = match self.deps.security.approvals.request(req).await {
            Ok(a) => Some(a.id),
            Err(e) => {
                tracing::warn!(error = %e, "approval request failed; the confirmation stays in the panel");
                None
            }
        };
        conf.approval_id = aid;
        let (tx, rx) = oneshot::channel();
        *ctrl.confirm.lock() = Some(tx);
        let code = conf.code.clone();
        self.patch(|s| s.confirmation = Some(conf));
        let say = self.say(&Moment::Confirm, lang);
        self.set_state(sid, CursorState::AwaitingConfirmation, None, say).await;
        self.publish(sid, "confirm", None, Some(code.clone())).await;
        self.hand_back(if lang.starts_with("ru") { "Ожидаю вашего подтверждения…" } else { "Waiting for your confirmation…" }).await;
        let inbox = async {
            match aid {
                Some(a) => self.deps.security.approvals.wait(a, CONFIRM_TTL, &ctrl.cancel).await.ok(),
                None => std::future::pending().await,
            }
        };
        let r = tokio::select! {
            a = rx => a.ok(),
            s = inbox => s.map(|s| s == ApprovalStatus::Approved),
            () = ctrl.cancel.cancelled() => None,
        };
        ctrl.confirm.lock().take();
        self.patch(|s| s.confirmation = None);
        self.audit(
            "computer.cursorConfirm",
            sid,
            if r == Some(true) { AuditOutcome::Allowed } else { AuditOutcome::Rejected },
            serde_json::json!({ "code": code, "approved": r }),
        )
        .await;
        if r.is_some() {
            self.retake(ctrl, settings, lang).await;
        }
        r
    }

    async fn observe(&self, settings: &CursorSettings) -> ComputerResult<Observed> {
        let screen = Arc::clone(&self.deps.screen);
        let own = self.deps.computer.own_pids();
        let max_side = settings.max_image_side;
        let raw = tokio::task::spawn_blocking(move || -> ComputerResult<_> {
            let fg = screen.foreground()?;
            let fg_is_own = fg.as_ref().is_some_and(|f| own.contains(&f.pid) || is_own_process(&f.process));
            let monitor = screen.monitor_of(fg.as_ref())?;
            let img = screen.capture(monitor)?;
            let (w, h, rgb) = downscale_rgb(&img, max_side);
            let png = encode_png(w, h, &rgb)?;
            let raw_els = match &fg {
                Some(f) if !fg_is_own => screen.elements(f, MAX_ELEMENTS).unwrap_or_default(),
                _ => Vec::new(),
            };
            let cursor = screen.cursor_pos().ok().filter(|(x, y)| monitor.contains(*x, *y));
            Ok((fg, fg_is_own, monitor, w, h, png, raw_els, cursor))
        })
        .await
        .map_err(|e| ComputerError::Backend(e.to_string()))??;
        let (fg, fg_is_own, monitor, w, h, png, raw_els, cursor) = raw;
        let png_b64 = base64::engine::general_purpose::STANDARD.encode(&png);
        let shot = self.keep_shot(png, w, h);
        let mut elements = Vec::new();
        let mut kept = Vec::new();
        for e in raw_els {
            let c = e.rect.center();
            if !monitor.contains(c.0, c.1) || e.rect.w <= 1 || e.rect.h <= 1 {
                continue;
            }
            let id = u32::try_from(elements.len() + 1).unwrap_or(u32::MAX);
            elements.push(CursorElement {
                id,
                role: e.role.clone(),
                name: e.name.chars().take(80).collect(),
                rect: to_shot(e.rect, monitor, w, h),
                enabled: e.enabled,
                password: e.password,
                value: e.value.clone(),
            });
            kept.push(e);
        }
        let cursor = cursor.map(|(x, y)| {
            let r = to_shot(ScreenRect { x, y, w: 1, h: 1 }, monitor, w, h);
            (r[0], r[1])
        });
        Ok(Observed { fg, monitor, width: w, height: h, shot, png_b64, elements, raw: kept, cursor, fg_is_own })
    }

    async fn act(
        &self,
        action: &CursorAction,
        target: Option<(i32, i32)>,
        obs: &Observed,
        ctrl: &Arc<Ctrl>,
    ) -> ComputerResult<String> {
        let screen = Arc::clone(&self.deps.screen);
        let backend = self.deps.computer.backend();
        match action.clone() {
            CursorAction::Click { button, double, .. } => {
                let (x, y) = target.ok_or_else(|| ComputerError::Invalid("no target".into()))?;
                blocking(move || {
                    screen.move_to(x, y)?;
                    std::thread::sleep(Duration::from_millis(60));
                    screen.click_at(x, y, button, double)
                })
                .await?;
                Ok(format!("clicked at {x},{y}"))
            }
            CursorAction::Move { .. } => {
                let (x, y) = target.ok_or_else(|| ComputerError::Invalid("no target".into()))?;
                blocking(move || screen.move_to(x, y)).await?;
                Ok(format!("moved to {x},{y}"))
            }
            CursorAction::Drag { to, .. } => {
                let from = target.ok_or_else(|| ComputerError::Invalid("no start".into()))?;
                let to = point_of(&to, obs).ok_or_else(|| ComputerError::Invalid("bad drag end".into()))?;
                blocking(move || screen.drag(from, to)).await?;
                Ok(format!("dragged to {},{}", to.0, to.1))
            }
            CursorAction::Scroll { direction, amount, .. } => {
                let (x, y) = target
                    .or_else(|| obs.fg.as_ref().map(|f| f.rect.center()).filter(|(x, y)| obs.monitor.contains(*x, *y)))
                    .unwrap_or_else(|| obs.monitor.center());
                blocking(move || screen.scroll_at(x, y, direction, amount)).await?;
                Ok(format!("scrolled {direction:?} ×{amount}"))
            }
            CursorAction::Type { text } => {
                let chars: Vec<char> = text.chars().collect();
                for chunk in chars.chunks(12) {
                    if ctrl.cancel.is_cancelled() {
                        return Err(ComputerError::Stopped);
                    }
                    let s: String = chunk.iter().collect();
                    let b = Arc::clone(&backend);
                    let sc = Arc::clone(&screen);
                    let own = self.deps.computer.own_pids();
                    blocking(move || {
                        // re-check right before input: never type into Orkestr
                        if sc.foreground()?.is_some_and(|f| own.contains(&f.pid)) {
                            return Err(ComputerError::Forbidden("Orkestr's window is in front".into()));
                        }
                        b.type_text(&s)
                    })
                    .await?;
                }
                Ok(format!("typed {} characters", chars.len()))
            }
            CursorAction::Keys { keys } => {
                let label = keys.join("+");
                let own = self.deps.computer.own_pids();
                blocking(move || {
                    if screen.foreground()?.is_some_and(|f| own.contains(&f.pid)) {
                        return Err(ComputerError::Forbidden("Orkestr's window is in front".into()));
                    }
                    backend.press(&keys)
                })
                .await?;
                Ok(format!("pressed {label}"))
            }
            CursorAction::Wait { ms } => {
                sleep_or(&ctrl.cancel, Duration::from_millis(u64::from(ms))).await;
                Ok(format!("waited {ms} ms"))
            }
            CursorAction::FocusWindow { title } => {
                self.deps.computer.run_helper(ComputerAction::FocusApp { app: title }).await
            }
            CursorAction::Launch { app } => self.deps.computer.run_helper(ComputerAction::Launch { target: app }).await,
            CursorAction::AskUser { .. } | CursorAction::Done { .. } => Ok(String::new()),
        }
    }

    #[allow(clippy::too_many_arguments)]
    async fn record(
        &self,
        sid: &str,
        index: u32,
        d: &Decision,
        outcome: &str,
        detail: &str,
        target_rect: Option<[i32; 4]>,
        obs: &Observed,
        model: &FailoverModel,
        think_ms: u64,
        history: &mut Vec<HistoryItem>,
    ) {
        let step = CursorStep {
            index,
            at: chrono::Utc::now(),
            thought: d.thought.clone(),
            action: d.action.clone(),
            outcome: outcome.to_owned(),
            detail: detail.chars().take(300).collect(),
            target_rect,
            shot: Some(obs.shot.clone()),
            backend: model.current_id(),
            think_ms,
        };
        history.push(HistoryItem {
            index,
            action: serde_json::to_string(&d.action).unwrap_or_default(),
            outcome: outcome.to_owned(),
            detail: step.detail.clone(),
        });
        self.patch(|s| s.steps.push(step));
        self.audit(
            "computer.cursorStep",
            sid,
            match outcome {
                "ok" => AuditOutcome::Executed,
                "refused" => AuditOutcome::Denied,
                "rejected" => AuditOutcome::Rejected,
                _ => AuditOutcome::Failed,
            },
            serde_json::json!({
                "index": index,
                "action": audit_action(&d.action),
                "outcome": outcome,
                "backend": model.current_id(),
                "window": obs.fg.as_ref().map(|f| f.process.clone()),
            }),
        )
        .await;
        self.publish(sid, "step", None, Some(format!("{index}. {} → {outcome}", d.action.type_name()))).await;
    }
}

/// Orkestr's own processes by name (the desktop app, the daemon).
#[must_use]
pub fn is_own_process(process: &str) -> bool {
    let p = crate::procs::norm(process);
    p == "orkestr-desktop" || p == "orkestrd" || p == "orkestr"
}

/// The action for the audit log: typed text is reduced to its length.
#[must_use]
pub fn audit_action(a: &CursorAction) -> serde_json::Value {
    match a {
        CursorAction::Type { text } => serde_json::json!({ "type": "type", "chars": text.chars().count() }),
        other => serde_json::to_value(other).unwrap_or_default(),
    }
}

async fn blocking<T: Send + 'static>(f: impl FnOnce() -> ComputerResult<T> + Send + 'static) -> ComputerResult<T> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ComputerError::Backend(e.to_string()))?
}

async fn sleep_or(cancel: &CancellationToken, d: Duration) {
    tokio::select! {
        () = tokio::time::sleep(d) => {}
        () = cancel.cancelled() => {}
    }
}

fn point_of(t: &CursorTarget, obs: &Observed) -> Option<(i32, i32)> {
    if let Some(id) = t.element {
        let i = usize::try_from(id).ok()?.checked_sub(1)?;
        return obs.raw.get(i).map(|e| e.rect.center());
    }
    match (t.x, t.y) {
        (Some(x), Some(y)) => Some(to_screen(x, y, obs.monitor, obs.width, obs.height)),
        _ => None,
    }
}

/// Screen point + the element it hits.
fn resolve_target<'a>(a: &CursorAction, obs: &'a Observed) -> (Option<(i32, i32)>, Option<&'a CursorElement>) {
    let Some(t) = a.target() else { return (None, None) };
    let p = point_of(t, obs);
    let el = match t.element {
        Some(id) => obs.elements.iter().find(|e| e.id == id),
        None => {
            // the smallest element under the point
            let (x, y) = (t.x.unwrap_or(-1), t.y.unwrap_or(-1));
            obs.elements
                .iter()
                .filter(|e| x >= e.rect[0] && y >= e.rect[1] && x < e.rect[0] + e.rect[2] && y < e.rect[1] + e.rect[3])
                .min_by_key(|e| i64::from(e.rect[2]) * i64::from(e.rect[3]))
        }
    };
    (p, el)
}

fn target_error(a: &CursorAction, p: Option<(i32, i32)>, obs: &Observed) -> Option<String> {
    let t = a.target()?;
    match p {
        None => Some(match t.element {
            Some(id) => format!("Element #{id} does not exist in the current observation; use an id from the list."),
            None => "The action needs a target.".into(),
        }),
        Some((x, y)) if !obs.monitor.contains(x, y) => {
            Some(format!("Point is outside the screenshot ({}x{}).", obs.width, obs.height))
        }
        _ => None,
    }
}

