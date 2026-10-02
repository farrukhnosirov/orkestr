//! [`FilesManager`]: the async façade used by the core RPC handlers and
//! (later) the agent tool registry. It runs blocking file I/O on the
//! blocking pool, serializes mutations, records actor attribution, trashes
//! deleted files and owns the watchers.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use orkestr_domain::{AgentId, ProjectId};
use orkestr_protocol::event::{FileChangeActor, FileChangeKind, FileChangedPayload};
use orkestr_protocol::methods::files::{
    DeleteFileResult, FileEntry, FindFilesResult, ListFilesResult, OpenTarget, ReadFileResult, RenameFileResult,
    RestoreFileResult, SearchFilesResult, TextEncoding, TrashItem, WriteFileResult,
};

use crate::error::{FilesError, FilesResult};
use crate::open::{ExternalOpener, OpenRequest};
use crate::ops;
use crate::path::{RelPath, WorkspaceRoot};
use crate::search::{self, SearchOptions};
use crate::text;
use crate::trash::{NewTrashEntry, TrashStore};
use crate::watch::{ChangeSink, RecentWrites, WatcherHub};

/// Limits and timings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FilesConfig {
    /// Default read limit.
    pub max_read_bytes: u64,
    /// Hard ceiling for a requested read limit.
    pub max_read_limit: u64,
    /// Max size of a written file.
    pub max_write_bytes: u64,
    /// Max entries returned per directory listing.
    pub max_list_entries: usize,
    /// Max bytes copied into the trash by one delete (bigger deletes are refused).
    pub max_trash_bytes: u64,
    /// Files bigger than this are skipped by content search.
    pub search_max_file_bytes: u64,
    /// Default / max content search results.
    pub search_default_results: usize,
    /// Hard ceiling of content search results.
    pub search_max_results: usize,
    /// Default / max fuzzy find results.
    pub find_default_results: usize,
    /// Hard ceiling of fuzzy find results.
    pub find_max_results: usize,
    /// Max files scanned by fuzzy find.
    pub find_max_scan: usize,
    /// Watcher quiet period.
    pub watch_debounce: Duration,
    /// Window in which self-made changes suppress watcher events.
    pub self_write_window: Duration,
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            max_read_bytes: 5 << 20,
            max_read_limit: 50 << 20,
            max_write_bytes: 50 << 20,
            max_list_entries: 5000,
            max_trash_bytes: 256 << 20,
            search_max_file_bytes: 2 << 20,
            search_default_results: 500,
            search_max_results: 5000,
            find_default_results: 50,
            find_max_results: 500,
            find_max_scan: 200_000,
            watch_debounce: Duration::from_millis(150),
            self_write_window: Duration::from_secs(2),
        }
    }
}

/// A project workspace (project root or agent worktree).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Workspace {
    /// Project.
    pub project_id: ProjectId,
    /// Root.
    pub root: WorkspaceRoot,
}

/// Who performs a mutation through the file service.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeActor {
    /// The user.
    User,
    /// An agent.
    Agent(AgentId),
}

impl ChangeActor {
    /// From an optional agent id (RPC params).
    #[must_use]
    pub fn from_agent(agent: Option<AgentId>) -> Self {
        agent.map_or(Self::User, Self::Agent)
    }

    fn wire(self) -> (FileChangeActor, Option<AgentId>) {
        match self {
            Self::User => (FileChangeActor::User, None),
            Self::Agent(a) => (FileChangeActor::Agent, Some(a)),
        }
    }

    fn change(self, path: String, old_path: Option<String>, change: FileChangeKind) -> FileChangedPayload {
        let (actor, agent_id) = self.wire();
        FileChangedPayload { path, old_path, change, actor, agent_id }
    }
}

/// Result of a mutation plus the `FileChanged` payloads to publish.
#[derive(Debug, Clone, PartialEq)]
pub struct Mutation<T> {
    /// Method result.
    pub result: T,
    /// Changes (publish with the request's correlation id).
    pub changes: Vec<FileChangedPayload>,
}

async fn blocking<T, F>(f: F) -> FilesResult<T>
where
    T: Send + 'static,
    F: FnOnce() -> FilesResult<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(|e| FilesError::Internal(format!("file task failed: {e}")))?
}

/// The file service.
pub struct FilesManager {
    config: FilesConfig,
    trash: Arc<dyn TrashStore>,
    opener: Arc<dyn ExternalOpener>,
    recent: Arc<RecentWrites>,
    watchers: WatcherHub,
    mutation_lock: tokio::sync::Mutex<()>,
}

