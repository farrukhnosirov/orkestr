# ADR-0018: Event sourcing light: state tables + an append-only event log for replay and subscriptions

- **Status:** Accepted
- **Date:** 2026-10-02
- **Phase:** 1 (log, bus, resume), 2 (Execution Replay UI, Decision Log), 4 (sync journal feed)
- **Spec:** §1.14, §21 (Replay), §49, §50, §57, §63 (A, H), §65 #6 (event model)
- **Review:** RR-A5, RR-A11, RR-G2, CA-3
- **Spike:** SP-14 · **Risk:** R-16

## Context

§49 asks for typed events consumed by the UI, notifications, automation, audit and plugins. §21 asks for Replay: user request → Manager plan → agent creation → delegation → tool events → files → tests → reviews → approvals → merge. Clients must resume after a reconnect (RR-P3). Hidden chain-of-thought is never stored (§1.14).

## Decision drivers

1. Current-state queries stay simple and fast (normal tables).
2. History is complete enough for replay and audit, but bounded in size.
3. A state change and its event are atomic: no "event published, state not saved" and no "state saved, event lost".
4. High-rate streams (agent tokens, terminal bytes) do not bloat the DB.
5. Persistence sits behind a trait, so the dependency rule holds (CA-3).

## Considered options

| Option | Verdict |
|---|---|
| **State tables as the source of truth + a transactional outbox-style event log (same SQLite transaction) + an in-memory broadcast bus** | **Chosen** ("light") |
| Full event sourcing (state = fold over events) | Rejected. Projections and snapshots are complex, schema evolution is painful, and it brings no product value over the light approach |
| Bus only, no persistence | Rejected. No replay, no resume, no audit |
| External broker (NATS, Redis) | Rejected. Local-first, zero-ops (§44) |

## Decision

### Envelope (contract §4)

```rust
#[derive(Clone, Serialize, Deserialize, TS)] #[serde(rename_all = "camelCase")] #[ts(export)]
pub struct Event {
    #[ts(type = "number")] pub seq: u64,          // global, monotonic, not dense (RR-A11)
    pub id: EventId,                               // UUID v7
    pub ts: DateTime<Utc>,
    #[serde(flatten)] pub kind: EventKind,         // tag = "kind", content = "payload"
    #[serde(skip_serializing_if = "Option::is_none")] pub project_id: Option<ProjectId>,
    #[serde(skip_serializing_if = "Option::is_none")] pub task_id: Option<TaskId>,
    #[serde(skip_serializing_if = "Option::is_none")] pub agent_id: Option<AgentId>,
    #[serde(skip_serializing_if = "Option::is_none")] pub node_id: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")] pub correlation_id: Option<CorrelationId>,
}
```

### Durability classes

| Class | Kinds | Persisted | Replay via `sinceSeq` |
|---|---|---|---|
| **Durable** | Everything not listed below (TaskCreated, TaskStatusChanged, AgentSpawned, ToolCallStarted/Finished, FileChanged, Build*, TestRunFinished, ReviewFinished, Approval*, Provider*, BudgetThreshold, SnapshotCreated, RollbackPerformed, DeploymentFinished, MemoryUpdated, DecisionRecorded, ManagerMessage, EmergencyStop, Notification, NodeOffline, SyncCompleted, TerminalExited, ProjectCreated/Updated/Deleted, AgentStarted, AgentStateChanged, AgentStopped, TaskUpdated, `Log` at `warn`+) | yes | yes |
| **Ephemeral** | `AgentMessage` (streamed deltas), `TerminalOutput`, `NodeStatus` (periodic resources), `Log` below `warn` | no | no |

`EventKind::durability()` is a `const fn` match, so a new kind cannot be added without classifying it. Final agent messages are stored in `executions`, not as events (RR-A5).

### Ports (CA-3)

