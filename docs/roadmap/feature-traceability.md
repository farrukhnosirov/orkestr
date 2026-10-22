# Feature Traceability Matrix (§68: "must not be lost")

Every §68 item, the 50 numbered ones plus the additional list, is mapped to its target phase, owning crate(s), UI panel(s) and delivery level in Phases 1–3. A level marked here as **Full** or **Partial** is a commitment checked at the milestone exit (see `mvp-milestones.md`).

**Final state after the Phase 0–3 audit (2026-10-07).** The P1, P2 and P3
columns below are the delivered levels, checked against the code in two audit
rounds (157 confirmed findings, all fixed; see
[`../audit/phase0-3-audit.md`](../audit/phase0-3-audit.md)). At the end of
Phase 3, of the 82 items in sections A and B, **41 are Full, 22 Partial and
16 architecture-only, and 3 not addressed** (Section D). The largest
gaps against the plan:
- M2.7 was not built at all: MCP Hub (#41) and Environment Manager (#43) are
  Arch, and the Monaco LSP bridge (#20) and the `orkestrd` hand-off (#26,
  Headless AI Node) are missing.
- The Visual Browser Agent (#17) is deferred to Phase 4.
- Rollback Everything (#34) has no memory or permission-rule versions.
- Voice (#28, #29) has no local Whisper, and a real microphone was never
  exercised.
- Only Windows is supported; there are no macOS or Linux packages.
- Several performance exit criteria were never measured.

All acceptance and smoke scripts pass on the final build: Phase 1 45/45,
Phase 2 47/47 and Phase 3 64/64.

**Delivery levels**
- **Full**: the user-visible capability is complete for the scope the spec describes in that phase.
- **Partial**: real, working subset; the remaining scope is named in Notes.
- **Arch**: architecture-only. The entities, traits, protocol methods/events and capability names exist, and the seams are tested. Calls return the explicit unimplemented-feature error (RR-G14; as built `bad_request` with `data.unimplementedFeature`, or `PlatformError::NotSupported` inside the platform layer). There is no fake behaviour.
- **—**: not addressed in that phase. No seam is needed yet, or the seam belongs to a later phase.

**Panels** are `apps/desktop/src/panels/<PanelName>` (contract §8). Panel names: Manager, Code, Files, Terminal, Git, Agents, Tasks, TaskGraph, Approvals, Logs, Memory, Costs, Snapshots, Providers, Settings, MissionControl, Architecture, Browser, Database, Deployments, Monitoring, Decisions, Replay, McpHub, Environments, ThemeEditor, LayoutEditor, RobotDesigner, Nodes. Shell-level UI (not dockable panels) is TitleBar, CommandPalette, Onboarding, Tray, HUD and the robot windows.

**As built (Phase 0, 2026-10-02):** `apps/desktop/src/panels/registry.ts` registers 15 panels — `manager`, `code`, `files`, `terminal`, `git`, `agents`, `tasks`, `architecture`, `browser`, `database`, `logs`, `deployments`, `memory`, `costs`, `monitoring`. Only **Logs** is functional (live event stream); the others render `PhasePlaceholder` ("Arrives in Phase N"). The shell-level TitleBar, CommandPalette, ProjectRail, StatusBar and ConnectionScreen exist. Panels named above but not registered yet (TaskGraph, Approvals, Snapshots, Providers, Settings, MissionControl, Decisions, Replay, McpHub, Environments, ThemeEditor, LayoutEditor, RobotDesigner, Nodes) are added by their phase owners. The registry's placeholder phases disagreed with this matrix for Architecture, Browser, Database and Deployments. Aligned in audit r2: placeholder phases now come from `apps/desktop/src/panels/plannedPhases.ts` (Browser, Database and Deployments: Phase 4; Architecture shipped in P2/P3).

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
| 17 | Visual Browser Agent | Full | — | Not built: no browser tool and no Browser panel (placeholder only). Deferred to Phase 4; the matrix row was downgraded in audit r2 and the Coding / Mission Control presets no longer spread the placeholder onto monitor 3. |
| 22 | Mission Control | Full | Partial | Widgets cover the planned set: projects, agents, task graph, local node CPU/RAM/GPU, provider health, limits, spend, approvals, quality builds/tests, reviews & merges, alerts. Live from events, with the arrangement saved per device. Missing: the 200-node / 50 fps exit check was not measured, and the charts poll on the performance-policy interval. |
| 23 | Visual Task Graph | Full | Partial | Interactive graph (link, delete dependency, open) in Tasks and the Mission Control widget. Reprioritize-by-drag and the 200-node performance bar were not verified. |
| 26 | Background Agents | Full | Partial | Tray keep-running and the autostart entry date from P1. The reboot check of M3.5 was not run. |
| 28 | Voice Command Center | Full | Partial | Scenario G passes through the post-STT path (`simulateTranscript` seam, TTS called), and typed HUD commands work, e.g. «show build errors» answered from the quality runner. Push-to-talk and destructive-intent confirmations work. STT is WebSpeech or OpenAI Whisper (cloud, consent-gated); **no local Whisper**. A real microphone was not exercised in acceptance. |
| 29 | Wake Word | Full | Partial | Local MFCC/DTW spotter with enrollment, opt-in, VAD-gated, paused in Battery Saver. SP-15 budgets and the zero-network packet capture were not measured. |
| 30 | Voice Notifications | Full | Full | Spoken notifications with DND / quiet hours. Robots with voice on speak for their agent, with no double announcement. |
| 34 | Rollback Everything | Full | Partial | UI part: every UI config change is versioned; AI patches carry an exact inverse patch, and Undo is verified. Scenario J was not re-run on the Phase 3 build. Memory-item and permission-rule versions are **not built** (hard delete, in-place update, no restore RPC), so the item as a whole is Partial. |
| 45 | Live Infrastructure Monitor | Partial | Partial | Local node resources shown in Monitoring and Mission Control.. **P7 (as built, ADR-0042):** per-node metric rings (local + enrolled nodes), HTTP/TCP health checks with hysteresis, log sources (file, deployment, ingest; memory-only, redacted), alert rules with dedup + defaults; Monitoring → Infrastructure. No GPU utilisation, no RPC/UI for rules and sources |
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
| 1 | Agent Marketplace | §6, §48 | 7 | agent-runtime, protocol, (plugin host P7) | Agents (Templates tab) | Partial | Partial | Partial | P1–3: local template import/export (JSON manifest with version and permissions). Official/community/private catalogue and signing are Phase 7. **P7 (as built, ADR-0040):** signed static registries — bundled official catalogue (embedded key), local directory, self-hosted gateway registry (`/registry/*`); publisher key TOFU + per-package pin + revocation; search / install / uninstall of templates, plugins and widgets; local publishing with a vault-held key; Marketplace panel (Browse / Installed / Sources / Keys) |
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
| 17 | Visual Browser Agent | §18 | 4 | agent-runtime (browser tool), security, platform | Browser | Arch | Arch | Arch | **Planned P2 Partial (stretch), P3 Full; delivered Arch — deferred to Phase 4** (audit r2): no browser agent tool was built in Phase 2, and the Browser panel is a placeholder ("Arrives in Phase 4") that no layout preset places. P1: `Browser` capability. P2 (stretch, not built): Playwright-driven tool (open, click, type, screenshot, console and network errors). Phase 4: the browser tool and the Browser panel with live screenshots and responsive checks (RR-G12) |
| 18 | Computer Control | §18 | 7 | platform (ComputerControl trait), security | Settings (permission) | Arch | Arch | Arch | Capability exists and is **Deny by default** (ADR-0010). Platform trait stubs return `unimplemented`. SP-22 |
| 19 | Terminal Workspace | §15 | 1 | terminal, platform, security | Terminal | Partial | Partial | Partial | **Planned Full; delivered Partial:** PTY sessions, gated runs and the agent command tab work; take-control handover and scrollback snapshots are not built. ADR-0014. Phase 4: remote terminal through the gateway |
| 20 | Monaco | §15 | 2 | apps/desktop, core (files/LSP) | Code | Partial | Partial | Partial | **Planned P2/P3 Full; delivered Partial:** no LSP bridge (RR-A6, M2.7 not built) and no inline AI actions; the P1 editor features are what ships. P1: tabs, syntax, built-in TS/JSON/CSS/HTML services, search, diff, Git decorations, agent change markers, open in external IDE. P2: LSP bridge (RR-A6) and inline AI actions |
| 21 | Project Desktop | §14 | 4 | apps/desktop, ui | all §14 panels | Partial | Partial | Partial | P1: Manager, Code, Files, Terminal, Git, Agents, Tasks, Logs, Memory, Costs (+ Approvals, Snapshots, Providers). P2: + Decisions, Replay, Environments. P3: + Architecture, Browser, MissionControl. Database, Deployments and Monitoring (beyond local) are Phase 4. Placeholders show "Arrives in Phase N" (`PhasePlaceholder`, phase from `apps/desktop/src/panels/plannedPhases.ts`). Browser was deferred from P3 to Phase 4 (see #17); built-in layout presets never place placeholder panels |
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
| 43 | Environment Manager | §26 | 4 | domain, security, core | Environments | Arch | Arch | Arch | **Planned P2/P3 Partial; delivered Arch:** the `Environment` entity has Dev/Test/Staging/Production kinds and a protected flag, but there is no environment manager, RPC or Environments panel (M2.7 not built). P1: `Environment` entity (Local) for secret scoping. P2: Dev/Test/Staging/Prod definitions, prod flag feeding the guard. P4: remote environments, DB connections. **P7:** `environments.*` RPC (target + non-secret adapter settings, production always protected) in the Deployments panel (ADR-0041) |
| 44 | Deployment Agents | §26 | 4 | orchestrator, security (Deploy), platform | Deployments | Arch | Arch | Arch | `Deploy` capability (always Ask), `Deployment` entity, `DeploymentFinished` event. Providers (Docker, Compose, VPS/SSH, Vercel, Cloudflare, clouds) in Phases 4 and 7. **P7 (as built, ADR-0041):** StaticSite + LocalProcess live, Docker gated on PATH, Vercel / Netlify / Fly / Kubernetes request-shape verified against a fake API; guarded production deploys, redacted logs, rollback; Deployments panel. Deferred: AWS / GCP / Azure / Cloudflare, live cloud runs, container sandbox |
| 45 | Live Infrastructure Monitor | §27 | 4 | node (sysinfo), core | Monitoring, MissionControl | Arch | Partial | Partial | P2: local node CPU/RAM/GPU/disk via `NodeStatus`. P3: shown in Mission Control. P4: services, containers, health checks, remote nodes |
| 46 | Bug Detective | §27 | 4 | orchestrator, memory, vcs | Logs, Tasks | — | — | — | Builds on #45, #15 and #16 plus production log access (Phase 4). No P1–3 seam beyond the existing Log and Task models. **P7 (as built, ADR-0042):** incidents from alert / log / test failure / manual; evidence collectors, schema-validated hypothesis, reproduction, fix on `detective/<id>` behind two approval gates, budget cap, no auto-merge; Bug Detective panel. Deferred: APM ingestion, GitHub PRs |
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
| Multi-monitor | §34 | 3 | apps/desktop, platform | Detached panels | — | — | Partial | **Planned P3 Full; delivered Partial** (final audit, 2026-10-07): per-device layouts and popouts are persisted with monitor ids (verified by `phase3-layout-screenshots.mjs`), but restore on a second monitor after a restart was never verified (single-monitor host), the same gap as the Docking row |
| AI-editable UI | §29 | 3 | protocol (ui_config), core (ui.*), orchestrator | ThemeEditor, Manager | Arch | Arch | Partial | **Planned P3 Full; delivered Partial:** contrast-lowering patches are not rejected, no `--safe-ui` recovery, device overrides not patchable. P1: schema, `ui.applyPatch` with validation and versions (user-driven). P3: Manager-proposed patches with preview (ADR-0015) |
| Sound Engine | §33 | 3 | ui, core::voice | Settings | — | — | Full | Wake, start, completion, error, approval, deploy and alert sounds |
| Voice per agent | §33 | 3 | domain (VoiceProfile), core::voice | Agents, RobotDesigner | Arch | Arch | Full | Field exists from P1 (RR-C1) |
| Performance Mode | §31 | 3 | ui, apps/desktop, core::voice | Settings, TitleBar | Partial | Partial | Full | P1: Balanced and Performance. P3: Ultra Visual and Battery Saver wired to robots and wake word |
| Windows-first + macOS/Linux | §2, §46, §47 | 3 | platform, all | — | Partial | Partial | Partial | **Planned P3 Full; delivered Partial:** Windows only, no macOS / Linux packages. P1/P2: Windows shipped, macOS/Linux compile and test in CI (RR-C9). P3: macOS and Linux packages pass the demo script |
| Web Client | §2, §35 | 4 | client, protocol, ui, (apps/web) | — | Arch | Arch | Arch | Protocol prerequisites RR-P1…P10 in P1. **P4 delivered Partial** (ADR-0036): `apps/web` — passkey/TOTP sign-in, pairing with SAS, projects, tasks, approvals with WebAuthn step-up, remote terminal (xterm), textarea editor, Web Push opt-in, en/ru; `@orkestr/client` RemoteTransport (`e2e-v1` in WebCrypto, non-extractable device key in IndexedDB). Not built: PWA signature/SRI (R-08), the remaining desktop panels |
| Android | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client. **P5 delivered Partial** (ADR-0032/0037/0038): `apps/mobile` Expo/React Native dev build, verified on the Android emulator — TOTP login, cores list, QR / deep-link pairing with SAS, monitoring + Emergency Stop + remote kill switch, approvals with biometric step-up (`orkestr-native-v1`), Manager chat + voice (core `voice.transcribe` + shared intent parser), quick review, textarea editor, devices + revoke, settings, en/ru; native push FCM HTTP v1 / APNs (mock-verified) + local notifications while connected. Not built: passkey login on mobile, on-device STT / wake word, detailed lock-screen text and approve-from-notification, non-exportable hardware keys |
| iOS | §2 | 5 | client, protocol, (apps/mobile) | — | Arch | Arch | Arch | Shared protocol and client. **P5: code shared with Android, APNs sender in the gateway (mock-verified); the app was never built or run** — needs macOS + Xcode |
| Secure remote access | §35–36 | 4 | core (session/auth), security, (gateway) | Settings | Arch | Arch | Arch | Opaque token → `Session{actor, deviceId, scopes}` from P1 (RR-P1). **P4 delivered Partial** (ADR-0030): self-hosted `orkestr-gateway`, passkeys (ES256, attestation none) + TOTP, CIK-signed tunnel, pairing (codeMac + SAS), `e2e-v1`, roles + MethodPolicy (fail closed), step-up with replay protection, revoke, kill switch, per-device rate limits, audit on gateway and core. Deferred: mTLS/CA, ACME/TLS (gateway refuses a non-loopback bind without `--behind-tls-proxy`), device-code flow, admin-device approval, DPoP refresh |
| Headless AI Node | §12, §38 | 4 | core (`orkestrd`), node | — | Partial | Partial | Partial | **The P2 hand off to the daemon is not built** (M2.7); manual `orkestrd` (P1) and autostart (P3) remain. P1: manual `orkestrd`. P2: handoff. P3: autostart. P4: node behind the gateway |
| Multi-computer cluster | §38 | 6 | node, orchestrator | Nodes | Arch | Arch | Arch | `nodeId` on events and entities from P1 (RR-P4). **P6 (as built):** enrollment by `kind: "node"` pairing with SAS + trust levels, node registry (Online/Suspect/Offline/Draining/Revoked), pull heartbeat, gateway-relayed core-to-core E2E, revoke, emergency stop on all nodes, desktop Nodes panel (ADR-0033) |
| Resource-aware scheduling | §11, §38 | 6 | task-engine, node | Tasks, MissionControl | Arch | Full | Full | Full for the local node in P2. **P6:** deterministic cross-node filter + score (os, arch, tools, GPU/VRAM, local models, trust, allowed projects, availability) with explanations stored on the task; `wait` / `infeasible` (task Blocked) |
| Task migration / checkpointing | §39 | 6 | task-engine, agent-runtime, snapshots | Tasks | Partial | Partial | Partial | P1: checkpoint and handoff record used for crash recovery (ADR-0022). **P6:** leases with epochs + fencing; checkpoints (git bundle, dirty tree, handoff, `AgentStateCheckpoint`, side-effect log) as content-addressed blobs; planned (`jobs.migrate`, drain) and unplanned (lease expiry) migration; side effects → `needs_user_decision`. Portable Claude Code / Codex session state deferred (handoff only) |
| Cloud Workspace | §40 | 4 | sync, storage | Settings | Arch | Arch | Arch | `SyncBackend` trait, syncable flags (ADR-0019). **P4 delivered Partial** (ADR-0031): end-to-end encrypted working-tree sync (FastCDC, zstd + XChaCha20, keyed chunk ids, Ed25519-signed manifests/refs, version vectors + anti-rollback) through the gateway; `workspace:` + `records:` streams. Deferred: events/snapshots/artifacts/worktree streams, recovery key, key rotation, PostgreSQL/S3 stores |
| Local Cache | §40 | 4 | storage, sync | — | Arch | Arch | Arch | The local DB and object store are the cache when cloud mode arrives |
| Continuous Sync | §42 | 4 | sync, snapshots, storage | Settings | Arch | Arch | Arch | `rev`/`origin_device_id` columns, content-addressed objects (ADR-0019). **P4 delivered Partial:** sync on demand (`sync.now`) + outbox retry loop; conflict copies with resolve (keep local / remote / both). Not built: 3-way text merge, continuous file-watch push |
| Git as a separate layer | §40–41 | 1 | vcs, snapshots | Git | Full | Full | Full | Snapshots ≠ commits, no auto-commit on user branches (RR-C5) |
| Self-Hosted | §45 | 4 | storage, sync, security | Settings | Arch | Arch | Arch | `MetadataStore`/`ObjectStore`/`SecretStore`/`SyncBackend` traits. **P4 delivered Partial:** `orkestr-gateway --data-dir … --web-dist apps/web/dist` (SQLite + filesystem object store). Not built: container image, PostgreSQL/S3 adapters, multi-instance |
| Local-Only | §45 | 1 | sync (LocalOnly), storage | Onboarding, Settings | Full | Full | Full | The only storage mode in P1–3 (RR-A8) |
| Offline-first | §44, §63I | 4 | all | — | Partial | Partial | Partial | P1–3: editor, Git, terminal, Ollama and snapshots work offline (no network dependency in core paths). **P4:** sync outbox keeps changes while the gateway is down and flushes after it returns (acceptance C) |
| Cross-device continuation | §43, §63C | 4 | sync, core | — | Arch | Arch | Arch | Resume summary and handoff model reused. **P4 delivered Partial:** Seamless Continue (`sync.continueOn` / `sync.resume`: working tree incl. uncommitted files, UI state, task summary); git history is never touched, so the other device needs the repo at the same commit |
| Plugin SDK | §48 | 7 | (plugin host), security (`Actor::Plugin`) | — | Arch | Arch | Arch | `Plugin` entity and actor kind. Manifest and sandbox per ADR-0034. **P7 (as built):** signed `.orkp`, wasmtime 50 ABI v1 sandbox (fuel, epoch, memory caps, quarantine after 3 traps), grants as permission rules, audited host API, kill switch + safe mode, agent tools `plugin_*`, Rust guest SDK (`crates/plugin-sdk`). Not wired: DeploymentProvider / PlatformIntegration runtime |
| Widget SDK | §48 | 7 | ui, (plugin host) | — | — | — | — | Phase 7. Widgets get no filesystem or secrets access by default. **P7 (as built):** `@orkestr/widget-sdk` postMessage API; iframe `sandbox="allow-scripts"` + strict CSP (no network), `orkestr-widget` URI scheme in the packaged app; Read-class binding allowlist enforced in the host and in core |
| Dashboard Builder | §48 | 7 | ui | — | — | — | — | Phase 7. MissionControl tile model (P3) is the precursor. **P7 (as built):** per-project and global dashboards on a 12-column grid with drag / resize, palette of 13 built-ins + plugin widgets, config validated against `configSchema` |

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
| Global search | §54 | — | Full | Full | palette `/` providers: projects, tasks, memory + knowledge (core FTS5 `memory.search` / `knowledge.search`), files (`files.find`); logs are searched in the Logs panel |
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

## Phase 4 acceptance check (2026-10-22)

`scripts/smoke/phase4-acceptance.mjs` — **40/40 checks pass** (self-hosted
gateway, two headless `orkestrd` cores, headless Edge with a virtual WebAuthn
authenticator driving the built web app, Node devices through
`@orkestr/client`, a mock Web Push endpoint).

| Scenario | Result | What is checked |
|---|---|---|
| D — secure remote access | Pass (26 checks + 2 setup) | Passkey register / logout / login in the web UI; wrong TOTP rejected; enrollment + tunnel; VAPID key in the tunnel welcome; web UI pairing with matching SAS lists projects over E2E; operator pairing; `projects.list`/`tasks.list` over E2E; relay sees no plaintext; local-only and viewer mutations forbidden; dangerous approval → `stepUpRequired` → resolved with a WebAuthn assertion, replay rejected; terminal forbidden without the grant, echo with it; editor save; burst → `rate_limited`; `ApprovalRequested` → Web Push (VAPID, RFC 8291 decrypts, category `approval`); revoke closes + refuses + drops push subscriptions; kill switch, no redial; gateway + core audit; `orkestrd` loopback only |
| C — encrypted sync | Pass (12 checks) | Same remote key; uncommitted + new file + UI state + task handed over (`continueOn` → `now` + `resume`), identical content, `git status` shows the change, `rev-list` unchanged; concurrent edit → one conflict with both versions on disk, resolved; tampered chunk → integrity error; offline outbox flushed after a gateway restart |

Not verified at runtime: delivery through a real push service (FCM /
Mozilla autopush — the gateway POSTs to whatever endpoint the browser gives;
only a local mock was used) and the web app on a phone.

Phase 4b (not started): M4.5/M4.6, OTLP export, the Visual Browser Agent
(#17). Native mobile push shipped in Phase 5 (below).

## Phase 5 acceptance check (2026-10-22)

`scripts/smoke/phase5-acceptance.mjs` (Node lane: temp gateway + headless
`orkestrd`, the pure-TS mobile core `apps/mobile/src/core` bundled with
in-memory adapters, `crypto.subtle` poisoned to prove the noble provider,
mock FCM (HTTP) and mock APNs (h2c)) and `scripts/smoke/phase5-android.mjs`
(Android lane: the debug APK on the `orkestr_pixel` emulator, driven through
adb + uiautomator).

<!-- P5-ACCEPTANCE -->
**Node lane 44/44 / Android lane 11/11 (emulator orkestr_pixel, incl. fingerprint step-up)** (filled in by the acceptance run).
<!-- /P5-ACCEPTANCE -->

| Area | What is checked |
|---|---|
| E1 — login and pairing | TOTP login → bearer; `/v1/cores` shows the core online; QR from `devices.startPairing` parsed; SAS equal; approved as admin; `projects.list` over E2E |
| E2 — native push | `push.registerNative` (FCM) `deliverable: true`; dangerous `ApprovalRequested` → mock FCM gets an RS256 JWT exchange, Bearer, high priority, generic notification, payload decrypts to the approval + eventId; same via mock APNs (ES256 JWT, headers) |
| E3 — biometric step-up | `approvals.resolve` → `forbidden {stepUpRequired}`; biometric gate called exactly once; native assertion resolves Approved; replay, stale counter, other device's origin and a denied gate are rejected (approval stays pending) |
| Roles | Operator resolves a non-dangerous approval without step-up; viewer cannot resolve but can `emergency.stopAll` |
| Manager / voice | `manager.chat` (mock provider) replies; "stop all agents" → intent → `emergency.stopAll`; `voice.transcribe` returns text or a named configuration error, never fake success |
| Monitoring / nodes | `dashboard.snapshot`, `nodes.list`, `nodes.resources`, `emergency.status`; second core paired → two records; `devices.revoke` with step-up; `remote.disable` from the phone drops the tunnel without redial; audit entries; DND suppresses low-priority push but not approvals |
| Android lane | APK install, deep-link pairing, TOTP in the UI, local approval of the pairing, approval raised → Orkestr local notification, approve in the app, screenshots in `docs/screenshots/phase5` |

Deferred with reasons (ADR-0032/0037/0038):

| Item | Reason |
|---|---|
| iOS build and run | Needs macOS + Xcode; the code is shared and the APNs sender exists, but nothing was built on this Windows host |
| Real FCM / APNs delivery | Needs the operator's Firebase project / Apple key; only mock servers were used (the request shapes, JWTs and payload encryption are tested) |
| Passkey login on mobile | Needs Digital Asset Links / Associated Domains bound to the gateway domain; the button is disabled with the unimplemented message (TOTP works) |
| On-device STT, mobile wake word | Transcription goes through the core's `voice.transcribe`; on-device engines and a wake-word budget are not built |
| Detailed lock-screen text, approve from the notification | Need background decryption (FCM data handler / Notification Service Extension); the alert is generic and a tap opens the app |
| Non-exportable hardware keys | Keys are SecureStore-wrapped software keys decrypted into JS memory while signing; a native Keystore/Secure Enclave module is a stretch goal |
| Biometric on the emulator | Step-up needs an enrolled fingerprint; `adb emu finger touch` may require enrolling it manually in the emulator settings first |
| DPoP, admin-device approval of new devices | Carried over from Phase 4 (ADR-0030); `devices.approve` stays local-only |
| Multi-node | Phase 6; `nodes.list` returns the local node |

## Phase 6 acceptance check

`scripts/smoke/phase6-acceptance.mjs`: one temp gateway and three headless
`orkestrd` cores, each with its own `ORKESTR_HOME` and tunnel — `win`
(coordinator, real profile; its gateway traffic goes through a recording TCP
proxy), `mac` (`fixtures/node-mac.json`: macOS / arm64, `xcodebuild` shimmed
by `fixtures/fake-xcodebuild.mjs`) and `gpu` (`fixtures/node-gpu.json`:
Linux / x86_64, RTX 4090 24 GB, CUDA). Heartbeat 500 ms, lease TTL 3 s,
condition waits only. Report: `acceptance.json`.

<!-- P6-ACCEPTANCE -->**47/47 checks passed on 2026-10-22** (C1–C9; gateway + coordinator `win` + simulated `mac` / `gpu` workers, 500 ms heartbeat, 3 s lease TTL). Regression on the same commit: `cargo test --workspace` 2449 passed / 0 failed, clippy `-D warnings` and fmt clean, npm typecheck / test (1067 + 166) / build, mobile tsc + jest 32/32, Phase 1 45/45, Phase 2 46/46 (`--no-shots`), Phase 4 40/40, Phase 5 Node lane 44/44.<!-- /P6-ACCEPTANCE -->

| Check | What is checked |
|---|---|
| C1 — setup | Gateway + account (passkey + TOTP); 3 cores tunneled and online; real GPU detection on the coordinator (informational) |
| C2 — registry | Wrong pairing code rejected; mac and gpu enrolled via `kind: "node"` pairing with the same SAS on both sides; signed enrollment pins the worker CIK; `nodes.list` shows 3 nodes with the profile resources and trust levels; `NodeRegistered`; relay capture has no plaintext |
| C3 — scenario F | "Build macOS app" (`requirements.os = MacOs`) placed on mac with a stored explanation; `JobAssigned`; the shim runs on the worker and streams `JobProgress`; the `.app.zip` artifact comes back as a blob whose SHA-256 matches; task `Completed` with owner = mac |
| C4 — routing | GPU ≥ 16 GB → gpu; impossible VRAM → `infeasible` with per-node explanation and task `Blocked`; draining node excluded; restricted node refuses AgentTask; trust cannot exceed the worker's grant |
| C5 — cross-node agent | AgentTask (mock provider) on gpu edits its worktree; the change lands in the coordinator's repository |
| C6 — migration | Planned gpu → mac: `JobCheckpointed` + `JobMigrated` (epoch + 1), resumes from the handoff without repeating steps; unplanned: gpu killed → lease expiry → `Interrupted` → `Recovering` → `Running` elsewhere; restarted gpu's stale epoch rejected; non-idempotent side effect → `failed(needs_user_decision)` + `ApprovalRequested` |
| C7 — models | Mock model imported on the coordinator, distributed to gpu, layers digest-verified on disk; tampered blob rejected; listed on gpu; a job requiring the model is routed there |
| C8 — security | Emergency stop cancels jobs on all nodes; revoked node refused at the next heartbeat and its job cancelled; audit has offer / accept / complete / migrate with `nodeId`; relay capture still has no plaintext |
| C9 — deferred | Secrets to a remote node, critical-job preemption and `service` jobs answer `unimplementedFeature` (or are infeasible / wait with a named reason) |

Deferred with reasons (ADR-0033 / ADR-0039; RR-G14 `unimplementedFeature`):

| Item | Reason |
|---|---|
| HPKE per-job secret delivery | Jobs with `needs.secrets` are infeasible on remote nodes until per-job sealed delivery exists |
| Critical-job preemption | The scheduler waits for capacity; no running job is preempted |
| `service` jobs | Long-running services need health checks and a restart policy; only `command` and `agentTask` run |
| Direct LAN mTLS | All core-to-core traffic is relayed through the gateway (E2E) |
| Coordinator failover, lease reconciliation after a coordinator crash | Jobs are only marked `Interrupted` on restart |
| Portable Claude Code / Codex session state | Migration hands off through the handoff record only |
| Mobile / web node UI | Nodes are managed in the desktop Nodes panel |
| Real macOS node | Acceptance simulates macOS with a node profile and a fake `xcodebuild` |
| Checkpoints through the sync chunk store | Direct blob transfer between cores instead |

## Phase 7 acceptance check (Ecosystem)

`scripts/smoke/phase7-acceptance.mjs`: a temp `orkestrd` home with
`ORKESTR_PLUGINS_TRUSTED_KEYS` set to the fixture publisher key, a temp
gateway (registry), the Mock provider, a git fixture project (tiny HTTP
service + a test) and `fixtures/fake-cloud-api.mjs`. Condition waits only;
`--only E2,E5`; report `acceptance.json`.

<!-- P7-ACCEPTANCE -->**Result: 78/78 checks passed** (+3 informational; E1 6, E2 16, E3 11, E4 8, E5 11, E6 7, E7 10, E8 6, E9 3) on Windows 11, 2026-10-22. Docker deploy runs only with `ORKESTR_P7_DOCKER=1` (otherwise reported as info; verified separately: `--only E5` with Docker 18/18, busybox container healthy, container and image removed afterwards); the `orkestr-widget` scheme was smoke-checked in the packaged app (WebView2); cloud adapters are request-shape verified against `fake-cloud-api.mjs`.<!-- /P7-ACCEPTANCE -->

| Check | What is checked |
|---|---|
| E1 — setup | Core, gateway, mock provider, fixture project, fake cloud API |
| E2 — plugin SDK | `hello-tool` install → `pendingGrants` → grant → an agent calls `plugin_*` tool; tampered / unsigned rejected; undeclared host denied + audited (`PluginCallDenied`); infinite loop trapped under the timeout; `killAll` → `PluginStateChanged` + invoke refused; safe mode survives a restart |
| E3 — marketplace | Publish template, widget and plugin to the gateway registry; search; template install appears in `templates.list`; untrusted key needs `trustKey`; tampered index, revoked entry and key-change update refused |
| E4 — widgets / dashboards | `widgets.list` includes the plugin widget; global + project dashboards saved and read back; bindings to a write or `local_only` method rejected; `widgets.bundle` carries the CSP |
| E5 — deployments | StaticSite healthy + `DeploymentFinished` Succeeded; LocalProcess healthy; Production → `ApprovalRequested`, deploys after approval; Vercel / Netlify / Fly / Kubernetes against the fake API with the token never in logs, `NeedsCredential` without it; Docker runs or is skipped with a reason; rollback |
| E6 — monitor | Overview CPU / RAM / disk; a check goes down then up → `HealthCheckChanged` + `AlertRaised` / `AlertResolved`; log tail redacted |
| E7 — Bug Detective | Broken version deployed → alert → `detective.open` from the alert; evidence with stack trace, deployed version and commit; hypothesis; reproduction fails as expected; `approveFix`; the fix on `detective/<id>` makes the tests pass, review runs, `AwaitingMergeApproval` with the project branch untouched |
| E8 — security | Emergency stop kills plugins, cancels a deployment and a detective run; a remote viewer is refused `plugins.install` and `deploy.start`; audit records |
| E9 — deferred | AWS adapter, JS plugins and the `Panel` kind answer `unimplementedFeature` |

| Feature | Spec | ADR | Crates / core | UI | Status |
|---|---|---|---|---|---|
| Plugin SDK (#48) | §48 | 0034 | `orkestr-plugins`, `orkestr-plugin-sdk`, `core/rpc/plugins.rs`, `core/plugin_tools.rs` | Marketplace (Installed, permission prompt) | **Done**: signed `.orkp`, trust store, wasmtime ABI v1 (fuel / epoch / memory, quarantine after 3 traps), host API with grants + audit, kill switch, safe mode, `Tool` kind for agents, declarative AgentTemplate / Widget / Mcp, Rust guest SDK + example. **Not done:** DeploymentProvider and PlatformIntegration runtime paths (`subscribes` not delivered) |
| Agent Marketplace (#1) | §6, §48 | 0040 | `orkestr-marketplace`, `gateway/registry.rs`, `core/rpc/marketplace.rs` | Marketplace | **Done**: official / directory / gateway sources, registry key TOFU, publisher trust + per-package pin, revocation, publish / revoke, install / uninstall |
| Widget SDK + Dashboard Builder | §48 | 0034 | `packages/widget-sdk`, `core/dashboards_store.rs`, `core/rpc/{dashboards,widgets}.rs` | Dashboards | **Done**: sandboxed iframe + CSP (`orkestr-widget` scheme in the packaged app), Read-only bindings checked in host and core, 12-column builder with config forms |
| Deployment Agents (#44) + environments (#43) | §26 | 0041 | `orkestr-deploy`, `core/rpc/deploy.rs` | Deployments | **Done** for local targets (StaticSite, LocalProcess; Docker when present); cloud adapters **request-shape verified** only |
| Live Infrastructure Monitor (#45) | §27 | 0042 | `orkestr-monitor`, `core/rpc/monitor.rs` | Monitoring → Infrastructure | **Done**: metrics, checks, logs, alerts. **Not done:** GPU utilisation, persistent logs, RPC/UI for alert rules and log sources (desktop tail polls every 2 s) |
| Bug Detective (#46) | §27 | 0042 | `orkestr-detective`, `core/rpc/detective.rs` | Bug Detective | **Done**: full stage machine with two approval gates, evidence, hypothesis, reproduction, fix + tests + review, budget, cancel, restart handling. **Not done:** RPC/UI for detective settings (auto-open stays off); the panel diff is two-dot `HEAD..detective/<id>` |

Deferred with reasons (ADR-0034 / 0040 / 0041 / 0042; RR-G14
`unimplementedFeature`):

| Item | Reason |
|---|---|
| JS / QuickJS plugins | One sandbox (wasmtime) in Phase 7; a second runtime doubles the attack surface |
| `Provider` and `Panel` plugin kinds | Provider plugins touch credentials and routing; panels need a stable UI extension API |
| Plugin auto-update | Updates are explicit installs with key continuity checks |
| Widget network access | Widgets get data only through Read-class RPC bindings (CSP `connect-src 'none'`) |
| AWS / GCP / Azure / Cloudflare adapters | Not in Phase 7 scope; the `DeployAdapter` trait is the extension point |
| Live cloud runs | Vercel / Netlify / Fly / Kubernetes are verified against a fake API only |
| Deploys inside a container sandbox | Deploy commands run with the Terminal capability + guard (residual risk in ADR-0041) |
| GitHub PR creation | The detective ends with a local `detective/<id>` branch and a merge approval |
| Public hosted marketplace, ratings, payments | Registries are self-hosted or bundled |
| External APM ingestion (Sentry, …) | Logs come from files, deployments and `monitor.ingest` |
| Mobile / web dashboard editing | Dashboards are edited in the desktop app |
| Computer Control (ADR-0035) | Stays deferred; capability is Deny by default |

## D. Summary

| Level | P1 | P2 | P3 |
|---|---|---|---|
| Full | 8 | 32 | 41 |
| Partial | 31 | 20 | 22 |
| Arch | 27 | 18 | 16 |
| — | 16 | 12 | 3 |

Counts cover sections A and B (82 items) and use the delivered levels
(reconciled 2026-10-07; Multi-monitor moved from Full to Partial in the final
audit pass). Before that reconciliation, the counts were taken from
the planned levels (P1/P2/P3 Full 10/36/57) and overstated what had shipped.