impl std::fmt::Debug for FilesManager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FilesManager")
            .field("config", &self.config)
            .field("watchers", &self.watchers)
            .finish_non_exhaustive()
    }
}

impl FilesManager {
    /// New manager.
    #[must_use]
    pub fn new(
        config: FilesConfig,
        trash: Arc<dyn TrashStore>,
        opener: Arc<dyn ExternalOpener>,
        sink: Arc<dyn ChangeSink>,
    ) -> Self {
        let recent = Arc::new(RecentWrites::new(config.self_write_window));
        let watchers = WatcherHub::new(sink, Arc::clone(&recent), config.watch_debounce);
        Self { config, trash, opener, recent, watchers, mutation_lock: tokio::sync::Mutex::new(()) }
    }

    /// Configuration.
    #[must_use]
    pub fn config(&self) -> &FilesConfig {
        &self.config
    }

    /// Opens a workspace rooted at an absolute directory.
    pub fn workspace(&self, project_id: ProjectId, root: &Path) -> FilesResult<Workspace> {
        Ok(Workspace { project_id, root: WorkspaceRoot::open(root)? })
    }

    fn record(&self, ws: &Workspace, rel: &RelPath) {
        self.recent.record(ws.root.path(), &rel.as_string());
    }

    /// `files.list`.
    pub async fn list(&self, ws: &Workspace, path: &str, show_ignored: bool) -> FilesResult<ListFilesResult> {
        let rel = RelPath::parse(path)?;
        let root = ws.root.clone();
        let max = self.config.max_list_entries;
        blocking(move || ops::list(&root, &rel, show_ignored, max)).await
    }

    /// `files.stat`.
    pub async fn stat(&self, ws: &Workspace, path: &str) -> FilesResult<FileEntry> {
        let rel = RelPath::parse(path)?;
        let root = ws.root.clone();
        blocking(move || ops::stat(&root, &rel)).await
    }

    /// `files.read`.
    pub async fn read(&self, ws: &Workspace, path: &str, max_bytes: Option<u64>) -> FilesResult<ReadFileResult> {
        let rel = RelPath::parse(path)?;
        let root = ws.root.clone();
        let limit = max_bytes.unwrap_or(self.config.max_read_bytes).clamp(1, self.config.max_read_limit);
        blocking(move || ops::read(&root, &rel, limit)).await
    }

    /// `files.write`.
    #[allow(clippy::too_many_arguments)]
    pub async fn write(
        &self,
        ws: &Workspace,
        path: &str,
        content: &str,
        encoding: Option<TextEncoding>,
        expected_hash: Option<String>,
        create: bool,
        actor: ChangeActor,
    ) -> FilesResult<Mutation<WriteFileResult>> {
        let rel = RelPath::parse(path)?;
        let bytes = text::encode(content, encoding.unwrap_or_default())?;
        if bytes.len() as u64 > self.config.max_write_bytes {
            return Err(FilesError::TooLarge {
                path: rel.as_string(),
                size: bytes.len() as u64,
                limit: self.config.max_write_bytes,
            });
        }
        let _guard = self.mutation_lock.lock().await;
        self.record(ws, &rel);
        let root = ws.root.clone();
        let out = blocking(move || ops::write(&root, &rel, &bytes, expected_hash.as_deref(), create)).await?;
        let path = out.rel.as_string();
        self.record(ws, &out.rel);
        let kind = if out.created { FileChangeKind::Created } else { FileChangeKind::Modified };
        tracing::debug!(project_id = %ws.project_id, %path, ?actor, "file written");
        Ok(Mutation {
            changes: vec![actor.change(path.clone(), None, kind)],
            result: WriteFileResult { path, hash: out.hash, size: out.size, created: out.created },
        })
    }

    /// `files.create`.
    pub async fn create_file(
        &self,
        ws: &Workspace,
        path: &str,
        content: Option<&str>,
        actor: ChangeActor,
    ) -> FilesResult<Mutation<FileEntry>> {
        let rel = RelPath::parse(path)?;
        let bytes = content.unwrap_or_default().as_bytes().to_vec();
        if bytes.len() as u64 > self.config.max_write_bytes {
            return Err(FilesError::TooLarge {
                path: rel.as_string(),
                size: bytes.len() as u64,
                limit: self.config.max_write_bytes,
            });
        }
        let _guard = self.mutation_lock.lock().await;
        self.record(ws, &rel);
        let root = ws.root.clone();
        let entry = blocking(move || ops::create_file(&root, &rel, &bytes)).await?;
        Ok(Mutation { changes: vec![actor.change(entry.path.clone(), None, FileChangeKind::Created)], result: entry })
    }

