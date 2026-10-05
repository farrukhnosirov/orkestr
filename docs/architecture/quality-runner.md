# Quality runner and quality gates (§17, §58)

Crate `crates/quality` (`orkestr-quality`), RPC area `quality.*`
(`crates/core/src/rpc/quality.rs`), UI `apps/desktop/src/features/quality`.

## Pipeline

1. **Detection** (`detect.rs`): scans the project root (bounded depth,
   skips `node_modules`, `target`, `.git`, venvs…) for manifests — Cargo,
   npm/pnpm/yarn (`package.json` scripts build/test/lint/typecheck/e2e/
   smoke/format), Python (pytest, ruff/flake8, mypy), Go, .NET, Maven,
   Gradle, Makefile targets. Each root (monorepo aware) gets `CheckDef`s
   with `cwd` relative to the project root.
2. **Profile**: detection result unless the user saved overrides
   (`quality.setProfile`, table `quality_profiles`); `reset: true` returns
   to detection.
3. **Run** (`service.rs`): checks execute in kind order (build → typecheck →
   lint → format → tests → e2e → smoke) in the project root, a worktree of
   the project repository, or the task's worktree. Every command goes through
   `TerminalExecutor` → terminal run API → security `CommandGate` (denied ⇒
   `Blocked`, approval needed ⇒ `Blocked` + `approvalId`). Limits: max 64
   checks, per-check timeout (default 10 min, max 1 h), overall run budget
   (default 30 min, max 2 h), one active run per workspace, cancellation
   (`quality.cancel`, emergency stop). Tests are skipped when the build failed.
4. **Parse** (`parse/`): libtest, Jest, Vitest, pytest, go test, dotnet,
   Maven Surefire, Gradle summaries + failures; rustc/clippy, tsc, ESLint,
   MSBuild, javac and generic `file:line:col` diagnostics. Output is redacted
   and only the tail (8 KiB) is stored.
5. **Events**: `BuildStarted`, `BuildSucceeded`/`BuildFailed` (non-test
   checks) and `TestRunFinished` (test kinds) per check, scoped to
   project/task/agent; build ids are `<runId>:<checkId>`.
6. **Gates** (`gates.rs`): per-project gates (build, typecheck, lint, format,
   tests, no critical security finding, reviewer approved, acceptance criteria
   met), each `enabled` + `required`, and the Definition of Done checklist
   (code, tests, docs, migrations, rollback, review, AC). `quality.evaluate`
   combines the latest result per check with the review verdict
   (`ReviewStatusSource`, default reads the task; the review area plugs in via
   `QualityService::set_review_source`) and security findings
   (`SecurityFindingsSource`, default `Unknown`).

## RPC

`quality.detect`, `quality.getProfile`, `quality.setProfile`, `quality.run`,
`quality.runs`, `quality.getRun`, `quality.cancel`, `quality.gates`,
`quality.setGates`, `quality.evaluate`. TS: the run/check status type is
exported as `QualityCheckStatus` (`CheckStatus` is the system-check status).

## UI

- Tasks → task drawer → **Quality**: gates + DoD with reasons, run checks in
  the task worktree, results with expandable failures/diagnostics; file
  locations open the Code panel at the line.
- Status bar: build/test indicator for the active project; click → latest run.
- Settings → **Quality gates**: toggle gates/required, DoD items, edit check
  commands (kind, name, command, folder, timeout).
- Commands: «Run tests» (Ctrl+Shift+T), «Run build», «Run all checks»,
  «Configure quality gates».
