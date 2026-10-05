//! `pipeline.*` — the end-to-end quality pipeline of a task and its
//! Self-Healing loop (§17, §68.15–16). Phase 2.
//!
//! Per task: **Author** (the agent implements in its worktree) → **Review**
//! (automatic code review) → **Build** → **Test** (quality runner in the
//! task's worktree) → **Security** (adversarial review; unresolved critical
//! findings block) → **Merge** (Merge Manager proposal, merged per the merge
//! policy). Stages are configurable per project and per task kind (role).
//! The pipeline drives the task state machine (`Running → Review → Testing →
//! Completed`) and publishes `PipelineUpdated` / `PipelineStageChanged`.
//!
//! **Self-Healing**: a failing Build / Test / Review / Security stage is
//! analysed into a structured [`FailureAnalysis`] (failing tests, compiler
//! errors with file:line, relevant code excerpts) and handed to a fix agent
//! (same model first, escalated model later) through the agent handoff; the
//! stages then run again (Build → Test → Review → Security). Hard limits:
//! attempts, wall time, budget, and a repeated identical failure signature
//! (no progress). When a limit is hit the pipeline escalates to the user with
//! an approval (approve = more attempts, reject = the task fails).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ManagerRunId, MergeProposalId, ModelRef, ProjectId, TaskId, Timestamp};

use super::method;
use super::quality::{Diagnostic, GateReport, TestFailure};
use super::reviews::ReviewVerdict;

/// Max healing attempts a configuration may allow.
pub const MAX_HEALING_ATTEMPTS: u32 = 10;
/// Max wall time (minutes) of a pipeline a configuration may allow.
pub const MAX_PIPELINE_MINS: u32 = 1440;
/// Max code excerpts kept in a failure analysis.
pub const MAX_EXCERPTS: usize = 8;
/// Max failing tests / compiler errors kept in a failure analysis.
pub const MAX_FAILURE_ITEMS: usize = 20;

/// A pipeline stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PipelineStageKind {
    /// The agent implements the task in its worktree.
    Author,
    /// Automatic code review.
    Review,
    /// Build / type check.
    Build,
    /// Tests.
    Test,
    /// Adversarial (security / QA) review.
    Security,
    /// Merge Manager proposal and merge per policy.
    Merge,
}

impl PipelineStageKind {
    /// Every stage in pipeline order.
    pub const ALL: [PipelineStageKind; 6] = [
        PipelineStageKind::Author,
        PipelineStageKind::Review,
        PipelineStageKind::Build,
        PipelineStageKind::Test,
        PipelineStageKind::Security,
        PipelineStageKind::Merge,
    ];
}

/// Status of a stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum StageStatus {
    /// Not reached yet.
    Pending,
    /// In progress.
    Running,
    /// Passed.
    Passed,
    /// Failed (the healing loop takes over, or the pipeline stops).
    Failed,
    /// Disabled, not applicable or nothing to run.
    Skipped,
    /// Waiting for the user (acceptance, merge approval).
    Waiting,
}

/// Status of a pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PipelineStatus {
    /// Stages are running.
    Running,
    /// A fix agent is working on a failure.
    Healing,
    /// Waiting for the user to accept the result (approval policy Ask, or a
    /// partial agent result).
    AwaitingUser,
    /// Self-healing hit a limit; an approval asks the user how to continue.
    Escalated,
    /// Every stage passed (merge proposed or merged).
    Passed,
    /// The task failed.
    Failed,
    /// Stopped (run cancelled, emergency stop, shutdown before the end).
    Cancelled,
}

impl PipelineStatus {
    /// Whether the pipeline is over.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Passed | Self::Failed | Self::Cancelled)
    }
}

/// One stage of a pipeline.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineStage {
    /// Stage.
    pub kind: PipelineStageKind,
    /// Status.
    pub status: StageStatus,
    /// How many times the stage ran (re-runs after fixes count).
    pub runs: u32,
    /// Start of the latest run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at: Option<Timestamp>,
    /// End of the latest run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
    /// Short detail (counts, verdict, reason for skipping).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub detail: Option<String>,
    /// Related record: review id, quality run id or merge proposal id.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ref_id: Option<String>,
}

impl PipelineStage {
    /// A pending stage.
    #[must_use]
    pub fn pending(kind: PipelineStageKind) -> Self {
        Self {
            kind,
            status: StageStatus::Pending,
            runs: 0,
            started_at: None,
            finished_at: None,
            detail: None,
            ref_id: None,
        }
    }
}