    /// `files.mkdir`.
    pub async fn mkdir(&self, ws: &Workspace, path: &str, actor: ChangeActor) -> FilesResult<Mutation<FileEntry>> {
        let rel = RelPath::parse(path)?;
        let _guard = self.mutation_lock.lock().await;
        self.record(ws, &rel);
        let root = ws.root.clone();
        let entry = blocking(move || ops::mkdir(&root, &rel)).await?;
        Ok(Mutation { changes: vec![actor.change(entry.path.clone(), None, FileChangeKind::Created)], result: entry })
    }

    /// `files.rename`.
    pub async fn rename(
        &self,
        ws: &Workspace,
        from: &str,
        to: &str,
        overwrite: bool,
        actor: ChangeActor,
    ) -> FilesResult<Mutation<RenameFileResult>> {
        let from = RelPath::parse(from)?;
        let to = RelPath::parse(to)?;
        let _guard = self.mutation_lock.lock().await;
        self.record(ws, &from);
        self.record(ws, &to);
        let root = ws.root.clone();
        let (src, entry) = blocking(move || ops::rename(&root, &from, &to, overwrite)).await?;
        self.record(ws, &src);
        let from = src.as_string();
        Ok(Mutation {
            changes: vec![actor.change(entry.path.clone(), Some(from.clone()), FileChangeKind::Renamed)],
            result: RenameFileResult { from, entry },
        })
    }

    /// `files.delete`: copies every file into the trash, then deletes.
    pub async fn delete(
        &self,
        ws: &Workspace,
        path: &str,
        recursive: bool,
        actor: ChangeActor,
    ) -> FilesResult<Mutation<DeleteFileResult>> {
        let rel = RelPath::parse(path)?;
        let _guard = self.mutation_lock.lock().await;
        let root = ws.root.clone();
        let max = self.config.max_trash_bytes;
        let plan = blocking(move || ops::plan_delete(&root, &rel, recursive, max)).await?;
        self.record(ws, &plan.rel);
        let (wire_actor, agent_id) = actor.wire();
        let root_str = ws.root.path().to_string_lossy().into_owned();
        let mut trashed = Vec::with_capacity(plan.files.len());
        for (file_rel, abs, _) in &plan.files {
            let bytes = tokio::fs::read(abs).await.map_err(|e| FilesError::io(file_rel.as_string(), e))?;
            let entry = NewTrashEntry {
                project_id: ws.project_id,
                root: root_str.clone(),
                path: file_rel.as_string(),
                actor: wire_actor,
                agent_id,
            };
            trashed.push(self.trash.stash(entry, bytes).await?);
        }
        let path = plan.rel.as_string();
        blocking(move || ops::execute_delete(&plan)).await?;
        tracing::info!(project_id = %ws.project_id, %path, files = trashed.len(), ?actor, "deleted (copies kept in trash)");
        Ok(Mutation {
            changes: vec![actor.change(path.clone(), None, FileChangeKind::Deleted)],
            result: DeleteFileResult { path, trashed },
        })
    }

    /// `files.trash`.
    pub async fn trash_list(&self, project_id: ProjectId, limit: Option<u32>) -> FilesResult<Vec<TrashItem>> {
        self.trash.list(project_id, limit.unwrap_or(200).clamp(1, 1000)).await
    }

    /// `files.restore`.
    pub async fn restore(
        &self,
        ws: &Workspace,
        trash_id: &str,
        overwrite: bool,
        actor: ChangeActor,
    ) -> FilesResult<Mutation<RestoreFileResult>> {
        let (record, bytes) =
            self.trash.fetch(trash_id).await?.ok_or_else(|| FilesError::NotFound(format!("trash entry {trash_id}")))?;
        if record.project_id != ws.project_id || Path::new(&record.root) != ws.root.path() {
            return Err(FilesError::Unsupported(
                "the trash entry belongs to another workspace (pass its worktreePath)".into(),
            ));
        }
        let rel = RelPath::parse(&record.item.path)?;
        let _guard = self.mutation_lock.lock().await;
        self.record(ws, &rel);
        let root = ws.root.clone();
        let out = blocking(move || {
            let res = root.resolve(&rel, true)?;
            if res.exists && !overwrite {
                return Err(FilesError::AlreadyExists(res.rel.as_string()));
            }
            ops::write(&root, &rel, &bytes, None, true)
        })
        .await?;
        self.trash.mark_restored(trash_id).await?;
        let path = out.rel.as_string();
        let kind = if out.created { FileChangeKind::Created } else { FileChangeKind::Modified };
        Ok(Mutation {
            changes: vec![actor.change(path.clone(), None, kind)],
            result: RestoreFileResult { path, hash: out.hash },
        })
    }

