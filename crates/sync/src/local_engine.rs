//! [`DefaultSyncEngine`]: the `SyncEngine` implementation (ADR-0031).
//!
//! Local state lives in `<dir>/sync.db` (journal, outbox, stream bases,
//! records, conflicts, file-hash cache) and sealed objects waiting for upload
//! in `<dir>/staging/`. One round (`sync_now`) per project:
//!
//! 1. scan the working tree + records → chunk (FastCDC) → seal new chunks into
//!    the outbox (works offline);
//! 2. flush the outbox (chunks before manifests);
//! 3. ensure this device is in the sealed key ring;
//! 4. per stream: fetch + verify the head (signature, anti-rollback), merge
//!    against the last synced base, apply remote changes atomically, publish
//!    a signed manifest + ref with compare-and-swap (retry on 412).
//!
//! Sync never runs Git and never writes into `.git/`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use orkestr_domain::{PrivacyPolicy, ProjectId, Timestamp};
use rusqlite::{params, Connection, OptionalExtension};

use crate::crypto::{DeviceKey, Purpose, WorkspaceKeys};
use crate::engine::{ConflictId, RemoteSyncConfig, Resolution, SyncConflict, SyncEngine, SyncStatus};
use crate::manifest::{self, ChunkRef, KeyRing, Manifest, RingMember, SignedManifest, SignedRef, TreeEntry};
use crate::merge::{self, Apply, Tree};
use crate::remote::{CasOutcome, HttpRemote};
use crate::scan;
use crate::vv::{Causality, Hlc, VersionVector};
use crate::{SyncError, SyncReport};

const RING_STREAM: &str = "_ring";
const MAX_CAS_ATTEMPTS: usize = 4;

/// Checks the project's privacy policy (§56, SR-76): `No Remote Sync` ⇒
/// `PolicyDenied`. The sync server is self-hosted (the user's gateway).
///
/// # Errors
/// `PolicyDenied` when remote sync is not allowed.
pub fn check_policy(policy: &PrivacyPolicy) -> Result<(), SyncError> {
    if policy.allows_remote_sync(true) {
        Ok(())
    } else {
        Err(SyncError::PolicyDenied("the project's privacy policy forbids remote sync".into()))
    }
}

fn db_err(e: impl std::fmt::Display) -> SyncError {
    SyncError::Storage(e.to_string())
}

#[derive(Debug, Clone)]
struct ProjectCfg {
    root: PathBuf,
    server_url: String,
    token: String,
    remote_key: String,
    project_key: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StreamKind {
    Workspace,
    Records,
}

impl StreamKind {
    fn name(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Records => "records",
        }
    }
    fn records(self) -> bool {
        self == Self::Records
    }
}

struct Inner {
    db: Mutex<Connection>,
    staging: PathBuf,
    device_id: String,
    device_key: DeviceKey,
    hlc: Mutex<Hlc>,
    round: tokio::sync::Mutex<()>,
}

