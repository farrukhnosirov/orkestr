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
| 34 | Rollback Everything | Partial | Partial | Task and workspace rollback work for every agent task (after-checkpoints are now always taken); HEAD/index restore is open. |
| 35 | Snapshots | Full | Partial | Capture, diff, restore and task rollback work; snapshot GC and the 500-file timing benchmark are open. |
| 37 | Smart Budgeting | Partial | Partial | Budgets are checked before each API model call; CLI agents are checked after the run only. |
| 49 | Decision Log | Arch | Partial | Decisions are recorded through the Manager's `record_decision` tool and the Memory panel; plan approvals do not record one automatically. |
| — | Background service | Partial | Partial | `orkestrd` + tray keep-running; no Job Object supervisor, so child processes of a hard-killed core are not reaped by the OS. |
| — | Installer / CI | (M1.1, M1.8) | — | No signed installer, auto-update or CI matrix yet. |

**Crate names** drop the `orkestr-` prefix. "client" means `@orkestr/client`, "ui" means `@orkestr/ui`.

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
| 9 | Knowledge Base | §20 | 2 | memory (KnowledgeSource), context | Memory (Knowledge tab) | Arch | Partial | Full | P1: `KnowledgeSource` entity. P2: Markdown/text/API docs ingest with FTS. P3: PDF and external docs, relevance in the context pack |
| 10 | Git Worktrees per agent | §16 | 1 | vcs, orchestrator, agent-runtime | Git, Agents | Full | Full | Full | ADR-0009. Merge Manager is #11 |
| 11 | AI Merge Manager | §16 | 2 | orchestrator (merge), vcs | Git, Approvals, Tasks | Arch | Full | Full | P1: `merge_preview` and conflict detection in vcs, manual merge by the user. P2: proposals, gates, policy auto-merge, revert proposals (P2 as built: `merge.*` + Git panel → Merge Manager; revert proposals open — see git-worktree-strategy §6.5) |
| 12 | Automatic Code Review | §17 | 2 | orchestrator, agent-runtime | Tasks, Code (diff), Agents | Partial | Full | Full | P1: user or Manager can assign a Reviewer agent to a task's diff; `ReviewFinished`. P2: automatic pipeline step and a quality gate |
| 13 | Consensus Mode | §8 | 2 | orchestrator, providers, memory (decisions) | Manager, Decisions | — | Full | Full | P2: N-model proposals, synthesis, alternatives stored (ADR-0027) |
| 14 | Adversarial Review | §8 | 2 | orchestrator, agent-runtime | Tasks, Decisions | — | Full | Full | Reviewer that cannot auto-agree; findings as structured items |
| 15 | Auto Testing | §17 | 2 | orchestrator, terminal, agent-runtime | Tasks, Logs | Partial | Full | Full | P1: agents run test commands through `run_command`; results recorded. P2: auto-detection of unit/integration/E2E/lint/typecheck/build/smoke, `TestRunFinished`, gates |
| 16 | Self-Healing Loop | §17 | 2 | orchestrator, task-engine, cost | Tasks, Logs | Arch | Full | Full | P1: hard limits (maxTurns, time, cost) that the loop will use. P2: Implement → Build → Test → Analyze → Fix loop |
| 17 | Visual Browser Agent | §18 | 3 | agent-runtime (browser tool), security, platform | Browser | Arch | Partial | Full | P1: `Browser` capability. P2 (stretch): Playwright-driven tool (open, click, type, screenshot, console and network errors). P3: Browser panel with live screenshots and responsive checks (RR-G12) |
| 18 | Computer Control | §18 | 7 | platform (ComputerControl trait), security | Settings (permission) | Arch | Arch | Arch | Capability exists and is **Deny by default** (ADR-0010). Platform trait stubs return `unimplemented`. SP-22 |
| 19 | Terminal Workspace | §15 | 1 | terminal, platform, security | Terminal | Full | Full | Full | ADR-0014. Phase 4: remote terminal through the gateway |
| 20 | Monaco | §15 | 2 | apps/desktop, core (files/LSP) | Code | Partial | Full | Full | P1: tabs, syntax, built-in TS/JSON/CSS/HTML services, search, diff, Git decorations, agent change markers, open in external IDE. P2: LSP bridge (RR-A6) and inline AI actions |
| 21 | Project Desktop | §14 | 4 | apps/desktop, ui | all §14 panels | Partial | Partial | Partial | P1: Manager, Code, Files, Terminal, Git, Agents, Tasks, Logs, Memory, Costs (+ Approvals, Snapshots, Providers). P2: + Decisions, Replay, Environments. P3: + Architecture, Browser, MissionControl. Database, Deployments and Monitoring (beyond local) are Phase 4. Placeholders show "Arrives in Phase N" (`PhasePlaceholder`; the phase numbers in `apps/desktop/src/panels/registry.ts` currently differ from this row for Architecture, Browser, Database and Deployments — see the note under the panel list) |
| 22 | Mission Control | §32 | 3 | ui, core (aggregation methods), node, cost | MissionControl | — | — | Full | P3 scope: projects, agents, task graph, local node resources, provider health, limits, spend, approvals, builds, alerts. Deployments and remote nodes tiles fill in during Phases 4 and 6 |
| 23 | Visual Task Graph | §11 | 3 | task-engine, apps/desktop (@xyflow/react) | TaskGraph, Tasks | Partial | Partial | Full | P1: read-only DAG view. P2: live status overlay. P3: interactive editing (link, reprioritize, create) |
| 24 | Parallel Execution | §1.8, §11 | 2 | task-engine (scheduler), orchestrator, vcs | Tasks, Agents | Partial | Full | Full | P1: up to `maxParallelAgents` independent tasks, simple FIFO by priority. P2: resource- and conflict-aware scheduling (M2.1) |
| 25 | Priority Queue | §11 | 2 | task-engine | Tasks | Partial | Full | Full | P1: strict classes Critical…Background. P2: aging and preemption of Background (RR-A13) |
| 26 | Background Agents | §12 | 3 | core, platform (autostart), apps/desktop (tray) | Tray, Settings | Partial | Partial | Full | P1: tray keep-running and manual `orkestrd`. P2: hand off to the daemon. P3: autostart opt-in (ADR-0004, RR-C2) |
| 27 | Cross-project Agents | §68 | 2 | orchestrator, security | Manager, Agents | Arch | Partial | Partial | Linked instances across projects with a `crossProjectGroupId`, each keeping its own project scope (RR-A14). Full cross-project Manager tasks in Phase 4 (multi-device context). **P2 (teams-pm):** a team can be assigned to several projects; each assignment carries a permission scope installed as agent-scope `Deny` rules on every team agent (secrets denied unless allowed); `Manager::with_team_secret` never resolves another project's or a global secret (tested) |
| 28 | Voice Command Center | §33 | 3 | core::voice, platform (Microphone, hotkeys), providers (STT/TTS) | HUD, Manager, Settings | Arch | Arch | Full | P1: `voice.*` namespace stubs, `Microphone` trait, `VoiceProfile` on templates (RR-C1). P3: PTT, STT, intents (ADR-0017) |
| 29 | Wake Word | §33 | 3 | core::voice, platform | Settings, HUD | Arch | Arch | Full | Local only, opt-in, VAD-gated (ADR-0017, SP-15) |
| 30 | Voice Notifications | §33 | 3 | core::voice, events (Notification) | Settings | — | — | Full | TTS on `Notification` with DND windows |
| 31 | Approval Levels | §23 | 1 | security, task-engine | Approvals, Settings, TitleBar | Full | Full | Full | Ask, SemiAuto, FullAuto + Disable Full Auto (ADR-0010). Phase 4: remote approvals; Phase 5: mobile approvals |
| 32 | Permission Sandbox | §23 | 2 | security, platform, terminal | Settings (Permissions), Approvals | Partial | Full | Full | P1: capability engine, scopes, worktree/path confinement, network deny switch. P2: OS-level process sandbox where practical (SP-17 → ADR) and containers for Docker-capable tasks |
| 33 | Dangerous Action Guard | §23 | 1 | security | Approvals | Full | Full | Full | ≥ 150 test cases; SQL/git/shell classification (ADR-0010) |
| 34 | Rollback Everything | §25 | 2 | snapshots, security (rules versions), protocol (ui config versions), memory | Snapshots, Settings, ThemeEditor | Partial | Full | Full | P1: task and workspace rollback, UI config versions, permission rule versions. P2: merged-task revert proposals, memory versions, rollback from Replay. Remote side effects are not reversible (RR-A4) |
| 35 | Snapshots | §25 | 1 | snapshots, storage (objects), vcs | Snapshots | Full | Full | Full | ADR-0021. P2 adds automatic checkpoints in loops. P4 syncs snapshots |
| 36 | Cost & Limit Center | §22 | 2 | cost, providers | Costs | Partial | Full | Full | P1: usage and estimated cost per provider/model/agent/project/task, daily and monthly budgets. P2: provider quota/limit state, forecasts. P3: Mission Control tiles |
| 37 | Smart Budgeting | §22 | 2 | cost, orchestrator | Costs, Settings | Partial | Full | Full | P1: per-task max, per-project monthly, "unrestricted" scopes, `BudgetThreshold` (RR-G4). P2: budget-aware routing and spawning |
| 38 | Model Benchmarking | §22 | 2 | cost (benchmarks), orchestrator | Costs (Benchmarks tab) | Arch | Full | Full | P1: usage and outcome records captured with the fields needed. P2: first-pass success, build success, defects, retries, latency, cost |
| 39 | Agent Performance Analytics | §22 | 2 | cost (analytics) | Agents, Costs | Arch | Full | Full | Same capture-in-P1, report-in-P2 split |
| 40 | Context Optimizer | §20 | 2 | context, memory, vcs | Manager (context preview) | Partial | Full | Full | P1: task, referenced files, project rules, relevant memory (FTS), token budget, concise handoff. P2: tree-sitter symbols, relevance ranking, dependency closure (ADR-0024) |
| 41 | MCP Hub | §19 | 7 | agent-runtime (MCP client), security, core | McpHub | — | Partial | Partial | P2: stdio MCP client, servers configured per project, capability gating (CA-4). P3: McpHub panel (status, tools, permissions, secrets, projects). P7: catalogue and community servers |
| 42 | Secrets Vault | §24 | 1 | security, platform | Settings (Secrets), Environments | Full | Full | Full | Keyring + encrypted vault, scopes, audit, Lock Vault (ADR-0011). Self-hosted and cloud secret providers in Phase 4 |
| 43 | Environment Manager | §26 | 4 | domain, security, core | Environments | Arch | Partial | Partial | P1: `Environment` entity (Local) for secret scoping. P2: Dev/Test/Staging/Prod definitions, prod flag feeding the guard. P4: remote environments, DB connections |
| 44 | Deployment Agents | §26 | 4 | orchestrator, security (Deploy), platform | Deployments | Arch | Arch | Arch | `Deploy` capability (always Ask), `Deployment` entity, `DeploymentFinished` event. Providers (Docker, Compose, VPS/SSH, Vercel, Cloudflare, clouds) in Phases 4 and 7 |
| 45 | Live Infrastructure Monitor | §27 | 4 | node (sysinfo), core | Monitoring, MissionControl | Arch | Partial | Partial | P2: local node CPU/RAM/GPU/disk via `NodeStatus`. P3: shown in Mission Control. P4: services, containers, health checks, remote nodes |
| 46 | Bug Detective | §27 | 4 | orchestrator, memory, vcs | Logs, Tasks | — | — | — | Builds on #45, #15 and #16 plus production log access (Phase 4). No P1–3 seam beyond the existing Log and Task models |
| 47 | AI Product Manager | §9 | 2 | orchestrator (planning), task-engine | Manager, Tasks | Partial | Full | Full | P1: Manager plan → tasks with acceptance criteria and dependencies → approve → run. P2: Idea → Epic → Features → Stories → AC → Tasks hierarchy. **P2 done (teams-pm):** `pm.*` RPC — structured, validated draft (one repair round), editable in Tasks → Product, approval creates the task hierarchy with dependencies and an approved Manager plan (optional run / team); Manager composer "Product Manager mode" |
| 48 | Architecture Map | §21 | 3 | context (symbol graph), memory | Architecture | — | Partial | Full | P2: modules, dependencies and APIs extracted from code and memory. P3: Architecture panel visual map. **As built (Phase 2):** new crate `orkestr-insight` (`arch`): Cargo/npm/Python/Go/.NET/dir modules, binaries/Dockerfiles/compose services, compose DBs/SQLite/migrations/ORM schemas, axum/actix/express/nest/fastapi/flask/aspnet/gin/net-http routes + OpenAPI, manifest + import edges, grouped externals; incremental per-file cache, persisted snapshots (`architecture_snapshots`, 50/project), diffs, debounced auto-refresh after `FileChanged`; RPC `architecture.{map,refresh,diff}`, event `ArchitectureMapUpdated`; Architecture panel (React Flow map + list, search, kind filters, details → open in Code, change highlighting, SVG export) already shipped |
| 49 | Decision Log | §21 | 2 | memory (decisions), events | Decisions | Arch | Full | Full | P1: `Decision` entity, `DecisionRecorded`, created by Manager plan approvals. P2: Decisions panel, consensus alternatives, links to tasks and files |
| 50 | Execution Replay | §21 | 2 | events (query), core | Replay, Logs | Partial | Full | Full | P1: durable event log with correlation ids, Logs filter by correlation. P2: Replay timeline across request → plan → agents → tools → files → tests → reviews → approvals → merge. **As built (Phase 2):** `orkestr-insight::replay` (identity + correlation relevance, bounded scan, agent/phase filters, state reconstruction), RPC `replay.{runs,timeline,step}`, Replay tab in the Logs panel (run/task picker, agent and phase filters, play/pause/step with speed, step details, state at cursor, files open in Code) |