    /// `files.search`.
    pub async fn search(&self, ws: &Workspace, mut opts: SearchOptions) -> FilesResult<SearchFilesResult> {
        if opts.max_results == 0 {
            opts.max_results = self.config.search_default_results;
        }
        opts.max_results = opts.max_results.min(self.config.search_max_results);
        opts.max_file_bytes = self.config.search_max_file_bytes;
        let root = ws.root.clone();
        blocking(move || search::search(&root, &opts)).await
    }

    /// `files.find`.
    pub async fn find(
        &self,
        ws: &Workspace,
        query: &str,
        max_results: Option<usize>,
        show_ignored: bool,
    ) -> FilesResult<FindFilesResult> {
        let max = max_results.unwrap_or(self.config.find_default_results).clamp(1, self.config.find_max_results);
        let root = ws.root.clone();
        let query = query.to_owned();
        let scan = self.config.find_max_scan;
        blocking(move || search::find(&root, &query, max, show_ignored, scan)).await
    }

    /// `files.watch` — returns the subscriber count.
    pub fn watch(&self, ws: &Workspace, subscriber: &str) -> FilesResult<u32> {
        self.watchers.watch(ws.project_id, &ws.root, subscriber)
    }

    /// `files.unwatch` — returns the remaining subscriber count.
    pub fn unwatch(&self, ws: &Workspace, subscriber: &str) -> u32 {
        self.watchers.unwatch(&ws.root, subscriber)
    }

    /// Drops every watch of a subscriber (session closed).
    pub fn unwatch_all(&self, subscriber: &str) {
        self.watchers.unwatch_all(subscriber);
    }

    /// Whether the workspace is watched.
    #[must_use]
    pub fn is_watching(&self, ws: &Workspace) -> bool {
        self.watchers.is_watching(&ws.root)
    }

    /// `files.openExternal`.
    pub async fn open_external(
        &self,
        ws: &Workspace,
        path: &str,
        mode: OpenTarget,
        ide: Option<String>,
    ) -> FilesResult<String> {
        let rel = RelPath::parse(path)?;
        let root = ws.root.clone();
        let opener = Arc::clone(&self.opener);
        blocking(move || {
            let res = root.resolve(&rel, true)?;
            if !res.exists {
                return Err(FilesError::NotFound(rel.as_string()));
            }
            let req =
                OpenRequest { root: root.path().to_path_buf(), is_dir: res.abs.is_dir(), target: res.abs, mode, ide };
            opener.open(&req)
        })
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::open::RecordingOpener;
    use crate::trash::MemoryTrash;

    struct NoSink;
    impl ChangeSink for NoSink {
        fn publish(&self, _: ProjectId, _: Vec<FileChangedPayload>) {}
    }

    fn manager() -> (FilesManager, Arc<RecordingOpener>) {
        let opener = Arc::new(RecordingOpener::default());
        let m =
            FilesManager::new(FilesConfig::default(), Arc::new(MemoryTrash::new()), opener.clone(), Arc::new(NoSink));
        (m, opener)
    }

    #[tokio::test]
    async fn write_read_conflict_and_actor_attribution() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager();
        let ws = m.workspace(ProjectId::new(), dir.path()).unwrap();
        let agent = AgentId::new();
        let w = m.write(&ws, "src/a.ts", "let a = 1;\n", None, None, true, ChangeActor::Agent(agent)).await.unwrap();
        assert!(w.result.created);
        assert_eq!(w.changes[0].actor, FileChangeActor::Agent);
        assert_eq!(w.changes[0].agent_id, Some(agent));
        assert_eq!(w.changes[0].change, FileChangeKind::Created);
        let r = m.read(&ws, "src/a.ts", None).await.unwrap();
        assert_eq!(r.hash, w.result.hash);
        let w2 = m
            .write(&ws, "src/a.ts", "let a = 2;\n", None, Some(r.hash.clone()), false, ChangeActor::User)
            .await
            .unwrap();
        assert_eq!(w2.changes[0].actor, FileChangeActor::User);
        assert_eq!(w2.changes[0].change, FileChangeKind::Modified);
        let err = m.write(&ws, "src/a.ts", "stale", None, Some(r.hash), false, ChangeActor::User).await.unwrap_err();
        assert!(matches!(err, FilesError::Conflict { .. }));
        // UTF-16 round trip keeps the encoding.
        m.write(&ws, "u16.txt", "héllo", Some(TextEncoding::Utf16Le), None, true, ChangeActor::User).await.unwrap();
        let r = m.read(&ws, "u16.txt", None).await.unwrap();
        assert_eq!(r.encoding, TextEncoding::Utf16Le);
        assert_eq!(r.content.as_deref(), Some("héllo"));
        // Traversal is rejected before any I/O.
        assert!(matches!(m.read(&ws, "../etc/passwd", None).await, Err(FilesError::InvalidPath { .. })));
    }

