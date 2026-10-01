# ADR-0020: UUID v7 identifiers with typed newtypes

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1
- **Spec:** §10 (Task ID), §30 (robot ↔ Agent ID), §38–39 (nodes, migration), §42 (offline sync), §50 (correlation IDs), §51
- **Related:** ADR-0006, ADR-0018, ADR-0019

## Context

Entities are created on many devices and nodes, often offline (§44), and are synced later (Phase 4) or migrated between nodes (Phase 6). IDs must be collision-free without coordination, sortable for indexes and logs, and type-safe in Rust so a `TaskId` cannot be passed where an `AgentId` is expected.

## Decision drivers

1. Generated without coordination (offline, multi-node).
2. Time-ordered, for B-tree locality in SQLite and for readable logs.
3. Type safety across crates and in TS.
4. Interoperability: standard text form, works in Postgres `uuid` (Phase 4).

## Considered options

| Option | Verdict |
|---|---|
| **UUID v7 (RFC 9562), `uuid` crate feature `v7`, string on the wire** | **Chosen** |
| UUID v4 | Random order hurts index locality and gives no time ordering |
| ULID | Sortable, but non-standard for Postgres `uuid` and needs another crate. v7 gives the same benefits in UUID form |
| Snowflake / integer autoincrement | Needs coordination; collisions across devices |
| Prefixed ids (`task_01H…`, Stripe-style) | Nice to read, but a custom format. Type safety comes from newtypes instead, and the UI adds a prefix for display only |

## Decision

```rust
// orkestr-domain::ids
macro_rules! id_type {
    ($name:ident) => {
        #[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[cfg_attr(feature = "ts", derive(ts_rs::TS), ts(export, type = "string"))]
        #[serde(transparent)]
        pub struct $name(uuid::Uuid);
        impl $name {
            pub fn new() -> Self { Self(uuid::Uuid::now_v7()) }
            pub fn short(&self) -> String { let s = self.0.simple().to_string(); s[s.len()-8..].to_string() } // last 8 hex = random bits; display/branch names (ADR-0009)
            pub fn as_uuid(&self) -> &uuid::Uuid { &self.0 }
        }
        impl std::str::FromStr for $name { type Err = IdParseError; fn from_str(s: &str) -> Result<Self, Self::Err> { /* parse + require version 7 */ } }
        impl std::fmt::Display for $name { /* hyphenated lowercase */ }
    };
}
id_type!(ProjectId); id_type!(TaskId); id_type!(AgentId); id_type!(AgentTemplateId); id_type!(NodeId); id_type!(DeviceId);
id_type!(EventId); id_type!(ApprovalId); id_type!(SnapshotId); id_type!(TerminalId); id_type!(CorrelationId); …
// ProviderId and ObjectId are NOT UUIDs (see exceptions below).
```

- **Wire and DB format:** lowercase hyphenated text (`018f3c2a-7b1e-7c4d-9a2b-3f6e8d1c0a55`), stored as `TEXT` in SQLite. Lexical order matches time order for v7.
- **Parsing:** `FromStr` accepts only version 7 for new entities. A legacy/imported reader that accepts any version is available for import tools only.
- **Exceptions (not UUIDs):**
  - `ProviderId` is a stable slug (`openai`, `anthropic`, `claude_code`, `ollama`, `custom:<slug>`), because providers are configured, not created per device. This deviates from contract §3 ("IDs: UUID v7"); see review CA-14.
  - `ObjectId` is a content hash (`b3:<hex>`, ADR-0007).
- **Monotonicity:** `Uuid::now_v7()` from the `uuid` crate (≥ 1.9) uses a counter within the same millisecond, so ids created in one process are strictly increasing.
- **Display:** the UI shows `short()` with a type prefix (`T-3f9a1c2e` for tasks, `A-…` for agents). The full id is in tooltips and is copyable.

## Consequences

- **Positive:** coordination-free, sortable, typed, and Postgres-compatible.
- **Negative:** the v7 timestamp leaks the creation time in ids. This is acceptable: ids are not secrets, and secret-bearing tokens never use UUIDs.
- **Negative:** a 36-char TEXT key uses more space than a 16-byte BLOB. Acceptable at local scale, and it keeps SQL debugging easy.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged, but **the code does not implement its exceptions yet**:

- Ids are `define_id!` newtypes over `uuid::Uuid` with `new()` = `Uuid::now_v7()`, `from_uuid`, `as_uuid`, `parse` (accepts **any** UUID version, not only v7), `Display` (hyphenated lowercase), `FromStr`, `KIND`; they derive `TS` unconditionally and export as `string`. There is no `short()` helper yet.
- **`ProviderId` is a UUID v7**, like every other id, not a slug. **`ObjectId`** (in `orkestr-storage`, not the domain) is the bare 64-hex blake3 hash without the `b3:` prefix.
- Action required before providers land (Phase 1, M1.5): either change `ProviderId` to the slug newtype decided here (and prefix `ObjectId` if still wanted) or write a superseding ADR. Until then the docs describe the as-built UUID form (`../architecture/data-model.md` §1.1).
