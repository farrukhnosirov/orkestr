# AI Development OS — Master Product Specification & Build Prompt

> Source of truth for the product. Copied verbatim (structure-preserving) from the
> user-provided `AI_Development_OS_Master_Prompt.md`. Section numbers (§N) are
> referenced throughout `/docs` and code comments.
>
> **Главные принципы:** Windows-first, cross-platform-by-design, local-first,
> remote-capable, multi-model, multi-agent, multi-project, secure-by-default.
>
> MVP может реализовывать лишь часть возможностей, но фундамент с первого дня не
> должен блокировать подтверждённые функции ниже.

---

## §1. Что нужно создать

Не расширение VS Code и не AI-chat wrapper, а полноценная **AI Development OS**:
собственная рабочая среда, где пользователь управляет проектами через одного AI
Manager, а тот координирует множество специализированных AI-агентов, моделей,
инструментов и вычислительных узлов.

Пример запроса: «Создай полноценный payroll-модуль, используй несколько агентов,
проверь архитектуру, напиши backend/frontend, протестируй, проведи security
review и покажи мне результат.»

Manager должен самостоятельно:
1. изучить проект и его память;
2. построить план;
3. декомпозировать цель;
4. определить зависимости;
5. создать нужное количество агентов;
6. подобрать модели;
7. создать Git branches/worktrees;
8. параллельно выполнить независимые задачи;
9. контролировать ресурсы, лимиты и бюджет;
10. передавать structured handoff между агентами;
11. запускать build/test/review/fix циклы;
12. объединять изменения;
13. запрашивать пользователя только там, где требуется решение или approval;
14. сохранять всю полезную историю выполнения, но не скрытые chain-of-thought моделей.

Пользователь также может вручную управлять каждым уровнем системы.

## §2. Целевые клиенты

**Desktop:** основная платформа первой версии — Windows 10/11; сборка под
Windows x64/ARM64, macOS Apple Silicon/Intel, Linux x64/ARM64. Windows — главная
среда, но не фундамент архитектуры. Стек: Tauri (shell), Rust (Core/native),
React + TypeScript (UI), Monaco (editor), Rive/Lottie (2D), Three.js/WebGL/WebGPU
(3D/HUD). Если Tauri не удовлетворяет критическому требованию — сначала ADR и spike.

**Web:** полноценный Web Client с удалённым доступом (не RDP; через application
protocol/API): проекты, Manager, агенты, approvals, Monaco, remote terminal,
файлы, Git/diff, logs, task graph, nodes, расходы/лимиты, deployments, voice.

**Mobile:** Android и iOS (предпочтительно React Native) — Command Center:
Manager chat, voice, проекты, агенты, задачи, approvals, push, quick diff/review,
logs, node status, start/stop, emergency stop, cost/limits, просмотр файлов,
лёгкое редактирование.

## §3. Высокоуровневая архитектура

```text
CLIENTS (Desktop Win/macOS/Linux, Web, iOS, Android)
        │
CONTROL PLANE / SECURE GATEWAY
        │
AI CORE: Orchestrator/AI Manager, Agent Runtime, Task Engine, Provider Router,
         Context Engine, Memory Engine, Permission Engine, Cost & Limit Engine,
         Sync Engine, Node Scheduler, Event Bus
        │
TOOLS: Files, Terminal, Git, Browser, Docker, Database, MCP, SSH, Computer Control
AI PROVIDERS: OpenAI API, OpenAI Codex, Anthropic API, Claude Code, Gemini,
              DeepSeek, Ollama, Custom/OpenAI-compatible
        │
PLATFORM ABSTRACTION: WindowsAdapter, MacOSAdapter, LinuxAdapter
```

## §4. AI Provider Hub

Произвольное количество AI-сервисов: OpenAI API, OpenAI Codex, Anthropic API,
Claude Code, Google Gemini, DeepSeek, Ollama, любые OpenAI-compatible endpoints,
custom HTTP providers; в будущем Bedrock, Vertex AI, Azure OpenAI.

