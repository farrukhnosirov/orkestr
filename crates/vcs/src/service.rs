//! [`Vcs`] — project-level VCS service: resolves which worktree a request
//! addresses and manages Orkestr's agent worktrees (ADR-0009).

use std::path::{Path, PathBuf};
use std::sync::Arc;

use orkestr_domain::{AgentId, ProjectId, TaskId};
use orkestr_protocol::methods::vcs::{FileChangeStatus, WorktreeInfo};
use orkestr_storage::Db;

use crate::cli::GitCli;
use crate::error::{VcsError, VcsResult};
use crate::naming::{
    agent_branch_name, path_key, short_id, slug, validate_rev, with_suffix, worktree_path, MAX_ROLE_SLUG,
};
use crate::parse::RawWorktree;
use crate::store::{VcsStore, WorktreeRecord, WorktreeState};

/// Max collision suffix tried for agent branches/paths.
const MAX_COLLISIONS: u32 = 50;

/// Request to create a worktree.
#[derive(Debug, Clone, Default)]
pub struct AddWorktreeRequest {
    /// Owning agent → agent naming.
    pub agent_id: Option<AgentId>,
    /// Role (slugified).
    pub role: Option<String>,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Explicit branch (required without agent).
    pub branch: Option<String>,
    /// Base revision (default HEAD of the project root).
    pub base: Option<String>,
}

/// Project-level VCS service.
#[derive(Debug, Clone)]
pub struct Vcs {
    git: Arc<GitCli>,
    store: VcsStore,
    worktrees_root: PathBuf,
}

impl Vcs {
    /// Creates the service. `worktrees_root` is `<data_dir>/worktrees`.
    #[must_use]
    pub fn new(git: Arc<GitCli>, db: Db, worktrees_root: PathBuf) -> Self {
        Self { git, store: VcsStore::new(db), worktrees_root }
    }

    /// The git CLI.
    #[must_use]
    pub fn git(&self) -> &Arc<GitCli> {
        &self.git
    }

    /// Root under which agent worktrees are created.
    #[must_use]
    pub fn worktrees_root(&self) -> &Path {
        &self.worktrees_root
    }

    /// Resolves the directory a request addresses: the project root, or —
    /// when `worktree` is given — one of the worktrees of the project's
    /// repository (anything else is rejected, so clients cannot point git at
    /// arbitrary directories).
    pub async fn resolve_dir(&self, project_root: &Path, worktree: Option<&str>) -> VcsResult<PathBuf> {
        let Some(w) = worktree.map(str::trim).filter(|w| !w.is_empty()) else {
            return Ok(project_root.to_path_buf());
        };
        let raw = self.find_worktree(project_root, w).await?;
        Ok(PathBuf::from(raw.path))
    }

    async fn find_worktree(&self, project_root: &Path, path: &str) -> VcsResult<RawWorktree> {
        let want = path_key(Path::new(path));
        self.git
            .worktree_list(project_root)
            .await?
            .into_iter()
            .find(|w| path_key(Path::new(&w.path)) == want)
            .ok_or_else(|| VcsError::NotFound(format!("{path} is not a worktree of this project's repository")))
    }

    /// Whether the worktree at `dir` has uncommitted (non-ignored) changes.
    pub async fn is_dirty(&self, dir: &Path) -> VcsResult<bool> {
        let st = self.git.status(dir).await?;
        Ok(st.entries.iter().any(|e| e.worktree != FileChangeStatus::Ignored))
    }

    /// All worktrees of the project's repository (main first), merged with
    /// Orkestr metadata.
    pub async fn worktrees(&self, project_id: ProjectId, project_root: &Path) -> VcsResult<Vec<WorktreeInfo>> {
        let raw = self.git.worktree_list(project_root).await?;
        let records = self.store.active(project_id).await?;
        let mut out = Vec::with_capacity(raw.len());
        for (i, w) in raw.iter().enumerate() {
            let key = path_key(Path::new(&w.path));
            let rec = records.iter().find(|r| r.path_key == key);
            let exists = Path::new(&w.path).is_dir();
            let dirty = if exists && !w.bare && !w.prunable {
                match self.is_dirty(Path::new(&w.path)).await {
                    Ok(d) => Some(d),
                    Err(e) => {
                        tracing::debug!(path = %w.path, error = %e, "worktree status failed");
                        None
                    }
                }
            } else {
                None
            };
            out.push(to_info(w, i == 0, rec, dirty));
        }
        Ok(out)
    }

