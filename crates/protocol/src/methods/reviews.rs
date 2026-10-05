//! `reviews.*` — automatic code review and adversarial review of a task's
//! changes (§8, §17, §68.12). Phase 2.
//!
//! * **Automatic review**: a Reviewer model reads the task's diff (agent
//!   branch vs. its base), the acceptance criteria and project context and
//!   returns a structured [`Review`]: verdict, findings by severity with file
//!   and line, and an acceptance-criteria checklist. A task in `Review` then
//!   moves to `Testing` (approved), `Running` (changes requested) or `Failed`
//!   (rejected) through the task engine.
//! * **Adversarial review**: a separate model — preferably a different
//!   provider/model than the author's — is told to refute the change
//!   (security, architecture, races, data loss, performance, edge cases,
//!   wrong assumptions, UX). Each finding is then verified: the author model
//!   concedes or rebuts it with evidence. Unresolved critical findings block
//!   the quality gate ([`ReviewGate`]).
//!
//! Only visible, structured output is stored — never hidden chain-of-thought.
//!
//! Events (topic `project:<id>` and `task:<id>`):
//! * `ReviewUpdated` — a review was created or changed (progress, findings);
//! * `ReviewFinished` — a review reached its verdict (contract minimum kind).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ModelRef, ProjectId, ReviewId, TaskId, TaskStatus, Timestamp, UsageSummary};

use super::method;

/// Max findings kept per review.
pub const MAX_FINDINGS: usize = 60;
/// Max characters of a finding text field.
pub const MAX_FINDING_TEXT: usize = 4_000;

/// Kind of review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReviewKind {
    /// Automatic code review by the Reviewer model.
    Standard,
    /// Adversarial review: a separate model tries to refute the change.
    Adversarial,
}

/// Lifecycle of a review.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReviewStatus {
    /// The model is reviewing.
    Running,
    /// Adversarial findings are being verified by the author model.
    Verifying,
    /// Finished with a verdict.
    Finished,
    /// The review could not be completed (no model, invalid output, cancelled).
    Failed,
}

impl ReviewStatus {
    /// Whether the review is over.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Finished | Self::Failed)
    }
}

/// Reviewer verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ReviewVerdict {
    /// Ready for testing.
    Approve,
    /// Fix the findings and resubmit.
    RequestChanges,
    /// The approach is wrong; the task failed.
    Reject,
}

/// Severity of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum FindingSeverity {
    /// Must be fixed before merging (blocks the gate while unresolved).
    Critical,
    /// Serious problem.
    High,
    /// Should be fixed.
    Medium,
    /// Minor.
    Low,
    /// Informational note.
    Info,
}

/// Status of a finding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum FindingStatus {
    /// Reported, not verified or resolved yet.
    Open,
    /// The author model conceded the finding (verified: it is real).
    Confirmed,
    /// The author model rebutted the finding with evidence.
    Rebutted,
    /// Marked fixed (by the user or a later attempt).
    Fixed,
    /// Dismissed by the user (with a note).
    Dismissed,
}

impl FindingStatus {
    /// Whether the finding still needs action.
    #[must_use]
    pub fn is_unresolved(self) -> bool {
        matches!(self, Self::Open | Self::Confirmed)
    }
}

/// One finding of a review.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewFinding {
    /// Stable id within the review (`f1`, `f2`, …).
    pub id: String,
    /// Severity.
    pub severity: FindingSeverity,
    /// Category (`security`, `correctness`, `architecture`, `concurrency`,
    /// `data-loss`, `performance`, `edge-case`, `assumption`, `ux`, `style`, `tests`, …).
    pub category: String,
    /// File (relative path), when the finding is about one.
    pub file: Option<String>,
    /// Line in the new version of the file.
    pub line: Option<u32>,
    /// One-line title.
    pub title: String,
    /// Explanation.
    pub detail: String,
    /// Suggested fix.
    pub suggestion: String,
    /// Evidence (quoted code, reasoning summary) — required for adversarial findings.
    #[serde(default)]
    pub evidence: String,
    /// Status.
    pub status: FindingStatus,
    /// The author model's answer in the verification pass (concession or rebuttal).
    #[serde(default)]
    pub response: Option<String>,
    /// Note of the user who resolved or dismissed it.
    #[serde(default)]
    pub resolution_note: Option<String>,
    /// When it was resolved.
    #[serde(default)]
    pub resolved_at: Option<Timestamp>,
}

