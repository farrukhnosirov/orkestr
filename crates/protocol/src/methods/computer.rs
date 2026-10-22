//! `computer.*` — the personal computer assistant (ADR-0035): voice / text
//! commands that open, switch and close programs, drive windows, keyboard
//! and mouse, system functions, files and the browser. A local matcher runs
//! the 1000+ command catalogue and the user's command sets; unknown commands
//! get an AI plan built from the same action vocabulary, which the user
//! approves (or corrects) before it runs and may remember as a new command.
//! Risky steps always need a confirmation (Dangerous Action Guard + approval).
//! **Local only**: remote principals can never call this area.
//! Crate `orkestr-computer`, handlers `core/src/rpc/computer.rs`.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ApprovalId, Timestamp};

use super::{method, Empty};

// ---------------------------------------------------------------------------
// Action vocabulary
// ---------------------------------------------------------------------------

/// A number that may also be a `{slot}` template (resolved before running).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(export)]
pub enum SlotNum {
    /// Literal number.
    Num(u32),
    /// Template text, e.g. `"{n}"`.
    Text(String),
}

impl Default for SlotNum {
    fn default() -> Self {
        Self::Num(0)
    }
}

impl From<u32> for SlotNum {
    fn from(n: u32) -> Self {
        Self::Num(n)
    }
}

/// Window operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum WindowOp {
    /// Bring to front.
    Focus,
    /// Minimize.
    Minimize,
    /// Maximize.
    Maximize,
    /// Restore from minimized / maximized.
    Restore,
    /// Ask the window to close (`WM_CLOSE`; the app may ask to save).
    Close,
    /// Snap to the left half.
    SnapLeft,
    /// Snap to the right half.
    SnapRight,
    /// Move to the next monitor.
    NextMonitor,
    /// Move to the previous monitor.
    PreviousMonitor,
    /// Switch to the next window (Alt+Tab).
    SwitchNext,
    /// Minimize everything / show the desktop.
    ShowDesktop,
}

/// Scroll direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ScrollDirection {
    /// Up.
    Up,
    /// Down.
    Down,
    /// Left.
    Left,
    /// Right.
    Right,
}

/// Scroll unit.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ScrollUnit {
    /// Wheel notches.
    #[default]
    Lines,
    /// Pages (Page Up / Page Down).
    Pages,
    /// To the very top / bottom (Home / End).
    Edge,
}

/// Mouse button.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MouseButton {
    /// Left.
    #[default]
    Left,
    /// Right.
    Right,
    /// Middle.
    Middle,
    /// Left double click.
    Double,
}

/// Media key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MediaKey {
    /// Play / pause.
    PlayPause,
    /// Next track.
    Next,
    /// Previous track.
    Previous,
    /// Stop.
    Stop,
}

/// Level change (volume, brightness).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum LevelOp {
    /// Increase by `value` percent (default 10).
    Up,
    /// Decrease by `value` percent (default 10).
    Down,
    /// Set to `value` percent.
    Set,
    /// Toggle mute (volume only).
    Mute,
}

/// System / power operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SystemOp {
    /// Lock the workstation.
    Lock,
    /// Sleep.
    Sleep,
    /// Shut down (always confirmed).
    Shutdown,
    /// Restart (always confirmed).
    Restart,
    /// Sign out (always confirmed).
    SignOut,
}

/// Assistant UI mode.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum AssistantMode {
    /// The project orchestrator UI.
    #[default]
    Orchestra,
    /// The minimalist assistant panel.
    Assistant,
}

/// Condition of an `If` step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum StepCondition {
    /// The foreground window belongs to `app` (process name or title fragment).
    WindowActive {
        /// App.
        app: String,
    },
    /// A window of `app` exists.
    WindowExists {
        /// App.
        app: String,
    },
    /// A process with this name runs.
    ProcessRunning {
        /// Process name (`notepad` / `notepad.exe`).
        name: String,
    },
}

