//! [`GitCli`] — every git operation Orkestr performs, through the `git` CLI.
//!
//! Read-only operations take no Orkestr lock. Operations that write shared
//! `.git` state (branch create/delete, worktree add/remove/prune, merges,
//! checkout) take the per-repository async mutex ([`RepoLocks`], keyed by the
//! canonical common dir) — ADR-0008/0009. Index writes additionally rely on
//! git's own `index.lock`, retried by the runner.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use dashmap::DashMap;
use orkestr_protocol::methods::vcs::{
    BranchResult, BranchesResult, CommitInfo, CommitTrailer, DeleteBranchResult, DiffResult, DiffTarget,
    FileChangeStatus, FileDiff, FileRevision, LogResult, MergePreview, MergeResult, MergeStatus, RepoStatus,
    ShowResult, StatusEntry, MAX_SHOW_BYTES,
};
use tokio::sync::{Mutex, OnceCell, OwnedMutexGuard};

use crate::error::{VcsError, VcsResult};
use crate::naming::{path_key, validate_rel_path, validate_rev};
use crate::parse::{self, GitVersion, RawWorktree};
use crate::runner::{command_error, GitOutput, GitRunner, RunOptions};

/// Per-repository mutation locks keyed by the canonical git common dir.
#[derive(Debug, Default)]
pub struct RepoLocks {
    locks: DashMap<String, Arc<Mutex<()>>>,
}

impl RepoLocks {
    /// Acquires the lock of `key`.
    pub async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let m = self.locks.entry(key.to_owned()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        m.lock_owned().await
    }
}

/// Repository locations of a directory.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RepoInfo {
    /// Worktree root (`--show-toplevel`).
    pub root: PathBuf,
    /// Shared `.git` dir of all worktrees (`--git-common-dir`), absolute.
    pub common_dir: PathBuf,
}

/// A diff request (see `vcs.diff`).
#[derive(Debug, Clone)]
pub struct DiffRequest {
    /// What to compare.
    pub target: DiffTarget,
    /// Path filter (validated relative paths).
    pub paths: Vec<String>,
    /// Context lines.
    pub context_lines: u32,
    /// Add untracked files as new files (Unstaged/Head).
    pub include_untracked: bool,
    /// Per-file patch budget.
    pub max_patch_bytes: usize,
}

/// Git through the CLI.
#[derive(Debug)]
pub struct GitCli {
    runner: GitRunner,
    version: OnceCell<Option<GitVersion>>,
    locks: RepoLocks,
}

fn resolve_against(dir: &Path, p: &str) -> PathBuf {
    let pb = PathBuf::from(p.trim());
    if pb.is_absolute() {
        pb
    } else {
        dir.join(pb)
    }
}

fn is_binary(bytes: &[u8]) -> bool {
    bytes.iter().take(8000).any(|b| *b == 0)
}

fn validate_paths(paths: &[String]) -> VcsResult<Vec<String>> {
    paths.iter().map(|p| validate_rel_path(p)).collect()
}

fn entry_matches(entry: &StatusEntry, p: &str) -> bool {
    entry.path == p || entry.path.starts_with(&format!("{p}/"))
}

impl GitCli {
    /// Wraps a runner.
    #[must_use]
    pub fn new(runner: GitRunner) -> Self {
        Self { runner, version: OnceCell::new(), locks: RepoLocks::default() }
    }

    /// Git found on PATH.
    #[must_use]
    pub fn discover() -> Self {
        Self::new(GitRunner::discover())
    }

    /// Whether a git binary was found.
    #[must_use]
    pub fn is_available(&self) -> bool {
        self.runner.git_path().is_some()
    }

    /// The runner (for advanced callers).
    #[must_use]
    pub fn runner(&self) -> &GitRunner {
        &self.runner
    }

    /// Installed git version (cached).
    pub async fn version(&self) -> VcsResult<GitVersion> {
        let v = self
            .version
            .get_or_try_init(|| async {
                let out = self.runner.run_ok(Path::new("."), ["--version"], RunOptions::read()).await?;
                Ok::<_, VcsError>(parse::parse_version(&out.text()))
            })
            .await?;
        v.ok_or_else(|| VcsError::Unsupported("could not parse `git --version`".into()))
    }

    async fn require_merge_tree(&self) -> VcsResult<()> {
        let v = self.version().await?;
        if v.at_least(GitVersion::MERGE_TREE_WRITE_TREE) {
            Ok(())
        } else {
            Err(VcsError::Unsupported(format!("merge preview needs git 2.38 or newer (found {v}); please update Git")))
        }
    }

    async fn run(&self, dir: &Path, args: &[&str], opts: RunOptions<'_>) -> VcsResult<GitOutput> {
        self.runner.run(dir, args, opts).await
    }

    async fn run_ok(&self, dir: &Path, args: &[&str], opts: RunOptions<'_>) -> VcsResult<GitOutput> {
        self.runner.run_ok(dir, args, opts).await
    }

