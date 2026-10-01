# Spike: Tauri shell + embedded core + WebSocket event streaming (foundation)

Status: **done, green** · Date: 2026-10-02 · Phase 0, step 4 (§66) · Host: Windows 11 Pro 26200, Rust 1.99, Node 24.21

Covers the functional part of **SP-01**, **SP-13** and **SP-14** (and the
functional part of SP-12) from `README.md`; their measured success criteria
are still open — see the "What the foundation verified" table there. Wire
details of what was tested: `../api/protocol.md` §0.

## Question

Can a Tauri 2 frameless window embed the Rust core in-process, with the webview
talking to it **only** over the shared RPC protocol (`ws://127.0.0.1:<port>/rpc`)?
Does a running `orkestrd` get discovered and reused? Do events stream live to the UI?
(Risks covered: §61 "Tauri spike", "local service", "WebSocket/event streaming"; §62.13 crash recovery.)

## Setup

- `apps/desktop/src-tauri` (`orkestr-desktop`, Tauri 2.12). On `setup`, a task:
  1. reads `<data_dir>/core.json`. If it finds a live pid, a readable token and
     `GET /health` returns 200, it uses that `orkestrd` (mode `external`);
  2. otherwise it runs `orkestr_core::start(CoreConfig::default())`, which binds
     127.0.0.1 on an ephemeral port with a 256-bit token (mode `embedded`).
- The webview calls the `core_connection` command (the only custom Tauri command).
  The command waits up to 30 s for `{url, token, mode, startupMs}`. After that,
  everything goes through `@orkestr/client`.
- The window is frameless (`decorations: false`, `transparent: true`, `shadow: true`,
  min 960×600). It has a custom title bar with `data-tauri-drag-region`, and the
  capabilities allow only minimize, maximize, close and start-dragging. The CSP
  allows `ws://127.0.0.1:*`.

## Results (measured)

| Check | Result |
| --- | --- |
| `cargo build -p orkestr-desktop` (debug) | OK |
| Embedded core startup, from `start()` call to listening (debug build, fresh data dir, 3 migrations) | **552 ms** first run, **250–260 ms** on later runs |
| Process launch → `core.json` written (debug exe) | ~2.5 s, which includes WebView2 and exe startup |
| `GET /health` on the embedded core | `200 {"status":"ok","version":"0.1.0"}` |
| Webview connects, authenticates with the first-frame token and subscribes | yes (`rpc session opened` in core debug log) |
| Live event streaming to the Logs panel | yes. 3 `projects.create` calls from an external Node WS client showed up immediately as `ProjectCreated` + `Log` rows (seq #5–#10), the project rail updated, and the status bar showed `seq 10` |
| Graceful close (WM_CLOSE) | core shuts down; `core.json`, `core.token` and `session.lock` are removed |
| `orkestrd` already running → desktop launched | desktop logs `connecting to running orkestrd` and does not start a second core |
| `orkestrd` killed with `taskkill /F`, then restarted | stale `core.json` is ignored because the pid is dead. Log: `previous core session ended uncleanly; running recovery`. Recovery hooks run (none registered yet) and a `Log` event records it |
| Second core on the same data dir | refused (`AlreadyRunning`), covered by an integration test |
| Unauthorized WS | wrong first-frame token → `res{error:unauthorized}` + close 4401; wrong `Authorization` header → HTTP 401 before upgrade (integration tests) |

## Findings / issues

1. **dockview 8 split its React bindings** into `dockview-react`. The `dockview`
   package is now the framework-agnostic core. Both are installed. Theming uses a
   `DockviewTheme` object; `className` must be a single class
   (`dockview-theme-nebula`), which re-declares the structural `--dv-*` variables
   from the built-in dark theme.
2. **React 18 constraint:** `@react-three/fiber@9` and `drei@10` need React 19, so
   fiber 8.18 and drei 9.122 are pinned, with `three ~0.173` to match drei 9.
3. **Debug builds load `devUrl`.** A plain `cargo build` exe expects the Vite dev
   server on :1420. The crate has a `custom-protocol` feature that serves `../dist`
   (`tauri build` turns it on automatically). It was used for the visual check.
4. **Ephemeral events and seq.** `TerminalOutput`/`AgentMessage`/`ManagerMessage`
   are broadcast but not persisted, so after a restart the server seq can be lower
   than the last seq a client saw. `@orkestr/client` detects this
   (`serverInfo.lastSeq < lastSeq`), resets its dedupe cursor and skips replay.
5. **Windows DPI:** screenshots need a DPI-aware process. This does not affect the
   app.
6. **Token file ACL on Windows** relies on the inherited `%APPDATA%` ACL; Unix uses
   0600. An explicit DACL is a security-phase follow-up.
7. `transparent: true` works with WebView2. The app paints its own nebula
   background, so the window currently looks opaque. That is intended until
   floating/HUD windows (§30/§34) need real transparency. Floating transparent
   always-on-top windows on macOS/Linux are still unverified (risk §62.1).

## Not verified here

- macOS and Linux builds (code compiles via `cfg` branches, but this run was Windows only).
- Production bundle (`tauri build`, installers).
- Browser dev mode (`?core=…&token=…`) was exercised by unit tests only.
