# ADR-0034 — Plugin and widget sandbox

Status: accepted — implemented (Phase 7; see "As built" below)

## Context

§48 asks for a Plugin SDK, a Widget SDK and a Dashboard Builder. Plugins run
third-party code inside the core process that holds the user's vault, worktrees
and provider sessions. They also extend what agents can do. Widgets show
third-party UI inside the desktop webview, which has the Tauri bridge.
Neither may get more power than the user grants explicitly, and both must be
stoppable at once, independently of providers (contract §6).

## Decision

1. **Packages.** A plugin is a signed `.orkp` zip: `orkestr-plugin.json`
   (`orkestr_domain::PluginManifest`: `id`, `name`, `version`, `publisher`,
   `abi = 1`, `kinds`, `entry`, `permissions[{capability, scope, reason}]`,
   `tools`, `widgets`, `deployProviders`, `agentTemplates`, `subscribes`,
   `limits{memoryMb = 64, fuelPerCall = 1e9, timeoutMs = 5000}`),
   `signature.json` (ed25519 by the publisher key over the content digest of
   every other file) and payload files (`plugin.wasm`, widget HTML).
   Extraction is safe: no `..`, absolute paths or links; at most 2 048
   entries, 32 MiB per file, 64 MiB unpacked, compression ratio ≤ 100.
   Manifest limits are capped at 512 MB memory and 1e11 fuel.
2. **Signing and trust.** `orkestr_plugins::signing` holds the publisher key
   pair (`PublisherKeyPair`), `keyId` (first 16 hex of sha256 of the public
   key), a human fingerprint, and the `TrustStore` (table
   `plugins_trusted_keys`; built-in keys from `ORKESTR_PLUGINS_TRUSTED_KEYS`,
   comma-separated base64). A package signed by a key that is not trusted,
   or unsigned, installs only with `ORKESTR_PLUGINS_DEV=1`. A tampered
   package (digest mismatch) is always refused. An update signed by a
   different key than the installed version is refused. The marketplace
   verifies with the same code (ADR-0040).
3. **Code plugins run in wasmtime 50** (no default features; `runtime`,
   `cranelift`, `wat`, `std`; no WASI, threads or component model; only in
   `crates/plugins`). One engine per core; each call runs in a fresh instance.
   Limits per call: fuel (`fuelPerCall`), an epoch deadline (`timeoutMs`; a
   ticker thread bumps the epoch every 10 ms) and `StoreLimits` (linear
   memory capped at `memoryMb`), payloads ≤ 4 MiB, ≤ 1 000 host calls. A
   trap fails the call and never the core. **Three consecutive traps
   quarantine the plugin** (state `Quarantined`, re-enable explicitly).
4. **ABI v1.** The module exports `memory`, `orkestr_alloc(len: i32) -> i32`
   and `orkestr_call(ptr: i32, len: i32) -> i64` (`ptr << 32 | len` of the
   JSON result). It imports `"orkestr" "host_call"(ptr, len) -> i64` with the
   same encoding. Call payload `{tool, input}`; results `{ok}` / `{error}`;
   host requests `{op, input}`. The Rust guest SDK is `crates/plugin-sdk`
   (`orkestr_plugin!` macro, `host::*` helpers, target
   `wasm32-unknown-unknown`); on the host target only its pure helpers
   compile, so `cargo build --workspace` stays green. Example:
   `examples/plugins/hello-tool` (not a workspace member). Tests use WAT
   fixtures, so CI needs no wasm toolchain.
5. **Host API** (`orkestr_plugins::HostApi`, ops `Log`, `HttpFetch`,
   `EmitEvent`, `KvGet`, `KvSet`, `RpcRead`, `SecretGet`). Every call is
   checked against the plugin's grants. **Grants are permission rules**: on
   `plugins.grant` each accepted request becomes a Global rule in the
   `PermissionEngine` with resource `plugin:<id>/<scope>`. `HttpFetch` may
   reach only granted `Network` scope hosts, `SecretGet` only granted vault
   refs, `RpcRead` only Read-class methods that are not `local_only`. Every
   host call is audited (actor `Actor::System`, target `plugin:<packageId>`);
   `Log` lines are kept for `plugins.logs`. A denial publishes
   `PluginCallDenied`.
6. **Install → grant → enable.** `plugins.install` (base64 bytes or path)
   returns the plugin in state `Installed` with `pendingGrants`. The UI shows
   a permission prompt and `plugins.grant` records the decisions. No code runs
   before every request is decided and the plugin is `Enabled`. Install,
   grant, enable and safe mode are `local_only` for remote callers.
