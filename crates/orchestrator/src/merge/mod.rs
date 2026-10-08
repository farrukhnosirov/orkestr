//! # AI Merge Manager (MASTER_SPEC §16, §17, §25)
//!
//! * [`MergeManager::propose`] collects the agent branches of a Manager run
//!   (and/or explicit branches), analyses each one ([`analyze`]: semantic
//!   diff with hunks, symbols and risk flags), detects conflicts pairwise and
//!   along the merge order with `git merge-tree --write-tree` (no worktree is
//!   touched), orders the branches along the task DAG (plus git ancestry),
//!   evaluates gates (task, review, tests, acceptance — through the
//!   [`MergeGateSource`] query trait, with task / agent-handoff fallbacks)
//!   and persists a [`MergeManagerProposal`] with blockers and a
//!   recommendation.
//! * [`MergeManager::execute`] requires an explicit approval (security
//!   `ApprovalService`) unless the project's [`MergePolicy`] is `Auto` and
//!   every gate is green. Execution: pre-merge snapshot → integration branch
//!   `orkestr/integration-<id>` in its own worktree → `--no-ff` merges in
//!   order → quality checks ([`MergeQualityRunner`]; core wires the quality runner) → guarded
//!   fast-forward of the target → cleanup per policy. The target is never
//!   touched before the final fast-forward, so failure and abort roll back by
//!   discarding the integration branch.
//! * Conflicts stop the merge; [`MergeManager::resolve_conflict`] resolves a
//!   file (ours / theirs / content) or asks the model for a suggestion that
//!   the user accepts; `execute` then continues.
//!
//! Every loop is bounded: at most [`MAX_MERGE_BRANCHES`] branches, one model
//! call per suggestion with a timeout, approval with a timeout.

pub mod analyze;
mod resolve;
pub mod store;
#[cfg(test)]
mod tests;

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use futures::StreamExt;
use orkestr_agent_runtime::{AgentRuntime, ModelGateway};
use orkestr_domain::{
    now, Actor, ApprovalStatus, Capability, MergeProposalId, ModelRef, Project, ProjectId, ReviewState, RiskLevel,
    SnapshotReason, Task, TaskId, TaskStatus, UsageRecord, UsageRecordId,
};
use orkestr_events::EventBus;
use orkestr_protocol::event::{EventKind, MergeProposalPayload};
use orkestr_protocol::methods::merge::{
    MergeAbortParams, MergeBlocker, MergeBlockerKind, MergeBranch, MergeBranchInput, MergeBranchState,
    MergeConflictInfo, MergeConflictScope, MergeExecuteParams, MergeExecution, MergeFileConflict, MergeGate,
    MergeGateStatus, MergeGates, MergeListParams, MergeManagerProposal, MergePolicy, MergePolicyMode,
    MergeProposalStatus, MergeProposeParams, MergeQualityReport, MergeRecommendation, MergeResolution,
    MergeResolveConflictParams, MergeRiskFlag, MergeSemanticDiff, MAX_CONFLICT_TEXT, MAX_MERGE_BRANCHES,
};
use orkestr_protocol::methods::vcs::MergeStatus;
use orkestr_protocol::NewEvent;
use orkestr_providers::{ChatEvent, ChatMessage, ChatRequest, RequiredCapabilities, ResponseFormat, RouteRequest};
use orkestr_security::{ApprovalRequest, Security};
use orkestr_storage::{Db, Migration, ProjectRepository};
use orkestr_vcs::{AddWorktreeRequest, BlobText, GitCli, Vcs, VcsError};
use parking_lot::Mutex;
use tokio_util::sync::CancellationToken;

use crate::deps::ManagerHub;
use crate::error::{OrchResult, OrchestratorError};
use crate::store::ManagerStore;

pub use store::MergeStore;

/// Migrations of the Merge Manager.
#[must_use]
pub fn merge_migrations() -> Vec<Migration> {
    store::MIGRATIONS.to_vec()
}

/// Max branches analysed pairwise (O(n²) `merge-tree` calls).
pub const MAX_PAIRWISE_BRANCHES: usize = 12;

// ---------------------------------------------------------------------------
// Seams
// ---------------------------------------------------------------------------

/// Review / test state of a branch from the quality and review subsystems.
/// Defaults answer `Unknown`; the Merge Manager then falls back to the
/// task's review state and the agent's last handoff.
#[async_trait]
pub trait MergeGateSource: Send + Sync {
    /// Code-review gate.
    async fn review(&self, _project: ProjectId, _task: Option<TaskId>, _branch: &str) -> MergeGate {
        MergeGate::default()
    }
    /// Tests gate.
    async fn tests(&self, _project: ProjectId, _task: Option<TaskId>, _branch: &str) -> MergeGate {
        MergeGate::default()
    }
}

/// A gate source without information (everything `Unknown`).
#[derive(Debug, Clone, Copy, Default)]
pub struct NoGateSource;

impl MergeGateSource for NoGateSource {}

/// Runs build / tests on the integration worktree before the target moves.
#[async_trait]
pub trait MergeQualityRunner: Send + Sync {
    /// Checks `dir`; `Err` = could not run (treated as failure).
    async fn check(
        &self,
        project: &Project,
        dir: &Path,
        cancel: &CancellationToken,
    ) -> Result<MergeQualityReport, String>;
}

/// Tunables.
#[derive(Debug, Clone)]
pub struct MergeConfig {
    /// How long an execution approval may stay pending.
    pub approval_timeout: Duration,
    /// Timeout of one AI suggestion.
    pub model_timeout: Duration,
    /// Max output tokens of an AI suggestion.
    pub max_output_tokens: u32,
    /// How long `merge.abort` waits for a running merge to roll back.
    pub abort_wait: Duration,
}

impl Default for MergeConfig {
    fn default() -> Self {
        Self {
            approval_timeout: Duration::from_secs(24 * 3600),
            model_timeout: Duration::from_secs(180),
            max_output_tokens: 16_000,
            abort_wait: Duration::from_secs(60),
        }
    }
}

/// Everything the Merge Manager touches (engines only).
pub struct MergeDeps {
    /// Database.
    pub db: Db,
    /// Events.
    pub events: EventBus,
    /// Security (approvals, emergency).
    pub security: Arc<Security>,
    /// Task engine.
    pub tasks: Arc<orkestr_task_engine::TaskService>,
    /// Projects.
    pub projects: Arc<dyn ProjectRepository>,
    /// Git.
    pub vcs: Vcs,
    /// Snapshots.
    pub snapshots: Arc<orkestr_snapshots::SnapshotService>,
    /// Agent runtime (handoff test state fallback).
    pub agents: Option<AgentRuntime>,
    /// Models (AI conflict suggestions).
    pub models: Option<Arc<dyn ModelGateway>>,
    /// Manager settings (model choice for suggestions).
    pub hub: Option<Arc<dyn ManagerHub>>,
    /// Usage recording.
    pub cost: Option<orkestr_cost::CostEngine>,
    /// Review / tests gates.
    pub gates: Arc<dyn MergeGateSource>,
    /// Quality checks on the integration branch.
    pub quality: Option<Arc<dyn MergeQualityRunner>>,
    /// Tunables.
    pub config: MergeConfig,
}

impl std::fmt::Debug for MergeDeps {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeDeps")
            .field("quality", &self.quality.is_some())
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

impl MergeDeps {
    /// Shares the AI Manager's engines (core composition root). Gates
    /// default to [`NoGateSource`]; no quality runner.
    #[must_use]
    pub fn from_manager(manager: &crate::Manager) -> Self {
        let d = &manager.inner.deps;
        Self {
            db: d.db.clone(),
            events: d.events.clone(),
            security: Arc::clone(&d.security),
            tasks: Arc::clone(&d.tasks),
            projects: Arc::clone(&d.projects),
            vcs: d.vcs.clone(),
            snapshots: Arc::clone(&d.snapshots),
            agents: Some(d.agents.clone()),
            models: Some(Arc::clone(&d.models)),
            hub: Some(Arc::clone(&d.hub)),
            cost: Some(d.cost.clone()),
            gates: Arc::new(NoGateSource),
            quality: None,
            config: MergeConfig {
                approval_timeout: d.config.approval_timeout.max(Duration::from_secs(3600)),
                ..MergeConfig::default()
            },
        }
    }

    /// Replaces the gate source.
    #[must_use]
    pub fn with_gates(mut self, gates: Arc<dyn MergeGateSource>) -> Self {
        self.gates = gates;
        self
    }

