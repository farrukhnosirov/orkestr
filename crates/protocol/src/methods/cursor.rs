//! `computer.cursor*` — cursor capture mode (ADR-0050): on an explicit
//! start (voice «Джарвис, возьми управление» or a button) the AI drives the
//! mouse and keyboard to perform a task described in natural language, then
//! hands control back. Observe (screenshot + UI Automation tree) → decide
//! (a vision model) → act → verify, until done / aborted / step or time
//! limit. Esc or a physical mouse movement aborts instantly; risky steps
//! need a confirmation with a screenshot of the target; Orkestr's own
//! windows are never acted on. **Local only** (the whole `computer.` area).
//! Crate `orkestr-computer` (`cursor`), handlers `core/src/rpc/cursor.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ApprovalId, Timestamp};

use super::computer::{MouseButton, ScrollDirection};
use super::{method, Empty};

// ---------------------------------------------------------------------------
// Action vocabulary (what the model may ask for)
// ---------------------------------------------------------------------------

/// Where an action points: a UI Automation element of the observation
/// (preferred) or a point in screenshot pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct CursorTarget {
    /// Element id from the observation's element list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub element: Option<u32>,
    /// X in screenshot pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub x: Option<i32>,
    /// Y in screenshot pixels.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub y: Option<i32>,
}

impl CursorTarget {
    /// Element target.
    #[must_use]
    pub fn element(id: u32) -> Self {
        Self { element: Some(id), x: None, y: None }
    }
    /// Point target.
    #[must_use]
    pub fn point(x: i32, y: i32) -> Self {
        Self { element: None, x: Some(x), y: Some(y) }
    }
    /// True when neither an element nor a full point is given.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.element.is_none() && (self.x.is_none() || self.y.is_none())
    }
}

/// One action of the cursor agent. The model answers with exactly this JSON
/// (`{"type":"click","element":12}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "snake_case", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum CursorAction {
    /// Click an element / point.
    Click {
        /// Target.
        #[serde(flatten)]
        target: CursorTarget,
        /// Button (`Double` = left double click).
        #[serde(default)]
        button: MouseButton,
        /// Double click.
        #[serde(default)]
        double: bool,
    },
    /// Move the pointer (hover).
    Move {
        /// Target.
        #[serde(flatten)]
        target: CursorTarget,
    },
    /// Drag with the left button.
    Drag {
        /// Start.
        from: CursorTarget,
        /// End.
        to: CursorTarget,
    },
    /// Scroll (over the target when given).
    Scroll {
        /// Target (optional).
        #[serde(flatten)]
        target: CursorTarget,
        /// Direction.
        direction: ScrollDirection,
        /// Wheel notches (1–15).
        #[serde(default = "one")]
        amount: u32,
    },
    /// Type text into the focused control (never into password fields).
    Type {
        /// Text.
        text: String,
    },
    /// Press a key combination (`["ctrl","n"]`).
    Keys {
        /// Canonical key names.
        keys: Vec<String>,
    },
    /// Wait for the UI to settle.
    Wait {
        /// Milliseconds (≤ 5000).
        ms: u32,
    },
    /// Bring a window to the front by (part of) its title.
    FocusWindow {
        /// Title.
        title: String,
    },
    /// Launch a program / site / settings page (the assistant's `OpenApp`).
    Launch {
        /// App name, URL or `ms-settings:` page.
        app: String,
    },
    /// Ask the user a question (the session waits for `computer.cursorAnswer`).
    AskUser {
        /// Question.
        question: String,
    },
    /// The task is finished (or impossible).
    Done {
        /// Short result for the user.
        summary: String,
        /// False when the task could not be completed.
        #[serde(default = "yes")]
        success: bool,
    },
}

fn one() -> u32 {
    1
}
fn yes() -> bool {
    true
}