Авторизация (adapter-based): official OAuth/account sign-in, API key,
authenticated CLI session, official SDK, cloud credentials, local endpoint,
custom headers/tokens. Для OpenAI/Codex и Claude Code account-based usage —
**только официально поддерживаемые механизмы**; никакого scraping cookies,
hijacking сессий, извлечения приватных токенов чужих приложений. Subscription
access и API billing — разные способы подключения.

Provider Adapter Contract — adapter сообщает: auth state, available models,
capabilities, context size, tool support, vision, structured output, streaming,
coding-agent capabilities, local/cloud, latency, health, rate/limit state,
cost model. Провайдеры hot-swappable.

## §5. AI Manager / Orchestrator

Пользователь вручную выбирает provider/model для orchestration (Claude, GPT/Codex,
Gemini, DeepSeek, Ollama, custom). Настройки: model, reasoning level, system
instructions, max parallel agents, budget, time limit, context policy,
permissions, fallback chain, approval policy. Manager видит реальное состояние:
projects, tasks, agents, nodes, Git, worktrees, providers, limits, resources,
memory, environments, approvals, deployments.

## §6. Agent System

Agent Template: name, role, system prompt, preferred model, fallback models,
tools, permissions, memory scope, budget, max turns, worktree policy, quality
gates, voice, avatar/robot, visual state. Роли: Architect, Backend, Frontend,
Database, QA, Security, DevOps, UI/UX, Product Manager, Reviewer, Researcher,
Custom. Dynamic Agent Spawn (с учётом сложности, зависимостей, CPU/RAM/GPU,
model limits, стоимости, coordination overhead). Agent Teams (ERP, Web, Mobile,
Security, Rapid Prototype, custom). Hierarchy (опционально): Manager → Team
Leads → Executors → Reviewers. Agent Marketplace: official/community/private
templates, import/export.

## §7. Multi-model Routing и Fallback

Ручная или автоматическая маршрутизация (напр. Architecture→Claude,
Backend→Codex, UI analysis→Gemini, Cheap classification→Ollama, Security→Claude+GPT
consensus). Учитывает качество, capability, latency, budget, current limits,
historical benchmark, privacy policy. Fallback chain (напр. Claude Code → Codex →
Gemini → DeepSeek → Ollama). При переключении — structured handoff и явное
событие пользователю.

## §8. Consensus и Adversarial Review

Consensus Mode: несколько моделей независимо предлагают решения, Manager
синтезирует; хранить alternatives, summaries, принятое решение. Adversarial
Review: агент ищет security issues, architecture weaknesses, race conditions,
data loss, performance bottlenecks, edge cases, неверные assumptions, UX-проблемы;
не соглашается автоматически.

## §9. AI Product Manager

Идея → Epic → Features → Stories → Acceptance Criteria → Tasks, + Dependencies.
После согласования Manager запускает реализацию.

## §10. Task Engine

Task: ID/project/parent, title/description, acceptance criteria, dependencies,
priority, status, agent, provider/model, worktree, files touched, execution
history, cost/usage, logs, artifacts, approvals, result, review state.
Статусы: `Planned → Ready → Running → Waiting/Blocked → Review → Testing →
Completed`; также `Failed`, `Cancelled`, `Interrupted`, `Recovering`. Явные state
machines.

## §11. Task Graph, Priority, Parallelism

Визуальный DAG. Приоритеты: Critical, High, Normal, Low, Background. Параллельное
выполнение независимых задач с учётом CPU, RAM, GPU/VRAM, disk, network, API
concurrency, provider limits, Git conflicts.

## §12. Background и Headless

Агенты работают, когда пользователь переключил проект / свернул GUI / использует
Web/Mobile. Core — background/headless service, автостарт при загрузке ОС (при
разрешении).

## §13. Multi-project Workspace

