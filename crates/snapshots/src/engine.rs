//! [`SnapshotService`] — create/list/diff snapshots and roll back (§25, §63.J).
//!
//! Invariants:
//! * snapshots never touch git (no commits/refs/stash/index changes);
//! * every rollback (workspace, task, single file) first takes a
//!   `PreRollback` safety snapshot, so it can itself be undone;
//! * rollbacks only overwrite a file when its current content is exactly the
//!   content the plan was computed against (re-checked right before writing);
//!   anything else is reported as a conflict and left untouched;
//! * task rollback only touches files the task changed (before ≠ after), and
//!   only when the file still holds the task's after-state. In a shared
//!   workspace the after-checkpoint records the paths the task itself wrote
//!   ([`Manifest::task_files`]); other paths that differ were changed by
//!   another agent or the user and are reported as conflicts, never reverted;
//! * a file missing from a snapshot is only deleted as "created after it" when
//!   it verifiably did not exist then: a path the snapshot did not capture
//!   (ignored by the rules in force then, unreadable, over the size limit) is
//!   kept and reported;
//! * a file that cannot be read right now is never treated as missing.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use dashmap::DashMap;
use orkestr_domain::{
    Actor, AgentId, ProjectId, SnapshotId, SnapshotReason, TaskId, TaskSnapshotPhase, WorkspaceSnapshot,
};
use orkestr_protocol::methods::snapshots::{
    ExcludedEntry, ManifestEntry, RollbackAction, RollbackConflict, RollbackPlan, RollbackPlanItem, RollbackResult,
    SnapshotChange, SnapshotChangeKind, SnapshotDetail, SnapshotDiffResult, SnapshotFileContents,
};
use orkestr_storage::{Db, ObjectId, ObjectStore};
use serde::{Deserialize, Serialize};
use tokio::sync::{Mutex, OwnedMutexGuard};

use crate::error::{SnapshotError, SnapshotResult};
use crate::scan::{self, FileState, ScanOptions};
use crate::store::SnapshotStore;

/// Max bytes of a text file returned by `diff` with a path.
const MAX_CONTENT_BYTES: usize = 8 * 1024 * 1024;

/// Git metadata of a workspace at snapshot time.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GitState {
    /// HEAD commit.
    pub head: Option<String>,
    /// Branch.
    pub branch: Option<String>,
    /// Uncommitted changes present.
    pub dirty: bool,
}

/// Source of git metadata (implemented for [`orkestr_vcs::GitCli`]).
#[async_trait]
pub trait GitStateProvider: Send + Sync {
    /// Git state of `dir`, `None` when not a repository / git missing.
    async fn git_state(&self, dir: &Path) -> Option<GitState>;
}

#[async_trait]
impl GitStateProvider for orkestr_vcs::GitCli {
    async fn git_state(&self, dir: &Path) -> Option<GitState> {
        self.head_state(dir).await.map(|(head, branch, dirty)| GitState { head, branch, dirty })
    }
}

/// Manifest stored as a JSON object in the object store.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Manifest {
    /// Format version (1).
    pub version: u32,
    /// Captured directory.
    pub root: String,
    /// Files, sorted by path.
    pub entries: Vec<ManifestEntry>,
    /// Skipped paths.
    pub excluded: Vec<ExcludedEntry>,
    /// Git metadata.
    pub git: Option<GitState>,
    /// Paths the snapshot's task changed itself (files or directories),
    /// recorded on a task's after-checkpoint in a workspace shared with other
    /// writers. `None`: unknown — every difference counts as the task's
    /// (a dedicated worktree, older snapshots).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_files: Option<Vec<String>>,
}

impl Manifest {
    fn map(&self) -> BTreeMap<&str, &ManifestEntry> {
        self.entries.iter().map(|e| (e.path.as_str(), e)).collect()
    }

    /// Whether the task of this (after-)checkpoint changed `path` itself.
    fn task_changed(&self, path: &str) -> bool {
        let Some(files) = &self.task_files else { return true };
        files.iter().any(|f| under(path, f))
    }

    /// Whether `path` was skipped while capturing (symlink, unreadable, over
    /// the size limit) — its absence from the entries proves nothing.
    fn excluded(&self, path: &str) -> bool {
        self.excluded.iter().any(|e| under(path, &e.path))
    }
}

/// `path` equals `dir` or lies below it (`/`-separated relative paths).
fn under(path: &str, dir: &str) -> bool {
    path == dir || (path.len() > dir.len() && path.starts_with(dir) && path.as_bytes()[dir.len()] == b'/')
}

/// Normalizes a recorded relative path (`\\` → `/`, no `./`, no trailing `/`).
fn normalize_rel(p: &str) -> Option<String> {
    let p = p.replace('\\', "/");
    let parts: Vec<&str> = p.split('/').filter(|c| !c.is_empty() && *c != ".").collect();
    (!parts.is_empty()).then(|| parts.join("/"))
}

/// Ignore-file names honored by the scanner, by increasing precedence.
const IGNORE_FILES: &[&str] = &[".gitignore", ".ignore", ".orkestrignore"];

/// The workspace ignore rules captured in a snapshot (its `.gitignore`,
/// `.ignore` and `.orkestrignore` files), to tell whether a path the snapshot
/// lacks was absent or merely ignored at the time.
struct CapturedIgnores {
    /// (directory of the ignore file, matcher), shallow and low-precedence first.
    rules: Vec<(String, ignore::gitignore::Gitignore)>,
}