/// One step of a command pipeline. String fields may contain `{slot}`
/// templates filled from the matched phrase.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ComputerAction {
    /// Start a program: Start-menu app name, alias, `.exe` / `.lnk` path,
    /// `shell:AppsFolder\<AUMID>`, URL or file.
    Launch {
        /// Target.
        target: String,
    },
    /// Open a web page.
    OpenUrl {
        /// URL (scheme optional).
        url: String,
    },
    /// Web search.
    WebSearch {
        /// Query.
        query: String,
        /// Engine / site: google, yandex, bing, duckduckgo, youtube, wikipedia, github, maps…
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        engine: Option<String>,
    },
    /// Open a Windows Settings page (`ms-settings:<page>`).
    OpenSettings {
        /// Page id, e.g. `display`.
        page: String,
    },
    /// Open a file or folder (known names: downloads, documents, desktop…).
    OpenPath {
        /// Path or known folder.
        path: String,
    },
    /// Close an app's windows (graceful). `discard` closes without saving
    /// (answers "Don't save"; always confirmed).
    CloseApp {
        /// App (empty = the active window).
        app: String,
        /// Close without saving.
        #[serde(default)]
        discard: bool,
    },
    /// Terminate processes by name (always confirmed).
    KillProcess {
        /// Process name.
        name: String,
    },
    /// Bring an app's window to the front.
    FocusApp {
        /// App.
        app: String,
    },
    /// Window operation on an app's window (default: the active window).
    Window {
        /// Operation.
        op: WindowOp,
        /// App (absent = active window).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        app: Option<String>,
    },
    /// Press a key combination, e.g. `ctrl+shift+t`, `alt+f4`, `win+d`.
    Keys {
        /// Combination.
        combo: String,
        /// Repeat count (default 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        times: Option<SlotNum>,
    },
    /// Type text into the focused control (confirmed for password fields).
    TypeText {
        /// Text.
        text: String,
    },
    /// Scroll the window under the cursor / focused window.
    Scroll {
        /// Direction.
        direction: ScrollDirection,
        /// Amount (wheel notches or pages; default 5 / 1).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        amount: Option<SlotNum>,
        /// Unit.
        #[serde(default)]
        unit: ScrollUnit,
    },
    /// Click a UI element of the foreground window by its accessible name
    /// (UI Automation).
    ClickElement {
        /// Element name.
        name: String,
    },
    /// Mouse click at the cursor (or at `x`,`y`).
    MouseClick {
        /// Button.
        #[serde(default)]
        button: MouseButton,
        /// X (screen px).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        x: Option<i32>,
        /// Y (screen px).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        y: Option<i32>,
    },
    /// Media key.
    Media {
        /// Key.
        key: MediaKey,
    },
    /// Volume.
    Volume {
        /// Operation.
        op: LevelOp,
        /// Percent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        value: Option<SlotNum>,
    },
    /// Display brightness (laptop panels).
    Brightness {
        /// Operation.
        op: LevelOp,
        /// Percent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        value: Option<SlotNum>,
    },
    /// Lock / sleep / shutdown / restart / sign out.
    System {
        /// Operation.
        op: SystemOp,
    },
    /// Screenshot of all screens to a PNG (default: Pictures\Screenshots).
    Screenshot {
        /// Target path.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        path: Option<String>,
    },
    /// Put text on the clipboard.
    ClipboardSet {
        /// Text.
        text: String,
    },
    /// Read the clipboard (result is spoken / shown).
    ClipboardGet {},
    /// Desktop notification.
    Notify {
        /// Title.
        title: String,
        /// Body.
        #[serde(default)]
        body: String,
    },
    /// Timer / reminder: notifies (and speaks) after `seconds`.
    Timer {
        /// Delay.
        seconds: SlotNum,
        /// Reminder text.
        #[serde(default)]
        message: String,
    },
    /// List running apps (result shown).
    ListProcesses {},
    /// List open windows (result shown).
    ListWindows {},
    /// Create a file (fails if it exists).
    FileCreate {
        /// Path (relative = Documents).
        path: String,
        /// Content.
        #[serde(default)]
        content: String,
    },
    /// Create a folder.
    FolderCreate {
        /// Path (relative = Documents).
        path: String,
    },
    /// Move / rename a file or folder (always confirmed).
    FileMove {
        /// Source.
        from: String,
        /// Destination.
        to: String,
    },
    /// Delete a file or folder to the recycle bin (always confirmed).
    FileDelete {
        /// Path.
        path: String,
    },
    /// Pause.
    Wait {
        /// Milliseconds.
        ms: SlotNum,
    },
    /// Say something (the client speaks it with the active TTS engine).
    Speak {
        /// Text.
        text: String,
    },
    /// Switch the UI mode (client effect).
    SetMode {
        /// Mode.
        mode: AssistantMode,
    },
    /// Conditional branch.
    If {
        /// Condition.
        condition: StepCondition,
        /// Steps when true.
        #[serde(default)]
        then: Vec<ComputerAction>,
        /// Steps when false.
        #[serde(default)]
        otherwise: Vec<ComputerAction>,
    },
}

