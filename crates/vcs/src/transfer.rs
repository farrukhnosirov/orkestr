//! Workspace transfer helpers for remote jobs (Phase 6, node-protocol.md §6–7):
//! a git bundle carries the committed history, a binary patch carries the
//! dirty tree (tracked changes + untracked files) relative to a base commit.
//!
//! * [`GitCli::create_bundle`] — `git bundle create <out> --all HEAD`;
//! * [`GitCli::clone_bundle`] — materializes a bundle at a commit;
//! * [`GitCli::working_patch`] — `git diff --binary <base>` including untracked
//!   files (registered with `--intent-to-add` for the diff, then reset);
//! * [`GitCli::apply_patch`] — `git apply --binary` into a worktree.

use std::path::Path;

use crate::cli::GitCli;
use crate::error::{VcsError, VcsResult};
use crate::naming::validate_rev;
use crate::runner::RunOptions;

/// Untracked-file listing chunk (keeps command lines short on Windows).
const PATH_CHUNK: usize = 64;

impl GitCli {
    /// Writes a bundle with every ref plus `HEAD` of the repository at `dir`.
    ///
    /// # Errors
    /// Not a repository, no commits yet, git failure.
    pub async fn create_bundle(&self, dir: &Path, out: &Path) -> VcsResult<()> {
        let out = out.to_string_lossy().into_owned();
        self.runner().run_ok(dir, ["bundle", "create", out.as_str(), "--all", "HEAD"], RunOptions::read()).await?;
        Ok(())
    }

    /// Clones `bundle` into the (new) directory `target` and checks out
    /// `commit` on branch `branch` (`orkestr-job` when `None`). Line endings
    /// are kept byte-exact (`core.autocrlf=false`) so patches apply on every OS.
    ///
    /// # Errors
    /// Missing parent directory, invalid revision, git failure.
    pub async fn clone_bundle(
        &self,
        bundle: &Path,
        target: &Path,
        commit: &str,
        branch: Option<&str>,
    ) -> VcsResult<()> {
        let commit = validate_rev(commit)?;
        let parent = target
            .parent()
            .ok_or_else(|| VcsError::NotFound(format!("no parent directory for {}", target.display())))?;
        let (b, t) = (bundle.to_string_lossy().into_owned(), target.to_string_lossy().into_owned());
        self.runner()
            .run_ok(
                parent,
                ["clone", "--quiet", "--no-checkout", "-c", "core.autocrlf=false", b.as_str(), t.as_str()],
                RunOptions::write(),
            )
            .await?;
        let branch = branch.unwrap_or("orkestr-job");
        let branch = self.check_branch_name(target, branch).await.unwrap_or_else(|_| "orkestr-job".to_owned());
        self.runner()
            .run_ok(target, ["checkout", "--quiet", "-B", branch.as_str(), commit.as_str()], RunOptions::write())
            .await?;
        Ok(())
    }

    /// Binary patch of the worktree at `dir` against `base` (tracked changes,
    /// staged or not, plus untracked non-ignored files). Empty when clean.
    ///
    /// # Errors
    /// Not a repository, invalid revision, git failure.
    pub async fn working_patch(&self, dir: &Path, base: &str) -> VcsResult<Vec<u8>> {
        let base = validate_rev(base)?;
        let untracked =
            self.runner().run_ok(dir, ["ls-files", "--others", "--exclude-standard", "-z"], RunOptions::read()).await?;
        let files: Vec<String> = untracked
            .stdout
            .split(|b| *b == 0)
            .filter(|s| !s.is_empty())
            .map(|s| String::from_utf8_lossy(s).into_owned())
            .collect();
        for chunk in files.chunks(PATH_CHUNK) {
            let mut args = vec!["add".to_owned(), "--intent-to-add".to_owned(), "--".to_owned()];
            args.extend(chunk.iter().cloned());
            self.runner().run_ok(dir, &args, RunOptions::write()).await?;
        }
        let diff = self
            .runner()
            .run(
                dir,
                ["-c", "core.quotepath=false", "diff", "--binary", "--no-color", "--no-ext-diff", base.as_str()],
                RunOptions::read(),
            )
            .await;
        for chunk in files.chunks(PATH_CHUNK) {
            let mut args = vec!["reset".to_owned(), "--quiet".to_owned(), "--".to_owned()];
            args.extend(chunk.iter().cloned());
            let _ = self.runner().run(dir, &args, RunOptions::write()).await;
        }
        let diff = diff?;
        if !diff.success() {
            return Err(VcsError::Command { command: "diff".into(), code: diff.code, stderr: diff.stderr });
        }
        Ok(diff.stdout)
    }

    /// Applies a binary patch (from [`GitCli::working_patch`]) to the worktree
    /// at `dir` without touching the index. A no-op for an empty patch.
    ///
    /// # Errors
    /// The patch does not apply (conflict) or git failure.
    pub async fn apply_patch(&self, dir: &Path, patch: &[u8]) -> VcsResult<()> {
        self.apply_patch_inner(dir, patch, false).await
    }

    /// Reverse-applies a patch (undoes [`GitCli::apply_patch`]).
    ///
    /// # Errors
    /// The worktree diverged from the patch or git failure.
    pub async fn revert_patch(&self, dir: &Path, patch: &[u8]) -> VcsResult<()> {
        self.apply_patch_inner(dir, patch, true).await
    }

    async fn apply_patch_inner(&self, dir: &Path, patch: &[u8], reverse: bool) -> VcsResult<()> {
        if patch.iter().all(u8::is_ascii_whitespace) {
            return Ok(());
        }
        let out = self
            .runner()
            .run(
                dir,
                if reverse {
                    ["apply", "--binary", "--whitespace=nowarn", "-R", "-"].as_slice()
                } else {
                    ["apply", "--binary", "--whitespace=nowarn", "-"].as_slice()
                },
                RunOptions { stdin: Some(patch), ..RunOptions::write() },
            )
            .await?;
        if out.success() {
            Ok(())
        } else {
            Err(VcsError::Command { command: "apply".into(), code: out.code, stderr: out.stderr })
        }
    }
}
