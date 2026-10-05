//! Cache of project file indexes + import graphs (§20).
//!
//! Building an index walks and reads the whole project, so it is cached per
//! root. Entries are invalidated precisely by `FileChanged` events
//! ([`IndexCache::invalidate_path`] marks the path dirty; the next
//! [`IndexCache::get`] re-indexes only dirty files and rebuilds the graph) and
//! coarsely by age (roots nobody watches — e.g. agent worktrees — are rebuilt
//! after `max_age`). The cache is bounded (least recently used root evicted).
//! All methods are blocking (call from `spawn_blocking`).

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

use once_cell::sync::Lazy;
use parking_lot::Mutex;

use crate::imports::ImportGraph;
use crate::index::{FileIndex, IndexOptions};
use crate::ContextError;

/// An indexed project.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IndexedProject {
    /// File index.
    pub index: FileIndex,
    /// Import graph.
    pub graph: ImportGraph,
}

#[derive(Debug)]
struct Entry {
    project: Arc<IndexedProject>,
    built_at: Instant,
    used_at: Instant,
    dirty: BTreeSet<String>,
    full_rebuild: bool,
}

/// Cache counters (tests / diagnostics).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CacheStats {
    /// Served without any work.
    pub hits: u64,
    /// Full builds.
    pub builds: u64,
    /// Incremental refreshes.
    pub refreshes: u64,
}

/// The cache.
#[derive(Debug)]
pub struct IndexCache {
    entries: Mutex<HashMap<PathBuf, Entry>>,
    stats: Mutex<CacheStats>,
    opts: IndexOptions,
    max_age: Duration,
    capacity: usize,
}

impl Default for IndexCache {
    fn default() -> Self {
        Self::new(IndexOptions::default(), Duration::from_secs(120), 16)
    }
}

static GLOBAL: Lazy<IndexCache> = Lazy::new(IndexCache::default);

/// The process-wide cache (core invalidates it from `FileChanged` events).
#[must_use]
pub fn global_cache() -> &'static IndexCache {
    &GLOBAL
}

fn key(root: &Path) -> PathBuf {
    std::fs::canonicalize(root).unwrap_or_else(|_| root.to_path_buf())
}

