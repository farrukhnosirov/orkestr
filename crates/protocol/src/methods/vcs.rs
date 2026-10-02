//! `vcs.*` — Git status, diffs, history, branches, worktrees and merges (§16).
//!
//! Every method addresses a repository through `projectId` (the project's
//! `rootPath`) and, optionally, `worktree`: the absolute path of one of the
//! repository's worktrees (agent worktrees, §16). Paths inside the repository
//! are always relative, `/`-separated.
//!
//! Mutating methods that can destroy uncommitted work (`vcs.discard`,
//! `vcs.deleteBranch` with `force`, `vcs.removeWorktree` with `force`) are
//! Dangerous (§23): the caller must pass `confirm: true` explicitly, and the
//! core takes a safety snapshot first where work could be lost.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use orkestr_domain::{AgentId, ProjectId, SnapshotId, TaskId, Timestamp};

use super::method;

/// Max commits returned by one `vcs.log` page.
pub const MAX_LOG_LIMIT: u32 = 500;
/// Default `vcs.log` page size.
pub const DEFAULT_LOG_LIMIT: u32 = 100;
/// Default per-file patch budget of `vcs.diff` (bytes).
pub const DEFAULT_MAX_PATCH_BYTES: u64 = 512 * 1024;
/// Max content returned by `vcs.show` (bytes).
pub const MAX_SHOW_BYTES: u64 = 8 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Shared params
// ---------------------------------------------------------------------------

/// Params addressing a repository (and optionally one of its worktrees).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoParams {
    /// Project whose `rootPath` is the repository.
    pub project_id: ProjectId,
    /// Absolute path of a worktree of that repository (default: the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
}

// ---------------------------------------------------------------------------
// Status
// ---------------------------------------------------------------------------

/// Change status of a path on one side (index or worktree).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum FileChangeStatus {
    /// No change on this side.
    Unmodified,
    /// Content changed.
    Modified,
    /// Newly added.
    Added,
    /// Deleted.
    Deleted,
    /// Renamed (see `origPath`).
    Renamed,
    /// Copied (see `origPath`).
    Copied,
    /// File type changed (e.g. file ↔ symlink).
    TypeChanged,
    /// Unmerged (conflict).
    Unmerged,
    /// Untracked.
    Untracked,
    /// Ignored.
    Ignored,
}

/// Kind of a merge conflict (porcelain v2 `XY` of an unmerged entry).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum ConflictKind {
    /// `DD`.
    BothDeleted,
    /// `AU`.
    AddedByUs,
    /// `UD`.
    DeletedByThem,
    /// `UA`.
    AddedByThem,
    /// `DU`.
    DeletedByUs,
    /// `AA`.
    BothAdded,
    /// `UU`.
    BothModified,
}

/// One entry of `git status`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct StatusEntry {
    /// Path relative to the worktree root.
    pub path: String,
    /// Source path of a rename/copy.
    pub orig_path: Option<String>,
    /// Status in the index (staged side).
    pub index: FileChangeStatus,
    /// Status in the worktree (unstaged side).
    pub worktree: FileChangeStatus,
    /// Conflict kind for unmerged entries.
    pub conflict: Option<ConflictKind>,
    /// Whether the entry is a submodule.
    pub submodule: bool,
}

impl StatusEntry {
    /// Has staged changes.
    #[must_use]
    pub fn is_staged(&self) -> bool {
        self.conflict.is_none()
            && !matches!(
                self.index,
                FileChangeStatus::Unmodified | FileChangeStatus::Untracked | FileChangeStatus::Ignored
            )
    }

    /// Has unstaged changes to a tracked file.
    #[must_use]
    pub fn is_unstaged(&self) -> bool {
        self.conflict.is_none()
            && !matches!(
                self.worktree,
                FileChangeStatus::Unmodified | FileChangeStatus::Untracked | FileChangeStatus::Ignored
            )
    }

    /// Is untracked.
    #[must_use]
    pub fn is_untracked(&self) -> bool {
        self.worktree == FileChangeStatus::Untracked
    }
}

/// Result of `vcs.status` and of mutating methods that return fresh status.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RepoStatus {
    /// Whether the directory is inside a git repository.
    pub is_repo: bool,
    /// Detected git version (`2.45.1`), `None` when git is missing.
    pub git_version: Option<String>,
    /// Absolute worktree root.
    pub root: Option<String>,
    /// Current branch (None when detached or not a repo).
    pub branch: Option<String>,
    /// HEAD commit (None before the first commit).
    pub head: Option<String>,
    /// HEAD is detached.
    pub detached: bool,
    /// Upstream branch, e.g. `origin/main`.
    pub upstream: Option<String>,
    /// Commits ahead of upstream.
    pub ahead: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// All changed/untracked/conflicted entries.
    pub entries: Vec<StatusEntry>,
    /// A merge is in progress (`MERGE_HEAD` exists).
    pub merge_in_progress: bool,
    /// A rebase is in progress.
    pub rebase_in_progress: bool,
    /// Nothing to commit and no untracked files.
    pub clean: bool,
}

