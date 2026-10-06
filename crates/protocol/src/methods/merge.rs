//! `merge.*` — the AI Merge Manager (§16, §17, §25).
//!
//! A *merge proposal* collects the agent branches of a Manager run (or an
//! explicit branch list), analyses each one (semantic diff: files, hunks,
//! symbols added/changed/removed, risk flags), detects conflicts pairwise and
//! along the proposed merge order without touching any worktree
//! (`git merge-tree --write-tree`), orders the branches along the task DAG and
//! reports the review / tests / acceptance gates and blockers.
//!
//! Executing a proposal needs an explicit approval unless the project's merge
//! policy is `Auto` **and** every gate is green. Execution takes a pre-merge
//! snapshot, merges `--no-ff` in order into an integration branch
//! (`orkestr/integration-<id>`) in its own worktree, runs the quality checks
//! there (when a quality runner is wired) and only then fast-forwards the
//! target. A conflict stops the merge; conflicts are resolved file by file
//! (ours / theirs / edited content / AI suggestion that the user accepts) and
//! `merge.execute` continues. `merge.abort` rolls back cleanly: the target is
//! never touched before the final fast-forward.
//!
//! The core sends codes and data only; the UI renders the text.
//!
//! Events (topic `project:<id>`): `MergeProposalUpdated`.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{ApprovalId, ManagerRunId, MergeProposalId, ProjectId, SnapshotId, TaskId, Timestamp};

use super::method;
use super::vcs::FileChangeStatus;

/// Max branches in one proposal.
pub const MAX_MERGE_BRANCHES: usize = 24;
/// Max characters of one conflicted-file side kept in a proposal.
pub const MAX_CONFLICT_TEXT: usize = 256 * 1024;

// ---------------------------------------------------------------------------
// Policy
// ---------------------------------------------------------------------------

/// Whether a proposal may merge without a human approval.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergePolicyMode {
    /// Every merge needs an explicit approval (default).
    #[default]
    Manual,
    /// Merge automatically when every gate is green; otherwise ask.
    Auto,
}

/// What happens to merged agent worktrees / branches.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeCleanup {
    /// Keep everything.
    Keep,
    /// Remove clean agent worktrees of merged branches (default).
    #[default]
    RemoveWorktrees,
    /// Remove the worktrees and delete the merged branches.
    RemoveWorktreesAndBranches,
}

/// Per-project merge policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergePolicy {
    /// Manual / Auto.
    #[serde(default)]
    pub mode: MergePolicyMode,
    /// Target branch (`None` = the branch checked out in the project root).
    #[serde(default)]
    #[ts(optional)]
    pub target_branch: Option<String>,
    /// The review gate must pass.
    #[serde(default = "yes")]
    pub require_review: bool,
    /// The tests gate must pass.
    #[serde(default = "yes")]
    pub require_tests: bool,
    /// Every acceptance criterion must be met.
    #[serde(default)]
    pub require_acceptance: bool,
    /// Run the quality checks on the integration branch before updating the target.
    #[serde(default = "yes")]
    pub run_quality: bool,
    /// Cleanup of merged agent worktrees / branches.
    #[serde(default)]
    pub cleanup: MergeCleanup,
}

fn yes() -> bool {
    true
}

impl Default for MergePolicy {
    fn default() -> Self {
        Self {
            mode: MergePolicyMode::Manual,
            target_branch: None,
            require_review: true,
            require_tests: true,
            require_acceptance: false,
            run_quality: true,
            cleanup: MergeCleanup::RemoveWorktrees,
        }
    }
}

// ---------------------------------------------------------------------------
// Analysis
// ---------------------------------------------------------------------------

/// Status of one gate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeGateStatus {
    /// Passed.
    Passed,
    /// Failed.
    Failed,
    /// Still running / not verified yet.
    Pending,
    /// No information.
    #[default]
    Unknown,
    /// Not required by the policy.
    NotRequired,
}

impl MergeGateStatus {
    /// Green for automatic merging.
    #[must_use]
    pub fn is_green(self) -> bool {
        matches!(self, Self::Passed | Self::NotRequired)
    }
}

/// One gate with optional technical detail.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeGate {
    /// Status.
    pub status: MergeGateStatus,
    /// Detail (command, reviewer note, counts …) — data, not UI text.
    #[serde(default)]
    #[ts(optional)]
    pub detail: Option<String>,
}

impl MergeGate {
    /// Gate with a status and no detail.
    #[must_use]
    pub fn of(status: MergeGateStatus) -> Self {
        Self { status, detail: None }
    }

