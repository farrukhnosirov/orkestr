# Orkestr documentation

Orkestr is the codename of the AI Development OS specified in
[`spec/MASTER_SPEC.md`](spec/MASTER_SPEC.md) (referenced everywhere as §N).
These two documents are binding for all others:

| Document | Role |
|---|---|
| [`spec/MASTER_SPEC.md`](spec/MASTER_SPEC.md) | Product specification (§1–§71) |
| [`ENGINEERING_CONTRACT.md`](ENGINEERING_CONTRACT.md) | Binding engineering decisions: layout, runtime model, wire protocol, task states, security invariants, provider contract, frontend rules, parallel-work rules |

**How to read the design documents.** Phase 0 produced the architecture
documents and the foundation code at the same time. A doc–code consistency
pass on 2026-10-02 checked every document against `crates/`, `apps/` and
`packages/`:

- The documents that describe built code start with an **"As built" section
  (§0)**. That section describes the code exactly. The rest of the document is
  the Phase 1+ target and marks unbuilt parts **(planned)**.
- Every ADR ends with **"Implementation notes (Phase 0 foundation)"**. These
  notes add errata and status only; the decisions are unchanged.
- The forward-looking specs (providers, agents, permissions, sync, nodes,
  remote access, plugins) say at the top what exists in code today.

## §65 architecture deliverables (all 22)

| # | Deliverable | Document | As-built section |
|---|---|---|---|
| 1 | System Context Diagram | [`architecture/system-context.md`](architecture/system-context.md) | — (context level) |
| 2 | Container Diagram | [`architecture/containers.md`](architecture/containers.md) | startup/discovery corrected to code |
| 3 | Component Diagram | [`architecture/components.md`](architecture/components.md) | §1.1 graph, "As built" per crate |
| 4 | Monorepo proposal | [`architecture/monorepo.md`](architecture/monorepo.md) | §1, §3 |
| 5 | Data Model | [`architecture/data-model.md`](architecture/data-model.md) | §0 |
| 6 | Event Model | [`architecture/event-model.md`](architecture/event-model.md) | §0 |
| 7 | Provider Adapter spec | [`providers/provider-adapter-spec.md`](providers/provider-adapter-spec.md) | §0 |
| 8 | Agent Runtime spec | [`agents/agent-runtime-spec.md`](agents/agent-runtime-spec.md) | §0 |
| 9 | Task state machine | [`architecture/task-state-machine.md`](architecture/task-state-machine.md) | §0 |
| 10 | Permission model | [`security/permission-model.md`](security/permission-model.md) | §0 |
| 11 | Threat model | [`security/threat-model.md`](security/threat-model.md) | §5 SR status table |
| 12 | Sync protocol | [`sync/sync-protocol.md`](sync/sync-protocol.md) | §11 |
| 13 | Git/worktree strategy | [`architecture/git-worktree-strategy.md`](architecture/git-worktree-strategy.md) | header |
| 14 | Node protocol | [`architecture/node-protocol.md`](architecture/node-protocol.md) | phase split |
| 15 | Remote-access architecture | [`architecture/remote-access.md`](architecture/remote-access.md) | header |
| 16 | Storage abstraction | [`architecture/storage-abstraction.md`](architecture/storage-abstraction.md) | §0 |
| 17 | Platform abstraction | [`architecture/platform-abstraction.md`](architecture/platform-abstraction.md) (+ [`platform/`](platform/README.md)) | §0 |
| 18 | Plugin architecture | [`architecture/plugin-architecture.md`](architecture/plugin-architecture.md) | header |
| 19 | Desktop/Web/Mobile shared protocol | [`api/protocol.md`](api/protocol.md) | §0 (normative for v1) |
| 20 | ADR list | [`adr/README.md`](adr/README.md) (ADR-0001…0022) | "Impl." column + per-ADR notes |
| 21 | Technical spike list | [`spikes/README.md`](spikes/README.md) | statuses + "What the foundation verified" |
| 22 | MVP milestones | [`roadmap/mvp-milestones.md`](roadmap/mvp-milestones.md) | M0.x status lines; Phase 1 wave A status table |

