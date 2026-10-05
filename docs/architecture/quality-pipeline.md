# Quality pipeline and Self-Healing (§17, §68.15–16) — as built

Code: `crates/orchestrator/src/pipeline.rs` (driver, seams, RPC-facing API),
`crates/orchestrator/src/healing.rs` (analysis, limits, briefing),
`crates/orchestrator/src/pipeline_store.rs` (tables `task_pipelines`,
`pipeline_configs`, migration `orchestrator_0005_quality_pipeline`),
`crates/core/src/integration.rs` (`PipelineQuality` over the quality runner),
`crates/core/src/rpc/pipeline.rs`, UI `apps/desktop/src/features/pipeline/`.

## Stages

| Stage | What runs | Task state |
|---|---|---|
| Author | the agent implements in its worktree (`submit_result`) | `Running → Review/Testing` (agent runtime) |
| Review | automatic code review (`reviews.request`, transition on) | `Testing → Review → Testing` (approve) / `Running` (changes) / `Failed` (reject) |
| Build | quality runner, kinds `Build` + `Typecheck`, in the task worktree | `Testing` |
| Test | quality runner, kind `Test` | `Testing` |
| Security | adversarial review; unresolved critical findings block | `Testing` |
| (gates) | `quality.evaluate` — a failed *required* gate waits for the user | `Testing` |
| (completion) | approval policy `Ask` → waits for the user, otherwise `Testing → Completed` | `Completed` |
| Merge | Merge Manager proposal for the task's branch; merged right away when the merge policy is `Auto` and every gate is green (`PipelineMergeMode::PerPolicy`), else waits for approval in the Merge Manager | — |

Review and Security are skipped without git or without an agent branch (no
diff); Build/Test are skipped when no check of that kind is configured.
Stages are configured per project (`pipeline.getConfig` / `pipeline.setConfig`)
with per task-kind overrides (agent role name or task tag). With the pipeline
disabled, Manager runs keep the Phase 1 acceptance.

## Self-Healing loop

A failing Build / Test / Review / Security stage is turned into a
`FailureAnalysis`: failing tests, compiler errors (`file:line`, code), review
findings, code excerpts (exact windows around failure lines plus files ranked by
the context optimizer with the failing files as focus), and a normalised
**signature** (stage + sorted failing items, digits collapsed; FNV-1a).

`healing::decide` applies the limits in order: disabled (0 attempts) → **no
progress** (a submitted fix produced the same signature) → attempts
(`maxAttempts` + attempts granted by the user) → wall time of the pipeline →
budget (pipeline limit, the task's own budget, the run's remaining budget, and
any exceeded cost-engine budget for the project/task).

A fix attempt: the fix agent goes through the global parallel scheduler (own
queue), is spawned with `SpawnExtras { fix_loop, briefing }` on top of the
current agent branch (the runtime injects the previous handoff as usual; the
briefing — `# Self-healing fix (attempt N of M)` — is inserted after the task
header). Attempt 1 uses the author's model; from attempt 2 a different model
from routing (`Coding`) is used when one exists (otherwise noted). A `done`
fix always lands in `Testing`. Then Build → Test → Review → Security run again.

When a limit is hit the pipeline is **Escalated**: an approval asks the user
(approve = `maxAttempts` more attempts and the no-progress check is skipped
once; reject / expiry = the task fails). The Manager conversation gets a
notice in the UI language.

Records: every fix attempt is an agent execution; `HealingAttemptStarted`
counts as a *retry* of the author in the benchmarks (Testing → Running is
counted as a fix loop as before).

## Events

`PipelineUpdated` (whole record), `PipelineStageChanged`,
`HealingAttemptStarted` — scoped to the project and the task.

## Manager runs

The run supervisor starts one driver per task in `Review`/`Testing` (the
author agent is then "handled": no blind retry), mirrors the pipeline on the
run card (`RunTask.pipeline`), treats `AwaitingUser`/`Escalated` as "waiting
for your review", waits for live drivers (the merge stage follows completion)
before finishing, and the final report adds a *Quality pipeline* section and
`RunReport.pipelines` (gates, review/security verdicts, healing attempts, merge
outcome). Pause holds new stages and fix attempts; cancel / bounds / emergency
stop the drivers; a core shutdown leaves the record resumable.

## Mock scenarios

Task titles with `[mock:heal-once]` fail their tests first (a `node:test`
suite + `package.json` and a greeting without punctuation) and are fixed by
the first fix attempt; `[mock:heal-never]` never converge (no-progress
escalation). Tests: `crates/orchestrator/src/tests/pipeline.rs`.
