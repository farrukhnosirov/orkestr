# Feature Traceability Matrix (§68: "must not be lost")

Every §68 item, the 50 numbered ones plus the additional list, is mapped to its target phase, owning crate(s), UI panel(s) and delivery level in Phases 1–3. A level marked here as **Full** or **Partial** is a commitment checked at the milestone exit (see `mvp-milestones.md`).

**Delivery levels**
- **Full**: the user-visible capability is complete for the scope the spec describes in that phase.
- **Partial**: real, working subset; the remaining scope is named in Notes.
- **Arch**: architecture-only. The entities, traits, protocol methods/events and capability names exist, and the seams are tested. Calls return the explicit unimplemented-feature error (RR-G14; as built `bad_request` with `data.unimplementedFeature`, or `PlatformError::NotSupported` inside the platform layer). There is no fake behaviour.
- **—**: not addressed in that phase. No seam is needed yet, or the seam belongs to a later phase.

**Panels** are `apps/desktop/src/panels/<PanelName>` (contract §8). Panel names: Manager, Code, Files, Terminal, Git, Agents, Tasks, TaskGraph, Approvals, Logs, Memory, Costs, Snapshots, Providers, Settings, MissionControl, Architecture, Browser, Database, Deployments, Monitoring, Decisions, Replay, McpHub, Environments, ThemeEditor, LayoutEditor, RobotDesigner, Nodes. Shell-level UI (not dockable panels) is TitleBar, CommandPalette, Onboarding, Tray, HUD and the robot windows.

**As built (Phase 0, 2026-10-02):** `apps/desktop/src/panels/registry.ts` registers 15 panels — `manager`, `code`, `files`, `terminal`, `git`, `agents`, `tasks`, `architecture`, `browser`, `database`, `logs`, `deployments`, `memory`, `costs`, `monitoring`. Only **Logs** is functional (live event stream); the others render `PhasePlaceholder` ("Arrives in Phase N"). The shell-level TitleBar, CommandPalette, ProjectRail, StatusBar and ConnectionScreen exist. Panels named above but not registered yet (TaskGraph, Approvals, Snapshots, Providers, Settings, MissionControl, Decisions, Replay, McpHub, Environments, ThemeEditor, LayoutEditor, RobotDesigner, Nodes) are added by their phase owners. The registry's placeholder phases disagree with this matrix for Architecture (registry 2, panel here P3), Browser (2 vs P3), Database (2 vs P4) and Deployments (2 vs P4); this matrix is the plan of record and the registry should be aligned (code-side follow-up).

**As built at Phase 1 acceptance (2026-10-02):** the P1 column below is the
plan of record; this list records where the delivered state differs or needs a
qualifier (checked by `scripts/smoke/phase1-acceptance.mjs`, the Rust/TS test
suites and the screenshot review in `docs/screenshots/phase1/`). Items not
listed here are delivered at their P1 level. The panel registry now has
Manager, Code, Files, Terminal, Git, Agents, Tasks, Logs, Memory and Costs
functional; approvals live in the title bar/security layer, snapshots in the
Git panel's Snapshots tab and providers in Settings → AI Providers rather than
in separate panels.

| # | Feature | Planned P1 | Delivered P1 | Difference |
|---|---|---|---|---|
| 2 | Dynamic Agent Spawn | Partial | Partial | As planned: the approved plan spawns one agent per ready task up to `maxParallelAgents`. |
| 6 | Model fallback | Full | Full | Fallback + structured handoff work for API models; Claude Code / Codex CLI agents have no fallback chain. |
| 10 | Git Worktrees per agent | Full | Full | Verified on disk (`git worktree list`). Startup reconciliation of orphaned worktrees is open. |
| 11 | AI Merge Manager | Arch | Arch | `vcs.mergePreview`/`vcs.merge` exist for the user; runs only list merge proposals with diff stats. |
| 12 | Automatic Code Review | Partial | Partial | A Reviewer agent can be spawned on a task; the Manager does not yet schedule reviews automatically. |
| 15 | Auto Testing | Partial | Partial | QA agents run tests through `run_command` and results reach the run report. |
| 19 | Terminal Workspace | Full | Partial | PTY sessions, gated runs and the agent command tab work; take-control handover and scrollback snapshots are open. |
| 20 | Monaco | Partial | Partial | Tabs, quick open, search, agent change markers; "open in external IDE" and Git gutter decorations not verified in this pass. |
| 23 | Visual Task Graph | Partial | Partial (above plan) | The graph is interactive already (drag to add a dependency, Delete removes one), with a MiniMap. |
| 28 | Voice Command Center | Arch | — | No `voice.*` namespace or `Microphone` trait yet; the seam is still to be added. |
| 31 | Approval Levels | Full | Full | "Remember this decision" is open. |
| 34 | Rollback Everything | Partial | Partial | Task and workspace rollback work for every agent task (after-checkpoints are now always taken); HEAD/index restore is open. Permission-rule versions are **not built**: `permissions.delete`/`set` overwrite or hard-delete rules with no history or restore. |
| 35 | Snapshots | Full | Partial | Capture, diff, restore and task rollback work; snapshot GC and the 500-file timing benchmark are open. |
| 37 | Smart Budgeting | Partial | Partial | Budgets are checked before each API model call; CLI agents are checked after the run only. |
| 49 | Decision Log | Arch | Partial | Decisions are recorded through the Manager's `record_decision` tool and the Memory panel; plan approvals do not record one automatically. |
| — | Background service | Partial | Partial | `orkestrd` + tray keep-running; no Job Object supervisor, so child processes of a hard-killed core are not reaped by the OS. |
| — | Installer / CI | (M1.1, M1.8) | — | No signed installer, auto-update or CI matrix yet. |

