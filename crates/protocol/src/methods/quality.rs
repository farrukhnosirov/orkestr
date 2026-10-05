//! `quality.*` — toolchain detection, check runner and quality gates (§17, §58).
//!
//! * **Detection** reads project manifests (Cargo, npm/pnpm/yarn, Python, Go,
//!   .NET, Maven/Gradle, Makefile) — in the root and nested package roots
//!   (monorepos) — and proposes build/test/lint/typecheck/format/E2E/smoke
//!   commands.
//! * The **quality profile** is the per-project list of checks; user overrides
//!   are persisted (`quality.setProfile`), otherwise detection is used.
//! * **Runs** execute checks in the project root, an agent worktree or a
//!   task's worktree through the terminal run API — every command passes the
//!   security `CommandGate` — with per-check timeouts, an overall time budget
//!   and cancellation. Output is parsed into structured results
//!   (`CheckResult`). `BuildStarted`/`BuildSucceeded`/`BuildFailed`/
//!   `TestRunFinished` events are published with project/task/agent scopes.
//! * **Gates** (§58) are configurable per project; `quality.evaluate` combines
//!   the latest check results with review state, security findings,
//!   acceptance criteria and the Definition of Done checklist.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, TaskId, Timestamp};

use super::method;

/// Default per-check timeout (seconds).
pub const DEFAULT_CHECK_TIMEOUT_SECS: u32 = 10 * 60;
/// Max per-check timeout (seconds).
pub const MAX_CHECK_TIMEOUT_SECS: u32 = 60 * 60;
/// Default overall time budget of one run (seconds).
pub const DEFAULT_RUN_BUDGET_SECS: u32 = 30 * 60;
/// Max overall time budget of one run (seconds).
pub const MAX_RUN_BUDGET_SECS: u32 = 2 * 60 * 60;

// ---------------------------------------------------------------------------
// Checks & profile
// ---------------------------------------------------------------------------

/// What a check verifies.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CheckKind {
    /// Compile / bundle.
    Build,
    /// Static type checking.
    Typecheck,
    /// Linters.
    Lint,
    /// Formatting check (no writes).
    Format,
    /// Unit / integration tests.
    Test,
    /// End-to-end tests.
    E2e,
    /// Smoke tests.
    Smoke,
}

impl CheckKind {
    /// All kinds in execution order.
    pub const ALL: [CheckKind; 7] = [
        CheckKind::Build,
        CheckKind::Typecheck,
        CheckKind::Lint,
        CheckKind::Format,
        CheckKind::Test,
        CheckKind::E2e,
        CheckKind::Smoke,
    ];

    /// Stable lowercase name.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Build => "build",
            Self::Typecheck => "typecheck",
            Self::Lint => "lint",
            Self::Format => "format",
            Self::Test => "test",
            Self::E2e => "e2e",
            Self::Smoke => "smoke",
        }
    }

    /// Whether results of this kind are test counts (vs. diagnostics).
    #[must_use]
    pub fn is_test(self) -> bool {
        matches!(self, Self::Test | Self::E2e | Self::Smoke)
    }
}

/// Toolchain a check belongs to (drives output parsing).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum Toolchain {
    /// Rust / Cargo.
    Cargo,
    /// npm.
    Npm,
    /// pnpm.
    Pnpm,
    /// Yarn.
    Yarn,
    /// Python (pytest, ruff, flake8, mypy, black).
    Python,
    /// Go.
    Go,
    /// .NET.
    Dotnet,
    /// Maven.
    Maven,
    /// Gradle.
    Gradle,
    /// Makefile targets.
    Make,
    /// User-defined command.
    Custom,
}

/// Where a check definition came from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum CheckSource {
    /// Auto-detected from manifests.
    Detected,
    /// Added or edited by the user.
    User,
}

