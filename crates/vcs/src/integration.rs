//! Building blocks of the Merge Manager (§16): commit-level merge simulation
//! without touching any worktree, blob / conflict-stage reads, conflict
//! continuation and a guarded fast-forward of the target branch.
//!
//! Simulation commits created by [`GitCli::simulate_merges`] are dangling
//! objects (never referenced by a ref) and are collected by `git gc`.

use std::path::{Path, PathBuf};

use crate::cli::GitCli;
use crate::error::{VcsError, VcsResult};
use crate::naming::{validate_rel_path, validate_rev};
use crate::parse::{self, GitVersion};
use crate::runner::{command_error, RunOptions};

/// Result of merging two commits' trees with `git merge-tree --write-tree`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeMerge {
    /// No conflicts.
    pub clean: bool,
    /// Resulting toplevel tree (with conflict markers when not clean).
    pub tree: String,
    /// Conflicting paths.
    pub conflicts: Vec<String>,
}

/// One step of a simulated ordered merge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulatedStep {
    /// Source commit-ish as given.
    pub source: String,
    /// Already contained in the accumulated result.
    pub up_to_date: bool,
    /// Conflicting paths (the step is then skipped in the accumulation).
    pub conflicts: Vec<String>,
}

/// Content of a blob.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BlobText {
    /// The path does not exist at that revision / stage.
    Missing,
    /// Binary content.
    Binary,
    /// Larger than the limit.
    TooLarge,
    /// UTF-8 (lossy) text.
    Text(String),
}

impl BlobText {
    /// The text, if any.
    #[must_use]
    pub fn text(self) -> Option<String> {
        match self {
            Self::Text(t) => Some(t),
            _ => None,
        }
    }
}

const FALLBACK_IDENTITY: [&str; 4] = ["-c", "user.name=Orkestr", "-c", "user.email=orkestr@localhost"];

fn identity_missing(stderr: &str) -> bool {
    stderr.contains("Please tell me who you are")
        || stderr.contains("user.email")
        || stderr.contains("empty ident")
        || stderr.contains("unable to auto-detect email")
}

impl GitCli {
    async fn ensure_merge_tree(&self) -> VcsResult<()> {
        let v = self.version().await?;
        if v.at_least(GitVersion::MERGE_TREE_WRITE_TREE) {
            Ok(())
        } else {
            Err(VcsError::Unsupported(format!(
                "the merge manager needs git 2.38 or newer (found {v}); please update Git"
            )))
        }
    }

    /// `git merge-base a b`.
    pub async fn merge_base(&self, dir: &Path, a: &str, b: &str) -> VcsResult<Option<String>> {
        let (a, b) = (validate_rev(a)?, validate_rev(b)?);
        let out = self.runner().run(dir, ["merge-base", a.as_str(), b.as_str()], RunOptions::read()).await?;
        Ok(out.success().then(|| out.line()).filter(|s| !s.is_empty()))
    }

    /// Whether `ancestor` is an ancestor of (or equal to) `descendant`.
    pub async fn is_ancestor(&self, dir: &Path, ancestor: &str, descendant: &str) -> VcsResult<bool> {
        let (a, d) = (validate_rev(ancestor)?, validate_rev(descendant)?);
        let out =
            self.runner().run(dir, ["merge-base", "--is-ancestor", a.as_str(), d.as_str()], RunOptions::read()).await?;
        match out.code {
            Some(0) => Ok(true),
            Some(1) => Ok(false),
            _ => Err(command_error(&["merge-base".into()], &out)),
        }
    }

    /// Commits reachable from `to` but not from `from`.
    pub async fn count_commits(&self, dir: &Path, from: &str, to: &str) -> VcsResult<u32> {
        let range = format!("{}..{}", validate_rev(from)?, validate_rev(to)?);
        let out = self.runner().run_ok(dir, ["rev-list", "--count", range.as_str()], RunOptions::read()).await?;
        Ok(out.line().parse().unwrap_or(0))
    }