    /// Repository root and common dir of `dir`; `None` when not in a repository.
    pub async fn repo_info(&self, dir: &Path) -> VcsResult<Option<RepoInfo>> {
        if !dir.is_dir() {
            return Err(VcsError::NotFound(format!("directory {} does not exist", dir.display())));
        }
        let out = self.run(dir, &["rev-parse", "--show-toplevel", "--git-common-dir"], RunOptions::read()).await?;
        if !out.success() {
            if out.stderr.contains("not a git repository") {
                return Ok(None);
            }
            return Err(command_error(&["rev-parse".into()], &out));
        }
        let text = out.text();
        let mut lines = text.lines();
        let root = lines.next().unwrap_or_default().trim();
        let common = lines.next().unwrap_or_default().trim();
        if root.is_empty() {
            // e.g. inside a bare repository or the .git dir itself
            return Ok(None);
        }
        Ok(Some(RepoInfo { root: PathBuf::from(root), common_dir: resolve_against(dir, common) }))
    }

    /// Like [`GitCli::repo_info`] but failing with `NotARepository`.
    pub async fn require_repo(&self, dir: &Path) -> VcsResult<RepoInfo> {
        self.repo_info(dir).await?.ok_or_else(|| VcsError::NotARepository(dir.display().to_string()))
    }

    /// Takes the repository mutation lock of the repo containing `dir`.
    pub async fn lock_repo(&self, dir: &Path) -> VcsResult<OwnedMutexGuard<()>> {
        let info = self.require_repo(dir).await?;
        Ok(self.locks.lock(&path_key(&info.common_dir)).await)
    }

    /// `rev-parse --verify <rev>^{commit}`; `None` when it does not resolve.
    pub async fn rev_parse(&self, dir: &Path, rev: &str) -> VcsResult<Option<String>> {
        let rev = validate_rev(rev)?;
        let spec = format!("{rev}^{{commit}}");
        let out =
            self.run(dir, &["rev-parse", "--verify", "--quiet", "--end-of-options", &spec], RunOptions::read()).await?;
        Ok(out.success().then(|| out.line()).filter(|s| !s.is_empty()))
    }

    async fn has_head(&self, dir: &Path) -> VcsResult<bool> {
        Ok(self.rev_parse(dir, "HEAD").await?.is_some())
    }

    async fn ref_exists(&self, dir: &Path, full_ref: &str) -> VcsResult<bool> {
        let out = self.run(dir, &["show-ref", "--verify", "--quiet", full_ref], RunOptions::read()).await?;
        Ok(out.success())
    }

    async fn empty_tree(&self, dir: &Path) -> VcsResult<String> {
        let opts = RunOptions { stdin: Some(b""), read_only: true, timeout: None };
        Ok(self.run_ok(dir, &["hash-object", "-t", "tree", "--stdin"], opts).await?.line())
    }

    /// Current branch (None when detached / unborn handled as the unborn name).
    pub async fn current_branch(&self, dir: &Path) -> VcsResult<Option<String>> {
        let out = self.run(dir, &["symbolic-ref", "--quiet", "--short", "HEAD"], RunOptions::read()).await?;
        Ok(out.success().then(|| out.line()).filter(|s| !s.is_empty()))
    }

    /// Validates a branch name with `git check-ref-format --branch`.
    pub async fn check_branch_name(&self, dir: &Path, name: &str) -> VcsResult<String> {
        let name = validate_rev(name)?;
        let out = self.run(dir, &["check-ref-format", "--branch", &name], RunOptions::read()).await?;
        if out.success() && out.line() == name {
            Ok(name)
        } else {
            Err(VcsError::invalid(format!("`{name}` is not a valid branch name")))
        }
    }

    // ---------------------------------------------------------------------
    // status
    // ---------------------------------------------------------------------

    /// `git status` of the worktree containing `dir`.
    pub async fn status(&self, dir: &Path) -> VcsResult<RepoStatus> {
        let git_version = self.version().await.ok().map(|v| v.to_string());
        let out = self
            .run(
                dir,
                &[
                    "rev-parse",
                    "--show-toplevel",
                    "--git-path",
                    "MERGE_HEAD",
                    "--git-path",
                    "rebase-merge",
                    "--git-path",
                    "rebase-apply",
                ],
                RunOptions::read(),
            )
            .await?;
        if !out.success() {
            if out.stderr.contains("not a git repository") {
                return Ok(RepoStatus { is_repo: false, git_version, ..RepoStatus::default() });
            }
            return Err(command_error(&["rev-parse".into()], &out));
        }
        let text = out.text();
        let lines: Vec<&str> = text.lines().map(str::trim).collect();
        let root = lines.first().copied().unwrap_or_default().to_owned();
        let exists = |i: usize| lines.get(i).is_some_and(|p| resolve_against(dir, p).exists());
        let merge_in_progress = exists(1);
        let rebase_in_progress = exists(2) || exists(3);

        let out = self
            .run_ok(dir, &["status", "--porcelain=v2", "-z", "--branch", "--untracked-files=all"], RunOptions::read())
            .await?;
        let p = parse::parse_status_v2(&out.stdout);
        let clean = p.entries.iter().all(|e| e.worktree == FileChangeStatus::Ignored);
        Ok(RepoStatus {
            is_repo: true,
            git_version,
            root: Some(root),
            branch: p.head,
            head: p.oid,
            detached: p.detached,
            upstream: p.upstream,
            ahead: p.ahead,
            behind: p.behind,
            entries: p.entries,
            merge_in_progress,
            rebase_in_progress,
            clean,
        })
    }

    // ---------------------------------------------------------------------
    // diff
    // ---------------------------------------------------------------------