// ---------------------------------------------------------------------------
// Diff
// ---------------------------------------------------------------------------

/// What to diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum DiffTarget {
    /// Index → worktree (unstaged changes).
    Unstaged,
    /// HEAD → index (staged changes).
    Staged,
    /// HEAD → worktree (all uncommitted changes).
    Head,
    /// The changes introduced by one commit (vs its first parent).
    Commit {
        /// Commit-ish.
        commit: String,
    },
    /// `from..to` (two-dot, tree comparison).
    Range {
        /// Base revision.
        from: String,
        /// Target revision.
        to: String,
    },
}

/// Params of `vcs.diff`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// What to compare.
    pub target: DiffTarget,
    /// Restrict to these paths.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub paths: Option<Vec<String>>,
    /// Context lines (default 3, max 100).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub context_lines: Option<u32>,
    /// For `Unstaged`/`Head`: also list untracked files as added.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub include_untracked: Option<bool>,
    /// Per-file patch budget in bytes (default 512 KiB).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub max_patch_bytes: Option<u64>,
}

/// One file of a diff.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct FileDiff {
    /// New path (or the deleted path).
    pub path: String,
    /// Old path for renames/copies.
    pub old_path: Option<String>,
    /// Change kind.
    pub status: FileChangeStatus,
    /// Added lines.
    pub additions: u32,
    /// Removed lines.
    pub deletions: u32,
    /// Binary file (no line stats / patch).
    pub binary: bool,
    /// Unified diff text of this file (`diff --git` header included).
    pub patch: String,
    /// The patch was cut at the byte budget.
    pub truncated: bool,
}

/// Result of `vcs.diff`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiffResult {
    /// Files in git order.
    pub files: Vec<FileDiff>,
    /// Sum of additions.
    pub additions: u32,
    /// Sum of deletions.
    pub deletions: u32,
}

// ---------------------------------------------------------------------------
// Log / show
// ---------------------------------------------------------------------------

/// Params of `vcs.log`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Start revision (default `HEAD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub rev: Option<String>,
    /// Only commits touching this path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub path: Option<String>,
    /// Commits to skip (pagination offset).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub skip: Option<u32>,
    /// Page size (default 100, max 500).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub limit: Option<u32>,
}

/// A commit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitInfo {
    /// Full object id.
    pub id: String,
    /// Abbreviated id.
    pub short_id: String,
    /// Parent ids.
    pub parents: Vec<String>,
    /// Author name.
    pub author_name: String,
    /// Author email.
    pub author_email: String,
    /// Author date (ISO 8601 / RFC 3339).
    pub author_date: String,
    /// Committer date (ISO 8601 / RFC 3339).
    pub committer_date: String,
    /// First line of the message.
    pub subject: String,
    /// Rest of the message.
    pub body: String,
    /// Decorations (`HEAD -> main`, `origin/main`, `tag: v1`).
    pub refs: Vec<String>,
}

/// Result of `vcs.log`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct LogResult {
    /// Commits, newest first.
    pub commits: Vec<CommitInfo>,
    /// More commits exist after this page.
    pub has_more: bool,
}

/// Which version of a file `vcs.show` returns.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(tag = "kind", rename_all_fields = "camelCase")]
#[ts(export)]
pub enum FileRevision {
    /// Committed version at HEAD.
    Head,
    /// Staged version (index stage 0).
    Index,
    /// Current file on disk.
    Worktree,
    /// Version at a revision (`abc123`, `main~2`, `abc123^`).
    Commit {
        /// Revision.
        rev: String,
    },
}

/// Params of `vcs.show`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShowParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Path relative to the worktree root.
    pub path: String,
    /// Version.
    pub rev: FileRevision,
}

/// Result of `vcs.show`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct ShowResult {
    /// Path.
    pub path: String,
    /// Whether the file exists in that version.
    pub exists: bool,
    /// Binary content (then `content` is empty).
    pub binary: bool,
    /// UTF-8 (lossy) text content.
    pub content: String,
    /// Size in bytes.
    pub size: u64,
    /// Content was cut at 8 MiB.
    pub truncated: bool,
}