    /// Gate with detail.
    #[must_use]
    pub fn with(status: MergeGateStatus, detail: impl Into<String>) -> Self {
        Self { status, detail: Some(detail.into()) }
    }
}

/// The gates of one branch.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeGates {
    /// Task status (completed?).
    pub task: MergeGate,
    /// Code review.
    pub review: MergeGate,
    /// Tests.
    pub tests: MergeGate,
    /// Acceptance criteria.
    pub acceptance: MergeGate,
}

impl MergeGates {
    /// Every gate green.
    #[must_use]
    pub fn all_green(&self) -> bool {
        [&self.task, &self.review, &self.tests, &self.acceptance].iter().all(|g| g.status.is_green())
    }

    /// Some gate failed.
    #[must_use]
    pub fn any_failed(&self) -> bool {
        [&self.task, &self.review, &self.tests, &self.acceptance].iter().any(|g| g.status == MergeGateStatus::Failed)
    }
}

/// Risk flags of a branch.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeRiskFlag {
    /// Database / schema migrations.
    Migration,
    /// Public API removed or its declaration changed.
    PublicApi,
    /// Dependency lockfiles.
    Lockfile,
    /// Build / package configuration.
    Config,
    /// CI pipelines.
    Ci,
    /// Security-sensitive paths (auth, secrets, permissions, crypto).
    Security,
    /// Files deleted.
    Deletion,
    /// Binary files.
    Binary,
    /// More than 1000 changed lines.
    LargeChange,
}

/// One risk with the paths (or symbols) that triggered it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeRisk {
    /// Flag.
    pub flag: MergeRiskFlag,
    /// Paths / symbols.
    pub items: Vec<String>,
}

/// Per-file summary of a branch's diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeFileSummary {
    /// Path.
    pub path: String,
    /// Previous path (renames).
    #[serde(default)]
    #[ts(optional)]
    pub old_path: Option<String>,
    /// Change.
    pub status: FileChangeStatus,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Number of hunks.
    pub hunks: u32,
    /// Hunk headers (`@@ … @@ <context>`), first few.
    pub hunk_headers: Vec<String>,
    /// Binary file.
    pub binary: bool,
}

/// How a symbol changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum SymbolChange {
    /// New.
    Added,
    /// Body or declaration changed.
    Changed,
    /// Removed.
    Removed,
}

/// One symbol-level change (regex symbol extraction, `orkestr-context`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeSymbolChange {
    /// File.
    pub path: String,
    /// Symbol name.
    pub name: String,
    /// Symbol kind (`Function`, `Struct`, …).
    pub kind: String,
    /// Change.
    pub change: SymbolChange,
    /// Looks public (`pub`, `export`, `public`, capitalised Go name …).
    pub public: bool,
    /// The declaration line itself changed (signature change).
    pub signature_changed: bool,
}

/// Semantic diff of one branch against its merge base with the target.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeSemanticDiff {
    /// Files.
    pub files: Vec<MergeFileSummary>,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Symbols.
    pub symbols: Vec<MergeSymbolChange>,
    /// Risks.
    pub risks: Vec<MergeRisk>,
    /// Some files were not analysed (limits).
    pub truncated: bool,
}

/// Where a branch stands in the merge.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeBranchState {
    /// Not merged yet.
    #[default]
    Pending,
    /// Merged into the integration branch.
    Merged,
    /// Already contained in the target.
    UpToDate,
    /// Stopped on conflicts.
    Conflicted,
}

/// One branch of a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeBranch {
    /// Branch name.
    pub branch: String,
    /// Head commit at analysis time.
    pub commit: String,
    /// Task the branch implements.
    #[serde(default)]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Task title (or the branch name).
    pub title: String,
    /// Worktree where the branch is checked out.
    #[serde(default)]
    #[ts(optional)]
    pub worktree_path: Option<String>,
    /// The worktree has uncommitted changes (they would not be merged).
    pub dirty: bool,
    /// Branches of this proposal that must merge first (task DAG + ancestry).
    pub depends_on: Vec<String>,
    /// Merge base with the target.
    #[serde(default)]
    #[ts(optional)]
    pub merge_base: Option<String>,
    /// Commits not yet in the target.
    pub commits_ahead: u32,
    /// Semantic diff.
    pub diff: MergeSemanticDiff,
    /// Gates.
    pub gates: MergeGates,
    /// Paths predicted to conflict when merged in the proposed order.
    pub predicted_conflicts: Vec<String>,
    /// Execution state.
    #[serde(default)]
    pub state: MergeBranchState,
    /// Merge commit on the integration branch.
    #[serde(default)]
    #[ts(optional)]
    pub merge_commit: Option<String>,
}

