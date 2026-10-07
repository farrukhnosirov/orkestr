# Phase 0–3 audit report

| | |
|---|---|
| Date | 2026-10-06 … 2026-10-07 |
| Scope | The whole repository on `main` after Phases 0–3 and the i18n pass: all Rust crates, the Tauri shell, the React app, `packages/*`, the smoke scripts and the docs |
| Platform | Windows 11 (the only OS the product is built and tested on so far) |
| Result | 157 findings confirmed over two rounds, and all of them fixed on `main` (`be352fc` and earlier); final verification green (see §5) |

## 1. Method

Each round worked in three steps.

1. **Find.** Ten independent auditors, one per dimension (§2), each read the code (and ran it where useful) and reported findings. Each finding came with a file, a severity and a concrete failure scenario. Round 1 also got 51 leads from a preliminary Phase 1 review to re-check. Round 2 got the list of round-1 fixes and re-reported a finding only when its fix was wrong or incomplete.
2. **Adversarial verification.** One verifier per finding tried to *refute* it against the current code. A finding counted only if the failure scenario really happens on `main`, and a finding stayed out when the verifier was still unsure after investigating. The verifier also re-assessed severity.
3. **Fix.** Confirmed findings were grouped into ten code areas: core-security, orchestration, providers-knowledge, data, tauri, fe-panels-a, fe-panels-b, fe-features, fe-shell-packages and docs-misc. Each area was fixed in its own git worktree and branch (`fix/r<round>-<area>`), with a regression test for each logic bug where feasible. The branches were then merged into `main` and re-verified in full, including every smoke script.

The numbers in §3 come from the auditors' and verifiers' structured outputs, which are kept in the workflow transcripts. Fix counts come from the fix commits and the fixers' reports.

## 2. Dimensions

| Dimension | What was checked |
|---|---|
| security | Permission engine, dangerous-action guard bypasses (real command variants), path confinement, approval races, secret leakage, RPC auth / Origin / Host, completeness of the emergency stop, agent identity spoofing, AI-UI patch injection, Tauri window capabilities |
| orchestration | Manager, planner, run loop, quality pipeline, self-healing, reviews, consensus, Merge Manager, teams, agent runtime, task engine: state-machine violations, unbounded loops, deadlocks, races, crash recovery, budget enforcement, fallback / handoff |
| data-integrity | vcs, snapshots, files, storage, events, sync, node: rollback safety (§25), worktree concurrency, merge gates, migrations, event persistence, write atomicity, object store, SQLite locking |
| providers-knowledge | SSE parsers, tool-call assembly, error classification, router and fallback, LocalOnly privacy, CLI adapters, cost and budget math, memory FTS, context optimizer, architecture map, replay |
| frontend-logic | React effects and subscriptions, stale closures, event races, RPC parameters vs the generated protocol types, store bugs, layout persistence, multi-window sync, performance modes, leaks |
| frontend-design | The running app at 960×600 and 1600×1000 in Russian and English, captured window-only through WebView2 DevTools: layout, overflow, contrast (WCAG AA), consistency, untranslated text |
| spec-consistency | Claims in the docs, roadmap, ADRs, README and UI compared with actual behaviour: stubs presented as features, dead RPC methods / events, settings that are stored but never applied, contract violations |
| i18n | Hard-coded strings, missing keys, Russian grammar / plurals / glossary, locale formatting, core text shown untranslated, Russian truncation |
| visual-os-native | Tauri windows, transparent robots, click-through, HUD, tray, autostart, single instance, capabilities, multi-monitor, voice (local wake word, consent), sound, Rust/TS theme validation parity, performance modes |
| tests-build | Full builds and test suites (twice, for flakiness), clippy, tests that assert nothing on critical paths, missing tests (§60.9), bundle size |

## 3. Numbers

### Findings per round

| | Round 1 | Round 2 | Total |
|---|---|---|---|
| Raw findings reported | 96 | 77 | 173 |
| After de-duplication | 94 | 77 | 171 |
| Rejected by the adversarial verifier | 12 | 2 | 14 |
| **Confirmed** | **82** | **75** | **157** |
| **Fixed on `main`** | **82** | **75** | **157** |

### Confirmed findings by severity (after the verifier's re-assessment)

