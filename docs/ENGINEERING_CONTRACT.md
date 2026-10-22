# Orkestr — Engineering Contract (binding for every contributor/agent)

Orkestr is the codename of the AI Development OS described in
`docs/spec/MASTER_SPEC.md` (referenced as §N). This contract fixes the expensive
decisions so that many contributors can work in parallel without diverging.
If you must deviate, write an ADR in `docs/adr/` and say so in your report — never
silently.

## 1. Toolchain & environment (Windows-first host)

- Rust stable (1.99) MSVC. `cargo` lives in `%USERPROFILE%\.cargo\bin` — in Git
  Bash run `export PATH="$HOME/.cargo/bin:$PATH"` first.
- Node 24 + **npm workspaces** (pnpm is not installed; see ADR-0002).
- Tauri **2.x** (`tauri = "2"`, `@tauri-apps/cli@^2`, `@tauri-apps/api@^2`).
- Edition 2021 for all crates, `rust-version = "1.90"` (raised from 1.80 in Phase 1: Tauri 2.12 needs it; see ADR-0003 notes).
- Never hardcode OS paths (`C:\Users\...`). Use `orkestr-platform` path helpers.
  `ORKESTR_HOME` env var overrides the data dir (tests MUST use a temp dir).

## 2. Repository layout (§60, finalized by ADR-0001)

```
/Cargo.toml                 cargo workspace (members = crates/*, apps/desktop/src-tauri)
/package.json               npm workspaces (apps/desktop, apps/web, packages/*); apps/mobile is standalone (own package-lock, React 19 — ADR-0032)
/docs                       architecture, adr, security, sync, agents, providers, platform, api, spikes
/crates
  domain/         orkestr-domain        entities (§51), ids, enums, value objects. NO I/O.
  protocol/       orkestr-protocol      wire protocol: RPC envelope, method names, Event enum (§49), ts-rs export
  events/         orkestr-events        EventBus (tokio broadcast) + persisted event log + correlation ids
  storage/        orkestr-storage       MetadataStore (SQLite via rusqlite bundled), ObjectStore (content-addressed FS), migrations
  platform/       orkestr-platform      PlatformAdapter traits + windows/macos/linux impls, paths, shells, credential store, autostart, notifications, resources
  security/       orkestr-security      capabilities, permission engine, approval policy, dangerous-action guard, secret vault abstraction, redaction, audit
  providers/      orkestr-providers     ProviderAdapter trait, registry, router, fallback; adapters: openai, anthropic, gemini, deepseek, ollama, openai_compatible, claude_code (CLI), codex (CLI), mock
  task-engine/    orkestr-task-engine   Task state machine, DAG, priority queue, resource-aware scheduler
  agent-runtime/  orkestr-agent-runtime AgentTemplate/Instance lifecycle, tool loop, tool registry, handoff, budgets
  orchestrator/   orkestr-orchestrator  AI Manager: plan, decompose, spawn, delegate, review/test/heal loops, consensus, merge manager
  context/        orkestr-context       context optimizer (relevance, symbol extraction, token budgeting)
  memory/         orkestr-memory        project/global memory, knowledge base, decision log, FTS search
  vcs/            orkestr-vcs           Git via `git` CLI: status/diff/branches/worktrees/merge/conflicts
  terminal/       orkestr-terminal      PTY sessions (portable-pty), shell abstraction, streaming, history, approval interception
  snapshots/      orkestr-snapshots     workspace snapshots incl. uncommitted state, task/workspace rollback
  cost/           orkestr-cost          usage records, pricing, budgets, limits, benchmarks, agent analytics
  sync/           orkestr-sync          SyncBackend trait + LocalOnly backend + journal (Phase 4 extends)
  node/           orkestr-node          NodeInfo/resources (sysinfo), node registry trait (Phase 6 extends)
  core/           orkestr-core          composition root, service wiring, RPC server (axum WS+HTTP), headless `orkestrd` bin, crash recovery
/apps
  desktop/        Tauri 2 + React + TS strict + Vite (src/, src-tauri/)
/packages
  protocol/       @orkestr/protocol     TS types GENERATED from Rust via ts-rs (src/generated/*) + hand-written index
  client/         @orkestr/client       TS WebSocket RPC client + event subscriptions (shared by desktop/web/mobile)
  ui/             @orkestr/ui           design system: tokens, theme engine, primitives (Button, Panel, ...)
```
Web (`apps/web`) and Mobile (`apps/mobile`) are Phase 4/5: they reuse
`@orkestr/client` + `@orkestr/protocol` (+ `@orkestr/ui` for web). `apps/mobile` is NOT a root
workspace: it has its own `package-lock.json` (Expo needs React 19, the root pins
React 18) and consumes the shared packages from source via Metro/TS paths; run
`npm install|typecheck|test` inside `apps/mobile` (ADR-0032). Nothing in
desktop UI may talk to Tauri for business data — **all business data goes through
the RPC protocol** so Web/Mobile get it for free (§67).