impl ReviewFinding {
    /// Whether this finding blocks the quality gate: an unresolved critical finding.
    #[must_use]
    pub fn is_blocking(&self) -> bool {
        self.severity == FindingSeverity::Critical && self.status.is_unresolved()
    }

    /// Whether this is a security finding.
    #[must_use]
    pub fn is_security(&self) -> bool {
        let c = self.category.to_ascii_lowercase();
        c.contains("security") || c.contains("secret") || c.contains("injection") || c.contains("auth")
    }
}

/// One acceptance criterion as checked by the reviewer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AcCheck {
    /// Criterion text.
    pub criterion: String,
    /// Whether the change meets it.
    pub met: bool,
    /// Evidence (files, tests, lines).
    pub evidence: String,
}

/// Size of the reviewed change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewDiffStats {
    /// Files changed.
    pub files: u32,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// The diff given to the model was cut at the size budget.
    pub truncated: bool,
    /// What was compared (`main...agent/x`, `uncommitted changes`, …).
    pub source: String,
}

/// A review of a task's changes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct Review {
    /// Id.
    pub id: ReviewId,
    /// Project.
    pub project_id: ProjectId,
    /// Task under review.
    pub task_id: TaskId,
    /// Kind.
    pub kind: ReviewKind,
    /// Status.
    pub status: ReviewStatus,
    /// Verdict (when finished).
    pub verdict: Option<ReviewVerdict>,
    /// Summary (visible output only).
    pub summary: String,
    /// Findings.
    pub findings: Vec<ReviewFinding>,
    /// Acceptance-criteria checklist.
    pub ac_check: Vec<AcCheck>,
    /// Model that reviewed.
    pub model: Option<ModelRef>,
    /// Model that wrote the change (author), when known.
    pub author_model: Option<ModelRef>,
    /// Adversarial review only: no different model was available, so the
    /// author's model reviewed with the adversarial prompt.
    #[serde(default)]
    pub same_model_fallback: bool,
    /// Model that answered the verification pass (adversarial).
    #[serde(default)]
    pub verifier_model: Option<ModelRef>,
    /// The reviewed change.
    pub diff: ReviewDiffStats,
    /// Task status the review moved the task to (if it did).
    pub transitioned_to: Option<TaskStatus>,
    /// Review run together with this one (standard ↔ adversarial).
    #[serde(default)]
    pub linked_review_id: Option<ReviewId>,
    /// Token usage + cost of the review calls.
    pub usage: UsageSummary,
    /// Failure reason (status `Failed`).
    pub error: Option<String>,
    /// Created.
    pub created_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// Finished.
    pub finished_at: Option<Timestamp>,
}

impl Review {
    /// Unresolved critical findings.
    #[must_use]
    pub fn blocking(&self) -> Vec<&ReviewFinding> {
        self.findings.iter().filter(|f| f.is_blocking()).collect()
    }
}

/// Quality-gate view of a task's reviews (read by the quality pipeline:
/// gates "reviewer approved" and "no critical security finding").
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewGate {
    /// Task.
    pub task_id: Option<TaskId>,
    /// The latest finished standard review approved the change.
    pub reviewer_approved: bool,
    /// No unresolved critical security finding in the latest reviews.
    pub no_critical_security: bool,
    /// Unresolved critical findings (any category) in the latest reviews.
    pub blocking_findings: u32,
    /// Latest standard review.
    pub latest_review_id: Option<ReviewId>,
    /// Latest adversarial review.
    pub latest_adversarial_id: Option<ReviewId>,
    /// The gate passes: approved and nothing blocking.
    pub passed: bool,
    /// Why it does not pass (one line per reason).
    pub reasons: Vec<String>,
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

/// Params of `reviews.request`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RequestReviewParams {
    /// Task to review.
    pub task_id: TaskId,
    /// Reviewer model (default: routing for purpose `Review`, then the AI Manager model).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Also run an adversarial review (default: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub adversarial: Option<bool>,
    /// Move a task in `Review` according to the verdict (default: true).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub transition: Option<bool>,
    /// Wait for the verdict before answering (default: false — progress arrives as events).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait: Option<bool>,
}