Много проектов одновременно. Project: repository, workspace, branches/worktrees,
environments, project memory, tasks, agents, conversations/summaries, secrets,
servers, databases, deployments, costs, snapshots, activity. Мгновенное переключение.

## §14. Project Desktop

Панели: Manager, Code, Files, Terminal, Git, Agents, Tasks, Architecture, Browser,
Database, Logs, Deployments, Memory, Costs, Monitoring.

## §15. Built-in Editor и Terminal

Monaco: tabs, syntax, LSP/IntelliSense, diagnostics, search, diff, Git decorations,
agent change markers, inline AI actions; не зависеть от VS Code, но позволять
открыть во внешнем IDE. Terminal: multiple sessions, user/agent-owned terminals,
tabs, stream, history, approval interception. Shells: PowerShell, CMD, Git Bash,
WSL, bash, zsh, fish, custom.

## §16. Git, Worktrees и AI Merge Manager

Git — основной version-control layer. Каждый параллельный агент по умолчанию
получает branch + worktree (`agent/backend`, `agent/frontend`, …). Merge Manager:
semantic diff, conflict detection, dependency check, acceptance criteria, tests,
review, merge proposal, automatic merge только при разрешённой policy.

## §17. Automatic Review, Testing, Self-Healing

Pipeline: Author → Reviewer → Build → Tests → Security/QA → Merge. Автообнаружение и
запуск unit, integration, E2E, lint, typecheck, build, smoke. Self-Healing:
Implement → Build → Test → Error → Analyze → Fix → Test → Review; всегда max
attempts/time/budget.

## §18. Browser Agent и Computer Control

Browser Agent: open URL, click/type/scroll, screenshot, responsive checks, console
errors, network failures, user-flow testing. Computer Control — отдельная
capability для Win/macOS/Linux; не выдавать агентам по умолчанию.

## §19. MCP Hub

Центральный менеджер MCP (filesystem, GitHub/GitLab/Bitbucket, DB, Docker, browser,
Figma, Notion, Google Drive, custom). Показывать status, tools, permissions,
secrets, связанные проекты.

## §20. Memory, Knowledge, Context

Project Memory: requirements, architecture, business logic, conventions, schema,
APIs, decisions, mistakes, TODO, known issues, agent summaries. Global Memory:
coding conventions, architecture preferences, quality standards, provider
preferences, reusable infrastructure. Project rules приоритетнее. Knowledge Base:
Markdown/PDF/docs/API docs/company rules/schemas/external docs. Context Optimizer:
не отправлять весь repo; собирать task, relevant files/symbols, dependencies,
project rules, relevant memory, concise handoff.

## §21. Architecture Map, Decision Log, Replay

Architecture Map (modules/services/databases/APIs/dependencies). Decision Log
(ADR-like: решение, alternatives, reasoning summary, последствия, tasks/files).
Replay: User request → Manager plan → Agent creation → Delegation → Tool events →
Files changed → Tests → Reviews → Approvals → Merge. Не хранить скрытый CoT.

## §22. Cost, Limits, Benchmarking

Cost & Limit Center по provider/model/agent/project/task: usage, estimated cost,
API spend, quota/limit state, daily/monthly budget. Smart Budgeting (Project
$100/month, Task max $5, Security unrestricted, Ollama unrestricted). Model
Benchmarking: first-pass success, build success, defects found, retries, latency,
cost. Agent Performance: completed/failed, review rejection, post-merge bugs,
retries, duration, cost.

## §23. Permissions, Sandbox, Safety

Agent = untrusted autonomous actor. Capabilities: Read, Write, Delete, Terminal,
Git, Network, Browser, DB read/write, Docker, SSH, Deploy, Computer Control,
Secrets. Scope: global, project, agent, task. Approval modes: Ask, Semi-Auto, Full
Auto (Full Auto не отменяет hard policies). Dangerous Action Guard: recursive
delete, DROP/TRUNCATE, production write/deploy, secrets changes, force push,
destructive reset, privileged OS/network changes. Sandbox: worktree, container
where practical, restricted filesystem, restricted network, env-specific creds.

