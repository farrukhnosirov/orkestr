//! `system.*` — health and information about the running core.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{Arch, NodeId, OsKind, Timestamp};

use super::{method, Empty};

/// Result of `system.health` (same data as `GET /health`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealthResult {
    /// Always `"ok"` when the core answers.
    pub status: String,
    /// Core version.
    pub version: String,
    /// Uptime in milliseconds.
    pub uptime_ms: u64,
}

/// Result of `system.info`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemInfo {
    /// Core version.
    pub version: String,
    /// Wire protocol version.
    pub protocol_version: u32,
    /// Id of the local node.
    pub node_id: NodeId,
    /// OS family.
    pub os: OsKind,
    /// CPU architecture.
    pub arch: Arch,
    /// Host name.
    pub hostname: String,
    /// Core process id.
    pub pid: u32,
    /// Data directory.
    pub data_dir: String,
    /// Start time.
    pub started_at: Timestamp,
    /// Whether the previous session ended uncleanly (crash recovery ran).
    pub recovered_from_crash: bool,
    /// Detected shells (display names), for the terminal panel.
    pub shells: Vec<String>,
}

/// Outcome of one system check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub enum CheckStatus {
    /// Ready.
    Ok,
    /// Usable with limitations.
    Warning,
    /// Missing or broken.
    Error,
    /// Not applicable on this OS / not checked.
    Skipped,
}

/// One environment check for onboarding (§55 steps 7–8) and Settings → System.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemCheck {
    /// Stable id: `git`, `shell`, `dataDir`, `webview`, `keyring`.
    pub id: String,
    /// Display title.
    pub title: String,
    /// Outcome.
    pub status: CheckStatus,
    /// One-line human summary ("git 2.45.1 at C:\Program Files\Git\cmd\git.exe").
    pub detail: String,
    /// What the user can do about a warning/error.
    pub hint: Option<String>,
    /// Detected version, when relevant.
    pub version: Option<String>,
}

/// Result of `system.checks`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SystemChecksResult {
    /// Checks in display order.
    pub checks: Vec<SystemCheck>,
    /// Time the checks took.
    pub duration_ms: u64,
}

/// Params of `system.resumeSummary`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResumeSummaryParams {
    /// Restrict to one project.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<orkestr_domain::ProjectId>,
    /// Count events since this time. Default: start of the previous core session.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub since: Option<Timestamp>,
}

/// Number of durable events of one kind.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct EventKindCount {
    /// Event kind (`TaskStatusChanged`, …).
    pub kind: String,
    /// Count.
    pub count: u64,
}

/// Result of `system.resumeSummary` (§57 Session Resume, RR-G9).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResumeSummary {
    /// Window start actually used (`None` = first session ever, everything counted).
    pub since: Option<Timestamp>,
    /// Start of the previous core session, if any.
    pub previous_session_started_at: Option<Timestamp>,
    /// The previous session ended uncleanly.
    pub recovered_from_crash: bool,
    /// Durable events in the window by kind, most frequent first.
    pub counts: Vec<EventKindCount>,
    /// Sum of `counts`.
    pub total_events: u64,
    /// The most recent noteworthy events in the window (failures, approvals,
    /// completions, notifications), newest last; at most 8.
    pub highlights: Vec<crate::event::Event>,
}

method!(
    /// `system.health`.
    Health, "system.health", Empty, HealthResult
);
method!(
    /// `system.checks`.
    Checks, "system.checks", Empty, SystemChecksResult
);
method!(
    /// `system.resumeSummary`.
    ResumeSummaryMethod, "system.resumeSummary", ResumeSummaryParams, ResumeSummary
);
method!(
    /// `system.info`.
    Info, "system.info", Empty, SystemInfo
);
