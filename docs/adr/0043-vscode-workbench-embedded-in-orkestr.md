# ADR-0043 — VS Code workbench embedded in the Orkestr window

Status: accepted — implemented (Wave 8, request #1)

Relates to: ADR-0025 (VS Code extensions in the native editor, VSCodium REH
web in a separate window, the Orkestr bridge extension), ADR-0013 (detached
panel windows), ADR-0034 (frames and CSP for widgets).

## Context

The user wants **every** installed VS Code extension to run inside Orkestr's
own interface, "without problems", not in a separate window.

Two mechanisms existed (ADR-0025):

1. **Native Monaco** support for the *declarative* parts of extensions
   (themes, icon themes, TextMate grammars, snippets, language
   configuration) plus approved language servers over the LSP bridge.
   Extension *code* (activation, commands, webviews, debuggers, tree views)
   cannot run there — there is no extension host.
2. **VSCodium REH web** (`orkestr-vscode-host`): the real VS Code server with
   the real extension host, on `127.0.0.1` with a connection token, shown in
   a **separate** top-level webview window. Extensions run fully, but outside
   the Orkestr UI.

ADR-0025 rejected an `<iframe>` in the panel: the server swaps `?tkn=` for a
`SameSite=Lax` cookie (`vscode-tkn`); inside the main webview
(`http://tauri.localhost`) a frame on `http://127.0.0.1:<port>` is
**cross-site**, so the cookie is never sent with the frame's sub-requests and
web sockets (verified again: the workbench shell loads, then the management
and extension-host connections fail — "Cannot reconnect").

Rewriting an extension host for Monaco (option 1 grown into a full
`vscode` API implementation) is out of reach: the API surface is huge and
webview/debug/tree/SCM/auth APIs are what users actually need
(Claude Code, GitLens, Python, debuggers).

## Decision drivers

- All extensions work, including webview-based ones, with the real host.
- It is part of the Orkestr window: panel layout, detaching, theme, focus.
- No widening of what the privileged Orkestr UI can frame or call.
- Keep the token model (loopback + secret) and the audit trail.
- Keep the native-Monaco features and the separate window as an option.

## Considered options

1. **iframe on `127.0.0.1`** — cross-site, cookie blocked (see above).
2. **Tauri child webview** positioned over a placeholder — first-party, but
   a native webview always paints above the HTML: Orkestr menus, dialogs,
   the palette, toasts and the floating assistant would be hidden behind it;
   docking/resizing/detaching need manual geometry sync.
3. **Reverse proxy** in the core serving the workbench under the app origin —
   needs HTTP + WebSocket proxying of a large, chatty app and a second auth
   scheme; custom URI schemes cannot carry WebSockets.
4. **iframe on `http://vscode.tauri.localhost:<port>`** — chosen.

## Decision

### Same-site frame host

The panel shows the workbench in an `<iframe>` whose URL is the
`vscode.start` URL with the host rewritten from `127.0.0.1` to
`vscode.<page host>` (`features/vscode/embed.ts`):

- `vscode.tauri.localhost` is the **same site** as `tauri.localhost`
  (registrable domain `tauri.localhost`; sites ignore ports), so the
  `SameSite=Lax` token cookie is first-party for the frame.
- Chromium/WebView2 resolve every `*.localhost` name to the loopback
  interface **without DNS** (RFC 6761), so the frame reaches the same
  `127.0.0.1` server. The server does not check `Host`.
- Dev (`http://127.0.0.1:1420`) uses `127.0.0.1` directly (same site);
  a page on any other host (the remote web client) cannot embed and the
  panel offers the separate window / browser.

Verified first in headless Edge (page on `app.localhost`, frame on
`vscode.app.localhost` → full workbench; frame on `127.0.0.1` → reconnect
failure), then in the packaged app (see *Verification*).

Hardening:

- **CSP** of the main and detached windows: `frame-src` gains exactly
  `http://vscode.tauri.localhost:*` — not `127.0.0.1`, not `localhost`
  (a test pins this). Any other local web server still cannot be framed.
- The frame is cross-*origin* to Orkestr (different host and port): no DOM
  access either way, and Tauri IPC is only injected for the app origin —
  the workbench gets **no** Tauri capability.
- `sandbox` = scripts, same-origin (needed by VS Code's service workers and
  storage), forms, popups (escaping the sandbox, for external links),
  modals, downloads, pointer lock — **no `allow-top-navigation`**, so the
  workbench can never navigate the Orkestr window. `allow` delegates only
  clipboard read/write. `referrerpolicy=no-referrer` keeps the token out of
  referrers.
- The frame URL (token) lives only in memory (`useVsCodeStore.embedUrl`),
  never in persisted layout state.
- The server keeps a stable port (`state.json` `lastPort`), so the frame
  origin — and the workbench's IndexedDB (layout, user settings, trust
  decisions) — survives restarts.