impl ComputerAction {
    /// The variant name (`"Launch"`, `"Keys"`, …).
    #[must_use]
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Launch { .. } => "Launch",
            Self::OpenUrl { .. } => "OpenUrl",
            Self::WebSearch { .. } => "WebSearch",
            Self::OpenSettings { .. } => "OpenSettings",
            Self::OpenPath { .. } => "OpenPath",
            Self::CloseApp { .. } => "CloseApp",
            Self::KillProcess { .. } => "KillProcess",
            Self::FocusApp { .. } => "FocusApp",
            Self::Window { .. } => "Window",
            Self::Keys { .. } => "Keys",
            Self::TypeText { .. } => "TypeText",
            Self::Scroll { .. } => "Scroll",
            Self::ClickElement { .. } => "ClickElement",
            Self::MouseClick { .. } => "MouseClick",
            Self::Media { .. } => "Media",
            Self::Volume { .. } => "Volume",
            Self::Brightness { .. } => "Brightness",
            Self::System { .. } => "System",
            Self::Screenshot { .. } => "Screenshot",
            Self::ClipboardSet { .. } => "ClipboardSet",
            Self::ClipboardGet {} => "ClipboardGet",
            Self::Notify { .. } => "Notify",
            Self::Timer { .. } => "Timer",
            Self::ListProcesses {} => "ListProcesses",
            Self::ListWindows {} => "ListWindows",
            Self::FileCreate { .. } => "FileCreate",
            Self::FolderCreate { .. } => "FolderCreate",
            Self::FileMove { .. } => "FileMove",
            Self::FileDelete { .. } => "FileDelete",
            Self::Wait { .. } => "Wait",
            Self::Speak { .. } => "Speak",
            Self::SetMode { .. } => "SetMode",
            Self::If { .. } => "If",
        }
    }
}

/// Safety level of a command / plan.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SafetyLevel {
    /// Runs immediately.
    #[default]
    Safe,
    /// Asks for confirmation (possible data loss / disruptive).
    Confirm,
    /// Dangerous: always confirmed, shown in the approvals inbox.
    Dangerous,
}

// ---------------------------------------------------------------------------
// Commands and sets
// ---------------------------------------------------------------------------

/// Text in both UI languages.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LocalizedText {
    /// English.
    #[serde(default)]
    pub en: String,
    /// Russian.
    #[serde(default)]
    pub ru: String,
}

impl LocalizedText {
    /// Text for `lang` (falls back to the other language).
    #[must_use]
    pub fn get(&self, lang: &str) -> &str {
        let (first, second) = if lang.starts_with("ru") { (&self.ru, &self.en) } else { (&self.en, &self.ru) };
        if first.is_empty() {
            second
        } else {
            first
        }
    }
}

/// Active-window filter of a context-specific command / set.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AppContext {
    /// Process names (`chrome`, `msedge.exe`); any matches.
    #[serde(default)]
    pub processes: Vec<String>,
    /// Window title fragments; any matches.
    #[serde(default)]
    pub title_contains: Vec<String>,
}

impl AppContext {
    /// No filter at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.processes.is_empty() && self.title_contains.is_empty()
    }
}

/// A voice / text command: phrases (with `{slot}`s) → step pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AssistantCommand {
    /// Stable id (unique inside the set; catalogue ids are globally unique).
    pub id: String,
    /// Category (apps, windows, keys, media, system, settings, web, files, …).
    #[serde(default)]
    pub category: String,
    /// Title.
    #[serde(default)]
    pub title: LocalizedText,
    /// Trigger phrases (any language). Slots: `{app}`, `{n}`, `{text}`,
    /// `{query}`, `{site}`, `{path}`, `{name}` (any identifier works).
    pub phrases: Vec<String>,
    /// Steps.
    pub steps: Vec<ComputerAction>,
    /// Declared safety (the effective level is the max of this and the steps').
    #[serde(default)]
    pub safety: SafetyLevel,
    /// Only active when the foreground window matches.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<AppContext>,
}

