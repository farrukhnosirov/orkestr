# ADR-0025 — VS Code extensions in Orkestr's own editor (declarative contributions + language servers)

Status: accepted (approach A of two; approach B — a full VS Code panel — is a separate ADR)

## Context

Users want their VS Code extensions in Orkestr. Running the VS Code extension
host (Node.js + the `vscode` API) inside Orkestr is a large, separate effort
(approach B). Most of what users notice day to day, however, is declarative:
color themes, file icon themes, syntax highlighting (TextMate grammars),
language configurations, snippets — plus language intelligence that comes
from language servers, not from extension JavaScript.

Threat model TB8 treats plugins as untrusted code and content; the plugin
architecture (WASM / process sandbox, signing) is P7 and out of scope here.

## Decision

New crate `orkestr-extensions` and RPC areas `extensions.*` / `lsp.*`:

1. **Registry: Open VSX only** (`https://open-vsx.org/api`, overridable with
   `ORKESTR_OPENVSX_URL` for self-hosted registries) and user-supplied
   `.vsix` files. The Microsoft Visual Studio Marketplace is never contacted:
   its terms of use restrict it to Microsoft products.
2. **Downloads are verified**: Open VSX publishes a SHA-256 per file; the
   `.vsix` is rejected before unpacking when it does not match. Only URLs on
   the registry origin under `/api/` are fetched (no SSRF through API data).
   Platform-specific builds (`win32-x64`, `darwin-arm64`, …) are chosen for
   the current machine, falling back to `universal`.
3. **Safe extraction**: zip-slip / absolute / drive / UNC / ADS / device-name
   paths and symbolic links are rejected, entry count, file and total sizes
   and compression ratio are capped, files are written with default
   permissions (archive executable bits are ignored), and extraction goes to
   a staging folder that is removed on any failure. Installed folders:
   `<data_dir>/extensions/<publisher>.<name>-<version>/`; registry in SQLite
   (`ext_installed`, `ext_prefs`, `lsp_trust`; migration
   `extensions_0001_registry`).
4. **Contributions are data, extension code never runs.** Supported:
   `themes` (JSONC, `include` chains, `.tmTheme` plists), `iconThemes`
   (SVG/PNG icons as `data:` URLs in `<img>`, icon fonts via the FontFace
   API from bytes), `grammars` (vscode-textmate + Oniguruma WASM in the
   webview; embedded languages and injections), `languages` (+
   `language-configuration.json`, regexes compiled from strings, invalid ones
   dropped) and `snippets`. Everything else — `commands`, `views`,
   `debuggers`, `keybindings`, `configuration`, `menus`, activation code
   (`main`/`browser`) — is reported per extension as "needs the full VS Code".
   Paths inside contributions are confined to the extension folder, and the
   core serves only enabled extensions' files of known asset types.
5. **Language servers** are spawned by the core through `orkestr-platform`
   (`stdio_command`, no console window, process-group isolation,
   `kill_tree` on stop) from built-in recipes (bundled in an installed
   extension, e.g. `rust-lang.rust-analyzer`'s `server/rust-analyzer`, or on
   `PATH`: rust-analyzer, basedpyright/pyright, typescript-language-server,
   gopls, clangd, csharp-ls/OmniSharp, vscode-langservers-extracted). The
   first start of an exact command (recipe + program + args + SHA-256 of the
   program file) creates an approval (capability `Terminal`, risk Medium);
   only after the user approved it is the fingerprint remembered. A replaced
   binary asks again; `lsp.revoke` forgets. Traffic: `lsp.send` (client →
   server) and ephemeral `LspMessage` events on the topic `lsp:<sessionId>`
   (server → client). Crashes restart with exponential backoff (5 restarts
   per unhealthy streak). Sessions stop on `lsp.stop`, project deletion,
   emergency stop (tokens from the `EmergencyController`), 30 minutes without
   client traffic, and core shutdown.
6. **Editor appearance is separate from the Orkestr UI theme**: Settings →
   Editor chooses the editor color theme and the file icon theme
   (`extensions.getPrefs/setPrefs`, persisted by the core).

## Consequences

- No extension JavaScript ever runs in Orkestr's editor; extensions that are
  mostly code (debuggers, views, commands) show as "needs the full VS Code".
- Language features depend on servers installed on the machine or bundled in
  an installed extension; Orkestr does not download servers by itself.
- Events: `ExtensionsChanged` (persisted), `ExtensionInstallProgress`,
  `LspMessage`, `LspStatus` (ephemeral). New topic prefix `lsp:`.