impl CursorAction {
    /// `type` tag.
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Click { .. } => "click",
            Self::Move { .. } => "move",
            Self::Drag { .. } => "drag",
            Self::Scroll { .. } => "scroll",
            Self::Type { .. } => "type",
            Self::Keys { .. } => "keys",
            Self::Wait { .. } => "wait",
            Self::FocusWindow { .. } => "focus_window",
            Self::Launch { .. } => "launch",
            Self::AskUser { .. } => "ask_user",
            Self::Done { .. } => "done",
        }
    }

    /// The primary target, if any.
    #[must_use]
    pub fn target(&self) -> Option<&CursorTarget> {
        match self {
            Self::Click { target, .. } | Self::Move { target } => Some(target),
            Self::Scroll { target, .. } if !target.is_empty() => Some(target),
            Self::Drag { from, .. } => Some(from),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Observation (shown in the panel; the screenshot itself is fetched by id)
// ---------------------------------------------------------------------------

/// A UI Automation element of the foreground window (screenshot pixels).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorElement {
    /// Id the model refers to.
    pub id: u32,
    /// Control type (`button`, `edit`, `checkbox`…).
    pub role: String,
    /// Accessible name (trimmed, ≤ 80 chars).
    pub name: String,
    /// `[x, y, w, h]` in screenshot pixels.
    pub rect: [i32; 4],
    /// Enabled.
    pub enabled: bool,
    /// Password field (never typed into).
    #[serde(default)]
    pub password: bool,
    /// Toggle / value state when cheap to read (`on`, `off`, text value ≤ 40 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub value: Option<String>,
}

/// A stored screenshot of the session (fetch with `computer.cursorScreenshot`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorShot {
    /// Id.
    pub id: String,
    /// Width (px).
    pub width: u32,
    /// Height (px).
    pub height: u32,
}

// ---------------------------------------------------------------------------
// Session
// ---------------------------------------------------------------------------

/// Session state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CursorState {
    /// Taking control (overlay, hooks, own windows aside).
    Starting,
    /// Capturing the screen.
    Observing,
    /// Waiting for the model.
    Thinking,
    /// Performing an action.
    Acting,
    /// Paused by the user (input is the user's).
    Paused,
    /// A risky step waits for a confirmation.
    AwaitingConfirmation,
    /// The model asked the user a question.
    AwaitingUser,
    /// Finished (the model said `done`).
    Done,
    /// Stopped (Esc, mouse, Stop, emergency, limit).
    Aborted,
    /// Failed (no model, repeated errors).
    Failed,
}

impl CursorState {
    /// Terminal state.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Done | Self::Aborted | Self::Failed)
    }
}

/// One executed (or refused) step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorStep {
    /// 1-based.
    pub index: u32,
    /// Time.
    pub at: Timestamp,
    /// Model's short reasoning.
    pub thought: String,
    /// The action.
    pub action: CursorAction,
    /// `ok`, `error`, `refused`, `rejected`, `confirmed`.
    pub outcome: String,
    /// Detail / error.
    pub detail: String,
    /// Target rectangle in screenshot pixels (`[x, y, w, h]`), for the highlight.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_rect: Option<[i32; 4]>,
    /// Screenshot the decision was made on.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shot: Option<CursorShot>,
    /// Backend that decided.
    pub backend: String,
    /// Model time (ms).
    pub think_ms: u64,
}

/// A risky step waiting for the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorConfirmation {
    /// Code: `password`, `secret`, `purchase`, `delete`, `send`, `closeUnsaved`, `system`, `modelFlagged`.
    pub code: String,
    /// Detail (element / window).
    pub detail: String,
    /// The step.
    pub action: CursorAction,
    /// Screenshot (target highlighted by `targetRect`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub shot: Option<CursorShot>,
    /// Target rectangle (screenshot pixels).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_rect: Option<[i32; 4]>,
    /// Approval in the approvals inbox.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
}

/// A cursor capture session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorSession {
    /// Id.
    pub session_id: String,
    /// The user's task.
    pub task: String,
    /// `voice` / `ui` / `palette` / `orb`.
    pub source: String,
    /// State.
    pub state: CursorState,
    /// Backend currently deciding (`claudeCode`, `codex`, `anthropic`, `openai`, `mock`).
    pub backend: String,
    /// Model label.
    pub model: String,
    /// The model's plan (updated as it goes).
    pub plan: Vec<String>,
    /// What it is doing now.
    pub current: String,
    /// Steps so far.
    pub steps: Vec<CursorStep>,
    /// Last screenshot.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_shot: Option<CursorShot>,
    /// Pending confirmation.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirmation: Option<CursorConfirmation>,
    /// Pending question.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub question: Option<String>,
    /// Result (done) or why it stopped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub summary: Option<String>,
    /// Abort reason code: `esc`, `mouse`, `stop`, `emergency`, `stepLimit`, `timeLimit`, `secureDesktop`, `error`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub abort_reason: Option<String>,
    /// Step limit.
    pub max_steps: u32,
    /// Time limit (s).
    pub max_seconds: u32,
    /// Started.
    pub started_at: Timestamp,
    /// Finished.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
    /// Language of the spoken phrases.
    pub lang: String,
}

