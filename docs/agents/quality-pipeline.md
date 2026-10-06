# Quality pipeline and Self-Healing (§17, §58, §68.15–16)

Phase 2: what happens to an agent's result before it may reach the target
branch, and how a failing result is repaired without the user.

Code: `crates/orchestrator/src/pipeline.rs` (driver, stages, Merge stage),
`crates/orchestrator/src/healing.rs` (failure analysis, limits, fix briefing),
`crates/orchestrator/src/review.rs` + `adversarial.rs` (Review and Security
stages), `crates/quality/` (the quality runner: toolchain detection, checks,
output parsers, gates), `crates/orchestrator/src/merge/` (Merge Manager),
`crates/core/src/rpc/pipeline.rs` (`pipeline.*`). UI:
`apps/desktop/src/features/pipeline/` (task drawer section, run-card chips),
`features/quality/`, `features/reviews/`, `panels/Git/merge/`.

## Stages

```
Author ─► Review ─► Build ─► Test ─► Security ─► gates ─► completion ─► Merge
             ▲         │        │         │
             └── Self-Healing (analysis → fix agent → Build → Test → Review → Security)
```

| Stage | What runs | Passes when |
|---|---|---|
| Author | the agent in its own worktree (`agent/<role>-<id>` branch) | the agent submitted `done` (a `partial` result waits for the user) |
| Review | automatic code review of the branch diff (`reviews.request`) | verdict Approve; "changes requested" enters Self-Healing |
| Build | quality runner: `build`, `typecheck` checks of the worktree | every enabled check passed (none configured = Skipped) |
| Test | quality runner: `test` checks of the worktree | every enabled check passed |
| Security | adversarial review (`reviews.adversarial`): a different model tries to refute the change, the author's model verifies each finding | no unresolved critical finding |
| Merge | a Merge Manager proposal for the branch | merged (auto under an `Auto` merge policy with green gates), or `Waiting` for approval |

A task that changed no files (e.g. a review-only security task) completes
with its own report and has no pipeline.

The pipeline drives the task state machine through the task engine only:
`Running → Review → Testing → Completed`; a fix goes back to `Running`.

### The quality runner

Checks come from toolchain detection (`quality.detect`): cargo, npm / pnpm /
yarn scripts (`build`, `typecheck`, `lint`, `test`, `e2e`), pytest, go test,
dotnet test, Maven / Gradle, make. Commands run through the command gate and
the DangerousActionGuard. On Windows checks run in `cmd.exe` so npm-style
`.cmd` shims work without touching the PowerShell execution policy (see
`docs/platform/README.md`). Output is parsed into counts, failing tests
(name, file, line, message) and compiler diagnostics: libtest, Node's
built-in runner (`node --test`, spec and TAP reporters), Jest / Vitest,
pytest, go test, dotnet, Surefire, Gradle.

## Self-Healing

On a Build / Test / Review / Security failure:

1. **Analyse** (`healing::analyze_checks` / `analyze_review`): failing tests,
   compiler errors with `file:line`, review findings, and code excerpts
   around them (context optimizer), plus a failure signature.
2. **Decide** (`healing::decide`): stop when a limit is reached —
   `maxAttempts` (default 3), `maxWallMins` (60), `maxBudgetUsd`, or the same
   signature again (`stopOnRepeat`, "no progress").
3. **Fix**: a fix agent on a new worktree branched from the previous attempt,
   with the analysis as its briefing (`# Self-healing fix (attempt N of M)`);
   from the second attempt a different model when `escalateModel` is on.
4. Run Build → Test → Review → Security again.

When it stops, the pipeline is `Escalated` and asks the user through an
approval: approve = more attempts, reject = the task fails. Every attempt is
stored on the pipeline (`healing[]`) and shown in the task drawer.

## Merge

The Merge stage proposes the task's branch. Under `PipelineMergeMode::PerPolicy`
it merges right away only when the project's merge policy is `Auto` and every
gate is green; otherwise the stage waits for approval in the Merge Manager.
The user can also merge a whole run at once (`merge.propose {runId}` →
`merge.execute` → approval). When any proposal completes:

* other open proposals whose branches it merged become `Superseded`;
* the Merge stage of each merged branch's pipeline becomes `Passed`
  ("merged `<branch>` into `<target>`"), and a finished run's card and report
  are updated (`MergeCompletedHook`).

Merging always happens on a temporary integration branch with a pre-merge
snapshot; the quality checks run there before the target is fast-forwarded.

## Rollback

`snapshots.rollbackTask` undoes exactly the files a task changed. A task whose
Self-Healing attempts ran in several worktrees is rolled back over all of
them in the latest worktree: each file returns to its state before the first
attempt that touched it. Files changed by anyone else — after the task, or
between two attempts — are reported as conflicts and never overwritten.

## Configuration and API

* `pipeline.getConfig` / `pipeline.setConfig` — enabled stages, per task
  kind overrides, healing limits, merge mode (per project).
* `pipeline.get {taskId}` / `pipeline.list {projectId, runId?}` /
  `pipeline.start {taskId}`.
* Events: `PipelineUpdated`, `PipelineStageChanged`, `HealingAttemptStarted`,
  plus `BuildStarted`, `BuildSucceeded` / `BuildFailed`, `TestRunFinished`,
  `ReviewFinished`, `MergeProposalUpdated`.

Stage details and reasons are stored in English (logs, agent briefings); the
UI shows the machine-written forms in the UI language
(`features/pipeline/text.ts`) and run reports are rendered from the report's
structured fields (`panels/Manager/RunReport.tsx`).

## Acceptance

`scripts/smoke/phase2-acceptance.mjs` drives the real app: a Manager run with
an injected failing test (`[mock:heal-once]`) that Self-Healing fixes, real
`npm run build` / `npm run test` through the quality runner, standard and
adversarial reviews, a run-wide merge into `main` of a temp repo, and a
failed refactor (`[mock:heal-never]`) that escalates, is declined and is
rolled back without touching unrelated edits.

Last result: 47/47 checks on Windows (2026-10-06); screenshots in
`docs/screenshots/phase2/`.
