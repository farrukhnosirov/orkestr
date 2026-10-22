//! Seams to the rest of Orkestr. The detective crate never depends on
//! `orkestr-core`; the composition root (`core/src/rpc/detective.rs`)
//! implements these traits over the real services (providers, agent runtime,
//! quality executor, reviews, monitor, deploy) and installs them with
//! [`crate::Detective::install`]. Tests use in-memory fakes.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::IncidentId;
use orkestr_domain::{AgentId, AlertId, Deployment, DeploymentId, ProjectId, Timestamp};
use orkestr_protocol::event::LogLevel;
use orkestr_protocol::methods::detective::{Hypothesis, IncidentRun};
use orkestr_protocol::methods::monitor::{Alert, LogLine, MetricSeries};
use tokio_util::sync::CancellationToken;

use crate::DetectiveError;

/// Read access to the infrastructure monitor (WP5). Shaped after
/// `orkestr_monitor::MonitorService` (`alert`, `active_alerts`,
/// `logs_around`, `recent_metrics`) so the integrator's adapter is a thin
/// pass-through (`NotFound` → `None`).
#[async_trait]
pub trait MonitorReader: Send + Sync {
    /// One alert (`None` when unknown).
    async fn alert(&self, id: AlertId) -> Result<Option<Alert>, DetectiveError>;
    /// Open (unresolved) alerts of the project.
    async fn active_alerts(&self, project_id: ProjectId) -> Result<Vec<Alert>, DetectiveError>;
    /// Redacted log lines in `[at - before, at + after]` (oldest first).
    async fn logs_around(
        &self,
        at: Timestamp,
        before: Duration,
        after: Duration,
        project_id: Option<ProjectId>,
        min_level: Option<LogLevel>,
        limit: u32,
    ) -> Result<Vec<LogLine>, DetectiveError>;
    /// Metric series of the local node over the last `window`.
    async fn recent_metrics(&self, window: Duration) -> Result<Vec<MetricSeries>, DetectiveError>;
}

/// Read access to deployments (WP4). Thin adapter over
/// `orkestr_deploy::DeployService`: `recent` = `list(project, None, Some(limit))`,
/// `logs` = tail of `logs(id, 0).lines`.
#[async_trait]
pub trait DeploymentsReader: Send + Sync {
    /// Deployments of the project, newest first.
    async fn recent(&self, project_id: ProjectId, limit: u32) -> Result<Vec<Deployment>, DetectiveError>;
    /// Log lines of one deployment (redacted, at most `max_lines`, tail).
    async fn logs(&self, deployment_id: DeploymentId, max_lines: u32) -> Result<Vec<String>, DetectiveError>;
}

/// The latest quality (test) run of a project, for `QualityFailures` evidence
/// and the `TestFailure` trigger.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct QualityFailure {
    /// Run id.
    pub run_id: String,
    /// Failed check names.
    pub failed_checks: Vec<String>,
    /// Output tail (redacted).
    pub output: String,
}

/// Read access to quality runs.
#[async_trait]
pub trait QualityReader: Send + Sync {
    /// A run (`run_id`) or the latest failed run of the project (`None`).
    async fn failure(
        &self,
        project_id: ProjectId,
        run_id: Option<&str>,
    ) -> Result<Option<QualityFailure>, DetectiveError>;
}

/// Project folder lookup.
#[async_trait]
pub trait ProjectRoots: Send + Sync {
    /// Absolute root of the project's checkout (must be a git repository).
    async fn root(&self, project_id: ProjectId) -> Result<PathBuf, DetectiveError>;
}

/// One recent commit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSummary {
    /// Full sha.
    pub sha: String,
    /// Subject line.
    pub subject: String,
    /// Author name.
    pub author: String,
    /// Author date.
    pub date: String,
}

/// VCS operations ([`crate::vcs_git::GitVcs`] implements them over `orkestr-vcs`).
#[async_trait]
pub trait VcsOps: Send + Sync {
    /// Recent commits of the checked-out branch (newest first).
    async fn recent_commits(&self, root: &Path, limit: u32) -> Result<Vec<CommitSummary>, DetectiveError>;
    /// Patch text of `from..HEAD` (or of the last commit when `from` is `None`).
    async fn diff_since(&self, root: &Path, from: Option<&str>, max_bytes: usize) -> Result<String, DetectiveError>;
    /// Patch text between two revisions.
    async fn diff_range(&self, dir: &Path, from: &str, to: &str, max_bytes: usize) -> Result<String, DetectiveError>;
    /// Resolves a revision to a sha.
    async fn rev_parse(&self, dir: &Path, rev: &str) -> Result<Option<String>, DetectiveError>;
    /// Current branch of `dir`.
    async fn current_branch(&self, dir: &Path) -> Result<Option<String>, DetectiveError>;
    /// Creates branch `branch` from `base` with its own worktree; returns its path.
    async fn create_worktree(
        &self,
        project_id: ProjectId,
        root: &Path,
        branch: &str,
        base: &str,
    ) -> Result<PathBuf, DetectiveError>;
    /// Stages and commits everything in `dir` (`None` when nothing changed).
    async fn commit_all(&self, dir: &Path, message: &str) -> Result<Option<String>, DetectiveError>;
    /// Merges `source` into `target` (merge commit sha).
    async fn merge(&self, root: &Path, source: &str, target: &str, message: &str) -> Result<String, DetectiveError>;
    /// Removes a worktree created by [`VcsOps::create_worktree`] (best effort).
    async fn remove_worktree(&self, project_id: ProjectId, root: &Path, path: &Path) -> Result<(), DetectiveError>;
}