/// Against what a conflict was detected.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeConflictScope {
    /// Merging `branch` after the earlier branches of the order into the target.
    Order,
    /// `branch` vs `other` merged directly (order independent).
    Pair,
}

/// A detected (predicted) conflict.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeConflictInfo {
    /// Scope.
    pub scope: MergeConflictScope,
    /// Branch.
    pub branch: String,
    /// Other branch (pair scope).
    #[serde(default)]
    #[ts(optional)]
    pub other: Option<String>,
    /// Conflicting paths.
    pub paths: Vec<String>,
}

/// Why a proposal cannot be executed as is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeBlockerKind {
    /// The branch does not resolve to a commit.
    BranchMissing,
    /// The branch's task is not completed.
    TaskNotCompleted,
    /// The branch's worktree has uncommitted changes.
    UncommittedChanges,
    /// A prerequisite task's branch is neither in the proposal nor merged.
    MissingDependency,
    /// A required gate failed.
    GateFailed,
    /// The target branch does not exist.
    TargetMissing,
    /// The target's worktree has uncommitted changes (the final fast-forward would be refused).
    TargetDirty,
    /// A merge or rebase is in progress in the target's worktree.
    TargetBusy,
}

/// A blocker.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeBlocker {
    /// Kind.
    pub kind: MergeBlockerKind,
    /// Branch concerned.
    #[serde(default)]
    #[ts(optional)]
    pub branch: Option<String>,
    /// Technical detail (paths, task id, gate) — data, not UI text.
    #[serde(default)]
    #[ts(optional)]
    pub detail: Option<String>,
}

/// What the Merge Manager recommends.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeRecommendation {
    /// Every gate is green and no conflict is predicted.
    MergeNow,
    /// Mergeable, but some gate is not green (unknown / pending) — review first.
    #[default]
    ReviewFirst,
    /// Conflicts are predicted: merge and resolve them.
    ResolveConflicts,
    /// Blockers must be fixed first.
    Blocked,
    /// Every branch is already in the target.
    NothingToMerge,
}

/// Lifecycle of a proposal.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeProposalStatus {
    /// Analysed, not executed.
    #[default]
    Proposed,
    /// Waiting for the user's approval.
    AwaitingApproval,
    /// Merging into the integration branch.
    Merging,
    /// Stopped on conflicts (resolve them, then execute again).
    Conflicted,
    /// Quality checks running on the integration branch.
    Testing,
    /// The target was updated.
    Completed,
    /// Failed (quality checks, git error); the target is unchanged.
    Failed,
    /// Aborted by the user; the target is unchanged.
    Aborted,
    /// The approval was rejected or expired.
    Rejected,
    /// Every branch of it was merged by another proposal (e.g. the run-wide
    /// one); `statusDetail` names it. Additive (Phase 2 acceptance).
    Superseded,
}

impl MergeProposalStatus {
    /// No further transitions.
    #[must_use]
    pub fn is_final(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Aborted | Self::Rejected | Self::Superseded)
    }

    /// Execution in progress (owns the integration worktree).
    #[must_use]
    pub fn is_active(self) -> bool {
        matches!(self, Self::AwaitingApproval | Self::Merging | Self::Conflicted | Self::Testing)
    }
}

/// One conflicted file during execution.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeFileConflict {
    /// Path.
    pub path: String,
    /// Common ancestor version (`None` = absent).
    #[serde(default)]
    #[ts(optional)]
    pub base: Option<String>,
    /// Integration-branch version ("ours").
    #[serde(default)]
    #[ts(optional)]
    pub ours: Option<String>,
    /// Incoming branch version ("theirs").
    #[serde(default)]
    #[ts(optional)]
    pub theirs: Option<String>,
    /// Current worktree content (with conflict markers until resolved).
    #[serde(default)]
    #[ts(optional)]
    pub result: Option<String>,
    /// AI-proposed resolution (not applied until the user accepts it).
    #[serde(default)]
    #[ts(optional)]
    pub suggestion: Option<String>,
    /// Short explanation of the suggestion (visible model output only).
    #[serde(default)]
    #[ts(optional)]
    pub suggestion_note: Option<String>,
    /// Binary / too large to show.
    pub binary: bool,
    /// Resolved and staged.
    pub resolved: bool,
}

/// One quality check.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeQualityCheck {
    /// Name (`build`, `test`, `lint`, …).
    pub name: String,
    /// Passed.
    pub passed: bool,
    /// Detail (command, failing tests, output tail).
    #[serde(default)]
    #[ts(optional)]
    pub detail: Option<String>,
}