    /// Wires a quality runner.
    #[must_use]
    pub fn with_quality(mut self, quality: Arc<dyn MergeQualityRunner>) -> Self {
        self.quality = Some(quality);
        self
    }
}

// ---------------------------------------------------------------------------
// Service
// ---------------------------------------------------------------------------

/// Notified after a proposal merged into its target (the quality pipeline
/// mirrors it on the tasks' Merge stage).
#[async_trait]
pub trait MergeCompletedHook: Send + Sync {
    /// `proposal` has status `Completed`.
    async fn merged(&self, proposal: &MergeManagerProposal);
}

struct Inner {
    hooks: parking_lot::RwLock<Vec<Arc<dyn MergeCompletedHook>>>,
    deps: MergeDeps,
    store: MergeStore,
    runs: ManagerStore,
    /// Proposals with a live background task (approval waiter / merge) → cancel.
    active: Mutex<HashMap<MergeProposalId, CancellationToken>>,
    /// Serialises git mutations per project.
    project_locks: Mutex<HashMap<ProjectId, Arc<tokio::sync::Mutex<()>>>>,
    /// Serialises execute / abort decisions.
    decide: tokio::sync::Mutex<()>,
}

/// The Merge Manager (cheap to clone).
#[derive(Clone)]
pub struct MergeManager {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for MergeManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("MergeManager").field("active", &self.inner.active.lock().len()).finish_non_exhaustive()
    }
}

fn short(id: &MergeProposalId) -> String {
    let s = id.to_string();
    s.chars().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect()
}

fn vcs_err(e: VcsError) -> OrchestratorError {
    match e {
        VcsError::NotFound(m) => OrchestratorError::NotFound(m),
        VcsError::InvalidArgument(m) | VcsError::NotARepository(m) | VcsError::Unsupported(m) => {
            OrchestratorError::Invalid(m)
        }
        VcsError::GitNotFound => OrchestratorError::Invalid(e.to_string()),
        VcsError::Conflict(_) | VcsError::Dirty(_) | VcsError::RefChanged(_) | VcsError::Locked(_) => {
            OrchestratorError::Conflict(e.to_string())
        }
        other => OrchestratorError::Internal(other.to_string()),
    }
}

fn root_of(project: &Project) -> OrchResult<PathBuf> {
    project
        .root_path
        .as_deref()
        .map(PathBuf::from)
        .ok_or_else(|| OrchestratorError::Invalid("the project has no folder: open a folder to use Git".into()))
}

impl MergeManager {
    /// Creates the service.
    #[must_use]
    pub fn new(deps: MergeDeps) -> Self {
        let store = MergeStore::new(deps.db.clone());
        let runs = ManagerStore::new(deps.db.clone());
        Self {
            inner: Arc::new(Inner {
                hooks: parking_lot::RwLock::new(Vec::new()),
                deps,
                store,
                runs,
                active: Mutex::new(HashMap::new()),
                project_locks: Mutex::new(HashMap::new()),
                decide: tokio::sync::Mutex::new(()),
            }),
        }
    }

    /// Registers a hook called after every completed merge.
    pub fn on_completed(&self, hook: Arc<dyn MergeCompletedHook>) {
        self.inner.hooks.write().push(hook);
    }

    /// After `done` merged: other open proposals whose branches it merged
    /// (same branch, same commit) are marked `Superseded` — their branches
    /// are in the target (and may be cleaned up already). Then the hooks run.
    async fn after_completed(&self, done: &MergeManagerProposal) {
        let merged: Vec<(&str, &str)> = done.branches.iter().map(|b| (b.branch.as_str(), b.commit.as_str())).collect();
        match self.inner.store.list(done.project_id, None, 500).await {
            Ok(all) => {
                for mut other in all {
                    let covered = !other.branches.is_empty()
                        && other.branches.iter().all(|b| merged.contains(&(b.branch.as_str(), b.commit.as_str())));
                    if other.id == done.id
                        || other.status != MergeProposalStatus::Proposed
                        || other.target != done.target
                        || !covered
                    {
                        continue;
                    }
                    other.status = MergeProposalStatus::Superseded;
                    other.status_detail = Some(format!("merged into `{}` by proposal {}", done.target, done.id));
                    self.save(&mut other).await;
                    tracing::info!(proposal = %other.id, by = %done.id, "merge proposal superseded");
                }
            }
            Err(e) => tracing::warn!(error = %e, "could not list proposals to supersede"),
        }
        let hooks: Vec<_> = self.inner.hooks.read().clone();
        for h in hooks {
            h.merged(done).await;
        }
    }

    /// Persistence.
    #[must_use]
    pub fn store(&self) -> &MergeStore {
        &self.inner.store
    }

    /// Recovers proposals of a previous session: approval waiters are
    /// re-armed; merges that were running are rolled back (`Failed`);
    /// conflicted merges stay resumable.
    pub async fn init(&self) -> OrchResult<()> {
        let stale = self
            .inner
            .store
            .with_status(&[
                MergeProposalStatus::AwaitingApproval,
                MergeProposalStatus::Merging,
                MergeProposalStatus::Testing,
            ])
            .await?;
        for p in stale {
            if self.inner.active.lock().contains_key(&p.id) {
                continue;
            }
            if p.status == MergeProposalStatus::AwaitingApproval {
                if let Some(aid) = p.execution.as_ref().and_then(|e| e.approval_id) {
                    self.spawn_waiter(p.id, aid);
                    continue;
                }
            }
            {
                let lock = self.project_lock(p.project_id);
                let _guard = lock.lock().await;
                if self.finish_interrupted(&p).await {
                    continue;
                }
            }
            let this = self.clone();
            this.rollback(p, MergeProposalStatus::Failed, Some("Orkestr stopped while the merge was running".into()))
                .await;
        }
        Ok(())
    }

    // ------------------------------------------------------------------
    // queries
    // ------------------------------------------------------------------

    /// One proposal.
    pub async fn get(&self, id: MergeProposalId) -> OrchResult<MergeManagerProposal> {
        self.inner.store.get(id).await
    }

    /// Proposals of a project, newest first.
    pub async fn list(&self, p: MergeListParams) -> OrchResult<Vec<MergeManagerProposal>> {
        self.project(p.project_id).await?;
        self.inner.store.list(p.project_id, p.run_id, p.limit.unwrap_or(50).clamp(1, 200)).await
    }

    /// The project's merge policy.
    pub async fn policy(&self, project: ProjectId) -> OrchResult<MergePolicy> {
        self.project(project).await?;
        self.inner.store.policy(project).await
    }

    /// Replaces the project's merge policy.
    pub async fn set_policy(&self, project: ProjectId, mut policy: MergePolicy) -> OrchResult<MergePolicy> {
        let p = self.project(project).await?;
        policy.target_branch = policy.target_branch.map(|t| t.trim().to_owned()).filter(|t| !t.is_empty());
        if let (Some(t), Some(root)) = (&policy.target_branch, p.root_path.as_deref()) {
            self.inner.deps.vcs.git().check_branch_name(Path::new(root), t).await.map_err(vcs_err)?;
        }
        self.inner.store.set_policy(project, &policy).await?;
        tracing::info!(project_id = %project, mode = ?policy.mode, "merge policy updated");
        Ok(policy)
    }

    async fn project(&self, id: ProjectId) -> OrchResult<Project> {
        self.inner.deps.projects.get(id).await?.ok_or_else(|| OrchestratorError::NotFound(format!("project {id}")))
    }

    fn git(&self) -> &Arc<GitCli> {
        self.inner.deps.vcs.git()
    }