impl CapturedIgnores {
    /// Whether the captured rules ignore `path` (deeper / higher-precedence
    /// files win; `!` whitelists un-ignore).
    fn ignores(&self, path: &str) -> bool {
        let mut ignored = false;
        for (dir, gi) in &self.rules {
            let sub = if dir.is_empty() {
                path
            } else {
                match path.strip_prefix(dir.as_str()).and_then(|r| r.strip_prefix('/')) {
                    Some(r) => r,
                    None => continue,
                }
            };
            match gi.matched_path_or_any_parents(sub, false) {
                ignore::Match::Ignore(_) => ignored = true,
                ignore::Match::Whitelist(_) => ignored = false,
                ignore::Match::None => {}
            }
        }
        ignored
    }
}

/// Plan reason of a file a snapshot did not capture (exists now).
const REASON_UNCAPTURED: &str = "not captured by the snapshot (ignored or unreadable then) — kept";
/// Plan reason of a file that cannot be read now.
const REASON_UNREADABLE: &str = "cannot be read now (locked or access denied) — left untouched";

/// Parameters of [`SnapshotService::create`].
#[derive(Debug, Clone)]
pub struct CreateSnapshot {
    /// Project.
    pub project_id: ProjectId,
    /// Directory to capture.
    pub workspace_path: PathBuf,
    /// Label.
    pub label: Option<String>,
    /// Reason.
    pub reason: SnapshotReason,
    /// Task.
    pub task_id: Option<TaskId>,
    /// Agent.
    pub agent_id: Option<AgentId>,
    /// Checkpoint phase.
    pub task_phase: Option<TaskSnapshotPhase>,
    /// Creator.
    pub created_by: Actor,
}

impl CreateSnapshot {
    /// A manual snapshot of `path`.
    #[must_use]
    pub fn manual(project_id: ProjectId, path: impl Into<PathBuf>) -> Self {
        Self {
            project_id,
            workspace_path: path.into(),
            label: None,
            reason: SnapshotReason::Manual,
            task_id: None,
            agent_id: None,
            task_phase: None,
            created_by: Actor::System,
        }
    }
}

/// Options of a workspace rollback.
#[derive(Debug, Clone, Default)]
pub struct WorkspaceRollback {
    /// Delete files created after the snapshot.
    pub delete_created: bool,
    /// Only these paths.
    pub paths: Option<Vec<String>>,
    /// Refuse if the plan fingerprint differs.
    pub expected_fingerprint: Option<String>,
}

/// The snapshot engine.
pub struct SnapshotService {
    store: SnapshotStore,
    objects: Arc<dyn ObjectStore>,
    git: Option<Arc<dyn GitStateProvider>>,
    options: ScanOptions,
    locks: DashMap<String, Arc<Mutex<()>>>,
}

impl std::fmt::Debug for SnapshotService {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SnapshotService").field("options", &self.options).finish_non_exhaustive()
    }
}

/// Normalized comparison key of a directory path.
#[must_use]
pub fn workspace_key(p: &Path) -> String {
    orkestr_vcs::naming::path_key(p)
}

fn canonical_dir(p: &Path) -> SnapshotResult<PathBuf> {
    if !p.is_absolute() {
        return Err(SnapshotError::invalid("workspace path must be absolute"));
    }
    if !p.is_dir() {
        return Err(SnapshotError::NotFound(format!("workspace directory {} does not exist", p.display())));
    }
    Ok(p.to_path_buf())
}

fn default_label(reason: SnapshotReason) -> &'static str {
    match reason {
        SnapshotReason::Manual => "Manual snapshot",
        SnapshotReason::BeforeRiskyAction => "Before risky action",
        SnapshotReason::TaskCheckpoint => "Task checkpoint",
        SnapshotReason::PreRollback => "Before rollback",
        SnapshotReason::Periodic => "Periodic snapshot",
    }
}

fn entry_state(e: &ManifestEntry) -> FileState {
    (e.hash.clone(), e.mode, e.size)
}

fn same(a: Option<&FileState>, b: Option<&FileState>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.0 == y.0 && x.1 == y.1,
        _ => false,
    }
}

fn same_content(a: Option<&FileState>, b: Option<&FileState>) -> bool {
    match (a, b) {
        (None, None) => true,
        (Some(x), Some(y)) => x.0 == y.0,
        _ => false,
    }
}

fn fingerprint(items: &[RollbackPlanItem]) -> String {
    let mut h = blake3::Hasher::new();
    for i in items {
        h.update(i.path.as_bytes());
        h.update(b"\0");
        h.update(i.current_hash.as_deref().unwrap_or("-").as_bytes());
        h.update(b"\0");
        h.update(format!("{:?}", i.action).as_bytes());
        h.update(b"\n");
    }
    h.finalize().to_hex().to_string()
}

impl SnapshotService {
    /// Creates the service.
    #[must_use]
    pub fn new(
        db: Db,
        objects: Arc<dyn ObjectStore>,
        git: Option<Arc<dyn GitStateProvider>>,
        options: ScanOptions,
    ) -> Self {
        Self { store: SnapshotStore::new(db), objects, git, options, locks: DashMap::new() }
    }

    async fn lock(&self, key: &str) -> OwnedMutexGuard<()> {
        let m = self.locks.entry(key.to_owned()).or_insert_with(|| Arc::new(Mutex::new(()))).clone();
        m.lock_owned().await
    }

