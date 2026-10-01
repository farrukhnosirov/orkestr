# Container Diagram (C4 Level 2)

> Deliverable §65 #2. A "container" here is a separately runnable/deployable
> unit or a data store (C4 sense), not a Docker container. Contract references:
> §3 (runtime model), §4 (wire protocol), §6 (security invariants).

## 1. Phase 1–3 containers (single machine)

```mermaid
flowchart TB
    user(["👤 Developer"])

    subgraph proc["OS process: orkestr-desktop (Tauri 2)"]
        webview["<b>Desktop UI</b> [Container: WebView]<br/>React 18 + TS strict + Vite<br/>panels, Monaco, xterm.js, dockview,<br/>@orkestr/client, @orkestr/ui"]
        host["<b>Desktop Host</b> [Container: Rust, src-tauri]<br/>window/tray/hotkey/host services,<br/>Tauri command `core_connection` only"]
        core["<b>Orkestr Core</b> [Container: Rust lib orkestr-core]<br/>RPC server (axum WS+HTTP) on 127.0.0.1:&lt;ephemeral&gt;<br/>engines: orchestrator, agent-runtime, task-engine,<br/>providers, security, vcs, terminal, snapshots, ..."]
    end

    daemon["<b>orkestrd</b> [Container: Rust bin, headless]<br/>same orkestr-core, no UI (§12)<br/>autostart via platform adapter"]

    subgraph data["Data dir (ORKESTR_HOME or OS default)"]
        db[("<b>orkestr.db</b><br/>SQLite, WAL<br/>[MetadataStore]")]
        objects[("<b>objects/</b><br/>blake3 content-addressed<br/>[ObjectStore]")]
        corejson[["core.json + core.token + session.lock<br/>discovery / crash marker"]]
        logs[["logs/<br/>tracing JSON, redacted"]]
    end

    keyring[("OS credential store<br/>+ encrypted file vault<br/>[SecretStore]")]
    gitc["git CLI"]
    pty["PTY shells<br/>(ConPTY / openpty)"]
    clis["claude / codex CLIs"]
    ollama["Ollama :11434"]
    cloud["Cloud AI APIs"]

    user --> webview
    webview -- "invoke('core_connection') → {url, token}" --> host
    webview == "WebSocket JSON frames<br/>ws://127.0.0.1:port/rpc<br/>auth frame / Bearer" ==> core
    host -- "in-process: orkestr_core::start(CoreConfig)" --> core
    webview -. "if daemon running:<br/>connect to orkestrd instead" .-> daemon
    core & daemon --> db & objects & logs
    core & daemon -- "writes" --> corejson
    host -- "reads (discovery)" --> corejson
    core --> keyring & gitc & pty & clis & ollama & cloud
```

### Container responsibilities