## All documents by folder

### Specification and review

- [`spec/MASTER_SPEC.md`](spec/MASTER_SPEC.md): the product specification.
- [`ENGINEERING_CONTRACT.md`](ENGINEERING_CONTRACT.md): the binding contract.
- [`requirements-review.md`](requirements-review.md): Phase 0 requirements review (contradictions, gaps, protocol coupling, contract ambiguities CA-1…CA-14). §5.1 records what the foundation did with each CA.
- [`risks.md`](risks.md): the risk register (§62 risks and review findings).

### `architecture/`

- [`system-context.md`](architecture/system-context.md): C4 level 1.
- [`containers.md`](architecture/containers.md): C4 level 2, runtime startup and discovery, failure behaviour.
- [`components.md`](architecture/components.md): C4 level 3, the crate graph, and the as-built and target surface of every crate and package.
- [`monorepo.md`](architecture/monorepo.md): layout rationale, dependency rules, CI and target matrix.
- [`data-model.md`](architecture/data-model.md): §51 entities, the target tables and the 3 tables built so far.
- [`event-model.md`](architecture/event-model.md): the event envelope, kinds and payloads, topics, persistence, replay.
- [`task-state-machine.md`](architecture/task-state-machine.md): task states and transitions, DAG, scheduler, crash recovery.
- [`storage-abstraction.md`](architecture/storage-abstraction.md): MetadataStore, ObjectStore, GitProvider, SecretStore, SyncBackend, storage modes.
- [`platform-abstraction.md`](architecture/platform-abstraction.md): `PlatformAdapter` and per-OS behaviour.
- [`git-worktree-strategy.md`](architecture/git-worktree-strategy.md): branches, worktrees, locks, Merge Manager.
- [`node-protocol.md`](architecture/node-protocol.md): AI nodes, scheduling, leases, migration (Phase 6).
- [`remote-access.md`](architecture/remote-access.md): gateway, device auth, E2E, MethodPolicy (Phases 4–5).
- [`plugin-architecture.md`](architecture/plugin-architecture.md): Plugin and Widget SDK, MCP Hub, Dashboard Builder.

### `api/`

- [`protocol.md`](api/protocol.md): the shared WebSocket protocol. §0 is v1 as built; §8 is the method catalogue; §8.27 reconciles method names.

### `agents/`, `providers/`, `security/`, `sync/`, `platform/`

- [`agents/agent-runtime-spec.md`](agents/agent-runtime-spec.md): templates and instances, agent state machine, tool loop, tools, budgets, handoff, cancellation.
- [`providers/provider-adapter-spec.md`](providers/provider-adapter-spec.md): `ProviderAdapter`, auth, errors, limits, CLI coding agents, routing and fallback, registry, mock.
- [`security/permission-model.md`](security/permission-model.md): capabilities, scopes, rules, approvals, Dangerous Action Guard, emergency controls.
- [`security/threat-model.md`](security/threat-model.md): STRIDE per trust boundary and the testable security requirements (SR-xx).
- [`sync/sync-protocol.md`](sync/sync-protocol.md): journal, chunks, encryption, conflicts, Git vs sync, storage modes (Phase 4).
- [`platform/README.md`](platform/README.md): where the platform material lives, plus per-OS status.

### `adr/`: Architecture Decision Records

[`adr/README.md`](adr/README.md) holds the index, the implementation status of
each ADR, the planned ADRs 0023–0035 and the decision map.