/// Quality checks on the integration branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeQualityReport {
    /// All checks passed.
    pub passed: bool,
    /// Checks.
    pub checks: Vec<MergeQualityCheck>,
}

/// Execution state of a proposal.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeExecution {
    /// Approval requested for this execution.
    #[serde(default)]
    #[ts(optional)]
    pub approval_id: Option<ApprovalId>,
    /// Merged without approval under the `Auto` policy.
    pub auto_approved: bool,
    /// Pre-merge snapshot of the target's worktree.
    #[serde(default)]
    #[ts(optional)]
    pub snapshot_id: Option<SnapshotId>,
    /// Target commit before the merge (rollback point).
    #[serde(default)]
    #[ts(optional)]
    pub pre_merge_commit: Option<String>,
    /// Integration branch.
    #[serde(default)]
    #[ts(optional)]
    pub integration_branch: Option<String>,
    /// Integration worktree.
    #[serde(default)]
    #[ts(optional)]
    pub integration_worktree: Option<String>,
    /// Index (in `order`) of the next branch to merge.
    pub next_index: u32,
    /// Branch stopped on conflicts.
    #[serde(default)]
    #[ts(optional)]
    pub conflict_branch: Option<String>,
    /// Conflicted files of `conflict_branch`.
    pub conflicts: Vec<MergeFileConflict>,
    /// Quality checks (`None` = not run).
    #[serde(default)]
    #[ts(optional)]
    pub quality: Option<MergeQualityReport>,
    /// Target commit after the merge.
    #[serde(default)]
    #[ts(optional)]
    pub final_commit: Option<String>,
    /// Worktrees / branches removed by the cleanup policy.
    pub cleaned: Vec<String>,
    /// Started.
    #[serde(default)]
    #[ts(optional)]
    pub started_at: Option<Timestamp>,
    /// Finished.
    #[serde(default)]
    #[ts(optional)]
    pub finished_at: Option<Timestamp>,
}

/// A merge proposal (persisted).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeManagerProposal {
    /// Id.
    pub id: MergeProposalId,
    /// Project.
    pub project_id: ProjectId,
    /// Manager run the branches come from.
    #[serde(default)]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Target branch.
    pub target: String,
    /// Target commit at analysis time.
    pub target_commit: String,
    /// Merge order (branch names).
    pub order: Vec<String>,
    /// Branches (in `order`).
    pub branches: Vec<MergeBranch>,
    /// Predicted conflicts.
    pub conflicts: Vec<MergeConflictInfo>,
    /// Blockers.
    pub blockers: Vec<MergeBlocker>,
    /// Recommendation.
    pub recommendation: MergeRecommendation,
    /// Status.
    pub status: MergeProposalStatus,
    /// Technical detail of the status (error, rejection note).
    #[serde(default)]
    #[ts(optional)]
    pub status_detail: Option<String>,
    /// Policy at analysis time.
    pub policy: MergePolicy,
    /// Would merge without approval under the current policy.
    pub auto_mergeable: bool,
    /// Execution state.
    #[serde(default)]
    #[ts(optional)]
    pub execution: Option<MergeExecution>,
    /// Created.
    pub created_at: Timestamp,
    /// Updated.
    pub updated_at: Timestamp,
}

// ---------------------------------------------------------------------------
// Params / results
// ---------------------------------------------------------------------------

/// A branch picked explicitly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeBranchInput {
    /// Branch name.
    pub branch: String,
    /// Task it implements.
    #[serde(default)]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
}

/// Params of `merge.propose`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeProposeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Collect the agent branches of this Manager run.
    #[serde(default)]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Explicit branches (added to the run's).
    #[serde(default)]
    #[ts(optional)]
    pub branches: Option<Vec<MergeBranchInput>>,
    /// Target branch (default: policy target or the project root's branch).
    #[serde(default)]
    #[ts(optional)]
    pub target: Option<String>,
}

/// Params naming a proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeProposalIdParams {
    /// Proposal.
    pub proposal_id: MergeProposalId,
}

/// A proposal.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeProposalResult {
    /// The proposal.
    pub proposal: MergeManagerProposal,
}

/// Params of `merge.list`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeListParams {
    /// Project.
    pub project_id: ProjectId,
    /// Only proposals of this run.
    #[serde(default)]
    #[ts(optional)]
    pub run_id: Option<ManagerRunId>,
    /// Max results (default 50).
    #[serde(default)]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// Result of `merge.list` (newest first).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeListResult {
    /// Proposals.
    pub proposals: Vec<MergeManagerProposal>,
}