    async fn diff_base_args(&self, dir: &Path, target: &DiffTarget) -> VcsResult<Vec<String>> {
        Ok(match target {
            DiffTarget::Unstaged => vec![],
            DiffTarget::Staged => vec!["--cached".into()],
            DiffTarget::Head => {
                if self.has_head(dir).await? {
                    vec!["HEAD".into()]
                } else {
                    vec![self.empty_tree(dir).await?]
                }
            }
            DiffTarget::Commit { commit } => {
                let commit = validate_rev(commit)?;
                let out = self
                    .run_ok(dir, &["rev-list", "--parents", "-n", "1", "--end-of-options", &commit], RunOptions::read())
                    .await?;
                let line = out.line();
                let mut ids = line.split_whitespace();
                let id =
                    ids.next().ok_or_else(|| VcsError::NotFound(format!("commit `{commit}` not found")))?.to_owned();
                let base = match ids.next() {
                    Some(p) => p.to_owned(),
                    None => self.empty_tree(dir).await?,
                };
                vec![base, id]
            }
            DiffTarget::Range { from, to } => vec![validate_rev(from)?, validate_rev(to)?],
        })
    }

    /// Diff with per-file stats and unified patches.
    pub async fn diff(&self, dir: &Path, req: &DiffRequest) -> VcsResult<DiffResult> {
        let paths = validate_paths(&req.paths)?;
        let base = self.diff_base_args(dir, &req.target).await?;
        let mk = |mode: &[&str]| -> Vec<String> {
            let mut a: Vec<String> = vec!["diff".into(), "--no-color".into(), "--no-ext-diff".into(), "-M".into()];
            a.extend(mode.iter().map(|s| (*s).to_owned()));
            a.extend(base.iter().cloned());
            a.push("--".into());
            a.extend(paths.iter().cloned());
            a
        };
        let numstat_args = mk(&["--numstat", "-z"]);
        let names_args = mk(&["--name-status", "-z"]);
        let unified = format!("-U{}", req.context_lines.min(100));
        let patch_args = mk(&["--patch", &unified]);
        let numstat = self.runner.run_ok(dir, &numstat_args, RunOptions::read()).await?;
        let names = self.runner.run_ok(dir, &names_args, RunOptions::read()).await?;
        let patch = self.runner.run_ok(dir, &patch_args, RunOptions::read()).await?;
        let stats = parse::parse_numstat_z(&numstat.stdout);
        let statuses = parse::parse_name_status_z(&names.stdout);
        let patch_text = String::from_utf8_lossy(&patch.stdout).into_owned();
        let chunks = parse::split_patch(&patch_text);
        let aligned = chunks.len() == stats.len();

        let mut result = DiffResult::default();
        for (i, s) in stats.iter().enumerate() {
            let status = statuses
                .iter()
                .find(|n| n.path == s.path && n.old_path == s.old_path)
                .or_else(|| statuses.get(i))
                .map_or(FileChangeStatus::Modified, |n| n.status);
            let chunk = if aligned {
                chunks.get(i).copied().unwrap_or_default()
            } else {
                chunks
                    .iter()
                    .copied()
                    .find(|c| c.lines().next().is_some_and(|l| l.ends_with(&format!(" b/{}", s.path))))
                    .unwrap_or_default()
            };
            let (text, truncated) = parse::truncate_utf8(chunk, req.max_patch_bytes);
            let binary = s.additions.is_none();
            let additions = s.additions.unwrap_or(0);
            let deletions = s.deletions.unwrap_or(0);
            result.additions += additions;
            result.deletions += deletions;
            result.files.push(FileDiff {
                path: s.path.clone(),
                old_path: s.old_path.clone(),
                status,
                additions,
                deletions,
                binary,
                patch: text.to_owned(),
                truncated,
            });
        }

        if req.include_untracked && matches!(req.target, DiffTarget::Unstaged | DiffTarget::Head) {
            let st = self.status(dir).await?;
            let root = st.root.as_deref().map(PathBuf::from).unwrap_or_else(|| dir.to_path_buf());
            for e in st.entries.iter().filter(|e| e.is_untracked()) {
                if !paths.is_empty() && !paths.iter().any(|p| entry_matches(e, p)) {
                    continue;
                }
                let fd = untracked_file_diff(&root, &e.path, req.max_patch_bytes);
                result.additions += fd.additions;
                result.files.push(fd);
            }
        }
        Ok(result)
    }

    // ---------------------------------------------------------------------
    // log / show
    // ---------------------------------------------------------------------

    /// Paginated `git log`.
    pub async fn log(
        &self,
        dir: &Path,
        rev: Option<&str>,
        path: Option<&str>,
        skip: u32,
        limit: u32,
    ) -> VcsResult<LogResult> {
        let rev = match rev {
            Some(r) => validate_rev(r)?,
            None => {
                if !self.has_head(dir).await? {
                    return Ok(LogResult::default());
                }
                "HEAD".to_owned()
            }
        };
        let limit = limit.clamp(1, orkestr_protocol::methods::vcs::MAX_LOG_LIMIT);
        let mut args: Vec<String> = vec![
            "log".into(),
            parse::LOG_FORMAT.into(),
            "--decorate=short".into(),
            format!("--skip={skip}"),
            format!("--max-count={}", limit + 1),
            "--end-of-options".into(),
            rev,
            "--".into(),
        ];
        if let Some(p) = path {
            args.push(validate_rel_path(p)?);
        }
        let out = self.runner.run_ok(dir, &args, RunOptions::read()).await?;
        let mut commits = parse::parse_log(&out.stdout);
        let has_more = commits.len() > limit as usize;
        commits.truncate(limit as usize);
        Ok(LogResult { commits, has_more })
    }