/// A command set (built-in catalogue or user-created, like LAITIS).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandSet {
    /// Id (`[a-z0-9-]`, 1..64).
    pub id: String,
    /// Display name.
    pub name: String,
    /// Description.
    #[serde(default)]
    pub description: String,
    /// Enabled.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// The built-in catalogue (read-only; may only be enabled / disabled).
    #[serde(default)]
    pub builtin: bool,
    /// Set-wide active-window filter (per-app context set).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<AppContext>,
    /// Commands.
    #[serde(default)]
    pub commands: Vec<AssistantCommand>,
    /// Last change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
}

const fn yes() -> bool {
    true
}

/// Set without its commands (lists).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandSetSummary {
    /// Id.
    pub id: String,
    /// Name.
    pub name: String,
    /// Description.
    pub description: String,
    /// Enabled.
    pub enabled: bool,
    /// Built-in.
    pub builtin: bool,
    /// Context filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context: Option<AppContext>,
    /// Number of commands.
    pub command_count: u32,
    /// Last change.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
}

/// Catalogue / search row.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandSummary {
    /// Set.
    pub set_id: String,
    /// Command id.
    pub command_id: String,
    /// Category.
    pub category: String,
    /// Title (requested language).
    pub title: String,
    /// Example phrases (requested language first).
    pub phrases: Vec<String>,
    /// Effective safety.
    pub safety: SafetyLevel,
    /// Context-specific.
    pub contextual: bool,
}

/// A matched command.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandMatch {
    /// Set.
    pub set_id: String,
    /// Command.
    pub command_id: String,
    /// Title.
    pub title: String,
    /// The phrase template that matched.
    pub phrase: String,
    /// Score 0..1.
    pub score: f32,
    /// Slot values.
    pub slots: BTreeMap<String, String>,
    /// Effective safety.
    pub safety: SafetyLevel,
}

// ---------------------------------------------------------------------------
// Runs, confirmations, plans
// ---------------------------------------------------------------------------

/// Result of one step.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StepReport {
    /// Index in the flattened run.
    pub index: u32,
    /// Action type.
    pub action: String,
    /// Succeeded.
    pub ok: bool,
    /// What happened / error.
    pub detail: String,
}

/// Something the client must do (speak, switch mode, show a list…).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum ClientEffect {
    /// Speak this text.
    Speak {
        /// Text.
        text: String,
    },
    /// Switch the UI mode.
    SetMode {
        /// Mode.
        mode: AssistantMode,
    },
    /// Show text (clipboard content, a list…).
    Show {
        /// Title.
        title: String,
        /// Lines.
        lines: Vec<String>,
    },
    /// A screenshot was saved.
    Screenshot {
        /// Path.
        path: String,
    },
}

/// Outcome of a finished run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum RunOutcome {
    /// All steps succeeded.
    Succeeded,
    /// A step failed.
    Failed,
    /// Stopped (global stop / emergency).
    Stopped,
    /// The user rejected the confirmation.
    Rejected,
}

/// A finished run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunReport {
    /// Run id.
    pub run_id: String,
    /// Command (absent for ad-hoc steps / plans).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command_id: Option<String>,
    /// Title.
    pub title: String,
    /// Outcome.
    pub outcome: RunOutcome,
    /// Steps.
    pub steps: Vec<StepReport>,
    /// Client effects.
    pub effects: Vec<ClientEffect>,
    /// Started.
    pub started_at: Timestamp,
    /// Duration.
    pub duration_ms: u64,
}

/// Why a step needs a confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RiskReason {
    /// Step index.
    pub step: u32,
    /// Code: `closeDiscard`, `closeUnsaved`, `killProcess`, `fileDelete`,
    /// `fileMove`, `power`, `passwordField`, `forbiddenPath`, `aiPlan`.
    pub code: String,
    /// Detail (target).
    pub detail: String,
    /// Level.
    pub level: SafetyLevel,
}