    fn project_lock(&self, project: ProjectId) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(self.inner.project_locks.lock().entry(project).or_default())
    }

    async fn save(&self, p: &mut MergeManagerProposal) {
        p.updated_at = now();
        if let Err(e) = self.inner.store.save(p).await {
            tracing::error!(proposal = %p.id, error = %e, "could not persist the merge proposal");
        }
        let ev = NewEvent::new(EventKind::MergeProposalUpdated(MergeProposalPayload { proposal: p.clone() }))
            .project(p.project_id);
        if let Err(e) = self.inner.deps.events.publish(ev).await {
            tracing::warn!(error = %e, "failed to publish MergeProposalUpdated");
        }
    }

    // ------------------------------------------------------------------
    // propose / analyse
    // ------------------------------------------------------------------

    /// Collects and analyses branches into a persisted proposal.
    pub async fn propose(&self, p: MergeProposeParams) -> OrchResult<MergeManagerProposal> {
        let project = self.project(p.project_id).await?;
        let root = root_of(&project)?;
        self.git().require_repo(&root).await.map_err(vcs_err)?;
        let policy = self.inner.store.policy(project.id).await?;
        let target = match p.target.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
            Some(t) => t.to_owned(),
            None => match &policy.target_branch {
                Some(t) => t.clone(),
                None => self.git().current_branch(&root).await.map_err(vcs_err)?.ok_or_else(|| {
                    OrchestratorError::Invalid(
                        "the project checkout is on a detached HEAD: choose a target branch".into(),
                    )
                })?,
            },
        };
        let mut inputs: Vec<MergeBranchInput> = Vec::new();
        if let Some(run_id) = p.run_id {
            let run = self.inner.runs.run(run_id).await?;
            if run.project_id != project.id {
                return Err(OrchestratorError::Invalid("the run belongs to another project".into()));
            }
            for t in &run.tasks {
                if let Some(b) = &t.branch {
                    inputs.push(MergeBranchInput { branch: b.clone(), task_id: Some(t.task_id) });
                }
            }
        }
        inputs.extend(p.branches.unwrap_or_default());
        let mut seen = std::collections::BTreeSet::new();
        inputs.retain(|i| {
            let b = i.branch.trim().trim_start_matches("refs/heads/").to_owned();
            !b.is_empty() && b != target && seen.insert(b)
        });
        for i in &mut inputs {
            i.branch = i.branch.trim().trim_start_matches("refs/heads/").to_owned();
        }
        if inputs.is_empty() {
            return Err(OrchestratorError::Invalid(
                "there are no branches to merge (the run has no agent branches and none were given)".into(),
            ));
        }
        if inputs.len() > MAX_MERGE_BRANCHES {
            return Err(OrchestratorError::Invalid(format!(
                "too many branches ({}); a proposal merges at most {MAX_MERGE_BRANCHES}",
                inputs.len()
            )));
        }
        let at = now();
        let mut proposal = MergeManagerProposal {
            id: MergeProposalId::new(),
            project_id: project.id,
            run_id: p.run_id,
            target,
            target_commit: String::new(),
            order: Vec::new(),
            branches: Vec::new(),
            conflicts: Vec::new(),
            blockers: Vec::new(),
            recommendation: MergeRecommendation::default(),
            status: MergeProposalStatus::Proposed,
            status_detail: None,
            policy: policy.clone(),
            auto_mergeable: false,
            execution: None,
            created_at: at,
            updated_at: at,
        };
        self.analyse(&project, &root, &mut proposal, &inputs, None).await?;
        self.save(&mut proposal).await;
        tracing::info!(
            proposal = %proposal.id,
            branches = proposal.branches.len(),
            recommendation = ?proposal.recommendation,
            "merge proposal created"
        );
        Ok(proposal)
    }

    /// Changes the merge order of a proposed merge (validated) and re-analyses.
    pub async fn reorder(&self, id: MergeProposalId, order: Vec<String>) -> OrchResult<MergeManagerProposal> {
        let _d = self.inner.decide.lock().await;
        let mut p = self.inner.store.get(id).await?;
        if p.status != MergeProposalStatus::Proposed {
            return Err(OrchestratorError::Conflict(format!(
                "the proposal is {:?}; only a proposed merge can be reordered",
                p.status
            )));
        }
        let project = self.project(p.project_id).await?;
        let root = root_of(&project)?;
        let inputs = inputs_of(&p);
        self.analyse(&project, &root, &mut p, &inputs, Some(order)).await?;
        self.save(&mut p).await;
        Ok(p)
    }

    /// (Re-)analyses `inputs` into `p` (branches, order, conflicts, gates,
    /// blockers, recommendation). Touches no worktree and no ref.
    async fn analyse(
        &self,
        project: &Project,
        root: &Path,
        p: &mut MergeManagerProposal,
        inputs: &[MergeBranchInput],
        order: Option<Vec<String>>,
    ) -> OrchResult<()> {
        let git = Arc::clone(self.git());
        let policy = self.inner.store.policy(project.id).await?;
        let target_commit = git
            .rev_parse(root, &format!("refs/heads/{}", p.target))
            .await
            .map_err(vcs_err)?
            .ok_or_else(|| OrchestratorError::Invalid(format!("target branch `{}` not found", p.target)))?;
        let worktrees = self.inner.deps.vcs.worktrees(project.id, root).await.map_err(vcs_err)?;
        let mut blockers: Vec<MergeBlocker> = Vec::new();
        let mut branches: Vec<MergeBranch> = Vec::new();
        let mut tasks: HashMap<String, Task> = HashMap::new();
        for input in inputs {
            let Some(commit) = git.rev_parse(root, &format!("refs/heads/{}", input.branch)).await.map_err(vcs_err)?
            else {
                blockers.push(MergeBlocker {
                    kind: MergeBlockerKind::BranchMissing,
                    branch: Some(input.branch.clone()),
                    detail: None,
                });
                continue;
            };
            let task = match input.task_id {
                Some(t) => self.inner.deps.tasks.get(t).await.ok(),
                None => None,
            };
            let wt = worktrees.iter().find(|w| w.branch.as_deref() == Some(input.branch.as_str()) && !w.is_main);
            let dirty = wt.and_then(|w| w.dirty).unwrap_or(false);
            let merge_base = git.merge_base(root, &target_commit, &commit).await.map_err(vcs_err)?;
            let commits_ahead = git.count_commits(root, &target_commit, &commit).await.map_err(vcs_err)?;
            let diff = if commits_ahead == 0 {
                MergeSemanticDiff::default()
            } else {
                let base = merge_base.clone().unwrap_or_else(|| target_commit.clone());
                analyze::semantic_diff(&git, root, &base, &commit).await.map_err(vcs_err)?
            };
            let gates = self.gates(project.id, &policy, task.as_ref(), &input.branch).await;
            if let Some(t) = &task {
                tasks.insert(input.branch.clone(), t.clone());
            }
            branches.push(MergeBranch {
                branch: input.branch.clone(),
                commit,
                task_id: input.task_id,
                title: task.as_ref().map_or_else(|| input.branch.clone(), |t| t.title.clone()),
                worktree_path: wt.map(|w| w.path.clone()),
                dirty,
                depends_on: Vec::new(),
                merge_base,
                commits_ahead,
                diff,
                gates,
                predicted_conflicts: Vec::new(),
                state: MergeBranchState::Pending,
                merge_commit: None,
            });
        }

        // Dependencies: task DAG (+ prerequisites outside the proposal) and git ancestry.
        let mut deps: BTreeMap<String, Vec<String>> = BTreeMap::new();
        for b in &branches {
            let mut ds: Vec<String> = Vec::new();
            if let Some(task) = tasks.get(&b.branch) {
                for dep in &task.dependencies {
                    if let Some(db) = branches.iter().find(|o| o.task_id == Some(*dep)) {
                        ds.push(db.branch.clone());
                        continue;
                    }
                    let Ok(dt) = self.inner.deps.tasks.get(*dep).await else { continue };
                    let dep_branch = dt.worktree.as_ref().map(|w| w.branch.clone());
                    let merged = match &dep_branch {
                        Some(name) => match git.rev_parse(root, &format!("refs/heads/{name}")).await.ok().flatten() {
                            Some(c) => git.is_ancestor(root, &c, &target_commit).await.unwrap_or(false),
                            None => true,
                        },
                        None => true,
                    };
                    if dt.status != TaskStatus::Completed || !merged {
                        blockers.push(MergeBlocker {
                            kind: MergeBlockerKind::MissingDependency,
                            branch: Some(b.branch.clone()),
                            detail: Some(match dep_branch {
                                Some(name) => format!("{} ({name})", dt.title),
                                None => dt.title.clone(),
                            }),
                        });
                    }
                }
            }
            for o in &branches {
                if o.branch != b.branch
                    && o.commit != b.commit
                    && o.commits_ahead > 0
                    && git.is_ancestor(root, &o.commit, &b.commit).await.unwrap_or(false)
                    && !ds.contains(&o.branch)
                {
                    ds.push(o.branch.clone());
                }
            }
            deps.insert(b.branch.clone(), ds);
        }
        let names: Vec<String> = branches.iter().map(|b| b.branch.clone()).collect();
        let order = match order {
            Some(o) => {
                analyze::validate_order(&o, &names, &deps).map_err(OrchestratorError::Invalid)?;
                o
            }
            None => analyze::order_branches(&names, &deps).map_err(|cycle| {
                OrchestratorError::Invalid(format!(
                    "the branches depend on each other in a cycle: {}",
                    cycle.join(", ")
                ))
            })?,
        };
        branches.sort_by_key(|b| order.iter().position(|o| o == &b.branch).unwrap_or(usize::MAX));
        for b in &mut branches {
            b.depends_on = deps.remove(&b.branch).unwrap_or_default();
        }

        // Conflicts along the order (simulated) and pairwise.
        let mut conflicts: Vec<MergeConflictInfo> = Vec::new();
        let commits: Vec<String> = branches.iter().map(|b| b.commit.clone()).collect();
        let steps = git.simulate_merges(root, &target_commit, &commits).await.map_err(vcs_err)?;
        for (b, step) in branches.iter_mut().zip(steps) {
            if !step.conflicts.is_empty() {
                b.predicted_conflicts = step.conflicts.clone();
                conflicts.push(MergeConflictInfo {
                    scope: MergeConflictScope::Order,
                    branch: b.branch.clone(),
                    other: None,
                    paths: step.conflicts,
                });
            }
        }
        if branches.len() <= MAX_PAIRWISE_BRANCHES {
            for i in 0..branches.len() {
                for j in (i + 1)..branches.len() {
                    let (a, b) = (&branches[i], &branches[j]);
                    if a.commits_ahead == 0
                        || b.commits_ahead == 0
                        || git.is_ancestor(root, &a.commit, &b.commit).await.unwrap_or(false)
                        || git.is_ancestor(root, &b.commit, &a.commit).await.unwrap_or(false)
                    {
                        continue;
                    }
                    let m = git.merge_trees(root, &a.commit, &b.commit).await.map_err(vcs_err)?;
                    if !m.clean {
                        conflicts.push(MergeConflictInfo {
                            scope: MergeConflictScope::Pair,
                            branch: a.branch.clone(),
                            other: Some(b.branch.clone()),
                            paths: m.conflicts,
                        });
                    }
                }
            }
        }

        // Branch blockers.
        for b in &branches {
            if b.dirty {
                blockers.push(MergeBlocker {
                    kind: MergeBlockerKind::UncommittedChanges,
                    branch: Some(b.branch.clone()),
                    detail: b.worktree_path.clone(),
                });
            }
            if b.task_id.is_some() && b.gates.task.status != MergeGateStatus::Passed {
                blockers.push(MergeBlocker {
                    kind: MergeBlockerKind::TaskNotCompleted,
                    branch: Some(b.branch.clone()),
                    detail: b.gates.task.detail.clone(),
                });
            }
            for (name, g) in
                [("review", &b.gates.review), ("tests", &b.gates.tests), ("acceptance", &b.gates.acceptance)]
            {
                if g.status == MergeGateStatus::Failed {
                    blockers.push(MergeBlocker {
                        kind: MergeBlockerKind::GateFailed,
                        branch: Some(b.branch.clone()),
                        detail: Some(match &g.detail {
                            Some(d) => format!("{name}: {d}"),
                            None => name.to_owned(),
                        }),
                    });
                }
            }
        }
        // Target worktree state (the final fast-forward runs there).
        if let Some(path) = git.branch_worktree(root, &p.target).await.map_err(vcs_err)? {
            let st = git.status(Path::new(&path)).await.map_err(vcs_err)?;
            if st.merge_in_progress || st.rebase_in_progress {
                blockers.push(MergeBlocker { kind: MergeBlockerKind::TargetBusy, branch: None, detail: Some(path) });
            } else if st.entries.iter().any(|e| e.conflict.is_some() || e.is_staged() || e.is_unstaged()) {
                blockers.push(MergeBlocker { kind: MergeBlockerKind::TargetDirty, branch: None, detail: Some(path) });
            }
        }

        let ordered_conflicts = conflicts.iter().any(|c| c.scope == MergeConflictScope::Order);
        p.recommendation = analyze::recommend(&branches, &blockers, ordered_conflicts);
        p.auto_mergeable = policy.mode == MergePolicyMode::Auto && p.recommendation == MergeRecommendation::MergeNow;
        p.target_commit = target_commit;
        p.order = order;
        p.branches = branches;
        p.conflicts = conflicts;
        p.blockers = blockers;
        p.policy = policy;
        Ok(())
    }

    async fn gates(&self, project: ProjectId, policy: &MergePolicy, task: Option<&Task>, branch: &str) -> MergeGates {
        let task_id = task.map(|t| t.id);
        let task_gate = match task {
            // A branch picked without a task has no task to complete.
            None => MergeGate::of(MergeGateStatus::NotRequired),
            Some(t) => match t.status {
                TaskStatus::Completed => MergeGate::of(MergeGateStatus::Passed),
                TaskStatus::Failed | TaskStatus::Cancelled => {
                    MergeGate::with(MergeGateStatus::Failed, format!("{:?}", t.status))
                }
                other => MergeGate::with(MergeGateStatus::Pending, format!("{other:?}")),
            },
        };
        let review = if policy.require_review {
            let g = self.inner.deps.gates.review(project, task_id, branch).await;
            if g.status != MergeGateStatus::Unknown {
                g
            } else {
                match task.map(|t| t.review_state) {
                    Some(ReviewState::Approved) => MergeGate::with(MergeGateStatus::Passed, "task review approved"),
                    Some(ReviewState::ChangesRequested) => {
                        MergeGate::with(MergeGateStatus::Failed, "changes requested")
                    }
                    Some(ReviewState::Rejected) => MergeGate::with(MergeGateStatus::Failed, "review rejected"),
                    Some(ReviewState::Pending) => MergeGate::of(MergeGateStatus::Pending),
                    _ => MergeGate::default(),
                }
            }
        } else {
            MergeGate::of(MergeGateStatus::NotRequired)
        };
        let tests = if policy.require_tests {
            let g = self.inner.deps.gates.tests(project, task_id, branch).await;
            if g.status != MergeGateStatus::Unknown {
                g
            } else {
                self.handoff_tests(task).await.unwrap_or_default()
            }
        } else {
            MergeGate::of(MergeGateStatus::NotRequired)
        };
        let acceptance = {
            let actual = match task {
                None => MergeGate::default(),
                Some(t) if t.acceptance_criteria.is_empty() => MergeGate::default(),
                Some(t) => {
                    let met = t.acceptance_criteria.iter().filter(|c| c.met).count();
                    let total = t.acceptance_criteria.len();
                    let detail = format!("{met}/{total}");
                    if met == total {
                        MergeGate::with(MergeGateStatus::Passed, detail)
                    } else if matches!(t.status, TaskStatus::Failed | TaskStatus::Cancelled) {
                        MergeGate::with(MergeGateStatus::Failed, detail)
                    } else {
                        MergeGate::with(MergeGateStatus::Pending, detail)
                    }
                }
            };
            if policy.require_acceptance {
                actual
            } else {
                MergeGate { status: MergeGateStatus::NotRequired, detail: actual.detail }
            }
        };
        MergeGates { task: task_gate, review, tests, acceptance }
    }

    async fn handoff_tests(&self, task: Option<&Task>) -> Option<MergeGate> {
        let agents = self.inner.deps.agents.as_ref()?;
        let agent = task?.agent_id?;
        let handoffs = agents.handoffs(agent).await.ok()?;
        let ts = handoffs.iter().rev().find_map(|h| h.test_state.clone())?;
        let status = if ts.passed { MergeGateStatus::Passed } else { MergeGateStatus::Failed };
        Some(MergeGate::with(status, ts.command))
    }

    // ------------------------------------------------------------------
    // execute
    // ------------------------------------------------------------------

    /// Starts a merge (approval or `Auto` policy) or continues one whose
    /// conflicts were resolved. Returns at once; progress arrives as
    /// `MergeProposalUpdated` events.
    pub async fn execute(&self, params: MergeExecuteParams) -> OrchResult<MergeManagerProposal> {
        let _d = self.inner.decide.lock().await;
        let mut p = self.inner.store.get(params.proposal_id).await?;
        match p.status {
            MergeProposalStatus::Proposed => {}
            MergeProposalStatus::Conflicted => return self.continue_after_conflicts(p).await,
            s if s.is_active() => {
                return Err(OrchestratorError::Conflict("this merge is already being executed".into()));
            }
            s => {
                return Err(OrchestratorError::Conflict(format!(
                    "the proposal is {s:?}; propose the merge again to retry"
                )))
            }
        }
        let project = self.project(p.project_id).await?;
        let root = root_of(&project)?;
        let others = self.inner.store.list(project.id, None, 200).await?;
        if others.iter().any(|o| o.id != p.id && o.status.is_active()) {
            return Err(OrchestratorError::Conflict(
                "another merge of this project is in progress — finish or abort it first".into(),
            ));
        }
        // Fresh analysis: branches or the target may have moved since the proposal.
        let inputs = inputs_of(&p);
        let order = params.order.unwrap_or_else(|| p.order.clone());
        self.analyse(&project, &root, &mut p, &inputs, Some(order)).await?;
        if !p.blockers.is_empty() {
            self.save(&mut p).await;
            return Err(OrchestratorError::Conflict(format!(
                "the merge is blocked ({} blocker(s)) — see the proposal",
                p.blockers.len()
            )));
        }
        if p.recommendation == MergeRecommendation::NothingToMerge {
            self.save(&mut p).await;
            return Err(OrchestratorError::Invalid("every branch is already merged into the target".into()));
        }
        let cancel = self
            .inner
            .deps
            .security
            .emergency
            .register_project(project.id)
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        let mut exec = MergeExecution { started_at: Some(now()), ..MergeExecution::default() };
        if p.auto_mergeable {
            exec.auto_approved = true;
            p.execution = Some(exec);
            p.status = MergeProposalStatus::Merging;
            p.status_detail = None;
            self.save(&mut p).await;
            self.inner.active.lock().insert(p.id, cancel);
            self.spawn_run(p.id);
            tracing::info!(proposal = %p.id, "merge started automatically (Auto policy, all gates green)");
            return Ok(p);
        }
        let risky = p.branches.iter().any(|b| {
            b.diff.risks.iter().any(|r| {
                matches!(r.flag, MergeRiskFlag::Migration | MergeRiskFlag::PublicApi | MergeRiskFlag::Security)
            })
        });
        let risk = if risky || !p.conflicts.is_empty() { RiskLevel::High } else { RiskLevel::Medium };
        let mut req = ApprovalRequest::new(
            Capability::Git,
            format!("Merge {} branch(es) into `{}`", p.order.len(), p.target),
            "The Merge Manager merges agent branches only with your approval (merge policy).",
            risk,
        );
        req.project_id = Some(project.id);
        req.ttl = Some(self.inner.deps.config.approval_timeout);
        req.command_preview = Some(
            p.branches
                .iter()
                .enumerate()
                .map(|(i, b)| {
                    format!("{}. {} (+{} −{}) — {}", i + 1, b.branch, b.diff.additions, b.diff.deletions, b.title)
                })
                .chain(std::iter::once(format!("→ {} (via orkestr/integration-{})", p.target, short(&p.id))))
                .collect::<Vec<_>>()
                .join("\n"),
        );
        let approval = self
            .inner
            .deps
            .security
            .approvals
            .request(req)
            .await
            .map_err(|e| OrchestratorError::Internal(format!("could not request the merge approval: {e}")))?;
        exec.approval_id = Some(approval.id);
        p.execution = Some(exec);
        p.status = MergeProposalStatus::AwaitingApproval;
        p.status_detail = None;
        self.save(&mut p).await;
        self.inner.active.lock().insert(p.id, cancel);
        self.spawn_waiter(p.id, approval.id);
        Ok(p)
    }

    fn spawn_waiter(&self, id: MergeProposalId, approval: orkestr_domain::ApprovalId) {
        let this = self.clone();
        let cancel = self.inner.active.lock().entry(id).or_default().clone();
        tokio::spawn(async move {
            let timeout = this.inner.deps.config.approval_timeout;
            let status = this.inner.deps.security.approvals.wait(approval, timeout, &cancel).await;
            let Ok(mut p) = this.inner.store.get(id).await else { return };
            if p.status != MergeProposalStatus::AwaitingApproval {
                return; // aborted meanwhile
            }
            match status {
                Ok(ApprovalStatus::Approved) => {
                    p.status = MergeProposalStatus::Merging;
                    this.save(&mut p).await;
                    this.run(id).await;
                }
                Ok(s) => {
                    let note = this
                        .inner
                        .deps
                        .security
                        .approvals
                        .get(approval)
                        .await
                        .ok()
                        .flatten()
                        .and_then(|a| a.resolution_note);
                    this.inner.active.lock().remove(&id);
                    p.status = if s == ApprovalStatus::Cancelled {
                        MergeProposalStatus::Aborted
                    } else {
                        MergeProposalStatus::Rejected
                    };
                    p.status_detail = Some(match note {
                        Some(n) => format!("{s:?}: {n}"),
                        None => format!("{s:?}"),
                    });
                    if let Some(e) = p.execution.as_mut() {
                        e.finished_at = Some(now());
                    }
                    this.save(&mut p).await;
                }
                Err(e) => {
                    this.inner.active.lock().remove(&id);
                    p.status = MergeProposalStatus::Failed;
                    p.status_detail = Some(format!("approval failed: {e}"));
                    this.save(&mut p).await;
                }
            }
        });
    }

    fn spawn_run(&self, id: MergeProposalId) {
        let this = self.clone();
        tokio::spawn(async move { this.run(id).await });
    }

    async fn continue_after_conflicts(&self, mut p: MergeManagerProposal) -> OrchResult<MergeManagerProposal> {
        let open: Vec<String> = p
            .execution
            .as_ref()
            .map(|e| e.conflicts.iter().filter(|c| !c.resolved).map(|c| c.path.clone()).collect())
            .unwrap_or_default();
        if !open.is_empty() {
            return Err(OrchestratorError::Conflict(format!("unresolved conflicts: {}", open.join(", "))));
        }
        let cancel = self
            .inner
            .deps
            .security
            .emergency
            .register_project(p.project_id)
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        p.status = MergeProposalStatus::Merging;
        p.status_detail = None;
        self.save(&mut p).await;
        self.inner.active.lock().insert(p.id, cancel);
        self.spawn_run(p.id);
        Ok(p)
    }

    /// The merge itself (background task, one per proposal).
    async fn run(&self, id: MergeProposalId) {
        let Ok(p) = self.inner.store.get(id).await else { return };
        let lock = self.project_lock(p.project_id);
        let _guard = lock.lock().await;
        let cancel = self.inner.active.lock().entry(id).or_default().clone();
        let outcome = self.run_locked(id, &cancel).await;
        self.inner.active.lock().remove(&id);
        if let Err((status, detail)) = outcome {
            if let Ok(p) = self.inner.store.get(id).await {
                self.rollback_locked(p, status, Some(detail)).await;
            }
        }
    }

    async fn run_locked(
        &self,
        id: MergeProposalId,
        cancel: &CancellationToken,
    ) -> Result<(), (MergeProposalStatus, String)> {
        let fail = |e: String| (MergeProposalStatus::Failed, e);
        let mut p = self.inner.store.get(id).await.map_err(|e| fail(e.to_string()))?;
        let project = self.project(p.project_id).await.map_err(|e| fail(e.to_string()))?;
        let root = root_of(&project).map_err(|e| fail(e.to_string()))?;
        let git = Arc::clone(self.git());
        let mut exec = p.execution.clone().unwrap_or_default();

        // 1. Fresh start: snapshot + integration worktree.
        if exec.integration_worktree.is_none() {
            let target_wt = git.branch_worktree(&root, &p.target).await.map_err(|e| fail(e.to_string()))?;
            let snap_dir = target_wt.map_or_else(|| root.clone(), PathBuf::from);
            let snap = orkestr_snapshots::CreateSnapshot {
                project_id: project.id,
                workspace_path: snap_dir,
                label: Some(format!("Before merge into {}", p.target)),
                reason: SnapshotReason::BeforeRiskyAction,
                task_id: None,
                agent_id: None,
                task_phase: None,
                created_by: Actor::System,
            };
            let snapshot = self
                .inner
                .deps
                .snapshots
                .create(snap)
                .await
                .map_err(|e| fail(format!("the pre-merge snapshot failed: {e}")))?;
            exec.snapshot_id = Some(snapshot.id);
            let pre = git
                .rev_parse(&root, &format!("refs/heads/{}", p.target))
                .await
                .map_err(|e| fail(e.to_string()))?
                .ok_or_else(|| fail(format!("target branch `{}` not found", p.target)))?;
            exec.pre_merge_commit = Some(pre.clone());
            let name = format!("orkestr/integration-{}", short(&p.id));
            self.remove_integration(&project, &root, &name, None).await;
            let wt = self
                .inner
                .deps
                .vcs
                .add_worktree(
                    project.id,
                    &root,
                    AddWorktreeRequest { branch: Some(name.clone()), base: Some(pre), ..AddWorktreeRequest::default() },
                )
                .await
                .map_err(|e| fail(format!("could not create the integration worktree: {e}")))?;
            exec.integration_branch = Some(name);
            exec.integration_worktree = Some(wt.path);
            exec.next_index = 0;
            p.execution = Some(exec.clone());
            p.status = MergeProposalStatus::Merging;
            self.save(&mut p).await;
        }
        let wt = PathBuf::from(exec.integration_worktree.clone().unwrap_or_default());
        let integration = exec.integration_branch.clone().unwrap_or_default();

        // 2. Continue after resolved conflicts.
        if let Some(cb) = exec.conflict_branch.clone() {
            let commit = git
                .conclude_merge(&wt)
                .await
                .map_err(|e| fail(format!("could not conclude the merge of {cb}: {e}")))?;
            if let Some(b) = p.branches.iter_mut().find(|b| b.branch == cb) {
                b.state = MergeBranchState::Merged;
                b.merge_commit = Some(commit);
            }
            exec.conflict_branch = None;
            exec.conflicts.clear();
            exec.next_index += 1;
            p.execution = Some(exec.clone());
            p.status = MergeProposalStatus::Merging;
            self.save(&mut p).await;
        }

        // 3. Merge in order.
        let total = p.order.len();
        let mut i = exec.next_index as usize;
        while i < total {
            if cancel.is_cancelled() {
                return Err((MergeProposalStatus::Aborted, "aborted".into()));
            }
            let name = p.order[i].clone();
            let Some(commit) = p.branches.iter().find(|b| b.branch == name).map(|b| b.commit.clone()) else {
                i += 1;
                continue;
            };
            let msg = format!("Merge branch '{name}' into {} (Orkestr Merge Manager)", p.target);
            let res = git
                .merge(&wt, &commit, Some(&integration), Some(&msg), true)
                .await
                .map_err(|e| fail(format!("merging {name} failed: {e}")))?;
            match res.status {
                MergeStatus::Merged | MergeStatus::UpToDate => {
                    if let Some(b) = p.branches.iter_mut().find(|b| b.branch == name) {
                        b.state = if res.status == MergeStatus::Merged {
                            MergeBranchState::Merged
                        } else {
                            MergeBranchState::UpToDate
                        };
                        b.merge_commit = res.commit.clone();
                    }
                }
                MergeStatus::Conflicted => {
                    let mut files = Vec::new();
                    for path in &res.conflicts {
                        files.push(conflict_file(&git, &wt, path).await);
                    }
                    if let Some(b) = p.branches.iter_mut().find(|b| b.branch == name) {
                        b.state = MergeBranchState::Conflicted;
                    }
                    exec.conflict_branch = Some(name.clone());
                    exec.conflicts = files;
                    exec.next_index = u32::try_from(i).unwrap_or(u32::MAX);
                    p.execution = Some(exec);
                    p.status = MergeProposalStatus::Conflicted;
                    p.status_detail = Some(name);
                    self.save(&mut p).await;
                    tracing::warn!(proposal = %p.id, "merge stopped on conflicts");
                    return Ok(());
                }
            }
            i += 1;
            exec.next_index = u32::try_from(i).unwrap_or(u32::MAX);
            p.execution = Some(exec.clone());
            self.save(&mut p).await;
        }

        // 4. Quality checks on the integration branch.
        if p.policy.run_quality {
            if let Some(q) = self.inner.deps.quality.clone() {
                p.status = MergeProposalStatus::Testing;
                self.save(&mut p).await;
                let report = tokio::select! {
                    r = q.check(&project, &wt, cancel) => r,
                    () = cancel.cancelled() => return Err((MergeProposalStatus::Aborted, "aborted".into())),
                };
                match report {
                    Ok(report) => {
                        let passed = report.passed;
                        exec.quality = Some(report);
                        p.execution = Some(exec.clone());
                        self.save(&mut p).await;
                        if !passed {
                            return Err(fail("the quality checks failed on the integration branch".into()));
                        }
                    }
                    Err(e) => return Err(fail(format!("the quality checks could not run: {e}"))),
                }
            }
        }
        if cancel.is_cancelled() {
            return Err((MergeProposalStatus::Aborted, "aborted".into()));
        }

        // 5. Fast-forward the target.
        let head = git
            .rev_parse(&wt, "HEAD")
            .await
            .map_err(|e| fail(e.to_string()))?
            .ok_or_else(|| fail("the integration branch has no HEAD".into()))?;
        let pre = exec.pre_merge_commit.clone().unwrap_or_default();
        git.fast_forward_branch(&root, &p.target, &pre, &head)
            .await
            .map_err(|e| fail(format!("could not update `{}`: {e}", p.target)))?;
        exec.final_commit = Some(head.clone());
        tracing::info!(proposal = %p.id, target = %p.target, commit = %head, "merge completed");
        // Checkpoint: the target holds the merge now. A stop during the
        // cleanup below is finished on the next start (`init`), never rolled
        // back as a failed merge.
        p.execution = Some(exec);
        self.save(&mut p).await;

        // 6. Cleanup.
        self.finish_completed(&project, &root, p).await;
        Ok(())
    }

    /// Cleanup + `Completed` + completion hooks of a proposal whose target
    /// was fast-forwarded to `execution.final_commit`. Under the project lock.
    async fn finish_completed(&self, project: &Project, root: &Path, mut p: MergeManagerProposal) {
        let Some(mut exec) = p.execution.take() else { return };
        let head = exec.final_commit.clone().unwrap_or_default();
        let integration = exec.integration_branch.clone().unwrap_or_default();
        self.remove_integration(project, root, &integration, exec.integration_worktree.as_deref()).await;
        exec.cleaned = self.cleanup_branches(project, root, &p, &head).await;
        exec.finished_at = Some(now());
        p.execution = Some(exec);
        p.status = MergeProposalStatus::Completed;
        p.status_detail = None;
        self.save(&mut p).await;
        self.after_completed(&p).await;
    }

    /// A merge interrupted after its fast-forward: the target already
    /// contains `execution.final_commit` → finish it (cleanup, `Completed`,
    /// hooks). `false` when the target does not hold the merge.
    async fn finish_interrupted(&self, p: &MergeManagerProposal) -> bool {
        let Some(head) = p.execution.as_ref().and_then(|e| e.final_commit.clone()) else { return false };
        let Ok(project) = self.project(p.project_id).await else { return false };
        let Ok(root) = root_of(&project) else { return false };
        let git = Arc::clone(self.git());
        let target = format!("refs/heads/{}", p.target);
        let landed = match git.rev_parse(&root, &target).await {
            Ok(Some(t)) if t == head => true,
            Ok(Some(_)) => git
                .runner()
                .run_ok(
                    &root,
                    ["merge-base", "--is-ancestor", head.as_str(), target.as_str()],
                    orkestr_vcs::RunOptions::read(),
                )
                .await
                .is_ok(),
            _ => false,
        };
        if !landed {
            return false;
        }
        tracing::info!(proposal = %p.id, target = %p.target, "finishing a merge interrupted after its fast-forward");
        self.finish_completed(&project, &root, p.clone()).await;
        true
    }

    async fn remove_integration(&self, project: &Project, root: &Path, branch: &str, path: Option<&str>) {
        let git = self.git();
        let wt_path = match path {
            Some(p) => Some(p.to_owned()),
            None => git.branch_worktree(root, branch).await.ok().flatten(),
        };
        if let Some(path) = wt_path {
            if Path::new(&path).is_dir() {
                let st = git.status(Path::new(&path)).await;
                if st.is_ok_and(|s| s.merge_in_progress) {
                    if let Err(e) = git.abort_merge(Path::new(&path)).await {
                        tracing::warn!(error = %e, "could not abort the integration merge");
                    }
                }
            }
            if let Err(e) = self.inner.deps.vcs.remove_worktree(project.id, root, &path, true).await {
                if !matches!(e, VcsError::NotFound(_)) {
                    tracing::warn!(error = %e, %path, "could not remove the integration worktree");
                }
            }
        }
        if git.rev_parse(root, &format!("refs/heads/{branch}")).await.ok().flatten().is_some() {
            if let Err(e) = git.delete_branch(root, branch, true).await {
                tracing::warn!(error = %e, %branch, "could not delete the integration branch");
            }
        }
    }

    async fn cleanup_branches(
        &self,
        project: &Project,
        root: &Path,
        p: &MergeManagerProposal,
        head: &str,
    ) -> Vec<String> {
        let mut cleaned = Vec::new();
        if p.policy.cleanup == orkestr_protocol::methods::merge::MergeCleanup::Keep {
            return cleaned;
        }
        let git = self.git();
        let managed: Vec<orkestr_protocol::methods::vcs::WorktreeInfo> =
            self.inner.deps.vcs.worktrees(project.id, root).await.unwrap_or_default();
        for b in p.branches.iter().filter(|b| matches!(b.state, MergeBranchState::Merged | MergeBranchState::UpToDate))
        {
            // The branch's *current* tip must be in the merge result: commits
            // added after the analysis (the approval can wait for hours) are
            // not merged, so such a branch and its worktree are kept.
            let Ok(Some(tip)) = git.rev_parse(root, &format!("refs/heads/{}", b.branch)).await else {
                continue;
            };
            if !git.is_ancestor(root, &tip, head).await.unwrap_or(false) {
                tracing::info!(branch = %b.branch, %tip, "the branch moved after the merge analysis; kept it");
                continue;
            }
            if let Some(w) =
                managed.iter().find(|w| w.branch.as_deref() == Some(b.branch.as_str()) && w.managed && !w.is_main)
            {
                match self.inner.deps.vcs.remove_worktree(project.id, root, &w.path, false).await {
                    Ok(path) => cleaned.push(path),
                    Err(e) => {
                        tracing::info!(error = %e, path = %w.path, "kept the agent worktree");
                        continue;
                    }
                }
            }
            if p.policy.cleanup == orkestr_protocol::methods::merge::MergeCleanup::RemoveWorktreesAndBranches {
                // Deleted only while it still points at the checked tip
                // (atomic), never with `-D` on whatever it points at now.
                match git.delete_branch_at(root, &b.branch, &tip).await {
                    Ok(_) => cleaned.push(b.branch.clone()),
                    Err(e) => tracing::info!(error = %e, branch = %b.branch, "kept the merged branch"),
                }
            }
        }
        cleaned
    }

    /// Rolls back (integration worktree + branch removed; the target was
    /// never touched) and finishes the proposal with `status`.
    async fn rollback(&self, p: MergeManagerProposal, status: MergeProposalStatus, detail: Option<String>) {
        let lock = self.project_lock(p.project_id);
        let _guard = lock.lock().await;
        self.rollback_locked(p, status, detail).await;
    }

    async fn rollback_locked(&self, mut p: MergeManagerProposal, status: MergeProposalStatus, detail: Option<String>) {
        if let Ok(project) = self.project(p.project_id).await {
            if let (Ok(root), Some(exec)) = (root_of(&project), p.execution.as_ref()) {
                if let Some(branch) = exec.integration_branch.clone() {
                    let path = exec.integration_worktree.clone();
                    self.remove_integration(&project, &root, &branch, path.as_deref()).await;
                }
            }
        }
        if let Some(exec) = p.execution.as_mut() {
            exec.finished_at = Some(now());
            exec.integration_worktree = None;
        }
        if let Some(e) = &detail {
            tracing::warn!(proposal = %p.id, ?status, detail = %e, "merge rolled back");
        }
        p.status = status;
        p.status_detail = detail;
        self.save(&mut p).await;
    }

    // ------------------------------------------------------------------
    // abort
    // ------------------------------------------------------------------

    /// Aborts a proposal and rolls back. The target is left unchanged.
    pub async fn abort(&self, params: MergeAbortParams) -> OrchResult<MergeManagerProposal> {
        let reason = params.reason.filter(|r| !r.trim().is_empty()).map(|r| r.chars().take(500).collect::<String>());
        let p = {
            let _d = self.inner.decide.lock().await;
            let mut p = self.inner.store.get(params.proposal_id).await?;
            match p.status {
                MergeProposalStatus::Proposed => {
                    p.status = MergeProposalStatus::Aborted;
                    p.status_detail = reason;
                    self.save(&mut p).await;
                    return Ok(p);
                }
                MergeProposalStatus::AwaitingApproval => {
                    if let Some(t) = self.inner.active.lock().remove(&p.id) {
                        t.cancel();
                    }
                    if let Some(aid) = p.execution.as_ref().and_then(|e| e.approval_id) {
                        let _ = self.inner.deps.security.approvals.cancel(aid, "merge aborted").await;
                    }
                    p.status = MergeProposalStatus::Aborted;
                    p.status_detail = reason;
                    if let Some(e) = p.execution.as_mut() {
                        e.finished_at = Some(now());
                    }
                    self.save(&mut p).await;
                    return Ok(p);
                }
                MergeProposalStatus::Conflicted => {
                    drop(_d);
                    self.rollback(p.clone(), MergeProposalStatus::Aborted, reason).await;
                    return self.inner.store.get(p.id).await;
                }
                MergeProposalStatus::Merging | MergeProposalStatus::Testing => {
                    if let Some(t) = self.inner.active.lock().get(&p.id) {
                        t.cancel();
                    }
                    p
                }
                s => return Err(OrchestratorError::Conflict(format!("the proposal is already {s:?}"))),
            }
        };
        // A running merge rolls back at its next checkpoint.
        let deadline = tokio::time::Instant::now() + self.inner.deps.config.abort_wait;
        loop {
            let cur = self.inner.store.get(p.id).await?;
            if cur.status.is_final() || cur.status == MergeProposalStatus::Conflicted {
                if cur.status == MergeProposalStatus::Conflicted {
                    self.rollback(cur, MergeProposalStatus::Aborted, reason.clone()).await;
                    return self.inner.store.get(p.id).await;
                }
                return Ok(cur);
            }
            if tokio::time::Instant::now() >= deadline {
                return Ok(cur);
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    // ------------------------------------------------------------------
    // conflicts
    // ------------------------------------------------------------------

    /// Resolves one conflicted file (staged in the integration worktree) or
    /// asks the model for a suggestion (stored, not applied).
    pub async fn resolve_conflict(&self, params: MergeResolveConflictParams) -> OrchResult<MergeManagerProposal> {
        let project_id = self.inner.store.get(params.proposal_id).await?.project_id;
        // Staging a resolution and recording it happen under the project
        // lock, on a proposal loaded under it: concurrent resolutions of
        // different files must not overwrite each other's `resolved` flag.
        // (The AI suggestion runs unlocked and takes the lock to record.)
        let lock = self.project_lock(project_id);
        let guard = if matches!(params.resolution, MergeResolution::Ai) { None } else { Some(lock.lock().await) };
        let mut p = self.inner.store.get(params.proposal_id).await?;
        if p.status != MergeProposalStatus::Conflicted {
            return Err(OrchestratorError::Conflict(format!(
                "the proposal is {:?}, not stopped on conflicts",
                p.status
            )));
        }
        let exec = p.execution.clone().unwrap_or_default();
        let idx = exec
            .conflicts
            .iter()
            .position(|c| c.path == params.path)
            .ok_or_else(|| OrchestratorError::NotFound(format!("conflicted file {}", params.path)))?;
        let wt = PathBuf::from(exec.integration_worktree.clone().unwrap_or_default());
        let conflict = exec.conflicts[idx].clone();
        if let MergeResolution::Ai = params.resolution {
            if conflict.binary {
                return Err(OrchestratorError::Invalid("binary files cannot be resolved by the AI".into()));
            }
            let project = self.project(p.project_id).await?;
            let merged: Vec<String> =
                p.branches.iter().filter(|b| b.state == MergeBranchState::Merged).map(|b| b.branch.clone()).collect();
            let theirs = exec.conflict_branch.clone().unwrap_or_default();
            let (content, note) = self.suggest(&project, &conflict, &merged.join(", "), &theirs).await?;
            let _guard = lock.lock().await;
            let mut p2 = self.inner.store.get(p.id).await?;
            if p2.status != MergeProposalStatus::Conflicted {
                return Err(OrchestratorError::Conflict("the merge changed while the AI was working".into()));
            }
            if let Some(c) = p2.execution.as_mut().and_then(|e| e.conflicts.iter_mut().find(|c| c.path == params.path))
            {
                c.suggestion = Some(content);
                c.suggestion_note = note;
            }
            self.save(&mut p2).await;
            return Ok(p2);
        }
        let _guard = guard;
        let git = Arc::clone(self.git());
        match params.resolution {
            MergeResolution::Ours | MergeResolution::Theirs => {
                let flag = if params.resolution == MergeResolution::Ours { "--ours" } else { "--theirs" };
                let stage = if flag == "--ours" { 2 } else { 3 };
                // An already resolved path has only stage 0 in the index:
                // restore its conflicted stages (git's resolve-undo record)
                // first, or every side would look deleted.
                if conflict.resolved {
                    git.runner()
                        .run_ok(
                            &wt,
                            ["update-index", "--unresolve", "--", conflict.path.as_str()],
                            orkestr_vcs::RunOptions::write(),
                        )
                        .await
                        .map_err(vcs_err)?;
                }
                let missing = |r: &Result<BlobText, _>| matches!(r, Ok(BlobText::Missing));
                let ours = git.stage_text(&wt, 2, &conflict.path, 1).await;
                let theirs = git.stage_text(&wt, 3, &conflict.path, 1).await;
                if missing(&ours) && missing(&theirs) {
                    // Not an unmerged path any more (and nothing to restore):
                    // never read that as "this side deleted the file".
                    return Err(OrchestratorError::Conflict(format!(
                        "{} is no longer conflicted in the integration worktree; save its content instead",
                        conflict.path
                    )));
                }
                let exists = !missing(if stage == 2 { &ours } else { &theirs });
                if exists {
                    git.runner()
                        .run_ok(&wt, ["checkout", flag, "--", conflict.path.as_str()], orkestr_vcs::RunOptions::write())
                        .await
                        .map_err(vcs_err)?;
                    git.runner()
                        .run_ok(&wt, ["add", "--", conflict.path.as_str()], orkestr_vcs::RunOptions::write())
                        .await
                        .map_err(vcs_err)?;
                } else {
                    git.stage_resolution(&wt, &conflict.path, None).await.map_err(vcs_err)?;
                }
            }
            MergeResolution::Content { content } => {
                if content.len() > MAX_CONFLICT_TEXT * 4 {
                    return Err(OrchestratorError::Invalid("the resolved content is too large".into()));
                }
                if resolve::has_conflict_markers(&content) {
                    return Err(OrchestratorError::Invalid(
                        "the resolved content still contains conflict markers".into(),
                    ));
                }
                git.stage_resolution(&wt, &conflict.path, Some(&content)).await.map_err(vcs_err)?;
            }
            MergeResolution::Ai => {}
        }
        let fresh = std::fs::read(wt.join(&conflict.path)).ok();
        if let Some(c) = p.execution.as_mut().and_then(|e| e.conflicts.get_mut(idx)) {
            c.resolved = true;
            c.result = fresh.and_then(|b| {
                (!b.iter().take(8000).any(|x| *x == 0)).then(|| cap_text(String::from_utf8_lossy(&b).into_owned()))
            });
        }
        self.save(&mut p).await;
        Ok(p)
    }

    async fn model_chain(&self, project: &Project) -> OrchResult<Vec<ModelRef>> {
        let models = self
            .inner
            .deps
            .models
            .as_ref()
            .ok_or_else(|| OrchestratorError::NoModel("no model gateway is configured".into()))?;
        let cfg = match &self.inner.deps.hub {
            Some(h) => h.manager_config().await.unwrap_or_default(),
            None => orkestr_domain::ManagerConfig::default(),
        };
        let usable = |m: &ModelRef| {
            models.provider(m.provider_id).is_some_and(|i| {
                i.enabled && i.chat_capable() && project.effective_privacy().allows_provider(m.provider_id, i.is_local)
            })
        };
        let mut chain: Vec<ModelRef> = Vec::new();
        for m in cfg.model.iter().chain(cfg.fallback_chain.iter()) {
            if usable(m) && !chain.contains(m) {
                chain.push(m.clone());
            }
        }
        if chain.is_empty() {
            let routed = models
                .route(&RouteRequest {
                    purpose: Some("Merge".into()),
                    required: RequiredCapabilities::default(),
                    preferred: None,
                    fallback_chain: Vec::new(),
                    policy: project.effective_privacy().ai,
                    max_cost_usd: None,
                    est_input_tokens: 8_000,
                    max_output_tokens: u64::from(self.inner.deps.config.max_output_tokens),
                    exclude: Vec::new(),
                })
                .await
                .map_err(OrchestratorError::NoModel)?;
            chain.extend(routed.into_iter().filter(|m| usable(m)));
        }
        if chain.is_empty() {
            return Err(OrchestratorError::NoModel(
                "no chat model is available for conflict suggestions — choose one in Settings → AI Manager".into(),
            ));
        }
        Ok(chain)
    }

    async fn suggest(
        &self,
        project: &Project,
        conflict: &MergeFileConflict,
        ours: &str,
        theirs: &str,
    ) -> OrchResult<(String, Option<String>)> {
        let chain = self.model_chain(project).await?;
        let models =
            self.inner.deps.models.clone().ok_or_else(|| OrchestratorError::NoModel("no model gateway".into()))?;
        let head = chain[0].clone();
        let mut req =
            ChatRequest::new(head.model.clone(), vec![ChatMessage::user(resolve::user_prompt(conflict, ours, theirs))]);
        req.system = Some(resolve::system_prompt());
        req.response_format = ResponseFormat::JsonObject;
        req.max_output_tokens = Some(self.inner.deps.config.max_output_tokens);
        // Stop All AI / Stop project cancel the suggestion.
        let cancel = self
            .inner
            .deps
            .security
            .emergency
            .register_project(project.id)
            .map_err(|e| OrchestratorError::Forbidden(e.to_string()))?;
        let started = std::time::Instant::now();
        let mut stream = models.chat(chain, req, None, cancel.clone());
        let mut text = String::new();
        let mut usage = None;
        let mut model = head;
        let collect = async {
            while let Some(ev) = stream.next().await {
                match ev {
                    ChatEvent::TextDelta(t) => text.push_str(&t),
                    ChatEvent::Usage(u) => usage = Some(u),
                    ChatEvent::Switched { to, .. } => {
                        text.clear();
                        model = to;
                    }
                    ChatEvent::Done { .. } => return Ok(()),
                    ChatEvent::Error(e) => return Err(e.to_string()),
                    _ => {}
                }
            }
            Err("the model stream ended unexpectedly".to_owned())
        };
        let res = tokio::time::timeout(self.inner.deps.config.model_timeout, collect).await;
        cancel.cancel();
        let ok = matches!(res, Ok(Ok(())));
        if let (Some(cost), Some(u)) = (&self.inner.deps.cost, usage) {
            let priced = orkestr_agent_runtime::call_cost(models.as_ref(), &model, &u);
            let rec = UsageRecord {
                id: UsageRecordId::new(),
                ts: now(),
                provider_id: model.provider_id,
                model: model.model.clone(),
                project_id: Some(project.id),
                task_id: None,
                agent_id: None,
                execution_id: None,
                input_tokens: u.input_tokens,
                output_tokens: u.output_tokens,
                cached_input_tokens: u.cached_input_tokens,
                reasoning_tokens: u.reasoning_tokens,
                cost_usd: priced.spend_usd,
                api_equivalent_usd: priced.api_equivalent_usd,
                estimated: u.estimated || u.reported_cost_usd.is_none(),
                latency_ms: Some(u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX)),
                success: ok,
            };
            if let Err(e) = cost.ingest(&rec).await {
                tracing::warn!(error = %e, "merge suggestion usage ingest failed");
            }
        }
        match res {
            Err(_) => Err(OrchestratorError::Planning("the AI suggestion timed out".into())),
            Ok(Err(e)) => {
                Err(OrchestratorError::Planning(format!("the AI suggestion failed: {}", crate::util::redact(&e))))
            }
            Ok(Ok(())) => {
                let (content, note) = resolve::parse_reply(&text).map_err(OrchestratorError::Planning)?;
                Ok((content, note.map(|n| crate::util::redact(&n))))
            }
        }
    }
}

fn inputs_of(p: &MergeManagerProposal) -> Vec<MergeBranchInput> {
    let mut inputs: Vec<MergeBranchInput> =
        p.branches.iter().map(|b| MergeBranchInput { branch: b.branch.clone(), task_id: b.task_id }).collect();
    // Branches that were missing at analysis time are retried too.
    for b in &p.blockers {
        if b.kind == MergeBlockerKind::BranchMissing {
            if let Some(name) = &b.branch {
                if !inputs.iter().any(|i| &i.branch == name) {
                    inputs.push(MergeBranchInput { branch: name.clone(), task_id: None });
                }
            }
        }
    }
    inputs
}

fn cap_text(mut s: String) -> String {
    if s.len() > MAX_CONFLICT_TEXT {
        let mut end = MAX_CONFLICT_TEXT;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
    }
    s
}

async fn conflict_file(git: &GitCli, wt: &Path, path: &str) -> MergeFileConflict {
    let side = |b: Result<BlobText, VcsError>| -> (Option<String>, bool) {
        match b {
            Ok(BlobText::Text(t)) => (Some(cap_text(t)), false),
            Ok(BlobText::Binary) | Ok(BlobText::TooLarge) => (None, true),
            _ => (None, false),
        }
    };
    let (base, b1) = side(git.stage_text(wt, 1, path, MAX_CONFLICT_TEXT).await);
    let (ours, b2) = side(git.stage_text(wt, 2, path, MAX_CONFLICT_TEXT).await);
    let (theirs, b3) = side(git.stage_text(wt, 3, path, MAX_CONFLICT_TEXT).await);
    let raw = std::fs::read(wt.join(path)).ok();
    let binary = b1 || b2 || b3 || raw.as_ref().is_some_and(|r| r.iter().take(8000).any(|b| *b == 0));
    let result = if binary { None } else { raw.map(|r| cap_text(String::from_utf8_lossy(&r).into_owned())) };
    MergeFileConflict {
        path: path.to_owned(),
        base,
        ours,
        theirs,
        result,
        suggestion: None,
        suggestion_note: None,
        binary,
        resolved: false,
    }
}