Dependency direction (no cycles): `domain` ← `protocol` ← `events` ← everything
else ← `core`. Engines depend on traits from lower crates, never on `core`.
`platform` holds ALL `#[cfg(target_os)]` code (§60.6). Provider-specific logic only
inside `providers` (§60.5).

## 3. Core runtime model

- `orkestr-core` is a library + binary `orkestrd` (headless service, §12).
- The desktop app (Tauri) **embeds** the core in-process by default: on startup
  it calls `orkestr_core::start(CoreConfig)` which binds the RPC server on
  `127.0.0.1:<ephemeral>` with a random 256-bit bearer token, and exposes
  `{url, token}` to the webview via the Tauri command `core_connection`.
  If an `orkestrd` daemon is already running (discovered via
  `<data_dir>/core.json` = `{url, token_file_hint, pid}`), the desktop connects to it
  instead. Never bind 0.0.0.0. Remote access is Phase 4 via gateway (§35).
- Async runtime: tokio. Errors: `thiserror` in libs, `anyhow` only in bins/tests.
  Logging: `tracing` + `tracing-subscriber` with JSON option; secrets redacted.
- Persistence: single SQLite DB `<data_dir>/orkestr.db` (WAL) via
  `orkestr-storage`; objects in `<data_dir>/objects/` (blake3 content addressed).
  Each engine owns its tables; schema via ordered, reversible migrations
  (`up`/`down` SQL) registered in `orkestr-storage::migrations` — each crate
  contributes a migration module, `core` composes them in a fixed order.
- IDs: UUID v7 strings (`uuid` crate, feature `v7`), newtype wrappers in domain
  (`ProjectId`, `TaskId`, `AgentId`, ...), serde-transparent.
- Time: `chrono::DateTime<Utc>` serialized RFC3339.

## 4. Wire protocol (§19 deliverable: Desktop/Web/Mobile shared protocol)

Transport: WebSocket at `ws://127.0.0.1:<port>/rpc` (auth: `Authorization: Bearer`
header OR first message `{"type":"auth","token":...}` because browsers cannot set
WS headers). HTTP `GET /health` (no auth) → `{status:"ok",version}`.

Frames (JSON, `type` tagged, camelCase fields):
```json
{"type":"auth","token":"..."}
{"type":"req","id":"<client-unique>","method":"projects.list","params":{...}}
{"type":"res","id":"<same>","ok":true,"result":{...}}
{"type":"res","id":"<same>","ok":false,"error":{"code":"not_found","message":"...","data":null}}
{"type":"sub","id":"..","topics":["project:<id>","global"]}     // subscribe to events
{"type":"event","seq":123,"event":{"kind":"TaskCreated", ...}}
```
Error codes: `unauthorized, bad_request, not_found, conflict, forbidden,
approval_required, rate_limited, budget_exceeded, provider_error, internal`.

Method naming: `<area>.<verb>` lowerCamel verbs, e.g. `projects.list`,
`projects.create`, `tasks.transition`, `agents.spawn`, `terminal.write`.
Each area's handler module lives in `crates/core/src/rpc/<area>.rs` and is
registered in `crates/core/src/rpc/mod.rs` (`Router::register(area_handlers)`).
Param/result types live in `orkestr-protocol::methods::<area>` and derive
`Serialize, Deserialize, TS` with `#[serde(rename_all = "camelCase")]` and
`#[ts(export)]`. TS bindings are generated into `packages/protocol/src/generated/`
by `cargo test -p orkestr-protocol export_bindings` (ts-rs `TS_RS_EXPORT_DIR`).
`packages/protocol/src/index.ts` re-exports generated types and declares the
`Methods` map `{ "projects.list": [Params, Result], ... }` for typed calls.