/// A run waiting for the user's confirmation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PendingConfirmation {
    /// Pending id.
    pub pending_id: String,
    /// Approval in the approvals inbox (Dangerous Action Guard).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
    /// Title.
    pub title: String,
    /// Reasons.
    pub reasons: Vec<RiskReason>,
    /// Steps that will run.
    pub steps: Vec<ComputerAction>,
}

/// An AI plan for an unknown command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanProposal {
    /// Plan id.
    pub plan_id: String,
    /// The user's request.
    pub request: String,
    /// Corrections applied so far.
    pub corrections: Vec<String>,
    /// Steps.
    pub steps: Vec<ComputerAction>,
    /// Model explanation.
    pub explanation: String,
    /// Model used (`provider/model`).
    pub model: String,
    /// Effective safety.
    pub safety: SafetyLevel,
    /// Risky steps.
    pub risks: Vec<RiskReason>,
    /// Created.
    pub created_at: Timestamp,
}

/// Result of `computer.handle` / `run` / `execute` / `confirm` / `approvePlan`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "status", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum HandleResult {
    /// The command ran.
    Executed {
        /// What matched (absent for plans / ad-hoc steps).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        matched: Option<CommandMatch>,
        /// Report.
        run: RunReport,
    },
    /// Waiting for a confirmation (`computer.confirm` or the approvals inbox).
    NeedsConfirmation {
        /// What matched.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        matched: Option<CommandMatch>,
        /// Pending run.
        pending: PendingConfirmation,
    },
    /// Unknown command: an AI plan to approve / correct.
    Proposed {
        /// Plan.
        plan: PlanProposal,
    },
    /// "Remember this as …": the last plan was saved as a user command.
    Remembered {
        /// Set.
        set_id: String,
        /// The new command.
        command: AssistantCommand,
    },
    /// Nothing matched (and no plan was requested / possible).
    NotUnderstood {
        /// Closest commands.
        suggestions: Vec<CommandMatch>,
        /// Why no plan was made (no model…).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        #[ts(optional)]
        reason: Option<String>,
    },
}

/// A history entry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryEntry {
    /// Time.
    pub at: Timestamp,
    /// What the user said / typed (or the command title).
    pub text: String,
    /// Command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command_id: Option<String>,
    /// Title.
    pub title: String,
    /// Outcome code: `Succeeded`, `Failed`, `Stopped`, `Rejected`,
    /// `NeedsConfirmation`, `Proposed`, `NotUnderstood`.
    pub outcome: String,
}

/// An open top-level window.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowInfo {
    /// Native handle.
    pub handle: String,
    /// Title.
    pub title: String,
    /// Process name (`notepad.exe`).
    pub process: String,
    /// Process id.
    pub pid: u32,
    /// Foreground window.
    pub active: bool,
    /// Minimized.
    pub minimized: bool,
}

/// A running process.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessInfo {
    /// Pid.
    pub pid: u32,
    /// Name.
    pub name: String,
    /// Resident memory bytes.
    pub memory_bytes: u64,
    /// Has a visible window.
    pub has_window: bool,
}

/// `ComputerCommand` event payload (no typed text, no clipboard content).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ComputerCommandPayload {
    /// Run / pending / plan id.
    pub id: String,
    /// Command.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub command_id: Option<String>,
    /// Title.
    pub title: String,
    /// Outcome code (as [`HistoryEntry::outcome`], plus `TimerFired`).
    pub outcome: String,
    /// Message (timer reminders).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}

// ---------------------------------------------------------------------------
// Params / results
// ---------------------------------------------------------------------------

/// `computer.status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ComputerStatus {
    /// Computer control works on this host.
    pub available: bool,
    /// Backend (`windows`, `unsupported`).
    pub backend: String,
    /// Why it is unavailable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub unavailable_reason: Option<String>,
    /// Built-in catalogue commands.
    pub catalogue_commands: u32,
    /// User commands (all sets).
    pub user_commands: u32,
    /// Enabled sets.
    pub enabled_sets: u32,
    /// Runs in progress.
    pub running: u32,
    /// Pending confirmations.
    pub pending: u32,
    /// Indexed Start-menu apps.
    pub indexed_apps: u32,
}