## §24. Secrets Vault

Никаких plaintext keys. Abstraction: Windows Credential Manager, macOS Keychain,
Linux Secret Service, encrypted local vault, self-hosted vault, cloud secret
provider. Scopes и audit.

## §25. Snapshots и Rollback

Checkpoint перед risky/autonomous changes; snapshot включает uncommitted state, ≠
Git commit. Create Snapshot, Rollback Task, Rollback Workspace. Rollback не
уничтожает unrelated changes.

## §26. Environment и Deployment

Environments: Local, Dev, Test, Staging, Production, custom (variables/secrets/DB/
URLs/permissions). Deployment: Docker, Compose, VPS/SSH, Vercel, Cloudflare,
AWS/GCP/Azure, custom scripts; audit trail.

## §27. Infrastructure Monitor и Bug Detective

CPU/RAM/GPU/disk, node health, services, containers, uptime, logs, health checks.
Bug Detective: Production error → logs/code/context → root-cause hypothesis →
reproduction → fix branch → tests → review → approval/PR.

## §28. Полностью кастомный Visual OS

Frameless, custom title bars, custom controls, transparency, blur, glow,
animations, custom shadows/shapes, docking, detachable panels, floating windows.
Ощущение собственной операционной среды.

## §29. Theme Engine и Layout Editor

Configurable: colors, gradients, typography, spacing, radius, opacity, blur, glow,
shadows, animations, backgrounds, sounds. Layout: drag/drop, resize, hide,
dock/undock, presets (Coding, Mission Control, Monitoring, Focus, Custom).
AI-editable UI: Manager изменяет безопасную UI configuration schema, а не
генерирует arbitrary executable UI-код.

## §30. Floating Robots

Каждый floating robot связан с реальным Agent ID. Состояния: Idle, Listening,
Thinking, Coding, Testing, Reviewing, Waiting, Error, Completed. Transparent
frameless, always-on-top, click-through when idle, drag/drop, snap, speech bubble,
context menu, task status, voice, open agent details; перетащить робота на проект
= назначить агента. Robot Designer: 2D/3D, skin, scale, animations, voice, sounds,
behavior. Rive/Lottie; архитектурно Three.js/WebGL/WebGPU.

## §31. Performance Mode

Ultra Visual, Balanced, Performance, Battery Saver. Визуальные эффекты не мешают
AI workload.

## §32. Mission Control

Projects, active agents, task graph, nodes, CPU/RAM/GPU, provider health, limits,
spend, approvals, builds, deployments, alerts.

## §33. Voice Command Center

Voice — first-class. Команды («Запусти QA по Schoolify», «Что делает Backend
Agent?», «Останови Thompson Finance», «Покажи ошибки сборки»). Wake Word:
настраиваемые, детекция по возможности локальная, не отправлять постоянный
microphone stream в cloud без явного согласия. Voice per Agent (TTS
provider/voice/language/style). Voice Notifications (DND). Sound Engine (wake/start/
completion/error/approval/deploy/alert).

## §34. HUD, Ambient, Multi-monitor

HUD/Ambient: маленький Manager, floating robots, status, voice indicator,
approvals. Multi-monitor: code / Mission Control / browser на разных мониторах;
layouts per-device.

## §35–37. Remote Web Control, Remote Security, Mobile Push

Self-Hosted AI Node; Browser/Mobile → TLS + MFA/Passkey → Secure Gateway/Relay →
mutually authenticated encrypted tunnel → Local AI Node. Не выставлять raw
Core/terminal port. Минимум: TLS, MFA, passkeys/WebAuthn, device authorization,
session management, revoke, rate limiting, audit, permissions, emergency kill
switch. Push: task completed, blocked, approval, build/test failed, security
issue, node offline, deployment, budget threshold.

## §38–39. AI Nodes, Cluster, Job Migration