    /// Merges the trees of two commits (no worktree, no ref touched).
    pub async fn merge_trees(&self, dir: &Path, ours: &str, theirs: &str) -> VcsResult<TreeMerge> {
        self.ensure_merge_tree().await?;
        let (o, t) = (validate_rev(ours)?, validate_rev(theirs)?);
        let out = self
            .runner()
            .run(
                dir,
                [
                    "merge-tree",
                    "--write-tree",
                    "--name-only",
                    "--messages",
                    "-z",
                    "--allow-unrelated-histories",
                    o.as_str(),
                    t.as_str(),
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
        Ok(TreeMerge {
            clean: !conflicted,
            tree: parsed.tree,
            conflicts: parsed.conflicts.into_iter().map(|c| c.path).collect(),
        })
    }

    /// `git commit-tree` (identity from git config; a neutral identity is
    /// used only when none is configured — for dangling simulation commits).
    pub async fn commit_tree(&self, dir: &Path, tree: &str, parents: &[&str], message: &str) -> VcsResult<String> {
        let tree = validate_rev(tree)?;
        let mut args: Vec<String> = vec!["commit-tree".into(), tree];
        for p in parents {
            args.push("-p".into());
            args.push(validate_rev(p)?);
        }
        args.push("-F".into());
        args.push("-".into());
        let opts = || RunOptions { stdin: Some(message.as_bytes()), read_only: false, timeout: None };
        let out = self.runner().run(dir, &args, opts()).await?;
        if out.success() {
            return Ok(out.line());
        }
        if identity_missing(&out.stderr) {
            let mut with_id: Vec<String> = FALLBACK_IDENTITY.iter().map(|s| (*s).to_owned()).collect();
            with_id.extend(args);
            return Ok(self.runner().run_ok(dir, &with_id, opts()).await?.line());
        }
        Err(command_error(&["commit-tree".into()], &out))
    }

    /// Simulates merging `sources` (commit-ish) one after the other into
    /// `base` without touching any worktree or ref. A conflicting step is
    /// reported and skipped (the accumulation continues without it).
    pub async fn simulate_merges(&self, dir: &Path, base: &str, sources: &[String]) -> VcsResult<Vec<SimulatedStep>> {
        let mut current = validate_rev(base)?;
        let mut steps = Vec::with_capacity(sources.len());
        for src in sources {
            let commit = self
                .rev_parse(dir, src)
                .await?
                .ok_or_else(|| VcsError::NotFound(format!("`{src}` does not resolve to a commit")))?;
            if self.is_ancestor(dir, &commit, &current).await? {
                steps.push(SimulatedStep { source: src.clone(), up_to_date: true, conflicts: Vec::new() });
                continue;
            }
            let m = self.merge_trees(dir, &current, &commit).await?;
            if !m.clean {
                steps.push(SimulatedStep { source: src.clone(), up_to_date: false, conflicts: m.conflicts });
                continue;
            }
            current = self.commit_tree(dir, &m.tree, &[&current, &commit], "orkestr: merge simulation").await?;
            steps.push(SimulatedStep { source: src.clone(), up_to_date: false, conflicts: Vec::new() });
        }
        Ok(steps)
    }

    async fn object_text(&self, dir: &Path, spec: &str, max: usize) -> VcsResult<BlobText> {
        let exists = self.runner().run(dir, ["cat-file", "-e", spec], RunOptions::read()).await?;
        if !exists.success() {
            return Ok(BlobText::Missing);
        }
        let size: usize = self.runner().run_ok(dir, ["cat-file", "-s", spec], RunOptions::read()).await?.line().parse().unwrap_or(0);
        if size > max {
            return Ok(BlobText::TooLarge);
        }
        let out = self.runner().run_ok(dir, ["cat-file", "-p", spec], RunOptions::read()).await?;
        if out.stdout.iter().take(8000).any(|b| *b == 0) {
            return Ok(BlobText::Binary);
        }
        Ok(BlobText::Text(String::from_utf8_lossy(&out.stdout).into_owned()))
    }

    /// Content of `path` at revision `rev`.
    pub async fn blob_at(&self, dir: &Path, rev: &str, path: &str, max: usize) -> VcsResult<BlobText> {
        let rev = validate_rev(rev)?;
        let path = validate_rel_path(path)?;
        self.object_text(dir, &format!("{rev}:{path}"), max).await
    }

    /// Content of a conflict stage of `path` in a merging worktree
    /// (1 = base, 2 = ours, 3 = theirs).
    pub async fn stage_text(&self, dir: &Path, stage: u8, path: &str, max: usize) -> VcsResult<BlobText> {
        if !(1..=3).contains(&stage) {
            return Err(VcsError::invalid("conflict stage must be 1, 2 or 3"));
        }
        let path = validate_rel_path(path)?;
        self.object_text(dir, &format!(":{stage}:{path}"), max).await
    }

    /// Paths with unresolved conflicts in a worktree.
    pub async fn unmerged_paths(&self, dir: &Path) -> VcsResult<Vec<String>> {
        let st = self.status(dir).await?;
        Ok(st.entries.into_iter().filter(|e| e.conflict.is_some()).map(|e| e.path).collect())
    }

    /// Concludes a merge whose conflicts were resolved and staged
    /// (`git commit --no-edit`). Returns the merge commit.
    pub async fn conclude_merge(&self, dir: &Path) -> VcsResult<String> {
        let _guard = self.lock_repo(dir).await?;
        let st = self.status(dir).await?;
        if !st.merge_in_progress {
            return Err(VcsError::invalid("no merge in progress"));
        }
        let open: Vec<String> = st.entries.iter().filter(|e| e.conflict.is_some()).map(|e| e.path.clone()).collect();
        if !open.is_empty() {
            return Err(VcsError::Conflict(open));
        }
        let out = self.runner().run(dir, ["commit", "--no-edit", "--no-verify"], RunOptions::write()).await?;
        if !out.success() {
            if identity_missing(&out.stderr) {
                return Err(VcsError::invalid(
                    "git user identity is not configured (set user.name and user.email in git config)",
                ));
            }
            return Err(command_error(&["commit".into()], &out));
        }
        self.rev_parse(dir, "HEAD").await?.ok_or_else(|| VcsError::NotFound("HEAD after merge commit".into()))
    }

    /// Writes a resolved file into a merging worktree and stages it; `None`
    /// removes the file (`git rm`).
    pub async fn stage_resolution(&self, dir: &Path, path: &str, content: Option<&str>) -> VcsResult<()> {
        let rel = validate_rel_path(path)?;
        let root = self.require_repo(dir).await?.root;
        let full = root.join(&rel);
        let canonical_root = std::fs::canonicalize(&root)?;
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent)?;
            let parent_ok = std::fs::canonicalize(parent).is_ok_and(|p| p.starts_with(&canonical_root));
            if !parent_ok {
                return Err(VcsError::invalid(format!("refusing to write outside the worktree: {rel}")));
            }
        }
        match content {
            Some(text) => {
                std::fs::write(&full, text)?;
                self.runner().run_ok(&root, ["add", "--", rel.as_str()], RunOptions::write()).await?;
            }
            None => {
                self.runner()
                    .run_ok(&root, ["rm", "-q", "--ignore-unmatch", "-f", "--", rel.as_str()], RunOptions::write())
                    .await?;
            }
        }
        Ok(())
    }