| Severity | Round 1 | Round 2 | Total |
|---|---|---|---|
| Critical | 3 | 0 | 3 |
| High | 19 | 11 | 30 |
| Medium | 35 | 44 | 79 |
| Low | 25 | 20 | 45 |
| **Total** | **82** | **75** | **157** |

Severity as reported by the auditors, before verification: round 1 had 5 critical, 26 high, 38 medium and 27 low; round 2 had 20 high, 38 medium and 19 low. The verifiers mostly downgraded severity.

### Raw findings per dimension

| Dimension | Round 1 | Round 2 |
|---|---|---|
| security | 11 | 4 |
| orchestration | 5 | 5 |
| data-integrity | 10 | 6 |
| providers-knowledge | 10 | 7 |
| frontend-logic | 11 | 10 |
| frontend-design | 5 | 14 |
| spec-consistency | 13 | 10 |
| i18n | 12 | 10 |
| visual-os-native | 12 | 7 |
| tests-build | 7 | 4 |

### How the fixes landed

- **Round 1:** ten area commits (`b6393af`, `83b68cf`, `945a722`, `c965394`, `78ffcbd`, `26a7f0d`, `d4e4f9a`, `f136b23`, `e46eb05`, `2796dcb`) closed 79 findings. The three the fixers left open were closed in `6d1e29d`: the effective Performance Mode in robot windows, the push-to-talk default in the protocol crate, and the warning for an unknown model price.
- **Round 2:** ten area commits (`2e051e1`, `581b42d`, `f4dc2d5`, `a9edd84`, `d77aafb`, `6f278b9`, `f0bb12a`, `b9ca46b`, `3b8b1ef`, `8e570bf`) plus their WIP commits. Their subjects add up to 73 items for 75 findings, because some commits closed two overlapping findings with one change. No fixer reported a finding as left open. They were merged in `be352fc` with post-merge fixes. Where the fix was a docs correction, the claim was downgraded rather than a feature invented. For example, the Browser panel is now honestly deferred to Phase 4.
- **The audit workflow's own counter says 136.** The round-2 run was resumed with a carried-over round-1 log of 61 entries (61 + 75 = 136). The verifier transcripts above are the primary record, and they show 82 confirmed in round 1.

## 4. Notable fixes

**Security and safety**
- The dangerous-action guard now fails closed on many bypass forms. It catches program names coming from variables, script text piped into a shell, here-strings, inline interpreter code, GTFOBins-style exec flags, `cmd` `if`/`for`/`call`, `forfiles`, `wmic`, `Start-Process -ArgumentList` arrays and shell functions. Writes rooted in a variable (`$env:`, `${HOME}`, `%APPDATA%`, `$(...)`) count as outside the worktree. Every bypass has a table-driven regression test.
- `.git` and secret rules are checked on the canonical path, with 8.3 aliases and links resolved. The data directory and `core.token` are protected against globs, environment variables, `~`, `..` and drive forms.
- The RPC WebSocket checks both Host and Origin. Only the bundled app and the `:1420` dev server are allowed origins.
- Agent shell commands need the Network, Secrets and out-of-root Read capabilities when they use them. `git push` and `git fetch` need Network.
- Codex / Claude Code CLI agents: Bash is never pre-approved, and a post-hoc guard classification stops dangerous commands. CLI file tools are scoped to the worktree.
- Cloud speech consent is bound to a specific provider, and disabled providers are never used.
- The tray "Stop All AI" works natively, without the webview's RPC connection. Stop All and Stop Project also cancel Manager chat, PM, planner and merge AI calls.
- Project privacy: the data classification tightens the effective AI and sync policy in every engine, and Settings → Project Privacy edits it.

**Orchestration and money**
- The run budget (`maxCostUsd`) is enforced while agents are live. Parallel agents share the remaining budget instead of each spending all of it.
- A RequestChanges review no longer deadlocks Self-Healing. Work that awaits acceptance survives a restart. A merge interrupted after the fast-forward is finished on restart.
- Subscription and local model calls cost $0, with the API-equivalent kept separately. Review, adversarial and consensus spend is attributed to the task.
- `write_file` / `edit_file` are atomic and check for concurrent changes. Agent history is compacted to the model's context window.

**Providers**
- The pricing table covers current Claude models, with family fallbacks, so new releases are no longer billed at $0.
- Several protocol fixes: parallel tool calls without an index are no longer merged; ids of id-less tool calls are process-unique; a half-open circuit breaker admits a single probe; streaming requests time out on response headers; Gemini thought signatures are echoed back.

