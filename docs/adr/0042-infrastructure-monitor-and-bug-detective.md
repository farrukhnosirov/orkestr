# ADR-0042 — Infrastructure monitor and Bug Detective workflow

Status: accepted — in progress (Phase 7)

## Context

§27 asks for a live infrastructure monitor (#45) and a Bug Detective (#46)
that goes from a production symptom to a reviewed fix. The detective may
spend provider budget and write code, so it needs hard limits and human
gates.

## Decision

### Infrastructure monitor (`orkestr-monitor`, `monitor.*`)

1. **Metrics.** Ring buffers of CPU, RAM, GPU and disk per node, fed by the
   local `NodeMonitor` and by `NodeHub` heartbeats from enrolled nodes
   (ADR-0033). Read through `monitor.overview` and `monitor.metrics`.
2. **Health checks.** HTTP (status / body contains) and TCP probes with
   interval, timeout and failure threshold (hysteresis on up and down). A
   transition publishes `HealthCheckChanged`.
3. **Logs.** `LogSource` trait (`id`, `query`, `tail` as a broadcast
   receiver; lagging receivers drop lines, so tailing never blocks). Sources:
   files, deployment logs, and the `monitor.ingest` webhook (Write rate
   class; local or operator token). Every line is redacted before it is
   stored or streamed.
4. **Alerts.** Rules on checks, metric thresholds and log error rates.
   Alerts are deduplicated by rule plus source (a `count` field), resolved
   when the condition clears, and announced with `AlertRaised` /
   `AlertResolved`. `monitor.ackAlert` acknowledges an alert.

### Bug Detective (`orkestr-detective`, `detective.*`)

5. **Stage machine.** `Opened → Collecting → Hypothesizing → Reproducing →
   AwaitingFixApproval → Fixing → Testing → Reviewing → AwaitingMergeApproval
   → Resolved`, with `Failed` and `Cancelled` reachable from every
   non-terminal stage. Each change publishes `IncidentStageChanged`.
6. **Triggers.** An alert, a log error, a test failure or a manual report.
   Auto-open on `AlertRaised` is optional and off by default.
7. **Evidence.** `EvidenceCollector` trait (`kind`, `collect(incident,
   budget)`). Collectors cover logs and metrics (monitor traits),
   deployments (deployed version), recent VCS commits and diff, quality
   failures, and code snippets from stack-trace `file:line` references. All
   evidence is redacted and size-capped.
8. **Hypothesis.** The provider returns JSON matching a schema: summary, root
   cause, confidence, evidence refs, suspected files, proposed fix and a
   reproduction. The reproduction runs through the quality executor.
9. **Two human gates.** `detective.approveFix` lets an agent write the fix on
   branch `detective/<id>` in its own worktree. Tests and review follow.
   `detective.approveMerge` (dangerous class) merges through the existing
   merge path. There is no auto-merge and `main` is never touched before
   gate 2. Each incident has a budget cap. `detective.cancel` and emergency
   stop work at every stage.

## Consequences

- Deferred: external APM ingestion (Sentry and similar) and GitHub PR
  creation. The detective ends with a local branch and a merge approval.