/// Params of `reviews.adversarial`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AdversarialReviewParams {
    /// Task to attack.
    pub task_id: TaskId,
    /// Adversary model (default: a model different from the author's, chosen by routing).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// Wait for the result before answering (default: false).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wait: Option<bool>,
}

/// Result of `reviews.request` / `reviews.adversarial` / `reviews.get` / `reviews.resolveFinding`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ReviewResult {
    /// The review (status `Running` unless `wait` was set).
    pub review: Review,
    /// The adversarial review started together with it.
    #[serde(default)]
    pub adversarial: Option<Review>,
}

/// Params of `reviews.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListReviewsParams {
    /// Project filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub project_id: Option<ProjectId>,
    /// Task filter (the result then includes the task's gate).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Max reviews (default 50, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `reviews.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListReviewsResult {
    /// Reviews, newest first.
    pub reviews: Vec<Review>,
    /// Gate of the task (when `taskId` was given).
    pub gate: Option<ReviewGate>,
}

/// Params of `reviews.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetReviewParams {
    /// Review.
    pub review_id: ReviewId,
}

/// Params of `reviews.resolveFinding`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ResolveFindingParams {
    /// Review.
    pub review_id: ReviewId,
    /// Finding id within the review.
    pub finding_id: String,
    /// New status: `Fixed`, `Dismissed` or `Open` (reopen).
    pub status: FindingStatus,
    /// Note (required to dismiss a critical finding).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
}

method!(
    /// `reviews.request` — automatic code review of a task (optionally + adversarial review).
    Request, "reviews.request", RequestReviewParams, ReviewResult
);
method!(
    /// `reviews.list` — reviews of a project or task (+ the task's quality-gate view).
    List, "reviews.list", ListReviewsParams, ListReviewsResult
);
method!(
    /// `reviews.get` — one review.
    Get, "reviews.get", GetReviewParams, ReviewResult
);
method!(
    /// `reviews.resolveFinding` — mark a finding fixed / dismissed / open again.
    ResolveFinding, "reviews.resolveFinding", ResolveFindingParams, ReviewResult
);
method!(
    /// `reviews.adversarial` — adversarial review by a different model, with verification.
    Adversarial, "reviews.adversarial", AdversarialReviewParams, ReviewResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocking_and_security() {
        let mut f = ReviewFinding {
            id: "f1".into(),
            severity: FindingSeverity::Critical,
            category: "Security".into(),
            file: None,
            line: None,
            title: "t".into(),
            detail: String::new(),
            suggestion: String::new(),
            evidence: String::new(),
            status: FindingStatus::Open,
            response: None,
            resolution_note: None,
            resolved_at: None,
        };
        assert!(f.is_blocking() && f.is_security());
        f.status = FindingStatus::Rebutted;
        assert!(!f.is_blocking());
        f.status = FindingStatus::Confirmed;
        assert!(f.is_blocking());
        f.severity = FindingSeverity::High;
        assert!(!f.is_blocking());
    }

    #[test]
    fn params_defaults() {
        let p: RequestReviewParams =
            serde_json::from_str(r#"{"taskId":"0190f0b0-0000-7000-8000-000000000001"}"#).unwrap();
        assert!(p.adversarial.is_none() && p.wait.is_none());
        assert_eq!(serde_json::to_value(FindingSeverity::Critical).unwrap(), "Critical");
    }
}