**Data integrity**
- Task rollback in a shared checkout reverts only the task's own files. Ignored or excluded files are never deleted, and unreadable files are kept and reported. The object store fsyncs before rename.

**UI, i18n and design**
- Approval reasons, pipeline escalations, healing reasons and run reports appear in the UI language everywhere.
- Global search covers tasks, memory, the Knowledge Base and files.
- Stop Project is reachable from the emergency dialog, the project rail and the palette. Emergency flags stay live in every window.
- Theme contrast was fixed. The Solar Light accent gradient and subtle text now meet WCAG AA, and the contrast report checks every gradient stop.

## 5. Final verification (2026-10-07, on `main`)

| Check | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | 1907 passed, 0 failed, 2 ignored |
| `npm run gen:protocol` | no diff |
| `npm run typecheck` | clean |
| `npm test` | 947 passed (desktop 904 in 89 files, plus the packages' 10 + 7 + 26) |
| `npm run build` | OK |
| `scripts/smoke/phase1-smoke.mjs --headless` | 38/38 |
| `scripts/smoke/phase1-acceptance.mjs --headless` | 45/45 |
| `scripts/smoke/phase2-acceptance.mjs` | 47/47 desktop app (46/46 `--headless`) |
| `scripts/smoke/phase3-acceptance.mjs` | 64/64 |
| Screenshot scripts: phase1, i18n-ru, phase3-{theme, voice, robots, hud, mission-control, layout} | all pass; captures refreshed in `docs/screenshots/` |

Three smoke scripts were red in this final pass. All three were script defects, not product defects, and all three were fixed:
- `phase3-voice-screenshots.mjs` sent Enter as a `rawKeyDown` with no text, so the HUD's typed-command form never submitted. Enter now carries `\r`, as in `phase3-acceptance.mjs`. The same helper was fixed in the robots and layout scripts. This script had never passed before; `docs/screenshots/phase3-voice/` is new.
- `phase1-screenshots.mjs` attached DevTools to the first `:1420` page. Since Phase 3, that can be a floating robot window. It now picks the main window only.
- `phase3-layout-screenshots.mjs` used a private dev server on `:1437`, which the audit's Origin allowlist now rightly rejects. It uses the standard `:1420` dev server.

## 6. Remaining known issues

These are not audit regressions. They are the honest open scope, and the details are in `docs/roadmap/mvp-milestones.md` and `feature-traceability.md`.

- **Not built:** MCP Hub (stdio client), Environment Manager, the Monaco LSP bridge, hand-off to `orkestrd` (M2.7 as a whole), the Visual Browser agent (deferred to Phase 4), local Whisper STT, memory-item and permission-rule versions, merged-task revert proposals, rollback from Replay, contrast rejection for AI-UI patches, `--safe-ui` recovery, terminal take-control and scrollback snapshots.
- **Windows only.** No macOS or Linux packages, no CI matrix, no signed installer or auto-update. The DoD items "`cargo check` on macOS/Linux" and "`npm run lint`" are not in place; there is no lint script.
- **Acceptance used the Mock provider.** Real providers, a real microphone and real speech recognition were not exercised end to end, and there are no recorded demos.
- **Unmeasured exit criteria:** the 200-node graph at ≥ 50 fps, UI CPU < 3 % with 5 agents, per-robot CPU/RAM (SP-04), SP-15 wake-word budgets and the zero-network packet capture, the 6-agent load test, the adversarial eval (≥ 4 of 5) and the context-pack eval (≥ 90 %).
- **Multi-monitor restore** after a restart was not verified, because the host has one monitor. Autostart was not verified after a reboot.
- **Phase 0 spikes:** most formal spike reports (SP-02…SP-11, SP-15…SP-18) were never written. The M0.4 gate was formally not met. Later phases answered the functional questions in code and tests, but the measured spike criteria remain open.
- **In the Russian UI**, the Logs panel and the Manager's activity feed show raw core tracing messages in English, such as "merge proposal created" or "task transition". Product names and paths also stay Latin, as intended.
- **Leftover directories:** `.claude/worktrees/` still holds directories from earlier workflow runs. They are not registered git worktrees (`git worktree list` shows only `main`) and can be deleted by hand. A stale Vite dev server from one of them was still listening on `:1437` during this pass; it was left running.