/// A code excerpt relevant to a failure.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CodeExcerpt {
    /// Project-relative path.
    pub file: String,
    /// First line (1-based).
    pub start_line: u32,
    /// Last line (inclusive).
    pub end_line: u32,
    /// Text of the lines.
    pub text: String,
    /// Why it was selected (`failure line`, `context optimizer`).
    pub reason: String,
}

/// Structured summary of a failing stage (input of a fix attempt).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FailureAnalysis {
    /// Stage that failed.
    pub stage: PipelineStageKind,
    /// One-paragraph summary.
    pub summary: String,
    /// Normalised fingerprint of the failure (no-progress detection).
    pub signature: String,
    /// Failing tests.
    #[serde(default)]
    pub failing_tests: Vec<TestFailure>,
    /// Compiler / type errors with file:line.
    #[serde(default)]
    pub compiler_errors: Vec<Diagnostic>,
    /// Review / adversarial findings to address.
    #[serde(default)]
    pub findings: Vec<String>,
    /// Relevant code (failure lines + context optimizer selection).
    #[serde(default)]
    pub excerpts: Vec<CodeExcerpt>,
    /// Quality run / review the analysis comes from.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub source_id: Option<String>,
}

/// Outcome of a healing attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum HealingOutcome {
    /// Waiting for a scheduler slot.
    Queued,
    /// The fix agent is working.
    Running,
    /// The fix agent submitted; the stages run again.
    Submitted,
    /// The stages passed after this fix.
    Fixed,
    /// The stages still fail after this fix.
    StillFailing,
    /// The fix agent failed or could not start.
    AgentFailed,
    /// Stopped (cancel, emergency stop, shutdown).
    Stopped,
}

/// One Self-Healing attempt.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealingAttempt {
    /// 1-based attempt number.
    pub attempt: u32,
    /// What it fixes.
    pub analysis: FailureAnalysis,
    /// Model of the fix agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRef>,
    /// The model was escalated (a different model than the previous attempt).
    pub escalated: bool,
    /// Fix agent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Outcome.
    pub outcome: HealingOutcome,
    /// Note (why the model stayed the same, why the agent failed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub note: Option<String>,
    /// Start.
    pub started_at: Timestamp,
    /// End.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
}

/// Hard limits of the Self-Healing loop.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct HealingLimits {
    /// Fix attempts before escalating (0 = no self-healing).
    pub max_attempts: u32,
    /// Wall time of the whole pipeline (minutes).
    pub max_wall_mins: u32,
    /// Spend of the task (USD) after which no new attempt starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_budget_usd: Option<f64>,
    /// Stop when a fix produces the same failure signature again.
    pub stop_on_repeat: bool,
    /// Use a different (stronger) model from the second attempt on.
    pub escalate_model: bool,
}

impl Default for HealingLimits {
    fn default() -> Self {
        Self { max_attempts: 3, max_wall_mins: 60, max_budget_usd: None, stop_on_repeat: true, escalate_model: true }
    }
}

/// What the Merge stage does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum PipelineMergeMode {
    /// No merge stage.
    Off,
    /// Create a Merge Manager proposal only.
    Propose,
    /// Propose, and merge right away when the merge policy allows it (Auto
    /// policy with every gate green); otherwise the proposal waits for approval.
    #[default]
    PerPolicy,
}

/// Stages of one task kind (agent role, e.g. `QA`, `Docs`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineKindOverride {
    /// Task kind: the agent role name (`Backend`, `QA`, `Architect`, …) or a task tag.
    pub kind: String,
    /// Enabled stages for this kind (Author is always on).
    pub stages: Vec<PipelineStageKind>,
}

/// Per-project pipeline configuration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineConfig {
    /// Project.
    pub project_id: ProjectId,
    /// Run the pipeline for Manager runs (off = the Phase 1 acceptance).
    pub enabled: bool,
    /// Enabled stages (Author is always on).
    pub stages: Vec<PipelineStageKind>,
    /// Per task kind stage sets.
    #[serde(default)]
    pub overrides: Vec<PipelineKindOverride>,
    /// Self-Healing limits.
    pub healing: HealingLimits,
    /// Merge stage behaviour.
    #[serde(default)]
    pub merge: PipelineMergeMode,
    /// Whether the user saved this configuration (else defaults).
    pub customized: bool,
    /// Last save.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub updated_at: Option<Timestamp>,
}

