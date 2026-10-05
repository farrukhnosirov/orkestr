//! Seams to other areas: review state (implemented by the review area) and
//! security findings. Defaults are conservative: unknown.

use async_trait::async_trait;
use orkestr_domain::{ProjectId, ReviewState, Task};

/// Review verdict as seen by the gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewStatus {
    /// Reviewer approved.
    Approved,
    /// Reviewer requested changes.
    ChangesRequested,
    /// Reviewer rejected.
    Rejected,
    /// Review in progress.
    Pending,
    /// No information.
    Unknown,
}

/// Security findings summary.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityStatus {
    /// Scanned, no critical findings.
    Clean,
    /// Critical findings.
    Critical {
        /// Number of critical findings.
        count: u32,
    },
    /// No scan information.
    Unknown,
}

/// Provides the review verdict for a task (the review area plugs its own in).
#[async_trait]
pub trait ReviewStatusSource: Send + Sync {
    /// Review verdict (project-level evaluations pass `None`).
    async fn review_status(&self, project_id: ProjectId, task: Option<&Task>) -> ReviewStatus;
}

/// Provides security findings.
#[async_trait]
pub trait SecurityFindingsSource: Send + Sync {
    /// Findings summary.
    async fn security_status(&self, project_id: ProjectId, task: Option<&Task>) -> SecurityStatus;
}

/// Default review source: the task's own `reviewState` field; `NotRequired`
/// and project-level evaluations are `Unknown`.
#[derive(Debug, Default, Clone, Copy)]
pub struct TaskFieldReview;

#[async_trait]
impl ReviewStatusSource for TaskFieldReview {
    async fn review_status(&self, _project_id: ProjectId, task: Option<&Task>) -> ReviewStatus {
        match task.map(|t| t.review_state) {
            Some(ReviewState::Approved) => ReviewStatus::Approved,
            Some(ReviewState::ChangesRequested) => ReviewStatus::ChangesRequested,
            Some(ReviewState::Rejected) => ReviewStatus::Rejected,
            Some(ReviewState::Pending) => ReviewStatus::Pending,
            Some(ReviewState::NotRequired) | None => ReviewStatus::Unknown,
        }
    }
}

/// Default security source: no scanner wired yet → `Unknown`.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoSecurityScanner;

#[async_trait]
impl SecurityFindingsSource for NoSecurityScanner {
    async fn security_status(&self, _project_id: ProjectId, _task: Option<&Task>) -> SecurityStatus {
        SecurityStatus::Unknown
    }
}
