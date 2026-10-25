//! The decision model of cursor capture mode (ADR-0050).
//!
//! [`ComputerUseModel`] decides the next action from an [`Observation`].
//! Most backends are "a vision model that answers one prompt": they
//! implement [`VisionCompleter`] (system + user text + one PNG → text) and
//! are wrapped by [`JsonVisionModel`], which owns the prompt and parses the
//! JSON answer into a [`Decision`]. [`ScriptedModel`] is the deterministic
//! mock used by tests and the sandboxed smoke test.

use std::collections::VecDeque;
use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use orkestr_protocol::methods::cursor::{CursorAction, CursorElement};

use crate::keys::parse_combo;

/// Why a model call failed. The first five kinds move the call to the next
/// backend by priority (see `select`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelErrorKind {
    /// Not signed in / bad key.
    Auth,
    /// Subscription usage limit / quota / billing.
    Limit,
    /// Rate limited (429).
    RateLimit,
    /// Network / timeout / 5xx.
    Network,
    /// Model or CLI not available (not installed, unknown model).
    Unavailable,
    /// Cancelled (session stopped).
    Cancelled,
    /// The answer was not a valid action.
    BadOutput,
    /// Anything else.
    Other,
}

impl ModelErrorKind {
    /// Failures that should try the next backend.
    #[must_use]
    pub fn fails_over(self) -> bool {
        matches!(self, Self::Auth | Self::Limit | Self::RateLimit | Self::Network | Self::Unavailable)
    }

    /// Classifies an error message (CLI stderr, HTTP error text).
    #[must_use]
    pub fn classify(message: &str) -> Self {
        let m = message.to_lowercase();
        let has = |xs: &[&str]| xs.iter().any(|x| m.contains(x));
        if has(&[
            "usage limit",
            "limit reached",
            "quota",
            "insufficient_quota",
            "billing",
            "credit balance",
            "out of credits",
            "hit your limit",
        ]) {
            Self::Limit
        } else if has(&["rate limit", "rate_limit", "429", "too many requests", "overloaded"]) {
            Self::RateLimit
        } else if has(&[
            "unauthorized",
            "401",
            "403",
            "invalid api key",
            "invalid_api_key",
            "not logged in",
            "please run /login",
            "login required",
            "authentication",
            "not signed in",
        ]) {
            Self::Auth
        } else if has(&[
            "model not found",
            "model_not_found",
            "unknown model",
            "does not exist",
            "not installed",
            "no such file",
            "cannot find",
            "not supported",
            "not available",
        ]) {
            Self::Unavailable
        } else if has(&[
            "timed out",
            "timeout",
            "connection",
            "network",
            "dns",
            "502",
            "503",
            "504",
            "500 ",
            "internal server error",
            "econnreset",
            "unreachable",
        ]) {
            Self::Network
        } else {
            Self::Other
        }
    }
}

/// A model failure.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{message}")]
pub struct ModelError {
    /// Kind.
    pub kind: ModelErrorKind,
    /// Message (never contains secrets).
    pub message: String,
}

impl ModelError {
    /// New error.
    #[must_use]
    pub fn new(kind: ModelErrorKind, message: impl Into<String>) -> Self {
        Self { kind, message: message.into() }
    }
    /// Classifies `message`.
    #[must_use]
    pub fn classified(message: impl Into<String>) -> Self {
        let message = message.into();
        Self { kind: ModelErrorKind::classify(&message), message }
    }
    /// Cancelled.
    #[must_use]
    pub fn cancelled() -> Self {
        Self::new(ModelErrorKind::Cancelled, "cancelled")
    }
}

/// What the model sees in one step.
#[derive(Debug, Clone, Default)]
pub struct Observation {
    /// PNG, base64 (no `data:` prefix).
    pub image_base64: String,
    /// Screenshot width (px).
    pub width: u32,
    /// Screenshot height (px).
    pub height: u32,
    /// Elements of the foreground window (screenshot pixels).
    pub elements: Vec<CursorElement>,
    /// Cursor in screenshot pixels (when on the captured monitor).
    pub cursor: Option<(i32, i32)>,
    /// Foreground window title.
    pub window_title: String,
    /// Foreground process.
    pub window_process: String,
}

/// One earlier step, as the model is told about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryItem {
    /// 1-based.
    pub index: u32,
    /// Action JSON.
    pub action: String,
    /// `ok` / `error` / `refused` / `rejected`.
    pub outcome: String,
    /// Detail.
    pub detail: String,
}