**As built at Phase 3 acceptance (2026-10-05):** this table covers the
Visual OS items that have a P3 commitment. The evidence is:
- `scripts/smoke/phase3-acceptance.mjs`, 64/64 checks against the real desktop app;
- the screenshot review in `docs/screenshots/phase3/`;
- the Rust and TS test suites.

The architecture is described in `docs/architecture/visual-os.md`. Items not
listed here were not re-verified in the Phase 3 pass. Windows was the only OS
tested.

| # | Feature | Planned P3 | Delivered P3 | Difference / evidence |
|---|---|---|---|---|
| 22 | Mission Control | Full | Partial | Widgets cover the planned set: projects, agents, task graph, local node CPU/RAM/GPU, provider health, limits, spend, approvals, quality builds/tests, reviews & merges, alerts. Live from events, with the arrangement saved per device. Missing: the 200-node / 50 fps exit check was not measured, and the charts poll on the performance-policy interval. |
| 23 | Visual Task Graph | Full | Partial | Interactive graph (link, delete dependency, open) in Tasks and the Mission Control widget. Reprioritize-by-drag and the 200-node performance bar were not verified. |
| 26 | Background Agents | Full | Partial | Tray keep-running and the autostart entry date from P1. The reboot check of M3.5 was not run. |
| 28 | Voice Command Center | Full | Partial | Scenario G passes through the post-STT path (`simulateTranscript` seam, TTS called), and typed HUD commands work, e.g. «show build errors» answered from the quality runner. Push-to-talk and destructive-intent confirmations work. STT is WebSpeech or OpenAI Whisper (cloud, consent-gated); **no local Whisper**. A real microphone was not exercised in acceptance. |
| 29 | Wake Word | Full | Partial | Local MFCC/DTW spotter with enrollment, opt-in, VAD-gated, paused in Battery Saver. SP-15 budgets and the zero-network packet capture were not measured. |
| 30 | Voice Notifications | Full | Full | Spoken notifications with DND / quiet hours. Robots with voice on speak for their agent, with no double announcement. |
| 34 | Rollback Everything | Full | Partial | UI part: every UI config change is versioned; AI patches carry an exact inverse patch, and Undo is verified. Scenario J was not re-run on the Phase 3 build. Memory-item and permission-rule versions are **not built** (hard delete, in-place update, no restore RPC), so the item as a whole is Partial. |
| 45 | Live Infrastructure Monitor | Partial | Partial | Local node resources shown in Monitoring and Mission Control. |
| — | Custom Visual OS | Full | Full | Frameless shell and custom controls. Transparent robot windows, verified: alpha-0 corners, `WS_EX_LAYERED|TRANSPARENT`, topmost. Floating and detached panels. |
| — | Floating robots | Full | Partial | Scenario H passes: robot per agent, live states, completion pose + bubble, click opens the result. Click-through, context menu, drag/snap, drop-on-project and overflow cluster all work. Missing: per-robot CPU/RAM budget not measured; no Linux Wayland fallback; robot windows do not follow the battery / frame-rate auto-downgrade. |
| — | Robot Designer | Full | Full | 2D/3D, skin, material, accent, size, accessories, animation intensity, voice, sounds, behaviour. Stored per template. The theme supplies defaults for unstyled robots. |
| — | Theme Engine | Full | Full | Six built-in themes, all screenshotted on Mission Control. Live editor with WCAG report, sounds and robot defaults. Applied in every window (main, HUD, robots, detached). |
| — | Layout Editor | Full | Full | Presets Coding / Mission Control / Monitoring / Focus / Review / Custom (Ctrl+1…5), save as, lock, export/import, per-panel placement. Persisted per device. |
| — | Docking (detach to OS windows) | Full | Partial | Detach and re-dock verified. Restoring popouts on a second monitor after a restart was not verified (single-monitor host). |
| — | HUD / Ambient | Full | Full | Status, agents, build/test chip, approvals, voice indicator, mini Manager. The voice indicator's cross-window click was not exercised. |
| — | AI-editable UI | Full | Partial | Manager patch → preview → Keep → Undo verified, behind the allowlist + safe-text + typed-schema validation. Missing: patches that lower contrast are not rejected; no `--safe-ui` recovery; device overrides are not patchable. |
| — | Voice per agent | Full | Full | Template voice profile + per-user override + robot voice id. |
| — | Performance Mode | Full | Full | One policy table for blur, glow, motion, backgrounds, robots, charts, HUD and voice visuals. All four modes verified on `<html>`. Auto Battery Saver and frame-budget step-down. Not measured: the UI CPU budget. |
| — | Windows-first + macOS/Linux | Full | Partial | Windows only. **No macOS / Linux packages.** |
| — | Onboarding | Partial (+9 voice) | Partial | The voice step is shown as P3 in the wizard. |