/// Params of `merge.reorder`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeReorderParams {
    /// Proposal.
    pub proposal_id: MergeProposalId,
    /// New order (a permutation of the proposal's branches that respects dependencies).
    pub order: Vec<String>,
}

/// Params of `merge.execute`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeExecuteParams {
    /// Proposal.
    pub proposal_id: MergeProposalId,
    /// Optional new order (validated like `merge.reorder`).
    #[serde(default)]
    #[ts(optional)]
    pub order: Option<Vec<String>>,
}

/// Params of `merge.abort`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeAbortParams {
    /// Proposal.
    pub proposal_id: MergeProposalId,
    /// Reason.
    #[serde(default)]
    #[ts(optional)]
    pub reason: Option<String>,
}

/// How to resolve one conflicted file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind")]
#[ts(export)]
pub enum MergeResolution {
    /// Keep the integration branch's version.
    Ours,
    /// Take the incoming branch's version.
    Theirs,
    /// Use this content (an edited result or an accepted AI suggestion).
    Content {
        /// File content.
        content: String,
    },
    /// Ask the AI for a suggestion (stored, not applied).
    Ai,
}

/// Params of `merge.resolveConflict`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeResolveConflictParams {
    /// Proposal.
    pub proposal_id: MergeProposalId,
    /// Conflicted path.
    pub path: String,
    /// Resolution.
    pub resolution: MergeResolution,
}

/// Params of `merge.policy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergePolicyParams {
    /// Project.
    pub project_id: ProjectId,
}

/// Params of `merge.setPolicy`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeSetPolicyParams {
    /// Project.
    pub project_id: ProjectId,
    /// New policy.
    pub policy: MergePolicy,
}

/// A project's merge policy.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergePolicyResult {
    /// Project.
    pub project_id: ProjectId,
    /// Policy.
    pub policy: MergePolicy,
}

method!(
    /// `merge.propose` — analyse branches into a persisted proposal.
    Propose, "merge.propose", MergeProposeParams, MergeProposalResult
);
method!(
    /// `merge.get` — one proposal.
    Get, "merge.get", MergeProposalIdParams, MergeProposalResult
);
method!(
    /// `merge.list` — proposals of a project, newest first.
    List, "merge.list", MergeListParams, MergeListResult
);
method!(
    /// `merge.reorder` — change the merge order (validated against dependencies) and re-analyse.
    Reorder, "merge.reorder", MergeReorderParams, MergeProposalResult
);
method!(
    /// `merge.execute` — start (approval / auto policy) or continue after resolved conflicts.
    Execute, "merge.execute", MergeExecuteParams, MergeProposalResult
);
method!(
    /// `merge.abort` — abort and roll back (the target is left unchanged).
    Abort, "merge.abort", MergeAbortParams, MergeProposalResult
);
method!(
    /// `merge.resolveConflict` — resolve one conflicted file, or ask the AI for a suggestion.
    ResolveConflict, "merge.resolveConflict", MergeResolveConflictParams, MergeProposalResult
);
method!(
    /// `merge.policy` — the project's merge policy.
    Policy, "merge.policy", MergePolicyParams, MergePolicyResult
);
method!(
    /// `merge.setPolicy` — replace the project's merge policy.
    SetPolicy, "merge.setPolicy", MergeSetPolicyParams, MergePolicyResult
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_defaults_are_conservative() {
        let p: MergePolicy = serde_json::from_str("{}").unwrap();
        assert_eq!(p, MergePolicy::default());
        assert_eq!(p.mode, MergePolicyMode::Manual);
        assert!(p.require_review && p.require_tests && p.run_quality);
    }

    #[test]
    fn resolution_json_shape() {
        let r: MergeResolution = serde_json::from_str(r#"{"kind":"Content","content":"x"}"#).unwrap();
        assert_eq!(r, MergeResolution::Content { content: "x".into() });
        assert_eq!(serde_json::to_string(&MergeResolution::Ai).unwrap(), r#"{"kind":"Ai"}"#);
    }

    #[test]
    fn gates_green() {
        let mut g = MergeGates {
            task: MergeGate::of(MergeGateStatus::Passed),
            review: MergeGate::of(MergeGateStatus::NotRequired),
            tests: MergeGate::of(MergeGateStatus::Passed),
            acceptance: MergeGate::of(MergeGateStatus::NotRequired),
        };
        assert!(g.all_green());
        g.tests.status = MergeGateStatus::Unknown;
        assert!(!g.all_green() && !g.any_failed());
        g.review.status = MergeGateStatus::Failed;
        assert!(g.any_failed());
    }
}