Любой Desktop Core может стать Node (OS, CPU, RAM, GPU/VRAM, disk, runtimes,
Docker, local models, load, availability). Scheduler по OS requirement,
resources, tools, local models, project cache, load, network, user preference.
Migration: task checkpoint, workspace state, agent state, context/handoff,
artifacts — логическое возобновление, не миграция OS-процесса.

## §40–45. Cloud Workspace, Sync, Offline, Storage Modes

Cloud workspace (metadata, snapshots, memory, tasks, agent state, UI state, logs,
artifacts) + local caches. Git ≠ Live Sync; не auto-commit. Continuous Sync: File
Change → Local Journal → Incremental Snapshot → Encrypted Sync → Cloud;
incremental, resumable, compressed, encrypted, offline queue, conflict detection,
versioning, content-addressed chunks. Seamless Continue между устройствами.
Offline-first: editor, Git, terminal, Ollama, snapshots, sync queue работают без
сети. Storage modes: Cloud, Self-Hosted (Control Plane + PostgreSQL + S3/MinIO +
Gateway), Local Only. Interfaces: MetadataStore, ObjectStore, GitProvider,
SecretStore, SyncBackend.

## §46. Cross-platform Platform Layer

PlatformAdapter: Process, Shell, Window, CredentialStore, Notifications,
Autostart, GlobalHotkeys, Microphone, ComputerControl, Power. Реализации
WindowsAdapter, MacOSAdapter, LinuxAdapter. Никаких hardcoded `C:\Users\...`.

## §47. Build Targets

Windows installer, macOS app, Linux AppImage/deb/rpm, Web, Android, iOS; 90–95%
общего кода.

## §48. Plugin SDK и Widget SDK

Plugins: provider, tool, MCP, panel, widget, agent template, deployment provider,
platform integration. Manifest + explicit permissions. Dashboard Builder. Widget
не получает filesystem/secrets автоматически.

## §49. Event-driven Architecture

Typed events: TaskCreated, AgentStarted, AgentStopped, FileChanged, BuildFailed,
ApprovalRequested, NodeOffline, ProviderRateLimited, SnapshotCreated,
SyncCompleted, DeploymentFinished. Подписчики: UI, notifications, automation,
audit, plugins.

## §50. Observability

Structured logs, traces, metrics, correlation IDs, project/task/agent/node IDs,
sensitive-data redaction.

## §51. Data Model

User, Device, Node, Project, Repository, Workspace, WorkspaceSnapshot, Provider,
ProviderCredentialReference, Model, AgentTemplate, AgentInstance, AgentTeam, Task,
TaskDependency, Execution, ToolCall, Approval, MemoryItem, KnowledgeSource,
Decision, Environment, Deployment, SecretReference, UsageRecord, AuditEvent,
Plugin, MCPServer, Notification. UI не обращается напрямую к DB.

## §52. Security Model

Least privilege, capability-based access, sandbox, approvals, secret isolation,
environment separation, network policy, audit. Emergency controls: Stop Agent,
Stop Project, Stop All AI, Revoke Remote Sessions, Disable Network Tools, Lock
Vault, Disable Full Auto. Local emergency stop работает даже при проблемах
Control Plane.

## §53. UX Philosophy

Natural (описать результат) и Expert (models, agents, context, worktrees,
permissions, budget, merge, nodes, infrastructure). Сложность доступна, но не навязана.

## §54. Command Palette и Global Search

Palette: Create/Switch Project, Ask Manager, Spawn Agent, Run Tests, Open Terminal,
Mission Control, Deploy, Snapshot, Rollback, Stop All. Search: projects, files,
symbols, tasks, agents, decisions, memory, logs, conversations, deployments.

## §55. Onboarding

1 local profile; 2 storage mode; 3 AI providers; 4 Manager model; 5 permission
mode; 6 import/create project; 7 Git check; 8 terminal check; 9 optional voice;
10 optional remote access; 11 optional cloud sync; 12 optional mobile pairing.
Provider UI: OpenAI [Official Sign-in where supported][API Key]; Claude [Claude
Code Login][Anthropic API Key]; Gemini [API Key]; DeepSeek [API Key]; Ollama
http://localhost:11434 Detected models.