| Container | Tech | Responsibility | Must NOT | Phase |
|---|---|---|---|---|
| **Desktop UI** | React 18, TS strict, Vite, zustand, Monaco, xterm.js, dockview, @xyflow/react, Rive/Lottie, three.js | Render panels (§14), command palette (§54), approvals, Mission Control (P3), themes from validated `ui_config` (§29) | Call Tauri for business data; hardcode colors; eval AI-generated code | P1 (visual P3) |
| **Desktop Host** (`apps/desktop/src-tauri`) | Tauri 2, Rust | Start/locate Core; expose `core_connection`; frameless windows, tray, floating robot windows (P3), global hotkeys (P3) as *host services* injected into Core | Contain business logic or `#[cfg(target_os)]` code (that is `orkestr-platform`'s job) | P1 |
| **Orkestr Core** (`crates/core` lib) | tokio, axum, rusqlite | Composition root; RPC router; event fan-out; crash recovery; emergency stop | Bind non-loopback; log secrets | P1 |
| **orkestrd** (`crates/core` bin) | same | Headless service so agents survive GUI close (§12); later the AI Node (§38) | Expose a raw terminal port (§35) | P1 (basic), P4/P6 (node) |
| **orkestr.db** | SQLite (rusqlite `bundled`), WAL | All metadata; each engine owns its tables via migrations | Store plaintext secrets | P1 |
| **objects/** | Filesystem, blake3 | Snapshot blobs, artifacts, terminal scrollback, knowledge files | Be addressed by anything but `ObjectId` | P1 |
| **SecretStore** | OS keyring via `keyring`; fallback `chacha20poly1305` file vault | API keys, tokens, env secrets | Be readable by agents directly | P1 |

## 2. Runtime: startup, discovery, connection

```mermaid
sequenceDiagram
    autonumber
    participant UI as Desktop UI (WebView)
    participant H as Desktop Host (Tauri)
    participant FS as data_dir
    participant D as orkestrd (optional)
    participant C as Core (embedded)

    H->>FS: read core.json {url, token_file_hint, pid, version, started_at}
    alt other core alive (pid alive AND token file readable AND GET /health ok)
        H->>FS: read core.token (must be this data dir's token file)
        H-->>UI: core_connection → {url, token, mode:"external", startupMs}
    else none / stale core.json
        H->>C: orkestr_core::start(CoreConfig::default())  (data dir from ORKESTR_HOME / OS, bind 127.0.0.1:0)
        C->>C: session.lock check, open DB, run migrations, crash recovery hooks (none registered in Phase 0)
        C->>FS: write core.token + core.json
        C-->>H: CoreHandle{url, token, …}
        H-->>UI: core_connection → {url, token, mode:"embedded", startupMs}
    end
    UI->>C: WS connect /rpc
    UI->>C: {"type":"auth","token":"…"}
    C-->>UI: {"type":"res","id":"auth","ok":true,"result":{"sessionId","serverVersion","protocolVersion":1,"lastSeq"}}
    UI->>C: {"type":"sub","id":"s1","topics":["global"]}
    UI->>C: {"type":"req","id":"1","method":"projects.list","params":{}}
    C-->>UI: {"type":"res","id":"1","ok":true,"result":{…}}
    C-->>UI: {"type":"event","seq":124,"event":{"kind":"TaskCreated",…}}
```

Rules (contract §3–4, §6):

- Bearer token: 256-bit random (64 hex chars), generated per Core start,
  compared in constant time. As built, **every** core (embedded or daemon)
  writes it to `<data_dir>/core.token` and references that file from
  `<data_dir>/core.json` (`token_file_hint`), so a desktop and a CLI can share
  one core (`CoreConfig::write_discovery`, default `true`). The files are
  written with `platform::fs::write_private_file` (Unix `0600`; on Windows they
  inherit the data directory ACL — an explicit owner-only DACL is a security
  follow-up). ADR-0004's "embedded keeps the token in memory unless the user
  allows other local clients" is **(planned)**.
- File layout and the daemon hand-off are specified in ADR-0004; frames in
  ADR-0005 and `../api/protocol.md` §0 (`unsub` is implemented; `hello`,
  `sinceSeq`, `ping`, `lagged` are planned).
- `core.json` is advisory: a stale file (dead pid or failing `/health`) is
  ignored and overwritten. Two Cores on one data dir are prevented by
  discovery (a live core found through `core.json` ⇒ `CoreError::AlreadyRunning`)
  plus the `<data_dir>/session.lock` marker (a live foreign pid in it ⇒
  `AlreadyRunning`). The exclusive OS file lock `core.lock` of ADR-0004 is
  **(planned)**.
- Browsers cannot set WS headers, so the first-frame `auth` is the portable
  path; desktop uses it too so all clients share one code path in
  `@orkestr/client`.
- `GET /health` is unauthenticated and returns only `{status:"ok",version}`.
- On shutdown the core removes `core.json`/`core.token` (only if they name its
  own pid) and `session.lock`.

## 3. Runtime: an agent tool call that needs approval

```mermaid
sequenceDiagram
    autonumber
    participant AR as agent-runtime (tool loop)
    participant SEC as security (PermissionEngine + Guard)
    participant TE as task-engine
    participant EV as events (EventBus)
    participant UI as any client (desktop/web/mobile)
    participant T as terminal

    AR->>SEC: check(actor=Agent(a1), Terminal, Command("git push --force"), ctx)
    SEC->>SEC: Guard: Dangerous(force push) ⇒ Ask even in FullAuto
    SEC-->>AR: Decision::Ask(reason)
    AR->>TE: transition(task, Running → Waiting)
    AR->>EV: ApprovalRequested{approvalId, action, reason}
    EV-->>UI: event (topics: project:p, task:t, agent:a1, global)
    UI->>SEC: req approvals.resolve {approvalId, decision:"approve"}
    SEC->>EV: ApprovalResolved
    SEC-->>AR: resolution = Approved (one-shot, bound to exact action hash)
    AR->>TE: transition(task, Waiting → Running)
    AR->>T: execute command (agent-owned session)
    T->>EV: TerminalOutput … TerminalExited
```

An approval is bound to the **exact** action (hash of capability + resource +
normalized command/args). Approving `git push --force origin feature` does not
authorize any other command.

## 4. Target-scope containers (P4–P6)

```mermaid
flowchart LR
    subgraph clients["Clients"]
        d["Desktop (Tauri)"]
        w["Web Client (P4)<br/>apps/web — React, @orkestr/client,<br/>@orkestr/ui"]
        m["Mobile (P5)<br/>apps/mobile — React Native,<br/>@orkestr/client, @orkestr/protocol"]
    end

    subgraph cp["Control Plane (P4) — managed or self-hosted"]
        gw["Gateway/Relay<br/>TLS termination, WebAuthn/MFA,<br/>device registry, session revoke,<br/>rate limit, audit"]
        api["Control API<br/>accounts, devices, nodes, sync index"]
        pg[("PostgreSQL<br/>[MetadataStore: control-plane impl]")]
        s3[("S3 / MinIO<br/>[ObjectStore: encrypted chunks]")]
        pushsvc["Push service (P5)"]
    end

    subgraph n1["Local AI Node (desktop Core or orkestrd)"]
        core["Core"]
        sq[("SQLite (authoritative locally)")]
        ob[("objects/ (local cache)")]
    end
    n2["Remote AI Node (P6)<br/>orkestrd on another machine"]

    d -- "WS localhost" --> core
    w & m -- "WSS + session token" --> gw
    gw == "outbound tunnel from node<br/>(mTLS, node identity key)" ==> core
    core -- "SyncBackend (P4)" --> api
    api --> pg & s3
    core --> sq & ob
    core <== "node protocol via gateway (P6)" ==> n2
    api --> pushsvc
```

Design decisions:

- **The local SQLite stays authoritative for the node**; the Control Plane holds
  a synced replica plus device/session state. This is why `MetadataStore`
  needs a PostgreSQL implementation only on the Control Plane side (see
  `storage-abstraction.md` §4), and why "cloud can be disabled" (§67) is cheap.
- The gateway forwards the **same frames** as localhost (contract §4). It adds
  identity (`actor = RemoteDevice(deviceId, userId)`) to every request so the
  PermissionEngine can treat remote approvals differently (e.g. Secrets/Deploy
  may require re-auth with a passkey, §36).
- The tunnel is dialed outbound by the node; there is no listening port on the
  node beyond `127.0.0.1` (§35: "do not expose raw Core/terminal port").

## 5. Inter-container interfaces

| From → To | Protocol | Auth | Payload contract | Phase |
|---|---|---|---|---|
| Desktop UI → Desktop Host | Tauri IPC `invoke` | Tauri capability ACL (only `core_connection` + window cmds) | `{url, token, mode}` | P1 |
| Desktop UI → Core | WS `/rpc` JSON frames | First-frame `auth` | contract §4, types in `@orkestr/protocol` | P1 |
| Desktop Host → Core | Rust fn call `orkestr_core::start` | in-process | `CoreConfig` / `CoreHandle` | P1 |
| Core → git / CLIs / shells | child processes | OS user | stdout/stderr/JSON lines | P1 |
| Core → Ollama / Cloud APIs | HTTP(S) | API key via SecretStore | provider-specific, normalized to `ChatEvent` | P1 |
| Core → Control Plane | HTTPS + WSS | device key + session | Sync protocol (`docs/sync/`) | P4 |
| Web/Mobile → Gateway | WSS | WebAuthn/MFA session | contract §4 frames, unchanged | P4/P5 |
| Node ↔ Node | via gateway, mTLS | node identity | node protocol (§65 #14) | P6 |

## 6. Failure behaviour per container

| Failure | Behaviour |
|---|---|
| WebView reload / crash | Core unaffected (embedded) — UI reconnects, resubscribes, refetches state; agents keep running. |
| Desktop window close / quit with embedded Core | ADR-0004: P1 asks "Keep running in tray" (default) / "Stop all agents"; quitting calls `shutdown(UserQuit)` and running tasks become `Interrupted` (ADR-0022). P2 adds hand-off to a detached `orkestrd` (tasks `Interrupted → Recovering → Running`, gap < 5 s). |
| Core crash | On restart the leftover `<data_dir>/session.lock` of a dead pid is detected (as built; ADR-0022's `sessions` table is planned), `system.info.recoveredFromCrash` becomes `true` and a `Log` event records the recovery. Recovery hooks run before RPC accepts connections; as of Phase 0 none are registered — the task-engine (`Running/Waiting/Testing/Review → Interrupted`), agent-runtime, terminal and child-process containment hooks land in Phase 1. |
| Provider outage / limit | Router fallback chain (§7), `ProviderFallback` event. |
| Network down | Local providers (Ollama), editor, Git, terminal, snapshots keep working; sync journal queues (§42). |
| Keyring unavailable (headless Linux) | SecretStore falls back to encrypted file vault unlocked by user passphrase (see `storage-abstraction.md` §5.3). |