**Crate names** drop the `orkestr-` prefix. "client" means `@orkestr/client`, "ui" means `@orkestr/ui`.

**How to read the P1 / P2 / P3 columns in sections A and B (reconciled
2026-10-07).** Phases 1–3 have passed acceptance, so these columns give the
**delivered** level for each phase, not the plan. Where an acceptance check
(the three tables above and in "Phase 2 acceptance check" below) recorded a
lower or higher level than planned, the column shows the delivered level, and
the Notes start with **"Planned …; delivered …"** and say what is missing.
Items that no acceptance pass contradicted keep their planned level. The
Section D counts are computed from these delivered columns.

## A. Numbered inventory (§68 #1–#50)

| # | Feature | Spec | Target phase | Crates | UI panel(s) | P1 | P2 | P3 | Notes / remaining scope |
|---|---|---|---|---|---|---|---|---|---|
| 1 | Agent Marketplace | §6, §48 | 7 | agent-runtime, protocol, (plugin host P7) | Agents (Templates tab) | Partial | Partial | Partial | P1–3: local template import/export (JSON manifest with version and permissions). Official/community/private catalogue and signing are Phase 7 |
| 2 | Dynamic Agent Spawn | §1.5, §6 | 2 | orchestrator, agent-runtime, node, task-engine | Manager, Agents | Partial | Full | Full | P1: Manager proposes agents from templates and the user approves the plan; `maxParallelAgents` (RR-A7). P2: resource, cost and coordination-aware spawning without approval in SemiAuto/FullAuto |
| 3 | Agent Teams | §6 | 2 | domain, orchestrator, agent-runtime | Agents | Arch | Full | Full | P1: `AgentTeam` entity and team template schema. P2: ERP/Web/Mobile/Security/Rapid Prototype/custom teams. **P2 done (teams-pm):** persistent teams with members (template × count, model override), optional lead and bounded default policies; `teams.*` RPC; Agents → Teams tab; Manager plans delegate to a team (`ManagerRequestOptions.teamId` or `teams.assign` with `planId`) |
| 4 | Agent Hierarchy | §6 | 2 | orchestrator, domain | Agents, TaskGraph | Arch | Full | Full | P1: `AgentInstance.parentAgentId`. P2: Manager → Team Leads → Executors → Reviewers delegation with handoffs. **P2 done (teams-pm):** `orchestrator::team_lead` — lead decomposes a delegated task (structured output, fan-out cap), creates child tasks (`parentId`) with dependencies, spawns executors (`parentAgentId` = lead, parallel cap, attempts, split budget, time limit), then a reviewer; depth ≤ 2 below the Manager; hierarchy tree in the Teams tab. Handoffs between executors are not used yet |
| 5 | Multi-model routing | §7 | 2 | providers (router), orchestrator, cost | Providers, Manager, Agents | Partial | Full | Full | P1: manual routing per template/role/task, privacy filter (RR-G5). P2: automatic scoring by quality, latency, budget, limits and benchmarks (ADR-0023) |
| 6 | Model fallback | §7, §63B | 1 | providers, agent-runtime, events | Providers, Manager (event toast) | Full | Full | Full | P1: manual fallback chain, `ProviderFallback` event, structured handoff (ADR-0012). P2 adds score-based chains |
| 7 | Project Memory | §20 | 2 | memory, context, storage | Memory | Partial | Full | Full | P1: CRUD, kinds (requirements, architecture, conventions, decisions, mistakes, TODO, known issues), FTS, project rules take priority. P2: automatic agent summaries and extraction |
| 8 | Global Memory | §20 | 2 | memory | Memory, Settings | Partial | Full | Full | Same split as #7. Project rules override global rules |
| 9 | Knowledge Base | §20 | 2 | memory (KnowledgeSource), context | Memory (Knowledge tab) | Arch | Partial | Partial | **Planned P3 Full; delivered Partial:** Markdown/text ingest with FTS, and relevant chunks go into the agent's initial context (`context_plan::knowledge`, after the bundle, within a share of the budget). PDF and URL sources are recorded but not ingested (`PDF_UNSUPPORTED` / `URL_UNSUPPORTED` in `memory::knowledge`). P1: `KnowledgeSource` entity. P2: Markdown/text/API docs ingest with FTS. P3: PDF and external docs, relevance in the context pack. Open: PDF and URL ingestion, knowledge in `context.preview` |
| 10 | Git Worktrees per agent | §16 | 1 | vcs, orchestrator, agent-runtime | Git, Agents | Full | Full | Full | ADR-0009. Merge Manager is #11 |
| 11 | AI Merge Manager | §16 | 2 | orchestrator (merge), vcs | Git, Approvals, Tasks | Arch | Partial | Partial | **Planned P2/P3 Full; delivered Partial:** revert proposals for merged-task rollback are not built. P1: `merge_preview` and conflict detection in vcs, manual merge by the user. P2: proposals, gates, policy auto-merge, revert proposals (P2 as built: `merge.*` + Git panel → Merge Manager; revert proposals open — see git-worktree-strategy §6.5) |
| 12 | Automatic Code Review | §17 | 2 | orchestrator, agent-runtime | Tasks, Code (diff), Agents | Partial | Full | Full | P1: user or Manager can assign a Reviewer agent to a task's diff; `ReviewFinished`. P2: automatic pipeline step and a quality gate |
| 13 | Consensus Mode | §8 | 2 | orchestrator, providers, memory (decisions) | Manager, Decisions | — | Full | Full | P2: N-model proposals, synthesis, alternatives stored (ADR-0027) |
| 14 | Adversarial Review | §8 | 2 | orchestrator, agent-runtime | Tasks, Decisions | — | Full | Full | Reviewer that cannot auto-agree; findings as structured items |
| 15 | Auto Testing | §17 | 2 | orchestrator, terminal, agent-runtime | Tasks, Logs | Partial | Full | Full | P1: agents run test commands through `run_command`; results recorded. P2: auto-detection of unit/integration/E2E/lint/typecheck/build/smoke, `TestRunFinished`, gates |
| 16 | Self-Healing Loop | §17 | 2 | orchestrator, task-engine, cost | Tasks, Logs | Arch | Full | Full | P1: hard limits (maxTurns, time, cost) that the loop will use. P2: Implement → Build → Test → Analyze → Fix loop. As built: `orchestrator::pipeline` + `healing` (per-task quality pipeline in Manager runs, `pipeline.*` RPC, task drawer stepper; see `docs/architecture/quality-pipeline.md`) |
| 17 | Visual Browser Agent | §18 | 3 | agent-runtime (browser tool), security, platform | Browser | Arch | Arch | Arch | **Planned P2 Partial (stretch), P3 Full; delivered Arch:** no browser agent tool was built in Phase 2, and the Browser panel is still a `PhasePlaceholder`. P1: `Browser` capability. P2 (stretch): Playwright-driven tool (open, click, type, screenshot, console and network errors). P3: Browser panel with live screenshots and responsive checks (RR-G12) |
| 18 | Computer Control | §18 | 7 | platform (ComputerControl trait), security | Settings (permission) | Arch | Arch | Arch | Capability exists and is **Deny by default** (ADR-0010). Platform trait stubs return `unimplemented`. SP-22 |
| 19 | Terminal Workspace | §15 | 1 | terminal, platform, security | Terminal | Partial | Partial | Partial | **Planned Full; delivered Partial:** PTY sessions, gated runs and the agent command tab work; take-control handover and scrollback snapshots are not built. ADR-0014. Phase 4: remote terminal through the gateway |
| 20 | Monaco | §15 | 2 | apps/desktop, core (files/LSP) | Code | Partial | Partial | Partial | **Planned P2/P3 Full; delivered Partial:** no LSP bridge (RR-A6, M2.7 not built) and no inline AI actions; the P1 editor features are what ships. P1: tabs, syntax, built-in TS/JSON/CSS/HTML services, search, diff, Git decorations, agent change markers, open in external IDE. P2: LSP bridge (RR-A6) and inline AI actions |
| 21 | Project Desktop | §14 | 4 | apps/desktop, ui | all §14 panels | Partial | Partial | Partial | P1: Manager, Code, Files, Terminal, Git, Agents, Tasks, Logs, Memory, Costs (+ Approvals, Snapshots, Providers). P2: + Decisions, Replay, Environments. P3: + Architecture, Browser, MissionControl. Database, Deployments and Monitoring (beyond local) are Phase 4. Placeholders show "Arrives in Phase N" (`PhasePlaceholder`; the phase numbers in `apps/desktop/src/panels/registry.ts` currently differ from this row for Architecture, Browser, Database and Deployments — see the note under the panel list) |
| 22 | Mission Control | §32 | 3 | ui, core (aggregation methods), node, cost | MissionControl | — | — | Partial | **Planned P3 Full; delivered Partial:** see the Phase 3 table (200-node / 50 fps check not measured; charts poll). P3 scope: projects, agents, task graph, local node resources, provider health, limits, spend, approvals, builds, alerts. Deployments and remote nodes tiles fill in during Phases 4 and 6 |
| 23 | Visual Task Graph | §11 | 3 | task-engine, apps/desktop (@xyflow/react) | TaskGraph, Tasks | Partial | Partial | Partial | **Planned P3 Full; delivered Partial:** reprioritize-by-drag and the 200-node performance bar not verified. P1: read-only DAG view. P2: live status overlay. P3: interactive editing (link, reprioritize, create) |
| 24 | Parallel Execution | §1.8, §11 | 2 | task-engine (scheduler), orchestrator, vcs | Tasks, Agents | Partial | Full | Full | P1: up to `maxParallelAgents` independent tasks, simple FIFO by priority. P2: resource- and conflict-aware scheduling (M2.1) |
| 25 | Priority Queue | §11 | 2 | task-engine | Tasks | Partial | Full | Full | P1: strict classes Critical…Background. P2: aging and preemption of Background (RR-A13) |
| 26 | Background Agents | §12 | 3 | core, platform (autostart), apps/desktop (tray) | Tray, Settings | Partial | Partial | Partial | **Planned P2 Partial (hand off), P3 Full; delivered:** P2 adds nothing over P1 (no hand off to `orkestrd`, M2.7 not built); P3 Partial (autostart opt-in exists, but the M3.5 reboot check was not run). P1: tray keep-running and manual `orkestrd`. P2: hand off to the daemon. P3: autostart opt-in (ADR-0004, RR-C2) |
| 27 | Cross-project Agents | §68 | 2 | orchestrator, security | Manager, Agents | Arch | Partial | Partial | Linked instances across projects with a `crossProjectGroupId`, each keeping its own project scope (RR-A14). Full cross-project Manager tasks in Phase 4 (multi-device context). **P2 (teams-pm):** a team can be assigned to several projects; each assignment carries a permission scope installed as agent-scope `Deny` rules on every team agent (secrets denied unless allowed); `Manager::with_team_secret` never resolves another project's or a global secret (tested) |
| 28 | Voice Command Center | §33 | 3 | core::voice, platform (Microphone, hotkeys), providers (STT/TTS) | HUD, Manager, Settings | — | — | Partial | **Planned P1/P2 Arch, P3 Full; delivered:** no `voice.*` seam before Phase 3; P3 Partial (STT is WebSpeech or cloud Whisper only, no local Whisper; a real microphone was not exercised in acceptance). P1: `voice.*` namespace stubs, `Microphone` trait, `VoiceProfile` on templates (RR-C1). P3: PTT, STT, intents (ADR-0017) |
| 29 | Wake Word | §33 | 3 | core::voice, platform | Settings, HUD | — | — | Partial | **Planned P1/P2 Arch, P3 Full; delivered:** no seam before Phase 3; P3 Partial (SP-15 budgets and the zero-network capture not measured). Local only, opt-in, VAD-gated (ADR-0017, SP-15) |
| 30 | Voice Notifications | §33 | 3 | core::voice, events (Notification) | Settings | — | — | Full | TTS on `Notification` with DND windows |
| 31 | Approval Levels | §23 | 1 | security, task-engine | Approvals, Settings, TitleBar | Full | Full | Full | Ask, SemiAuto, FullAuto + Disable Full Auto (ADR-0010). Phase 4: remote approvals; Phase 5: mobile approvals |
| 32 | Permission Sandbox | §23 | 2 | security, platform, terminal | Settings (Permissions), Approvals | Partial | Full | Full | P1: capability engine, scopes, worktree/path confinement, network deny switch. P2: OS-level process sandbox where practical (SP-17 → ADR) and containers for Docker-capable tasks |
| 33 | Dangerous Action Guard | §23 | 1 | security | Approvals | Full | Full | Full | ≥ 150 test cases; SQL/git/shell classification (ADR-0010) |
| 34 | Rollback Everything | §25 | 2 | snapshots, security (rules versions), protocol (ui config versions), memory | Snapshots, Settings, ThemeEditor | Partial | Partial | Partial | **Planned P2/P3 Full; delivered Partial:** task/workspace rollback and versioned UI config (with Undo) work. **Not built:** permission rule versions (planned P1) and memory versions (planned P2) — `memory.delete` and `permissions.delete` hard-delete, updates overwrite in place, and there is no history or restore RPC; merged-task revert proposals and rollback from Replay are not built either. Remote side effects are not reversible (RR-A4) |
| 35 | Snapshots | §25 | 1 | snapshots, storage (objects), vcs | Snapshots | Partial | Full | Full | **Planned P1 Full; delivered P1 Partial** (snapshot GC and the 500-file benchmark were open at P1 acceptance). ADR-0021. P2 adds automatic checkpoints in loops. P4 syncs snapshots |
| 36 | Cost & Limit Center | §22 | 2 | cost, providers | Costs | Partial | Full | Full | P1: usage and estimated cost per provider/model/agent/project/task, daily and monthly budgets. P2: provider quota/limit state, forecasts. P3: Mission Control tiles |
| 37 | Smart Budgeting | §22 | 2 | cost, orchestrator | Costs, Settings | Partial | Full | Full | P1: per-task max, per-project monthly, "unrestricted" scopes, `BudgetThreshold` (RR-G4). P2: budget-aware routing and spawning |
| 38 | Model Benchmarking | §22 | 2 | cost (benchmarks), orchestrator | Costs (Benchmarks tab) | Arch | Full | Full | P1: usage and outcome records captured with the fields needed. P2: first-pass success, build success, defects, retries, latency, cost |
| 39 | Agent Performance Analytics | §22 | 2 | cost (analytics) | Agents, Costs | Arch | Full | Full | Same capture-in-P1, report-in-P2 split |
| 40 | Context Optimizer | §20 | 2 | context, memory, vcs | Manager (context preview) | Partial | Full | Full | P1: task, referenced files, project rules, relevant memory (FTS), token budget, concise handoff. P2: tree-sitter symbols, relevance ranking, dependency closure (ADR-0024) |
| 41 | MCP Hub | §19 | 7 | agent-runtime (MCP client), security, core | McpHub | — | Arch | Arch | **Planned P2/P3 Partial; delivered Arch:** only the `McpServer` domain entity exists. No stdio MCP client, no capability gating and no McpHub panel (M2.7 not built). P2: stdio MCP client, servers configured per project, capability gating (CA-4). P3: McpHub panel (status, tools, permissions, secrets, projects). P7: catalogue and community servers |
| 42 | Secrets Vault | §24 | 1 | security, platform | Settings (Secrets), Environments | Full | Full | Full | Keyring + encrypted vault, scopes, audit, Lock Vault (ADR-0011). Self-hosted and cloud secret providers in Phase 4 |
| 43 | Environment Manager | §26 | 4 | domain, security, core | Environments | Arch | Arch | Arch | **Planned P2/P3 Partial; delivered Arch:** the `Environment` entity has Dev/Test/Staging/Production kinds and a protected flag, but there is no environment manager, RPC or Environments panel (M2.7 not built). P1: `Environment` entity (Local) for secret scoping. P2: Dev/Test/Staging/Prod definitions, prod flag feeding the guard. P4: remote environments, DB connections |
| 44 | Deployment Agents | §26 | 4 | orchestrator, security (Deploy), platform | Deployments | Arch | Arch | Arch | `Deploy` capability (always Ask), `Deployment` entity, `DeploymentFinished` event. Providers (Docker, Compose, VPS/SSH, Vercel, Cloudflare, clouds) in Phases 4 and 7 |
| 45 | Live Infrastructure Monitor | §27 | 4 | node (sysinfo), core | Monitoring, MissionControl | Arch | Partial | Partial | P2: local node CPU/RAM/GPU/disk via `NodeStatus`. P3: shown in Mission Control. P4: services, containers, health checks, remote nodes |
| 46 | Bug Detective | §27 | 4 | orchestrator, memory, vcs | Logs, Tasks | — | — | — | Builds on #45, #15 and #16 plus production log access (Phase 4). No P1–3 seam beyond the existing Log and Task models |
| 47 | AI Product Manager | §9 | 2 | orchestrator (planning), task-engine | Manager, Tasks | Partial | Full | Full | P1: Manager plan → tasks with acceptance criteria and dependencies → approve → run. P2: Idea → Epic → Features → Stories → AC → Tasks hierarchy. **P2 done (teams-pm):** `pm.*` RPC — structured, validated draft (one repair round), editable in Tasks → Product, approval creates the task hierarchy with dependencies and an approved Manager plan (optional run / team); Manager composer "Product Manager mode" |
| 48 | Architecture Map | §21 | 3 | context (symbol graph), memory | Architecture | — | Partial | Full | P2: modules, dependencies and APIs extracted from code and memory. P3: Architecture panel visual map. **As built (Phase 2):** new crate `orkestr-insight` (`arch`): Cargo/npm/Python/Go/.NET/dir modules, binaries/Dockerfiles/compose services, compose DBs/SQLite/migrations/ORM schemas, axum/actix/express/nest/fastapi/flask/aspnet/gin/net-http routes + OpenAPI, manifest + import edges, grouped externals; incremental per-file cache, persisted snapshots (`architecture_snapshots`, 50/project), diffs, debounced auto-refresh after `FileChanged`; RPC `architecture.{map,refresh,diff}`, event `ArchitectureMapUpdated`; Architecture panel (React Flow map + list, search, kind filters, details → open in Code, change highlighting, SVG export) already shipped |
| 49 | Decision Log | §21 | 2 | memory (decisions), events | Decisions | Partial | Full | Full | **Planned P1 Arch; delivered P1 Partial (above plan):** decisions are recorded through the Manager tool and the Memory panel. P1: `Decision` entity, `DecisionRecorded`, created by Manager plan approvals. P2: Decisions panel, consensus alternatives, links to tasks and files |
| 50 | Execution Replay | §21 | 2 | events (query), core | Replay, Logs | Partial | Full | Full | P1: durable event log with correlation ids, Logs filter by correlation. P2: Replay timeline across request → plan → agents → tools → files → tests → reviews → approvals → merge. **As built (Phase 2):** `orkestr-insight::replay` (identity + correlation relevance, bounded scan, agent/phase filters, state reconstruction), RPC `replay.{runs,timeline,step}`, Replay tab in the Logs panel (run/task picker, agent and phase filters, play/pause/step with speed, step details, state at cursor, files open in Code) |

