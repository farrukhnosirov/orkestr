# ADR-0007: SQLite (rusqlite) for local metadata + content-addressed object store

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (local), 4 (Postgres/S3 behind the same traits)
- **Spec:** §13, §21, §25, §40–45 (interfaces MetadataStore, ObjectStore), §51, §60.10, §67 ("S3 replaceable", "cloud can be disabled")
- **Review:** CA-3, CA-9, RR-G13, RR-P10
- **Spike:** SP-12 · **Risk:** R-16

## Context

The core persists projects, tasks, agents, approvals, usage, memory, decisions and the event log (§51). It also stores binary or large content: snapshot blobs, artifacts, terminal history and screenshots. It must work offline (§44). §45 names the `MetadataStore` and `ObjectStore` interfaces so that self-hosted Postgres and S3/MinIO can be added in Phase 4.

## Decision drivers

1. Embedded, zero-ops, a single file, crash-safe (WAL).
2. FTS for memory, search and logs (§20, §54).
3. Reversible migrations (§60.10).
4. Large blobs stay out of the DB, and are deduplicated (snapshots repeat content).
5. A trait boundary so Phase 4 can swap backends (§67).

## Considered options

| Option | Verdict |
|---|---|
| **rusqlite (`bundled`, FTS5) + blake3 content-addressed files** | **Chosen** |
| sqlx (SQLite) | Rejected for local use. Its async API sits on a thread pool anyway, compile-time query checks need a DB at build time, and it gives less control over pragmas and FTS. sqlx/Postgres is likely for the Phase 4 self-hosted control plane |
| sled / redb (KV) | Rejected. No SQL or FTS, and relational queries (task DAG, costs) would be hand-built |
| SurrealDB / DuckDB embedded | Rejected. Heavy, and the transactional OLTP fit and maturity are weaker |
| Blobs in SQLite | Rejected. Bloats the DB, slows WAL checkpoints, and gives no dedup across snapshots |

## Decision

### Metadata (`orkestr-storage`)

- A single DB: `<data_dir>/orkestr.db`.
- Pragmas on open: `journal_mode=WAL`, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5000`, `temp_store=MEMORY`, `wal_autocheckpoint=1000`.
- **Connection model:**
  - One **writer** connection, owned by a dedicated OS thread with an mpsc command queue. All writes are serialised and grouped into transactions.
  - A pool of 4 **reader** connections (`SQLITE_OPEN_READ_ONLY`) used through `tokio::task::spawn_blocking`.
  - No `rusqlite::Connection` ever crosses an `.await`.

```rust
pub struct Db { /* writer handle + reader pool */ }
impl Db {
    pub async fn open(path: &Path, migrations: &[MigrationSet]) -> Result<Db, StorageError>;
    /// Runs `f` in one IMMEDIATE transaction on the writer thread.
    pub async fn write<T: Send + 'static>(&self, f: impl FnOnce(&rusqlite::Transaction<'_>) -> Result<T, StorageError> + Send + 'static) -> Result<T, StorageError>;
    pub async fn read<T: Send + 'static>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<T, StorageError> + Send + 'static) -> Result<T, StorageError>;
}

/// Engine-facing port. Engines own typed repositories built on `Db`, never raw SQL from other crates.
pub trait MetadataStore: Send + Sync + 'static {
    fn db(&self) -> &Db;
}
```

- **Migrations (CA-9):**
  ```rust
  pub struct Migration { pub version: u32, pub name: &'static str, pub up: &'static str, pub down: &'static str }
  pub struct MigrationSet { pub owner: &'static str /* crate, e.g. "task-engine" */, pub migrations: &'static [Migration] }
  // _migrations(owner TEXT, version INTEGER, name TEXT, checksum TEXT, applied_at TEXT, PRIMARY KEY(owner, version))
  ```
  - `core` applies the sets in the fixed owner order (CA-9).
  - The checksum (blake3 of `up`) is checked, and a mismatch on an applied migration is a fatal startup error.
  - `orkestrd migrate down --owner X --to N` is provided for development.
  - Every migration must come with an `up`→`down`→`up` round-trip test.
- **Conventions:**
  - Every entity row has `id TEXT PRIMARY KEY` (UUID v7), `created_at`, `updated_at` (RFC3339 UTC), `rev INTEGER NOT NULL DEFAULT 0` (RR-P10), `origin_device_id TEXT`, and `node_id TEXT` where relevant (RR-P4).
  - Optimistic update: `UPDATE … SET rev = rev + 1 … WHERE id = ? AND rev = ?`. Zero rows updated means `conflict`.
  - Soft delete (`deleted_at`) for user-authored content: memory, decisions, UI config versions.
- **FTS:** `memory_fts`, `decisions_fts` and `tasks_fts` are FTS5 tables with content triggers (Phase 1 memory, Phase 2 global search).

### Object store (`orkestr-storage::objects`)

```rust
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize, TS)]
#[serde(transparent)] #[ts(export)]
pub struct ObjectId(String); // "b3:" + 64 lowercase hex

