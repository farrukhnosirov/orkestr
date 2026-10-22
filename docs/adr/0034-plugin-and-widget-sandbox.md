# ADR-0034 — Plugin and widget sandbox

Status: accepted — in progress (Phase 7)

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
   `limits{memoryMb = 64, fuelPerCall, timeoutMs = 5000}`), `signature.json`
   (ed25519 over the sha256 of every file) and payload files. Extraction is
   safe: no `..`, absolute paths or links, and caps on entry count and
   unpacked size. Unsigned packages install only with `ORKESTR_PLUGINS_DEV=1`.
   The trust store (`orkestr_plugins::signing`) is shared with the
   marketplace (ADR-0040).
2. **Code plugins run in wasmtime** (`wasmtime` 50, features `runtime`,
   `cranelift`, `wat`, `std`; no WASI, threads or component model). Limits
   per call: fuel (`fuelPerCall`), an epoch deadline (`timeoutMs`, ticked by
   one epoch thread per core) and `StoreLimits` (linear memory capped at
   `memoryMb`). A trap fails the call and never the core. Repeated traps
   quarantine the plugin.
3. **ABI v1.** The module exports `orkestr_alloc(len: i32) -> i32` and
   `orkestr_call(ptr: i32, len: i32) -> i64` (`ptr << 32 | len` of the JSON
   result). It imports `"orkestr" "host_call"(ptr, len) -> i64` with the same
   encoding. All payloads are JSON. The Rust guest SDK is `crates/plugin-sdk`
   (target `wasm32-unknown-unknown`). On the host target only its pure helpers
   compile, so `cargo build --workspace` stays green. Tests use WAT fixtures,
   so no wasm toolchain is needed in CI.
4. **Host API** (`orkestr_plugins::HostApi`, ops `Log`, `HttpFetch`,
   `EmitEvent`, `KvGet`, `KvSet`, `RpcRead`, `SecretGet`). Every call is
   checked against the plugin's grants. Grants are stored in the
   `PermissionEngine` under principal `plugin:<id>`. `HttpFetch` may reach
   only granted `Network` scope hosts, `SecretGet` only granted refs, and
   `RpcRead` only Read-class methods that are not local-only. Every call is
   audited. A denial publishes `PluginCallDenied`.
5. **Install → grant → enable.** `plugins.install` returns the plugin in state
   `Installed` with `pendingGrants`. The UI shows a permission prompt, and
   `plugins.grant` records the decisions. No code runs before every request
   is decided and the plugin is `Enabled`. Install, grant, enable and safe
   mode are `local_only` for remote callers.
6. **Stop controls.** Per plugin: `plugins.disable`. Global kill switch:
   `plugins.killAll` interrupts in-flight calls (epoch bump) and moves every
   plugin to `Killed`. Emergency Stop All AI calls the same path
   (`PluginService::kill(None)`). **Safe mode** (`plugins.setSafeMode`,
   persisted) loads no plugin code at boot.
7. **Kinds in Phase 7.** Code plugins: `Tool` (exposed to agents through
   `orkestr_agent_runtime::tools::external::ExternalTools`, names
   `plugin_*`, agent permission check plus plugin grant check),
   `DeploymentProvider` (a `DeployAdapter`, ADR-0041) and
   `PlatformIntegration` (an event subscriber). Declarative kinds:
   `AgentTemplate`, `Widget` and `Mcp`. MCP configs are stored only;
   launching one needs a Terminal grant.
8. **Widgets** run in `<iframe sandbox="allow-scripts">` with an opaque
   origin (no `allow-same-origin`), so there is no `__TAURI_INTERNALS__`.
   They use `srcdoc` HTML from `widgets.bundle` with an injected CSP
   (`default-src 'none'; script-src 'unsafe-inline'; style-src
   'unsafe-inline'; img-src data:`), so they have no network. Their only
   channel is `postMessage` (`@orkestr/widget-sdk`): `init`, `data`, `event`,
   `theme` and `error` go in; `query`, `subscribe`, `resize` and `ready` come
   out. The host checks the message source and the binding allowlist, and
   only Read-class, non-local-only methods are allowed. Core enforces the
   allowlist again.
9. **Dashboards.** Per-project and global dashboards on a 12-column grid
   (`dashboards.*`) hold built-in and plugin widgets. Per-instance config is
   validated against the widget's `configSchema`.

## Fallback (not taken)

If wasmtime could not be built on the Windows host, plugins would run out of
process under `node --permission`. That isolation is weaker (process-level,
with Node's permission model). The build was verified on Windows: a cold
build of wasmtime and its dependencies took about 110 s with `-j 8`, so the
fallback is not used.

## Consequences

- One more heavy dependency, confined to `crates/plugins`.
- Deferred (RR-G14, `unimplementedFeature`): JS/QuickJS plugins, `Provider`
  and `Panel` kinds, plugin auto-update and widget network access.