    /// One commit.
    pub async fn commit_info(&self, dir: &Path, rev: &str) -> VcsResult<CommitInfo> {
        let rev = validate_rev(rev)?;
        let out = self
            .run_ok(
                dir,
                &["log", parse::LOG_FORMAT, "--decorate=short", "--max-count=1", "--end-of-options", &rev, "--"],
                RunOptions::read(),
            )
            .await?;
        parse::parse_log(&out.stdout)
            .into_iter()
            .next()
            .ok_or_else(|| VcsError::NotFound(format!("commit `{rev}` not found")))
    }

    /// A file at a revision, in the index or on disk.
    pub async fn show(&self, dir: &Path, path: &str, rev: &FileRevision) -> VcsResult<ShowResult> {
        let path = validate_rel_path(path)?;
        let max = usize::try_from(MAX_SHOW_BYTES).unwrap_or(usize::MAX);
        let spec = match rev {
            FileRevision::Head => format!("HEAD:{path}"),
            FileRevision::Index => format!(":0:{path}"),
            FileRevision::Commit { rev } => format!("{}:{path}", validate_rev(rev)?),
            FileRevision::Worktree => {
                let info = self.require_repo(dir).await?;
                let full = info.root.join(&path);
                let meta = match std::fs::symlink_metadata(&full) {
                    Ok(m) if m.is_file() => m,
                    _ => return Ok(ShowResult { path, ..ShowResult::default() }),
                };
                let size = meta.len();
                if size > MAX_SHOW_BYTES {
                    return Ok(ShowResult { path, exists: true, size, truncated: true, ..ShowResult::default() });
                }
                let bytes = tokio::fs::read(&full).await?;
                return Ok(text_result(path, &bytes, max));
            }
        };
        let size_out = self.run(dir, &["cat-file", "-s", &spec], RunOptions::read()).await?;
        if !size_out.success() {
            return Ok(ShowResult { path, ..ShowResult::default() });
        }
        let size: u64 = size_out.line().parse().unwrap_or(0);
        if size > MAX_SHOW_BYTES {
            return Ok(ShowResult { path, exists: true, size, truncated: true, ..ShowResult::default() });
        }
        let out = self.run_ok(dir, &["cat-file", "blob", &spec], RunOptions::read()).await?;
        Ok(text_result(path, &out.stdout, max))
    }

    // ---------------------------------------------------------------------
    // branches
    // ---------------------------------------------------------------------

    /// Local + remote-tracking branches.
    pub async fn branches(&self, dir: &Path) -> VcsResult<BranchesResult> {
        let out = self
            .run_ok(dir, &["for-each-ref", parse::BRANCH_FORMAT, "refs/heads", "refs/remotes"], RunOptions::read())
            .await?;
        let branches = parse::parse_branches(&out.stdout);
        let current = self.current_branch(dir).await?;
        Ok(BranchesResult { current, branches })
    }

    /// Creates a branch (optionally switching to it).
    pub async fn create_branch(
        &self,
        dir: &Path,
        name: &str,
        start: Option<&str>,
        checkout: bool,
    ) -> VcsResult<BranchResult> {
        let name = self.check_branch_name(dir, name).await?;
        let start = start.map(validate_rev).transpose()?;
        let _guard = self.lock_repo(dir).await?;
        if self.ref_exists(dir, &format!("refs/heads/{name}")).await? {
            return Err(VcsError::RefChanged(format!("branch `{name}` already exists")));
        }
        let mut args = vec!["branch", "--no-track", "--end-of-options", name.as_str()];
        if let Some(s) = start.as_deref() {
            args.push(s);
        }
        self.run_ok(dir, &args, RunOptions::write()).await?;
        if checkout {
            self.run_ok(dir, &["switch", "--no-guess", &name], RunOptions::write()).await?;
        }
        let commit = self.rev_parse(dir, &format!("refs/heads/{name}")).await?.unwrap_or_default();
        tracing::info!(branch = %name, "branch created");
        Ok(BranchResult { branch: name, commit })
    }

    /// Deletes a local branch (`-d`, or `-D` with `force`).
    pub async fn delete_branch(&self, dir: &Path, name: &str, force: bool) -> VcsResult<DeleteBranchResult> {
        let name = validate_rev(name)?;
        let _guard = self.lock_repo(dir).await?;
        let commit = self
            .rev_parse(dir, &format!("refs/heads/{name}"))
            .await?
            .ok_or_else(|| VcsError::NotFound(format!("branch `{name}` not found")))?;
        let flag = if force { "-D" } else { "-d" };
        let out = self.run(dir, &["branch", flag, "--end-of-options", &name], RunOptions::write()).await?;
        if !out.success() {
            if out.stderr.contains("not fully merged") {
                return Err(VcsError::Dirty(format!(
                    "branch `{name}` is not fully merged; deleting it needs force + confirmation"
                )));
            }
            if out.stderr.contains("checked out") || out.stderr.contains("used by worktree") {
                return Err(VcsError::Dirty(format!("branch `{name}` is checked out in a worktree")));
            }
            return Err(command_error(&["branch".into()], &out));
        }
        tracing::info!(branch = %name, %commit, force, "branch deleted");
        Ok(DeleteBranchResult { name, commit })
    }