/// `computer.handle`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HandleParams {
    /// Utterance / typed text.
    pub text: String,
    /// UI language (`en` / `ru`) for titles.
    #[serde(default)]
    pub lang: String,
    /// Ask the AI for a plan when nothing matches (default true).
    #[serde(default = "yes")]
    pub plan_unknown: bool,
    /// Where it came from (`voice`, `text`, `palette`…), for the audit.
    #[serde(default)]
    pub source: String,
}

/// `computer.match`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MatchParams {
    /// Text.
    pub text: String,
    /// UI language.
    #[serde(default)]
    pub lang: String,
    /// Max results (default 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Matches.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MatchResult {
    /// Best first.
    pub matches: Vec<CommandMatch>,
    /// Matcher time.
    pub elapsed_us: u64,
}

/// `computer.run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunComputerCommandParams {
    /// Set (absent = search all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_id: Option<String>,
    /// Command.
    pub command_id: String,
    /// Slot values.
    #[serde(default)]
    pub slots: BTreeMap<String, String>,
    /// UI language.
    #[serde(default)]
    pub lang: String,
}

/// `computer.execute` — ad-hoc steps (command editor "Test").
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExecuteParams {
    /// Title.
    #[serde(default)]
    pub title: String,
    /// Steps.
    pub steps: Vec<ComputerAction>,
    /// Slot values.
    #[serde(default)]
    pub slots: BTreeMap<String, String>,
}

/// `computer.confirm`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ConfirmParams {
    /// Pending id.
    pub pending_id: String,
    /// Approve (true) or reject.
    pub approve: bool,
}

/// `computer.plan` — ask (or re-ask with a correction) for a plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanParams {
    /// Request (ignored when `plan_id` is set).
    #[serde(default)]
    pub text: String,
    /// Existing plan to correct.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plan_id: Option<String>,
    /// The user's correction.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub correction: Option<String>,
    /// UI language.
    #[serde(default)]
    pub lang: String,
}

/// A plan.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanResult {
    /// Plan.
    pub plan: PlanProposal,
}

/// `computer.approvePlan`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PlanIdParams {
    /// Plan.
    pub plan_id: String,
}

/// `computer.rememberPlan` — save an (approved) plan as a user command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RememberPlanParams {
    /// Plan.
    pub plan_id: String,
    /// Trigger phrases (default: the original request).
    #[serde(default)]
    pub phrases: Vec<String>,
    /// Title (default: the request).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// Target set (default `my-commands`, created on demand).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_id: Option<String>,
}

/// A saved command.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommandResult {
    /// Set.
    pub set_id: String,
    /// Command.
    pub command: AssistantCommand,
}

/// `computer.catalogue`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CatalogueParams {
    /// Free-text filter (titles / phrases).
    #[serde(default)]
    pub query: String,
    /// Category filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub category: Option<String>,
    /// Set filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub set_id: Option<String>,
    /// UI language.
    #[serde(default)]
    pub lang: String,
    /// Page size (default 50, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
    /// Offset.
    #[serde(default)]
    pub offset: u32,
}

/// Category with its size.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CategoryCount {
    /// Category.
    pub category: String,
    /// Commands.
    pub count: u32,
}

/// Catalogue page.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CatalogueResult {
    /// Rows.
    pub commands: Vec<CommandSummary>,
    /// Total matching rows.
    pub total: u32,
    /// Categories (all enabled sets).
    pub categories: Vec<CategoryCount>,
}

/// Sets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetsResult {
    /// Sets.
    pub sets: Vec<CommandSetSummary>,
}

/// A set id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetIdParams {
    /// Set.
    pub set_id: String,
}

/// A full set.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetResult {
    /// Set.
    pub set: CommandSet,
}

/// `computer.saveSet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SaveSetParams {
    /// Set (user sets only).
    pub set: CommandSet,
}

/// `computer.setEnabled`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetEnabledParams {
    /// Set.
    pub set_id: String,
    /// Enabled.
    pub enabled: bool,
}

/// `computer.exportSet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ExportSetResult {
    /// Suggested file name.
    pub file_name: String,
    /// JSON document (`{"format":"orkestr-command-set","version":1,"set":{…}}`).
    pub json: String,
}

/// `computer.importSet`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ImportSetParams {
    /// JSON document (export format or a bare set).
    pub json: String,
    /// Replace a set with the same id (else a new id is chosen).
    #[serde(default)]
    pub replace: bool,
}