/// One configured check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckDef {
    /// Stable id within the profile (e.g. `cargo:test:.`).
    pub id: String,
    /// Kind.
    pub kind: CheckKind,
    /// Toolchain (parser selection).
    pub toolchain: Toolchain,
    /// Display label, e.g. `cargo test`.
    pub label: String,
    /// Shell command line.
    pub command: String,
    /// Working directory relative to the workspace root (`.` = root, `/`-separated).
    #[serde(default = "dot")]
    pub cwd: String,
    /// Whether the check runs.
    #[serde(default = "yes")]
    pub enabled: bool,
    /// Per-check timeout in seconds (default 600, max 3600).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub timeout_secs: Option<u32>,
    /// Origin.
    #[serde(default = "user_source")]
    pub source: CheckSource,
}

fn dot() -> String {
    ".".into()
}
fn yes() -> bool {
    true
}
fn user_source() -> CheckSource {
    CheckSource::User
}

/// A detected project root (monorepos have several).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DetectedRoot {
    /// Path relative to the workspace root (`.` = root).
    pub path: String,
    /// Toolchains found there.
    pub toolchains: Vec<Toolchain>,
    /// Manifest files that triggered detection (relative paths).
    pub manifests: Vec<String>,
}

/// Result of toolchain detection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Detection {
    /// Detected roots, root first.
    pub roots: Vec<DetectedRoot>,
    /// Proposed checks.
    pub checks: Vec<CheckDef>,
    /// Non-fatal notes (e.g. unreadable manifests).
    pub notes: Vec<String>,
}

/// Per-project quality profile.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QualityProfile {
    /// Project.
    pub project_id: ProjectId,
    /// Checks (in display order; runs execute in kind order).
    pub checks: Vec<CheckDef>,
    /// `true` when the user saved overrides; `false` = live detection.
    pub customized: bool,
    /// Last save time (customized profiles).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
    /// Detected roots (only for detection-backed profiles).
    #[serde(default)]
    pub roots: Vec<DetectedRoot>,
}

/// Params of `quality.detect` / `quality.getProfile` / `quality.gates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QualityProjectParams {
    /// Project.
    pub project_id: ProjectId,
}

/// Params of `quality.setProfile`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetProfileParams {
    /// Project.
    pub project_id: ProjectId,
    /// New checks (ignored with `reset`).
    #[serde(default)]
    pub checks: Vec<CheckDef>,
    /// Drop overrides and go back to live detection.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset: Option<bool>,
}

// ---------------------------------------------------------------------------
// Runs & results
// ---------------------------------------------------------------------------

/// Where checks run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(export)]
pub enum RunTarget {
    /// The project root.
    Project,
    /// A worktree of the project's repository (absolute path).
    Worktree {
        /// Absolute worktree path.
        path: String,
    },
    /// The task's worktree (or the project root when it has none).
    Task {
        /// Task.
        #[serde(rename = "taskId")]
        task_id: TaskId,
    },
}

/// Status of a run or check (TS: `QualityCheckStatus` — `CheckStatus` is the
/// system-check status of `methods::system`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export, rename = "QualityCheckStatus")]
pub enum CheckStatus {
    /// Waiting to start.
    Queued,
    /// Running.
    Running,
    /// Succeeded.
    Passed,
    /// Failed (non-zero exit / failures).
    Failed,
    /// Killed by the timeout or the run's time budget.
    TimedOut,
    /// Cancelled by the user.
    Cancelled,
    /// Not run (prerequisite failed / budget exhausted / disabled).
    Skipped,
    /// Blocked by the security gate (denied or awaiting approval).
    Blocked,
    /// Could not be executed (spawn failure).
    Error,
}

impl CheckStatus {
    /// Whether this status is final.
    #[must_use]
    pub fn is_final(self) -> bool {
        !matches!(self, Self::Queued | Self::Running)
    }
}

/// Severity of a diagnostic.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DiagnosticSeverity {
    /// Error.
    Error,
    /// Warning.
    Warning,
    /// Informational.
    Info,
}