### Panel

`panels/VsCode` becomes the workbench host with tabs **Workbench ·
Extensions · Commands · Log**. The frame stays mounted while another tab is
shown. Lifecycle:

- **Start on demand:** opening the panel starts (or reuses) the server for
  the active project's folder — at most once per project and mount, so a
  user *Stop* or a failed start is not retried in a loop. Switching the
  active project re-targets the frame.
- **Heartbeat / idle stop:** unchanged (`vscode.keepAlive` while mounted,
  `autoStop` after `idleStopSecs`).
- **Crash restart:** the supervisor restarts the server with the same token
  and port (ADR-0025); a new `running.startedAt` makes the panel reload the
  frame (the new process does not know the old reconnection tokens).
- **Detaching** works like every panel (ADR-0013): the detached window loads
  the same app origin and CSP. Detached panel windows may now also open the
  separate VS Code window (capability `panel-windows` +5 commands).
- `config.embed` (default `true`) switches back to the separate window;
  the toolbar always offers "Open in a separate window" and "Open in
  browser".

### Bridge extension 0.2.0

The bundled bridge (auto-installed, loopback, one-time token) gains:

- `applyTheme {theme, colors}` — writes Orkestr's colors as
  `workbench.colorCustomizations["[Default Dark Modern|Default Light Modern]"]`
  and selects that base theme **only while a built-in default theme is
  selected** (a theme extension the user picked is never replaced).
  `vscode.syncTheme {kind, colors}` validates ids (`[A-Za-z][\w.-]*`) and
  values (`#hex`, ≤ 400 entries). The panel computes the colors from the
  `--ok-color-*` tokens (glass surfaces composited over `--ok-color-bg`) and
  pushes them when the bridge connects and on every theme change
  (`config.syncTheme`, default on). Not audited (cosmetic, frequent).
- **Keyboard focus:** while the frame has focus, VS Code owns the keyboard
  (its Ctrl+P / Ctrl+Shift+P / chords work unchanged). Two bridge
  keybindings send a `signal` back over the bridge socket:
  `Ctrl+Alt+K` → Orkestr's command palette, `Ctrl+Alt+O` → focus returns to
  the panel's tab bar. The core forwards only `focusOrkestr` / `showPalette`
  as the ephemeral event `VsCodeBridgeSignal`; anything else is dropped.

### Opening files from Orkestr

`openFileInVsCode({projectId, path, line, column})` shows the panel (or the
window), starts VS Code on the project when needed and calls
`vscode.openFile` with a 90 s wait for the bridge. Wired into the Code
editor context menu ("Open in VS Code" at the cursor), the Files panel
context menu and Bug Detective's suspected files. The launcher / palette
"VS Code: …" mode and Mission Control quick actions start the embedded
workbench instead of the window when no window is connected.

### Extension management inside Orkestr

New RPC (area `vscode`, local-only policy like the rest of the area):

| Method | Does | Audit |
|---|---|---|
| `vscode.uninstallExtension {id}` | server CLI `--uninstall-extension` (or deletes a disabled one) | `vscode.uninstallExtension` |
| `vscode.installVsix {path}` | absolute `.vsix` ≤ 1 GiB → CLI `--install-extension <path>` | `vscode.installExtension` (`registry: vsix`) |
| `vscode.updateExtensions` | CLI `--update-extensions` (Open VSX) | `vscode.updateExtensions` |
| `vscode.setExtensionEnabled {id, enabled}` | see below | `vscode.setExtensionEnabled` |
| `vscode.syncTheme` | bridge `applyTheme` | — |

**Enable / disable.** The REH server has no persistent "disabled" flag for
the CLI, the web workbench keeps its own enablement in the frame's
IndexedDB, and — measured — the server **deletes every folder of its
extensions directory that `extensions.json` does not list**. So a disabled
extension is *moved*: its folder to `<root>/disabled-extensions/` and its
`extensions.json` entry to `disabled-extensions/entries.json`
(`orkestr-vscode-host::manage`). Moves happen only while the server is not
running (Windows locks loaded native modules), i.e. immediately when it is
stopped and otherwise at the next start (`restartRequired`; the UI offers
"Restart VS Code"). `relativeLocation` must be a single plain path
component. The bridge extension cannot be disabled or uninstalled.
`vscode.listExtensions` merges stashed extensions and reports `disabled` /
`restartRequired`.

