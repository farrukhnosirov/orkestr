//! Cross-area adapters of Phase 2 (composition root only — the engines know
//! each other through their seam traits, never directly):
//!
//! * quality gates ← review state: [`ReviewGates`] implements the quality
//!   runner's [`ReviewStatusSource`] / [`SecurityFindingsSource`] over the AI
//!   Manager's code reviews (`reviewer approved`, `no critical security
//!   finding`), falling back to the task's own review state;
//! * Merge Manager ← review state + quality runner: [`MergeGates`] answers the
//!   review gate from the reviews and the tests gate from the latest quality
//!   run of the task; [`MergeQuality`] runs the project's build / typecheck /
//!   test checks on the integration worktree before the target moves.

use std::path::Path;

use async_trait::async_trait;
use orkestr_domain::{Project, ProjectId, Task, TaskId};
use orkestr_orchestrator::{Manager, MergeGateSource, MergeQualityRunner};
use orkestr_protocol::methods::merge::{MergeGate, MergeGateStatus, MergeQualityCheck, MergeQualityReport};
use orkestr_protocol::methods::quality::{CheckKind, CheckStatus, QualityRun, RunTarget};
use orkestr_protocol::methods::reviews::ReviewGate;
use orkestr_quality::{
    QualityError, QualityService, ReviewStatus, ReviewStatusSource, SecurityFindingsSource, SecurityStatus, StartRun,
    TaskFieldReview,
};
use tokio_util::sync::CancellationToken;

/// Check kinds run on a merge's integration branch.
pub const MERGE_CHECK_KINDS: [CheckKind; 3] = [CheckKind::Build, CheckKind::Typecheck, CheckKind::Test];

/// Review gate of a task, `None` when no review has finished yet.
async fn review_gate(manager: &Manager, task: TaskId) -> Option<ReviewGate> {
    match manager.review_gate(task).await {
        Ok(g) if g.latest_review_id.is_some() || g.latest_adversarial_id.is_some() => Some(g),
        Ok(_) => None,
        Err(e) => {
            tracing::warn!(task = %task, error = %e, "review gate lookup failed");
            None
        }
    }
}

/// Review / security sources of the quality gates backed by the reviews.
#[derive(Clone)]
pub struct ReviewGates {
    manager: Manager,
}

impl ReviewGates {
    /// Over the AI Manager's reviews.
    #[must_use]
    pub fn new(manager: Manager) -> Self {
        Self { manager }
    }
}

impl std::fmt::Debug for ReviewGates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReviewGates").finish_non_exhaustive()
    }
}

#[async_trait]
impl ReviewStatusSource for ReviewGates {
    async fn review_status(&self, project_id: ProjectId, task: Option<&Task>) -> ReviewStatus {
        let Some(t) = task else { return ReviewStatus::Unknown };
        match review_gate(&self.manager, t.id).await {
            Some(g) if g.latest_review_id.is_none() => {
                // Only an adversarial review so far: the task state decides.
                TaskFieldReview.review_status(project_id, task).await
            }
            Some(g) if g.reviewer_approved => ReviewStatus::Approved,
            Some(_) => match t.review_state {
                orkestr_domain::ReviewState::Rejected => ReviewStatus::Rejected,
                _ => ReviewStatus::ChangesRequested,
            },
            None => TaskFieldReview.review_status(project_id, task).await,
        }
    }
}

#[async_trait]
impl SecurityFindingsSource for ReviewGates {
    async fn security_status(&self, _project_id: ProjectId, task: Option<&Task>) -> SecurityStatus {
        let Some(t) = task else { return SecurityStatus::Unknown };
        match review_gate(&self.manager, t.id).await {
            Some(g) if g.no_critical_security => SecurityStatus::Clean,
            Some(g) => SecurityStatus::Critical { count: g.blocking_findings.max(1) },
            None => SecurityStatus::Unknown,
        }
    }
}

/// Merge gates from the reviews and the quality runs.
#[derive(Clone)]
pub struct MergeGates {
    manager: Manager,
    quality: QualityService,
}