    /// Switches branch. A remote-tracking name creates a local tracking branch.
    pub async fn checkout(&self, dir: &Path, branch: &str) -> VcsResult<BranchResult> {
        let branch = validate_rev(branch)?;
        let _guard = self.lock_repo(dir).await?;
        let local = self.ref_exists(dir, &format!("refs/heads/{branch}")).await?;
        let remote = !local && self.ref_exists(dir, &format!("refs/remotes/{branch}")).await?;
        let out = if remote {
            self.run(dir, &["switch", "--track", &branch], RunOptions::write()).await?
        } else if local {
            self.run(dir, &["switch", "--no-guess", &branch], RunOptions::write()).await?
        } else {
            return Err(VcsError::NotFound(format!("branch `{branch}` not found")));
        };
        if !out.success() {
            if out.stderr.contains("would be overwritten") {
                return Err(VcsError::Dirty(format!(
                    "cannot switch to `{branch}`: local changes would be overwritten (commit, stash or discard them first)"
                )));
            }
            if out.stderr.contains("already checked out") || out.stderr.contains("already used by worktree") {
                return Err(VcsError::Dirty(format!("branch `{branch}` is checked out in another worktree")));
            }
            return Err(command_error(&["switch".into()], &out));
        }
        let name = self.current_branch(dir).await?.unwrap_or(branch);
        let commit = self.rev_parse(dir, "HEAD").await?.unwrap_or_default();
        Ok(BranchResult { branch: name, commit })
    }

    // ---------------------------------------------------------------------
    // index
    // ---------------------------------------------------------------------

    /// `git add -A -- <paths>` (or everything).
    pub async fn stage(&self, dir: &Path, paths: &[String], all: bool) -> VcsResult<()> {
        let mut args: Vec<String> = vec!["add".into(), "-A".into(), "--".into()];
        if all {
            args.push(".".into());
        } else {
            let paths = validate_paths(paths)?;
            if paths.is_empty() {
                return Err(VcsError::invalid("no paths given"));
            }
            args.extend(paths);
        }
        let info = self.require_repo(dir).await?;
        self.runner.run_ok(&info.root, &args, RunOptions::write()).await?;
        Ok(())
    }

    /// Removes paths from the index (keeps worktree content).
    pub async fn unstage(&self, dir: &Path, paths: &[String], all: bool) -> VcsResult<()> {
        let targets = if all {
            vec![".".to_owned()]
        } else {
            let p = validate_paths(paths)?;
            if p.is_empty() {
                return Err(VcsError::invalid("no paths given"));
            }
            p
        };
        let info = self.require_repo(dir).await?;
        let mut args: Vec<String> = if self.has_head(dir).await? {
            vec!["restore".into(), "--staged".into(), "--".into()]
        } else {
            vec!["rm".into(), "-r".into(), "-q".into(), "--cached".into(), "--ignore-unmatch".into(), "--".into()]
        };
        args.extend(targets);
        self.runner.run_ok(&info.root, &args, RunOptions::write()).await?;
        Ok(())
    }

    /// Discards unstaged changes of tracked paths (restore from the index) and
    /// deletes untracked paths. Returns `(restored, deleted)`. Callers must
    /// have obtained explicit confirmation (Dangerous).
    pub async fn discard(&self, dir: &Path, paths: &[String]) -> VcsResult<(Vec<String>, Vec<String>)> {
        let paths = validate_paths(paths)?;
        if paths.is_empty() {
            return Err(VcsError::invalid("no paths given"));
        }
        let st = self.status(dir).await?;
        let root = PathBuf::from(st.root.clone().unwrap_or_default());
        let mut restore = Vec::new();
        let mut delete = Vec::new();
        for e in &st.entries {
            if !paths.iter().any(|p| entry_matches(e, p)) {
                continue;
            }
            if e.conflict.is_some() {
                return Err(VcsError::Conflict(vec![e.path.clone()]));
            }
            if e.is_untracked() {
                delete.push(e.path.clone());
            } else if e.is_unstaged() {
                restore.push(e.path.clone());
            }
        }
        if !restore.is_empty() {
            let mut args: Vec<String> = vec!["restore".into(), "--worktree".into(), "--".into()];
            args.extend(restore.iter().cloned());
            self.runner.run_ok(&root, &args, RunOptions::write()).await?;
        }
        let canonical_root = std::fs::canonicalize(&root)?;
        for p in &delete {
            let full = root.join(p);
            // Never follow a path out of the worktree.
            let parent_ok = full
                .parent()
                .and_then(|d| std::fs::canonicalize(d).ok())
                .is_some_and(|d| d.starts_with(&canonical_root));
            if !parent_ok {
                return Err(VcsError::invalid(format!("refusing to delete outside the worktree: {p}")));
            }
            match std::fs::symlink_metadata(&full) {
                Ok(m) if m.is_dir() => std::fs::remove_dir_all(&full)?,
                Ok(_) => std::fs::remove_file(&full)?,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(e.into()),
            }
        }
        tracing::info!(restored = restore.len(), deleted = delete.len(), "changes discarded");
        Ok((restore, delete))
    }