// ---------------------------------------------------------------------------
// Branches
// ---------------------------------------------------------------------------

/// A local or remote-tracking branch.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchInfo {
    /// Short name (`main`, `origin/main`).
    pub name: String,
    /// Full ref (`refs/heads/main`).
    pub full_ref: String,
    /// Remote-tracking branch.
    pub remote: bool,
    /// Checked out in the addressed worktree.
    pub current: bool,
    /// Tip commit.
    pub commit: String,
    /// Upstream (`origin/main`).
    pub upstream: Option<String>,
    /// Commits ahead of upstream.
    pub ahead: u32,
    /// Commits behind upstream.
    pub behind: u32,
    /// Upstream is configured but gone.
    pub gone: bool,
    /// Worktree where this branch is checked out (any worktree of the repo).
    pub worktree_path: Option<String>,
    /// Tip subject.
    pub subject: String,
    /// Tip committer date (ISO 8601).
    pub committer_date: Option<String>,
}

/// Result of `vcs.branches`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchesResult {
    /// Current branch of the addressed worktree.
    pub current: Option<String>,
    /// Local branches first, then remote-tracking ones; each sorted by name.
    pub branches: Vec<BranchInfo>,
}

/// Params of `vcs.createBranch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CreateBranchParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// New branch name (validated with `git check-ref-format --branch`).
    pub name: String,
    /// Start point (default `HEAD`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub start_point: Option<String>,
    /// Switch to the new branch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub checkout: Option<bool>,
}

/// Result of `vcs.createBranch` / `vcs.checkout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct BranchResult {
    /// Branch name.
    pub branch: String,
    /// Tip commit.
    pub commit: String,
}

/// Params of `vcs.deleteBranch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteBranchParams {
    /// Project.
    pub project_id: ProjectId,
    /// Local branch name.
    pub name: String,
    /// Delete even when not merged (`-D`). Dangerous: requires `confirm`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub force: Option<bool>,
    /// Explicit confirmation for `force`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirm: Option<bool>,
}

/// Result of `vcs.deleteBranch`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DeleteBranchResult {
    /// Deleted branch.
    pub name: String,
    /// Its former tip (for manual recovery).
    pub commit: String,
}

/// Params of `vcs.checkout`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CheckoutParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Branch to switch to (a remote branch `origin/x` creates tracking `x`).
    pub branch: String,
}

// ---------------------------------------------------------------------------
// Index / commit
// ---------------------------------------------------------------------------

/// Params of `vcs.stage` / `vcs.unstage`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct PathsParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Paths (relative). Ignored when `all` is true.
    #[serde(default)]
    pub paths: Vec<String>,
    /// Apply to every changed path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub all: Option<bool>,
}

/// Params of `vcs.discard` (Dangerous).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscardParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Paths whose unstaged changes are discarded; untracked paths are deleted.
    pub paths: Vec<String>,
    /// Must be `true`: discarding destroys uncommitted work.
    #[serde(default)]
    pub confirm: bool,
}

/// Result of `vcs.discard`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct DiscardResult {
    /// Paths reverted to the index version.
    pub restored: Vec<String>,
    /// Untracked paths deleted.
    pub deleted: Vec<String>,
    /// Safety snapshot taken before discarding (restore from it to undo).
    pub safety_snapshot_id: Option<SnapshotId>,
    /// Fresh status.
    pub status: RepoStatus,
}

/// A commit trailer (`Orkestr-Task: <id>`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitTrailer {
    /// Token, e.g. `Orkestr-Task`.
    pub key: String,
    /// Value.
    pub value: String,
}

/// Params of `vcs.commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Commit message (author/committer come from git config).
    pub message: String,
    /// Allow a commit without staged changes.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub allow_empty: Option<bool>,
    /// Trailers appended to the message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub trailers: Option<Vec<CommitTrailer>>,
}

/// Result of `vcs.commit`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct CommitResult {
    /// The new commit.
    pub commit: CommitInfo,
}

// ---------------------------------------------------------------------------
// Worktrees
// ---------------------------------------------------------------------------

