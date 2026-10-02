//! Debounced workspace watcher (`notify`) emitting `FileChanged` payloads.
//!
//! * One OS watcher per workspace root, reference-counted by subscriber
//!   (RPC session) ids.
//! * Raw events are coalesced per path over a quiet period (`debounce`) and
//!   flushed at the latest after `max_wait`.
//! * `.git`, `target`, `node_modules`, atomic-write temp files and paths
//!   matched by the root `.gitignore` / `.git/info/exclude` are dropped.
//! * Changes Orkestr itself just made (recorded in [`RecentWrites`]) are
//!   suppressed: those were already published with their real actor
//!   (user/agent), so the watcher only reports `external` changes.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

use ignore::gitignore::{Gitignore, GitignoreBuilder};
use notify::event::{EventKind as NotifyKind, ModifyKind, RenameMode};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};
use orkestr_domain::ProjectId;
use orkestr_protocol::event::{FileChangeActor, FileChangeKind, FileChangedPayload};
use parking_lot::Mutex;

use crate::error::{FilesError, FilesResult};
use crate::ops::TEMP_MARKER;
use crate::path::WorkspaceRoot;

/// Directory names whose contents never produce watcher events.
pub const NOISY_DIRS: &[&str] = &[".git", "target", "node_modules"];

/// Above this many coalesced changes a batch collapses into one root-level
/// `Modified` event (clients rescan).
pub const MAX_BATCH: usize = 500;

/// Receives coalesced change batches.
pub trait ChangeSink: Send + Sync {
    /// Publishes changes of a project workspace.
    fn publish(&self, project_id: ProjectId, changes: Vec<FileChangedPayload>);
}

/// Paths recently changed through the file service, to suppress duplicate
/// watcher events.
#[derive(Debug)]
pub struct RecentWrites {
    window: Duration,
    inner: Mutex<HashMap<(PathBuf, String), Instant>>,
}

impl RecentWrites {
    /// New registry with a suppression window.
    #[must_use]
    pub fn new(window: Duration) -> Self {
        Self { window, inner: Mutex::new(HashMap::new()) }
    }

    /// Records a path (call **before** touching the disk).
    pub fn record(&self, root: &Path, rel: &str) {
        let now = Instant::now();
        let mut map = self.inner.lock();
        map.retain(|_, t| now.duration_since(*t) < self.window);
        map.insert((root.to_path_buf(), rel.to_owned()), now);
    }

