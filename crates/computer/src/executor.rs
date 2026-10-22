//! Runs a step pipeline: slot substitution, runtime safety checks
//! (password fields, unsaved windows), the user's target window (never
//! Orkestr's own windows), cancellation (global stop) between and inside
//! steps, and a report per step.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio_util::sync::CancellationToken;

use orkestr_protocol::methods::computer::{
    ClientEffect, ComputerAction, LevelOp, RiskReason, RunOutcome, SafetyLevel, ScrollUnit, SlotNum, StepCondition,
    StepReport, WindowInfo, WindowOp,
};

use crate::apps;
use crate::backend::{ComputerBackend, InstalledApp};
use crate::error::{ComputerError, ComputerResult};
use crate::keys::parse_combo;
use crate::safety::{title_looks_unsaved, PROTECTED_PROCESSES};
use crate::text::parse_number;

/// Callback for `Timer` steps: (seconds, message).
pub type TimerFn = Arc<dyn Fn(u32, String) + Send + Sync>;

/// Execution environment.
#[derive(Clone)]
pub struct ExecEnv {
    /// OS backend.
    pub backend: Arc<dyn ComputerBackend>,
    /// Start-menu apps.
    pub installed: Arc<Vec<InstalledApp>>,
    /// Global stop.
    pub cancel: CancellationToken,
    /// Orkestr's own processes (never the target of input).
    pub own_pids: Vec<u32>,
    /// Timer sink.
    pub timers: TimerFn,
    /// Screenshot folder.
    pub screenshot_dir: PathBuf,
}

/// The run stopped for a runtime confirmation.
#[derive(Debug, Clone)]
pub struct RuntimeConfirm {
    /// Top-level index to resume from.
    pub resume_at: usize,
    /// Why.
    pub reasons: Vec<RiskReason>,
}

/// Result of [`execute`].
#[derive(Debug, Clone)]
pub struct ExecResult {
    /// Outcome (`None` = waiting for a runtime confirmation).
    pub outcome: Option<RunOutcome>,
    /// Steps run.
    pub steps: Vec<StepReport>,
    /// Client effects.
    pub effects: Vec<ClientEffect>,
    /// Runtime confirmation request.
    pub confirm: Option<RuntimeConfirm>,
    /// Duration.
    pub elapsed: Duration,
}

/// Replaces `{slot}`s in every string of the steps. Unknown slots → `Invalid`.
///
/// # Errors
/// `Invalid` for unresolved slots / bad JSON.
pub fn substitute(steps: &[ComputerAction], slots: &BTreeMap<String, String>) -> ComputerResult<Vec<ComputerAction>> {
    let mut v = serde_json::to_value(steps).map_err(|e| ComputerError::Invalid(e.to_string()))?;
    fill(&mut v, slots)?;
    serde_json::from_value(v).map_err(|e| ComputerError::Invalid(format!("steps: {e}")))
}

fn fill(v: &mut serde_json::Value, slots: &BTreeMap<String, String>) -> ComputerResult<()> {
    match v {
        serde_json::Value::String(s) => {
            if s.contains('{') {
                for name in crate::catalogue::slots_in(s) {
                    let val =
                        slots.get(&name).ok_or_else(|| ComputerError::Invalid(format!("no value for {{{name}}}")))?;
                    *s = s.replace(&format!("{{{name}}}"), val);
                }
            }
        }
        serde_json::Value::Array(a) => {
            for x in a {
                fill(x, slots)?;
            }
        }
        serde_json::Value::Object(o) => {
            for x in o.values_mut() {
                fill(x, slots)?;
            }
        }
        _ => {}
    }
    Ok(())
}

fn num(v: Option<&SlotNum>, default: u32) -> ComputerResult<u32> {
    match v {
        None => Ok(default),
        Some(SlotNum::Num(n)) => Ok(*n),
        Some(SlotNum::Text(t)) => {
            parse_number(&t.to_lowercase()).ok_or_else(|| ComputerError::Invalid(format!("“{t}” is not a number")))
        }
    }
}