#[async_trait::async_trait]
pub trait ObjectStore: Send + Sync + 'static {
    async fn put_bytes(&self, data: bytes::Bytes) -> Result<ObjectId, StorageError>;
    async fn put_file(&self, path: &Path) -> Result<ObjectId, StorageError>;          // streaming hash + copy
    async fn get_bytes(&self, id: &ObjectId) -> Result<bytes::Bytes, StorageError>;
    async fn open_read(&self, id: &ObjectId) -> Result<Box<dyn tokio::io::AsyncRead + Send + Unpin>, StorageError>;
    async fn restore_to(&self, id: &ObjectId, dest: &Path) -> Result<(), StorageError>; // atomic temp+rename
    async fn exists(&self, id: &ObjectId) -> Result<bool, StorageError>;
    async fn size(&self, id: &ObjectId) -> Result<u64, StorageError>;
    async fn delete_unreferenced(&self, live: &dyn Fn(&ObjectId) -> bool) -> Result<GcReport, StorageError>;
}
```

- **Layout:** `<data_dir>/objects/b3/ab/cdef…` (the first 2 hex characters are the fan-out directory).
- **Writes:** `objects/tmp/<uuid>`, then `fsync`, then `rename`. A write is a no-op if the object already exists.
- **Verification:** reads re-hash when `verify=true` (in snapshot restore and in GC).
- Objects are stored uncompressed in Phase 1. A header byte reserved for `zstd` is added in Phase 4, when sync needs compressed, chunked transfer (§42).
- **References:** the `object_refs(object_id, owner_kind, owner_id)` table. GC is mark-and-sweep over `object_refs`. It runs at idle and never while a snapshot is being created (an advisory lock).
- **Phase 4:** `S3ObjectStore` implements the same trait, and `ObjectId` stays the content hash, so local and remote objects deduplicate.

## Consequences

- **Positive:** zero-ops, offline, FTS included, and snapshot dedup comes for free. A trait boundary is in place for Phase 4.
- **Negative:** a single writer serialises writes. Expected peak at 10 agents is about 300 writes/s, mostly events. SP-12 must show sustained ≥ 2000 small writes/s with p99 < 20 ms on Windows NVMe, with batching of `AgentMessage` handled by keeping it ephemeral (ADR-0018).
- **Negative:** Windows antivirus can slow WAL fsyncs. This is measured in SP-12, and `<data_dir>` exclusion is documented as optional.
- **Negative:** cross-crate foreign keys force a migration order (CA-9). Cyclic foreign keys are forbidden. Use soft references (no FK) where ownership flows the other way.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged. Details: `../architecture/storage-abstraction.md` §0.

- **Connection model as built:** one connection behind a mutex, run on `spawn_blocking` (`Db::call`), not a writer thread + 4 readers; pragmas WAL, `synchronous=NORMAL`, `foreign_keys=ON`, `busy_timeout=5s`, `temp_store=MEMORY` (no `wal_autocheckpoint` override). `Db::write`/`read` do not exist yet.
- `MetadataStore` is `{ fn projects(&self) -> &dyn ProjectRepository }`, not `{ fn db(&self) -> &Db }`.
- **Migrations as built:** `Migration { id, owner, up, down }` with globally unique ids and bookkeeping in `_orkestr_migrations(seq, id, owner, checksum, down_sql, applied_at)`; no `MigrationSet`/`version`. Checksum drift and unknown applied migrations are refused; `down` is mandatory. Order today: core → storage → events. No `orkestrd migrate` command.
- **Object store as built:** `ObjectStore { put(Vec<u8>), get, exists, delete }`; `ObjectId` = bare 64-hex blake3 (no `b3:` prefix); layout `objects/<2>/<2>/<hash>` (no `b3/` level); `OKO1` header + codec byte, and **zstd is already used** (level 3, kept only when smaller). No `object_refs`, GC, streaming or `restore_to` yet.
- No `rev`/`origin_device_id`/`deleted_at` columns and no FTS tables yet (only `core_meta`, `projects`, `events`).