    /// Commits the index. Author/committer come from git config; hooks run.
    pub async fn commit(
        &self,
        dir: &Path,
        message: &str,
        allow_empty: bool,
        trailers: &[CommitTrailer],
    ) -> VcsResult<CommitInfo> {
        let message = message.trim();
        if message.is_empty() {
            return Err(VcsError::invalid("commit message must not be empty"));
        }
        let mut args: Vec<String> = vec!["commit".into(), "--cleanup=strip".into(), "-F".into(), "-".into()];
        if allow_empty {
            args.push("--allow-empty".into());
        }
        for t in trailers {
            let key_ok =
                !t.key.is_empty() && t.key.len() <= 64 && t.key.chars().all(|c| c.is_ascii_alphanumeric() || c == '-');
            if !key_ok || t.value.contains(['\n', '\r']) || t.value.trim().is_empty() {
                return Err(VcsError::invalid(format!("invalid trailer `{}`", t.key)));
            }
            args.push(format!("--trailer={}: {}", t.key, t.value.trim()));
        }
        let info = self.require_repo(dir).await?;
        let opts = RunOptions {
            stdin: Some(message.as_bytes()),
            read_only: false,
            timeout: Some(std::time::Duration::from_secs(300)),
        };
        let out = self.runner.run(&info.root, &args, opts).await?;
        if !out.success() {
            let text = out.text();
            if text.contains("nothing to commit")
                || text.contains("no changes added to commit")
                || out.stderr.contains("nothing to commit")
            {
                return Err(VcsError::NothingToCommit);
            }
            if out.stderr.contains("Please tell me who you are") || out.stderr.contains("user.email") {
                return Err(VcsError::invalid(
                    "git user identity is not configured (set user.name and user.email in git config)",
                ));
            }
            return Err(command_error(&["commit".into()], &out));
        }
        let commit = self.commit_info(&info.root, "HEAD").await?;
        tracing::info!(commit = %commit.short_id, "commit created");
        Ok(commit)
    }

    // ---------------------------------------------------------------------
    // worktrees
    // ---------------------------------------------------------------------

    /// `git worktree list --porcelain -z`.
    pub async fn worktree_list(&self, dir: &Path) -> VcsResult<Vec<RawWorktree>> {
        let out = self.run_ok(dir, &["worktree", "list", "--porcelain", "-z"], RunOptions::read()).await?;
        Ok(parse::parse_worktrees_z(&out.stdout))
    }

    /// `git worktree add --no-track -b <branch> <path> <base>`. Caller holds the repo lock.
    pub async fn worktree_add_locked(&self, dir: &Path, path: &Path, branch: &str, base_commit: &str) -> VcsResult<()> {
        let base_commit = validate_rev(base_commit)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let args: Vec<std::ffi::OsString> = vec![
            "worktree".into(),
            "add".into(),
            "--no-track".into(),
            "-b".into(),
            branch.into(),
            path.as_os_str().to_owned(),
            base_commit.into(),
        ];
        self.runner.run_ok(dir, &args, RunOptions::write()).await?;
        Ok(())
    }

    /// `git worktree remove [--force] <path>` + prune. Caller holds the repo lock.
    pub async fn worktree_remove_locked(&self, dir: &Path, path: &str, force: bool) -> VcsResult<()> {
        let mut args: Vec<&str> = vec!["worktree", "remove"];
        if force {
            args.push("--force");
        }
        args.push(path);
        let out = self.run(dir, &args, RunOptions::write()).await?;
        if !out.success() {
            if out.stderr.contains("contains modified or untracked files") {
                return Err(VcsError::Dirty(format!("worktree {path} has uncommitted changes")));
            }
            if out.stderr.contains("is not a working tree") {
                return Err(VcsError::NotFound(format!("{path} is not a worktree of this repository")));
            }
            return Err(command_error(&["worktree".into()], &out));
        }
        self.worktree_prune_locked(dir).await
    }

    /// `git worktree prune`. Caller holds the repo lock.
    pub async fn worktree_prune_locked(&self, dir: &Path) -> VcsResult<()> {
        self.run_ok(dir, &["worktree", "prune"], RunOptions::write()).await?;
        Ok(())
    }

    // ---------------------------------------------------------------------
    // merge
    // ---------------------------------------------------------------------

    async fn resolve_target(&self, dir: &Path, target: Option<&str>) -> VcsResult<String> {
        match target {
            Some(t) => Ok(validate_rev(t)?.trim_start_matches("refs/heads/").to_owned()),
            None => self
                .current_branch(dir)
                .await?
                .ok_or_else(|| VcsError::invalid("HEAD is detached: specify a target branch")),
        }
    }