| ADR | Title |
|---|---|
| [0001](adr/0001-monorepo-layout.md) | Monorepo layout: cargo + npm workspaces |
| [0002](adr/0002-npm-workspaces-not-pnpm.md) | npm workspaces instead of pnpm |
| [0003](adr/0003-tauri-2-desktop-shell.md) | Tauri 2 as the desktop shell |
| [0004](adr/0004-embedded-core-and-orkestrd-daemon.md) | Embedded core + headless `orkestrd` daemon |
| [0005](adr/0005-websocket-rpc-protocol.md) | WebSocket JSON RPC-like protocol shared by all clients |
| [0006](adr/0006-ts-rs-generated-types.md) | TS types generated from Rust with ts-rs |
| [0007](adr/0007-sqlite-and-object-store.md) | SQLite metadata + content-addressed object store |
| [0008](adr/0008-git-cli-over-libgit2.md) | Git through the `git` CLI |
| [0009](adr/0009-worktree-per-agent.md) | Branch + worktree per agent |
| [0010](adr/0010-permission-engine-and-dangerous-action-guard.md) | Permission engine + Dangerous Action Guard |
| [0011](adr/0011-secrets-keyring-and-encrypted-vault.md) | Secrets via OS keyring + encrypted fallback vault |
| [0012](adr/0012-official-only-provider-auth.md) | Official-only provider auth; CLI coding agents |
| [0013](adr/0013-dockview-docking-layout.md) | dockview for docking layout |
| [0014](adr/0014-portable-pty-terminals.md) | Terminals on portable-pty |
| [0015](adr/0015-ui-config-validated-schema.md) | UI config as a validated schema |
| [0016](adr/0016-floating-robots-transparent-windows.md) | Floating robots as transparent Tauri windows |
| [0017](adr/0017-local-wake-word.md) | Local wake-word strategy |
| [0018](adr/0018-event-sourcing-light.md) | Event sourcing light |
| [0019](adr/0019-sync-deferred-behind-syncbackend.md) | Sync deferred behind `SyncBackend` |
| [0020](adr/0020-uuid-v7-ids.md) | UUID v7 identifiers |
| [0021](adr/0021-snapshots-content-manifests.md) | Snapshots as content manifests |
| [0022](adr/0022-crash-recovery-and-interruption.md) | Crash recovery, interruption and child-process containment |

### `spikes/`

- [`spikes/README.md`](spikes/README.md): spike list SP-01…SP-22 with goals, methods, success criteria and statuses.
- [`spikes/foundation-tauri-websocket.md`](spikes/foundation-tauri-websocket.md): the only report so far. It covers the Tauri shell, the embedded core and WebSocket event streaming (the functional part of SP-01, SP-13 and SP-14).

### `roadmap/`

- [`roadmap/mvp-milestones.md`](roadmap/mvp-milestones.md): milestones M0.1–M3.6 with exit criteria, plus an outline of Phases 4–7 and the Phase 1 wave A integration status. The end-to-end smoke test is `scripts/smoke/phase1-smoke.mjs` (see the root README).
- [`roadmap/feature-traceability.md`](roadmap/feature-traceability.md): every §68 feature mapped to a phase, crates, panels and delivery level.

## What exists in code (summary, 2026-10-02)

| Area | State |
|---|---|
| `orkestr-domain`, `orkestr-protocol` (39 event kinds, 9 methods, ts-rs export) | built |
| `orkestr-events` (bus, SQLite event log, replay), `orkestr-storage` (SQLite + migrations, object store, projects) | built |
| `orkestr-platform` (paths, process, shells, keyring, autostart, power, resources; Windows/macOS/Linux branches) | built (core set) |
| `orkestr-core` + `orkestrd` (RPC server, discovery, crash detection, logging bridge) | built |
| `orkestr-task-engine` state machine; `orkestr-sync` `LocalOnlyBackend`; `orkestr-node` local node description; `orkestr-context` packing; `orkestr-orchestrator` plan validation; `orkestr-cost` budget check | pure logic built, unit-tested |
| `security`, `providers`, `agent-runtime`, `memory`, `vcs`, `terminal`, `snapshots` | skeleton traits only |
| `apps/desktop` (Tauri 2 shell, dockview workspace, 15 panels, Logs live) | built (shell) |
| `@orkestr/protocol`, `@orkestr/client`, `@orkestr/ui` | built |

Implemented RPC methods: `system.health`, `system.info`, `projects.list`,
`projects.get`, `projects.create`, `projects.update`, `projects.delete`,
`projects.open`, `events.replay` ([`api/protocol.md`](api/protocol.md) §0.4).
