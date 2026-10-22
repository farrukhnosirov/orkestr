//! `detective.*` — Bug Detective: incident from an alert / log error / test
//! failure / manual report → evidence → root-cause hypothesis → reproduction
//! → (approval) agent fix on `detective/<id>` worktree → tests → review →
//! (approval) merge (Phase 7, ADR-0042). **Owner: WP6** (crate
//! `orkestr-detective`, handlers `core/src/rpc/detective.rs`).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, AlertId, ApprovalId, IncidentId, ProjectId, Timestamp};

use super::method;

/// Incident stage (state machine; terminal: `Resolved`, `Failed`, `Cancelled`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum IncidentStage {
    /// Created.
    Opened,
    /// Evidence collectors running.
    Collecting,
    /// Provider writes the root-cause hypothesis.
    Hypothesizing,
    /// Reproduction via the quality executor.
    Reproducing,
    /// Human gate 1: approve the agent fix.
    AwaitingFixApproval,
    /// Agent writes the fix on `detective/<id>`.
    Fixing,
    /// Tests on the fix branch.
    Testing,
    /// Code review of the fix.
    Reviewing,
    /// Human gate 2: approve the merge.
    AwaitingMergeApproval,
    /// Merged / closed as fixed.
    Resolved,
    /// Failed (budget, errors, tests never green).
    Failed,
    /// Cancelled by the user / emergency stop.
    Cancelled,
}

impl IncidentStage {
    /// Terminal stage.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Resolved | Self::Failed | Self::Cancelled)
    }
}

/// What opened the incident.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum IncidentTrigger {
    /// A monitoring alert.
    Alert {
        /// Alert.
        alert_id: AlertId,
    },
    /// An error line in a log source.
    LogError {
        /// Log source.
        source_id: String,
        /// The line (redacted).
        line: String,
    },
    /// A failing quality run.
    TestFailure {
        /// Quality run id.
        run_id: String,
    },
    /// Reported by the user.
    Manual {
        /// Description.
        description: String,
    },
}

/// Evidence kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum EvidenceKind {
    /// Log lines around the error.
    Logs,
    /// Metric series around the incident.
    Metrics,
    /// Recent deployments (deployed version).
    Deployments,
    /// Recent commits.
    Commits,
    /// Diff of recent commits.
    Diff,
    /// Failing quality checks / tests.
    QualityFailures,
    /// Code snippets from stack-trace `file:line` references.
    CodeSnippet,
    /// Parsed stack trace.
    StackTrace,
    /// The triggering alert.
    Alert,
}

/// One piece of evidence (redacted, size-capped).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Evidence {
    /// Id (`ev1`, `ev2`, ...; referenced by the hypothesis).
    pub id: String,
    /// Kind.
    pub kind: EvidenceKind,
    /// Title.
    pub title: String,
    /// Content (redacted, truncated).
    pub content: String,
    /// Reference (`file:line`, commit sha, deployment id, ...).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reference: Option<String>,
    /// Collected at.
    pub collected_at: Timestamp,
}

/// Structured root-cause hypothesis written by the provider.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Hypothesis {
    /// One-line summary.
    pub summary: String,
    /// Root cause explanation.
    pub root_cause: String,
    /// Confidence 0..1.
    pub confidence: f64,
    /// Evidence ids supporting it.
    pub evidence_refs: Vec<String>,
    /// Suspected files (`path[:line]`).
    pub suspected_files: Vec<String>,
    /// Proposed fix.
    pub proposed_fix: String,
    /// Reproduction command / test, if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reproduction: Option<String>,
}

/// Result of a reproduction or test run.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IncidentRun {
    /// Command / quality check.
    pub command: String,
    /// Exit code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub exit_code: Option<i32>,
    /// Passed.
    pub passed: bool,
    /// Output tail (redacted).
    pub output_tail: String,
    /// Finished at.
    pub finished_at: Timestamp,
}

/// An incident.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Incident {
    /// Id.
    pub id: IncidentId,
    /// Project.
    pub project_id: ProjectId,
    /// Title.
    pub title: String,
    /// Trigger.
    pub trigger: IncidentTrigger,
    /// Stage.
    pub stage: IncidentStage,
    /// Evidence.
    pub evidence: Vec<Evidence>,
    /// Hypothesis.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub hypothesis: Option<Hypothesis>,
    /// Reproduction (expected to fail before the fix).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reproduction: Option<IncidentRun>,
    /// Fix branch (`detective/<id>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fix_branch: Option<String>,
    /// Agent writing the fix.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Tests on the fix branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fix_tests: Option<IncidentRun>,
    /// Review id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub review_id: Option<String>,
    /// Pending approval (fix or merge gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
    /// Budget cap (USD).
    pub budget_usd: f64,
    /// Spent so far (USD).
    pub spent_usd: f64,
    /// Failure reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

/// `detective.open` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct OpenIncidentParams {
    /// Project.
    pub project_id: ProjectId,
    /// Trigger.
    pub trigger: IncidentTrigger,
    /// Title (default derived from the trigger).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// Budget cap (USD; default from settings).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub budget_usd: Option<f64>,
}

/// Result carrying one incident.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IncidentResult {
    /// The incident.
    pub incident: Incident,
}

/// `detective.list` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListIncidentsParams {
    /// Project (`None` = all).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Include terminal incidents.
    #[serde(default)]
    pub include_closed: bool,
    /// Max rows (newest first; default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// `detective.list` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListIncidentsResult {
    /// Incidents.
    pub incidents: Vec<Incident>,
}

/// Params naming one incident (approvals, cancel).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IncidentActionParams {
    /// Incident.
    pub incident_id: IncidentId,
    /// Note / reason (audited).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
}

/// `IncidentOpened`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IncidentOpenedPayload {
    /// The incident.
    pub incident: Incident,
}

/// `IncidentStageChanged`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct IncidentStageChangedPayload {
    /// Incident.
    pub incident_id: IncidentId,
    /// Project.
    pub project_id: ProjectId,
    /// Previous stage.
    pub from: IncidentStage,
    /// New stage.
    pub to: IncidentStage,
    /// Message (failure reason, gate hint).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
}

method!(/// Opens an incident and starts the investigation.
    Open, "detective.open", OpenIncidentParams, IncidentResult);
method!(/// Incidents.
    List, "detective.list", ListIncidentsParams, ListIncidentsResult);
method!(/// One incident.
    Get, "detective.get", IncidentActionParams, IncidentResult);
method!(/// Human gate 1: approve the agent fix.
    ApproveFix, "detective.approveFix", IncidentActionParams, IncidentResult);
method!(/// Human gate 2: approve merging the fix branch.
    ApproveMerge, "detective.approveMerge", IncidentActionParams, IncidentResult);
method!(/// Cancels an incident at any stage.
    Cancel, "detective.cancel", IncidentActionParams, IncidentResult);