## §56. Privacy Policies

Per-project: Cloud AI Allowed, Selected Providers Only, Local Models Only, No
Remote Sync, Self-Hosted Only. Classification: Public, Internal, Confidential,
Secret. Provider/tool policy может зависеть от classification.

## §57. Session Resume и Agent Handoff

Resume: «Last session: Backend API completed; Frontend 70%; QA found 2 issues; 1
approval pending; Last snapshot …». Handoff: task, concise context, relevant
files, decisions, unresolved issues, test state, artifacts. Не передавать
бесконечную историю чата.

## §58. Quality Gates

Build passes, lint passes, tests pass, no critical security finding, reviewer
approved, acceptance criteria met. DoD: code, tests, docs, migrations if needed,
rollback where relevant, review, acceptance criteria.

## §59. Документация проекта

`/docs/{architecture,adr,security,sync,agents,providers,platform,api}`.

## §60. Development Rules

1 TypeScript strict. 2 Rust safety/quality first. 3 No god services/classes.
4 Interfaces at boundaries. 5 No provider-specific logic in generic Agent Core.
6 No OS-specific logic outside Platform Layer. 7 No secrets in logs. 8 No silent
destructive actions. 9 Critical orchestration/sync/security logic covered by
tests. 10 Migrations reversible where practical.

## §61. Roadmap

- **Phase 0 — Architecture & Risk Spikes:** system diagrams, threat model, provider
  abstraction, agent/task model, Tauri spike, Monaco spike, terminal spike,
  floating transparent window spike, Claude Code integration, OpenAI/Codex
  integration, Ollama integration, Git worktree, local service, WebSocket/event
  streaming.
- **Phase 1 — Windows Desktop Core MVP:** custom desktop shell, projects, Manager,
  provider hub, manual agents, basic dynamic spawn, terminal, files, Monaco,
  Git/worktrees, tasks, basic memory, approvals, logs, snapshots.
- **Phase 2 — Multi-Agent Quality:** teams/hierarchy, parallel execution, review,
  testing, self-healing, merge manager, consensus, adversarial review, context
  optimizer, decision log, benchmarks.
- **Phase 3 — Visual OS:** Mission Control, theme/layout editor, floating robots,
  robot designer, voice/wake word, HUD, multi-monitor, performance modes.
- Phase 4 — Remote & Sync. Phase 5 — Mobile. Phase 6 — Multi-node. Phase 7 — Ecosystem.

## §62. Главные технические риски

1 Tauri floating/transparent windows на трёх ОС. 2 Secure terminal streaming.
3 Live sync conflict resolution. 4 Git + live sync interaction. 5 Worktree
concurrency. 6 Provider auth restrictions. 7 Subscription-login limitations.
8 Wake-word battery/CPU. 9 Mobile remote security. 10 Node task migration.
11 Sandboxing terminal commands. 12 Cross-platform Computer Control. 13 Crash
recovery. 14 Sync of uncommitted state. Для каждого high-risk — ADR/spike.

## §63. Acceptance Scenarios

A Multi-agent payroll; B Provider fallback on limit; C Cross-device; D Remote Web
with MFA/passkey; E Mobile approval; F Multi-node (macOS build → Mac node);
G Voice («Jarvis, что происходит со Schoolify?»); H Floating Robot (QA robot
completes → animation → bubble → click → result); I Offline; J Rollback (Rollback
Task without destroying unrelated changes).

## §64. Запрещено как фундамент

Chatbot wrapper; VS Code extension; Windows-only Core; provider lock-in; один
giant orchestrator service; бесконтрольные autonomous loops; plaintext secrets;
exposed raw terminal port; auto-commit spam; cloud-only; hardcoded UI; fake
«агенты» (просто разные prompts без task/runtime isolation).