The Open VSX **catalog** (search, details, "Install in VS Code", update
badges) stays in the Extensions panel (ADR-0025); the VS Code panel's
Extensions tab manages what is installed.

### Unchanged

Native-Monaco extension features (themes, icons, grammars, snippets, LSP),
the Open VSX catalog, the bridge command catalog / launcher, dangerous
command confirmation and audit, Stop All, install consent.

## Consequences

- Every extension runs inside the Orkestr window with the real extension
  host; one implementation for all extension kinds.
- Relies on WebView2/Chromium resolving `*.localhost` to loopback and on
  site computation for `.localhost` names. Both are long-standing Chromium
  behaviour; if either changed, the panel would show the reconnect error and
  the separate window remains available (and is what `embed: false` uses).
- The frame is a third-party-looking context only to the browser's eyes;
  sign-in flows of some extensions (OAuth popups) may still work better in
  the separate window — the toolbar keeps it one click away.
- Extensions keep running as the OS user (TB8); nothing about that changed.

### Workspace Trust

`config.workspaceTrust` (default **off**) — the server starts with
`--disable-workspace-trust`, so Orkestr projects (folders the user registered)
open trusted and every extension runs. Measured: in Restricted Mode most
extensions (e.g. a PDF viewer, Claude Code) stay disabled, which contradicts
"all extensions work without problems". Turning it on restores VS Code's
prompt (applied on the next server start).

## Verification

`scripts/smoke/vscode-embed-screenshots.mjs` drives the packaged
(custom-protocol) build as a second instance with a temporary ORKESTR_HOME
(the installed VSCodium server is linked in, not downloaded) over the
WebView2 DevTools protocol. Screenshots + `manifest.json` in
`docs/screenshots/vscode-embed/`. Checks (2026-10-23):

- V1 the panel shows the frame on `http://vscode.tauri.localhost:<port>`;
  the real workbench renders inside the Orkestr main window with the
  project folder; management + extension-host connections and the bridge
  connect (the frame is same-process — same site — read through an isolated
  world of the frame);
- V2 `dracula-theme.theme-dracula`, `redhat.vscode-yaml`,
  `anthropic.claude-code` installed from Open VSX through Orkestr;
- V3 YAML language-server diagnostics on a broken file; Claude Code's
  webview host (vscode-cdn.net + its service worker) runs nested in the frame
  and receives the extension's document with the theme; Dracula applies via
  VS Code's own theme picker; contributed commands listed;
- V4 Orkestr's theme reaches the workbench
  (`--vscode-sideBar-background` = `--ok-color-bg`);
- V5 Ctrl+Alt+K inside VS Code opens Orkestr's palette;
- V6 Extensions tab with icons; Disable → "Restart VS Code" → folder moved
  out, reported disabled; Enable → restart → back;
- V7 `vscode.openFile` reveals `config.yaml:3` in the frame;
- V8 the panel works detached into its own Orkestr window.

Found and fixed while verifying: theme ids changed in VSCodium 1.140
(`Dark Modern`, not `Default Dark Modern`) — the bridge resolves the id
from `vscode.theme-defaults`; `openFile` uses `vscode.open` so custom editors
stay the default editor when no line is given.

Not verified: Claude Code's own React UI inside its webview stayed empty in
the throw-away test home (no Claude login / CLI there; the webview pipeline
itself works — host, service worker, document, theme); OAuth sign-in popups
of extensions (handled by the new popup handler → system browser);
macOS/Linux.

## As built (2026-10-23)

- Rust: `crates/vscode-host/src/manage.rs` (+5 tests), `bridge.rs` signals
  (+1 test), `server.rs` trust flag (+1 test), `host.rs` management + theme +
  signal forwarding; protocol additions in `methods/vscode.rs` (+1 test),
  event `VsCodeBridgeSignal`; core handlers in `crates/core/src/rpc/vscode.rs`
  (+ `tests/vscode_rpc.rs` assertions); desktop shell: main window created in
  `setup` (`create: false`) with popup → system browser and clipboard-read
  handlers (also on detached panel windows), CSP and popup tests in
  `vscode_window.rs`, `panel-windows` capability + 5 VS Code window commands.
- Frontend: `features/vscode/embed.ts`, `ExtensionsManager.tsx`, the panel,
  store and quick-access changes, "Open in VS Code" in the Code editor
  context menu, Files context menu and Bug Detective; `embed.test.tsx`.
