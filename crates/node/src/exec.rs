//! Worker-side job execution (Phase 6, node-protocol.md §6–7). **Owner: WP-C.**
//!
//! The worker core receives `jobs.offer`, verifies the signed policy snapshot,
//! materializes the job worktree under `ORKESTR_HOME/jobs/<id>`, runs the
//! command (tool shims apply) or agent, streams `JobProgress`, and checkpoints
//! at safe points / on `jobs.drain`. Every call is fenced by the lease epoch.

use async_trait::async_trait;
use orkestr_domain::{JobId, LeaseEpoch};
use orkestr_protocol::methods::jobs::{JobCheckpoint, JobOffer, JobProgressPayload};
use tokio::sync::{mpsc, watch};

/// Job errors.
#[derive(Debug, thiserror::Error)]
pub enum JobError {
    /// Unknown job.
    #[error("job {0} not found")]
    NotFound(JobId),
    /// Epoch is not the current one for the job (fencing, SR-81).
    #[error("stale epoch {got} for job {job} (current {current})")]
    StaleEpoch {
        /// Job.
        job: JobId,
        /// Epoch sent.
        got: LeaseEpoch,
        /// Current epoch.
        current: LeaseEpoch,
    },
    /// Offer rejected (capacity, trust, policy signature).
    #[error("offer rejected: {0}")]
    Rejected(String),
    /// Execution failure.
    #[error("job failed: {0}")]
    Failed(String),
    /// Explicit stub marker.
    #[error("unimplemented-feature: {0}")]
    Unimplemented(&'static str),
}

/// Control handle given to an executor for one job.
#[derive(Debug, Clone)]
pub struct JobControl {
    /// Progress sink (published as `JobProgress` events on topic `jobs`).
    pub progress: mpsc::UnboundedSender<JobProgressPayload>,
    /// Flips to `true` when the job must stop (cancel / emergency stop).
    pub cancelled: watch::Receiver<bool>,
}

/// Runs jobs on the worker.
#[async_trait]
pub trait JobExecutor: Send + Sync {
    /// Starts an accepted offer (returns once started; progress via `ctl`).
    async fn start(&self, offer: JobOffer, ctl: JobControl) -> Result<(), JobError>;
    /// Runs to the next safe point, stops, and returns the final checkpoint.
    async fn drain(&self, job: JobId, epoch: LeaseEpoch) -> Result<JobCheckpoint, JobError>;
    /// Cancels a job (`immediate` kills instead of stopping at a safe point).
    async fn cancel(&self, job: JobId, epoch: LeaseEpoch, immediate: bool) -> Result<(), JobError>;
}
