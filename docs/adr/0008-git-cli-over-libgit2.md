# ADR-0008: Git through the `git` CLI, not libgit2/gitoxide

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1
- **Spec:** §1.7, §16, §25, §40 ("Git as a separate layer"), §45 (`GitProvider`), §61 (Git worktree spike)
- **Spike:** SP-10 · **Risk:** R-05

## Context

Orkestr needs status, diff, branches, log, worktree add/list/remove/prune, commit on agent branches, merge preview with conflict detection, merge, and in later phases push/fetch with the user's credentials. It must behave exactly like the user's git: same hooks, same config, same credential helpers (GCM on Windows), same `core.autocrlf`, LFS and sparse checkout.

## Decision drivers

1. Exact parity with the user's git: hooks, credential helpers, LFS, `includeIf`, fsmonitor.
2. Full worktree support, plus `merge-tree --write-tree` for conflict prediction without touching any worktree.
3. No native C dependency that complicates Windows ARM64/macOS cross builds.
4. Easy to debug: every operation can be reproduced from a logged command line.

## Considered options

| Option | Verdict |
|---|---|
| **`git` CLI (≥ 2.38) via `tokio::process`, porcelain/`-z` parsing** | **Chosen** |
| libgit2 (`git2` crate) | Rejected. Partial worktree support, no hooks, no credential-helper parity (GCM), no LFS, `merge-tree` semantics differ, and an OpenSSL/libssh2 build burden |
| gitoxide (`gix`) | Promising, but as of 2026 its worktree mutation, merge and push coverage do not reach CLI parity. Allowed **only** for read-only hot paths (e.g. fast `status` on huge repos) behind the same trait, if SP-10 shows the CLI is too slow |
| Bundled git (MinGit) | Optional fallback for Windows users without git. Onboarding step 7 detects git and offers a download link. Bundling is deferred |

## Decision

`orkestr-vcs` implements `GitProvider` (named in §45) through `GitCli`:

```rust
#[async_trait::async_trait]
pub trait GitProvider: Send + Sync + 'static {
    async fn version(&self) -> Result<GitVersion, VcsError>;                       // must be >= 2.38
    async fn repo_root(&self, path: &Path) -> Result<PathBuf, VcsError>;
    async fn status(&self, repo: &Path) -> Result<RepoStatus, VcsError>;           // status --porcelain=v2 -z --branch
    async fn diff(&self, repo: &Path, spec: &DiffSpec) -> Result<Vec<FileDiff>, VcsError>; // diff -z --numstat + --patch
    async fn log(&self, repo: &Path, q: &LogQuery) -> Result<Vec<CommitInfo>, VcsError>;
    async fn branches(&self, repo: &Path) -> Result<Vec<BranchInfo>, VcsError>;     // for-each-ref --format
    async fn worktree_add(&self, repo: &Path, req: &WorktreeAdd) -> Result<WorktreeInfo, VcsError>;
    async fn worktree_list(&self, repo: &Path) -> Result<Vec<WorktreeInfo>, VcsError>; // worktree list --porcelain -z
    async fn worktree_remove(&self, repo: &Path, path: &Path, force: bool) -> Result<(), VcsError>;
    async fn worktree_prune(&self, repo: &Path) -> Result<(), VcsError>;
    async fn commit(&self, worktree: &Path, req: &CommitRequest) -> Result<CommitId, VcsError>; // add -A -- <paths>; commit -F -
    async fn merge_preview(&self, repo: &Path, base: &str, head: &str) -> Result<MergePreview, VcsError>; // merge-tree --write-tree --name-only -z
    async fn merge(&self, repo: &Path, req: &MergeRequest) -> Result<MergeOutcome, VcsError>;
    async fn rev_parse(&self, repo: &Path, rev: &str) -> Result<CommitId, VcsError>;
}

pub struct MergePreview { pub clean: bool, pub tree: Option<String>, pub conflicts: Vec<ConflictEntry> }
pub struct CommitRequest { pub message: String, pub paths: Option<Vec<PathBuf>>, pub author: GitIdentity, pub trailers: Vec<(String, String)> }
// trailers: "Orkestr-Task: <taskId>", "Orkestr-Agent: <agentId>"
```

### Invocation rules

- The binary is resolved once through `PlatformAdapter::shell().which("git")`, or taken from the user setting `git.path`.
- Global args: `git --no-pager -c core.quotepath=off -c color.ui=never -c gc.auto=0 -C <dir> …`. `gc.auto=0` applies only to operations Orkestr starts. The user's own gc is untouched.
- Environment:
  - `GIT_TERMINAL_PROMPT=0` (no hanging prompts);
  - `GIT_OPTIONAL_LOCKS=0` for read operations (status does not take `index.lock`);
  - `LC_ALL=C` and `LANGUAGE=` for parseable messages;
  - `GIT_EDITOR=true`.
  - The user's `HOME` is inherited so credential helpers work.
- Every invocation is logged at `debug` with argv (redacted) and duration, plus a `correlationId`.
- Timeouts: 30 s for local operations by default, 10 min for network operations. On timeout the process tree is killed through `PlatformAdapter::process().kill_tree`.
- Output is parsed with `-z` NUL-separated formats only. Human porcelain is never parsed.
- **Per-repository mutation lock** (see ADR-0009): operations that write shared `.git` state (`worktree add/remove/prune`, `branch` create/delete, `merge` into the base, `fetch`, ref updates) take an async `Mutex` keyed by the canonical common dir (`git rev-parse --git-common-dir`). Commits inside separate worktrees run in parallel.
- **Minimum version 2.38** (`merge-tree --write-tree`). Onboarding step 7 checks it. With an older git, Orkestr runs with merge preview disabled and shows a warning.

## Consequences

- **Positive:** perfect parity with the user's git; LFS, hooks and credentials work; easy to debug.
- **Negative:** process spawn overhead of about 5–20 ms on Windows per call. Mitigated by batching (`for-each-ref` instead of N `rev-parse` calls), status caching invalidated by `FileChanged` events, and debouncing.
- **Negative:** git hooks run during agent commits. They may be slow or interactive. Agent commits pass `--no-verify` **only** if the project setting `git.agentCommitsSkipHooks` is true (default false), and quality gates (§58) run the project's checks explicitly.
- **Negative:** depends on a user-installed git. Onboarding step 7 handles detection.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Not implemented yet. `orkestr-vcs` contains only the skeleton trait `Git` (`status`, `diff`, `branches`, `worktrees`, `add_worktree`, `remove_worktree`) and `VcsError`. Git discovery is available through `orkestr_platform::ProcessControl::which("git")` (the platform trait is `process().which`, not `shell().which`). SP-10 has not run.
