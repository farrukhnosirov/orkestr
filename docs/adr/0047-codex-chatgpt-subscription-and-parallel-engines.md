# ADR-0047 — Codex (ChatGPT subscription) engine and parallel engines

Status: accepted — implemented (wave 8, user request #8)

## Context

The user wants their ChatGPT subscription (OpenAI Codex CLI signed in with
ChatGPT) usable inside Orkestr exactly like the Claude Code subscription, and
both running in parallel. The old Codex provider ran `codex exec --json` with
launch-time-only gating (ADR-0012): no per-command approvals, no resume, no
limits, no panel.

codex-cli 0.163.0 ships `codex app-server`: JSON-RPC over stdio (`initialize`,
`thread/start|resume|fork|read|list`, `turn/start|interrupt`,
`account/read`, `account/rateLimits/read`, `account/login/start`,
`model/list`, …) with server→client approval requests
(`item/commandExecution/requestApproval`, `item/fileChange/requestApproval`).

## Decision

1. **Everything goes through `codex app-server`** (`crates/providers/src/codex_app`):
   * agents: one process per run, `thread/start` (or `thread/resume` with the
     agent's saved thread) + `turn/start`, `approvalPolicy: untrusted`,
     sandbox `workspace-write` only when the template may write, else
     `read-only`; `danger-full-access` is never used. Each approval request
     becomes a `PermissionRequest` decided by the same `bridge_decide` as
     Claude Code (template tools → worktree confinement → permission engine +
     Dangerous Action Guard → approvals → checkpoint). Commands Codex ran
     without asking are classified when they finish (post-hoc violation stops
     the run); file changes are confined to the worktree. Supersedes ADR-0012
     for Codex.
   * Manager / reviews (chat): a persistent pool — the `claude_chat` pool is
     now generic over a `PoolBackend`; Codex keeps one process + one ephemeral
     read-only thread per conversation and sends only new messages; tools are
     emulated as for Claude Code.
   * panel, status, limits, models, threads, sign-in: one shared long-lived
     app-server process (`CodexHub`) with one thread per panel session.
2. **Sign-in** is the CLI's own ChatGPT flow (`account/login/start` browser or
   device code; `codex login --device-auth` in an Orkestr terminal as a third
   path). Orkestr never reads `~/.codex/auth.json`.
3. **Install / update**: `npm install --prefix <data_dir>/tools/codex
   @openai/codex@latest` (never global); binary lookup: pinned provider path →
   Orkestr's tools folder → `PATH`; latest version from `npm view` (6 h cache).
4. **Limits**: `account/rateLimits/read` (primary / secondary windows, plan,
   credits); usage-limit errors (`codexErrorInfo: usageLimitExceeded`) map to
   `QuotaExceeded`, so the existing CLI usage-limit fallback (structured
   handoff) moves work Codex ↔ Claude Code ↔ API providers.
5. **Models**: live `model/list`; `<model>@<effort>` carries a reasoning
   effort in routing (`gpt-5-codex@high`).
6. **Parallel engines** (`engines.*`, `<data_dir>/engines.json`) only rewrite
   the existing Manager settings and routing rules: `single`, `split` (roles
   in `codexRoles` on Codex), `balanced` (roles re-pointed to the less loaded
   engine on every agent start/stop, an almost exhausted plan counts as busy),
   `race` (reviews on one engine, adversarial reviews on the other, consensus
   with both) and `consensus` (Phase 2 consensus draws participants from
   both). `crossFallback` makes the other engine the first fallback. True
   concurrent first-answer-wins racing would need an orchestrator change and
   is not done.
7. **Templates**: built-in Codex agents, `~/.codex/prompts/*.md` and the
   project's `AGENTS.md` are mirrored as templates (`TemplateSource::Codex`).

## Consequences

* New protocol: `codex.*`, `engines.*`, events `CodexEvent`, `CodexStatus`,
  `CodexAccount`, topic `codex:<sessionId>`; policies fail-closed
  (installs / sign-in / engine config local-only, send/start expensive,
  stop/interrupt emergency).
* Tests use `orkestr-fake-codex` (scripted app-server). The app-server
  protocol is marked experimental upstream; message mapping lives in one pure
  module (`codex_app::map`) with recorded fixtures.