/// A git worktree, enriched with Orkestr metadata for agent worktrees.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorktreeInfo {
    /// Absolute path.
    pub path: String,
    /// HEAD commit.
    pub head: Option<String>,
    /// Checked out branch (short name).
    pub branch: Option<String>,
    /// Detached HEAD.
    pub detached: bool,
    /// Bare repository entry.
    pub bare: bool,
    /// Locked (`git worktree lock`).
    pub locked: bool,
    /// Git considers it prunable (directory missing).
    pub prunable: bool,
    /// The main worktree (the user's checkout).
    pub is_main: bool,
    /// Created and recorded by Orkestr.
    pub managed: bool,
    /// Owning agent.
    pub agent_id: Option<AgentId>,
    /// Task it was created for.
    pub task_id: Option<TaskId>,
    /// Agent role slug.
    pub role: Option<String>,
    /// Base commit at creation.
    pub base_commit: Option<String>,
    /// Creation time (managed worktrees).
    pub created_at: Option<Timestamp>,
    /// Has uncommitted changes (None when unknown/missing).
    pub dirty: Option<bool>,
}

/// Result of `vcs.worktrees`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorktreesResult {
    /// Main worktree first.
    pub worktrees: Vec<WorktreeInfo>,
}

/// Params of `vcs.addWorktree`.
///
/// With `agentId`: branch `agent/<role>-<last 8 hex of agentId>` (role default
/// `agent`), path `<data_dir>/worktrees/<project short>/<agent short>`.
/// Without: `branch` is required and becomes the new branch name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct AddWorktreeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Agent owning the worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub agent_id: Option<AgentId>,
    /// Agent role (slugified, ≤ 24 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub role: Option<String>,
    /// Task the worktree is for.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub task_id: Option<TaskId>,
    /// Explicit new branch name (required without `agentId`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub branch: Option<String>,
    /// Base revision (default: HEAD of the project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub base: Option<String>,
}

/// Result of `vcs.addWorktree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct WorktreeResult {
    /// The new worktree.
    pub worktree: WorktreeInfo,
}

/// Params of `vcs.removeWorktree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveWorktreeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Absolute worktree path.
    pub path: String,
    /// Remove even with uncommitted changes (Dangerous: requires `confirm`;
    /// a safety snapshot is taken first).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub force: Option<bool>,
    /// Explicit confirmation for `force`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub confirm: Option<bool>,
}

/// Result of `vcs.removeWorktree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct RemoveWorktreeResult {
    /// Removed path.
    pub path: String,
    /// Safety snapshot of the worktree taken before a forced removal.
    pub safety_snapshot_id: Option<SnapshotId>,
}

// ---------------------------------------------------------------------------
// Merge
// ---------------------------------------------------------------------------

/// Params of `vcs.mergePreview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergePreviewParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root); its branch is the default target.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Branch/revision to merge.
    pub source: String,
    /// Branch to merge into (default: current branch of the worktree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
}

/// A conflicting path predicted by `git merge-tree`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeConflict {
    /// Path.
    pub path: String,
    /// Conflict types reported by git (`CONFLICT (contents)`, …).
    pub kinds: Vec<String>,
    /// Messages for this path.
    pub messages: Vec<String>,
}

/// Result of `vcs.mergePreview` (touches no worktree).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergePreview {
    /// Source revision as given.
    pub source: String,
    /// Target branch.
    pub target: String,
    /// Source commit.
    pub source_commit: String,
    /// Target commit.
    pub target_commit: String,
    /// Merge base.
    pub merge_base: Option<String>,
    /// No conflicts.
    pub clean: bool,
    /// Resulting tree when clean.
    pub tree: Option<String>,
    /// Conflicting paths.
    pub conflicts: Vec<MergeConflict>,
    /// Informational messages (auto-merges etc.).
    pub messages: Vec<String>,
    /// Commits in source not yet in target.
    pub commits_to_merge: u32,
    /// Source is already contained in target.
    pub up_to_date: bool,
}

/// Params of `vcs.merge` (`--no-ff`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeParams {
    /// Project.
    pub project_id: ProjectId,
    /// Worktree path (default: project root).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub worktree: Option<String>,
    /// Branch/revision to merge.
    pub source: String,
    /// Target branch (default: current branch of the worktree).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<String>,
    /// Merge commit message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub message: Option<String>,
    /// Start the merge even when conflicts are predicted (leaves the worktree
    /// in a merging state with conflict markers; only for a checked-out target).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub allow_conflicts: Option<bool>,
}

/// Outcome of `vcs.merge`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[ts(export)]
pub enum MergeStatus {
    /// A merge commit was created.
    Merged,
    /// Nothing to merge.
    UpToDate,
    /// Conflicts: the worktree is now in a merging state.
    Conflicted,
}

/// Result of `vcs.merge`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct MergeResult {
    /// Outcome.
    pub status: MergeStatus,
    /// Merge commit.
    pub commit: Option<String>,
    /// Conflicting paths (when `Conflicted`).
    pub conflicts: Vec<String>,
    /// Merged without a checkout: the target ref was updated with
    /// compare-and-swap because it is not checked out anywhere.
    pub ref_only: bool,
}