```rust
// orkestr-events
pub trait EventLogStore: Send + Sync + 'static {
    /// Called inside the caller's write transaction (storage provides the Tx type via an associated adapter).
    fn append(&self, tx: &dyn EventTx, ev: &NewEvent) -> Result<Event, EventError>;
    fn read_since(&self, since_seq: u64, filter: &TopicFilter, limit: usize) -> Result<Vec<Event>, EventError>;
    fn query(&self, q: &EventQuery) -> Result<Page<Event>, EventError>; // by correlationId/project/task/kind/time — Replay §21
    fn last_seq(&self) -> Result<u64, EventError>;
}
pub struct EventBus { /* tokio::sync::broadcast::Sender<Arc<Event>> (capacity 8192) + seq allocator (AtomicU64 seeded from last_seq) */ }
impl EventBus {
    pub fn publish_ephemeral(&self, ev: NewEvent) -> Arc<Event>;
    /// Publish durable events AFTER the transaction that appended them commits.
    pub fn publish_committed(&self, evs: Vec<Event>);
    pub fn subscribe(&self) -> broadcast::Receiver<Arc<Event>>;
}
// orkestr-storage implements EventLogStore as SqliteEventLog:
// events(seq INTEGER PRIMARY KEY, id TEXT UNIQUE, ts TEXT, kind TEXT, project_id, task_id, agent_id, node_id, correlation_id, payload TEXT /* JSON, redacted */)
// indexes: (project_id, seq), (task_id, seq), (correlation_id), (kind, ts)
```

- **Write path:**
  1. An engine opens `Db::write`.
  2. It mutates state, calls `append` for each event, and commits.
  3. It hands the returned events to `publish_committed`.
  If the process crashes after the commit and before the publish, nothing is lost: subscribers resume from the log through `sinceSeq`.
- **Seq allocation:** durable events use the SQLite `INTEGER PRIMARY KEY` (monotonic under the single writer). Ephemeral events take `max(lastDurable, lastEphemeral) + 1` from the same atomic counter. The writer sets the counter after each commit so ordering stays monotonic, and gaps are acceptable (RR-A11).
- **Correlation:**
  - Every user request (RPC `req`) starts a `CorrelationId` (or takes one from `params.correlationId`).
  - It is carried through `tracing` spans, tasks spawned for it, agent tool calls and provider calls.
  - Replay (§21) is `events.query {correlationId}` (plus child correlations linked through `TaskCreated.parentCorrelationId`), ordered by seq.
- **Redaction:** payload strings go through the `Redactor` before `append` (ADR-0011).
- **Retention (RR-G13):** durable events are kept for 180 days, except `Approval*`, `DecisionRecorded`, `EmergencyStop`, `RollbackPerformed`, `SnapshotCreated` and the audit-related kinds, which are kept forever. A daily compaction job deletes old rows in batches.
- **Lagged subscribers:** the broadcast receiver reports `Lagged(n)`, so the RPC connection sends the `lagged` frame and the client resumes with `sinceSeq` (ADR-0005).

## Consequences

- **Positive:** atomic state and event writes; resume and replay for free; the hot streams do not touch disk; the dependency direction is preserved.
- **Negative:** two representations (state plus events) can diverge if an engine forgets to append. Mitigation: engine repository methods return `(T, Vec<NewEvent>)`, and a lint test asserts that every state-mutating repository method emits at least one event kind.
- **Negative:** replay shows *what happened*, but it cannot reconstruct past state exactly. Snapshots (ADR-0021) cover file state. This is acceptable for §21.
- **Phase 4:** the sync journal (ADR-0019) is fed from durable events plus entity revs, not from a separate mechanism.

## Implementation notes (Phase 0 foundation, 2026-10-02)

Status and errata only; the decision above is unchanged. Details: `../architecture/event-model.md` §0.

- **No transactional outbox yet.** `EventBus::publish` assigns the seq under a mutex, appends non-ephemeral events through `EventLog::append` (its own statement), then broadcasts. State writes happen before, separately, in the RPC handler. Drivers 3 and the "write path" above are therefore **not met yet** (planned Phase 1).
- **Ports as built:** the trait is `EventLog { append, last_seq, range, prune_before }` and `SqliteEventLog` lives in **`orkestr-events`** (on `orkestr_storage::Db`), so `events → storage` (CA-3 placement reversed; see ADR-0001 notes). There is no `EventTx`, `publish_ephemeral`/`publish_committed` split, `query`, or `read_since`.
- **Envelope:** scope ids are always serialized (no `skip_serializing_if`).
- **Durability as built:** only `TerminalOutput`, `AgentMessage`, `ManagerMessage` are ephemeral (`EventKind::is_ephemeral`); `NodeStatus` and all `Log` levels are persisted. No `durability()` / retention classes, no retention job.
- **Seq allocation:** a single counter seeded from the last persisted seq; ephemeral seqs are not persisted, so they can be re-issued after a restart (no ceiling yet).
- **Correlation:** each RPC request gets a fresh `CorrelationId`, installed as task-local ambient id; `params.correlationId` is not adopted.
- **Lagged subscribers** get a local `Log` event with `seq: 0`, not a `lagged` frame; replay is `events.replay`.