## B. Additional inventory (§68 "Также")

| Feature | Spec | Target phase | Crates | UI panel(s) / surface | P1 | P2 | P3 | Notes |
|---|---|---|---|---|---|---|---|---|
| Custom Visual OS | §28 | 3 | apps/desktop, ui | TitleBar, all | Partial | Partial | Full | P1: frameless, custom controls, tokens, blur/glow in Balanced. P3: full effects, transparency, floating windows |
| Floating robots | §30 | 3 | apps/desktop (src-tauri overlay), platform | Robot windows | — | — | Full | ADR-0016, SP-04 |
| Robot Designer | §30 | 3 | ui, protocol (ui_config.robots) | RobotDesigner | — | — | Full | Skins, scale, animations, voice, sounds, behaviour |
| Theme Engine | §29 | 3 | ui, protocol (ui_config) | ThemeEditor, Settings | Partial | Partial | Full | P1: token engine, dark/light built-ins, schema, versions. P3: full editor |
| Layout Editor | §29 | 3 | ui, apps/desktop (dockview) | LayoutEditor | Partial | Partial | Full | P1: drag, resize, hide and presets (Coding, Focus). P3: editor plus Mission Control and Monitoring presets |
| Docking | §28 | 1 | apps/desktop (dockview) | all panels | Full | Full | Full | In-window docking in P1. Detach to OS windows in P3 (ADR-0013) |
| HUD / Ambient | §34 | 3 | apps/desktop, ui | HUD | — | — | Full | Mini Manager, status, voice indicator, approvals |
| Multi-monitor | §34 | 3 | apps/desktop, platform | Detached panels | — | — | Full | Per-device layouts, monitor-aware restore |
| AI-editable UI | §29 | 3 | protocol (ui_config), core (ui.*), orchestrator | ThemeEditor, Manager | Arch | Arch | Full | P1: schema, `ui.applyPatch` with validation and versions (user-driven). P3: Manager-proposed patches with preview (ADR-0015) |
| Sound Engine | §33 | 3 | ui, core::voice | Settings | — | — | Full | Wake, start, completion, error, approval, deploy and alert sounds |
| Voice per agent | §33 | 3 | domain (VoiceProfile), core::voice | Agents, RobotDesigner | Arch | Arch | Full | Field exists from P1 (RR-C1) |
| Performance Mode | §31 | 3 | ui, apps/desktop, core::voice | Settings, TitleBar | Partial | Partial | Full | P1: Balanced and Performance. P3: Ultra Visual and Battery Saver wired to robots and wake word |
| Windows-first + macOS/Linux | §2, §46, §47 | 3 | platform, all | — | Partial | Partial | Full | P1/P2: Windows shipped, macOS/Linux compile and test in CI (RR-C9). P3: macOS and Linux packages pass the demo script |
| Web Client | §2, §35 | 4 | client, protocol, ui, (apps/web) | — | Arch | Arch | Arch | Protocol prerequisites RR-P1…P10 in P1 |
| Android | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client |
| iOS | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client |
| Secure remote access | §35–36 | 4 | core (session/auth), security, (gateway) | Settings | Arch | Arch | Arch | Opaque token → `Session{actor, deviceId, scopes}` from P1 (RR-P1). Gateway, MFA and passkeys in P4 |
| Headless AI Node | §12, §38 | 4 | core (`orkestrd`), node | — | Partial | Partial | Partial | P1: manual `orkestrd`. P2: handoff. P3: autostart. P4: node behind the gateway |
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
| Privacy policies and classification | §56 | Partial | Full | Full | security, providers router (RR-G5) |
| Session resume and handoff | §57 | Partial | Full | Full | core recovery, orchestrator (ADR-0022) |
| Quality gates and DoD | §58 | Arch | Full | Full | orchestrator |
| Command palette | §54 | Partial | Full | Full | apps/desktop (RR-G6) |
| Global search | §54 | — | Full | Full | memory FTS, core `search.query` |
| Onboarding | §55 | Partial (steps 1–8) | Partial | Partial (+9 voice) | apps/desktop (RR-G7); 10–12 in Phases 4–5 |
| No hidden chain-of-thought stored | §1.14, §21 | Full | Full | Full | providers (no reasoning variant), RR-A5 |

## D. Summary

| Level | P1 | P2 | P3 |
|---|---|---|---|
| Full | 10 | 36 | 57 |
| Partial | 28 | 19 | 9 |
| Arch | 30 | 17 | 13 |
| — | 14 | 10 | 3 |

Counts cover sections A and B (82 items).