## B. Additional inventory (§68 "Также")

| Feature | Spec | Target phase | Crates | UI panel(s) / surface | P1 | P2 | P3 | Notes |
|---|---|---|---|---|---|---|---|---|
| Custom Visual OS | §28 | 3 | apps/desktop, ui | TitleBar, all | Partial | Partial | Full | P1: frameless, custom controls, tokens, blur/glow in Balanced. P3: full effects, transparency, floating windows |
| Floating robots | §30 | 3 | apps/desktop (src-tauri overlay), platform | Robot windows | — | — | Partial | **Planned P3 Full; delivered Partial:** per-robot CPU/RAM budget not measured, no Wayland fallback, no battery / frame-rate auto-downgrade for robot windows. ADR-0016, SP-04 |
| Robot Designer | §30 | 3 | ui, protocol (ui_config.robots) | RobotDesigner | — | — | Full | Skins, scale, animations, voice, sounds, behaviour |
| Theme Engine | §29 | 3 | ui, protocol (ui_config) | ThemeEditor, Settings | Partial | Partial | Full | P1: token engine, dark/light built-ins, schema, versions. P3: full editor |
| Layout Editor | §29 | 3 | ui, apps/desktop (dockview) | LayoutEditor | Partial | Partial | Full | P1: drag, resize, hide and presets (Coding, Focus). P3: editor plus Mission Control and Monitoring presets |
| Docking | §28 | 1 | apps/desktop (dockview) | all panels | Full | Full | Partial | **Planned P3 Full; delivered Partial:** restoring detached panels on a second monitor after a restart was not verified. In-window docking in P1. Detach to OS windows in P3 (ADR-0013) |
| HUD / Ambient | §34 | 3 | apps/desktop, ui | HUD | — | — | Full | Mini Manager, status, voice indicator, approvals |
| Multi-monitor | §34 | 3 | apps/desktop, platform | Detached panels | — | — | Full | Per-device layouts, monitor-aware restore |
| AI-editable UI | §29 | 3 | protocol (ui_config), core (ui.*), orchestrator | ThemeEditor, Manager | Arch | Arch | Partial | **Planned P3 Full; delivered Partial:** contrast-lowering patches are not rejected, no `--safe-ui` recovery, device overrides not patchable. P1: schema, `ui.applyPatch` with validation and versions (user-driven). P3: Manager-proposed patches with preview (ADR-0015) |
| Sound Engine | §33 | 3 | ui, core::voice | Settings | — | — | Full | Wake, start, completion, error, approval, deploy and alert sounds |
| Voice per agent | §33 | 3 | domain (VoiceProfile), core::voice | Agents, RobotDesigner | Arch | Arch | Full | Field exists from P1 (RR-C1) |
| Performance Mode | §31 | 3 | ui, apps/desktop, core::voice | Settings, TitleBar | Partial | Partial | Full | P1: Balanced and Performance. P3: Ultra Visual and Battery Saver wired to robots and wake word |
| Windows-first + macOS/Linux | §2, §46, §47 | 3 | platform, all | — | Partial | Partial | Partial | **Planned P3 Full; delivered Partial:** Windows only, no macOS / Linux packages. P1/P2: Windows shipped, macOS/Linux compile and test in CI (RR-C9). P3: macOS and Linux packages pass the demo script |
| Web Client | §2, §35 | 4 | client, protocol, ui, (apps/web) | — | Arch | Arch | Arch | Protocol prerequisites RR-P1…P10 in P1 |
| Android | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client |
| iOS | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client |
| Secure remote access | §35–36 | 4 | core (session/auth), security, (gateway) | Settings | Arch | Arch | Arch | Opaque token → `Session{actor, deviceId, scopes}` from P1 (RR-P1). Gateway, MFA and passkeys in P4 |
| Headless AI Node | §12, §38 | 4 | core (`orkestrd`), node | — | Partial | Partial | Partial | **The P2 hand off to the daemon is not built** (M2.7); manual `orkestrd` (P1) and autostart (P3) remain. P1: manual `orkestrd`. P2: handoff. P3: autostart. P4: node behind the gateway |
| Multi-computer cluster | §38 | 6 | node, orchestrator | Nodes | Arch | Arch | Arch | `nodeId` on events and entities from P1 (RR-P4) |
| Resource-aware scheduling | §11, §38 | 6 | task-engine, node | Tasks, MissionControl | Arch | Full | Full | Full for the local node in P2. Cross-node in P6 |
| Task migration / checkpointing | §39 | 6 | task-engine, agent-runtime, snapshots | Tasks | Partial | Partial | Partial | P1: checkpoint and handoff record used for crash recovery (ADR-0022). P6: migration between nodes |
| Cloud Workspace | §40 | 4 | sync, storage | Settings | Arch | Arch | Arch | `SyncBackend` trait, syncable flags (ADR-0019) |
| Local Cache | §40 | 4 | storage, sync | — | Arch | Arch | Arch | The local DB and object store are the cache when cloud mode arrives |
| Continuous Sync | §42 | 4 | sync, snapshots, storage | Settings | Arch | Arch | Arch | `rev`/`origin_device_id` columns, content-addressed objects (ADR-0019) |
| Git as a separate layer | §40–41 | 1 | vcs, snapshots | Git | Full | Full | Full | Snapshots ≠ commits, no auto-commit on user branches (RR-C5) |
| Self-Hosted | §45 | 4 | storage, sync, security | Settings | Arch | Arch | Arch | `MetadataStore`/`ObjectStore`/`SecretStore`/`SyncBackend` traits |
| Local-Only | §45 | 1 | sync (LocalOnly), storage | Onboarding, Settings | Full | Full | Full | The only storage mode in P1–3 (RR-A8) |
| Offline-first | §44, §63I | 4 | all | — | Partial | Partial | Partial | P1–3: editor, Git, terminal, Ollama and snapshots work offline (no network dependency in core paths). P4: sync queue |
| Cross-device continuation | §43, §63C | 4 | sync, core | — | Arch | Arch | Arch | Resume summary and handoff model reused |
| Plugin SDK | §48 | 7 | (plugin host), security (`Actor::Plugin`) | — | Arch | Arch | Arch | `Plugin` entity and actor kind. Manifest and sandbox per ADR-0034 |
| Widget SDK | §48 | 7 | ui, (plugin host) | — | — | — | — | Phase 7. Widgets get no filesystem or secrets access by default |
| Dashboard Builder | §48 | 7 | ui | — | — | — | — | Phase 7. MissionControl tile model (P3) is the precursor |