    /// Fast-forwards `branch` from `expected_old` to `new` (which must
    /// descend from it). When the branch is checked out in a worktree, that
    /// worktree must have no tracked changes and `git merge --ff-only` runs
    /// there; otherwise the ref moves with a compare-and-swap `update-ref`.
    /// Returns the worktree that was updated, if any.
    pub async fn fast_forward_branch(
        &self,
        dir: &Path,
        branch: &str,
        expected_old: &str,
        new: &str,
    ) -> VcsResult<Option<PathBuf>> {
        let branch = validate_rev(branch)?.trim_start_matches("refs/heads/").to_owned();
        let (old, new) = (validate_rev(expected_old)?, validate_rev(new)?);
        let _guard = self.lock_repo(dir).await?;
        let current = self
            .rev_parse(dir, &format!("refs/heads/{branch}"))
            .await?
            .ok_or_else(|| VcsError::NotFound(format!("target branch `{branch}` not found")))?;
        if current != old {
            return Err(VcsError::RefChanged(format!(
                "`{branch}` moved while merging (expected {old}, found {current}); propose the merge again"
            )));
        }
        if !self.is_ancestor(dir, &old, &new).await? {
            return Err(VcsError::invalid(format!("{new} does not descend from {old}: not a fast-forward")));
        }
        let checked_out =
            self.worktree_list(dir).await?.into_iter().find(|w| w.branch().as_deref() == Some(branch.as_str()) && !w.bare);
        let Some(wt) = checked_out else {
            let full_ref = format!("refs/heads/{branch}");
            let reason = format!("orkestr: merge manager fast-forward {branch}");
            let out = self
                .runner()
                .run(dir, ["update-ref", "-m", reason.as_str(), full_ref.as_str(), new.as_str(), old.as_str()], RunOptions::write())
                .await?;
            if !out.success() {
                return Err(VcsError::RefChanged(format!("`{branch}` moved while merging; propose the merge again")));
            }
            return Ok(None);
        };
        let wt_path = PathBuf::from(&wt.path);
        let st = self.status(&wt_path).await?;
        if st.merge_in_progress || st.rebase_in_progress {
            return Err(VcsError::Dirty(format!("a merge or rebase is in progress in {}", wt.path)));
        }
        if st.entries.iter().any(|e| e.is_staged() || e.is_unstaged() || e.conflict.is_some()) {
            return Err(VcsError::Dirty(format!(
                "{} has uncommitted changes; commit or discard them before merging",
                wt.path
            )));
        }
        let out = self.runner().run(&wt_path, ["merge", "--ff-only", "--quiet", new.as_str()], RunOptions::write()).await?;
        if !out.success() {
            if out.stderr.contains("would be overwritten") {
                return Err(VcsError::Dirty(format!(
                    "untracked files in {} would be overwritten by the merge; move them away first",
                    wt.path
                )));
            }
            return Err(command_error(&["merge".into()], &out));
        }
        Ok(Some(wt_path))
    }