    /// Loads a snapshot record.
    pub async fn get(&self, id: SnapshotId) -> SnapshotResult<WorkspaceSnapshot> {
        Ok(self.load(id).await?.0)
    }

    async fn load(&self, id: SnapshotId) -> SnapshotResult<(WorkspaceSnapshot, String)> {
        self.store.get(id).await?.ok_or_else(|| SnapshotError::NotFound(format!("snapshot {id} not found")))
    }

    async fn manifest(&self, snap: &WorkspaceSnapshot) -> SnapshotResult<Manifest> {
        let id = ObjectId::parse(&snap.manifest_object_id)?;
        let bytes = self
            .objects
            .get(&id)
            .await?
            .ok_or_else(|| SnapshotError::NotFound(format!("manifest of snapshot {} is missing", snap.id)))?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    async fn blob(&self, hash: &str) -> SnapshotResult<Vec<u8>> {
        let id = ObjectId::parse(hash)?;
        self.objects
            .get(&id)
            .await?
            .ok_or_else(|| SnapshotError::NotFound(format!("snapshot object {hash} is missing")))
    }

    fn root_of(snap: &WorkspaceSnapshot) -> SnapshotResult<PathBuf> {
        let p =
            snap.workspace_path.as_deref().ok_or_else(|| SnapshotError::invalid("snapshot has no workspace path"))?;
        canonical_dir(Path::new(p))
    }

    /// Snapshot + manifest.
    pub async fn detail(&self, id: SnapshotId) -> SnapshotResult<SnapshotDetail> {
        let snap = self.get(id).await?;
        let m = self.manifest(&snap).await?;
        Ok(SnapshotDetail { snapshot: snap, entries: m.entries, excluded: m.excluded })
    }

    /// Lists snapshots (newest first).
    pub async fn list(
        &self,
        project_id: ProjectId,
        task_id: Option<TaskId>,
        workspace_path: Option<&Path>,
        limit: u32,
    ) -> SnapshotResult<Vec<WorkspaceSnapshot>> {
        let key = workspace_path.map(workspace_key);
        Ok(self.store.list(project_id, task_id, key, limit.clamp(1, 1000)).await?)
    }

    /// Deletes a snapshot record (content objects stay until GC).
    pub async fn delete(&self, id: SnapshotId) -> SnapshotResult<()> {
        if self.store.delete(id).await? {
            Ok(())
        } else {
            Err(SnapshotError::NotFound(format!("snapshot {id} not found")))
        }
    }

    /// Captures every non-ignored file of a workspace.
    pub async fn create(&self, req: CreateSnapshot) -> SnapshotResult<WorkspaceSnapshot> {
        let root = canonical_dir(&req.workspace_path)?;
        let key = workspace_key(&root);
        let _guard = self.lock(&key).await;
        self.create_locked(req, &root, &key, None).await
    }

    /// Captures a task's checkpoint in a workspace shared with other writers
    /// (other agents, the user), recording the paths the task changed itself
    /// so that rolling the task back never reverts anyone else's work.
    pub async fn create_with_task_files(
        &self,
        req: CreateSnapshot,
        task_files: &[String],
    ) -> SnapshotResult<WorkspaceSnapshot> {
        let root = canonical_dir(&req.workspace_path)?;
        let key = workspace_key(&root);
        let mut files: Vec<String> = task_files.iter().filter_map(|f| normalize_rel(f)).collect();
        files.sort();
        files.dedup();
        let _guard = self.lock(&key).await;
        self.create_locked(req, &root, &key, Some(files)).await
    }

    async fn create_locked(
        &self,
        req: CreateSnapshot,
        root: &Path,
        key: &str,
        task_files: Option<Vec<String>>,
    ) -> SnapshotResult<WorkspaceSnapshot> {
        let started = std::time::Instant::now();
        let git = match &self.git {
            Some(g) => g.git_state(root).await,
            None => None,
        };
        let opts = self.options.clone();
        let scan_root = root.to_path_buf();
        let (files, mut excluded) = tokio::task::spawn_blocking(move || scan::list_files(&scan_root, &opts))
            .await
            .map_err(|e| SnapshotError::Io(std::io::Error::other(e.to_string())))??;

        let mut entries = Vec::with_capacity(files.len());
        let mut total: u64 = 0;
        let mut new_bytes: u64 = 0;
        for f in files {
            let bytes = match tokio::fs::read(&f.abs).await {
                Ok(b) => b,
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(_) => {
                    excluded.push(ExcludedEntry {
                        path: f.rel,
                        reason: orkestr_protocol::methods::snapshots::ExclusionReason::Unreadable,
                    });
                    continue;
                }
            };
            let size = bytes.len() as u64;
            total += size;
            if total > self.options.max_total_bytes {
                return Err(SnapshotError::TooLarge(format!(
                    "workspace content exceeds {} bytes; add ignore rules (.gitignore / .orkestrignore)",
                    self.options.max_total_bytes
                )));
            }
            // `put_checked` verifies an existing object and rewrites it when
            // corrupt, so a damaged object is never silently reused.
            let (id, written) = self.objects.put_checked(bytes).await?;
            if written {
                new_bytes += size;
            }
            entries.push(ManifestEntry { path: f.rel, hash: id.as_str().to_owned(), mode: f.mode, size });
        }
        excluded.sort_by(|a, b| a.path.cmp(&b.path));

        let root_str = root.to_string_lossy().into_owned();
        let manifest = Manifest { version: 1, root: root_str.clone(), entries, excluded, git: git.clone(), task_files };
        let manifest_bytes = serde_json::to_vec(&manifest)?;
        let manifest_id = self.objects.put(manifest_bytes).await?;
        let workspace_id = self.store.workspace_id(req.project_id, key, &root_str).await?;
        let label = req
            .label
            .map(|l| l.trim().chars().take(200).collect::<String>())
            .filter(|l| !l.is_empty())
            .unwrap_or_else(|| default_label(req.reason).to_owned());
        let snap = WorkspaceSnapshot {
            id: SnapshotId::new(),
            project_id: req.project_id,
            workspace_id,
            task_id: req.task_id,
            label,
            reason: req.reason,
            base_commit: git.as_ref().and_then(|g| g.head.clone()),
            manifest_object_id: manifest_id.as_str().to_owned(),
            includes_uncommitted: true,
            file_count: manifest.entries.len() as u64,
            size_bytes: total,
            created_by: req.created_by,
            created_at: chrono::Utc::now(),
            workspace_path: Some(root_str),
            branch: git.as_ref().and_then(|g| g.branch.clone()),
            dirty: git.as_ref().map(|g| g.dirty),
            agent_id: req.agent_id,
            task_phase: req.task_phase,
            new_bytes: Some(new_bytes),
        };
        self.store.insert(&snap, key).await?;
        tracing::info!(
            snapshot_id = %snap.id,
            files = snap.file_count,
            bytes = total,
            new_bytes,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "snapshot created"
        );
        Ok(snap)
    }

    async fn current_tree(&self, root: &Path) -> SnapshotResult<scan::CurrentTree> {
        let opts = self.options.clone();
        let root = root.to_path_buf();
        tokio::task::spawn_blocking(move || scan::current_tree(&root, &opts))
            .await
            .map_err(|e| SnapshotError::Io(std::io::Error::other(e.to_string())))?
    }

    /// The ignore rules captured in `m`.
    async fn captured_ignores(&self, m: &Manifest) -> CapturedIgnores {
        let mut rules = Vec::new();
        for e in &m.entries {
            let (dir, name) = e.path.rsplit_once('/').unwrap_or(("", e.path.as_str()));
            let Some(rank) = IGNORE_FILES.iter().position(|f| *f == name) else { continue };
            let bytes = match self.blob(&e.hash).await {
                Ok(b) => b,
                Err(err) => {
                    tracing::warn!(path = %e.path, error = %err, "ignore file of a snapshot is unavailable");
                    continue;
                }
            };
            let mut b = ignore::gitignore::GitignoreBuilder::new("");
            for line in String::from_utf8_lossy(&bytes).lines() {
                let _ = b.add_line(None, line);
            }
            if let Ok(gi) = b.build() {
                let depth = if dir.is_empty() { 0 } else { dir.split('/').count() };
                rules.push((depth, rank, dir.to_owned(), gi));
            }
        }
        rules.sort_by_key(|r| (r.0, r.1));
        CapturedIgnores { rules: rules.into_iter().map(|(_, _, dir, gi)| (dir, gi)).collect() }
    }

    /// Whether `path`, missing from snapshot `m`, may nevertheless have
    /// existed when `m` was taken (excluded, or ignored by the rules then).
    fn uncaptured(m: &Manifest, rules: &CapturedIgnores, path: &str) -> bool {
        m.excluded(path) || rules.ignores(path)
    }

    /// Differences between a snapshot and the current workspace; with `path`,
    /// both contents of that file.
    pub async fn diff(&self, id: SnapshotId, path: Option<&str>) -> SnapshotResult<SnapshotDiffResult> {
        let snap = self.get(id).await?;
        let root = Self::root_of(&snap)?;
        let manifest = self.manifest(&snap).await?;
        let tree = self.current_tree(&root).await?;
        let current = &tree.files;
        let snap_map = manifest.map();
        let mut changes = Vec::new();
        for (p, e) in &snap_map {
            if tree.unreadable.contains(*p) {
                // Exists but cannot be read now: state unknown, not "deleted".
                continue;
            }
            match current.get(*p) {
                None => changes.push(SnapshotChange {
                    path: (*p).to_owned(),
                    kind: SnapshotChangeKind::Deleted,
                    snapshot_hash: Some(e.hash.clone()),
                    current_hash: None,
                }),
                Some(c) if !same(Some(&entry_state(e)), Some(c)) => changes.push(SnapshotChange {
                    path: (*p).to_owned(),
                    kind: SnapshotChangeKind::Modified,
                    snapshot_hash: Some(e.hash.clone()),
                    current_hash: Some(c.0.clone()),
                }),
                Some(_) => {}
            }
        }
        for (p, c) in current {
            if !snap_map.contains_key(p.as_str()) {
                changes.push(SnapshotChange {
                    path: p.clone(),
                    kind: SnapshotChangeKind::Added,
                    snapshot_hash: None,
                    current_hash: Some(c.0.clone()),
                });
            }
        }
        changes.sort_by(|a, b| a.path.cmp(&b.path));

        let file = match path {
            None => None,
            Some(p) => {
                scan::validate_rel(p)?;
                let snap_bytes = match snap_map.get(p) {
                    Some(e) => Some(self.blob(&e.hash).await?),
                    None => None,
                };
                let target = scan::safe_target(&root, p)?;
                let cur_bytes = match tokio::fs::read(&target).await {
                    Ok(b) => Some(b),
                    Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
                    Err(e) => return Err(e.into()),
                };
                let is_bin = |b: &Option<Vec<u8>>| {
                    b.as_ref().is_some_and(|b| b.len() > MAX_CONTENT_BYTES || b.iter().take(8000).any(|x| *x == 0))
                };
                let binary = is_bin(&snap_bytes) || is_bin(&cur_bytes);
                let text = |b: Option<Vec<u8>>| {
                    b.map(|b| if binary { String::new() } else { String::from_utf8_lossy(&b).into_owned() })
                };
                Some(SnapshotFileContents {
                    path: p.to_owned(),
                    snapshot: text(snap_bytes),
                    current: text(cur_bytes),
                    binary,
                })
            }
        };
        Ok(SnapshotDiffResult { snapshot_id: id, changes, file })
    }

    /// Plans a workspace rollback (no changes made).
    pub async fn plan_rollback(&self, id: SnapshotId, delete_created: bool) -> SnapshotResult<RollbackPlan> {
        let snap = self.get(id).await?;
        let root = Self::root_of(&snap)?;
        let manifest = self.manifest(&snap).await?;
        self.plan_with(&snap, &root, &manifest, delete_created).await
    }

    async fn plan_with(
        &self,
        snap: &WorkspaceSnapshot,
        root: &Path,
        manifest: &Manifest,
        delete_created: bool,
    ) -> SnapshotResult<RollbackPlan> {
        let tree = self.current_tree(root).await?;
        let current = &tree.files;
        let snap_map = manifest.map();
        let mut items = Vec::new();
        for p in &tree.unreadable {
            items.push(RollbackPlanItem {
                path: p.clone(),
                action: RollbackAction::Keep,
                reason: REASON_UNREADABLE.into(),
                current_hash: None,
                target_hash: snap_map.get(p.as_str()).map(|e| e.hash.clone()),
            });
        }
        for (p, e) in &snap_map {
            if tree.unreadable.contains(*p) {
                continue;
            }
            match current.get(*p) {
                None => items.push(RollbackPlanItem {
                    path: (*p).to_owned(),
                    action: RollbackAction::Recreate,
                    reason: "deleted since the snapshot".into(),
                    current_hash: None,
                    target_hash: Some(e.hash.clone()),
                }),
                Some(c) if !same(Some(&entry_state(e)), Some(c)) => items.push(RollbackPlanItem {
                    path: (*p).to_owned(),
                    action: RollbackAction::Restore,
                    reason: if c.0 == e.hash {
                        "mode changed since the snapshot".into()
                    } else {
                        "modified since the snapshot".into()
                    },
                    current_hash: Some(c.0.clone()),
                    target_hash: Some(e.hash.clone()),
                }),
                Some(_) => {}
            }
        }
        let created: Vec<(&String, &FileState)> =
            current.iter().filter(|(p, _)| !snap_map.contains_key(p.as_str())).collect();
        let rules =
            if delete_created && !created.is_empty() { Some(self.captured_ignores(manifest).await) } else { None };
        for (p, c) in created {
            let (action, reason) = if rules.as_ref().is_some_and(|r| Self::uncaptured(manifest, r, p)) {
                // It may well have existed when the snapshot was taken.
                (RollbackAction::Keep, REASON_UNCAPTURED)
            } else if delete_created {
                (RollbackAction::Delete, "created after the snapshot")
            } else {
                (RollbackAction::Keep, "created after the snapshot — kept (enable “delete created files” to remove)")
            };
            items.push(RollbackPlanItem {
                path: p.clone(),
                action,
                reason: reason.into(),
                current_hash: Some(c.0.clone()),
                target_hash: if action == RollbackAction::Keep { Some(c.0.clone()) } else { None },
            });
        }
        items.sort_by(|a, b| a.path.cmp(&b.path));
        let fp = fingerprint(&items);
        Ok(RollbackPlan {
            snapshot_id: snap.id,
            workspace_path: root.to_string_lossy().into_owned(),
            items,
            fingerprint: fp,
        })
    }

    async fn safety_snapshot(
        &self,
        snap: &WorkspaceSnapshot,
        root: &Path,
        key: &str,
        task_id: Option<TaskId>,
        label: String,
    ) -> SnapshotResult<WorkspaceSnapshot> {
        self.create_locked(
            CreateSnapshot {
                project_id: snap.project_id,
                workspace_path: root.to_path_buf(),
                label: Some(label),
                reason: SnapshotReason::PreRollback,
                task_id,
                agent_id: None,
                task_phase: None,
                created_by: Actor::System,
            },
            root,
            key,
            None,
        )
        .await
    }

    /// Applies one planned change if the file still has `expected` content.
    async fn apply_one(
        &self,
        root: &Path,
        path: &str,
        expected: Option<&str>,
        target: Option<&ManifestEntry>,
    ) -> Result<bool, String> {
        let cur = scan::current_state(root, path).map_err(|e| e.to_string())?;
        if cur.as_ref().map(|c| c.0.as_str()) != expected {
            return Err("changed after the rollback was planned — left untouched".into());
        }
        match target {
            Some(e) => {
                let bytes = self.blob(&e.hash).await.map_err(|e| e.to_string())?;
                if scan::hash_bytes(&bytes) != e.hash {
                    return Err("snapshot content is corrupt".into());
                }
                scan::write_file(root, path, &bytes, e.mode).map_err(|e| e.to_string())?;
                Ok(true)
            }
            None => {
                scan::delete_file(root, path).map_err(|e| e.to_string())?;
                Ok(false)
            }
        }
    }

    /// Restores a workspace to a snapshot (after a `PreRollback` snapshot).
    /// Files created after the snapshot are kept unless `delete_created`.
    pub async fn rollback_workspace(&self, id: SnapshotId, opts: WorkspaceRollback) -> SnapshotResult<RollbackResult> {
        let (snap, _) = self.load(id).await?;
        let root = Self::root_of(&snap)?;
        let key = workspace_key(&root);
        let _guard = self.lock(&key).await;
        let manifest = self.manifest(&snap).await?;
        let plan = self.plan_with(&snap, &root, &manifest, opts.delete_created).await?;
        if let Some(fp) = &opts.expected_fingerprint {
            if *fp != plan.fingerprint {
                return Err(SnapshotError::Stale(
                    "the workspace changed since the rollback was planned — review the new plan".into(),
                ));
            }
        }
        let selected: Option<BTreeSet<&str>> = opts.paths.as_ref().map(|p| p.iter().map(String::as_str).collect());
        let todo: Vec<&RollbackPlanItem> = plan
            .items
            .iter()
            .filter(|i| i.action != RollbackAction::Keep)
            .filter(|i| selected.as_ref().is_none_or(|s| s.contains(i.path.as_str())))
            .collect();
        let mut result = RollbackResult {
            snapshot_id: id,
            pre_rollback_snapshot_id: None,
            restored: Vec::new(),
            deleted: Vec::new(),
            conflicts: Vec::new(),
            unchanged: Vec::new(),
            dry_run: false,
        };
        if todo.is_empty() {
            return Ok(result);
        }
        let safety =
            self.safety_snapshot(&snap, &root, &key, None, format!("Before rollback to “{}”", snap.label)).await?;
        result.pre_rollback_snapshot_id = Some(safety.id);
        let snap_map = manifest.map();
        // Deletions first, so a file can replace a deleted directory and vice versa.
        let mut ordered = todo.clone();
        ordered.sort_by_key(|i| (i.action != RollbackAction::Delete, i.path.clone()));
        for item in ordered {
            let target = snap_map.get(item.path.as_str()).copied();
            let target = if item.action == RollbackAction::Delete { None } else { target };
            match self.apply_one(&root, &item.path, item.current_hash.as_deref(), target).await {
                Ok(true) => result.restored.push(item.path.clone()),
                Ok(false) => result.deleted.push(item.path.clone()),
                Err(reason) => result.conflicts.push(RollbackConflict { path: item.path.clone(), reason }),
            }
        }
        tracing::info!(
            snapshot_id = %id,
            restored = result.restored.len(),
            deleted = result.deleted.len(),
            conflicts = result.conflicts.len(),
            "workspace rolled back"
        );
        Ok(result)
    }

    /// Restores one file from a snapshot (after a `PreRollback` snapshot).
    pub async fn restore_file(&self, id: SnapshotId, path: &str) -> SnapshotResult<RollbackResult> {
        scan::validate_rel(path)?;
        let (snap, _) = self.load(id).await?;
        let root = Self::root_of(&snap)?;
        let key = workspace_key(&root);
        let _guard = self.lock(&key).await;
        let manifest = self.manifest(&snap).await?;
        let entry = manifest
            .entries
            .iter()
            .find(|e| e.path == path)
            .ok_or_else(|| SnapshotError::NotFound(format!("{path} is not in snapshot {id}")))?;
        let cur = scan::current_state(&root, path)?;
        let mut result = RollbackResult {
            snapshot_id: id,
            pre_rollback_snapshot_id: None,
            restored: Vec::new(),
            deleted: Vec::new(),
            conflicts: Vec::new(),
            unchanged: Vec::new(),
            dry_run: false,
        };
        if same(cur.as_ref(), Some(&entry_state(entry))) {
            result.unchanged.push(path.to_owned());
            return Ok(result);
        }
        let safety = self.safety_snapshot(&snap, &root, &key, None, format!("Before restoring {path}")).await?;
        result.pre_rollback_snapshot_id = Some(safety.id);
        match self.apply_one(&root, path, cur.as_ref().map(|c| c.0.as_str()), Some(entry)).await {
            Ok(_) => result.restored.push(path.to_owned()),
            Err(reason) => result.conflicts.push(RollbackConflict { path: path.to_owned(), reason }),
        }
        Ok(result)
    }

    /// Finds a task's checkpoint pair: explicit ids, else the earliest
    /// `Before` (or oldest) and the latest `After` (or newest other) snapshot.
    pub async fn task_pair(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
        before: Option<SnapshotId>,
        after: Option<SnapshotId>,
    ) -> SnapshotResult<(WorkspaceSnapshot, WorkspaceSnapshot)> {
        let mut all = self.store.list(project_id, Some(task_id), None, 1000).await?;
        all.reverse(); // oldest first
        all.retain(|s| s.reason != SnapshotReason::PreRollback);
        let pick = |id: SnapshotId| -> SnapshotResult<WorkspaceSnapshot> {
            all.iter()
                .find(|s| s.id == id)
                .cloned()
                .ok_or_else(|| SnapshotError::NotFound(format!("snapshot {id} does not belong to task {task_id}")))
        };
        let b = match before {
            Some(id) => pick(id)?,
            None => all
                .iter()
                .find(|s| s.task_phase == Some(TaskSnapshotPhase::Before))
                .or_else(|| all.iter().find(|s| s.task_phase != Some(TaskSnapshotPhase::After)))
                .cloned()
                .ok_or_else(|| SnapshotError::NotFound(format!("task {task_id} has no checkpoint snapshot")))?,
        };
        let a = match after {
            Some(id) => pick(id)?,
            None => all
                .iter()
                .rev()
                .find(|s| s.task_phase == Some(TaskSnapshotPhase::After) && s.id != b.id)
                .or_else(|| all.iter().rev().find(|s| s.id != b.id && s.created_at >= b.created_at))
                .cloned()
                .ok_or_else(|| {
                    SnapshotError::NotFound(format!(
                        "task {task_id} has no after-checkpoint; take one when the task stops (snapshots.create with taskPhase After)"
                    ))
                })?,
        };
        if a.id == b.id {
            return Err(SnapshotError::invalid("before and after snapshots must differ"));
        }
        if a.workspace_id != b.workspace_id {
            return Err(SnapshotError::invalid("before and after snapshots belong to different workspaces"));
        }
        Ok((b, a))
    }

    /// Same content, or the same text up to CRLF/LF line endings (a git
    /// checkout with `core.autocrlf` rewrites them).
    async fn same_text(&self, a: Option<&FileState>, b: Option<&FileState>) -> bool {
        if same_content(a, b) {
            return true;
        }
        let (Some(a), Some(b)) = (a, b) else { return false };
        const MAX_TEXT: u64 = 4 * 1024 * 1024;
        if a.2 > MAX_TEXT || b.2 > MAX_TEXT {
            return false;
        }
        let (Ok(x), Ok(y)) = (self.blob(&a.0).await, self.blob(&b.0).await) else { return false };
        let lf = |v: &[u8]| -> Vec<u8> {
            let mut out = Vec::with_capacity(v.len());
            for (i, &c) in v.iter().enumerate() {
                if !(c == b'\r' && v.get(i + 1) == Some(&b'\n')) {
                    out.push(c);
                }
            }
            out
        };
        lf(&x) == lf(&y)
    }

    /// A task's checkpoint pairs, oldest first. Explicit ids give one pair
    /// ([`Self::task_pair`]). Otherwise one pair per workspace the task worked
    /// in: a Self-Healing fix attempt runs in a new worktree branched from
    /// the previous attempt, so the task's history can span several
    /// workspaces.
    pub async fn task_segments(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
        before: Option<SnapshotId>,
        after: Option<SnapshotId>,
    ) -> SnapshotResult<Vec<(WorkspaceSnapshot, WorkspaceSnapshot)>> {
        if before.is_some() || after.is_some() {
            return Ok(vec![self.task_pair(project_id, task_id, before, after).await?]);
        }
        let mut all = self.store.list(project_id, Some(task_id), None, 1000).await?;
        all.reverse(); // oldest first
        all.retain(|s| s.reason != SnapshotReason::PreRollback);
        let mut workspaces = Vec::new();
        for s in &all {
            if !workspaces.contains(&s.workspace_id) {
                workspaces.push(s.workspace_id);
            }
        }
        if workspaces.len() <= 1 {
            return Ok(vec![self.task_pair(project_id, task_id, None, None).await?]);
        }
        let mut segments = Vec::new();
        for ws in workspaces {
            let own: Vec<&WorkspaceSnapshot> = all.iter().filter(|s| s.workspace_id == ws).collect();
            let Some(b) = own
                .iter()
                .find(|s| s.task_phase == Some(TaskSnapshotPhase::Before))
                .or_else(|| own.iter().find(|s| s.task_phase != Some(TaskSnapshotPhase::After)))
            else {
                continue;
            };
            let a = own
                .iter()
                .rev()
                .find(|s| s.task_phase == Some(TaskSnapshotPhase::After) && s.id != b.id)
                .or_else(|| own.iter().rev().find(|s| s.id != b.id && s.created_at >= b.created_at));
            if let Some(a) = a {
                segments.push(((*b).clone(), (*a).clone()));
            }
        }
        if segments.is_empty() {
            return Err(SnapshotError::NotFound(format!(
                "task {task_id} has no before/after checkpoint pair in any of its workspaces"
            )));
        }
        segments.sort_by_key(|(b, _)| b.created_at);
        Ok(segments)
    }

    /// Rolls back exactly the changes a task made between its before/after
    /// checkpoints (§63.J), in the workspace of its latest checkpoint. For
    /// each file the task touched: current == after-state → restore the
    /// before-state; current == before-state → already reverted; otherwise
    /// someone else changed it → conflict, untouched. Unrelated files are
    /// never read for writing.
    ///
    /// A task that worked in several workspaces (Self-Healing fix attempts)
    /// is rolled back over all of them: a file's target is its state before
    /// the first attempt that touched it, its expected state the one after
    /// the last attempt. A file that changed between two attempts outside
    /// the task (the next attempt did not start from the previous one's
    /// result) is a conflict and stays untouched.
    pub async fn rollback_task(
        &self,
        project_id: ProjectId,
        task_id: TaskId,
        before: Option<SnapshotId>,
        after: Option<SnapshotId>,
        dry_run: bool,
    ) -> SnapshotResult<RollbackResult> {
        let segments = self.task_segments(project_id, task_id, before, after).await?;
        let b = segments[0].0.clone();
        let a = segments[segments.len() - 1].1.clone();
        let root = Self::root_of(&a)?;
        let key = workspace_key(&root);
        let _guard = self.lock(&key).await;
        let mut manifests = Vec::with_capacity(segments.len());
        for (sb, sa) in &segments {
            manifests.push((self.manifest(sb).await?, self.manifest(sa).await?));
        }
        let maps: Vec<_> = manifests.iter().map(|(mb, ma)| (mb.map(), ma.map())).collect();
        let state = |m: &BTreeMap<&str, &ManifestEntry>, p: &str| m.get(p).map(|e| entry_state(e));
        // Path → index of the first segment in which the task changed it.
        // A path that differs only where the after-checkpoint records that
        // the task did not write it was changed by someone else working in
        // the same (shared) workspace meanwhile.
        let mut touched: BTreeMap<&str, usize> = BTreeMap::new();
        let mut foreign: BTreeSet<&str> = BTreeSet::new();
        for (i, (before_map, after_map)) in maps.iter().enumerate() {
            for p in before_map.keys().chain(after_map.keys()).copied() {
                if touched.contains_key(p) || same(state(before_map, p).as_ref(), state(after_map, p).as_ref()) {
                    continue;
                }
                if manifests[i].1.task_changed(p) {
                    touched.insert(p, i);
                    foreign.remove(p);
                } else {
                    foreign.insert(p);
                }
            }
        }
        let last = maps.len() - 1;
        // Ignore rules of each segment's before-checkpoint, loaded on demand.
        let mut rules: BTreeMap<usize, CapturedIgnores> = BTreeMap::new();

        let mut result = RollbackResult {
            snapshot_id: b.id,
            pre_rollback_snapshot_id: None,
            restored: Vec::new(),
            deleted: Vec::new(),
            conflicts: Vec::new(),
            unchanged: Vec::new(),
            dry_run,
        };
        for p in &foreign {
            result.conflicts.push(RollbackConflict {
                path: (*p).to_owned(),
                reason: "changed during the task by someone else in the shared workspace — not reverted".into(),
            });
        }
        // (path, current hash, before entry)
        let mut actions: Vec<(&str, Option<String>, Option<&ManifestEntry>)> = Vec::new();
        for (&p, &first) in &touched {
            // Every later attempt must have started from the previous one's
            // result (a fresh checkout may differ only in line endings).
            let mut gap = false;
            for j in first..last {
                if !self.same_text(state(&maps[j].1, p).as_ref(), state(&maps[j + 1].0, p).as_ref()).await {
                    gap = true;
                    break;
                }
            }
            if gap {
                result.conflicts.push(RollbackConflict {
                    path: p.to_owned(),
                    reason: "changed outside the task between its attempts — not overwritten".into(),
                });
                continue;
            }
            let cur = match scan::current_state(&root, p) {
                Ok(c) => c,
                Err(e) => {
                    result.conflicts.push(RollbackConflict { path: p.to_owned(), reason: e.to_string() });
                    continue;
                }
            };
            let before_s = state(&maps[first].0, p);
            let after_s = state(&maps[last].1, p);
            if same_content(cur.as_ref(), after_s.as_ref()) {
                let target = maps[first].0.get(p).copied();
                if target.is_none() {
                    // Missing from the before-checkpoint: only delete it when
                    // it verifiably did not exist then.
                    let before_m = &manifests[first].0;
                    let r = match rules.entry(first) {
                        std::collections::btree_map::Entry::Occupied(o) => o.into_mut(),
                        std::collections::btree_map::Entry::Vacant(v) => {
                            v.insert(self.captured_ignores(before_m).await)
                        }
                    };
                    if Self::uncaptured(before_m, r, p) {
                        result.conflicts.push(RollbackConflict {
                            path: p.to_owned(),
                            reason: "not captured by the task's before-checkpoint (ignored or unreadable then), so it may predate the task — not deleted".into(),
                        });
                        continue;
                    }
                }
                actions.push((p, cur.map(|c| c.0), target));
            } else if same_content(cur.as_ref(), before_s.as_ref()) {
                result.unchanged.push(p.to_owned());
            } else {
                result.conflicts.push(RollbackConflict {
                    path: p.to_owned(),
                    reason: "changed by someone else after the task — not overwritten".into(),
                });
            }
        }
        if dry_run || actions.is_empty() {
            for (p, _, target) in &actions {
                if target.is_some() {
                    result.restored.push((*p).to_owned());
                } else {
                    result.deleted.push((*p).to_owned());
                }
            }
            result.conflicts.sort_by(|x, y| x.path.cmp(&y.path));
            return Ok(result);
        }
        let safety = self
            .safety_snapshot(
                &a,
                &root,
                &key,
                Some(task_id),
                format!("Before rolling back task {}", orkestr_vcs::naming::short_id(&task_id.to_string())),
            )
            .await?;
        result.pre_rollback_snapshot_id = Some(safety.id);
        for (p, cur, target) in actions {
            match self.apply_one(&root, p, cur.as_deref(), target).await {
                Ok(true) => result.restored.push(p.to_owned()),
                Ok(false) => result.deleted.push(p.to_owned()),
                Err(reason) => result.conflicts.push(RollbackConflict { path: p.to_owned(), reason }),
            }
        }
        result.conflicts.sort_by(|x, y| x.path.cmp(&y.path));
        tracing::info!(
            task_id = %task_id,
            restored = result.restored.len(),
            deleted = result.deleted.len(),
            conflicts = result.conflicts.len(),
            "task rolled back"
        );
        Ok(result)
    }
}