/// A decision request.
#[derive(Debug, Clone, Default)]
pub struct DecideRequest {
    /// The user's task.
    pub task: String,
    /// Language for `say` / `summary` / questions.
    pub lang: String,
    /// 1-based step.
    pub step: u32,
    /// Step limit.
    pub max_steps: u32,
    /// Earlier steps (oldest first, capped).
    pub history: Vec<HistoryItem>,
    /// Questions asked and the user's answers.
    pub answers: Vec<(String, String)>,
    /// Notes from the runtime (refusals, failover…).
    pub notes: Vec<String>,
    /// Current screen.
    pub observation: Observation,
}

/// The model's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Decision {
    /// Short reasoning.
    pub thought: String,
    /// Remaining plan (may be empty = unchanged).
    pub plan: Vec<String>,
    /// Optional phrase for the user.
    pub say: Option<String>,
    /// Self-declared risk code (`purchase`, `delete`, `send`, `password`, `system`, `closeUnsaved`).
    pub risk: Option<String>,
    /// The action.
    pub action: CursorAction,
}

impl Decision {
    /// A decision with only an action.
    #[must_use]
    pub fn act(action: CursorAction) -> Self {
        Self { thought: String::new(), plan: Vec::new(), say: None, risk: None, action }
    }
}

/// Decides the next action.
#[async_trait]
pub trait ComputerUseModel: Send + Sync {
    /// Backend id (`claudeCode`, `codex`, `anthropic`, `openai`, `mock`).
    fn id(&self) -> &str;
    /// Model label (`claude-sonnet-4-5`, …).
    fn label(&self) -> String;
    /// Next action.
    ///
    /// # Errors
    /// [`ModelError`] (its kind decides failover).
    async fn decide(&self, req: &DecideRequest, cancel: &CancellationToken) -> Result<Decision, ModelError>;
}

/// "Answer this prompt about this image" — the transport of a vision model.
#[async_trait]
pub trait VisionCompleter: Send + Sync {
    /// Backend id.
    fn id(&self) -> &str;
    /// Model label.
    fn label(&self) -> String;
    /// Returns the model's text.
    ///
    /// # Errors
    /// [`ModelError`].
    async fn complete(
        &self,
        system: &str,
        user: &str,
        png_base64: &str,
        cancel: &CancellationToken,
    ) -> Result<String, ModelError>;
}

/// A [`VisionCompleter`] with Orkestr's JSON action prompt.
pub struct JsonVisionModel {
    inner: Arc<dyn VisionCompleter>,
}

impl JsonVisionModel {
    /// Wraps a transport.
    #[must_use]
    pub fn new(inner: Arc<dyn VisionCompleter>) -> Self {
        Self { inner }
    }
}

#[async_trait]
impl ComputerUseModel for JsonVisionModel {
    fn id(&self) -> &str {
        self.inner.id()
    }
    fn label(&self) -> String {
        self.inner.label()
    }
    async fn decide(&self, req: &DecideRequest, cancel: &CancellationToken) -> Result<Decision, ModelError> {
        let system = system_prompt(&req.lang);
        let mut user = user_prompt(req);
        let mut last = String::new();
        // one repair round for a malformed answer
        for attempt in 0..2 {
            let text = self.inner.complete(&system, &user, &req.observation.image_base64, cancel).await?;
            match parse_decision(&text) {
                Ok(d) => return Ok(d),
                Err(e) => {
                    last = e;
                    if attempt == 0 {
                        let _ = write!(
                            user,
                            "\n\nYour previous answer was invalid ({last}). Answer with ONE JSON object exactly as specified."
                        );
                    }
                }
            }
        }
        Err(ModelError::new(ModelErrorKind::BadOutput, format!("the model's answer is not a valid action: {last}")))
    }
}

/// Marker the providers' Mock recognizes (offline tests).
pub const CURSOR_MARKER: &str = "Orkestr cursor agent";