impl MergeGates {
    /// Over the reviews and the quality runs.
    #[must_use]
    pub fn new(manager: Manager, quality: QualityService) -> Self {
        Self { manager, quality }
    }
}

impl std::fmt::Debug for MergeGates {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeGates").finish_non_exhaustive()
    }
}

/// Tests gate of a quality run.
#[must_use]
pub fn tests_gate(run: &QualityRun) -> MergeGate {
    let tests: Vec<_> = run.results.iter().filter(|r| matches!(r.kind, CheckKind::Test | CheckKind::E2e)).collect();
    let relevant: Vec<_> = if tests.is_empty() { run.results.iter().collect() } else { tests };
    if relevant.is_empty() {
        return MergeGate::default();
    }
    let passed: u32 = relevant.iter().map(|r| r.passed).sum();
    let failed: u32 = relevant.iter().map(|r| r.failed).sum();
    let detail = format!("quality run {}: {passed} passed, {failed} failed", run.id);
    if relevant.iter().any(|r| matches!(r.status, CheckStatus::Queued | CheckStatus::Running)) {
        return MergeGate::with(MergeGateStatus::Pending, detail);
    }
    if relevant.iter().all(|r| matches!(r.status, CheckStatus::Passed)) {
        MergeGate::with(MergeGateStatus::Passed, detail)
    } else if relevant
        .iter()
        .any(|r| matches!(r.status, CheckStatus::Failed | CheckStatus::TimedOut | CheckStatus::Error))
    {
        MergeGate::with(MergeGateStatus::Failed, detail)
    } else {
        // Cancelled / skipped / blocked: not verified.
        MergeGate::with(MergeGateStatus::Unknown, detail)
    }
}

#[async_trait]
impl MergeGateSource for MergeGates {
    async fn review(&self, _project: ProjectId, task: Option<TaskId>, _branch: &str) -> MergeGate {
        let Some(task) = task else { return MergeGate::default() };
        match review_gate(&self.manager, task).await {
            Some(g) if g.passed => MergeGate::with(MergeGateStatus::Passed, "code review approved"),
            Some(g) => MergeGate::with(MergeGateStatus::Failed, g.reasons.join("; ")),
            None => MergeGate::default(),
        }
    }

    async fn tests(&self, project: ProjectId, task: Option<TaskId>, _branch: &str) -> MergeGate {
        let Some(task) = task else { return MergeGate::default() };
        match self.quality.runs(project, Some(task), 1).await {
            Ok(runs) => runs.first().map(tests_gate).unwrap_or_default(),
            Err(e) => {
                tracing::warn!(task = %task, error = %e, "quality runs lookup failed");
                MergeGate::default()
            }
        }
    }
}

/// Runs the project's quality checks on a merge's integration worktree.
#[derive(Clone)]
pub struct MergeQuality {
    quality: QualityService,
}

impl MergeQuality {
    /// Over the quality service.
    #[must_use]
    pub fn new(quality: QualityService) -> Self {
        Self { quality }
    }
}

impl std::fmt::Debug for MergeQuality {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeQuality").finish_non_exhaustive()
    }
}

/// Merge report of a finished quality run.
#[must_use]
pub fn merge_report(run: &QualityRun) -> MergeQualityReport {
    let checks: Vec<MergeQualityCheck> = run
        .results
        .iter()
        .filter(|r| r.status != CheckStatus::Skipped || r.summary.is_empty())
        .map(|r| MergeQualityCheck {
            name: r.label.clone(),
            passed: r.status == CheckStatus::Passed,
            detail: Some(if r.summary.is_empty() { r.command.clone() } else { r.summary.clone() }),
        })
        .collect();
    let passed = run.status == CheckStatus::Passed && checks.iter().all(|c| c.passed);
    MergeQualityReport { passed, checks }
}