/// The default sync engine (journal + outbox in SQLite, HTTP remote backend).
#[derive(Clone)]
pub struct DefaultSyncEngine {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DefaultSyncEngine {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DefaultSyncEngine").field("device_id", &self.inner.device_id).finish_non_exhaustive()
    }
}

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
CREATE TABLE IF NOT EXISTS meta (k TEXT PRIMARY KEY, v TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS projects (project_id TEXT PRIMARY KEY, root TEXT NOT NULL, server_url TEXT NOT NULL,
  token TEXT NOT NULL, remote_key TEXT NOT NULL, project_key TEXT NOT NULL, last_sync_at TEXT, last_error TEXT);
CREATE TABLE IF NOT EXISTS streams (project_id TEXT NOT NULL, kind TEXT NOT NULL, base TEXT, seen_vv TEXT,
  head_id TEXT, PRIMARY KEY (project_id, kind));
CREATE TABLE IF NOT EXISTS records (project_id TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL,
  hlc TEXT NOT NULL, PRIMARY KEY (project_id, key));
CREATE TABLE IF NOT EXISTS file_cache (project_id TEXT NOT NULL, path TEXT NOT NULL, size INTEGER NOT NULL,
  mtime_ns INTEGER NOT NULL, content TEXT NOT NULL, chunks TEXT NOT NULL, PRIMARY KEY (project_id, path));
CREATE TABLE IF NOT EXISTS uploaded (project_id TEXT NOT NULL, object_id TEXT NOT NULL, PRIMARY KEY (project_id, object_id));
CREATE TABLE IF NOT EXISTS sync_outbox (id INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL,
  kind TEXT NOT NULL, object_id TEXT NOT NULL, attempts INTEGER NOT NULL DEFAULT 0, next_attempt_at TEXT,
  last_error TEXT, UNIQUE (project_id, kind, object_id));
CREATE TABLE IF NOT EXISTS sync_journal (seq INTEGER PRIMARY KEY AUTOINCREMENT, project_id TEXT NOT NULL,
  stream TEXT NOT NULL, entry_kind TEXT NOT NULL, path_or_key TEXT NOT NULL, content_hash TEXT,
  device_id TEXT NOT NULL, created_at TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS sync_conflicts (id TEXT PRIMARY KEY, project_id TEXT NOT NULL, path TEXT NOT NULL,
  copy_path TEXT, local_hash TEXT, remote_hash TEXT NOT NULL, remote_device TEXT NOT NULL,
  detected_at TEXT NOT NULL, resolution TEXT, UNIQUE (project_id, path, remote_hash));
";

impl DefaultSyncEngine {
    /// Opens (creating) the engine state under `dir` (`<data_dir>/sync`).
    ///
    /// # Errors
    /// `Storage` when the directory or database cannot be opened.
    pub fn open(dir: &Path) -> Result<Self, SyncError> {
        let staging = dir.join("staging");
        std::fs::create_dir_all(&staging).map_err(db_err)?;
        let c = Connection::open(dir.join("sync.db")).map_err(db_err)?;
        c.execute_batch(SCHEMA).map_err(db_err)?;
        let get = |k: &str| -> Result<Option<String>, SyncError> {
            c.query_row("SELECT v FROM meta WHERE k=?1", [k], |r| r.get(0)).optional().map_err(db_err)
        };
        let device_id = match get("device_id")? {
            Some(d) => d,
            None => {
                let d = format!("d_{}", hex16());
                c.execute("INSERT INTO meta(k,v) VALUES('device_id',?1)", [&d]).map_err(db_err)?;
                d
            }
        };
        let device_key = match get("device_key")? {
            Some(k) => {
                let b: [u8; 32] = crate::crypto::unb64(&k)?
                    .try_into()
                    .map_err(|_| SyncError::Storage("corrupt device key".into()))?;
                DeviceKey::from_bytes(&b)
            }
            None => {
                let k = DeviceKey::generate();
                c.execute("INSERT INTO meta(k,v) VALUES('device_key',?1)", [crate::crypto::b64(&k.to_bytes())])
                    .map_err(db_err)?;
                k
            }
        };
        Ok(Self {
            inner: Arc::new(Inner {
                db: Mutex::new(c),
                staging,
                device_id,
                device_key,
                hlc: Mutex::new(Hlc::default()),
                round: tokio::sync::Mutex::new(()),
            }),
        })
    }

    /// This device's sync id.
    #[must_use]
    pub fn device_id(&self) -> &str {
        &self.inner.device_id
    }

    /// Runs forever: every `interval`, retries projects with a pending outbox
    /// or a failed last round (offline queue, §8). Spawn it and abort on shutdown.
    pub async fn run_retry_loop(self, interval: Duration) {
        let mut tick = tokio::time::interval(interval);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tick.tick().await;
            let due = self.inner.with_db(|c| {
                let mut s = c.prepare(
                    "SELECT project_id FROM projects p WHERE last_error IS NOT NULL
                       OR EXISTS (SELECT 1 FROM sync_outbox o WHERE o.project_id = p.project_id)",
                )?;
                let rows = s.query_map([], |r| r.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
                Ok(rows)
            });
            for p in due.unwrap_or_default() {
                if let Ok(pid) = ProjectId::parse(&p) {
                    if let Err(e) = self.sync_now(pid).await {
                        tracing::debug!(project = %p, error = %e, "sync retry failed");
                    }
                }
            }
        }
    }
}

fn hex16() -> String {
    crate::crypto::random::<8>().iter().map(|b| format!("{b:02x}")).collect()
}

impl Inner {
    fn with_db<T>(&self, f: impl FnOnce(&Connection) -> rusqlite::Result<T>) -> Result<T, SyncError> {
        let c = self.db.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        f(&c).map_err(db_err)
    }

    fn project(&self, p: ProjectId) -> Result<Option<ProjectCfg>, SyncError> {
        self.with_db(|c| {
            c.query_row(
                "SELECT root, server_url, token, remote_key, project_key FROM projects WHERE project_id=?1",
                [p.to_string()],
                |r| {
                    Ok(ProjectCfg {
                        root: PathBuf::from(r.get::<_, String>(0)?),
                        server_url: r.get(1)?,
                        token: r.get(2)?,
                        remote_key: r.get(3)?,
                        project_key: r.get(4)?,
                    })
                },
            )
            .optional()
        })
    }

    fn set_error(&self, p: ProjectId, err: Option<&str>) {
        let _ = self.with_db(|c| {
            c.execute("UPDATE projects SET last_error=?2 WHERE project_id=?1", params![p.to_string(), err])
        });
    }

    fn journal(&self, p: ProjectId, kind: StreamKind, entry_kind: &str, path: &str, hash: Option<&str>) {
        let _ = self.with_db(|c| {
            c.execute(
                "INSERT INTO sync_journal(project_id, stream, entry_kind, path_or_key, content_hash, device_id, created_at)
                 VALUES (?1,?2,?3,?4,?5,?6,?7)",
                params![p.to_string(), kind.name(), entry_kind, path, hash, self.device_id, orkestr_domain::now().to_rfc3339()],
            )
        });
    }

    /// Stages a sealed object for upload (idempotent).
    fn stage(&self, p: ProjectId, kind: &str, id: &str, sealed: &[u8]) -> Result<(), SyncError> {
        let known = self.with_db(|c| {
            let up: bool = c
                .query_row(
                    "SELECT 1 FROM uploaded WHERE project_id=?1 AND object_id=?2",
                    params![p.to_string(), id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            let queued: bool = c
                .query_row(
                    "SELECT 1 FROM sync_outbox WHERE project_id=?1 AND kind=?2 AND object_id=?3",
                    params![p.to_string(), kind, id],
                    |_| Ok(true),
                )
                .optional()?
                .unwrap_or(false);
            Ok(up || queued)
        })?;
        if known {
            return Ok(());
        }
        let path = self.staging.join(format!("{kind}-{id}"));
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, sealed).map_err(db_err)?;
        std::fs::rename(&tmp, &path).map_err(db_err)?;
        self.with_db(|c| {
            c.execute(
                "INSERT OR IGNORE INTO sync_outbox(project_id, kind, object_id) VALUES (?1,?2,?3)",
                params![p.to_string(), kind, id],
            )
        })?;
        Ok(())
    }

    fn pending(&self, p: ProjectId) -> Result<u64, SyncError> {
        self.with_db(|c| {
            c.query_row("SELECT COUNT(*) FROM sync_outbox WHERE project_id=?1", [p.to_string()], |r| r.get::<_, i64>(0))
        })
        .map(|n| u64::try_from(n).unwrap_or(0))
    }

    /// Uploads the outbox: chunks before manifests (§8 ordering).
    async fn flush(&self, p: ProjectId, remote: &HttpRemote, report: &mut SyncReport) -> Result<(), SyncError> {
        let items: Vec<(i64, String, String)> = self.with_db(|c| {
            let mut s = c.prepare(
                "SELECT id, kind, object_id FROM sync_outbox WHERE project_id=?1 ORDER BY (kind='manifests'), id",
            )?;
            let rows = s
                .query_map([p.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        if items.is_empty() {
            return Ok(());
        }
        let chunk_ids: Vec<String> = items.iter().filter(|i| i.1 == "chunks").map(|i| i.2.clone()).collect();
        let have: BTreeMap<String, bool> = match remote.has_chunks(&chunk_ids).await {
            Ok(h) => chunk_ids.into_iter().zip(h).collect(),
            Err(e) => return Err(self.outbox_failed(p, &e)),
        };
        for (row, kind, id) in items {
            let path = self.staging.join(format!("{kind}-{id}"));
            if !have.get(&id).copied().unwrap_or(false) {
                let body = match std::fs::read(&path) {
                    Ok(b) => b,
                    Err(_) => {
                        // staging lost (crash): drop the row; the next scan re-stages.
                        self.with_db(|c| c.execute("DELETE FROM sync_outbox WHERE id=?1", [row]))?;
                        continue;
                    }
                };
                let len = body.len() as u64;
                if let Err(e) = remote.put_object(&kind, &id, body).await {
                    return Err(self.outbox_failed(p, &e));
                }
                report.pushed += 1;
                report.bytes += len;
            }
            self.with_db(|c| {
                c.execute("DELETE FROM sync_outbox WHERE id=?1", [row])?;
                c.execute(
                    "INSERT OR IGNORE INTO uploaded(project_id, object_id) VALUES (?1,?2)",
                    params![p.to_string(), id],
                )
            })?;
            let _ = std::fs::remove_file(&path);
        }
        Ok(())
    }

    fn outbox_failed(&self, p: ProjectId, e: &SyncError) -> SyncError {
        let msg = e.to_string();
        let _ = self.with_db(|c| {
            c.execute(
                "UPDATE sync_outbox SET attempts = attempts + 1, last_error = ?2, next_attempt_at = ?3 WHERE project_id = ?1",
                params![p.to_string(), msg, orkestr_domain::now().to_rfc3339()],
            )
        });
        SyncError::Network(msg)
    }

    /// Scans the working tree, staging sealed chunks of new content.
    fn build_workspace(&self, p: ProjectId, cfg: &ProjectCfg, keys: &WorkspaceKeys) -> Result<Tree, SyncError> {
        let pid = p.to_string();
        let cache: BTreeMap<String, (i64, i64, String, String)> = self.with_db(|c| {
            let mut s =
                c.prepare("SELECT path, size, mtime_ns, content, chunks FROM file_cache WHERE project_id=?1")?;
            let rows = s
                .query_map([&pid], |r| Ok((r.get::<_, String>(0)?, (r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))))?
                .collect::<Result<BTreeMap<_, _>, _>>()?;
            Ok(rows)
        })?;
        let mut tree = Tree::new();
        for f in scan::scan(&cfg.root) {
            let size = i64::try_from(f.size).unwrap_or(i64::MAX);
            if let Some((cs, cm, content, chunks)) = cache.get(&f.path) {
                if *cs == size && *cm == f.mtime_ns {
                    if let Ok(chunks) = serde_json::from_str::<Vec<ChunkRef>>(chunks) {
                        tree.insert(
                            f.path.clone(),
                            TreeEntry {
                                path: f.path,
                                content: content.clone(),
                                size: f.size,
                                exec: f.exec,
                                chunks,
                                hlc: None,
                            },
                        );
                        continue;
                    }
                }
            }
            let Ok(data) = std::fs::read(&f.abs) else { continue };
            let content = blake3::hash(&data).to_hex().to_string();
            let mut chunks = Vec::new();
            for (off, len) in manifest::chunk_bounds(&data) {
                let plain = &data[off..off + len];
                let id = keys.chunk_id(plain);
                let sealed = keys.seal(Purpose::Chunk, chunk_aad(&id).as_bytes(), plain, true)?;
                self.stage(p, "chunks", &id, &sealed)?;
                chunks.push(ChunkRef { id, len: len as u64 });
            }
            let chunks_json = serde_json::to_string(&chunks).map_err(db_err)?;
            self.with_db(|c| {
                c.execute(
                    "INSERT OR REPLACE INTO file_cache(project_id, path, size, mtime_ns, content, chunks) VALUES (?1,?2,?3,?4,?5,?6)",
                    params![pid, f.path, size, f.mtime_ns, content, chunks_json],
                )
            })?;
            tree.insert(
                f.path.clone(),
                TreeEntry { path: f.path, content, size: f.size, exec: f.exec, chunks, hlc: None },
            );
        }
        Ok(tree)
    }

    fn build_records(&self, p: ProjectId, keys: &WorkspaceKeys) -> Result<Tree, SyncError> {
        let rows: Vec<(String, String, String)> = self.with_db(|c| {
            let mut s = c.prepare("SELECT key, value, hlc FROM records WHERE project_id=?1")?;
            let rows = s
                .query_map([p.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        let mut tree = Tree::new();
        for (key, value, hlc) in rows {
            let bytes = value.into_bytes();
            let id = record_chunk_id(keys, &bytes);
            let sealed = keys.seal_with(Purpose::Chunk, true, chunk_aad(&id).as_bytes(), &bytes, true)?;
            self.stage(p, "chunks", &id, &sealed)?;
            tree.insert(
                key.clone(),
                TreeEntry {
                    path: key,
                    content: blake3::hash(&bytes).to_hex().to_string(),
                    size: bytes.len() as u64,
                    exec: false,
                    chunks: vec![ChunkRef { id, len: bytes.len() as u64 }],
                    hlc: Some(hlc),
                },
            );
        }
        Ok(tree)
    }

    fn stream_state(&self, p: ProjectId, kind: StreamKind) -> Result<(Tree, VersionVector), SyncError> {
        let row: Option<(Option<String>, Option<String>)> = self.with_db(|c| {
            c.query_row(
                "SELECT base, seen_vv FROM streams WHERE project_id=?1 AND kind=?2",
                params![p.to_string(), kind.name()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()
        })?;
        let (base, seen) = row.unwrap_or((None, None));
        let base: Vec<TreeEntry> = base.and_then(|b| serde_json::from_str(&b).ok()).unwrap_or_default();
        let seen = seen.and_then(|s| serde_json::from_str(&s).ok()).unwrap_or_default();
        Ok((base.into_iter().map(|e| (e.path.clone(), e)).collect(), seen))
    }

    fn save_stream(
        &self,
        p: ProjectId,
        kind: StreamKind,
        base: &Tree,
        seen: &VersionVector,
        head: &str,
    ) -> Result<(), SyncError> {
        let base = serde_json::to_string(&base.values().collect::<Vec<_>>()).map_err(db_err)?;
        let seen = serde_json::to_string(seen).map_err(db_err)?;
        self.with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO streams(project_id, kind, base, seen_vv, head_id) VALUES (?1,?2,?3,?4,?5)",
                params![p.to_string(), kind.name(), base, seen, head],
            )
        })?;
        Ok(())
    }

    async fn download(
        &self,
        remote: &HttpRemote,
        keys: &WorkspaceKeys,
        e: &TreeEntry,
        records: bool,
    ) -> Result<Vec<u8>, SyncError> {
        let mut out = Vec::with_capacity(usize::try_from(e.size).unwrap_or(0));
        for c in &e.chunks {
            let sealed = remote.get_object("chunks", &c.id).await?;
            let plain = keys.open_with(Purpose::Chunk, records, chunk_aad(&c.id).as_bytes(), &sealed)?;
            let expected = if records { record_chunk_id(keys, &plain) } else { keys.chunk_id(&plain) };
            if expected != c.id {
                return Err(SyncError::Integrity(format!("chunk {} does not match its id", c.id)));
            }
            out.extend_from_slice(&plain);
        }
        if blake3::hash(&out).to_hex().as_str() != e.content {
            return Err(SyncError::Integrity(format!("content hash mismatch for {}", e.path)));
        }
        Ok(out)
    }

    async fn ensure_ring(&self, remote: &HttpRemote, keys: &WorkspaceKeys) -> Result<KeyRing, SyncError> {
        for _ in 0..MAX_CAS_ATTEMPTS {
            let head = remote.get_head(RING_STREAM).await?;
            let mut ring = match &head {
                Some(h) => KeyRing::open(keys, &crate::crypto::unb64(&h.value)?)?,
                None => KeyRing { epoch: 1, devices: Vec::new() },
            };
            if let Some(m) = ring.devices.iter().find(|d| d.device_id == self.device_id) {
                if m.ed25519 != self.device_key.public() {
                    return Err(SyncError::Integrity("key ring holds a different key for this device".into()));
                }
                return Ok(ring);
            }
            ring.devices.push(RingMember { device_id: self.device_id.clone(), ed25519: self.device_key.public() });
            let value = crate::crypto::b64(&ring.seal(keys)?);
            match remote.cas_head(RING_STREAM, head.as_ref().map(|h| h.etag.as_str()), value).await? {
                CasOutcome::Updated(_) => return Ok(ring),
                CasOutcome::Conflict(_) => {}
            }
        }
        Err(SyncError::Network("key ring is changing too fast; retry".into()))
    }

    /// Merges + applies + publishes one stream. Returns new conflicts.
    #[allow(clippy::too_many_arguments)]
    async fn sync_stream(
        &self,
        p: ProjectId,
        cfg: &ProjectCfg,
        keys: &WorkspaceKeys,
        remote: &HttpRemote,
        ring: &KeyRing,
        kind: StreamKind,
        mut local: Tree,
        report: &mut SyncReport,
    ) -> Result<(), SyncError> {
        let sid = keys.stream_id(&format!("{}:{}", kind.name(), cfg.project_key));
        let (mut base, mut seen) = self.stream_state(p, kind)?;
        if base.is_empty() && seen.0.is_empty() && kind == StreamKind::Workspace {
            // First sync: files identical to Git HEAD count as unmodified, so the
            // remote side wins for them instead of raising false conflicts.
            let pristine = git_head_blobs(&cfg.root);
            for (path, e) in &local {
                if pristine.get(path).is_some_and(|oid| *oid == git_blob_oid(&cfg.root.join(path))) {
                    base.insert(path.clone(), e.clone());
                }
            }
        }
        for (path, e) in &local {
            if base.get(path).map(|b| &b.content) != Some(&e.content) {
                self.journal(p, kind, if kind.records() { "record_op" } else { "file_upsert" }, path, Some(&e.content));
            }
        }
        for path in base.keys().filter(|k| !local.contains_key(*k)) {
            self.journal(p, kind, "file_delete", path, None);
        }
        let mut head = remote.get_head(&sid).await?;
        for _ in 0..MAX_CAS_ATTEMPTS {
            // 1. verify the remote head.
            let remote_state = match &head {
                None => {
                    if !seen.0.is_empty() {
                        return Err(SyncError::Integrity("remote head disappeared (rollback)".into()));
                    }
                    None
                }
                Some(h) => {
                    let r: SignedRef =
                        serde_json::from_str(&h.value).map_err(|e| SyncError::Integrity(format!("ref: {e}")))?;
                    if r.stream_id != sid {
                        return Err(SyncError::Integrity("ref names another stream".into()));
                    }
                    r.verify(ring)?;
                    if matches!(r.version.compare(&seen), Causality::Before | Causality::Concurrent) {
                        return Err(SyncError::Integrity(
                            "remote head is older than one already seen (rollback)".into(),
                        ));
                    }
                    let sm: SignedManifest =
                        serde_json::from_slice(&remote.get_object("manifests", &r.manifest_id).await?)
                            .map_err(|e| SyncError::Integrity(format!("manifest: {e}")))?;
                    let m = manifest::open_manifest(keys, ring, &sid, &r.manifest_id, &sm)?;
                    if m.version != r.version {
                        return Err(SyncError::Integrity("manifest version differs from its ref".into()));
                    }
                    Some((m, h.etag.clone(), r))
                }
            };
            // 2. merge + apply.
            let (merged, remote_vv, parent, etag) = match &remote_state {
                Some((m, etag, r)) => {
                    let rtree: Tree = m.entries.iter().map(|e| (e.path.clone(), e.clone())).collect();
                    let out = merge::merge(&base, &local, &rtree, kind.records(), &m.device_id);
                    self.apply(p, cfg, keys, remote, kind, &out.apply, report).await?;
                    for c in &out.conflicts {
                        let new = self.with_db(|db| {
                            db.execute(
                                "INSERT OR IGNORE INTO sync_conflicts(id, project_id, path, copy_path, local_hash, remote_hash,
                                   remote_device, detected_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?8)",
                                params![
                                    format!("c_{}", hex16()),
                                    p.to_string(),
                                    c.path,
                                    c.copy_path,
                                    c.local,
                                    c.remote,
                                    m.device_id,
                                    orkestr_domain::now().to_rfc3339()
                                ],
                            )
                        })?;
                        report.conflicts += u32::try_from(new).unwrap_or(0);
                    }
                    // Remote state is integrated: it becomes the base.
                    base = rtree.clone();
                    seen = r.version.clone();
                    self.save_stream(p, kind, &base, &seen, &r.manifest_id)?;
                    if out.tree == rtree {
                        return Ok(());
                    }
                    (out.tree, r.version.clone(), vec![r.manifest_id.clone()], Some(etag.clone()))
                }
                None => (local.clone(), VersionVector::default(), Vec::new(), None),
            };
            // 3. publish.
            let mut vv = remote_vv;
            vv.merge(&seen);
            vv.bump(&self.device_id);
            let m = Manifest {
                format: 1,
                stream: sid.clone(),
                parent,
                version: vv.clone(),
                device_id: self.device_id.clone(),
                created_at: orkestr_domain::now().to_rfc3339(),
                entries: merged.values().cloned().collect(),
            };
            let sm = manifest::seal_manifest(keys, &self.device_key, &m)?;
            self.stage(p, "manifests", &sm.manifest_id, &serde_json::to_vec(&sm).map_err(db_err)?)?;
            self.flush(p, remote, report).await?;
            let r = SignedRef::sign(&self.device_key, &self.device_id, &sid, &sm.manifest_id, vv.clone());
            let value = serde_json::to_string(&r).map_err(db_err)?;
            match remote.cas_head(&sid, etag.as_deref(), value).await? {
                CasOutcome::Updated(_) => {
                    report.pushed += 1;
                    self.save_stream(p, kind, &merged, &vv, &sm.manifest_id)?;
                    return Ok(());
                }
                CasOutcome::Conflict(cur) => {
                    local = merged;
                    head = cur;
                }
            }
        }
        Err(SyncError::Network("remote head keeps moving; retry later".into()))
    }

    #[allow(clippy::too_many_arguments)]
    async fn apply(
        &self,
        p: ProjectId,
        cfg: &ProjectCfg,
        keys: &WorkspaceKeys,
        remote: &HttpRemote,
        kind: StreamKind,
        ops: &[Apply],
        report: &mut SyncReport,
    ) -> Result<(), SyncError> {
        for op in ops {
            match op {
                Apply::Write(e) => {
                    let data = self.download(remote, keys, e, kind.records()).await?;
                    report.pulled += 1;
                    report.bytes += data.len() as u64;
                    if kind.records() {
                        let value =
                            String::from_utf8(data).map_err(|_| SyncError::Integrity("record is not UTF-8".into()))?;
                        self.with_db(|c| {
                            c.execute(
                                "INSERT OR REPLACE INTO records(project_id, key, value, hlc) VALUES (?1,?2,?3,?4)",
                                params![p.to_string(), e.path, value, e.hlc.clone().unwrap_or_default()],
                            )
                        })?;
                        if let Some(h) = &e.hlc {
                            self.hlc
                                .lock()
                                .unwrap_or_else(std::sync::PoisonError::into_inner)
                                .tick(&self.device_id, Some(h));
                        }
                    } else {
                        write_file(&cfg.root, &e.path, &data)?;
                        self.forget_cache(p, &e.path)?;
                    }
                }
                Apply::Delete(path) => {
                    report.pulled += 1;
                    if kind.records() {
                        self.with_db(|c| {
                            c.execute(
                                "DELETE FROM records WHERE project_id=?1 AND key=?2",
                                params![p.to_string(), path],
                            )
                        })?;
                    } else if !scan::is_excluded_path(path) && manifest::validate_path(path).is_ok() {
                        let _ = std::fs::remove_file(cfg.root.join(path));
                        self.forget_cache(p, path)?;
                    }
                }
            }
        }
        Ok(())
    }

    fn forget_cache(&self, p: ProjectId, path: &str) -> Result<(), SyncError> {
        self.with_db(|c| {
            c.execute("DELETE FROM file_cache WHERE project_id=?1 AND path=?2", params![p.to_string(), path])
        })?;
        Ok(())
    }

    async fn round(&self, p: ProjectId) -> Result<SyncReport, SyncError> {
        let cfg = self.project(p)?.ok_or(SyncError::NotConfigured)?;
        let keys = WorkspaceKeys::from_remote_key(&cfg.remote_key)?;
        let remote = HttpRemote::new(&cfg.server_url, &cfg.token)?;
        let ws = self.build_workspace(p, &cfg, &keys)?;
        let recs = self.build_records(p, &keys)?;
        let mut report = SyncReport::default();
        self.flush(p, &remote, &mut report).await?;
        let ring = self.ensure_ring(&remote, &keys).await?;
        self.sync_stream(p, &cfg, &keys, &remote, &ring, StreamKind::Workspace, ws, &mut report).await?;
        self.sync_stream(p, &cfg, &keys, &remote, &ring, StreamKind::Records, recs, &mut report).await?;
        Ok(report)
    }
}

fn chunk_aad(id: &str) -> String {
    format!("orkestr-chunk-v1|{id}")
}

fn record_chunk_id(keys: &WorkspaceKeys, bytes: &[u8]) -> String {
    let mut v = b"record\0".to_vec();
    v.extend_from_slice(bytes);
    keys.chunk_id(&v)
}

/// Atomic write (temp file in the same directory + rename).
fn write_file(root: &Path, rel: &str, data: &[u8]) -> Result<(), SyncError> {
    manifest::validate_path(rel)?;
    if scan::is_excluded_path(rel) {
        return Err(SyncError::Integrity(format!("refusing to write excluded path {rel}")));
    }
    let target = root.join(rel);
    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(db_err)?;
    }
    let tmp = target.with_file_name(format!(".orkestr-sync-{}.tmp", hex16()));
    std::fs::write(&tmp, data).map_err(db_err)?;
    std::fs::rename(&tmp, &target).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        db_err(e)
    })
}

/// `path → blob oid` of Git HEAD (read-only `git ls-tree`; empty without Git).
fn git_head_blobs(root: &Path) -> BTreeMap<String, String> {
    if !root.join(".git").exists() {
        return BTreeMap::new();
    }
    let Ok(out) = orkestr_platform::process::background_command("git")
        .arg("-C")
        .arg(root)
        .args(["ls-tree", "-r", "-z", "HEAD"])
        .output()
    else {
        return BTreeMap::new();
    };
    if !out.status.success() {
        return BTreeMap::new();
    }
    String::from_utf8_lossy(&out.stdout)
        .split('\0')
        .filter_map(|line| {
            let (meta, path) = line.split_once('\t')?;
            let mut it = meta.split(' ');
            let (_mode, kind, oid) = (it.next()?, it.next()?, it.next()?);
            (kind == "blob").then(|| (path.to_owned(), oid.to_owned()))
        })
        .collect()
}

/// Git blob id (SHA-1 of `blob <len>\0<content>`) of a file.
fn git_blob_oid(path: &Path) -> String {
    use sha1::Digest as _;
    let Ok(data) = std::fs::read(path) else { return String::new() };
    let mut h = sha1::Sha1::new();
    h.update(format!("blob {}\0", data.len()).as_bytes());
    h.update(&data);
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

fn parse_ts(s: &str) -> Option<Timestamp> {
    serde_json::from_value(serde_json::Value::String(s.to_owned())).ok()
}

#[async_trait]
impl SyncEngine for DefaultSyncEngine {
    async fn configure(&self, p: ProjectId, root: PathBuf, cfg: RemoteSyncConfig) -> Result<(), SyncError> {
        WorkspaceKeys::from_remote_key(&cfg.remote_key)?;
        HttpRemote::new(&cfg.server_url, &cfg.access_token)?;
        if !root.is_dir() {
            return Err(SyncError::Config(format!("project root {} is not a directory", root.display())));
        }
        let project_key = cfg.project_key.clone().unwrap_or_else(|| p.to_string());
        let prev = self.inner.project(p)?;
        let reset = prev.as_ref().is_none_or(|o| {
            o.server_url != cfg.server_url
                || o.remote_key != cfg.remote_key
                || o.project_key != project_key
                || o.root != root
        });
        let pid = p.to_string();
        self.inner.with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO projects(project_id, root, server_url, token, remote_key, project_key, last_sync_at, last_error)
                 VALUES (?1,?2,?3,?4,?5,?6,(SELECT last_sync_at FROM projects WHERE project_id=?1),NULL)",
                params![pid, root.to_string_lossy(), cfg.server_url, cfg.access_token, cfg.remote_key, project_key],
            )?;
            if reset {
                for t in ["streams", "file_cache", "uploaded", "sync_outbox"] {
                    c.execute(&format!("DELETE FROM {t} WHERE project_id=?1"), [&pid])?;
                }
            }
            Ok(())
        })?;
        Ok(())
    }

    async fn disable(&self, p: ProjectId) -> Result<(), SyncError> {
        let pid = p.to_string();
        self.inner.with_db(|c| {
            for t in ["projects", "streams", "file_cache", "uploaded", "sync_outbox"] {
                c.execute(&format!("DELETE FROM {t} WHERE project_id=?1"), [&pid])?;
            }
            Ok(())
        })
    }

    async fn put_record(&self, p: ProjectId, key: &str, v: serde_json::Value) -> Result<(), SyncError> {
        manifest::validate_path(key).map_err(|_| SyncError::Config(format!("invalid record key {key}")))?;
        let hlc =
            self.inner.hlc.lock().unwrap_or_else(std::sync::PoisonError::into_inner).tick(&self.inner.device_id, None);
        let value = serde_json::to_string(&v).map_err(db_err)?;
        self.inner.with_db(|c| {
            c.execute(
                "INSERT OR REPLACE INTO records(project_id, key, value, hlc) VALUES (?1,?2,?3,?4)",
                params![p.to_string(), key, value, hlc],
            )
        })?;
        self.inner.journal(p, StreamKind::Records, "record_op", key, None);
        Ok(())
    }

    async fn get_record(&self, p: ProjectId, key: &str) -> Result<Option<serde_json::Value>, SyncError> {
        let v: Option<String> = self.inner.with_db(|c| {
            c.query_row("SELECT value FROM records WHERE project_id=?1 AND key=?2", params![p.to_string(), key], |r| {
                r.get(0)
            })
            .optional()
        })?;
        v.map(|s| serde_json::from_str(&s).map_err(db_err)).transpose()
    }

    async fn sync_now(&self, p: ProjectId) -> Result<SyncReport, SyncError> {
        let _guard = self.inner.round.lock().await;
        match self.inner.round(p).await {
            Ok(r) => {
                self.inner.with_db(|c| {
                    c.execute(
                        "UPDATE projects SET last_sync_at=?2, last_error=NULL WHERE project_id=?1",
                        params![p.to_string(), orkestr_domain::now().to_rfc3339()],
                    )
                })?;
                Ok(r)
            }
            Err(e) => {
                if !matches!(e, SyncError::NotConfigured) {
                    self.inner.set_error(p, Some(&e.to_string()));
                }
                Err(e)
            }
        }
    }

    async fn status(&self, p: ProjectId) -> Result<SyncStatus, SyncError> {
        let row: Option<(Option<String>, Option<String>)> = self.inner.with_db(|c| {
            c.query_row("SELECT last_sync_at, last_error FROM projects WHERE project_id=?1", [p.to_string()], |r| {
                Ok((r.get(0)?, r.get(1)?))
            })
            .optional()
        })?;
        let Some((last, err)) = row else { return Ok(SyncStatus::default()) };
        let conflicts: i64 = self.inner.with_db(|c| {
            c.query_row(
                "SELECT COUNT(*) FROM sync_conflicts WHERE project_id=?1 AND resolution IS NULL",
                [p.to_string()],
                |r| r.get(0),
            )
        })?;
        Ok(SyncStatus {
            configured: true,
            last_sync_at: last.as_deref().and_then(parse_ts),
            pending: self.inner.pending(p)?,
            conflicts: u32::try_from(conflicts).unwrap_or(u32::MAX),
            last_error: err,
        })
    }

    async fn list_conflicts(&self, p: ProjectId) -> Result<Vec<SyncConflict>, SyncError> {
        let rows: Vec<(String, String, Option<String>, String)> = self.inner.with_db(|c| {
            let mut s = c.prepare(
                "SELECT id, path, copy_path, detected_at FROM sync_conflicts
                 WHERE project_id=?1 AND resolution IS NULL ORDER BY detected_at, id",
            )?;
            let rows = s
                .query_map([p.to_string()], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
                .collect::<Result<Vec<_>, _>>()?;
            Ok(rows)
        })?;
        Ok(rows
            .into_iter()
            .map(|(id, path, copy, at)| SyncConflict {
                id: ConflictId(id),
                project_id: p,
                path,
                conflict_copy: copy,
                detected_at: parse_ts(&at).unwrap_or_else(orkestr_domain::now),
            })
            .collect())
    }

    async fn resolve_conflict(&self, id: ConflictId, r: Resolution) -> Result<(), SyncError> {
        let row: Option<(String, String, Option<String>)> = self.inner.with_db(|c| {
            c.query_row(
                "SELECT project_id, path, copy_path FROM sync_conflicts WHERE id=?1 AND resolution IS NULL",
                [&id.0],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()
        })?;
        let (pid, path, copy) = row.ok_or(SyncError::NotFound)?;
        let p = ProjectId::parse(&pid).map_err(db_err)?;
        let cfg = self.inner.project(p)?.ok_or(SyncError::NotConfigured)?;
        if let Some(copy) = copy {
            manifest::validate_path(&copy)?;
            manifest::validate_path(&path)?;
            let copy_abs = cfg.root.join(&copy);
            match r {
                Resolution::KeepLocal => {
                    let _ = std::fs::remove_file(&copy_abs);
                }
                Resolution::KeepRemote => {
                    if copy_abs.exists() {
                        std::fs::rename(&copy_abs, cfg.root.join(&path)).map_err(db_err)?;
                    }
                }
                Resolution::KeepBoth => {}
            }
            self.inner.forget_cache(p, &copy)?;
            self.inner.forget_cache(p, &path)?;
        }
        let label = match r {
            Resolution::KeepLocal => "keepLocal",
            Resolution::KeepRemote => "keepRemote",
            Resolution::KeepBoth => "keepBoth",
        };
        self.inner
            .with_db(|c| c.execute("UPDATE sync_conflicts SET resolution=?2 WHERE id=?1", params![id.0, label]))?;
        Ok(())
    }
}
