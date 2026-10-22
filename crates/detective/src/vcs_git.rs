//! [`VcsOps`] over `orkestr-vcs` (managed worktrees, CAS-safe merge).

use std::path::{Path, PathBuf};

use async_trait::async_trait;
use orkestr_domain::ProjectId;
use orkestr_protocol::methods::vcs::{DiffTarget, MergeStatus};
use orkestr_vcs::{AddWorktreeRequest, DiffRequest, Vcs, VcsError};

use crate::collectors::cap;
use crate::ports::{CommitSummary, VcsOps};
use crate::DetectiveError;

fn map(e: VcsError) -> DetectiveError {
    match e {
        VcsError::NotFound(m) => DetectiveError::NotFound(m),
        VcsError::Conflict(paths) => DetectiveError::Conflict(format!("merge conflicts in {}", paths.join(", "))),
        VcsError::Dirty(m) | VcsError::RefChanged(m) => DetectiveError::Conflict(m),
        other => DetectiveError::Internal(other.to_string()),
    }
}

/// Git-backed VCS operations.
#[derive(Debug, Clone)]
pub struct GitVcs {
    vcs: Vcs,
}

impl GitVcs {
    /// Wraps the VCS service.
    #[must_use]
    pub fn new(vcs: Vcs) -> Self {
        Self { vcs }
    }

    async fn patch(&self, dir: &Path, target: DiffTarget, max_bytes: usize) -> Result<String, DetectiveError> {
        let req = DiffRequest {
            target,
            paths: Vec::new(),
            context_lines: 3,
            include_untracked: false,
            max_patch_bytes: max_bytes.max(1024),
        };
        let d = self.vcs.git().diff(dir, &req).await.map_err(map)?;
        let mut out = String::new();
        for f in &d.files {
            out.push_str(&format!("--- {} (+{} -{})\n", f.path, f.additions, f.deletions));
            if f.binary {
                out.push_str("(binary)\n");
            } else {
                out.push_str(&f.patch);
                if !f.patch.ends_with('\n') {
                    out.push('\n');
                }
            }
            if out.len() > max_bytes {
                break;
            }
        }
        Ok(cap(&out, max_bytes))
    }
}

#[async_trait]
impl VcsOps for GitVcs {
    async fn recent_commits(&self, root: &Path, limit: u32) -> Result<Vec<CommitSummary>, DetectiveError> {
        let log = self.vcs.git().log(root, None, None, 0, limit).await.map_err(map)?;
        Ok(log
            .commits
            .into_iter()
            .map(|c| CommitSummary { sha: c.id, subject: c.subject, author: c.author_name, date: c.author_date })
            .collect())
    }

    async fn diff_since(&self, root: &Path, from: Option<&str>, max_bytes: usize) -> Result<String, DetectiveError> {
        let target = match from {
            Some(f) => DiffTarget::Range { from: f.to_owned(), to: "HEAD".into() },
            None => {
                let head = self.vcs.git().rev_parse(root, "HEAD").await.map_err(map)?;
                let Some(head) = head else { return Ok(String::new()) };
                DiffTarget::Commit { commit: head }
            }
        };
        self.patch(root, target, max_bytes).await
    }

    async fn diff_range(&self, dir: &Path, from: &str, to: &str, max_bytes: usize) -> Result<String, DetectiveError> {
        self.patch(dir, DiffTarget::Range { from: from.to_owned(), to: to.to_owned() }, max_bytes).await
    }

    async fn rev_parse(&self, dir: &Path, rev: &str) -> Result<Option<String>, DetectiveError> {
        self.vcs.git().rev_parse(dir, rev).await.map_err(map)
    }

    async fn current_branch(&self, dir: &Path) -> Result<Option<String>, DetectiveError> {
        self.vcs.git().current_branch(dir).await.map_err(map)
    }

    async fn create_worktree(
        &self,
        project_id: ProjectId,
        root: &Path,
        branch: &str,
        base: &str,
    ) -> Result<PathBuf, DetectiveError> {
        let req = AddWorktreeRequest {
            agent_id: None,
            role: Some("detective".into()),
            task_id: None,
            branch: Some(branch.to_owned()),
            base: Some(base.to_owned()),
        };
        let w = self.vcs.add_worktree(project_id, root, req).await.map_err(map)?;
        Ok(PathBuf::from(w.path))
    }

    async fn commit_all(&self, dir: &Path, message: &str) -> Result<Option<String>, DetectiveError> {
        let git = self.vcs.git();
        git.stage(dir, &[], true).await.map_err(map)?;
        match git.commit(dir, message, false, &[]).await {
            Ok(c) => Ok(Some(c.id)),
            Err(VcsError::NothingToCommit) => Ok(None),
            Err(e) => Err(map(e)),
        }
    }

    async fn merge(&self, root: &Path, source: &str, target: &str, message: &str) -> Result<String, DetectiveError> {
        let r = self.vcs.git().merge(root, source, Some(target), Some(message), false).await.map_err(map)?;
        match r.status {
            MergeStatus::Merged => Ok(r.commit.unwrap_or_default()),
            MergeStatus::UpToDate => Ok(self.vcs.git().rev_parse(root, target).await.map_err(map)?.unwrap_or_default()),
            MergeStatus::Conflicted => {
                let _ = self.vcs.git().abort_merge(root).await;
                Err(DetectiveError::Conflict(format!("merge conflicts in {}", r.conflicts.join(", "))))
            }
        }
    }

    async fn remove_worktree(&self, project_id: ProjectId, root: &Path, path: &Path) -> Result<(), DetectiveError> {
        self.vcs.remove_worktree(project_id, root, &path.to_string_lossy(), true).await.map_err(map)?;
        Ok(())
    }
}
