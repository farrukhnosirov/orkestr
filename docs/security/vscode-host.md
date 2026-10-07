# Embedded VS Code (VSCodium REH web) — design and security notes

Crate: `crates/vscode-host` (`orkestr-vscode-host`); RPC: `vscode.*`
(`crates/core/src/rpc/vscode.rs`); shell window: `apps/desktop/src-tauri/src/vscode_window.rs`;
UI: `apps/desktop/src/panels/VsCode`, `apps/desktop/src/features/vscode`.

## What runs

* The open-source **VSCodium "remote extension host — web"** server
  (`vscodium-reh-web-<platform>-<arch>-<version>.tar.gz`, MIT) from
  <https://github.com/VSCodium/vscodium/releases>. Its `product.json` points the
  extension gallery at **Open VSX**; the Microsoft Visual Studio Marketplace is
  never used (its terms forbid non-Microsoft products). A package whose
  `product.json` names the Microsoft Marketplace is rejected.
* Downloaded **on demand** after an explicit consent dialog (version, size,
  platform, source, license, registry, risk). `vscode.install` without
  `accept: true` is refused by the core.
* Integrity: release metadata from the GitHub API; download URLs must start with
  `https://github.com/VSCodium/vscodium/releases/download/`; the archive's
  sha256 (computed while streaming) must equal the published `.sha256` file;
  extraction goes to `<version>.partial` and is renamed only after success;
  entries with absolute paths, drive prefixes or `..` abort the install; links
  and special entries are skipped.
* Layout: `<data_dir>/vscode-server/{<version>,user-data,extensions,server-data,downloads}`
  plus `installed.json`, `config.json`, `state.json`, `connection-token`.
* Extensions from Orkestr's UI (`vscode.listExtensions`,
  `vscode.installExtension {id}` — the VS Code panel's "Extensions" section,
  Ctrl+Shift+X) go through the installed server's own CLI
  (`server-main.js --install-extension <id> --force` with Orkestr's
  `--extensions-dir`/`--user-data-dir`/`--server-data-dir`), i.e. from Open
  VSX only. The id is validated strictly (`publisher.name[@version]`, ASCII
  alphanumerics, `-`, `_`), so it can never be read as a CLI flag, URL or local
  `.vsix` path; every install is written to the audit log. The CLI runs with
  the same scrubbed environment as the server (no `VSCODE_*`, `ELECTRON_*`,
  `NODE_OPTIONS`) and a 10-minute timeout.

## Process and network

* Started with the bundled Node.js: `--host 127.0.0.1`, a random 256-bit token
  through `--connection-token-file` (user-only ACL, never on the command line),
  `--telemetry-level off`; `--without-connection-token` is never used. Parent
  `VSCODE_*`, `ELECTRON_*` and `NODE_OPTIONS` variables are not inherited.
* One server instance; any project folder is opened through the URL
  (`?folder=`). The port is remembered so the UI origin (and VS Code's browser
  state) stays stable; the token stays the same across crash restarts of one
  start, so an open window reconnects.
* Supervision: health check `GET /version` every 15 s (3 failures = hung →
  restart), restart after crashes with exponential back-off (1 s … 30 s, at most
  5 restarts in 10 min, then `error`), stop on `vscode.stop`,
  `emergency.stopAll`, core shutdown, and (default on) after
  `idleStopSecs` (120 s) without a VS Code panel/window (`vscode.keepAlive`).
  The server pid is recorded; a server left behind by a crashed Orkestr session
  is killed on the next start, only if the pid still runs an executable from
  `<data_dir>/vscode-server`.
* Server output goes to the Orkestr log (target `orkestr::vscode`) and
  `vscode.logs` with the token replaced by `[REDACTED]` (it is also registered
  with the global redactor). Status events never carry the token; only the
  `vscode.start` RPC result does (needed to open the UI).
* Audit log: `vscode.install`, `vscode.uninstall`, `vscode.start`.

## Why a dedicated window, not an iframe

The server exchanges `?tkn=` for a `SameSite=Lax` cookie. Inside the main
webview (`http://tauri.localhost`) an iframe on `127.0.0.1` is cross-site, so
the cookie is not sent with the frame's requests and every asset fails with
403. Allowing `frame-src http://127.0.0.1:*` in the main window CSP would also
let any local web server be framed into the privileged Orkestr UI. The VS Code
UI therefore opens in its own top-level webview window (label `vscode`):

* **no Tauri capability** matches it → the page cannot call any command;
* only `http://127.0.0.1:<port>` URLs are accepted; navigation is pinned to the
  server origin; other `http(s)` links open in the system browser; `window.open`
  never creates an app window;
* clipboard read is allowed (paste); camera, microphone, location etc. are denied;
* the main window CSP is unchanged.

Verified in WebView2: Ctrl+P (Quick Open), Ctrl+Shift+P (Command Palette),
Ctrl+Shift+X (Extensions) work inside the window.

## Residual risks (accepted, shown to the user)

* **Extensions are untrusted code running as the OS user**, outside the
  permission engine, approvals and the dangerous-action guard (threat model
  TB8). They can read/write any file and use the network. Mitigations: consent,
  VS Code's own workspace trust ("Restricted Mode") and publisher-trust prompts,
  auto-stop when unused, Stop All AI.
* Other local processes of the same user can read the token file (same-user
  malware is out of scope, R-01). Other OS users cannot (ACL).
* VS Code web loads extension **webviews** (extension pages, Markdown preview)
  from `https://*.vscode-cdn.net` (VSCodium's default `webviewContentExternalBaseUrlTemplate`);
  that is a network request to a Microsoft CDN, not to the Marketplace.
* Windows on ARM runs the x64 build under emulation (no `win32-arm64` REH-web
  package exists).
