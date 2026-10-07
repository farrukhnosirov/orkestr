# Orkestr

**Orkestr** is an AI Development OS: a local-first desktop environment where you
steer projects through one AI Manager that plans the work and coordinates
specialised agents, models, tools and compute nodes. The product spec is in
[`docs/spec/MASTER_SPEC.md`](docs/spec/MASTER_SPEC.md). The rules every
contributor follows are in [`docs/ENGINEERING_CONTRACT.md`](docs/ENGINEERING_CONTRACT.md).

> Status: **Phase 3 — Visual OS** (Windows only). Phases 1–3 have passed
> acceptance: Phase 1, the desktop core MVP, on 2026-10-02; Phase 2,
> Multi-Agent Quality, on 2026-10-06 with mock providers; Phase 3, Visual OS,
> on 2026-10-05.
> - **Phase 1:** you can open a project, ask the AI Manager to build something,
>   approve its plan and watch agents work in parallel in their own git
>   worktrees. Dangerous actions need approval, and you also get snapshots,
>   task rollback, an emergency stop and crash recovery.
> - **Phase 2:** adds teams and hierarchy, the Author → Review → Build → Test
>   pipeline with Self-Healing, the AI Merge Manager, consensus and adversarial
>   review, the Architecture map, the Decision Log, Replay and benchmarks.
> - **Phase 3:** adds Mission Control, themes, the layout editor, floating
>   robots, the HUD, voice and detachable panels.
>
> Still not built: the MCP Hub, the Environment Manager, the Monaco LSP bridge,
> hand-off to `orkestrd`, the Browser agent, and macOS / Linux packages. The
> delivered level of every feature is in
> [`docs/roadmap/feature-traceability.md`](docs/roadmap/feature-traceability.md),
> and open items per phase are in
> [`docs/roadmap/mvp-milestones.md`](docs/roadmap/mvp-milestones.md).
> Screenshots: [`phase1`](docs/screenshots/phase1/),
> [`phase2`](docs/screenshots/phase2/), [`phase3`](docs/screenshots/phase3/).

## Architecture in one picture

```
Desktop (Tauri 2 + React)  ─┐
Web / Mobile (later)       ─┼─ @orkestr/client ── ws://127.0.0.1:<port>/rpc ──► orkestr-core
                            ┘   (typed RPC + events)                            ├─ events  (bus + SQLite log)
                                                                                ├─ storage (SQLite WAL, migrations, objects)
                                                                                ├─ platform (all OS-specific code)
                                                                                └─ engines (providers, agents, tasks, …)
```

The desktop app **embeds** the core in-process. If an `orkestrd` daemon is
already running, the app connects to it instead; discovery uses
`<data dir>/core.json`. All business data goes through the RPC protocol, which
lets the Web and Mobile clients reuse the same code.

## Prerequisites

- Rust stable (MSVC on Windows). In Git Bash, run `export PATH="$HOME/.cargo/bin:$PATH"` first.
- Node.js 20.19 or newer with npm.
- Windows: WebView2, which ships with Windows 10/11. Linux: the usual Tauri
  system packages (`webkit2gtk-4.1` and related libraries).

## Build and run

```bash
npm install                 # installs every JS workspace
npm run gen:protocol        # regenerate TS types from Rust (ts-rs)
cargo build --workspace     # all crates
cargo test --workspace      # Rust tests (unit + WebSocket integration)
npm run typecheck && npm test && npm run build

npm run tauri dev           # desktop app with hot reload (embedded core)
cargo run -p orkestr-core --bin orkestrd -- --port 0   # headless core
```

### First run (offline, no API keys)

1. `npm run tauri dev` (or build with `cargo build -p orkestr-desktop` and run
   `target/debug/orkestr-desktop` while `npm run dev -w @orkestr/desktop`
   serves the UI on port 1420).
2. The onboarding wizard opens. Under **AI providers** / **Manager model** pick
   **Mock (offline demo)** — a scripted, clearly labelled provider that needs
   no network — or connect a real provider (API key, Ollama, or the official
   Claude Code / Codex CLI with your own login).
3. In **First project**, import a folder (a git repository gives every agent
   its own worktree).
4. In the **Manager** panel, ask e.g. *"Create a hello module with a unit
   test"*, review the plan card and press **Approve**. Agents, tasks, the
   task graph, terminal output and worktrees appear in their panels; dangerous
   commands wait for you in the title bar's Approvals. **Stop** in the title
   bar stops all AI activity.

Data lives in the OS data directory. To keep everything in one folder (tests,
portable setups), set `ORKESTR_HOME=/path`. To develop the UI in a plain
browser, start `orkestrd` and open
`http://localhost:1420/?core=<url from core.json>&token=<contents of core.token>`.

## End-to-end smoke test (Phase 1)

`scripts/smoke/phase1-smoke.mjs` launches the debug desktop app with a
throw-away `ORKESTR_HOME`, reads `core.json`/`core.token`, and drives every
Phase 1 area over the real WebSocket: projects, files, Git status and agent
worktrees, snapshots with a task rollback, the task DAG, a PTY terminal, a
gated dangerous `terminal.run` with an approval round-trip, providers (Mock),
memory, cost, settings, nodes and the emergency stop. It never touches your
real data directory or the OS keyring (secret backend `vault-file`).

