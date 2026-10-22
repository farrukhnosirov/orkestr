# ADR-0033 — Node protocol & task migration

Status: accepted (Phase 6, Step 0 skeleton; work packages A–E fill in the bodies)

## Context

`docs/architecture/node-protocol.md` designs multi-node execution (MASTER_SPEC
§38–39): workers enroll with a coordinator, report resources, receive jobs
under leases, checkpoint at safe points and migrate (planned or after a lost
node). It assumed Ed25519 node keys, a node → coordinator RPC channel
(`nodes.register`, `jobs.accept`, `jobs.progress`, …), a sync chunk store for
checkpoints and HPKE secret delivery. Phase 4 already shipped a gateway with an
E2E channel (`e2e-v1`, ADR-0030) and a P-256 core identity key (CIK); Phase 6
should reuse it instead of inventing a second transport.

## Decision

1. **Transport = gateway + `e2e-v1`, coordinator as a paired device.** The
   coordinator connects to each worker core through the gateway `/v1/connect`
   exactly like a paired device, signing the handshake with its existing
   **P-256 CIK** (not Ed25519). `DeviceHandshake`/`CoreHandshake`
   (`crates/core/src/remote/e2e.rs`) are reused unchanged. The gateway accepts a
   **core bearer token** on `/v1/connect` when source and target core belong to
   the same account (rate-limited, audited); otherwise a user session is still
   required. No direct P2P; LAN mTLS is deferred.
2. **Enrollment = pairing with SAS.** The worker creates a pairing with
   `kind: "node"` and a trust level (`restricted | standard | trusted`); the
   coordinator calls `nodes.enroll {coreId, pairingId, code}`, which runs a Rust
   device-side pairing client (`crates/core/src/remote/client.rs`). Both sides
   show the SAS; the worker confirms locally and stores the coordinator as a
   paired principal with `coordinator = true` + trust level. The coordinator
   stores a CIK-signed `NodeEnrollment` (`node_enrollments` table).
3. **Pull model, no bidirectional RPC.** The coordinator calls
   `node.heartbeat` on each worker every 5 s (`ORKESTR_NODE_HEARTBEAT_MS`); the
   result carries the sample, slots and lease states, and the params renew
   leases in the same round trip. Worker job progress travels as events
   (`JobProgress`, `JobCheckpointed`) on topic `jobs` of the same E2E session.
   Worker-side methods (`node.*`, `jobs.offer`, `jobs.drain`, `node.jobCancel`)
   have `MethodPolicy.coordinatorOnly = true` and rate class `node`.
4. **Fencing both ways.** Every job message carries the lease epoch
   (`LeaseEpoch(u64)`); the worker rejects stale epochs, the coordinator drops
   progress with a stale epoch. Lease TTL 30 s (`ORKESTR_NODE_LEASE_TTL_MS`).
5. **Task engine stays the only status writer.** `Task.owner:
   Option<ExecutorRef>` (stored in the task document) records the executor;
   transitions use the C§5 table (`Running → Interrupted → Recovering →
   Running|Failed`). Unknown / non-idempotent side effects after the last
   checkpoint → `Failed(needs_user_decision)` + `ApprovalRequested`.
6. **Checkpoints as blobs.** Workspace state = `git bundle` of unpushed commits
   + tar of the dirty tree, transferred as content-addressed SHA-256 blobs in
   512 KiB chunks via `node.blobPut|blobGet|blobHas` over E2E (under the 1 MiB
   record limit), hash-verified on finalize (shared with ADR-0039). The sync
   chunk store is not used for checkpoints yet.
7. **Deterministic scheduler** (`orkestr_node::NodeScheduler`, implemented by
   `orkestr_task_engine::node_scheduler::DefaultNodeScheduler`): filter → score
   (node-protocol.md §5 weights) → tie-break (coordinator, then lowest
   `NodeId`), explanations stored on the job/task.
8. **Simulated nodes.** `ORKESTR_NODE_PROFILE=<json>` overrides os, arch, cpu,
   memory, GPUs, tools, labels, models and tool shims (`xcodebuild → node
   fake-xcodebuild.mjs`) for acceptance tests; without it detection is real.

Seams fixed in Step 0: `NodeLink` / `NodeConnector` (`crates/node/src/link.rs`),
`NodeScheduler` (`scheduling.rs`), `BlobStore` + `push_blob` (`blobs.rs`),
`JobExecutor` + `JobControl` (`exec.rs`), `load_profile` (`profile.rs`),
`LoopbackLink` (`crates/core/src/nodes/loopback.rs`, in-process worker router
for tests). Protocol types live in `orkestr_protocol::methods::{nodes, jobs,
models}`; events `NodeRegistered`, `JobAssigned`, `JobProgress`,
`JobCheckpointed`, `JobMigrated`, `ModelDistributed` are appended.

## Deferred (`unimplementedFeature`, RR-G14)

HPKE per-job secret delivery (jobs with `needs.secrets` are Infeasible on remote
nodes); critical-job preemption; `Service` jobs; direct LAN mTLS; coordinator
failover and lease reconciliation after a coordinator crash (tasks are only
marked `Interrupted`); portable Claude Code / Codex session state (handoff
only); mobile/web node UI; real macOS node (profile-simulated); sync chunk
store for checkpoints.

## Consequences

- No new listener or key type: node traffic inherits gateway blindness,
  revocation and audit from ADR-0030.
- Pull heartbeats cost one round trip per worker per interval but avoid a
  worker → coordinator RPC surface and keep authorization one-directional.
- Diverges from node-protocol.md (Ed25519, push RPCs, chunk store); the doc is
  updated "as built" by WP-E.