#[async_trait]
impl MergeQualityRunner for MergeQuality {
    async fn check(
        &self,
        project: &Project,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<MergeQualityReport, String> {
        let root = project.root_path.as_deref().map(Path::new);
        let profile = self.quality.profile(project.id, root.or(Some(dir))).await.map_err(|e| e.to_string())?;
        let started = self
            .quality
            .start(StartRun {
                project_id: project.id,
                task_id: None,
                agent_id: None,
                target: RunTarget::Worktree { path: dir.to_string_lossy().into_owned() },
                workspace: dir.to_path_buf(),
                checks: profile.checks,
                kinds: MERGE_CHECK_KINDS.to_vec(),
                check_ids: Vec::new(),
                budget: QualityService::default_budget(),
            })
            .await;
        let run = match started {
            Ok(run) => run,
            // Nothing configured for build / typecheck / test: nothing to verify.
            Err(QualityError::NothingToRun(_)) => return Ok(MergeQualityReport { passed: true, checks: Vec::new() }),
            Err(e) => return Err(e.to_string()),
        };
        let finished = tokio::select! {
            r = self.quality.wait(&run.id) => r.map_err(|e| e.to_string())?,
            () = cancel.cancelled() => {
                let _ = self.quality.cancel(&run.id);
                return Err("cancelled".into());
            }
        };
        Ok(merge_report(&finished))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orkestr_protocol::methods::quality::CheckResult;

    fn result(kind: CheckKind, status: CheckStatus, passed: u32, failed: u32) -> CheckResult {
        serde_json::from_value(serde_json::json!({
            "checkId": format!("{kind:?}"),
            "kind": kind,
            "toolchain": "Cargo",
            "label": format!("{kind:?}"),
            "command": "cmd",
            "cwd": ".",
            "status": status,
            "durationMs": 1,
            "passed": passed,
            "failed": failed,
            "skipped": 0,
            "failures": [],
            "diagnostics": [],
            "rawTail": "",
            "summary": "",
        }))
        .expect("check result")
    }

    fn run(status: CheckStatus, results: Vec<CheckResult>) -> QualityRun {
        serde_json::from_value(serde_json::json!({
            "id": "r1",
            "projectId": ProjectId::new(),
            "target": { "type": "project" },
            "workspace": ".",
            "kinds": [],
            "status": status,
            "results": results,
            "startedAt": orkestr_domain::now(),
            "durationMs": 1,
        }))
        .expect("quality run")
    }

    #[test]
    fn tests_gate_follows_the_test_checks() {
        let ok = run(
            CheckStatus::Failed,
            vec![
                result(CheckKind::Lint, CheckStatus::Failed, 0, 0),
                result(CheckKind::Test, CheckStatus::Passed, 5, 0),
            ],
        );
        assert_eq!(tests_gate(&ok).status, MergeGateStatus::Passed, "lint failures do not fail the tests gate");
        let bad = run(CheckStatus::Failed, vec![result(CheckKind::Test, CheckStatus::Failed, 4, 1)]);
        let g = tests_gate(&bad);
        assert_eq!(g.status, MergeGateStatus::Failed);
        assert!(g.detail.unwrap().contains("1 failed"));
        let running = run(CheckStatus::Running, vec![result(CheckKind::Test, CheckStatus::Running, 0, 0)]);
        assert_eq!(tests_gate(&running).status, MergeGateStatus::Pending);
        assert_eq!(tests_gate(&run(CheckStatus::Passed, vec![])).status, MergeGateStatus::Unknown);
    }

    #[test]
    fn merge_report_passes_only_when_every_check_passed() {
        let ok = run(CheckStatus::Passed, vec![result(CheckKind::Build, CheckStatus::Passed, 0, 0)]);
        assert!(merge_report(&ok).passed);
        let bad = run(
            CheckStatus::Failed,
            vec![
                result(CheckKind::Build, CheckStatus::Passed, 0, 0),
                result(CheckKind::Test, CheckStatus::Failed, 1, 2),
            ],
        );
        let r = merge_report(&bad);
        assert!(!r.passed);
        assert_eq!(r.checks.len(), 2);
    }
}