    /// Whether the path or one of its ancestors was recorded within the window.
    #[must_use]
    pub fn is_recent(&self, root: &Path, rel: &str) -> bool {
        let now = Instant::now();
        let map = self.inner.lock();
        let hit = |p: &str| {
            map.get(&(root.to_path_buf(), p.to_owned())).is_some_and(|t| now.duration_since(*t) < self.window)
        };
        if hit(rel) {
            return true;
        }
        let mut cur = rel;
        while let Some((parent, _)) = cur.rsplit_once('/') {
            if hit(parent) {
                return true;
            }
            cur = parent;
        }
        false
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Pending {
    kind: FileChangeKind,
    old_path: Option<String>,
}

/// Coalesces raw changes per path.
#[derive(Debug, Default)]
pub struct Coalescer {
    pending: BTreeMap<String, Pending>,
}

impl Coalescer {
    /// Adds one change.
    pub fn add(&mut self, path: &str, kind: FileChangeKind) {
        use FileChangeKind::{Created, Deleted, Modified, Renamed};
        let prev = self.pending.get(path).map(|p| p.kind);
        let next = match (prev, kind) {
            (None, k) => Some(k),
            (Some(Created), Modified) => Some(Created),
            (Some(Created), Deleted) => None,
            (Some(Deleted), Created) => Some(Modified),
            (Some(Renamed), Modified) => Some(Renamed),
            (Some(Renamed), Deleted) => {
                if let Some(old) = self.pending.get(path).and_then(|p| p.old_path.clone()) {
                    self.pending.insert(old, Pending { kind: Deleted, old_path: None });
                }
                Some(Deleted)
            }
            (_, k) => Some(k),
        };
        match next {
            Some(kind) => {
                let old_path =
                    if kind == Renamed { self.pending.get(path).and_then(|p| p.old_path.clone()) } else { None };
                self.pending.insert(path.to_owned(), Pending { kind, old_path });
            }
            None => {
                self.pending.remove(path);
            }
        }
    }

    /// Adds a rename `from → to`.
    pub fn rename(&mut self, from: &str, to: &str) {
        match self.pending.remove(from).map(|p| p.kind) {
            // Created and renamed in the same batch: it is simply new at `to`.
            Some(FileChangeKind::Created) => self.add(to, FileChangeKind::Created),
            _ => {
                self.pending
                    .insert(to.to_owned(), Pending { kind: FileChangeKind::Renamed, old_path: Some(from.to_owned()) });
            }
        }
    }

    /// Whether nothing is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    /// Drains pending changes as `external` payloads (path order).
    pub fn drain(&mut self) -> Vec<FileChangedPayload> {
        std::mem::take(&mut self.pending)
            .into_iter()
            .map(|(path, p)| FileChangedPayload {
                path,
                old_path: p.old_path,
                change: p.kind,
                actor: FileChangeActor::External,
                agent_id: None,
            })
            .collect()
    }
}

/// Filters out noisy and ignored paths.
pub struct PathFilter {
    gitignore: Gitignore,
}

impl PathFilter {
    /// Builds from the root `.gitignore` and `.git/info/exclude`.
    #[must_use]
    pub fn new(root: &Path) -> Self {
        let mut b = GitignoreBuilder::new(root);
        for f in [root.join(".gitignore"), root.join(".git").join("info").join("exclude")] {
            if f.is_file() {
                if let Some(e) = b.add(&f) {
                    tracing::debug!(error = %e, file = %f.display(), "ignoring unreadable ignore file");
                }
            }
        }
        let gitignore = b.build().unwrap_or_else(|e| {
            tracing::debug!(error = %e, "invalid ignore rules; watching everything");
            Gitignore::empty()
        });
        Self { gitignore }
    }

    /// Whether a relative path should be reported.
    #[must_use]
    pub fn accepts(&self, rel: &str, is_dir: bool) -> bool {
        if rel.is_empty() {
            return false;
        }
        for c in rel.split('/') {
            if NOISY_DIRS.iter().any(|n| c.eq_ignore_ascii_case(n)) || c.contains(TEMP_MARKER) {
                return false;
            }
        }
        !self.gitignore.matched_path_or_any_parents(Path::new(rel), is_dir).is_ignore()
    }
}

struct DebounceCtx {
    project_id: ProjectId,
    root: WorkspaceRoot,
    sink: Arc<dyn ChangeSink>,
    recent: Arc<RecentWrites>,
    debounce: Duration,
    max_wait: Duration,
}

impl DebounceCtx {
    fn rel(&self, p: &Path) -> Option<String> {
        self.root.relative_of(p).map(|r| r.as_string())
    }

    fn absorb(&self, ev: notify::Event, filter: &mut PathFilter, co: &mut Coalescer) {
        let kinds: Vec<(String, FileChangeKind)> = match ev.kind {
            NotifyKind::Modify(ModifyKind::Name(RenameMode::Both)) if ev.paths.len() == 2 => {
                let (from, to) = (self.rel(&ev.paths[0]), self.rel(&ev.paths[1]));
                let to_dir = ev.paths[1].is_dir();
                match (from, to) {
                    (Some(f), Some(t)) => {
                        let fa = filter.accepts(&f, to_dir);
                        let ta = filter.accepts(&t, to_dir);
                        match (fa, ta) {
                            (true, true) => co.rename(&f, &t),
                            (true, false) => co.add(&f, FileChangeKind::Deleted),
                            (false, true) => co.add(&t, FileChangeKind::Created),
                            (false, false) => {}
                        }
                    }
                    (Some(f), None) if filter.accepts(&f, to_dir) => co.add(&f, FileChangeKind::Deleted),
                    (None, Some(t)) if filter.accepts(&t, to_dir) => co.add(&t, FileChangeKind::Created),
                    _ => {}
                }
                return;
            }
            NotifyKind::Create(_) | NotifyKind::Modify(ModifyKind::Name(RenameMode::To)) => {
                ev.paths.iter().filter_map(|p| self.rel(p)).map(|r| (r, FileChangeKind::Created)).collect()
            }
            NotifyKind::Remove(_) | NotifyKind::Modify(ModifyKind::Name(RenameMode::From)) => {
                ev.paths.iter().filter_map(|p| self.rel(p)).map(|r| (r, FileChangeKind::Deleted)).collect()
            }
            NotifyKind::Modify(ModifyKind::Name(_)) => ev
                .paths
                .iter()
                .filter_map(|p| {
                    let kind = if p.exists() { FileChangeKind::Created } else { FileChangeKind::Deleted };
                    self.rel(p).map(|r| (r, kind))
                })
                .collect(),
            NotifyKind::Modify(ModifyKind::Metadata(_)) | NotifyKind::Access(_) | NotifyKind::Other => Vec::new(),
            NotifyKind::Modify(_) | NotifyKind::Any => {
                ev.paths.iter().filter_map(|p| self.rel(p)).map(|r| (r, FileChangeKind::Modified)).collect()
            }
        };
        for (rel, kind) in kinds {
            let abs = self.root.path().join(&rel);
            if rel == ".gitignore" {
                *filter = PathFilter::new(self.root.path());
            }
            if filter.accepts(&rel, abs.is_dir()) {
                co.add(&rel, kind);
            }
        }
    }

    fn flush(&self, co: &mut Coalescer) {
        if co.is_empty() {
            return;
        }
        let root = self.root.path();
        let mut changes: Vec<FileChangedPayload> = co
            .drain()
            .into_iter()
            .filter(|c| {
                !self.recent.is_recent(root, &c.path)
                    && !c.old_path.as_deref().is_some_and(|o| self.recent.is_recent(root, o))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        if changes.len() > MAX_BATCH {
            changes = vec![FileChangedPayload {
                path: String::new(),
                old_path: None,
                change: FileChangeKind::Modified,
                actor: FileChangeActor::External,
                agent_id: None,
            }];
        }
        tracing::debug!(project_id = %self.project_id, count = changes.len(), "external file changes");
        self.sink.publish(self.project_id, changes);
    }

    fn run(self, rx: Receiver<notify::Result<notify::Event>>) {
        let mut filter = PathFilter::new(self.root.path());
        let mut co = Coalescer::default();
        let mut first: Option<Instant> = None;
        loop {
            let msg = if co.is_empty() {
                rx.recv().map_err(|_| RecvTimeoutError::Disconnected)
            } else {
                rx.recv_timeout(self.debounce)
            };
            match msg {
                Ok(Ok(ev)) => {
                    self.absorb(ev, &mut filter, &mut co);
                    let started = *first.get_or_insert_with(Instant::now);
                    if started.elapsed() >= self.max_wait {
                        self.flush(&mut co);
                        first = None;
                    }
                }
                Ok(Err(e)) => tracing::warn!(error = %e, "file watcher error"),
                Err(RecvTimeoutError::Timeout) => {
                    self.flush(&mut co);
                    first = None;
                }
                Err(RecvTimeoutError::Disconnected) => {
                    self.flush(&mut co);
                    break;
                }
            }
        }
        tracing::debug!(project_id = %self.project_id, "file watcher stopped");
    }
}

struct ActiveWatch {
    project_id: ProjectId,
    subscribers: HashSet<String>,
    _watcher: RecommendedWatcher,
}

/// Reference-counted watchers keyed by workspace root.
pub struct WatcherHub {
    sink: Arc<dyn ChangeSink>,
    recent: Arc<RecentWrites>,
    debounce: Duration,
    max_wait: Duration,
    active: Mutex<HashMap<PathBuf, ActiveWatch>>,
}

impl std::fmt::Debug for WatcherHub {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WatcherHub").field("watching", &self.active.lock().len()).finish_non_exhaustive()
    }
}

impl WatcherHub {
    /// New hub.
    #[must_use]
    pub fn new(sink: Arc<dyn ChangeSink>, recent: Arc<RecentWrites>, debounce: Duration) -> Self {
        Self { sink, recent, debounce, max_wait: debounce * 8, active: Mutex::new(HashMap::new()) }
    }

    /// Adds `subscriber` to the watcher of `root` (starting it if needed).
    /// Returns the number of subscribers.
    pub fn watch(&self, project_id: ProjectId, root: &WorkspaceRoot, subscriber: &str) -> FilesResult<u32> {
        let mut active = self.active.lock();
        if let Some(w) = active.get_mut(root.path()) {
            w.subscribers.insert(subscriber.to_owned());
            return Ok(u32::try_from(w.subscribers.len()).unwrap_or(u32::MAX));
        }
        let (tx, rx) = mpsc::channel();
        let mut watcher = notify::recommended_watcher(move |res| {
            let _ = tx.send(res);
        })
        .map_err(|e| FilesError::Watch(e.to_string()))?;
        watcher.watch(root.path(), RecursiveMode::Recursive).map_err(|e| FilesError::Watch(e.to_string()))?;
        let ctx = DebounceCtx {
            project_id,
            root: root.clone(),
            sink: Arc::clone(&self.sink),
            recent: Arc::clone(&self.recent),
            debounce: self.debounce,
            max_wait: self.max_wait,
        };
        std::thread::Builder::new()
            .name("orkestr-files-watch".into())
            .spawn(move || ctx.run(rx))
            .map_err(|e| FilesError::Watch(e.to_string()))?;
        tracing::info!(%project_id, "file watcher started");
        active.insert(
            root.path().to_path_buf(),
            ActiveWatch { project_id, subscribers: HashSet::from([subscriber.to_owned()]), _watcher: watcher },
        );
        Ok(1)
    }

    /// Removes `subscriber`; stops the watcher when none remain. Returns remaining subscribers.
    pub fn unwatch(&self, root: &WorkspaceRoot, subscriber: &str) -> u32 {
        let mut active = self.active.lock();
        let Some(w) = active.get_mut(root.path()) else { return 0 };
        w.subscribers.remove(subscriber);
        let n = w.subscribers.len();
        if n == 0 {
            if let Some(w) = active.remove(root.path()) {
                tracing::info!(project_id = %w.project_id, "file watcher stopping");
            }
        }
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    /// Removes `subscriber` from every watcher (e.g. when a session disconnects).
    pub fn unwatch_all(&self, subscriber: &str) {
        let mut active = self.active.lock();
        active.retain(|_, w| {
            w.subscribers.remove(subscriber);
            !w.subscribers.is_empty()
        });
    }

    /// Whether `root` is watched.
    #[must_use]
    pub fn is_watching(&self, root: &WorkspaceRoot) -> bool {
        self.active.lock().contains_key(root.path())
    }

    /// Number of subscribers of `root`.
    #[must_use]
    pub fn subscribers(&self, root: &WorkspaceRoot) -> u32 {
        self.active.lock().get(root.path()).map_or(0, |w| u32::try_from(w.subscribers.len()).unwrap_or(u32::MAX))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use FileChangeKind::{Created, Deleted, Modified, Renamed};

    fn kinds(co: &mut Coalescer) -> Vec<(String, FileChangeKind, Option<String>)> {
        co.drain().into_iter().map(|c| (c.path, c.change, c.old_path)).collect()
    }

    #[test]
    fn coalescing_rules() {
        let mut co = Coalescer::default();
        co.add("a", Created);
        co.add("a", Modified);
        co.add("b", Created);
        co.add("b", Deleted);
        co.add("c", Deleted);
        co.add("c", Created);
        co.add("d", Modified);
        co.add("d", Modified);
        co.add("e", Modified);
        co.add("e", Deleted);
        assert_eq!(
            kinds(&mut co),
            vec![
                ("a".into(), Created, None),
                ("c".into(), Modified, None),
                ("d".into(), Modified, None),
                ("e".into(), Deleted, None),
            ]
        );
        assert!(co.is_empty());
    }

    #[test]
    fn rename_coalescing() {
        let mut co = Coalescer::default();
        co.rename("old.txt", "new.txt");
        co.add("new.txt", Modified);
        assert_eq!(kinds(&mut co), vec![("new.txt".into(), Renamed, Some("old.txt".into()))]);
        co.add("tmp", Created);
        co.rename("tmp", "final");
        assert_eq!(kinds(&mut co), vec![("final".into(), Created, None)]);
        co.rename("x", "y");
        co.add("y", Deleted);
        assert_eq!(kinds(&mut co), vec![("x".into(), Deleted, None), ("y".into(), Deleted, None)]);
    }

    #[test]
    fn filter_drops_noise_and_gitignored() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(".gitignore"), "dist/\n*.log\n").unwrap();
        let f = PathFilter::new(dir.path());
        assert!(f.accepts("src/main.rs", false));
        assert!(!f.accepts(".git/index", false));
        assert!(!f.accepts("target/debug/x", false));
        assert!(!f.accepts("web/node_modules/react/index.js", false));
        assert!(!f.accepts("dist/app.js", false));
        assert!(!f.accepts("dist", true));
        assert!(!f.accepts("server.log", false));
        assert!(!f.accepts(".a.txt.1234abcd.orkestr-tmp", false));
        assert!(!f.accepts("", true));
    }

    #[test]
    fn recent_writes_cover_descendants_and_expire() {
        let r = RecentWrites::new(Duration::from_millis(80));
        let root = Path::new("/w");
        r.record(root, "dir");
        assert!(r.is_recent(root, "dir"));
        assert!(r.is_recent(root, "dir/sub/file.txt"));
        assert!(!r.is_recent(root, "other"));
        assert!(!r.is_recent(Path::new("/x"), "dir"));
        std::thread::sleep(Duration::from_millis(120));
        assert!(!r.is_recent(root, "dir"));
    }

    struct Collect(Mutex<Vec<(ProjectId, Vec<FileChangedPayload>)>>);
    impl ChangeSink for Collect {
        fn publish(&self, project_id: ProjectId, changes: Vec<FileChangedPayload>) {
            self.0.lock().push((project_id, changes));
        }
    }

    fn wait_for(sink: &Collect, pred: impl Fn(&FileChangedPayload) -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if sink.0.lock().iter().flat_map(|(_, c)| c).any(&pred) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(25));
        }
        false
    }

    #[test]
    fn hub_reports_external_changes_and_refcounts() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("node_modules")).unwrap();
        let root = WorkspaceRoot::open(dir.path()).unwrap();
        let sink = Arc::new(Collect(Mutex::new(Vec::new())));
        let recent = Arc::new(RecentWrites::new(Duration::from_secs(2)));
        let hub = WatcherHub::new(sink.clone(), Arc::clone(&recent), Duration::from_millis(60));
        let pid = ProjectId::new();
        assert_eq!(hub.watch(pid, &root, "s1").unwrap(), 1);
        assert_eq!(hub.watch(pid, &root, "s2").unwrap(), 2);
        assert_eq!(hub.watch(pid, &root, "s2").unwrap(), 2, "idempotent per subscriber");
        // Give the OS watcher a moment to arm.
        std::thread::sleep(Duration::from_millis(150));

        recent.record(root.path(), "mine.txt");
        std::fs::write(root.path().join("mine.txt"), "x").unwrap();
        std::fs::write(root.path().join("node_modules/pkg.js"), "x").unwrap();
        std::fs::write(root.path().join("external.txt"), "hello").unwrap();
        assert!(
            wait_for(&sink, |c| c.path == "external.txt" && c.actor == FileChangeActor::External),
            "external change must be reported: {:?}",
            sink.0.lock()
        );
        std::thread::sleep(Duration::from_millis(200));
        let all: Vec<_> = sink.0.lock().iter().flat_map(|(p, c)| c.iter().map(move |c| (*p, c.path.clone()))).collect();
        assert!(all.iter().all(|(p, _)| *p == pid));
        assert!(!all.iter().any(|(_, path)| path == "mine.txt"), "self-made changes are suppressed: {all:?}");
        assert!(!all.iter().any(|(_, path)| path.starts_with("node_modules")), "{all:?}");

        assert_eq!(hub.unwatch(&root, "s1"), 1);
        assert!(hub.is_watching(&root));
        hub.unwatch_all("s2");
        assert!(!hub.is_watching(&root));
        assert_eq!(hub.subscribers(&root), 0);
    }
}