    /// Creates a branch + worktree. With an agent: branch
    /// `agent/<role>-<agent short>` and path `<root>/<project short>/<agent short>`
    /// (suffix `-2`, `-3`… on collision); otherwise `branch` is required.
    pub async fn add_worktree(
        &self,
        project_id: ProjectId,
        project_root: &Path,
        req: AddWorktreeRequest,
    ) -> VcsResult<WorktreeInfo> {
        let info = self.git.require_repo(project_root).await?;
        let base_ref = req.base.as_deref().map(validate_rev).transpose()?.unwrap_or_else(|| "HEAD".to_owned());
        let base_commit = self.git.rev_parse(project_root, &base_ref).await?.ok_or_else(|| {
            if base_ref == "HEAD" {
                VcsError::invalid("the repository has no commits yet — make an initial commit first")
            } else {
                VcsError::NotFound(format!("base `{base_ref}` does not resolve to a commit"))
            }
        })?;
        let role = req.role.as_deref().map(|r| slug(r, MAX_ROLE_SLUG, "agent"));
        let (branch_base, leaf_base, suffix_branch) = match (req.agent_id, req.branch.as_deref()) {
            (Some(agent), None) => {
                let agent_s = agent.to_string();
                (agent_branch_name(role.as_deref(), &agent_s), short_id(&agent_s), true)
            }
            (_, Some(b)) => {
                let b = self.git.check_branch_name(project_root, b).await?;
                let leaf = slug(&b, 48, "worktree");
                (b, leaf, false)
            }
            (None, None) => return Err(VcsError::invalid("either agentId or branch is required")),
        };

        let _guard = self.git.lock_repo(project_root).await?;
        let mut chosen: Option<(String, PathBuf)> = None;
        for n in 1..=MAX_COLLISIONS {
            let branch = if suffix_branch { with_suffix(&branch_base, n) } else { branch_base.clone() };
            let path = worktree_path(&self.worktrees_root, &project_id.to_string(), &with_suffix(&leaf_base, n));
            if self.git.rev_parse(project_root, &format!("refs/heads/{branch}")).await?.is_some() {
                if suffix_branch {
                    continue;
                }
                return Err(VcsError::RefChanged(format!("branch `{branch}` already exists")));
            }
            if path.exists() {
                continue;
            }
            chosen = Some((branch, path));
            break;
        }
        let (branch, path) =
            chosen.ok_or_else(|| VcsError::RefChanged("could not find a free branch/worktree name".into()))?;

        if let Err(e) = self.git.worktree_add_locked(&info.root, &path, &branch, &base_commit).await {
            self.cleanup_failed_add(&info.root, &path, &branch).await;
            return Err(e);
        }
        let path_str = path.to_string_lossy().into_owned();
        let rec = WorktreeRecord {
            id: uuid_like(),
            project_id,
            path: path_str.clone(),
            path_key: path_key(&path),
            branch: branch.clone(),
            agent_id: req.agent_id,
            task_id: req.task_id,
            role: role.clone().or_else(|| req.agent_id.map(|_| "agent".to_owned())),
            base_ref: Some(base_ref),
            base_commit: base_commit.clone(),
            state: WorktreeState::Active,
            created_at: chrono::Utc::now(),
        };
        if let Err(e) = self.store.insert(&rec).await {
            self.cleanup_failed_add(&info.root, &path, &branch).await;
            return Err(e.into());
        }
        tracing::info!(project_id = %project_id, %branch, path = %path_str, "worktree created");
        let raw = RawWorktree {
            path: path_str,
            head: Some(base_commit),
            branch_ref: Some(format!("refs/heads/{branch}")),
            ..RawWorktree::default()
        };
        Ok(to_info(&raw, false, Some(&rec), Some(false)))
    }

    async fn cleanup_failed_add(&self, root: &Path, path: &Path, branch: &str) {
        if path.exists() {
            let p = path.to_string_lossy().into_owned();
            if let Err(e) = self.git.worktree_remove_locked(root, &p, true).await {
                tracing::warn!(error = %e, path = %p, "cleanup of failed worktree failed");
            }
        }
        if let Ok(Some(_)) = self.git.rev_parse(root, &format!("refs/heads/{branch}")).await {
            let out = self
                .git
                .runner()
                .run(root, ["branch", "-D", "--end-of-options", branch], crate::runner::RunOptions::write())
                .await;
            if let Err(e) = out {
                tracing::warn!(error = %e, %branch, "cleanup of failed worktree branch failed");
            }
        }
    }

    /// Removes a worktree of the project's repository (never the main one).
    /// Without `force` a dirty worktree is refused. The caller is responsible
    /// for confirmation and a safety snapshot before a forced removal.
    pub async fn remove_worktree(
        &self,
        project_id: ProjectId,
        project_root: &Path,
        path: &str,
        force: bool,
    ) -> VcsResult<String> {
        let raw_list = self.git.worktree_list(project_root).await?;
        let want = path_key(Path::new(path));
        let (idx, raw) = raw_list
            .iter()
            .enumerate()
            .find(|(_, w)| path_key(Path::new(&w.path)) == want)
            .ok_or_else(|| VcsError::NotFound(format!("{path} is not a worktree of this project's repository")))?;
        if idx == 0 {
            return Err(VcsError::invalid("the main worktree cannot be removed"));
        }
        let exists = Path::new(&raw.path).is_dir();
        if exists && !force && self.is_dirty(Path::new(&raw.path)).await? {
            return Err(VcsError::Dirty(format!(
                "worktree {} has uncommitted changes; removing it needs force + confirmation",
                raw.path
            )));
        }
        let _guard = self.git.lock_repo(project_root).await?;
        if exists {
            self.git.worktree_remove_locked(project_root, &raw.path, force).await?;
        } else {
            self.git.worktree_prune_locked(project_root).await?;
        }
        self.store.mark_removed(project_id, &want).await?;
        tracing::info!(project_id = %project_id, path = %raw.path, force, "worktree removed");
        Ok(raw.path.clone())
    }
}

fn uuid_like() -> String {
    orkestr_domain::WorkspaceId::new().to_string()
}

fn to_info(w: &RawWorktree, is_main: bool, rec: Option<&WorktreeRecord>, dirty: Option<bool>) -> WorktreeInfo {
    WorktreeInfo {
        path: w.path.clone(),
        head: w.head.clone(),
        branch: w.branch(),
        detached: w.detached,
        bare: w.bare,
        locked: w.locked,
        prunable: w.prunable,
        is_main,
        managed: rec.is_some(),
        agent_id: rec.and_then(|r| r.agent_id),
        task_id: rec.and_then(|r| r.task_id),
        role: rec.and_then(|r| r.role.clone()),
        base_commit: rec.map(|r| r.base_commit.clone()),
        created_at: rec.map(|r| r.created_at),
        dirty,
    }
}