struct Runner<'a> {
    env: &'a ExecEnv,
    confirmed: bool,
    steps: Vec<StepReport>,
    effects: Vec<ClientEffect>,
    index: u32,
}

enum Flow {
    Next,
    Confirm(Vec<RiskReason>),
}

impl Runner<'_> {
    async fn call<T: Send + 'static>(
        &self,
        f: impl FnOnce(&dyn ComputerBackend) -> ComputerResult<T> + Send + 'static,
    ) -> ComputerResult<T> {
        let b = Arc::clone(&self.env.backend);
        let task = tokio::task::spawn_blocking(move || f(b.as_ref()));
        tokio::select! {
            r = task => r.map_err(|e| ComputerError::Backend(format!("backend task: {e}")))?,
            () = self.env.cancel.cancelled() => Err(ComputerError::Stopped),
        }
    }

    async fn sleep(&self, ms: u64) -> ComputerResult<()> {
        tokio::select! {
            () = tokio::time::sleep(Duration::from_millis(ms)) => Ok(()),
            () = self.env.cancel.cancelled() => Err(ComputerError::Stopped),
        }
    }

    fn is_own(&self, w: &WindowInfo) -> bool {
        let p = crate::procs::norm(&w.process);
        self.env.own_pids.contains(&w.pid) || p == "orkestr-desktop" || p == "orkestrd" || p == "orkestr"
    }

    async fn windows(&self) -> ComputerResult<Vec<WindowInfo>> {
        self.call(|b| b.windows()).await
    }

    /// The window the user means by "this window": the foreground one unless
    /// it is Orkestr's own (then the next one in z-order).
    async fn user_window(&self) -> ComputerResult<Option<WindowInfo>> {
        let ws = self.windows().await?;
        if let Some(a) = ws.iter().find(|w| w.active) {
            if !self.is_own(a) {
                return Ok(Some(a.clone()));
            }
        }
        Ok(ws.into_iter().find(|w| !self.is_own(w) && !w.minimized))
    }

    /// Makes sure keyboard / mouse input goes to the user's window.
    async fn ensure_target(&self) -> ComputerResult<Option<WindowInfo>> {
        let ws = self.windows().await?;
        let active = ws.iter().find(|w| w.active).cloned();
        if let Some(a) = &active {
            if !self.is_own(a) {
                return Ok(active);
            }
        }
        let Some(target) = ws.into_iter().find(|w| !self.is_own(w) && !w.minimized) else {
            return Ok(active);
        };
        let h = target.handle.clone();
        self.call(move |b| b.window_op(Some(&h), WindowOp::Focus)).await?;
        self.sleep(120).await?;
        Ok(Some(target))
    }

    async fn app_windows(&self, app: &str) -> ComputerResult<Vec<WindowInfo>> {
        Ok(self.windows().await?.into_iter().filter(|w| !self.is_own(w) && apps::window_matches(app, w)).collect())
    }

    fn push(&mut self, action: &ComputerAction, r: &ComputerResult<String>) {
        self.steps.push(StepReport {
            index: self.index,
            action: action.type_name().to_owned(),
            ok: r.is_ok(),
            detail: match r {
                Ok(d) => d.clone(),
                Err(e) => e.to_string(),
            },
        });
        self.index += 1;
    }

    async fn run_list(&mut self, steps: &[ComputerAction]) -> ComputerResult<Flow> {
        for (i, s) in steps.iter().enumerate() {
            if self.env.cancel.is_cancelled() {
                return Err(ComputerError::Stopped);
            }
            let more = i + 1 < steps.len();
            if let ComputerAction::If { condition, then, otherwise } = s {
                let cond = self.eval(condition).await;
                let r = cond
                    .as_ref()
                    .map(|b| format!("condition {}", if *b { "true" } else { "false" }))
                    .map_err(clone_err);
                self.push(s, &r);
                let branch = if cond? { then } else { otherwise };
                match Box::pin(self.run_list(branch)).await? {
                    Flow::Next => continue,
                    c @ Flow::Confirm(_) => return Ok(c),
                }
            }
            match self.step(s, more).await {
                Ok(StepOut::Done(detail)) => self.push(s, &Ok(detail)),
                Ok(StepOut::Confirm(reasons)) => return Ok(Flow::Confirm(reasons)),
                Err(ComputerError::Stopped) => return Err(ComputerError::Stopped),
                Err(e) => {
                    self.push(s, &Err(clone_err(&e)));
                    return Err(e);
                }
            }
        }
        Ok(Flow::Next)
    }

    async fn eval(&self, c: &StepCondition) -> ComputerResult<bool> {
        Ok(match c {
            StepCondition::WindowActive { app } => {
                self.user_window().await?.is_some_and(|w| apps::window_matches(app, &w))
            }
            StepCondition::WindowExists { app } => !self.app_windows(app).await?.is_empty(),
            StepCondition::ProcessRunning { name } => {
                let names = apps::process_names(name);
                self.call(|b| b.processes())
                    .await?
                    .iter()
                    .any(|p| names.iter().any(|n| crate::procs::norm(n) == crate::procs::norm(&p.name)))
            }
        })
    }

    #[allow(clippy::too_many_lines)]
    async fn step(&mut self, s: &ComputerAction, more: bool) -> ComputerResult<StepOut> {
        use ComputerAction as A;
        let done = |d: String| Ok(StepOut::Done(d));
        match s {
            A::Launch { target } => {
                let resolved = apps::launch_target(target, &self.env.installed);
                let before = if more { self.user_window().await?.map(|w| w.handle) } else { None };
                let t = resolved.clone();
                self.call(move |b| b.shell_open(&t, None)).await?;
                if more {
                    self.wait_new_foreground(before).await?;
                }
                done(format!("opened {resolved}"))
            }
            A::OpenUrl { url } => {
                let u = apps::site_url(url).unwrap_or_else(|| apps::normalize_url(url));
                let t = u.clone();
                self.call(move |b| b.shell_open(&t, None)).await?;
                done(u)
            }
            A::WebSearch { query, engine } => {
                if query.trim().is_empty() {
                    return Err(ComputerError::Invalid("empty search query".into()));
                }
                let u = apps::search_url(query, engine.as_deref());
                let t = u.clone();
                self.call(move |b| b.shell_open(&t, None)).await?;
                done(u)
            }
            A::OpenSettings { page } => {
                let u = apps::settings_uri(page);
                let t = u.clone();
                self.call(move |b| b.shell_open(&t, None)).await?;
                done(u)
            }
            A::OpenPath { path } => {
                let target = apps::known_folder(path).unwrap_or_else(|| {
                    let p = apps::file_path(path);
                    p.display().to_string()
                });
                if !apps::is_uri(&target) && !std::path::Path::new(&target).exists() {
                    return Err(ComputerError::NotFound(format!("“{path}” does not exist")));
                }
                let t = target.clone();
                self.call(move |b| b.shell_open(&t, None)).await?;
                done(target)
            }
            A::FocusApp { app } => {
                let ws = self.app_windows(app).await?;
                let w = ws.first().ok_or_else(|| ComputerError::NotFound(format!("no open window of “{app}”")))?;
                let h = w.handle.clone();
                self.call(move |b| b.window_op(Some(&h), WindowOp::Focus)).await?;
                done(w.title.clone())
            }
            A::Window { op, app } => {
                let w = match app.as_deref().filter(|a| !a.trim().is_empty()) {
                    Some(a) => self
                        .app_windows(a)
                        .await?
                        .into_iter()
                        .next()
                        .ok_or_else(|| ComputerError::NotFound(format!("no open window of “{a}”")))?,
                    None => match op {
                        WindowOp::SwitchNext | WindowOp::ShowDesktop => {
                            let op = *op;
                            self.call(move |b| b.window_op(None, op)).await?;
                            return done(format!("{op:?}"));
                        }
                        _ => self.user_window().await?.ok_or_else(|| ComputerError::NotFound("no window".into()))?,
                    },
                };
                if *op == WindowOp::Close && !self.confirmed && title_looks_unsaved(&w.title) {
                    return Ok(StepOut::Confirm(vec![unsaved_reason(self.index, &w.title)]));
                }
                let (h, op) = (w.handle.clone(), *op);
                self.call(move |b| b.window_op(Some(&h), op)).await?;
                done(format!("{op:?}: {}", w.title))
            }
            A::CloseApp { app, discard } => self.close_app(app, *discard).await,
            A::KillProcess { name } => {
                let names: Vec<String> = apps::process_names(name).iter().map(|n| crate::procs::norm(n)).collect();
                if names.iter().any(|n| PROTECTED_PROCESSES.contains(&n.as_str())) {
                    return Err(ComputerError::Forbidden(format!("refusing to kill the system process `{name}`")));
                }
                let own = self.env.own_pids.clone();
                let procs = self.call(|b| b.processes()).await?;
                let pids: Vec<u32> = procs
                    .iter()
                    .filter(|p| names.contains(&crate::procs::norm(&p.name)) && !own.contains(&p.pid))
                    .map(|p| p.pid)
                    .collect();
                if pids.is_empty() {
                    return Err(ComputerError::NotFound(format!("no process named “{name}”")));
                }
                let n = pids.len();
                self.call(move |b| pids.iter().try_for_each(|p| b.kill(*p))).await?;
                done(format!("terminated {n} process(es)"))
            }
            A::Keys { combo, times } => {
                let keys = parse_combo(combo)?;
                let n = num(times.as_ref(), 1)?.clamp(1, 50);
                self.ensure_target().await?;
                for _ in 0..n {
                    let k = keys.clone();
                    self.call(move |b| b.press(&k)).await?;
                    if n > 1 {
                        self.sleep(40).await?;
                    }
                }
                done(format!("{} ×{n}", keys.join("+")))
            }
            A::TypeText { text } => {
                self.ensure_target().await?;
                if !self.confirmed && self.call(|b| b.focused_is_password()).await.unwrap_or(false) {
                    return Ok(StepOut::Confirm(vec![RiskReason {
                        step: self.index,
                        code: "passwordField".into(),
                        detail: "the focused field is a password field".into(),
                        level: SafetyLevel::Dangerous,
                    }]));
                }
                let t = text.clone();
                self.call(move |b| b.type_text(&t)).await?;
                done(format!("typed {} characters", text.chars().count()))
            }
            A::Scroll { direction, amount, unit } => {
                let default = if *unit == ScrollUnit::Lines { 5 } else { 1 };
                let n = num(amount.as_ref(), default)?.clamp(1, 100);
                self.ensure_target().await?;
                let (d, u) = (*direction, *unit);
                self.call(move |b| b.scroll(d, n, u)).await?;
                done(format!("{d:?} {n} {u:?}"))
            }
            A::ClickElement { name } => {
                let w = self.ensure_target().await?;
                let (h, n) = (w.map(|w| w.handle), name.clone());
                let label = self.call(move |b| b.click_element(h.as_deref(), &n)).await?;
                done(format!("clicked “{label}”"))
            }
            A::MouseClick { button, x, y } => {
                let at = x.zip(*y);
                if at.is_none() {
                    self.ensure_target().await?;
                }
                let bt = *button;
                self.call(move |b| b.mouse_click(bt, at)).await?;
                done(format!("{bt:?}"))
            }
            A::Media { key } => {
                let k = *key;
                self.call(move |b| b.media(k)).await?;
                done(format!("{k:?}"))
            }
            A::Volume { op, value } => {
                let v = num(value.as_ref(), 10)?.min(100);
                let op = *op;
                #[allow(clippy::cast_possible_wrap)]
                self.call(move |b| match op {
                    LevelOp::Up => b.volume_step(v as i32, false),
                    LevelOp::Down => b.volume_step(-(v as i32), false),
                    LevelOp::Mute => b.volume_step(0, true),
                    LevelOp::Set => {
                        b.volume_step(-100, false)?;
                        if v > 0 {
                            b.volume_step(v as i32, false)?;
                        }
                        Ok(())
                    }
                })
                .await?;
                done(if op == LevelOp::Mute { "mute toggled".into() } else { format!("{op:?} {v}%") })
            }
            A::Brightness { op, value } => {
                let v = num(value.as_ref(), 10)?.min(100);
                let op = *op;
                let now = self
                    .call(move |b| {
                        let target = match op {
                            LevelOp::Set => v,
                            LevelOp::Up => (b.brightness(None)? + v).min(100),
                            LevelOp::Down => b.brightness(None)?.saturating_sub(v),
                            LevelOp::Mute => return Err(ComputerError::Invalid("brightness cannot be muted".into())),
                        };
                        b.brightness(Some(target))
                    })
                    .await?;
                done(format!("brightness {now}%"))
            }
            A::System { op } => {
                let op = *op;
                self.call(move |b| b.system(op)).await?;
                done(format!("{op:?}"))
            }
            A::Screenshot { path } => {
                let p = path.as_deref().map_or_else(
                    || {
                        self.env
                            .screenshot_dir
                            .join(format!("orkestr-{}.png", chrono::Local::now().format("%Y%m%d-%H%M%S")))
                    },
                    apps::file_path,
                );
                let pp = p.clone();
                self.call(move |b| b.screenshot(&pp)).await?;
                let shown = p.display().to_string();
                self.effects.push(ClientEffect::Screenshot { path: shown.clone() });
                done(shown)
            }
            A::ClipboardSet { text } => {
                let t = text.clone();
                self.call(move |b| b.clipboard_set(&t)).await?;
                done(format!("copied {} characters", text.chars().count()))
            }
            A::ClipboardGet {} => {
                let t = self.call(|b| b.clipboard_get()).await?;
                let lines: Vec<String> = t.lines().take(40).map(|l| l.chars().take(300).collect()).collect();
                self.effects.push(ClientEffect::Show { title: "clipboard".into(), lines });
                done(format!("{} characters", t.chars().count()))
            }
            A::Notify { title, body } => {
                let (t, b2) = (title.clone(), body.clone());
                self.call(move |b| b.notify(&t, &b2)).await?;
                done(title.clone())
            }
            A::Timer { seconds, message } => {
                let secs = num(Some(seconds), 60)?.clamp(1, 7 * 24 * 3600);
                (self.env.timers)(secs, message.clone());
                done(format!("timer {secs}s"))
            }
            A::ListProcesses {} => {
                let mut ps = self.call(|b| b.processes()).await?;
                ps.sort_by(|a, b| b.has_window.cmp(&a.has_window).then(b.memory_bytes.cmp(&a.memory_bytes)));
                let mut seen = std::collections::BTreeSet::new();
                let lines: Vec<String> = ps
                    .iter()
                    .filter(|p| seen.insert(p.name.to_lowercase()))
                    .take(30)
                    .map(|p| format!("{} — {} MB", p.name, p.memory_bytes / 1_048_576))
                    .collect();
                let n = lines.len();
                self.effects.push(ClientEffect::Show { title: "processes".into(), lines });
                done(format!("{n} apps"))
            }
            A::ListWindows {} => {
                let ws = self.windows().await?;
                let lines: Vec<String> =
                    ws.iter().filter(|w| !self.is_own(w)).map(|w| format!("{} ({})", w.title, w.process)).collect();
                let n = lines.len();
                self.effects.push(ClientEffect::Show { title: "windows".into(), lines });
                done(format!("{n} windows"))
            }
            A::FileCreate { path, content } => {
                let p = apps::file_path(path);
                if let Some(dir) = p.parent() {
                    std::fs::create_dir_all(dir)?;
                }
                let mut f = std::fs::OpenOptions::new().write(true).create_new(true).open(&p).map_err(|e| {
                    if e.kind() == std::io::ErrorKind::AlreadyExists {
                        ComputerError::Conflict(format!("{} already exists", p.display()))
                    } else {
                        ComputerError::Io(e)
                    }
                })?;
                std::io::Write::write_all(&mut f, content.as_bytes())?;
                done(p.display().to_string())
            }
            A::FolderCreate { path } => {
                let p = apps::file_path(path);
                std::fs::create_dir_all(&p)?;
                done(p.display().to_string())
            }
            A::FileMove { from, to } => {
                let (a, mut b) = (apps::file_path(from), apps::file_path(to));
                if !a.exists() {
                    return Err(ComputerError::NotFound(format!("{} does not exist", a.display())));
                }
                if b.is_dir() {
                    b = b.join(a.file_name().unwrap_or_default());
                }
                if b.exists() {
                    return Err(ComputerError::Conflict(format!("{} already exists", b.display())));
                }
                std::fs::rename(&a, &b)?;
                done(format!("{} → {}", a.display(), b.display()))
            }
            A::FileDelete { path } => {
                let p = apps::file_path(path);
                if !p.exists() {
                    return Err(ComputerError::NotFound(format!("{} does not exist", p.display())));
                }
                let pp = p.clone();
                self.call(move |b| b.recycle(&pp)).await?;
                done(format!("{} → recycle bin", p.display()))
            }
            A::Wait { ms } => {
                let ms = num(Some(ms), 500)?.min(10 * 60 * 1000);
                self.sleep(u64::from(ms)).await?;
                done(format!("{ms} ms"))
            }
            A::Speak { text } => {
                self.effects.push(ClientEffect::Speak { text: text.clone() });
                done(String::new())
            }
            A::SetMode { mode } => {
                self.effects.push(ClientEffect::SetMode { mode: *mode });
                done(format!("{mode:?}"))
            }
            A::If { .. } => done(String::new()),
        }
    }

    /// After a launch: wait (≤ 5 s) until a new window of another app is in
    /// front, so the next keys go to it.
    async fn wait_new_foreground(&self, before: Option<String>) -> ComputerResult<()> {
        let started = Instant::now();
        while started.elapsed() < Duration::from_secs(5) {
            self.sleep(150).await?;
            let ws = self.windows().await?;
            if let Some(a) = ws.iter().find(|w| w.active) {
                if !self.is_own(a) && Some(&a.handle) != before.as_ref() {
                    self.sleep(350).await?;
                    return Ok(());
                }
            }
        }
        Ok(())
    }

    async fn close_app(&mut self, app: &str, discard: bool) -> ComputerResult<StepOut> {
        let targets = if app.trim().is_empty() {
            self.user_window().await?.into_iter().collect::<Vec<_>>()
        } else {
            self.app_windows(app).await?
        };
        if targets.is_empty() {
            return Err(ComputerError::NotFound(format!("no open window of “{app}”")));
        }
        if !discard && !self.confirmed {
            if let Some(w) = targets.iter().find(|w| title_looks_unsaved(&w.title)) {
                return Ok(StepOut::Confirm(vec![unsaved_reason(self.index, &w.title)]));
            }
        }
        let no_kill = apps::find_known_app(app).is_some_and(|a| a.no_kill);
        let mut closed = 0usize;
        for w in &targets {
            let h = w.handle.clone();
            self.call(move |b| b.window_op(Some(&h), WindowOp::Close)).await?;
            if !discard {
                closed += 1;
                continue;
            }
            // Discard: answer "Don't save" when the app asks, kill as a last resort.
            let started = Instant::now();
            let mut gone = false;
            while started.elapsed() < Duration::from_secs(6) {
                self.sleep(300).await?;
                let alive = self.windows().await?.iter().any(|x| x.handle == w.handle);
                if !alive {
                    gone = true;
                    break;
                }
                for label in ["Don't save", "Don’t save", "Не сохранять", "Don't Save", "No", "Нет"] {
                    let h = w.handle.clone();
                    if self.call(move |b| b.click_element(Some(&h), label)).await.is_ok() {
                        break;
                    }
                    if self.call(move |b| b.click_element(None, label)).await.is_ok() {
                        break;
                    }
                }
            }
            if !gone && !no_kill && !self.env.own_pids.contains(&w.pid) {
                let pid = w.pid;
                self.call(move |b| b.kill(pid)).await?;
                gone = true;
            }
            if gone {
                closed += 1;
            }
        }
        Ok(StepOut::Done(format!("closed {closed} window(s){}", if discard { " without saving" } else { "" })))
    }
}