impl PipelineConfig {
    /// Defaults: every stage, 3 attempts, 60 minutes, merge per policy.
    #[must_use]
    pub fn defaults(project_id: ProjectId) -> Self {
        Self {
            project_id,
            enabled: true,
            stages: PipelineStageKind::ALL.to_vec(),
            overrides: Vec::new(),
            healing: HealingLimits::default(),
            merge: PipelineMergeMode::PerPolicy,
            customized: false,
            updated_at: None,
        }
    }

    /// Enabled stages for a task of `kinds` (role name + tags): the first
    /// matching override wins (case-insensitive), else the project stages.
    #[must_use]
    pub fn stages_for(&self, kinds: &[String]) -> Vec<PipelineStageKind> {
        let picked = self
            .overrides
            .iter()
            .find(|o| kinds.iter().any(|k| k.eq_ignore_ascii_case(o.kind.trim())))
            .map_or(&self.stages, |o| &o.stages);
        let mut out: Vec<PipelineStageKind> = PipelineStageKind::ALL
            .iter()
            .copied()
            .filter(|s| *s == PipelineStageKind::Author || picked.contains(s))
            .collect();
        if self.merge == PipelineMergeMode::Off {
            out.retain(|s| *s != PipelineStageKind::Merge);
        }
        out
    }
}

/// The quality pipeline of one task.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct TaskPipeline {
    /// Id.
    pub id: String,
    /// Project.
    pub project_id: ProjectId,
    /// Task.
    pub task_id: TaskId,
    /// Manager run (when started by a run).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Status.
    pub status: PipelineStatus,
    /// Why it is in this status.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub status_reason: Option<String>,
    /// Stages in order (disabled ones are `Skipped`).
    pub stages: Vec<PipelineStage>,
    /// Self-Healing attempts, oldest first.
    #[serde(default)]
    pub healing: Vec<HealingAttempt>,
    /// Limits in effect.
    pub limits: HealingLimits,
    /// Extra attempts granted by the user after an escalation.
    #[serde(default)]
    pub granted_attempts: u32,
    /// The agent that implemented the task.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author_agent_id: Option<AgentId>,
    /// Model of the author.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub author_model: Option<ModelRef>,
    /// Agent branch (dedicated worktree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch: Option<String>,
    /// Escalation approval (pending while `Escalated`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub approval_id: Option<String>,
    /// Latest code review verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub review_verdict: Option<ReviewVerdict>,
    /// Latest adversarial review verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub security_verdict: Option<ReviewVerdict>,
    /// Quality gates evaluated before completion.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gates: Option<GateReport>,
    /// Merge Manager proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_proposal_id: Option<MergeProposalId>,
    /// Merge outcome (`merged`, `proposed — awaiting approval`, error).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_outcome: Option<String>,
    /// Task spend when the pipeline started (USD).
    pub cost_at_start_usd: f64,
    /// Task spend now (USD).
    pub cost_usd: f64,
    /// Start.
    pub started_at: Timestamp,
    /// Last change.
    pub updated_at: Timestamp,
    /// End.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
}

impl TaskPipeline {
    /// The stage of `kind`.
    #[must_use]
    pub fn stage(&self, kind: PipelineStageKind) -> Option<&PipelineStage> {
        self.stages.iter().find(|s| s.kind == kind)
    }
}

/// Params of `pipeline.get`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetPipelineParams {
    /// Task.
    pub task_id: TaskId,
}

/// Result of `pipeline.get`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct GetPipelineResult {
    /// Latest pipeline of the task (`null` when none ran).
    pub pipeline: Option<TaskPipeline>,
    /// Earlier pipelines of the task, newest first.
    pub history: Vec<TaskPipeline>,
    /// Whether a pipeline driver is working on the task now.
    pub live: bool,
}

/// Params of `pipeline.list`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListPipelinesParams {
    /// Project.
    pub project_id: ProjectId,
    /// Only pipelines of this Manager run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Max rows (default 50).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `pipeline.list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ListPipelinesResult {
    /// Pipelines, newest first.
    pub pipelines: Vec<TaskPipeline>,
}

/// Params of `pipeline.start`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StartPipelineParams {
    /// A task in `Review` or `Testing` (its agent finished).
    pub task_id: TaskId,
}