7. **Stop controls.** Per plugin: `plugins.disable`. Global kill switch:
   `plugins.killAll` bumps the epoch (interrupting in-flight calls within one
   tick) and moves every plugin to `Killed`. Emergency Stop All AI calls the
   same path (`PluginService::kill(None)`). **Safe mode**
   (`plugins.setSafeMode`, persisted) loads no plugin code at boot; widget
   HTML is refused in safe mode too.
8. **Kinds.** Code: `Tool` — exposed to agents through
   `orkestr_agent_runtime::tools::external::ExternalTools` as
   `plugin_<package slug>__<tool>`; both the agent's permission check and the
   plugin grant check apply. Declarative: `AgentTemplate`, `Widget`, `Mcp`
   (config stored only; launching needs a Terminal grant).
   `DeploymentProvider` and `PlatformIntegration` are accepted in manifests,
   but their runtime paths are not wired (see Consequences).
9. **Plugin events.** `EmitEvent` does not get a dedicated event kind: it is
   published as a `Log` event with target `plugin:<packageId>` (no
   `PluginEvent` kind). Lifecycle events: `PluginInstalled`,
   `PluginStateChanged{reason}`, `PluginCallDenied`.
10. **Widgets** run in `<iframe sandbox="allow-scripts">` with an opaque
    origin (no `allow-same-origin`), so there is no `__TAURI_INTERNALS__`.
    `widgets.bundle` returns the HTML (≤ 512 KB, sha256) with the CSP
    `default-src 'none'; script-src 'unsafe-inline'; style-src
    'unsafe-inline'; img-src data:; font-src data:; connect-src 'none';
    media-src 'none'; object-src 'none'; frame-src 'none'; worker-src 'none';
    form-action 'none'; base-uri 'none'` — no network. In the browser/dev
    build the CSP is injected first in `<head>` of a `srcdoc` document. In
    the **packaged app** a `srcdoc` frame would inherit the app CSP
    (`script-src 'self'`) and block the widget's inline scripts, so the
    desktop serves widget documents through the Tauri custom URI scheme
    **`orkestr-widget`** (`http://orkestr-widget.localhost` on Windows) with
    the widget CSP as a response header and a one-time URL; the scheme is in
    `frame-src` of `tauri.conf.json` and the frame gets no Tauri
    capabilities. The only channel is `postMessage` (`@orkestr/widget-sdk`):
    `init`, `data`, `event`, `theme` and `error` go in; `query`, `subscribe`,
    `resize` and `ready` come out. The host accepts messages only from that
    iframe with origin `"null"`, limits in-flight queries to 8 and checks
    the binding allowlist (Read-class, non-`local_only`, known methods;
    events must be known kinds). Core re-checks the allowlist in
    `widgets.list` / `widgets.bundle` (invalid plugin widgets are hidden).
11. **Dashboards.** Per-project and global dashboards (`dashboards.*`, table
    `dashboards_dashboards`) on a 12-column grid: `h` 1–24, no overlap,
    ≤ 64 widgets, ≤ 16 KB config per instance, `expectedVersion` conflict
    check, `DashboardChanged`. `widgets.list` returns 13 built-ins
    (`builtin:<id>`: the Mission Control tiles + `builtin:note`, rendered
    natively) and the widgets of enabled plugins (rendered in the iframe).
    Per-instance config is validated against the widget's `configSchema`
    (a JSON-schema subset; unsupported keywords are rejected).

## Fallback (not taken)

If wasmtime could not be built on the Windows host, plugins would run out of
process under `node --permission`. That isolation is weaker (process-level,
with Node's permission model). The build was verified on Windows: a cold
build of wasmtime and its dependencies took about 110 s with `-j 8`, so the
fallback is not used.

## As built (Phase 7)

- Fixtures: `cargo run -p orkestr-plugins --example make-fixtures -- <out>`
  writes `hello-tool.orkp`, `hello-widget.orkp` (widget bound to
  `projects.list`), `evil-loop` (traps at 500 ms), `evil-memory` (16 MiB cap),
  `evil-network`, `evil-tampered`, `evil-unsigned`, `evil-traversal`,
  `dev-publisher.pub` and `fixtures.json`. Start the core with
  `ORKESTR_PLUGINS_TRUSTED_KEYS=<dev-publisher.pub>` to trust them.
- Plugins installed from a marketplace record the source
  `marketplace:<source>/<id>`.
- Agent runtime: `run.rs` gained an `authorize()` helper shared by built-in
  and plugin tools.

## Consequences

- One more heavy dependency, confined to `crates/plugins`.
- Not wired in Phase 7: the `DeploymentProvider` runtime path (a plugin
  `DeployAdapter`; `DeployService::register_adapter` exists) and the
  `PlatformIntegration` path (`subscribes` events are not delivered).
- Deferred (RR-G14, `unimplementedFeature`): JS/QuickJS plugins, `Provider`
  and `Panel` kinds, plugin auto-update and widget network access.