/// One structured model call.
#[derive(Debug, Clone)]
pub struct ModelRequest {
    /// Project (privacy policy, usage attribution).
    pub project_id: ProjectId,
    /// System prompt.
    pub system: String,
    /// Conversation (`("user" | "assistant", text)`).
    pub messages: Vec<(String, String)>,
    /// Output JSON schema.
    pub schema: serde_json::Value,
}

/// A model answer.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ModelReply {
    /// Visible text.
    pub text: String,
    /// Spend of this call (USD).
    pub cost_usd: f64,
}

/// Provider access for the hypothesis.
#[async_trait]
pub trait HypothesisModel: Send + Sync {
    /// Runs one call.
    async fn complete(&self, req: ModelRequest, cancel: CancellationToken) -> Result<ModelReply, DetectiveError>;
}

/// Everything the fix agent gets.
#[derive(Debug, Clone)]
pub struct FixRequest {
    /// Incident.
    pub incident_id: IncidentId,
    /// Project.
    pub project_id: ProjectId,
    /// Project root.
    pub project_root: PathBuf,
    /// Fix worktree (checked out on `branch`).
    pub worktree: PathBuf,
    /// `detective/<id>`.
    pub branch: String,
    /// Task title.
    pub title: String,
    /// Task (hypothesis + evidence + previous test output).
    pub prompt: String,
    /// Hypothesis.
    pub hypothesis: Hypothesis,
    /// Remaining incident budget (USD) — the agent must not exceed it.
    pub budget_usd: f64,
    /// Attempt (1-based).
    pub attempt: u32,
}

/// Fix agent result. The agent's changes are on `branch` (committed by the
/// detective when the agent left them uncommitted in the worktree).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct FixOutcome {
    /// Agent.
    pub agent_id: Option<AgentId>,
    /// Task the agent ran (reviews are task based).
    pub task_id: Option<String>,
    /// Spend (USD).
    pub cost_usd: f64,
    /// Agent summary.
    pub summary: String,
}

/// Runs an agent on the fix worktree.
#[async_trait]
pub trait FixAgent: Send + Sync {
    /// Runs to completion (or until `cancel`); `on_agent` is called as soon as
    /// the agent exists so a concurrent cancel can stop it.
    async fn run(
        &self,
        req: FixRequest,
        on_agent: Arc<dyn Fn(AgentId) + Send + Sync>,
        cancel: CancellationToken,
    ) -> Result<FixOutcome, DetectiveError>;
    /// Stops a running agent (cancel / emergency).
    async fn stop(&self, agent_id: AgentId);
}

/// One test run request.
#[derive(Debug, Clone)]
pub struct TestRequest {
    /// Project.
    pub project_id: ProjectId,
    /// Directory (project root for reproduction, fix worktree after the fix).
    pub dir: PathBuf,
    /// Explicit command (hypothesis reproduction), else the project's test checks.
    pub command: Option<String>,
}

/// Quality executor seam.
#[async_trait]
pub trait TestRunner: Send + Sync {
    /// Runs tests; must stop child processes when `cancel` fires.
    async fn run(&self, req: TestRequest, cancel: CancellationToken) -> Result<IncidentRun, DetectiveError>;
}

/// Review request.
#[derive(Debug, Clone)]
pub struct ReviewRequest {
    /// Project.
    pub project_id: ProjectId,
    /// Task of the fix agent.
    pub task_id: Option<String>,
    /// Fix worktree.
    pub worktree: PathBuf,
    /// Branch.
    pub branch: String,
    /// Diff of the fix.
    pub diff: String,
}

/// Review verdict.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ReviewOutcome {
    /// Review id.
    pub review_id: Option<String>,
    /// Approved by the reviewer.
    pub approved: bool,
    /// Summary.
    pub summary: String,
    /// Spend (USD).
    pub cost_usd: f64,
}

/// Reviews service seam.
#[async_trait]
pub trait Reviewer: Send + Sync {
    /// Reviews the fix (waits for the verdict).
    async fn review(&self, req: ReviewRequest, cancel: CancellationToken) -> Result<ReviewOutcome, DetectiveError>;
}

/// Everything the detective orchestrates. Optional readers degrade to "no
/// evidence of that kind" when absent.
#[derive(Clone)]
pub struct Ports {
    /// Project roots.
    pub projects: Arc<dyn ProjectRoots>,
    /// VCS.
    pub vcs: Arc<dyn VcsOps>,
    /// Provider.
    pub model: Arc<dyn HypothesisModel>,
    /// Fix agent.
    pub agent: Arc<dyn FixAgent>,
    /// Tests.
    pub tests: Arc<dyn TestRunner>,
    /// Reviews.
    pub reviewer: Arc<dyn Reviewer>,
    /// Monitor (WP5).
    pub monitor: Option<Arc<dyn MonitorReader>>,
    /// Deployments (WP4).
    pub deployments: Option<Arc<dyn DeploymentsReader>>,
    /// Quality runs.
    pub quality: Option<Arc<dyn QualityReader>>,
}

impl std::fmt::Debug for Ports {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Ports")
            .field("monitor", &self.monitor.is_some())
            .field("deployments", &self.deployments.is_some())
            .field("quality", &self.quality.is_some())
            .finish_non_exhaustive()
    }
}