    /// The worktree (path) where `branch` is checked out, if any.
    pub async fn branch_worktree(&self, dir: &Path, branch: &str) -> VcsResult<Option<String>> {
        let branch = branch.trim_start_matches("refs/heads/");
        Ok(self
            .worktree_list(dir)
            .await?
            .into_iter()
            .find(|w| w.branch().as_deref() == Some(branch) && !w.bare)
            .map(|w| w.path))
    }

    /// Whether a worktree has tracked (staged / unstaged / conflicted)
    /// changes — untracked files are ignored.
    pub async fn has_tracked_changes(&self, dir: &Path) -> VcsResult<bool> {
        let st = self.status(dir).await?;
        Ok(st.merge_in_progress || st.entries.iter().any(|e| e.conflict.is_some() || e.is_staged() || e.is_unstaged()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::GitCli;

    fn git(args: &[&str], dir: &Path) {
        let out = std::process::Command::new("git").args(args).current_dir(dir).output().unwrap();
        assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    }

    fn repo() -> Option<tempfile::TempDir> {
        if !GitCli::discover().is_available() {
            return None;
        }
        let d = tempfile::tempdir().unwrap();
        let r = d.path();
        git(&["init", "-q", "-b", "main"], r);
        git(&["config", "user.name", "T"], r);
        git(&["config", "user.email", "t@e"], r);
        std::fs::write(r.join("a.txt"), "1\n2\n3\n").unwrap();
        git(&["add", "-A"], r);
        git(&["commit", "-q", "-m", "init"], r);
        Some(d)
    }

    fn branch_with(r: &Path, name: &str, file: &str, content: &str) {
        git(&["checkout", "-q", "-b", name, "main"], r);
        std::fs::write(r.join(file), content).unwrap();
        git(&["add", "-A"], r);
        git(&["commit", "-q", "-m", name], r);
        git(&["checkout", "-q", "main"], r);
    }

    #[tokio::test]
    async fn simulation_detects_ordered_conflicts_without_touching_refs() {
        let Some(d) = repo() else { return };
        let r = d.path();
        branch_with(r, "a", "a.txt", "1\nA\n3\n");
        branch_with(r, "b", "b.txt", "new\n");
        branch_with(r, "c", "a.txt", "1\nC\n3\n");
        let g = GitCli::discover();
        let main_before = g.rev_parse(r, "main").await.unwrap().unwrap();
        let steps = g.simulate_merges(r, "main", &["a".into(), "b".into(), "c".into()]).await.unwrap();
        assert!(steps[0].conflicts.is_empty() && steps[1].conflicts.is_empty());
        assert_eq!(steps[2].conflicts, vec!["a.txt".to_owned()]);
        assert_eq!(g.rev_parse(r, "main").await.unwrap().unwrap(), main_before);
        // Pairwise: a vs b clean, a vs c conflicting.
        let a = g.rev_parse(r, "a").await.unwrap().unwrap();
        let c = g.rev_parse(r, "c").await.unwrap().unwrap();
        assert!(!g.merge_trees(r, &a, &c).await.unwrap().clean);
        assert!(g.is_ancestor(r, &main_before, &a).await.unwrap());
        assert_eq!(g.count_commits(r, "main", "a").await.unwrap(), 1);
        assert_eq!(g.blob_at(r, "a", "a.txt", 1000).await.unwrap(), BlobText::Text("1\nA\n3\n".into()));
        assert_eq!(g.blob_at(r, "a", "zzz.txt", 1000).await.unwrap(), BlobText::Missing);
    }

    #[tokio::test]
    async fn fast_forward_guards_moved_refs() {
        let Some(d) = repo() else { return };
        let r = d.path();
        branch_with(r, "feat", "f.txt", "x\n");
        let g = GitCli::discover();
        let main = g.rev_parse(r, "main").await.unwrap().unwrap();
        let feat = g.rev_parse(r, "feat").await.unwrap().unwrap();
        // Wrong expectation → refused.
        assert!(matches!(g.fast_forward_branch(r, "main", &feat, &feat).await, Err(VcsError::RefChanged(_))));
        // main is checked out in the root worktree → ff-only there.
        let wt = g.fast_forward_branch(r, "main", &main, &feat).await.unwrap();
        assert!(wt.is_some());
        assert_eq!(g.rev_parse(r, "main").await.unwrap().unwrap(), feat);
        assert!(r.join("f.txt").exists());
    }
}