## C. Cross-cutting requirements (spec sections outside §68 that also must not be lost)

| Requirement | Spec | P1 | P2 | P3 | Where |
|---|---|---|---|---|---|
| Emergency controls (stop agent/project/all, disable network tools, lock vault, disable full auto) | §52 | Full | Full | Full | security, core, TitleBar (RR-C3) |
| Audit log | §23, §24, §52 | Full | Full | Full | security (`audit_events`) |
| Observability (structured logs, correlation ids, redaction) | §50 | Full (logs) | Full | Full | events, tracing, Logs panel; OTLP in P4 |
| Privacy policies and classification | §56 | Partial | Full | Full | security, providers router (RR-G5); `Project::effective_privacy` (classification tightens the AI/sync policy), Settings → Project Privacy |
| Session resume and handoff | §57 | Partial | Full | Full | core recovery, orchestrator (ADR-0022) |
| Quality gates and DoD | §58 | Arch | Full | Full | orchestrator |
| Command palette | §54 | Partial | Full | Full | apps/desktop (RR-G6) |
| Global search | §54 | — | Full | Full | memory FTS, core `search.query` |
| Onboarding | §55 | Partial (steps 1–8) | Partial | Partial (+9 voice) | apps/desktop (RR-G7); 10–12 in Phases 4–5 |
| No hidden chain-of-thought stored | §1.14, §21 | Full | Full | Full | providers (no reasoning variant), RR-A5 |