// ---------------------------------------------------------------------------
// Backends + settings
// ---------------------------------------------------------------------------

/// Where the model comes from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CursorSource {
    /// The user's subscriptions: Claude Code CLI / Codex CLI (no API key).
    #[default]
    Subscriptions,
    /// API keys from the vault: Anthropic / OpenAI / Gemini vision models.
    Api,
}

/// A model backend and whether it can run now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorBackendInfo {
    /// `claudeCode`, `codex`, `anthropic`, `openai`, `mock`.
    pub id: String,
    /// Display label.
    pub label: String,
    /// Source group.
    pub source: CursorSource,
    /// Usable now (installed / key present).
    pub available: bool,
    /// Why not / what it uses.
    pub detail: String,
    /// Model (default or the user's override).
    pub model: String,
    /// The user agreed to send screenshots to this provider.
    pub consented: bool,
}

/// Cursor mode settings (`<data>/computer/cursor.json`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase", default)]
#[ts(export)]
pub struct CursorSettings {
    /// Model source switch.
    pub source: CursorSource,
    /// Priority of the subscription backends (`claudeCode`, `codex`).
    pub subscription_order: Vec<String>,
    /// Priority of the API backends (`anthropic`, `openai`, `gemini`).
    pub api_order: Vec<String>,
    /// Model override per backend.
    pub models: BTreeMap<String, String>,
    /// Step limit (1–100).
    pub max_steps: u32,
    /// Time limit in seconds (30–1800).
    pub max_seconds: u32,
    /// A physical mouse movement aborts (Esc always does).
    pub abort_on_mouse: bool,
    /// Minimize Orkestr's windows while the AI works (restored after).
    pub minimize_orkestr: bool,
    /// Longest screenshot side sent to the model (px, 640–1600).
    pub max_image_side: u32,
    /// One-time privacy consent per backend (time granted).
    pub consents: BTreeMap<String, Timestamp>,
}

impl Default for CursorSettings {
    fn default() -> Self {
        Self {
            source: CursorSource::Subscriptions,
            subscription_order: vec!["claudeCode".into(), "codex".into()],
            api_order: vec!["anthropic".into(), "openai".into(), "gemini".into()],
            models: BTreeMap::new(),
            max_steps: 30,
            max_seconds: 300,
            abort_on_mouse: true,
            minimize_orkestr: true,
            max_image_side: 1280,
            consents: BTreeMap::new(),
        }
    }
}

/// `computer.cursorStatus` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorStatus {
    /// Current / last session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub session: Option<CursorSession>,
    /// Backends in effective priority order (selected source first).
    pub backends: Vec<CursorBackendInfo>,
    /// The backend that will be tried first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub active_backend: Option<String>,
    /// Settings.
    pub settings: CursorSettings,
    /// The OS backend can capture and drive input (`windows`; `mock` in tests).
    pub supported: bool,
    /// Why not.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unsupported_reason: Option<String>,
}

// ---------------------------------------------------------------------------
// Params / results
// ---------------------------------------------------------------------------

/// `computer.cursorStart`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorStartParams {
    /// Task in natural language.
    pub task: String,
    /// UI / voice language.
    #[serde(default)]
    pub lang: String,
    /// `voice` / `ui` / `palette` / `orb`.
    #[serde(default)]
    pub source: String,
}

/// `computer.cursorStart` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum CursorStartResult {
    /// Running.
    Started {
        /// Session.
        session: Box<CursorSession>,
    },
    /// The first backend needs the one-time privacy consent (`computer.cursorConsent`).
    NeedsConsent {
        /// Backend.
        backend: CursorBackendInfo,
    },
    /// No model backend can run.
    NoBackend {
        /// Why.
        reason: String,
    },
}

/// `computer.cursorAnswer`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorAnswerParams {
    /// The answer.
    pub text: String,
}

/// `computer.cursorConfirm`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorConfirmParams {
    /// Approve the pending step.
    pub approve: bool,
}

/// `computer.cursorConsent`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorConsentParams {
    /// Backend id.
    pub backend: String,
    /// Grant (true) or revoke.
    pub granted: bool,
}