```bash
cargo build -p orkestr-desktop               # builds target/debug/orkestr-desktop(.exe)
node scripts/smoke/phase1-smoke.mjs          # desktop app (a window opens for ~30 s)
node scripts/smoke/phase1-smoke.mjs --headless   # same checks against target/debug/orkestrd
```

It needs Node 22+ (global `WebSocket`) and `git` on `PATH`, prints one
`PASS`/`FAIL` line per check and exits non-zero on failure.

## Phase 1 acceptance

`scripts/smoke/phase1-acceptance.mjs` drives the whole Manager flow against
the real app with the Mock provider: setup → chat → plan → approve → a
parallel run in per-agent worktrees (checked with `git worktree list`) → a
dangerous command that waits for `approvals.resolve` → snapshots and a task
rollback that keeps an unrelated user edit (§63 J) → `emergency.stopAll` on a
live run → a hard kill mid-run and a restart that marks in-flight work
`Interrupted`.

```bash
cargo build -p orkestr-desktop
node scripts/smoke/phase1-acceptance.mjs             # desktop app
node scripts/smoke/phase1-acceptance.mjs --headless  # against orkestrd
```

`scripts/smoke/phase1-screenshots.mjs` (Windows) seeds a demo project, drives
the UI through the WebView2 DevTools protocol and saves the main views to
`docs/screenshots/phase1/` with any console errors in `manifest.json`. Start
`npm run dev -w @orkestr/desktop` first; `--window` captures the native window
with PowerShell `CopyFromScreen` instead (that also captures anything on top
of the window, such as notifications from other apps).

## Phase 2 and Phase 3 acceptance

`scripts/smoke/phase2-acceptance.mjs` runs Phase 2 scenarios A-lite, B and J
with the Mock provider and a local fake OpenAI-compatible server. A-lite
covers the parallel agents, the quality pipeline, Self-Healing and the Merge
Manager; B covers the provider fallback; J covers Rollback Task. The script
also checks consensus, the architecture map, Replay, benchmarks, teams and the
AI Product Manager.

`scripts/smoke/phase3-acceptance.mjs` (Windows) runs the Visual OS checks:
scenario H (floating robots), scenario G (voice through a finished
transcript), and every theme, layout preset and performance mode. It also
checks the HUD, AI-editable UI and detaching panels into their own windows.

```bash
cargo build -p orkestr-desktop
node scripts/smoke/phase2-acceptance.mjs              # desktop app
node scripts/smoke/phase2-acceptance.mjs --headless   # against orkestrd

npm run build && cargo build -p orkestr-desktop --features custom-protocol
node scripts/smoke/phase3-acceptance.mjs              # add --no-shots to skip screenshots
```

## Layout

| Path | What |
| --- | --- |
| `crates/domain` | Entities (§51), ids, enums. Pure data, exported to TS. |
| `crates/protocol` | Frames, RPC errors, methods, events, UI config schema. |
| `crates/events` | Event bus, persisted event log, replay, correlation ids. |
| `crates/storage` | SQLite (WAL), reversible migrations, metadata store, content-addressed objects. |
| `crates/platform` | Paths, shells, credential store, autostart, power, resources. Holds every `cfg(target_os)`. |
| `crates/core` | Composition root, axum RPC server, discovery, crash recovery, `orkestrd`. |
| `crates/security` | Permission engine, dangerous-action guard, path confinement, approvals, secret vault, audit log, emergency controls. |
| `crates/providers` | Provider hub: adapters (OpenAI/compatible, Anthropic, Gemini, DeepSeek, Ollama, Claude Code CLI, Codex CLI, Mock), registry, router, fallback, pricing. |
| `crates/task-engine` | Task state machine, DAG, priority queue, resource-aware scheduler (pure), persistence, recovery. |
| `crates/{vcs,snapshots}` | Git via the `git` CLI (status/diff/branches/worktrees/merge) and content snapshots with safe workspace/task rollback. |
| `crates/terminal` | PTY sessions and gated command execution (security gate wired in `crates/core/src/gates.rs`). |
| `crates/files` | Confined file service, search/find, trash, debounced watcher. |
| `crates/{memory,context,cost}` | Memory/decisions/knowledge (FTS5), context optimizer, usage + budgets. |
| `crates/node` | Node inventory and live resource sampling. |
| `crates/{agent-runtime,orchestrator,sync}` | Engine crates whose implementations arrive in later waves/phases. |
| `packages/protocol` | `@orkestr/protocol`: generated TS types and helpers. |
| `packages/client` | `@orkestr/client`: typed WebSocket client with reconnect and replay. |
| `packages/ui` | `@orkestr/ui`: `--ok-*` tokens, theme engine, primitives. |
| `apps/desktop` | Tauri 2 shell and React UI: title bar, project rail, docking workspace. |
| `docs/` | Spec, contract, ADRs, spike reports. |