    #[tokio::test]
    async fn delete_trashes_and_restore_recovers() {
        let dir = tempfile::tempdir().unwrap();
        let (m, _) = manager();
        let ws = m.workspace(ProjectId::new(), dir.path()).unwrap();
        m.create_file(&ws, "d/one.txt", Some("1"), ChangeActor::User).await.unwrap();
        m.create_file(&ws, "d/sub/two.txt", Some("22"), ChangeActor::User).await.unwrap();
        assert!(matches!(m.delete(&ws, "d", false, ChangeActor::User).await, Err(FilesError::RecursiveRequired(_))));
        let del = m.delete(&ws, "d", true, ChangeActor::User).await.unwrap();
        assert_eq!(del.result.trashed.len(), 2);
        assert_eq!(del.changes[0].change, FileChangeKind::Deleted);
        assert!(!dir.path().join("d").exists());
        let items = m.trash_list(ws.project_id, None).await.unwrap();
        assert_eq!(items.len(), 2);
        let two = items.iter().find(|i| i.path == "d/sub/two.txt").unwrap();
        let restored = m.restore(&ws, &two.id, false, ChangeActor::User).await.unwrap();
        assert_eq!(restored.result.path, "d/sub/two.txt");
        assert_eq!(std::fs::read_to_string(dir.path().join("d/sub/two.txt")).unwrap(), "22");
        assert!(matches!(m.restore(&ws, &two.id, false, ChangeActor::User).await, Err(FilesError::AlreadyExists(_))));
        m.restore(&ws, &two.id, true, ChangeActor::User).await.unwrap();
        // Another workspace cannot restore it.
        let other_dir = tempfile::tempdir().unwrap();
        let other = m.workspace(ws.project_id, other_dir.path()).unwrap();
        assert!(matches!(m.restore(&other, &two.id, true, ChangeActor::User).await, Err(FilesError::Unsupported(_))));
    }

    #[tokio::test]
    async fn rename_mkdir_search_find_open() {
        let dir = tempfile::tempdir().unwrap();
        let (m, opener) = manager();
        let ws = m.workspace(ProjectId::new(), dir.path()).unwrap();
        m.mkdir(&ws, "lib", ChangeActor::User).await.unwrap();
        m.create_file(&ws, "lib/util.rs", Some("pub fn helper() {}\n"), ChangeActor::User).await.unwrap();
        let r = m.rename(&ws, "lib/util.rs", "src/util.rs", false, ChangeActor::User).await.unwrap();
        assert_eq!(r.result.from, "lib/util.rs");
        assert_eq!(r.changes[0].old_path.as_deref(), Some("lib/util.rs"));
        let s = m.search(&ws, SearchOptions { query: "helper".into(), ..Default::default() }).await.unwrap();
        assert_eq!(s.matches.len(), 1);
        assert_eq!(s.matches[0].path, "src/util.rs");
        let f = m.find(&ws, "util", None, false).await.unwrap();
        assert_eq!(f.matches[0].path, "src/util.rs");
        let label = m.open_external(&ws, "src/util.rs", OpenTarget::Reveal, None).await.unwrap();
        assert_eq!(label, "recorded");
        {
            let reqs = opener.requests.lock();
            assert_eq!(reqs.len(), 1);
            assert!(!reqs[0].is_dir);
            assert!(reqs[0].target.ends_with("util.rs"));
        }
        assert!(matches!(m.open_external(&ws, "nope", OpenTarget::Default, None).await, Err(FilesError::NotFound(_))));
        let listing = m.list(&ws, "", false).await.unwrap();
        assert_eq!(listing.entries.iter().map(|e| e.name.as_str()).collect::<Vec<_>>(), vec!["lib", "src"]);
    }
}