/// System prompt.
#[must_use]
pub fn system_prompt(lang: &str) -> String {
    let language = if lang.starts_with("ru") { "Russian" } else { "English" };
    format!(
        "You are the {CURSOR_MARKER} (a calm JARVIS-like butler) operating the user's Windows PC with the mouse and \
keyboard to complete the user's task. Every turn you get a screenshot of the screen, the UI Automation elements of the \
foreground window (id, role, name, [x,y,w,h] in screenshot pixels), the cursor position, the active window and your \
previous actions with their results. Decide ONE next action, then you will see the new screen and must verify its effect.

Answer with ONE JSON object only — no prose, no markdown fences:
{{\"thought\":\"one short sentence\",\"plan\":[\"remaining steps…\"],\"say\":\"\",\"risk\":\"\",\"action\":{{…}}}}

Actions (field `type`):
- {{\"type\":\"click\",\"element\":12}} or {{\"type\":\"click\",\"x\":640,\"y\":360}}; optional \"button\":\"Right\"|\"Middle\", \"double\":true
- {{\"type\":\"move\",\"element\":12}} (hover)
- {{\"type\":\"drag\",\"from\":{{\"element\":3}},\"to\":{{\"x\":500,\"y\":200}}}}
- {{\"type\":\"scroll\",\"direction\":\"Down\"|\"Up\"|\"Left\"|\"Right\",\"amount\":3}} (optional element / x,y to scroll over)
- {{\"type\":\"type\",\"text\":\"hello\"}} (into the focused control; click it first)
- {{\"type\":\"keys\",\"keys\":[\"ctrl\",\"l\"]}} (names: ctrl shift alt win enter esc tab backspace delete space up down left right home end pageup pagedown f1…f12, letters, digits)
- {{\"type\":\"wait\",\"ms\":1000}}
- {{\"type\":\"focus_window\",\"title\":\"part of a window title\"}}
- {{\"type\":\"launch\",\"app\":\"notepad\"}} (app name, URL, or a settings page such as ms-settings:bluetooth)
- {{\"type\":\"ask_user\",\"question\":\"…\"}} when only the user can decide or provide information
- {{\"type\":\"done\",\"summary\":\"…\",\"success\":true}} when the task is complete (success false when impossible)

Rules:
- Prefer element ids over coordinates; coordinates are screenshot pixels. One action per turn.
- Check the screenshot after every action; if nothing changed, try another way (a keyboard shortcut, another element).
- launch is the fastest way to open apps, sites and Windows settings pages.
- Never type passwords, PINs, card numbers, API keys or other secrets; never type into password fields — ask_user instead.
- Never touch Orkestr's own windows (titles containing \"Orkestr\").
- Set \"risk\" when the action buys or pays (\"purchase\"), deletes files or data (\"delete\"), sends a message, email or \
post (\"send\"), changes system settings (\"system\") or discards unsaved work (\"closeUnsaved\"); the user will confirm it.
- `say`, `summary` and questions are for the user, in {language}, very short (≤ 8 words), polite, butler style; `say` may be empty.
- Stay strictly within the user's task; do not explore unrelated apps."
    )
}

fn clip(s: &str, n: usize) -> String {
    let t: String = s.chars().take(n).collect();
    if s.chars().count() > n {
        format!("{t}…")
    } else {
        t
    }
}

/// User prompt (text part; the screenshot goes alongside).
#[must_use]
pub fn user_prompt(req: &DecideRequest) -> String {
    let o = &req.observation;
    let mut s = String::new();
    let _ = writeln!(s, "Task: {}", req.task);
    let _ = writeln!(s, "Step {} of at most {}.", req.step, req.max_steps);
    for (q, a) in &req.answers {
        let _ = writeln!(s, "You asked: {q}\nUser answered: {a}");
    }
    let _ = writeln!(s, "Screenshot: {}x{} px.", o.width, o.height);
    let _ = writeln!(s, "Active window: \"{}\" ({})", clip(&o.window_title, 120), o.window_process);
    if let Some((x, y)) = o.cursor {
        let _ = writeln!(s, "Cursor: {x},{y}");
    }
    if o.elements.is_empty() {
        let _ = writeln!(s, "Elements: none available (use coordinates).");
    } else {
        let _ = writeln!(s, "Elements of the active window:");
        for e in &o.elements {
            let [x, y, w, h] = e.rect;
            let mut flags = String::new();
            if !e.enabled {
                flags.push_str(" disabled");
            }
            if e.password {
                flags.push_str(" PASSWORD");
            }
            if let Some(v) = &e.value {
                let _ = write!(flags, " value=\"{}\"", clip(v, 40));
            }
            let _ = writeln!(s, "#{} {} \"{}\" [{x},{y},{w},{h}]{flags}", e.id, e.role, clip(&e.name, 80));
        }
    }
    if !req.history.is_empty() {
        let _ = writeln!(s, "Previous actions:");
        for h in &req.history {
            let _ = writeln!(s, "{}. {} → {} {}", h.index, h.action, h.outcome, clip(&h.detail, 160));
        }
    }
    for n in &req.notes {
        let _ = writeln!(s, "Note: {n}");
    }
    s.push_str("Reply with the JSON object for the next action.");
    s
}

#[derive(Deserialize)]
struct Wire {
    #[serde(default)]
    thought: String,
    #[serde(default)]
    plan: Vec<String>,
    #[serde(default)]
    say: Option<String>,
    #[serde(default)]
    risk: Option<String>,
    action: Option<serde_json::Value>,
}

/// Extracts the first balanced JSON object of `text`.
fn json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let mut depth = 0i32;
    let mut in_str = false;
    let mut esc = false;
    for (i, c) in text[start..].char_indices() {
        if in_str {
            match c {
                _ if esc => esc = false,
                '\\' => esc = true,
                '"' => in_str = false,
                _ => {}
            }
            continue;
        }
        match c {
            '"' => in_str = true,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(&text[start..start + i + 1]);
                }
            }
            _ => {}
        }
    }
    None
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.map(|x| x.trim().to_owned()).filter(|x| !x.is_empty())
}

/// Parses a model answer.
///
/// # Errors
/// Readable reason.
pub fn parse_decision(text: &str) -> Result<Decision, String> {
    let obj = json_object(text).ok_or_else(|| "no JSON object".to_owned())?;
    let v: serde_json::Value = serde_json::from_str(obj).map_err(|e| format!("bad JSON: {e}"))?;
    let wire: Wire = serde_json::from_value(v.clone()).map_err(|e| format!("bad shape: {e}"))?;
    // tolerate an action at the top level
    let raw = match wire.action {
        Some(a) => a,
        None if v.get("type").is_some() => v,
        None => return Err("missing `action`".into()),
    };
    let mut action: CursorAction =
        serde_json::from_value(normalize_action(raw)).map_err(|e| format!("bad action: {e}"))?;
    validate(&mut action)?;
    Ok(Decision {
        thought: clip(wire.thought.trim(), 300),
        plan: wire.plan.into_iter().map(|p| clip(p.trim(), 120)).filter(|p| !p.is_empty()).take(12).collect(),
        say: nonempty(wire.say).map(|s| clip(&s, 120)),
        risk: nonempty(wire.risk).filter(|r| r != "none"),
        action,
    })
}

/// Lower-cases the `type` tag and the enum values models like to vary.
fn normalize_action(mut v: serde_json::Value) -> serde_json::Value {
    if let Some(o) = v.as_object_mut() {
        if let Some(t) = o.get("type").and_then(|t| t.as_str()).map(|t| t.to_lowercase().replace(['-', ' '], "_")) {
            let t = match t.as_str() {
                "left_click" | "tap" => "click".to_owned(),
                "double_click" => {
                    o.insert("double".into(), serde_json::Value::Bool(true));
                    "click".to_owned()
                }
                "right_click" => {
                    o.insert("button".into(), "Right".into());
                    "click".to_owned()
                }
                "key" | "hotkey" | "press" => "keys".to_owned(),
                "type_text" | "write" => "type".to_owned(),
                "open" | "open_app" => "launch".to_owned(),
                "ask" => "ask_user".to_owned(),
                "finish" | "finished" | "complete" => "done".to_owned(),
                _ => t,
            };
            o.insert("type".into(), t.into());
        }
        for key in ["button", "direction"] {
            if let Some(s) = o.get(key).and_then(|b| b.as_str()) {
                let mut c = s.to_lowercase();
                if let Some(f) = c.get_mut(0..1) {
                    f.make_ascii_uppercase();
                }
                o.insert(key.into(), c.into());
            }
        }
        // "keys": "ctrl+l" → ["ctrl","l"]
        if let Some(k) = o.get("keys").and_then(|k| k.as_str()).map(str::to_owned) {
            o.insert("keys".into(), serde_json::json!([k]));
        }
    }
    v
}

fn validate(a: &mut CursorAction) -> Result<(), String> {
    match a {
        CursorAction::Click { target, .. } | CursorAction::Move { target } => {
            if target.is_empty() {
                return Err("the action needs `element` or `x`,`y`".into());
            }
        }
        CursorAction::Drag { from, to } => {
            if from.is_empty() || to.is_empty() {
                return Err("drag needs `from` and `to`".into());
            }
        }
        CursorAction::Scroll { amount, .. } => *amount = (*amount).clamp(1, 15),
        CursorAction::Type { text } => {
            if text.is_empty() {
                return Err("empty text".into());
            }
            if text.chars().count() > 2000 {
                return Err("text too long".into());
            }
        }
        CursorAction::Keys { keys } => {
            let combo = keys.join("+");
            *keys = parse_combo(&combo).map_err(|e| e.to_string())?;
        }
        CursorAction::Wait { ms } => *ms = (*ms).clamp(50, 5000),
        CursorAction::FocusWindow { title } | CursorAction::Launch { app: title } => {
            if title.trim().is_empty() {
                return Err("empty name".into());
            }
        }
        CursorAction::AskUser { question } => {
            if question.trim().is_empty() {
                return Err("empty question".into());
            }
        }
        CursorAction::Done { .. } => {}
    }
    Ok(())
}

/// Deterministic scripted model (tests, sandboxed smoke). Returns the
/// scripted decisions in order, then `done`.
#[derive(Debug, Default)]
pub struct ScriptedModel {
    id: String,
    script: Mutex<VecDeque<Result<Decision, ModelError>>>,
    seen: Mutex<Vec<DecideRequest>>,
}

impl ScriptedModel {
    /// A script of decisions.
    #[must_use]
    pub fn new(id: &str, script: Vec<Decision>) -> Self {
        Self::with_results(id, script.into_iter().map(Ok).collect())
    }
    /// A script that may contain failures.
    #[must_use]
    pub fn with_results(id: &str, script: Vec<Result<Decision, ModelError>>) -> Self {
        Self { id: id.to_owned(), script: Mutex::new(script.into()), seen: Mutex::new(Vec::new()) }
    }
    /// A script of actions.
    #[must_use]
    pub fn actions(id: &str, actions: Vec<CursorAction>) -> Self {
        Self::new(id, actions.into_iter().map(Decision::act).collect())
    }
    /// Requests seen so far (screenshots stripped).
    #[must_use]
    pub fn seen(&self) -> Vec<DecideRequest> {
        self.seen.lock().clone()
    }
}

#[async_trait]
impl ComputerUseModel for ScriptedModel {
    fn id(&self) -> &str {
        &self.id
    }
    fn label(&self) -> String {
        "scripted".into()
    }
    async fn decide(&self, req: &DecideRequest, cancel: &CancellationToken) -> Result<Decision, ModelError> {
        if cancel.is_cancelled() {
            return Err(ModelError::cancelled());
        }
        let mut r = req.clone();
        r.observation.image_base64.clear();
        self.seen.lock().push(r);
        self.script.lock().pop_front().unwrap_or_else(|| {
            Ok(Decision::act(CursorAction::Done { summary: "script finished".into(), success: true }))
        })
    }
}

#[cfg(test)]
mod tests {
    use orkestr_protocol::methods::computer::{MouseButton, ScrollDirection};
    use orkestr_protocol::methods::cursor::CursorTarget;

    use super::*;

    #[test]
    fn parses_answers_with_noise() {
        let d = parse_decision(
            "Sure!\n```json\n{\"thought\":\"open it\",\"plan\":[\"a\",\"b\"],\"say\":\"Открываю, сэр.\",\"action\":{\"type\":\"click\",\"element\":7}}\n```",
        )
        .unwrap();
        assert_eq!(
            d.action,
            CursorAction::Click { target: CursorTarget::element(7), button: MouseButton::Left, double: false }
        );
        assert_eq!(d.plan.len(), 2);
        assert_eq!(d.say.as_deref(), Some("Открываю, сэр."));
        assert!(d.risk.is_none());
        let d = parse_decision(r#"{"type":"double_click","x":1,"y":2}"#).unwrap();
        assert_eq!(
            d.action,
            CursorAction::Click { target: CursorTarget::point(1, 2), button: MouseButton::Left, double: true }
        );
        let d = parse_decision(r#"{"action":{"type":"key","keys":"Ctrl+L"},"risk":"none"}"#).unwrap();
        assert_eq!(d.action, CursorAction::Keys { keys: vec!["ctrl".into(), "l".into()] });
        let d = parse_decision(r#"{"action":{"type":"scroll","direction":"down","amount":99}}"#).unwrap();
        assert_eq!(
            d.action,
            CursorAction::Scroll { target: CursorTarget::default(), direction: ScrollDirection::Down, amount: 15 }
        );
        let d =
            parse_decision(r#"{"risk":"purchase","action":{"type":"click","element":1,"button":"right"}}"#).unwrap();
        assert_eq!(d.risk.as_deref(), Some("purchase"));
        assert!(matches!(d.action, CursorAction::Click { button: MouseButton::Right, .. }));
    }

    #[test]
    fn rejects_bad_answers() {
        assert!(parse_decision("I will click OK").is_err());
        assert!(parse_decision(r#"{"action":{"type":"click"}}"#).is_err());
        assert!(parse_decision(r#"{"action":{"type":"keys","keys":["ctrl","nothing"]}}"#).is_err());
        assert!(parse_decision(r#"{"action":{"type":"fly"}}"#).is_err());
        assert!(parse_decision(r#"{"action":{"type":"type","text":""}}"#).is_err());
    }

    #[test]
    fn braces_inside_strings_do_not_confuse_the_extractor() {
        let d = parse_decision(r#"{"thought":"a } b","action":{"type":"type","text":"{x}"}}"#).unwrap();
        assert_eq!(d.action, CursorAction::Type { text: "{x}".into() });
    }

    #[test]
    fn classifies_errors_for_failover() {
        for (m, k) in [
            ("Claude AI usage limit reached|1730000000", ModelErrorKind::Limit),
            ("HTTP 429 Too Many Requests", ModelErrorKind::RateLimit),
            ("Invalid API key · Please run /login", ModelErrorKind::Auth),
            ("program not installed", ModelErrorKind::Unavailable),
            ("request timed out", ModelErrorKind::Network),
            ("weird", ModelErrorKind::Other),
        ] {
            assert_eq!(ModelErrorKind::classify(m), k, "{m}");
        }
        assert!(ModelErrorKind::Limit.fails_over());
        assert!(!ModelErrorKind::BadOutput.fails_over());
        assert!(!ModelErrorKind::Cancelled.fails_over());
    }

    #[test]
    fn prompt_lists_elements_and_history() {
        let req = DecideRequest {
            task: "open bluetooth settings".into(),
            lang: "ru".into(),
            step: 2,
            max_steps: 30,
            history: vec![HistoryItem {
                index: 1,
                action: "{\"type\":\"launch\"}".into(),
                outcome: "ok".into(),
                detail: String::new(),
            }],
            observation: Observation {
                width: 1280,
                height: 720,
                elements: vec![CursorElement {
                    id: 3,
                    role: "button".into(),
                    name: "Bluetooth".into(),
                    rect: [1, 2, 3, 4],
                    enabled: true,
                    password: false,
                    value: Some("off".into()),
                }],
                ..Observation::default()
            },
            ..DecideRequest::default()
        };
        let p = user_prompt(&req);
        assert!(p.contains("#3 button \"Bluetooth\" [1,2,3,4] value=\"off\""), "{p}");
        assert!(p.contains("1. {\"type\":\"launch\"} → ok"));
        assert!(system_prompt("ru").contains("Russian"));
    }

    struct Echo(Mutex<Vec<String>>);
    #[async_trait]
    impl VisionCompleter for Echo {
        fn id(&self) -> &str {
            "echo"
        }
        fn label(&self) -> String {
            "echo".into()
        }
        async fn complete(&self, _: &str, _: &str, _: &str, _: &CancellationToken) -> Result<String, ModelError> {
            Ok(self.0.lock().remove(0))
        }
    }

    #[tokio::test]
    async fn json_model_repairs_once() {
        let echo = Arc::new(Echo(Mutex::new(vec!["nonsense".into(), r#"{"action":{"type":"wait","ms":10}}"#.into()])));
        let m = JsonVisionModel::new(echo.clone());
        let d = m.decide(&DecideRequest::default(), &CancellationToken::new()).await.unwrap();
        assert_eq!(d.action, CursorAction::Wait { ms: 50 });
        echo.0.lock().extend(["x".to_owned(), "y".to_owned()]);
        let e = m.decide(&DecideRequest::default(), &CancellationToken::new()).await.unwrap_err();
        assert_eq!(e.kind, ModelErrorKind::BadOutput);
    }
}