/// Params of `pipeline.getConfig`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineConfigParams {
    /// Project.
    pub project_id: ProjectId,
}

/// Params of `pipeline.setConfig`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct SetPipelineConfigParams {
    /// Project.
    pub project_id: ProjectId,
    /// Run the pipeline for Manager runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub enabled: Option<bool>,
    /// Enabled stages.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub stages: Option<Vec<PipelineStageKind>>,
    /// Per task kind stage sets (replaces the list).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub overrides: Option<Vec<PipelineKindOverride>>,
    /// Self-Healing limits.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub healing: Option<HealingLimits>,
    /// Merge stage behaviour.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge: Option<PipelineMergeMode>,
    /// Restore defaults.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub reset: Option<bool>,
}

/// Compact pipeline view on a Manager run task (run card).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunTaskPipeline {
    /// Pipeline id.
    pub pipeline_id: String,
    /// Status.
    pub status: PipelineStatus,
    /// Stages with their status.
    pub stages: Vec<RunStageBrief>,
    /// Self-Healing attempts so far.
    pub healing_attempts: u32,
}

/// One stage on the run card.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RunStageBrief {
    /// Stage.
    pub kind: PipelineStageKind,
    /// Status.
    pub status: StageStatus,
}

impl From<&TaskPipeline> for RunTaskPipeline {
    fn from(p: &TaskPipeline) -> Self {
        Self {
            pipeline_id: p.id.clone(),
            status: p.status,
            stages: p.stages.iter().map(|s| RunStageBrief { kind: s.kind, status: s.status }).collect(),
            healing_attempts: u32::try_from(p.healing.len()).unwrap_or(u32::MAX),
        }
    }
}

/// Per-task pipeline result in a run report (gates, verdicts, merge).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PipelineReport {
    /// Task.
    pub task_id: TaskId,
    /// Task title.
    pub title: String,
    /// Pipeline status.
    pub status: PipelineStatus,
    /// Stages.
    pub stages: Vec<RunStageBrief>,
    /// Code review verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub review_verdict: Option<ReviewVerdict>,
    /// Adversarial review verdict.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub security_verdict: Option<ReviewVerdict>,
    /// Overall gate status (`Passed`, `Failed`, `Pending`, …) when evaluated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub gates: Option<super::quality::GateStatus>,
    /// Failing required gates.
    #[serde(default)]
    pub failed_gates: Vec<super::quality::GateKind>,
    /// Self-Healing attempts.
    pub healing_attempts: u32,
    /// Merge outcome.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_outcome: Option<String>,
    /// Merge proposal.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub merge_proposal_id: Option<MergeProposalId>,
}

method!(
    /// `pipeline.get` — the quality pipeline of a task (stages, healing attempts, analyses).
    Get, "pipeline.get", GetPipelineParams, GetPipelineResult
);
method!(
    /// `pipeline.list` — pipelines of a project (optionally of one Manager run).
    List, "pipeline.list", ListPipelinesParams, ListPipelinesResult
);
method!(
    /// `pipeline.start` — run the quality pipeline for a task whose agent finished.
    Start, "pipeline.start", StartPipelineParams, TaskPipeline
);
method!(
    /// `pipeline.getConfig` — stages, per-kind overrides, healing limits and merge mode of a project.
    GetConfig, "pipeline.getConfig", PipelineConfigParams, PipelineConfig
);
method!(
    /// `pipeline.setConfig` — change the project's pipeline configuration.
    SetConfig, "pipeline.setConfig", SetPipelineConfigParams, PipelineConfig
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stages_follow_overrides_and_merge_mode() {
        let mut c = PipelineConfig::defaults(ProjectId::new());
        assert_eq!(c.stages_for(&[]).len(), 6);
        c.overrides.push(PipelineKindOverride { kind: "docs".into(), stages: vec![PipelineStageKind::Review] });
        assert_eq!(
            c.stages_for(&["Docs".into()]),
            vec![PipelineStageKind::Author, PipelineStageKind::Review],
            "author is always on; overrides match case-insensitively"
        );
        c.merge = PipelineMergeMode::Off;
        assert!(!c.stages_for(&["Backend".into()]).contains(&PipelineStageKind::Merge));
        assert!(PipelineStatus::Passed.is_final() && !PipelineStatus::Escalated.is_final());
    }
}