/// `computer.cursorSetSettings`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorSetSettingsParams {
    /// New settings (`consents` is ignored — use `computer.cursorConsent`).
    pub settings: CursorSettings,
}

/// `computer.cursorScreenshot`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorShotParams {
    /// Shot id.
    pub id: String,
}

/// A screenshot (PNG, base64).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorShotResult {
    /// MIME type.
    pub media_type: String,
    /// Base64 data.
    pub data_base64: String,
    /// Width.
    pub width: u32,
    /// Height.
    pub height: u32,
}

/// `ComputerCursor` event payload (no screenshots, no typed text).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CursorEventPayload {
    /// Session.
    pub session_id: String,
    /// `state`, `step`, `say`, `failover`, `confirm`, `question`, `finished`.
    pub event: String,
    /// State after the event.
    pub state: CursorState,
    /// Phrase for the Jarvis voice (already localized).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub say: Option<String>,
    /// Detail (backend switch, step summary…).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
    /// Step count so far.
    pub steps: u32,
}

method!(/// Cursor mode status, backends and settings.
    CursorStatusMethod, "computer.cursorStatus", Empty, CursorStatus);
method!(/// Starts a cursor capture session.
    CursorStart, "computer.cursorStart", CursorStartParams, CursorStartResult);
method!(/// Stops the session and hands control back.
    CursorStop, "computer.cursorStop", Empty, CursorStatus);
method!(/// Pauses the session (input returns to the user).
    CursorPause, "computer.cursorPause", Empty, CursorStatus);
method!(/// Resumes a paused session.
    CursorResume, "computer.cursorResume", Empty, CursorStatus);
method!(/// Answers the model's question.
    CursorAnswer, "computer.cursorAnswer", CursorAnswerParams, CursorStatus);
method!(/// Approves / rejects the pending risky step.
    CursorConfirm, "computer.cursorConfirm", CursorConfirmParams, CursorStatus);
method!(/// Grants / revokes the one-time privacy consent for a backend.
    CursorConsent, "computer.cursorConsent", CursorConsentParams, CursorStatus);
method!(/// Saves the cursor mode settings.
    CursorSetSettings, "computer.cursorSetSettings", CursorSetSettingsParams, CursorStatus);
method!(/// A session screenshot (local only, never persisted beyond the session).
    CursorScreenshot, "computer.cursorScreenshot", CursorShotParams, CursorShotResult);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn model_json_parses_into_actions() {
        let a: CursorAction = serde_json::from_str(r#"{"type":"click","element":12}"#).unwrap();
        assert_eq!(
            a,
            CursorAction::Click { target: CursorTarget::element(12), button: MouseButton::Left, double: false }
        );
        let a: CursorAction = serde_json::from_str(r#"{"type":"click","x":10,"y":20,"button":"Right"}"#).unwrap();
        assert_eq!(a.target(), Some(&CursorTarget::point(10, 20)));
        let a: CursorAction = serde_json::from_str(r#"{"type":"scroll","direction":"Down"}"#).unwrap();
        assert_eq!(
            a,
            CursorAction::Scroll { target: CursorTarget::default(), direction: ScrollDirection::Down, amount: 1 }
        );
        assert!(a.target().is_none());
        let a: CursorAction = serde_json::from_str(r#"{"type":"done","summary":"ok"}"#).unwrap();
        assert_eq!(a, CursorAction::Done { summary: "ok".into(), success: true });
        let a: CursorAction = serde_json::from_str(r#"{"type":"focus_window","title":"Notepad"}"#).unwrap();
        assert_eq!(a.type_name(), "focus_window");
        let a: CursorAction =
            serde_json::from_str(r#"{"type":"drag","from":{"element":1},"to":{"x":5,"y":6}}"#).unwrap();
        assert_eq!(a.target(), Some(&CursorTarget::element(1)));
        let v = serde_json::to_value(CursorAction::AskUser { question: "?".into() }).unwrap();
        assert_eq!(v["type"], "ask_user");
    }

    #[test]
    fn settings_default_and_partial() {
        let s: CursorSettings = serde_json::from_str(r#"{"source":"api"}"#).unwrap();
        assert_eq!(s.source, CursorSource::Api);
        assert_eq!(s.max_steps, 30);
        assert_eq!(s.subscription_order, vec!["claudeCode".to_owned(), "codex".to_owned()]);
    }
}