Event envelope (`orkestr-protocol::Event`): `{ seq, id, ts, kind, projectId?,
taskId?, agentId?, nodeId?, correlationId?, payload }` where `kind` +
payload are a tagged enum `EventKind` (serde `tag="kind", content="payload"`).
Minimum kinds (§49 + needed): ProjectCreated, ProjectUpdated, ProjectDeleted,
TaskCreated, TaskUpdated, TaskStatusChanged, AgentSpawned, AgentStarted,
AgentStateChanged, AgentStopped, AgentMessage (streamed output delta),
ToolCallStarted, ToolCallFinished, FileChanged, BuildStarted, BuildFailed,
BuildSucceeded, TestRunFinished, ReviewFinished, ApprovalRequested,
ApprovalResolved, ProviderStatusChanged, ProviderRateLimited, ProviderFallback,
BudgetThreshold, SnapshotCreated, RollbackPerformed, TerminalOutput,
TerminalExited, NodeStatus, NodeOffline, SyncCompleted, DeploymentFinished,
MemoryUpdated, DecisionRecorded, ManagerMessage, EmergencyStop, Notification, Log.
New kinds may be added (append-only); never rename.

Topics: `global`, `project:<id>`, `task:<id>`, `agent:<id>`, `terminal:<id>`.

## 5. Task state machine (§10) — implemented ONLY in `orkestr-task-engine`

States: `Planned, Ready, Running, Waiting, Blocked, Review, Testing, Completed,
Failed, Cancelled, Interrupted, Recovering`.
Allowed transitions (anything else → `InvalidTransition` error):
```
Planned    → Ready | Blocked | Cancelled
Ready      → Running | Blocked | Cancelled | Planned
Running    → Waiting | Blocked | Review | Testing | Completed | Failed | Cancelled | Interrupted
Waiting    → Running | Blocked | Cancelled | Failed
Blocked    → Ready | Cancelled | Failed
Review     → Testing | Running | Completed | Failed | Cancelled      (Running = changes requested)
Testing    → Review | Running | Completed | Failed | Cancelled       (Running = fix loop)
Failed     → Ready | Recovering | Cancelled                         (retry)
Interrupted→ Recovering | Cancelled
Recovering → Ready | Running | Failed | Cancelled
Completed, Cancelled: terminal
```
`Waiting` = waiting for approval/user/provider; `Blocked` = unmet dependency or
policy. A task becomes `Ready` only when all dependencies are `Completed`.
On core start, every `Running/Waiting/Testing/Review` task of a crashed session
becomes `Interrupted` (crash recovery §62.13).

Agent instance states (robot states §30 derive from these): `Idle, Listening,
Thinking, Coding, Testing, Reviewing, Waiting, Error, Completed, Stopped`.

## 6. Security invariants (§23, §52, §70) — non-negotiable

- Every agent tool call goes through `orkestr-security::PermissionEngine::check(
  actor, capability, resource, context) -> Decision{Allow|Ask(reason)|Deny(reason)}`.
- Capabilities: `Read, Write, Delete, Terminal, Git, Network, Browser, DbRead,
  DbWrite, Docker, Ssh, Deploy, ComputerControl, Secrets`. Scopes: Global ⊃
  Project ⊃ Agent ⊃ Task (narrowest explicit rule wins; Deny beats Allow at same
  scope). Approval modes: `Ask, SemiAuto, FullAuto`.
- `DangerousActionGuard` classifies commands/actions (recursive delete, DROP/
  TRUNCATE, prod write/deploy, secret changes, `git push --force`, `git reset
  --hard`, `git clean -fd`, privileged OS/network changes, format/diskpart,
  registry edits, etc.). Dangerous ⇒ ALWAYS `Ask` even in FullAuto; hard-denied
  list ⇒ `Deny`. Unit-tested with many cases (PowerShell, cmd, bash forms).
