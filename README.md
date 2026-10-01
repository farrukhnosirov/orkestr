# Orkestr

**Orkestr** is an AI Development OS: a local-first desktop environment where you
steer projects through one AI Manager that plans the work and coordinates
specialised agents, models, tools and compute nodes. The product spec is in
[`docs/spec/MASTER_SPEC.md`](docs/spec/MASTER_SPEC.md). The rules every
contributor follows are in [`docs/ENGINEERING_CONTRACT.md`](docs/ENGINEERING_CONTRACT.md).

> Status: **Phase 0 foundation**. The core service, wire protocol, storage,
> event bus, platform layer, TypeScript client, design system and desktop shell
> work end to end. Engines such as providers, agents and terminal are typed
> skeletons that later phases fill in.

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

Data lives in the OS data directory. To keep everything in one folder (tests,
portable setups), set `ORKESTR_HOME=/path`. To develop the UI in a plain
browser, start `orkestrd` and open
`http://localhost:1420/?core=<url from core.json>&token=<contents of core.token>`.

## Layout

| Path | What |
| --- | --- |
| `crates/domain` | Entities (§51), ids, enums. Pure data, exported to TS. |
| `crates/protocol` | Frames, RPC errors, methods, events, UI config schema. |
| `crates/events` | Event bus, persisted event log, replay, correlation ids. |
| `crates/storage` | SQLite (WAL), reversible migrations, metadata store, content-addressed objects. |
| `crates/platform` | Paths, shells, credential store, autostart, power, resources. Holds every `cfg(target_os)`. |
| `crates/core` | Composition root, axum RPC server, discovery, crash recovery, `orkestrd`. |
| `crates/task-engine` | Task state machine (implemented) and scheduler (skeleton). |
| `crates/{security,providers,agent-runtime,orchestrator,context,memory,vcs,terminal,snapshots,cost,sync,node}` | Engine crates: public traits fixed, implementations arrive in later phases. |
| `packages/protocol` | `@orkestr/protocol`: generated TS types and helpers. |
| `packages/client` | `@orkestr/client`: typed WebSocket client with reconnect and replay. |
| `packages/ui` | `@orkestr/ui`: `--ok-*` tokens, theme engine, primitives. |
| `apps/desktop` | Tauri 2 shell and React UI: title bar, project rail, docking workspace. |
| `docs/` | Spec, contract, ADRs, spike reports. |