/// Windows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WindowsResult {
    /// Windows (z-order).
    pub windows: Vec<WindowInfo>,
}

/// Processes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ProcessesResult {
    /// Processes (by memory).
    pub processes: Vec<ProcessInfo>,
}

/// `computer.stop`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StopResult {
    /// Runs stopped.
    pub runs_stopped: u32,
    /// Pending confirmations cancelled.
    pub pending_cancelled: u32,
    /// Timers cancelled.
    pub timers_cancelled: u32,
}

/// `computer.history`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryParams {
    /// Max entries (default 20).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// History.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HistoryResult {
    /// Newest first.
    pub entries: Vec<HistoryEntry>,
}

method!(/// Backend availability and catalogue size.
    Status, "computer.status", Empty, ComputerStatus);
method!(/// Match + run (or plan) an utterance.
    Handle, "computer.handle", HandleParams, HandleResult);
method!(/// Matches only (quick search, no side effects).
    Match, "computer.match", MatchParams, MatchResult);
method!(/// Runs a command by id.
    Run, "computer.run", RunComputerCommandParams, HandleResult);
method!(/// Runs ad-hoc steps (editor test).
    Execute, "computer.execute", ExecuteParams, HandleResult);
method!(/// Approves / rejects a pending run.
    Confirm, "computer.confirm", ConfirmParams, HandleResult);
method!(/// Asks the AI for a plan (or a corrected plan).
    Plan, "computer.plan", PlanParams, PlanResult);
method!(/// Runs an approved plan.
    ApprovePlan, "computer.approvePlan", PlanIdParams, HandleResult);
method!(/// Saves a plan as a user command.
    RememberPlan, "computer.rememberPlan", RememberPlanParams, CommandResult);
method!(/// Searches the catalogue and user sets.
    Catalogue, "computer.catalogue", CatalogueParams, CatalogueResult);
method!(/// Command sets.
    ListSets, "computer.listSets", Empty, SetsResult);
method!(/// One set with its commands.
    GetSet, "computer.getSet", SetIdParams, SetResult);
method!(/// Creates / replaces a user set.
    SaveSet, "computer.saveSet", SaveSetParams, SetResult);
method!(/// Deletes a user set.
    DeleteSet, "computer.deleteSet", SetIdParams, Empty);
method!(/// Enables / disables a set.
    SetEnabled, "computer.setEnabled", SetEnabledParams, SetResult);
method!(/// Exports a set as JSON.
    ExportSet, "computer.exportSet", SetIdParams, ExportSetResult);
method!(/// Imports a set from JSON.
    ImportSet, "computer.importSet", ImportSetParams, SetResult);
method!(/// Open windows.
    Windows, "computer.windows", Empty, WindowsResult);
method!(/// Running processes.
    Processes, "computer.processes", Empty, ProcessesResult);
method!(/// Global stop: cancels runs, pending confirmations and timers.
    Stop, "computer.stop", Empty, StopResult);
method!(/// Recent commands.
    History, "computer.history", HistoryParams, HistoryResult);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn actions_round_trip_with_slot_numbers() {
        let a = ComputerAction::Scroll {
            direction: ScrollDirection::Down,
            amount: Some(SlotNum::Text("{n}".into())),
            unit: ScrollUnit::Lines,
        };
        let v = serde_json::to_value(&a).unwrap();
        assert_eq!(v["type"], "Scroll");
        assert_eq!(v["amount"], "{n}");
        let back: ComputerAction = serde_json::from_value(v).unwrap();
        assert_eq!(back, a);
        let n: ComputerAction = serde_json::from_str(r#"{"type":"Wait","ms":500}"#).unwrap();
        assert_eq!(n, ComputerAction::Wait { ms: SlotNum::Num(500) });
        let e: ComputerAction = serde_json::from_str(r#"{"type":"ClipboardGet"}"#).unwrap();
        assert_eq!(e.type_name(), "ClipboardGet");
    }

    #[test]
    fn localized_text_falls_back() {
        let t = LocalizedText { en: "Open".into(), ru: String::new() };
        assert_eq!(t.get("ru"), "Open");
        assert_eq!(t.get("en"), "Open");
    }
}