- Filesystem tools are confined to the agent's worktree/project root; path
  traversal (`..`, symlinks, absolute paths outside root, UNC, `\\?\`) rejected.
- Secrets: never in SQLite plaintext, never in logs/events. Stored via
  `SecretStore` (OS keyring through `keyring` crate; fallback encrypted file
  vault with `chacha20poly1305` + key in keyring). DB stores `SecretReference` only.
- Emergency stop (`emergency.stopAll`, `emergency.stopProject`, `agents.stop`) is
  handled locally in core with cancellation tokens, independent of providers.
- RPC server: localhost only, bearer token compared in constant time.

## 7. Providers (§4)

`ProviderAdapter` (async trait, object-safe): `id()`, `descriptor()`
(capabilities/context/local/cost model), `auth_state()`, `list_models()`,
`health()`, `chat(ChatRequest, CancellationToken) -> stream of ChatEvent`
(TextDelta, ToolCall, Usage, Done, Error), optional `run_coding_agent(...)` for
CLI agents (Claude Code `claude -p --output-format stream-json`, Codex `codex
exec --json`) — official CLIs only, using the user's own authenticated CLI
session; never read their token files. Errors classify `RateLimited{retry_after}`,
`QuotaExceeded`, `Auth`, `Network`, `BadRequest`, `Server`, `Cancelled`.
`mock` provider is deterministic and clearly labelled "Mock (offline demo)".
HTTP via `reqwest` (rustls). Keys via `SecretStore` references.

## 8. Frontend rules

- React 18 + TS `strict: true`, `noUncheckedIndexedAccess: true`. Vite. No `any`
  without comment. State: `zustand`. Data: `@orkestr/client` only.
- Libraries pre-installed by foundation (do NOT run `npm install` in parallel
  work; ask the integrator): monaco-editor, @xterm/xterm, @xterm/addon-fit,
  @xyflow/react, dockview, zustand, lucide-react, motion, lottie-react,
  @rive-app/react-canvas, three, @react-three/fiber, @react-three/drei, clsx,
  i18next, react-i18next,
  vitest, @testing-library/react, jsdom.
- Panels live in `apps/desktop/src/panels/<PanelName>/index.tsx`, each default-
  exporting a component `({ projectId }: PanelProps)` and registered in
  `apps/desktop/src/panels/registry.ts`.
- Visual design uses tokens from `@orkestr/ui` (CSS variables `--ok-*`); never
  hardcode colors in panels. Frameless window + custom title bar (§28).
- UI config (themes/layouts, AI-editable §29) is a validated JSON schema
  (`orkestr-protocol::ui_config`), applied by the theme engine — never eval'd code.
- **i18n (binding):** every user-visible string (JSX text, `title`,
  `placeholder`, `aria-label`, labels, notifications, command/panel/section
  titles, tray) goes through `t()` from `apps/desktop/src/i18n` — never
  hardcoded. Every key exists in **both** `locales/en` and `locales/ru`
  (English is the key source; Russian plurals need `_one/_few/_many/_other`).
  Russian wording follows `docs/i18n/glossary.md`. Numbers, money, dates,
  sizes and durations use the i18n formatters; core errors are shown via
  their codes (`describeError`). Guard tests (`src/i18n/*.test.ts`) must
  pass. How-to: `docs/i18n/README.md`.

## 9. Parallel-work rules (for agents building this repo)

1. Only edit files inside your assigned directories. Shared files
   (`/Cargo.toml`, `/package.json`, `crates/core/src/rpc/mod.rs`,
   `apps/desktop/src/panels/registry.ts`, `packages/protocol/src/index.ts`)
   are edited by the integrator unless your assignment says otherwise.
2. Need a new Rust dependency? Prefer `[workspace.dependencies]` entries
   (`foo.workspace = true`). If missing, add it with explicit version to YOUR
   crate's Cargo.toml and list it in your report.
3. Build/test your crate with an isolated target dir to avoid lock contention:
   `CARGO_TARGET_DIR=target/agent-<yourname> cargo test -p <crate>`.
4. Every crate: `#![forbid(unsafe_code)]` unless platform FFI requires it (then
   only in `platform`, documented). `cargo clippy -p <crate> -- -D warnings` clean.
5. Tests: unit tests for logic; critical orchestration/security/state logic MUST
   have tests (§60.9). Tests use temp dirs (`tempfile`), never the real data dir.
6. Report honestly: what works, what is stubbed, what you could not verify.
   Stubs must be explicit (`unimplemented-feature` errors with clear messages),
   never fake success.
