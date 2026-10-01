# ADR-0019: Sync deferred to Phase 4 behind a `SyncBackend` trait (LocalOnly in Phases 1–3)

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (seam + LocalOnly + journal columns), 4 (cloud and self-hosted backends)
- **Spec:** §40–45 (cloud workspace, Git ≠ live sync, continuous sync pipeline, offline-first, storage modes, interfaces incl. `SyncBackend`), §56 (No Remote Sync), §62.3, §62.4, §62.14, §63C, §63I, §67 ("cloud can be disabled", "managed cloud replaceable by self-hosted")
- **Review:** RR-A8, RR-P10
- **Spike:** SP-20 (Phase 4 prep) · **Risks:** R-03, R-04, R-14

## Context

Continuous encrypted sync (§42) is one of the hardest parts of the product: conflict resolution, Git interaction, uncommitted state (§62.3/4/14). The roadmap puts Remote & Sync in Phase 4. If Phases 1–3 ignore sync entirely, Phase 4 has to change every table and every write path. If Phase 1 builds sync, the MVP slips.

## Decision drivers

1. Ship Phases 1–3 local-only without carrying sync complexity.
2. Phase 4 must not need schema rewrites or a change to every engine.
3. Offline-first is the default mode, not a degraded one (§44).
4. Git stays separate from live sync (§41): sync never commits.

## Considered options

| Option | Verdict |
|---|---|
| **Define the trait and journal seams now; implement only `LocalOnly`; add per-row `rev`/`origin_device_id`/`updated_at` now** | **Chosen** |
| Build sync in Phase 1 (e.g. CRDT via Automerge for all metadata) | Rejected. Huge scope, and CRDT is the wrong fit for task state machines that need a single authority |
| Ignore sync until Phase 4 | Rejected. Leads to the schema and write-path rewrite |

## Decision

### Trait (`orkestr-sync`)

```rust
#[async_trait::async_trait]
pub trait SyncBackend: Send + Sync + 'static {
    fn kind(&self) -> SyncBackendKind;                         // LocalOnly | Cloud | SelfHosted
    async fn status(&self) -> SyncStatus;                      // { state: Disabled|Idle|Syncing|Offline|Error, pendingOps, lastSyncedAt? }
    async fn push(&self, batch: JournalBatch) -> Result<PushAck, SyncError>;            // ack per op: Applied | Conflict{server_rev}
    async fn pull(&self, cursor: &SyncCursor, limit: u32) -> Result<PullBatch, SyncError>;
    async fn put_chunks(&self, chunks: Vec<ObjectId>) -> Result<(), SyncError>;        // content-addressed, encrypted client-side (Phase 4)
    async fn get_chunk(&self, id: &ObjectId) -> Result<bytes::Bytes, SyncError>;
}

pub struct JournalOp {
    pub op_id: OpId,                    // UUID v7
    pub entity: EntityKind,             // Project | Task | AgentInstance | MemoryItem | Decision | UiConfig | Snapshot | …
    pub entity_id: String,
    pub op: OpKind,                     // Upsert | Delete
    #[ts(type = "number")] pub base_rev: u64,
    #[ts(type = "number")] pub new_rev: u64,
    pub origin_device_id: DeviceId,
    pub at: DateTime<Utc>,
    pub payload: serde_json::Value,     // redacted entity JSON; never secrets
}
```

### `LocalOnly` (Phases 1–3)

- `kind() = LocalOnly`. `status()` returns `Disabled`. `push`/`pull`/`put_chunks` return `SyncError::Disabled` and are never called, because the sync worker does not start.
- **Journal:** **off by default** in Phases 1–3, because filling a journal nobody reads wastes disk. Instead, every syncable table carries `rev`, `updated_at` and `origin_device_id` (RR-P10), and every durable event carries entity ids (ADR-0018). When the user enables sync in Phase 4, the initial upload is a **full-state snapshot** (all rows at their current rev, plus referenced objects). After that, the journal starts. No backfill of past operations is needed.
- **Which entities sync:** the `syncable` flag is set per entity in `orkestr-domain` now (`EntityKind::is_syncable()`), so Phase 4 does not need to re-audit what may leave the device. `SecretReference` syncs only the reference (and only to self-hosted vault providers); secret values never sync. Privacy `noRemoteSync`/`localOnly` projects are excluded (§56).

### Phase 4 outline (ADR-0031 will decide)

- **Metadata:** per-entity last-writer-wins on `rev`, with conflicts surfaced for user-authored text (memory, decisions), and an authoritative-owner rule for task state (the node that runs the task owns its state machine; other devices send *commands*, not state).
- **Files:** §42 pipeline (file change → local journal → incremental snapshot → encrypted chunks → remote). Synced **as snapshots** (ADR-0021 manifests) over content-addressed chunks, **never through Git** (§41, RR-C5). Git refs/branches are synced only by explicit push.
- **Encryption:** client-side, with per-workspace keys wrapped by device keys.

## Consequences

- **Positive:** Phases 1–3 stay simple; Phase 4 starts from versioned rows and content-addressed objects; cloud can be disabled (§67).
- **Negative:** the `rev`/`origin_device_id` columns are carried by every table from day one, a small cost.
- **Negative:** the full-state initial upload in Phase 4 can be large. It uses the same dedupe as the object store.

## Implementation notes (Phase 0 foundation, 2026-10-02)

The seam exists in a minimal form: `orkestr_sync::SyncBackend { name() -> &'static str, is_remote() -> bool, push(&[JournalEntry]) -> SyncReport, pull(ProjectId) -> SyncReport }` with `JournalEntry { seq, project_id, path, object_id, ts }`, `SyncReport { pushed, pulled, bytes, conflicts }`, `SyncError { NotAllowed, Backend }`, and the real `LocalOnlyBackend` (name `local-only`, never transfers, unit-tested). It is not wired into the core, there is no journal table, and entities carry no `rev`/`origin_device_id` yet. The trait shape differs from both the sketch above and `../sync/sync-protocol.md` §11; ADR-0031 picks the final one (see `../architecture/storage-abstraction.md` S5).
