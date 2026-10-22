# ADR-0042 — Infrastructure monitor and Bug Detective workflow

Status: accepted — implemented (Phase 7)

## Context

§27 asks for a live infrastructure monitor (#45) and a Bug Detective (#46)
that goes from a production symptom to a reviewed fix. The detective may
spend provider budget and write code, so it needs hard limits and human
gates.

## Decision

### Infrastructure monitor (`orkestr-monitor`, `monitor.*`)

1. **Metrics.** A bounded ring of snapshots per node, fed by the local
   `NodeMonitor` and by `NodeHub` snapshots of enrolled nodes (ADR-0033,
   through `set_node_source`). Series are derived on read — `cpu`, `mem`,
   `disk` and `gpuN` in percent — optionally averaged into
   `resolutionSecs` buckets (`monitor.overview`, `monitor.metrics`). GPU
   utilisation is not sampled yet (memory only where available).
2. **Health checks** (`monitor.checksList/checkUpsert/checkDelete/
   checkRun`). HTTP (method, expected status, body contains) and TCP probes
   with interval, timeout and `failureThreshold`. **Hysteresis:** `Up → Down`
   after `threshold` consecutive failures, `Down → Up` after `threshold`
   consecutive successes. Status keeps the last 60 samples (`history`). A
   transition publishes `HealthCheckChanged`.
3. **Logs.** `LogSource` trait (`id`, `query`, `tail`). Every source is a
   bounded **in-memory ring of 10 000 lines** (no persistence; lines ≤ 8 KiB)
   plus a broadcast channel for live tail; a slow consumer lags and counts
   dropped lines, so tailing never blocks the sender. Sources: files
   (tailer with a 64 KiB backfill), deployment logs (`deploy:<id>`, fed by
   `DeployService::recent` / `logs`), and the `monitor.ingest` webhook (Write
   rate class; local or token). **Every line is redacted before it is
   stored or streamed** (`orkestr_security::redact`: vault values, bearer
   tokens, provider keys; sensitive attribute names are masked).
4. **Alerts** (`monitor.alerts`, `monitor.ackAlert`). Rule kinds:
   `CheckDown` (resolved on `Up`), `MetricThreshold` (`metric > above` for
   `samples` consecutive samples; resolved after as many samples at or below
   — hysteresis) and `LogRate` (≥ `count` lines at `minLevel` matching an
   optional pattern within `windowSecs`; resolved after a quiet window).
   Alerts are deduplicated by `rule|source`: while one is open or
   acknowledged, new triggers only bump `count`. `AlertRaised` once per
   opening, `AlertResolved` once on clearing. **Default rules:**
   `check-down` (critical), `cpu-high` (> 95 % for 12 samples), `mem-high`
   (> 95 %, 6), `disk-high` (> 95 %, 2), `log-errors` (≥ 20 errors in 60 s,
   warning). **Rules over RPC** (`monitor.rules` read, `monitor.ruleUpsert`
   / `monitor.ruleDelete` write): every upsert is validated — id `1..64` of
   `[A-Za-z0-9._-]`, name `1..=120` chars, metric `cpu|mem|disk|gpuN`,
   `above` in `[0, 100)`, `samples` `1..=1000`, `count` `1..=100000`,
   `windowSecs` `1..=86400`, pattern `1..=200` chars, and a `CheckDown`
   `checkId` must exist. Defaults are seeded **once**: migration
   `monitor_0002_meta` adds `monitor_meta` with a `rules_seeded` flag
   (pre-existing databases with rules are marked seeded by the migration), so
   deleting every rule is permanent across restarts. The desktop Monitoring →
   Infrastructure → *Alert rules* tab edits them with a typed form per kind.
   **File log sources over RPC** (`monitor.sourceAdd` / `monitor.sourceRemove`,
   local-only): the path must be absolute and name an existing, readable
   regular file (directories, `\\.\` / `/dev/` device paths and other
   non-files are refused); the display name is unique (case-insensitive) and
   the same path cannot be added twice. Ingest streams and deployment logs are
   built in and cannot be removed. The *Log sources* tab lists every source
   and adds files through the native file dialog when the shell provides it.

### Bug Detective (`orkestr-detective`, `detective.*`)

5. **Stage machine.** `Opened → Collecting → Hypothesizing → Reproducing →
   AwaitingFixApproval → Fixing → Testing → Reviewing → AwaitingMergeApproval
   → Resolved`, with `Failed` and `Cancelled` reachable from every
   non-terminal stage. Each change publishes `IncidentStageChanged`
   (`IncidentOpened` on creation) and is audited (`detective.open`,
   `detective.stage`, `detective.merge`).
6. **Triggers.** `detective.open` from an alert id, a log line, a test run or
   a manual report. Auto-open on `AlertRaised` (only alerts that carry a
   project) is `DetectiveSettings.autoOpenOnAlert` (default **off**).
   Settings are read with `detective.settings` and saved with
   `detective.setSettings` (local-only; budget `(0, 1000]` USD, max fix
   attempts `1..=5`); the Bug Detective panel has a Settings form.
7. **Evidence.** `EvidenceCollector` trait (`kind`, `collect(incident,
   budget)`). Built-in collectors: the alert, logs around the incident and
   metrics (monitor reader), recent deployments and the deployed version,
   VCS commits and the diff since the last good deploy, quality failures,
   and code snippets from stack-trace `file:line` references. All content is
   redacted and capped by `EvidenceBudget` (default 20 items and 32 KiB per
   collector, 1 h look-back). Items get ids `ev1, ev2, …`.
8. **Hypothesis.** The provider must return JSON matching a strict schema:
   `summary`, `rootCause`, `confidence` (0–1), `evidenceRefs` (existing `evN`
   ids), `suspectedFiles`, `proposedFix`, `reproduction` (command or null).
   An invalid answer is sent back with the validation error; **at most 3
   attempts**, then the incident is `Failed`. The reproduction runs through
   the quality executor and is expected to fail.
9. **Two human gates via approvals.** Both gates create approval requests
   (answerable in the Approvals inbox or with `detective.approveFix` /
   `detective.approveMerge`); rejection → `Cancelled`, expiry → `Failed`.
   After gate 1 an agent writes the fix on branch **`detective/<id>`** in its
   own worktree (up to `maxFixAttempts` = 2); tests run through the quality
   executor and the reviews service reviews the diff. Gate 2
   (`detective.approveMerge`, dangerous class) merges `detective/<id>` into
   the project's checked-out branch → `Resolved`. **No auto-merge**; the
   project branch is untouched before gate 2.
10. **Budget.** Each incident has a cap in USD (`(0, 1000]`, default 2.0)
    covering model, agent and review spend; exceeding it fails the incident.
11. **Stop and restart.** `detective.cancel` and emergency stop
    (`cancel_all`) work at every stage. After a core restart, incidents at
    a gate stage re-watch their approval; incidents in any other running
    stage become `Failed("interrupted: Orkestr restarted during <stage>")`.

## Consequences

- The detective needs a provider that follows the schema; acceptance uses a
  scripted mock provider keyed on the Bug Detective system prompt.
- Deferred: external APM ingestion (Sentry and similar), GitHub PR creation
  (the detective ends with a local branch and a merge approval), persistent
  log storage. No dedicated events for rule / source / settings changes: the
  mutating calls return the stored value and the UI reloads.