    /// Conflict detection without touching any worktree
    /// (`git merge-tree --write-tree`, git ≥ 2.38).
    pub async fn merge_preview(&self, dir: &Path, source: &str, target: Option<&str>) -> VcsResult<MergePreview> {
        self.require_merge_tree().await?;
        let source = validate_rev(source)?;
        let target = self.resolve_target(dir, target).await?;
        let source_commit = self
            .rev_parse(dir, &source)
            .await?
            .ok_or_else(|| VcsError::NotFound(format!("`{source}` does not resolve to a commit")))?;
        let target_commit = self
            .rev_parse(dir, &format!("refs/heads/{target}"))
            .await?
            .ok_or_else(|| VcsError::NotFound(format!("target branch `{target}` not found")))?;
        let mb = self.run(dir, &["merge-base", &target_commit, &source_commit], RunOptions::read()).await?;
        let merge_base = mb.success().then(|| mb.line()).filter(|s| !s.is_empty());
        let up_to_date = self
            .run(dir, &["merge-base", "--is-ancestor", &source_commit, &target_commit], RunOptions::read())
            .await?
            .success();
        let range = format!("{target_commit}..{source_commit}");
        let commits_to_merge: u32 =
            self.run_ok(dir, &["rev-list", "--count", &range], RunOptions::read()).await?.line().parse().unwrap_or(0);
        if up_to_date {
            return Ok(MergePreview {
                source,
                target,
                source_commit,
                target_commit,
                merge_base,
                clean: true,
                tree: None,
                conflicts: Vec::new(),
                messages: Vec::new(),
                commits_to_merge,
                up_to_date,
            });
        }
        let out = self
            .run(
                dir,
                &[
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "--messages",
                    "-z",
                    "--allow-unrelated-histories",
                    &target_commit,
                    &source_commit,
                ],
                RunOptions::read(),
            )
            .await?;
        let conflicted = match out.code {
            Some(0) => false,
            Some(1) => true,
            _ => return Err(command_error(&["merge-tree".into()], &out)),
        };
        let parsed = parse::parse_merge_tree_z(&out.stdout, conflicted);
        Ok(MergePreview {
            source,
            target,
            source_commit,
            target_commit,
            merge_base,
            clean: !conflicted,
            tree: (!conflicted).then_some(parsed.tree),
            conflicts: parsed.conflicts,
            messages: parsed.messages,
            commits_to_merge,
            up_to_date,
        })
    }

    /// `--no-ff` merge of `source` into `target`.
    ///
    /// * Target checked out in a worktree → that worktree must have no tracked
    ///   changes; `git merge --no-ff` runs there. Predicted conflicts are
    ///   refused unless `allow_conflicts` (then the worktree is left merging).
    /// * Target not checked out anywhere → the merge commit is built with
    ///   `commit-tree` from the merge-tree result and the branch is moved with
    ///   compare-and-swap `update-ref` (no worktree touched). Conflicts are
    ///   always refused in this mode.
    pub async fn merge(
        &self,
        dir: &Path,
        source: &str,
        target: Option<&str>,
        message: Option<&str>,
        allow_conflicts: bool,
    ) -> VcsResult<MergeResult> {
        let _guard = self.lock_repo(dir).await?;
        let preview = self.merge_preview(dir, source, target).await?;
        if preview.up_to_date {
            return Ok(MergeResult {
                status: MergeStatus::UpToDate,
                commit: None,
                conflicts: Vec::new(),
                ref_only: false,
            });
        }
        let message = message
            .map(str::trim)
            .filter(|m| !m.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| format!("Merge branch '{}' into {}", preview.source, preview.target));
        let conflict_paths: Vec<String> = preview.conflicts.iter().map(|c| c.path.clone()).collect();
        let checked_out = self
            .worktree_list(dir)
            .await?
            .into_iter()
            .find(|w| w.branch().as_deref() == Some(preview.target.as_str()) && !w.bare);

        let Some(wt) = checked_out else {
            if !preview.clean {
                return Err(VcsError::Conflict(conflict_paths));
            }
            let tree = preview.tree.clone().unwrap_or_default();
            let opts = RunOptions { stdin: Some(message.as_bytes()), read_only: false, timeout: None };
            let new = self
                .runner
                .run_ok(
                    dir,
                    [
                        "commit-tree",
                        tree.as_str(),
                        "-p",
                        &preview.target_commit,
                        "-p",
                        &preview.source_commit,
                        "-F",
                        "-",
                    ],
                    opts,
                )
                .await?
                .line();
            let full_ref = format!("refs/heads/{}", preview.target);
            let reason = format!("orkestr: merge {} into {}", preview.source, preview.target);
            let out = self
                .run(dir, &["update-ref", "-m", &reason, &full_ref, &new, &preview.target_commit], RunOptions::write())
                .await?;
            if !out.success() {
                return Err(VcsError::RefChanged(format!(
                    "`{}` moved while merging; preview again and retry",
                    preview.target
                )));
            }
            tracing::info!(target = %preview.target, source = %preview.source, commit = %new, "merged (ref only)");
            return Ok(MergeResult {
                status: MergeStatus::Merged,
                commit: Some(new),
                conflicts: Vec::new(),
                ref_only: true,
            });
        };

        let wt_path = PathBuf::from(&wt.path);
        let st = self.status(&wt_path).await?;
        if st.merge_in_progress || st.rebase_in_progress {
            return Err(VcsError::Dirty(format!("a merge or rebase is already in progress in {}", wt.path)));
        }
        if st.entries.iter().any(|e| e.is_staged() || e.is_unstaged() || e.conflict.is_some()) {
            return Err(VcsError::Dirty(format!(
                "{} has uncommitted changes; commit or discard them before merging",
                wt.path
            )));
        }
        if !preview.clean && !allow_conflicts {
            return Err(VcsError::Conflict(conflict_paths));
        }
        let out = self
            .run(
                &wt_path,
                &["merge", "--no-ff", "--no-edit", "-m", &message, &preview.source_commit],
                RunOptions::write(),
            )
            .await?;
        if out.success() {
            let commit = self.rev_parse(&wt_path, "HEAD").await?;
            tracing::info!(target = %preview.target, source = %preview.source, "merged");
            return Ok(MergeResult { status: MergeStatus::Merged, commit, conflicts: Vec::new(), ref_only: false });
        }
        let after = self.status(&wt_path).await?;
        if after.merge_in_progress {
            let conflicts: Vec<String> =
                after.entries.iter().filter(|e| e.conflict.is_some()).map(|e| e.path.clone()).collect();
            tracing::warn!(target = %preview.target, conflicts = conflicts.len(), "merge stopped with conflicts");
            return Ok(MergeResult { status: MergeStatus::Conflicted, commit: None, conflicts, ref_only: false });
        }
        Err(command_error(&["merge".into()], &out))
    }