/// A compiler/linter diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Diagnostic {
    /// Severity.
    pub severity: DiagnosticSeverity,
    /// Message.
    pub message: String,
    /// File (relative to the check's cwd when the tool printed it so).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<String>,
    /// 1-based line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line: Option<u32>,
    /// 1-based column.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub column: Option<u32>,
    /// Rule / error code (`E0308`, `TS2322`, `no-unused-vars`, …).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub code: Option<String>,
}

/// A failing test.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TestFailure {
    /// Test name.
    pub name: String,
    /// Failure message (trimmed).
    pub message: String,
    /// File.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub file: Option<String>,
    /// 1-based line.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub line: Option<u32>,
}

/// Structured result of one check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckResult {
    /// Check id (from the profile).
    pub check_id: String,
    /// Kind.
    pub kind: CheckKind,
    /// Toolchain.
    pub toolchain: Toolchain,
    /// Label.
    pub label: String,
    /// Command line.
    pub command: String,
    /// Working directory (relative to the workspace).
    pub cwd: String,
    /// Status.
    pub status: CheckStatus,
    /// Exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit_code: Option<i32>,
    /// Duration in ms.
    pub duration_ms: u64,
    /// Passed tests (test kinds).
    pub passed: u32,
    /// Failed tests.
    pub failed: u32,
    /// Skipped / ignored tests.
    pub skipped: u32,
    /// Failing tests.
    pub failures: Vec<TestFailure>,
    /// Diagnostics (errors first, capped).
    pub diagnostics: Vec<Diagnostic>,
    /// Last part of the combined output (redacted).
    pub raw_tail: String,
    /// One-line summary.
    pub summary: String,
    /// Approval request id when the command awaits approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<String>,
    /// Start time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at: Option<Timestamp>,
}

/// One quality run (a set of checks in one workspace).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct QualityRun {
    /// Run id (UUID v7).
    pub id: String,
    /// Project.
    pub project_id: ProjectId,
    /// Task scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Agent scope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Target as requested.
    pub target: RunTarget,
    /// Absolute workspace directory the checks ran in.
    pub workspace: String,
    /// Kinds requested (empty = all enabled).
    pub kinds: Vec<CheckKind>,
    /// Overall status.
    pub status: CheckStatus,
    /// Results in execution order.
    pub results: Vec<CheckResult>,
    /// Start time.
    pub started_at: Timestamp,
    /// End time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
    /// Total duration in ms (final runs).
    pub duration_ms: u64,
}

/// Params of `quality.run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunChecksParams {
    /// Project.
    pub project_id: ProjectId,
    /// Kinds subset (default: all enabled checks).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub kinds: Option<Vec<CheckKind>>,
    /// Specific check ids (default: all of `kinds`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub check_ids: Option<Vec<String>>,
    /// Where to run (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<RunTarget>,
    /// Agent on whose behalf the checks run (gated as that agent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Overall time budget in seconds (default 1800, max 7200).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub budget_secs: Option<u32>,
    /// Wait for completion before answering (default: return immediately).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait: Option<bool>,
}

/// Params of `quality.runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRunsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Only runs of this task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Max results (default 50, max 200), newest first.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `quality.runs`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListRunsResult {
    /// Runs, newest first.
    pub runs: Vec<QualityRun>,
}

/// Params of `quality.getRun` / `quality.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunIdParams {
    /// Run id.
    pub run_id: String,
}

/// Result of `quality.cancel`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CancelRunResult {
    /// Whether a running run was signalled.
    pub cancelled: bool,
}

// ---------------------------------------------------------------------------
// Gates
// ---------------------------------------------------------------------------

/// A quality gate (§58).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum GateKind {
    /// Build passes.
    BuildPasses,
    /// Type checking passes.
    TypecheckPasses,
    /// Lint passes.
    LintPasses,
    /// Formatting check passes.
    FormatPasses,
    /// Tests pass (unit/integration + E2E + smoke that are configured).
    TestsPass,
    /// No critical security finding.
    NoCriticalSecurity,
    /// Reviewer approved.
    ReviewerApproved,
    /// Acceptance criteria met.
    AcceptanceCriteriaMet,
}