enum StepOut {
    Done(String),
    Confirm(Vec<RiskReason>),
}

fn unsaved_reason(step: u32, title: &str) -> RiskReason {
    RiskReason { step, code: "closeUnsaved".into(), detail: title.to_owned(), level: SafetyLevel::Confirm }
}

fn clone_err(e: &ComputerError) -> ComputerError {
    match e {
        ComputerError::Invalid(m) => ComputerError::Invalid(m.clone()),
        ComputerError::NotFound(m) => ComputerError::NotFound(m.clone()),
        ComputerError::Unsupported(m) => ComputerError::Unsupported(m.clone()),
        ComputerError::Forbidden(m) => ComputerError::Forbidden(m.clone()),
        ComputerError::Stopped => ComputerError::Stopped,
        ComputerError::Planner(m) => ComputerError::Planner(m.clone()),
        ComputerError::Conflict(m) => ComputerError::Conflict(m.clone()),
        other => ComputerError::Backend(other.to_string()),
    }
}

/// Runs resolved steps. `confirmed` skips runtime confirmations.
pub async fn execute(env: &ExecEnv, steps: &[ComputerAction], confirmed: bool) -> ExecResult {
    let started = Instant::now();
    let mut r = Runner { env, confirmed, steps: Vec::new(), effects: Vec::new(), index: 0 };
    // top level one by one so a runtime confirmation can resume there
    let mut outcome = Some(RunOutcome::Succeeded);
    let mut confirm = None;
    for (i, s) in steps.iter().enumerate() {
        match r.run_list(std::slice::from_ref(s)).await {
            Ok(Flow::Next) => {
                // let the target app catch up between top-level steps that use input
                if i + 1 < steps.len() && matches!(s, ComputerAction::FocusApp { .. } | ComputerAction::Window { .. }) {
                    let _ = r.sleep(150).await;
                }
            }
            Ok(Flow::Confirm(reasons)) => {
                outcome = None;
                confirm = Some(RuntimeConfirm { resume_at: i, reasons });
                break;
            }
            Err(ComputerError::Stopped) => {
                outcome = Some(RunOutcome::Stopped);
                break;
            }
            Err(_) => {
                outcome = Some(RunOutcome::Failed);
                break;
            }
        }
    }
    ExecResult { outcome, steps: r.steps, effects: r.effects, confirm, elapsed: started.elapsed() }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::{MockBackend, MockCall};
    use orkestr_protocol::methods::computer::ScrollDirection;

    fn env(b: Arc<MockBackend>) -> ExecEnv {
        ExecEnv {
            backend: b,
            installed: Arc::new(Vec::new()),
            cancel: CancellationToken::new(),
            own_pids: vec![999],
            timers: Arc::new(|_, _| {}),
            screenshot_dir: std::env::temp_dir(),
        }
    }

    fn steps(v: serde_json::Value) -> Vec<ComputerAction> {
        serde_json::from_value(v).unwrap()
    }

    #[tokio::test]
    async fn substitutes_slots_and_numbers() {
        let s = steps(
            serde_json::json!([{"type":"Scroll","direction":"Down","amount":"{n}"},{"type":"TypeText","text":"hi {text}!"}]),
        );
        let slots: BTreeMap<String, String> =
            [("n".to_owned(), "3".to_owned()), ("text".to_owned(), "Ann".to_owned())].into();
        let r = substitute(&s, &slots).unwrap();
        assert_eq!(r[1], ComputerAction::TypeText { text: "hi Ann!".into() });
        let b = Arc::new(MockBackend::default());
        b.add_window("0x1", "Doc - Notepad", "Notepad.exe", true);
        let out = execute(&env(Arc::clone(&b)), &r, false).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded), "{:?}", out.steps);
        let calls = b.take_calls();
        assert!(calls.contains(&MockCall::Scroll(ScrollDirection::Down, 3, ScrollUnit::Lines)));
        assert!(calls.contains(&MockCall::Type("hi Ann!".into())));
        assert!(substitute(&s, &BTreeMap::new()).is_err(), "missing slots are errors");
    }

    #[tokio::test]
    async fn input_never_goes_to_orkestr_itself() {
        let b = Arc::new(MockBackend::default());
        b.add_window("0x1", "Untitled - Notepad", "Notepad.exe", false);
        b.add_window("0x2", "Orkestr", "orkestr-desktop.exe", true);
        let out =
            execute(&env(Arc::clone(&b)), &steps(serde_json::json!([{"type":"Keys","combo":"ctrl+a"}])), false).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded));
        let calls = b.take_calls();
        assert_eq!(calls[0], MockCall::Window(Some("0x1".into()), WindowOp::Focus));
        assert_eq!(calls[1], MockCall::Press(vec!["ctrl".into(), "a".into()]));
    }

    #[tokio::test]
    async fn password_fields_and_unsaved_windows_ask_first() {
        let b = Arc::new(MockBackend::default());
        b.add_window("0x1", "*Untitled - Notepad", "Notepad.exe", true);
        *b.password_focused.lock() = true;
        let s = steps(serde_json::json!([{"type":"Keys","combo":"enter"},{"type":"TypeText","text":"secret"}]));
        let out = execute(&env(Arc::clone(&b)), &s, false).await;
        assert!(out.outcome.is_none());
        let c = out.confirm.unwrap();
        assert_eq!((c.resume_at, c.reasons[0].code.as_str()), (1, "passwordField"));
        assert!(!b.take_calls().contains(&MockCall::Type("secret".into())));
        let out = execute(&env(Arc::clone(&b)), &s[1..], true).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded));

        let close = steps(serde_json::json!([{"type":"CloseApp","app":"notepad"}]));
        let out = execute(&env(Arc::clone(&b)), &close, false).await;
        assert_eq!(out.confirm.unwrap().reasons[0].code, "closeUnsaved");
        let out = execute(&env(Arc::clone(&b)), &close, true).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded));
        assert!(b.windows.lock().is_empty());
    }

    #[tokio::test]
    async fn conditions_and_errors() {
        let b = Arc::new(MockBackend::default());
        let s = steps(serde_json::json!([{"type":"If","condition":{"type":"WindowExists","app":"notepad"},
            "then":[{"type":"FocusApp","app":"notepad"}],"otherwise":[{"type":"Launch","target":"notepad"}]}]));
        let out = execute(&env(Arc::clone(&b)), &s, false).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded));
        assert_eq!(b.take_calls(), vec![MockCall::Open("notepad.exe".into(), None)]);
        let out =
            execute(&env(Arc::clone(&b)), &steps(serde_json::json!([{"type":"FocusApp","app":"word"}])), false).await;
        assert_eq!(out.outcome, Some(RunOutcome::Failed));
        assert!(!out.steps[0].ok);
    }

    #[tokio::test]
    async fn global_stop_interrupts_waits() {
        let b = Arc::new(MockBackend::default());
        let e = env(b);
        let c = e.cancel.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(50)).await;
            c.cancel();
        });
        let t = Instant::now();
        let out =
            execute(&e, &steps(serde_json::json!([{"type":"Wait","ms":60000},{"type":"Speak","text":"x"}])), false)
                .await;
        assert_eq!(out.outcome, Some(RunOutcome::Stopped));
        assert!(t.elapsed() < Duration::from_secs(5));
        assert!(out.effects.is_empty());
    }

    #[tokio::test]
    async fn effects_and_timers() {
        let b = Arc::new(MockBackend::default());
        let fired = Arc::new(parking_lot::Mutex::new(Vec::new()));
        let f2 = Arc::clone(&fired);
        let mut e = env(Arc::clone(&b));
        e.timers = Arc::new(move |s, m| f2.lock().push((s, m)));
        let s = steps(
            serde_json::json!([{"type":"Timer","seconds":"300","message":"tea"},{"type":"Speak","text":"ok"},{"type":"SetMode","mode":"Assistant"}]),
        );
        let out = execute(&e, &s, false).await;
        assert_eq!(out.outcome, Some(RunOutcome::Succeeded));
        assert_eq!(fired.lock().as_slice(), &[(300, "tea".to_owned())]);
        assert_eq!(out.effects.len(), 2);
    }
}
