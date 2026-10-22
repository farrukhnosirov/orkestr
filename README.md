# Orkestr

**Orkestr** is an AI Development OS: a local-first desktop environment where you
steer projects through one AI Manager that plans the work and coordinates
specialised agents, models, tools and compute nodes. The product spec is in
[`docs/spec/MASTER_SPEC.md`](docs/spec/MASTER_SPEC.md). The rules every
contributor follows are in [`docs/ENGINEERING_CONTRACT.md`](docs/ENGINEERING_CONTRACT.md).

> Status: **Phases 0–5 complete on Windows** (Phase 5 for Android). Phase 5
> (Mobile: Expo/React Native app, native push, biometric step-up) closed on
> 2026-10-22 — acceptance <!-- P5-ACCEPTANCE --> Node lane 44/44 / Android lane 11/11 (emulator orkestr_pixel, incl. fingerprint step-up) <!-- /P5-ACCEPTANCE -->; see [Mobile app (Android)](#mobile-app-android).
> iOS is not built (needs macOS) and real FCM/APNs delivery was verified with
> mock servers only. Phase 4 (Remote & Sync:
> self-hosted gateway, web client, end-to-end encrypted sync, Web Push) passed
> its acceptance run 40/40 on 2026-10-22 — see
> [Remote access and sync](#remote-access-and-sync-phase-4). Phases 0–3 were
> audited on 2026-10-07. Phases 1–3
> passed acceptance against the real desktop app with the offline Mock
> provider: Phase 1 45/45 checks, Phase 2 47/47 and Phase 3 64/64. Two audit
> rounds then confirmed and fixed 157 findings; see
> [`docs/audit/phase0-3-audit.md`](docs/audit/phase0-3-audit.md). Of the 82
> features tracked in
> [`docs/roadmap/feature-traceability.md`](docs/roadmap/feature-traceability.md),
> 41 are fully delivered, 22 are partial and 16 are architecture-only.
> Open items per phase are in
> [`docs/roadmap/mvp-milestones.md`](docs/roadmap/mvp-milestones.md).

## Features

**Phase 1: desktop core**
- Frameless desktop shell with a dockable workspace, command palette, tray mode, onboarding and the full UI in **English and Russian**, switchable live.
- Projects with per-project approval modes: Ask, SemiAuto or FullAuto.
- An **AI Manager** that plans the work as tasks with dependencies, waits for your approval and supervises the run.
- Agents work in parallel, each in its **own git worktree**, from 11 official templates or custom ones.
- Provider hub:
  - OpenAI and OpenAI-compatible, Anthropic, Gemini, DeepSeek and Ollama;
  - the official Claude Code and Codex CLIs, with your own login;
  - an offline Mock provider;
  - routing and fallback with a structured handoff.
- Code (Monaco), Files, Terminal (PTY), Git, Tasks (list, board and graph), Memory, Costs and Logs panels.
- Security:
  - a permission engine with path confinement;
  - a **dangerous-action guard** whose commands always wait for your approval;
  - a secret vault and an audit log;
  - an **emergency stop**: Stop All, Stop Project, disable network tools, lock the vault.
- Snapshots and task rollback that keep unrelated edits, plus crash recovery with a resume summary.

**Phase 2: multi-agent quality**
- Teams and team leads.
- A resource-aware scheduler.
- The Author → Review → Build → Test → Security **quality pipeline** with **Self-Healing**.
- Adversarial review and consensus mode.
- The **AI Merge Manager**, with merges behind approval.
- An AI Product Manager (idea → stories → tasks).
- The Architecture map, the Decision Log and Execution **Replay**.
- Model and agent benchmarks.
- A Knowledge Base for Markdown and text.
- Global search and project privacy policies.

**Phase 3: Visual OS**
- **Mission Control**.
- Six themes and a Theme Editor with a WCAG report.
- Layout presets (Ctrl+1…5), a Layout Editor and panels you can detach into their own windows.
- **Floating robots**, one transparent click-through window per agent, and a Robot Designer.
- A **HUD**.
- Voice:
  - push-to-talk and typed voice commands;
  - a local wake word, opt-in;
  - spoken notifications and a voice per agent.
- A sound engine.
- Four performance modes, from Ultra Visual to Battery Saver.
- **AI-editable UI**: the Manager proposes a theme or layout patch, and you preview it, keep it or undo it.

**Not built yet:**
- the MCP Hub, the Environment Manager, the Monaco LSP bridge and the hand-off to `orkestrd` (M2.7);
- the Browser agent (Phase 4);
- local Whisper speech-to-text;
- memory and permission-rule versions;
- macOS and Linux packages.

Acceptance used the Mock provider, so real providers and a real microphone have not been exercised end to end.

## Screenshots

All screenshots are captures of the app window only, taken on the final build.

| Set | What |
|---|---|
| [`phase1`](docs/screenshots/phase1/) | Manager plan card, agents, task graph, code, terminal, Git, providers, onboarding |
| [`phase2`](docs/screenshots/phase2/) | Quality pipeline, reviews, AI PM, Merge Manager, architecture map, Replay, teams, benchmarks (English and Russian) |
| [`phase3`](docs/screenshots/phase3/) | Acceptance run: robots, voice, every theme, layout preset and performance mode, HUD, AI-UI, detached panels |
| [`phase3-mission-control`](docs/screenshots/phase3-mission-control/) · [`phase3-theme`](docs/screenshots/phase3-theme/) · [`phase3-layout`](docs/screenshots/phase3-layout/) · [`phase3-robots`](docs/screenshots/phase3-robots/) · [`phase3-hud`](docs/screenshots/phase3-hud/) · [`phase3-voice`](docs/screenshots/phase3-voice/) | Per-area Visual OS reviews |
| [`i18n-ru`](docs/screenshots/i18n-ru/) | Every panel, settings section, onboarding step and dialog in Russian at 960×600, then a live switch to English |

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

- Rust stable (MSVC on Windows; `winget install Rustlang.Rustup`). In Git Bash, run `export PATH="$HOME/.cargo/bin:$PATH"` first.
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

### Screenshot scripts

Each script launches the app with a throw-away `ORKESTR_HOME`, seeds it over
RPC and captures only the webview through the WebView2 DevTools protocol. Pass
`--out <dir>` to write somewhere other than `docs/screenshots/<set>`.

| Script | Needs |
|---|---|
| `phase3-{theme,voice,robots,hud}-screenshots.mjs` | `npm run build && cargo build -p orkestr-desktop --features custom-protocol` |
| `phase3-mission-control-screenshots.mjs` | `npm run build` and `cargo build -p orkestr-core --bin orkestrd` (headless Edge) |
| `phase1-screenshots.mjs`, `i18n-ru-screenshots.mjs`, `phase3-layout-screenshots.mjs` | `npm run dev -w @orkestr/desktop` (Vite on `127.0.0.1:1420`) and `cargo build -p orkestr-desktop` |

The core accepts WebSocket connections only from the bundled app and the
`:1420` dev server, so the dev server must run on port 1420.

### Full verification

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
npm run gen:protocol && git diff --exit-code   # generated TS types up to date
npm run typecheck && npm test && npm run build
```

## Remote access and sync (Phase 4)

Phase 4 adds a self-hosted gateway, a web client and end-to-end encrypted
sync. The gateway only relays ciphertext: RPC traffic is encrypted between
the paired device and your core (`e2e-v1`, ADR-0030), and synced files are
encrypted before they leave the computer (ADR-0031). The web client and Web
Push are described in ADR-0036.

### 1. Run the gateway and the web client

```bash
npm run build                                   # builds apps/web/dist too
cargo build -p orkestr-gateway -p orkestr-core --bins
target/debug/orkestr-gateway --data-dir ~/.orkestr-gateway   --bind 127.0.0.1:8787 --web-dist apps/web/dist
```

Open `http://localhost:8787` and create the owner account with a passkey
(the first visitor becomes the owner). The WebAuthn relying party defaults to
`localhost`; for another host pass `--rp-id <host>` and
`--origin https://<host>` (repeatable). The gateway refuses to bind a
non-loopback address unless it runs behind a TLS proxy
(`--behind-tls-proxy`); TLS/ACME is not built in. An authenticator-app code
(TOTP) can be added as a fallback sign-in through `POST /v1/auth/totp/setup`
and `/confirm`.

### 2. Connect a core

1. Get a one-time enrollment token: after signing in, the web client's
   **Pair this browser** screen has **Create enrollment token**
   (`POST /v1/cores/enrollment-token`).
2. Desktop: **Settings → Remote & Sync → Remote access**: enter the gateway
   URL and the token, then **Enable remote access**. The core dials out
   (nothing listens on the network; `orkestrd` stays on loopback). The same
   works headless with `remote.enable {gatewayUrl, enrollmentToken}`.
3. **Disable** or the **kill switch** (revoke all sessions + disable) cut the
   tunnel immediately; the core does not redial.

### 3. Pair a device

1. Desktop: **Pair a new device**. Copy the pairing text (`orkestr-pair:{…}`).
2. Web client (signed in): paste it under **Pair this browser** and choose a
   device name.
3. Both screens show a 6-digit security code (SAS). If they match, tick
   **The security codes match**, pick a role (viewer / operator / admin) and,
   if needed, **Allow remote terminal**, then **Approve device**.
4. The browser keeps a non-extractable device key in IndexedDB. Dangerous
   actions (for example approving a high-risk command, discarding changes)
   ask for a passkey confirmation (step-up). Revoke a device from the same
   settings page; its sessions close and its push subscriptions are dropped.

### 4. Configure sync

**Settings → Remote & Sync → Encrypted sync** for the active project:

- Gateway URL and access token can stay empty when remote access is enabled
  on this core (the core's gateway credentials are used); otherwise use the
  gateway URL and a session token.
- On the first device leave **Remote key** empty: a key is generated and
  shown once. Paste the same key and the same **Project name at the remote**
  on every other device. Lose the key and the synced data cannot be read.
- **Sync now** pushes and pulls; **Continue on another device** publishes the
  working tree (including uncommitted files), open editors and a task summary;
  **Resume from another device** restores them on the other computer. Git
  history is never synced, so the other device needs the repository at the
  same commit. Concurrent edits keep both versions (a `*.conflict-*` copy)
  until you resolve them.

### Phase 4 acceptance

```bash
cargo build -p orkestr-core -p orkestr-gateway --bins && npm run build
node scripts/smoke/phase4-acceptance.mjs        # needs Microsoft Edge
```

It starts a gateway, two headless cores, headless Edge with a virtual
passkey authenticator (driving the built web app) and a mock push endpoint,
and runs scenarios D (secure remote access) and C (encrypted sync).

## Mobile app (Android)

`apps/mobile` is an Expo SDK 58 / React Native 0.88 (React 19) app built as a
dev build (ADR-0032). It is a standalone npm project outside the root
workspaces: run `npm install`, `npm run typecheck` and `npm test` inside
`apps/mobile`. It talks to your computer through the same gateway and
`e2e-v1` channel as the web client: login with TOTP, pairing with SAS,
monitoring, Emergency Stop and the remote kill switch, approvals with a
biometric step-up (ADR-0038), Manager chat and voice, quick review (tasks,
diff, logs), a light editor, devices, settings (en/ru) and push (ADR-0037).

### Toolchain (Windows, no Android Studio)

- JDK 21 (Eclipse Temurin); set `JAVA_HOME`.
- Android SDK command-line tools in `%LOCALAPPDATA%\Android\Sdk` (set
  `ANDROID_HOME`; add `platform-tools`, `emulator` and
  `cmdline-tools\latest\bin` to `PATH`), then with `sdkmanager`:
  `platforms;android-36`, `platforms;android-37.0`, `build-tools;36.0.0`,
  `build-tools;37.0.0`, `ndk;27.1.12297006`, `cmake;3.22.1`,
  `platform-tools`, `emulator`, `system-images;android-36;google_apis;x86_64`.
- Long paths: enable Windows long paths (`LongPathsEnabled=1`) and replace
  `%ANDROID_HOME%\cmake\3.22.1\bin\ninja.exe` with ninja 1.12.1 (the bundled
  one breaks on long paths in the New Architecture native build). Build from
  the repository path itself; a `subst` drive breaks React Native codegen.
- Emulator: an AVD named `orkestr_pixel` (Pixel 7, android-36, x86_64), e.g.
  `avdmanager create avd -n orkestr_pixel -k "system-images;android-36;google_apis;x86_64" -d pixel_7`.

### Build

```powershell
cd apps/mobile
npm install
npx expo prebuild -p android          # generates apps/mobile/android (gitignored)
cd android
.\gradlew.bat assembleDebug -PreactNativeArchitectures=x86_64
# → apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk
```

The first build takes 15–25 minutes (Gradle and native caches); later builds
take a few minutes.

### Run on the emulator

```powershell
emulator -avd orkestr_pixel
adb install -r apps/mobile/android/app/build/outputs/apk/debug/app-debug.apk
cd apps/mobile; npx expo start --dev-client   # Metro serves the JS bundle
adb reverse tcp:8081 tcp:8081                  # Metro
adb reverse tcp:18787 tcp:18787                # a local gateway on port 18787
```

The debug build allows cleartext HTTP, so the app can reach a gateway on your
computer as `http://10.0.2.2:<port>` or, after `adb reverse`, as
`http://127.0.0.1:<port>`. If Metro serves a stale bundle, restart it.

### Pair the phone with your computer

1. Run the gateway and connect your core as in
   [Remote access and sync](#remote-access-and-sync-phase-4). Add a TOTP
   authenticator for the owner account (`POST /v1/auth/totp/setup` and
   `/confirm`): the app signs in with the gateway URL and a TOTP code
   (passkey sign-in on mobile is not built yet).
2. Desktop: **Settings → Remote & Sync → Pair a new device** shows a QR
   code (the `orkestr://pair?offer=…` deep link) next to the pairing text
   (`orkestr-pair:{…}`) and a copy button. On the phone open **Pair** and scan
   the QR, paste the text, or open a deep link
   `orkestr://pair?offer=<urlencoded text>` (on the emulator:
   `adb shell am start -a android.intent.action.VIEW -d "orkestr://pair?offer=…"`).
3. Compare the 6-digit SAS on both screens, then approve the device on the
   desktop with a role.
4. Right after pairing the app registers a biometric-gated step-up key.
   Without an enrolled fingerprint or face (on the emulator: enroll one in
   Settings, then `adb emu finger touch 1`) dangerous actions such as
   approving a high-risk command are not available from the phone.

### Push notifications

While the app is connected it shows local notifications for approvals and
other events; this works without Firebase. For push while the app is closed:

- Android: put your Firebase `google-services.json` into `apps/mobile`
  (picked up by `app.config.js`), rebuild, and start the gateway with
  `--fcm-service-account <service-account.json>` (`--fcm-endpoint <URL>`
  overrides the API base, for mocks).
- iOS: `--apns-key <AuthKey.p8> --apns-key-id <ID> --apns-team-id <ID> --apns-topic dev.orkestr.mobile`
  (`--apns-endpoint <URL>` for a sandbox or mock).

Payloads are encrypted end to end (AES-256-GCM); FCM/APNs only see a generic
"Orkestr — Action needed" alert. Real FCM/APNs delivery has only been tested
against mock servers.

### iOS

The code is shared, but the iOS app has not been built or run: it needs macOS
with Xcode (`npx expo prebuild -p ios`, then build in Xcode).

### Phase 5 acceptance

See `scripts/smoke`: `phase5-acceptance.mjs` (Node lane: gateway, headless
core, the mobile core with in-memory adapters, mock FCM/APNs) and
`phase5-android.mjs` (Android lane on the `orkestr_pixel` emulator, uses
`scripts/android-e2e.mjs`). Results and the deferred list are in
[`docs/roadmap/feature-traceability.md`](docs/roadmap/feature-traceability.md).

## Troubleshooting

- **`npm run tauri dev` fails because port 1420 is unavailable.** Windows may
  reserve it for Hyper-V / WinNAT. Check with
  `netsh interface ipv4 show excludedportrange protocol=tcp`. If 1420 is in an
  excluded range, run an elevated terminal: `net stop winnat` then
  `net start winnat` (or reboot), or use a custom-protocol build instead of
  the dev server: `npm run build && cargo build -p orkestr-desktop --features custom-protocol`.

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
| `crates/agent-runtime` | Agent tool loop, templates, CLI agents, checkpoints, budgets. |
| `crates/orchestrator` | AI Manager, planner, run supervisor, quality pipeline, Self-Healing, reviews, consensus, Merge Manager, teams, AI PM. |
| `crates/quality` | Quality runner: build/test/lint detection, execution and output parsers. |
| `crates/insight` | Architecture map and Execution Replay. |
| `crates/sync` | `SyncBackend` seam; Local-only until Phase 4. |
| `packages/protocol` | `@orkestr/protocol`: generated TS types and helpers. |
| `packages/client` | `@orkestr/client`: typed WebSocket client with reconnect and replay. |
| `packages/ui` | `@orkestr/ui`: `--ok-*` tokens, theme engine, primitives. |
| `apps/mobile` | Expo / React Native Android app (standalone npm project, not in the root workspaces). |
| `apps/desktop` | Tauri 2 shell (windows, tray, robots, HUD) and the React UI (panels, features, i18n en/ru). |
| `scripts/smoke` | End-to-end smoke, acceptance and screenshot scripts. |
| `docs/` | Spec, contract, ADRs, architecture, roadmap, audit report, screenshots. |