## §65. Architecture Deliverables

1 System Context Diagram. 2 Container Diagram. 3 Component Diagram. 4 Monorepo
proposal. 5 Data Model. 6 Event Model. 7 Provider Adapter spec. 8 Agent Runtime
spec. 9 Task state machine. 10 Permission model. 11 Threat model. 12 Sync
protocol. 13 Git/worktree strategy. 14 Node protocol. 15 Remote-access
architecture. 16 Storage abstraction. 17 Platform abstraction. 18 Plugin
architecture. 19 Desktop/Web/Mobile shared protocol. 20 ADR list. 21 Technical
spike list. 22 MVP milestones.

## §66–67. Порядок работы и критерии успеха

Steps: проверить требования/риски → repository architecture → ADR → foundation
(monorepo, shared types, event protocol, Tauri shell, Core service, provider
interfaces, project model, task state machine) → spikes → MVP.

Архитектура правильная, если: Claude заменяется GPT без переписывания Agent
Runtime; Gemini добавляется adapter'ом; macOS build не требует переписывать
UI/Core; Linux добавляется PlatformAdapter'ом; Web/Mobile используют общий
protocol; managed cloud заменяется self-hosted; S3 заменяем; новый Node без
изменения Project Core; cloud можно отключить; workspace восстанавливается после
crash; agent не видит secrets другого проекта без permission; UI theme/layout
меняются без переписывания Core.

## §68. Feature Inventory (must not be lost)

1 Agent Marketplace 2 Dynamic Agent Spawn 3 Agent Teams 4 Agent Hierarchy
5 Multi-model routing 6 Model fallback 7 Project Memory 8 Global Memory
9 Knowledge Base 10 Git Worktrees per agent 11 AI Merge Manager 12 Automatic Code
Review 13 Consensus Mode 14 Adversarial Review 15 Auto Testing 16 Self-Healing
Loop 17 Visual Browser Agent 18 Computer Control 19 Terminal Workspace 20 Monaco
21 Project Desktop 22 Mission Control 23 Visual Task Graph 24 Parallel Execution
25 Priority Queue 26 Background Agents 27 Cross-project Agents 28 Voice Command
Center 29 Wake Word 30 Voice Notifications 31 Approval Levels 32 Permission
Sandbox 33 Dangerous Action Guard 34 Rollback Everything 35 Snapshots 36 Cost &
Limit Center 37 Smart Budgeting 38 Model Benchmarking 39 Agent Performance
Analytics 40 Context Optimizer 41 MCP Hub 42 Secrets Vault 43 Environment Manager
44 Deployment Agents 45 Live Infrastructure Monitor 46 Bug Detective 47 AI Product
Manager 48 Architecture Map 49 Decision Log 50 Execution Replay.

Также: custom Visual OS, floating robots, Robot Designer, Theme Engine, Layout
Editor, Docking, HUD/Ambient, multi-monitor, AI-editable UI, Sound Engine, voice
per agent, Performance Mode, Windows-first + macOS/Linux, Web Client, Android,
iOS, secure remote access, headless AI Node, multi-computer cluster,
resource-aware scheduling, task migration/checkpointing, Cloud Workspace, Local
Cache, Continuous Sync, Git как отдельный layer, Self-Hosted, Local-Only,
Offline-first, cross-device continuation, Plugin SDK, Widget SDK, Dashboard Builder.

## §70. Приоритеты при конфликте требований

1 безопасность данных; 2 возможность восстановления; 3 корректность;
4 сохранность проекта; 5 контроль пользователя; 6 качество AI-результата;
7 стабильность; 8 производительность; 9 UX; 10 визуальная премиальность;
11 эффектность анимаций. «Красивый робот, удаливший production database, всё ещё
удалил production database.»

## §71. Финальная команда

Начать с архитектурной фазы. MVP может быть маленьким, но архитектура должна
оставлять прямой путь ко всему target scope.