/// Params of `vcs.init`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(export)]
pub struct InitParams {
    /// Project (its `rootPath` is initialised).
    pub project_id: ProjectId,
    /// Initial branch name (default: git's configured default).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub initial_branch: Option<String>,
}

// ---------------------------------------------------------------------------
// Methods
// ---------------------------------------------------------------------------

method!(
    /// `vcs.status` — status of the project repository (or one of its worktrees).
    Status, "vcs.status", RepoParams, RepoStatus
);
method!(
    /// `vcs.diff` — worktree/staged/commit/range diffs with per-file stats.
    Diff, "vcs.diff", DiffParams, DiffResult
);
method!(
    /// `vcs.log` — paginated history.
    Log, "vcs.log", LogParams, LogResult
);
method!(
    /// `vcs.show` — one file at a revision, in the index or on disk.
    Show, "vcs.show", ShowParams, ShowResult
);
method!(
    /// `vcs.branches` — local and remote-tracking branches.
    Branches, "vcs.branches", RepoParams, BranchesResult
);
method!(
    /// `vcs.createBranch`.
    CreateBranch, "vcs.createBranch", CreateBranchParams, BranchResult
);
method!(
    /// `vcs.deleteBranch` — safe delete (`-d`); `force` needs `confirm`.
    DeleteBranch, "vcs.deleteBranch", DeleteBranchParams, DeleteBranchResult
);
method!(
    /// `vcs.checkout` — switch branch (git refuses to overwrite local changes).
    Checkout, "vcs.checkout", CheckoutParams, BranchResult
);
method!(
    /// `vcs.stage`.
    Stage, "vcs.stage", PathsParams, RepoStatus
);
method!(
    /// `vcs.unstage`.
    Unstage, "vcs.unstage", PathsParams, RepoStatus
);
method!(
    /// `vcs.discard` — Dangerous; `confirm: true` required; safety snapshot first.
    Discard, "vcs.discard", DiscardParams, DiscardResult
);
method!(
    /// `vcs.commit` — commits the index (never automatic, §64).
    Commit, "vcs.commit", CommitParams, CommitResult
);
method!(
    /// `vcs.worktrees`.
    Worktrees, "vcs.worktrees", RepoParams, WorktreesResult
);
method!(
    /// `vcs.addWorktree` — new branch + worktree (agent naming when `agentId`).
    AddWorktree, "vcs.addWorktree", AddWorktreeParams, WorktreeResult
);
method!(
    /// `vcs.removeWorktree` — refuses dirty worktrees unless `force` + `confirm`.
    RemoveWorktree, "vcs.removeWorktree", RemoveWorktreeParams, RemoveWorktreeResult
);
method!(
    /// `vcs.mergePreview` — conflict detection via `git merge-tree --write-tree`.
    MergePreviewMethod, "vcs.mergePreview", MergePreviewParams, MergePreview
);
method!(
    /// `vcs.merge` — `--no-ff` merge of `source` into `target`.
    Merge, "vcs.merge", MergeParams, MergeResult
);
method!(
    /// `vcs.abortMerge` — `git merge --abort`.
    AbortMerge, "vcs.abortMerge", RepoParams, RepoStatus
);
method!(
    /// `vcs.init` — `git init` in the project root.
    Init, "vcs.init", InitParams, RepoStatus
);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diff_target_json_shape() {
        let t: DiffTarget = serde_json::from_str(r#"{"kind":"Commit","commit":"abc"}"#).unwrap();
        assert_eq!(t, DiffTarget::Commit { commit: "abc".into() });
        let t: DiffTarget = serde_json::from_str(r#"{"kind":"Range","from":"a","to":"b"}"#).unwrap();
        assert_eq!(t, DiffTarget::Range { from: "a".into(), to: "b".into() });
        assert_eq!(serde_json::to_string(&DiffTarget::Staged).unwrap(), r#"{"kind":"Staged"}"#);
    }

    #[test]
    fn discard_defaults_to_unconfirmed() {
        let p: DiscardParams =
            serde_json::from_str(r#"{"projectId":"0190f0b0-0000-7000-8000-000000000001","paths":["a"]}"#).unwrap();
        assert!(!p.confirm);
    }

    #[test]
    fn entry_flags() {
        let e = StatusEntry {
            path: "a".into(),
            orig_path: None,
            index: FileChangeStatus::Modified,
            worktree: FileChangeStatus::Modified,
            conflict: None,
            submodule: false,
        };
        assert!(e.is_staged() && e.is_unstaged() && !e.is_untracked());
    }
}