impl GateKind {
    /// All gates in display order.
    pub const ALL: [GateKind; 8] = [
        GateKind::BuildPasses,
        GateKind::TypecheckPasses,
        GateKind::LintPasses,
        GateKind::FormatPasses,
        GateKind::TestsPass,
        GateKind::NoCriticalSecurity,
        GateKind::ReviewerApproved,
        GateKind::AcceptanceCriteriaMet,
    ];
}

/// Configuration of one gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GateSetting {
    /// Gate.
    pub gate: GateKind,
    /// Evaluated at all.
    pub enabled: bool,
    /// Blocks completion when failing (otherwise advisory).
    #[serde(default = "yes")]
    pub required: bool,
}

/// Definition of Done item (§58).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum DodItem {
    /// Code changed.
    Code,
    /// Tests added/updated and passing.
    Tests,
    /// Docs updated.
    Docs,
    /// Migrations, if needed.
    Migrations,
    /// Rollback possible where relevant (snapshot exists).
    Rollback,
    /// Reviewed.
    Review,
    /// Acceptance criteria met.
    AcceptanceCriteria,
}

impl DodItem {
    /// All items in display order.
    pub const ALL: [DodItem; 7] = [
        DodItem::Code,
        DodItem::Tests,
        DodItem::Docs,
        DodItem::Migrations,
        DodItem::Rollback,
        DodItem::Review,
        DodItem::AcceptanceCriteria,
    ];
}

/// Configuration of one DoD item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DodSetting {
    /// Item.
    pub item: DodItem,
    /// Part of the checklist.
    pub enabled: bool,
}

/// Per-project gate configuration.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GateConfig {
    /// Project.
    pub project_id: ProjectId,
    /// Gates (every [`GateKind`] exactly once).
    pub gates: Vec<GateSetting>,
    /// Definition of Done (every [`DodItem`] exactly once).
    pub dod: Vec<DodSetting>,
    /// Whether the user saved this configuration (else defaults).
    pub customized: bool,
    /// Last save time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
}

/// Params of `quality.setGates`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetGatesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Gate settings (missing gates keep their defaults).
    #[serde(default)]
    pub gates: Vec<GateSetting>,
    /// DoD settings (missing items keep their defaults).
    #[serde(default)]
    pub dod: Vec<DodSetting>,
    /// Restore defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset: Option<bool>,
}

/// Status of a gate / DoD item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum GateStatus {
    /// Satisfied.
    Passed,
    /// Violated.
    Failed,
    /// Not evaluated yet (no run / review pending).
    Pending,
    /// Cannot be determined automatically.
    Unknown,
    /// Disabled or not applicable.
    Skipped,
}

/// Why a gate has its status (machine-readable, localized by clients).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum GateReason {
    /// Gate disabled in settings.
    Disabled,
    /// No check of this kind is configured.
    NoCheckConfigured,
    /// No run result yet.
    NotRun,
    /// The latest result passed.
    CheckPassed,
    /// The latest result failed.
    CheckFailed,
    /// The check is running.
    CheckRunning,
    /// The check could not complete (timeout, blocked, cancelled, error).
    CheckIncomplete,
    /// Reviewer approved.
    ReviewApproved,
    /// Reviewer requested changes / rejected.
    ReviewRejected,
    /// Review in progress.
    ReviewPending,
    /// No review information.
    ReviewUnknown,
    /// No critical findings.
    SecurityClean,
    /// Critical findings exist.
    SecurityCritical,
    /// No security scan information.
    SecurityUnknown,
    /// All acceptance criteria are met.
    CriteriaMet,
    /// Some acceptance criteria are not met.
    CriteriaUnmet,
    /// The task has no acceptance criteria.
    NoCriteria,
    /// Evaluated without a task.
    NoTask,
    /// DoD: evidence found in the task's touched files / results.
    EvidenceFound,
    /// DoD: no evidence found.
    NoEvidence,
    /// DoD: not applicable to this change.
    NotApplicable,
}