impl IndexCache {
    /// Cache with explicit limits.
    #[must_use]
    pub fn new(opts: IndexOptions, max_age: Duration, capacity: usize) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            stats: Mutex::new(CacheStats::default()),
            opts,
            max_age,
            capacity: capacity.max(1),
        }
    }

    /// The index of `root`, built or refreshed as needed.
    pub fn get(&self, root: &Path) -> Result<Arc<IndexedProject>, ContextError> {
        let k = key(root);
        let now = Instant::now();
        // Take what needs doing under the lock, do the I/O outside it.
        let job = {
            let mut entries = self.entries.lock();
            match entries.get_mut(&k) {
                Some(e) if !e.full_rebuild && now.duration_since(e.built_at) < self.max_age => {
                    e.used_at = now;
                    if e.dirty.is_empty() {
                        self.stats.lock().hits += 1;
                        return Ok(Arc::clone(&e.project));
                    }
                    Some((Arc::clone(&e.project), std::mem::take(&mut e.dirty), e.built_at))
                }
                _ => None,
            }
        };
        let (project, built_at) = match job {
            Some((current, dirty, built_at)) => {
                let mut index = current.index.clone();
                let mut changed = false;
                for rel in &dirty {
                    changed |= index.refresh_path(&k, rel, self.opts);
                }
                self.stats.lock().refreshes += 1;
                let project = if changed {
                    let graph = ImportGraph::build(&index);
                    Arc::new(IndexedProject { index, graph })
                } else {
                    current
                };
                (project, built_at)
            }
            None => {
                let index = FileIndex::build(&k, self.opts)?;
                let graph = ImportGraph::build(&index);
                self.stats.lock().builds += 1;
                (Arc::new(IndexedProject { index, graph }), now)
            }
        };
        let mut entries = self.entries.lock();
        // Paths invalidated while we were building stay dirty.
        let pending = entries.get_mut(&k).map(|e| std::mem::take(&mut e.dirty)).unwrap_or_default();
        entries.insert(
            k,
            Entry { project: Arc::clone(&project), built_at, used_at: now, dirty: pending, full_rebuild: false },
        );
        if entries.len() > self.capacity {
            if let Some(oldest) = entries.iter().min_by_key(|(_, e)| e.used_at).map(|(p, _)| p.clone()) {
                entries.remove(&oldest);
            }
        }
        Ok(project)
    }

    /// Marks `rel` (relative to `root`) changed.
    pub fn invalidate_path(&self, root: &Path, rel: &str) {
        let k = key(root);
        if let Some(e) = self.entries.lock().get_mut(&k) {
            if e.dirty.len() >= 512 {
                e.full_rebuild = true;
            } else {
                e.dirty.insert(rel.replace('\\', "/"));
            }
        }
    }

    /// Forces a full rebuild of `root` on next use.
    pub fn invalidate_root(&self, root: &Path) {
        if let Some(e) = self.entries.lock().get_mut(&key(root)) {
            e.full_rebuild = true;
        }
    }

    /// Drops every entry.
    pub fn clear(&self) {
        self.entries.lock().clear();
    }

    /// Counters.
    #[must_use]
    pub fn stats(&self) -> CacheStats {
        *self.stats.lock()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caches_and_invalidates_incrementally() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        std::fs::create_dir_all(root.join("src")).unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod a;\n").unwrap();
        std::fs::write(root.join("src/a.rs"), "pub fn alpha() {}\n").unwrap();
        let cache = IndexCache::new(IndexOptions::default(), Duration::from_secs(60), 2);
        let p1 = cache.get(root).unwrap();
        assert_eq!(p1.index.files.len(), 2);
        assert_eq!(p1.graph.imports_of("src/lib.rs"), vec!["src/a.rs"]);
        let p2 = cache.get(root).unwrap();
        assert!(Arc::ptr_eq(&p1, &p2));
        assert_eq!(cache.stats(), CacheStats { hits: 1, builds: 1, refreshes: 0 });

        // New file + edit → incremental refresh.
        std::fs::write(root.join("src/b.rs"), "pub fn beta() {}\n").unwrap();
        std::fs::write(root.join("src/lib.rs"), "mod a;\nmod b;\n").unwrap();
        cache.invalidate_path(root, "src/b.rs");
        cache.invalidate_path(root, "src\\lib.rs");
        let p3 = cache.get(root).unwrap();
        assert_eq!(p3.index.files.len(), 3);
        assert_eq!(p3.graph.imports_of("src/lib.rs"), vec!["src/a.rs", "src/b.rs"]);
        assert_eq!(p3.index.get("src/b.rs").unwrap().symbols[0].name, "beta");
        assert_eq!(cache.stats().refreshes, 1);
        assert_eq!(cache.stats().builds, 1);

        // Delete → removed.
        std::fs::remove_file(root.join("src/a.rs")).unwrap();
        cache.invalidate_path(root, "src/a.rs");
        let p4 = cache.get(root).unwrap();
        assert!(p4.index.get("src/a.rs").is_none());
        // Escaping paths are ignored.
        cache.invalidate_path(root, "../outside.rs");
        assert_eq!(cache.get(root).unwrap().index.files.len(), 2);

        cache.invalidate_root(root);
        cache.get(root).unwrap();
        assert_eq!(cache.stats().builds, 2);
    }

    #[test]
    fn bounded_and_expiring() {
        let dirs: Vec<_> = (0..3).map(|_| tempfile::tempdir().unwrap()).collect();
        for d in &dirs {
            std::fs::write(d.path().join("x.rs"), "fn x() {}").unwrap();
        }
        let cache = IndexCache::new(IndexOptions::default(), Duration::from_millis(0), 2);
        for d in &dirs {
            cache.get(d.path()).unwrap();
        }
        assert!(cache.entries.lock().len() <= 2);
        // max_age 0 ⇒ every get rebuilds.
        cache.get(dirs[2].path()).unwrap();
        assert_eq!(cache.stats().builds, 4);
        assert!(cache.get(&dirs[0].path().join("missing")).is_err());
    }
}