    /// `git merge --abort`.
    pub async fn abort_merge(&self, dir: &Path) -> VcsResult<()> {
        let _guard = self.lock_repo(dir).await?;
        let out = self.run(dir, &["merge", "--abort"], RunOptions::write()).await?;
        if !out.success() {
            if out.stderr.contains("There is no merge to abort") || out.stderr.contains("MERGE_HEAD missing") {
                return Err(VcsError::invalid("no merge in progress"));
            }
            return Err(command_error(&["merge".into()], &out));
        }
        Ok(())
    }

    /// `git init` in `dir`.
    pub async fn init(&self, dir: &Path, initial_branch: Option<&str>) -> VcsResult<()> {
        if !dir.is_dir() {
            return Err(VcsError::NotFound(format!("directory {} does not exist", dir.display())));
        }
        let mut args: Vec<String> = vec!["init".into(), "--quiet".into()];
        if let Some(b) = initial_branch {
            let b = self.check_branch_name(dir, b).await?;
            args.push(format!("--initial-branch={b}"));
        }
        self.runner.run_ok(dir, &args, RunOptions::write()).await?;
        Ok(())
    }

    /// Head commit, branch and dirtiness of a worktree (for snapshots);
    /// `None` when not a repository or git is missing.
    pub async fn head_state(&self, dir: &Path) -> Option<(Option<String>, Option<String>, bool)> {
        if !self.is_available() {
            return None;
        }
        match self.status(dir).await {
            Ok(st) if st.is_repo => {
                let dirty = st.entries.iter().any(|e| e.worktree != FileChangeStatus::Ignored);
                Some((st.head, st.branch, dirty))
            }
            _ => None,
        }
    }
}

fn text_result(path: String, bytes: &[u8], max: usize) -> ShowResult {
    let size = bytes.len() as u64;
    if is_binary(bytes) {
        return ShowResult { path, exists: true, binary: true, content: String::new(), size, truncated: false };
    }
    let text = String::from_utf8_lossy(bytes);
    let (t, truncated) = parse::truncate_utf8(&text, max);
    ShowResult { path, exists: true, binary: false, content: t.to_owned(), size, truncated }
}

fn untracked_file_diff(root: &Path, path: &str, max_patch: usize) -> FileDiff {
    let mut fd = FileDiff {
        path: path.to_owned(),
        old_path: None,
        status: FileChangeStatus::Untracked,
        additions: 0,
        deletions: 0,
        binary: false,
        patch: String::new(),
        truncated: false,
    };
    let full = root.join(path);
    let bytes = match std::fs::metadata(&full) {
        Ok(m) if m.is_file() && m.len() <= MAX_SHOW_BYTES => std::fs::read(&full).unwrap_or_default(),
        Ok(m) if m.is_file() => {
            fd.truncated = true;
            return fd;
        }
        _ => return fd,
    };
    if is_binary(&bytes) {
        fd.binary = true;
        fd.patch =
            format!("diff --git a/{path} b/{path}\nnew file mode 100644\nBinary files /dev/null and b/{path} differ\n");
        return fd;
    }
    let text = String::from_utf8_lossy(&bytes);
    let lines: Vec<&str> = text.lines().collect();
    fd.additions = u32::try_from(lines.len()).unwrap_or(u32::MAX);
    let mut patch = format!(
        "diff --git a/{path} b/{path}\nnew file mode 100644\n--- /dev/null\n+++ b/{path}\n@@ -0,0 +1,{} @@\n",
        lines.len()
    );
    for l in &lines {
        patch.push('+');
        patch.push_str(l);
        patch.push('\n');
    }
    if !text.is_empty() && !text.ends_with('\n') {
        patch.push_str("\\ No newline at end of file\n");
    }
    let (t, truncated) = parse::truncate_utf8(&patch, max_patch);
    fd.patch = t.to_owned();
    fd.truncated = truncated;
    fd
}

/// Groups status entries by path for quick lookup.
#[must_use]
pub fn entries_by_path(st: &RepoStatus) -> HashMap<&str, &StatusEntry> {
    st.entries.iter().map(|e| (e.path.as_str(), e)).collect()
}