## Phase 2 acceptance check (2026-10-06)

The P2 column in sections A and B now shows the level delivered at the end of
Phase 2, and the planned level is kept in the Notes where the two differ. The table below
shows what `scripts/smoke/phase2-acceptance.mjs` verified in the running app
(Windows, Mock provider, 47/47 checks pass). It also lists P2 targets that are
not built or not verified. Details are in `mvp-milestones.md` → Phase 2 Status.

| # | Feature | Verified in the app | Not built / not verified |
|---|---|---|---|
| 2, 24 | Dynamic spawn, parallel execution | Parallel agents in separate worktrees (≤ `maxParallelAgents`) | 6-agent load, overlap hold-back and CPU throttle not measured |
| 3, 4 | Teams, hierarchy | Team created and listed; built-in team templates | Lead delegation not exercised by this script (unit-tested) |
| 6 | Model fallback | 429 `insufficient_quota` → `ProviderFallback` → handoff → fallback finishes; toast shown | — |
| 11 | AI Merge Manager | Run-wide proposal, DAG order, approval, merged into `main` on disk; superseded per-task proposals | Revert proposals still open |
| 12, 15, 16 | Review, auto testing, Self-Healing | Real `npm run build/test` (Windows), parsed `node --test` failure, healed in 1 attempt; stop on "no progress" → escalation → declined → Failed | Browser tool (stretch) not built |
| 13 | Consensus | Decision with 3 alternatives | Only one model available (perspectives, not providers) |
| 34, 35 | Rollback | Rollback Task across healing worktrees; unrelated edits survive | Merged-task revert, rollback from Replay; memory-item and permission-rule versions **not built** |
| 38, 39 | Benchmarks, agent analytics | Rows from the real runs | — |
| 40 | Context optimizer | — | ≥ 90 % context-pack eval not run |
| 41 | MCP Hub | — | **Not built** (no stdio MCP client) |
| 43 | Environment Manager | — | **Not built** (no Dev/Test/Staging/Prod) |
| 47 | AI Product Manager | Draft → approve → 4 tasks + Manager plan | — |
| 48 | Architecture Map | Non-empty map of the temp repo | — |
| 49, 50 | Decision Log, Replay | Plan → agents → tools → files → tests → reviews → approvals → merge | — |
| 20, 26 | Monaco LSP, background hand-off | — | **Not built** (no LSP bridge; no hand off to `orkestrd`) |

## D. Summary

| Level | P1 | P2 | P3 |
|---|---|---|---|
| Full | 8 | 32 | 42 |
| Partial | 31 | 20 | 21 |
| Arch | 27 | 18 | 16 |
| — | 16 | 12 | 3 |

Counts cover sections A and B (82 items) and use the delivered levels
(reconciled 2026-10-07). Before that reconciliation, the counts were taken from
the planned levels (P1/P2/P3 Full 10/36/57) and overstated what had shipped.