/// Evaluation of one gate.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GateResult {
    /// Gate.
    pub gate: GateKind,
    /// Enabled in settings.
    pub enabled: bool,
    /// Required (blocking).
    pub required: bool,
    /// Status.
    pub status: GateStatus,
    /// Reason code.
    pub reason: GateReason,
    /// Short detail (counts, check labels).
    pub detail: String,
    /// Run the status comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<String>,
}

/// Evaluation of one DoD item.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DodResult {
    /// Item.
    pub item: DodItem,
    /// Status.
    pub status: GateStatus,
    /// Reason code.
    pub reason: GateReason,
    /// Detail.
    pub detail: String,
}

/// Combined gate verdict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GateReport {
    /// Project.
    pub project_id: ProjectId,
    /// Task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// `Passed` when every required enabled gate passed; `Failed` when any
    /// required gate failed; otherwise `Pending`.
    pub overall: GateStatus,
    /// Gates in display order.
    pub gates: Vec<GateResult>,
    /// DoD checklist.
    pub dod: Vec<DodResult>,
    /// Latest run considered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<String>,
    /// Evaluation time.
    pub evaluated_at: Timestamp,
}

/// Params of `quality.evaluate`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EvaluateParams {
    /// Project.
    pub project_id: ProjectId,
    /// Task (default: project-level evaluation of the latest project run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
}

method!(
    /// `quality.detect` — detect toolchains and propose checks.
    Detect, "quality.detect", QualityProjectParams, Detection
);
method!(
    /// `quality.getProfile` — saved overrides or live detection.
    GetProfile, "quality.getProfile", QualityProjectParams, QualityProfile
);
method!(
    /// `quality.setProfile` — save overrides (or reset to detection).
    SetProfile, "quality.setProfile", SetProfileParams, QualityProfile
);
method!(
    /// `quality.run` — run checks.
    Run, "quality.run", RunChecksParams, QualityRun
);
method!(
    /// `quality.runs` — run history.
    Runs, "quality.runs", ListRunsParams, ListRunsResult
);
method!(
    /// `quality.getRun` — one run.
    GetRun, "quality.getRun", RunIdParams, QualityRun
);
method!(
    /// `quality.cancel` — cancel a running run.
    Cancel, "quality.cancel", RunIdParams, CancelRunResult
);
method!(
    /// `quality.gates` — gate configuration.
    Gates, "quality.gates", QualityProjectParams, GateConfig
);
method!(
    /// `quality.setGates` — update gate configuration.
    SetGates, "quality.setGates", SetGatesParams, GateConfig
);
method!(
    /// `quality.evaluate` — evaluate gates + DoD.
    Evaluate, "quality.evaluate", EvaluateParams, GateReport
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_target_wire_shape() {
        let t = RunTarget::Worktree { path: "C:/w".into() };
        assert_eq!(serde_json::to_value(&t).unwrap(), serde_json::json!({"type":"worktree","path":"C:/w"}));
        let p: RunTarget = serde_json::from_value(serde_json::json!({"type":"project"})).unwrap();
        assert_eq!(p, RunTarget::Project);
        let id = TaskId::new();
        let v = serde_json::to_value(RunTarget::Task { task_id: id }).unwrap();
        assert_eq!(v["taskId"], serde_json::json!(id.to_string()));
    }

    #[test]
    fn check_def_defaults() {
        let d: CheckDef = serde_json::from_value(serde_json::json!({
            "id":"x","kind":"Test","toolchain":"Custom","label":"t","command":"make test"
        }))
        .unwrap();
        assert_eq!(d.cwd, ".");
        assert!(d.enabled);
        assert_eq!(d.source, CheckSource::User);
    }
}
