# Parallel execution (§11, §12, §38, §62.5, §68.24–26)

Phase 2 hardening of how many agents run at once, which ones, and what
happens to them when the UI moves elsewhere or the core restarts.

Code: `crates/task-engine/src/scheduler.rs` (pure admission checks),
`crates/orchestrator/src/parallel.rs` (the global `ParallelScheduler`, the
`GatedGateway` API gate and restart auto-resume), `crates/core/src/rpc/scheduler.rs`
(`scheduler.*`), UI in `apps/desktop/src/features/scheduler/`.

## One admission controller

Every live Manager run supervisor offers its `Ready` tasks to the single
`ParallelScheduler` of the core (`admit`). The decision is computed over the
ready tasks of **all** live runs (all projects) against every task that holds
an agent slot (`Running`/`Waiting`, all projects), so runs share capacity
instead of each assuming the machine is theirs. Checks, in order
(`scheduler.explain` returns each with its verdict):

| Check | Reason (`WaitReason.kind`) | Source |
|---|---|---|
| `global` | `globalLimit` | `maxParallelGlobal` |
| `project` | `projectLimit` | `projectCaps` / `maxParallelPerProject`, never above the run's own `maxParallelAgents` |
| `team` | `teamLimit` | `teamCaps`; a task's team is its `team:<name>` tag |
| `rateLimit` | `rateLimited` | provider back-off (below) |
| `provider` | `providerConcurrency` | `providerCaps`, else the provider's agent slots |
| `model` | `modelConcurrency` | `modelCaps` |
| `cpu` | `cpu` | live node sample + per-task weight vs `maxCpuPercent` |
| `memory` | `memory` | free RAM − task estimate vs `minFreeRamMb` |
| `disk` | `disk` | free space on the data volume vs `minFreeDiskMb` |
| `gpu` | `gpu` | GPU load vs `maxGpuPercent` (off by default) |
| `files` | `fileConflict` | predicted files (plan) ∪ files touched, same checkout |

Run-level holds are reported the same way: `paused` (user paused the run) and
`agentBudget` (the run spawned `maxAgents`).

**Progress guarantee.** CPU and GPU thresholds only hold work back while at
least one agent is already running; load caused by other programs can never
starve the queue. RAM and disk reserves always apply.

**Ordering and fairness.** The task-engine priority queue with aging (a task
waiting `agingMinutes` rises one class, up to High). With `fairShare`, among
equal effective priorities the least-loaded project goes first, so a project
that already runs agents cannot take every freed slot from another project.
Runs keep going whichever project the UI shows (§12): nothing in the scheduler
depends on UI state.

**Git conflict avoidance.** Two agents never edit the same file in the same
checkout. Agents with their own worktree may touch the same files, but the
overlap is flagged early: the supervisor checks in-flight agents every ≥ 15 s,
confirms with `git merge-tree` (≤ 6 previews per pass) and publishes
`MergeRiskDetected` plus a Manager note. All git mutations of one repository
are serialised by the `orkestr-vcs` per-repo lock (keyed by the common git
dir); `many_agents_create_worktrees_concurrently` stresses it with 10 agents
plus concurrent add/remove.

## API concurrency and rate limits

`GatedGateway` wraps the model gateway of agents and the Manager. Each request
waits for a per-provider (and optional per-model) semaphore permit, held for
the life of the stream; waiting is cancellable. `scheduler.status` shows
in-flight and queued requests per provider.

A rate limit — a `ProviderRateLimited` event from the provider layer (listened
to only while Manager runs are registered, so no permanent bus subscriber keeps
node heartbeats running), or a
rate-limit error / rate-limit fallback seen through the gate — puts the
provider into back-off: `retryAfter` when the provider sent one, otherwise
`rateLimitBackoffSecs` doubling per hit (reset after 10 quiet minutes), capped
at 10 minutes. New agents routed to that provider wait with `rateLimited`.

## Background agents and restart recovery

On shutdown, live runs become `Interrupted` (a run the user had paused keeps
that fact in its reason) and the crash-recovery hook marks in-flight tasks
`Interrupted`. After start, `Manager::auto_resume_interrupted` decides per
interrupted run (and per interrupted stand-alone agent task):

* policy `autoResume`: `off` (always wait), `followApproval` (default; resume
  unless the run or Manager approval policy is *Ask*), `always`;
* never when Stop All AI is active, the user had paused the run, or a task was
  interrupted more than `maxAutoResumeAttempts` times (bounded loop);
* resuming: tasks go `Interrupted → Recovering → Ready`; with git the new
  agent's worktree is based on the interrupted agent's branch (last
  checkpoint) and the runtime injects the previous handoff;
* otherwise the run waits and the Manager conversation says why.

## RPC and events

| Method | Params → Result |
|---|---|
| `scheduler.status` | `{projectId?}` → `SchedulerStatus` (settings, counts, running, queue with reasons and positions, provider loads, project loads, host resources, merge risks) |
| `scheduler.setLimits` | `{settings}` → `{settings}`; validated (`bad_request`), persisted in `scheduler_settings`, survives restarts |
| `scheduler.explain` | `{taskId}` → `SchedulerExplanation` (every check, the entry, a one-line summary) |

Events: `SchedulerUpdated` (ephemeral, topic `global`) when running / queued /
waiting counts or settings change; `MergeRiskDetected` (topic `project:<id>`).

## UI

* Tasks panel: scheduler bar under the toolbar — running / queued / waiting
  counts, the dominant wait reason ("Waiting: provider concurrency 2/2 (Mock)"),
  global and project capacity, providers in back-off, merge-risk badge, an
  expandable queue and the limits editor.
* Agents panel: **Queue** tab — every slot of the core across projects with
  reasons, aging, provider/model, "Why?" (admission checks), provider load and
  host resources.

Strings: namespace `scheduler` (en/ru).
