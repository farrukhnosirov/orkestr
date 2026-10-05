/**
 * Pure reducers of the Mission Control event feed (unit-tested):
 *
 * - alerts — failures, rate limits, provider outages, budget thresholds,
 *   risky approval requests, emergency stops, core errors, review findings;
 * - quality runs — builds and test runs reconstructed from the
 *   `Build*` / `TestRunFinished` events.
 *
 * Text is never built here: alerts carry a kind + params, the widget
 * renders them through `t()`.
 */
import type { Event } from "@orkestr/protocol";

export type AlertKind =
  | "buildFailed"
  | "testsFailed"
  | "rateLimited"
  | "providerDown"
  | "providerDegraded"
  | "budget"
  | "agentError"
  | "taskFailed"
  | "emergency"
  | "approval"
  | "reviewFindings"
  | "coreError";

export type AlertSeverity = "critical" | "warning" | "info";

export interface Alert {
  /** Event id (stable, dedupe key). */
  id: string;
  seq: number;
  ts: string;
  kind: AlertKind;
  severity: AlertSeverity;
  projectId: string | null;
  agentId: string | null;
  taskId: string | null;
  /** Interpolation values for the localized title. */
  params: Record<string, string | number>;
  /** Free text from the core (summary / reason), shown verbatim as detail. */
  detail: string | null;
}

function base(ev: Event, kind: AlertKind, severity: AlertSeverity, params: Alert["params"] = {}, detail: string | null = null): Alert {
  return {
    id: ev.id,
    seq: ev.seq,
    ts: ev.ts,
    kind,
    severity,
    projectId: ev.projectId,
    agentId: ev.agentId,
    taskId: ev.taskId,
    params,
    detail,
  };
}

/** Maps an event to an alert, or null when the event is not alert-worthy. */
export function alertFromEvent(ev: Event): Alert | null {
  switch (ev.kind) {
    case "BuildFailed":
      return base(ev, "buildFailed", "critical", { code: ev.payload.exitCode ?? "—" }, ev.payload.summary || null);
    case "TestRunFinished":
      return ev.payload.failed > 0
        ? base(ev, "testsFailed", "critical", { count: ev.payload.failed, passed: ev.payload.passed }, ev.payload.summary || null)
        : null;
    case "ProviderRateLimited":
      return base(ev, "rateLimited", "warning", { model: ev.payload.model ?? "—", secs: ev.payload.retryAfterSecs ?? 0 });
    case "ProviderStatusChanged":
      if (ev.payload.health === "Down") return base(ev, "providerDown", "critical", {}, ev.payload.detail);
      if (ev.payload.health === "Degraded") return base(ev, "providerDegraded", "warning", {}, ev.payload.detail);
      return null;
    case "BudgetThreshold":
      return base(
        ev,
        "budget",
        ev.payload.fraction >= 1 ? "critical" : "warning",
        { percent: Math.round(ev.payload.fraction * 100), scope: ev.payload.scope, spent: ev.payload.spentUsd, limit: ev.payload.limitUsd },
      );
    case "AgentStateChanged":
      return ev.payload.to === "Error" ? base(ev, "agentError", "critical", {}, ev.payload.activity) : null;
    case "TaskStatusChanged":
      return ev.payload.to === "Failed" ? base(ev, "taskFailed", "warning", {}, ev.payload.reason) : null;
    case "EmergencyStop":
      return base(ev, "emergency", "critical", { count: ev.payload.agentsStopped });
    case "ApprovalRequested": {
      const risk = ev.payload.approval.risk;
      if (risk !== "High" && risk !== "Critical") return null;
      return base(ev, "approval", risk === "Critical" ? "critical" : "warning", { risk }, ev.payload.approval.actionSummary);
    }
    case "ReviewFinished":
      // INTEGRATOR(P2): security findings get their own event kind in the quality pipeline; map it here.
      return ev.payload.verdict === "Rejected" || ev.payload.verdict === "ChangesRequested"
        ? base(ev, "reviewFindings", ev.payload.verdict === "Rejected" ? "critical" : "warning", { count: ev.payload.findings }, ev.payload.summary || null)
        : null;
    case "Log":
      return ev.payload.level === "error" ? base(ev, "coreError", "warning", { target: ev.payload.target }, ev.payload.message) : null;
    default:
      return null;
  }
}

function newerFirst(a: { seq: number; ts: string }, b: { seq: number; ts: string }): number {
  // Ephemeral/local notices may share seq 0: fall back to time.
  if (a.seq !== b.seq && a.seq > 0 && b.seq > 0) return b.seq - a.seq;
  return Date.parse(b.ts) - Date.parse(a.ts);
}

/** Merges alerts (dedupe by id), newest first, capped. */
export function mergeAlerts(list: readonly Alert[], incoming: readonly Alert[], max = 100): Alert[] {
  if (incoming.length === 0) return list as Alert[];
  const byId = new Map(list.map((a) => [a.id, a]));
  for (const a of incoming) byId.set(a.id, a);
  return [...byId.values()].sort(newerFirst).slice(0, max);
}

// ── quality runs ────────────────────────────────────────────────────────

export type QualityKind = "build" | "test";
export type QualityStatus = "running" | "passed" | "failed";

/**
 * A build or test run as Mission Control shows it.
 * INTEGRATOR(P2): the quality pipeline exposes runs over RPC (`quality.*`);
 * bind them through `setQualitySource` in `data.ts` — this shape is the
 * contract the widget renders.
 */
export interface QualityRun {
  id: string;
  kind: QualityKind;
  status: QualityStatus;
  projectId: string | null;
  /** Command (builds) or summary. */
  label: string;
  startedAt: string;
  finishedAt: string | null;
  durationMs: number | null;
  passed: number | null;
  failed: number | null;
  skipped: number | null;
  seq: number;
}

/** Applies a build/test event to the run list (newest first, capped). */
export function applyQualityEvent(runs: readonly QualityRun[], ev: Event, max = 20): QualityRun[] {
  let next: QualityRun | null = null;
  const existing = (id: string) => runs.find((r) => r.id === id);
  switch (ev.kind) {
    case "BuildStarted":
      next = {
        id: ev.payload.buildId,
        kind: "build",
        status: "running",
        projectId: ev.projectId,
        label: ev.payload.command,
        startedAt: ev.ts,
        finishedAt: null,
        durationMs: null,
        passed: null,
        failed: null,
        skipped: null,
        seq: ev.seq,
      };
      break;
    case "BuildFailed":
    case "BuildSucceeded": {
      const prev = existing(ev.payload.buildId);
      next = {
        id: ev.payload.buildId,
        kind: "build",
        status: ev.kind === "BuildSucceeded" ? "passed" : "failed",
        projectId: ev.projectId ?? prev?.projectId ?? null,
        label: prev?.label || ev.payload.summary,
        startedAt: prev?.startedAt ?? ev.ts,
        finishedAt: ev.ts,
        durationMs: ev.payload.durationMs,
        passed: null,
        failed: null,
        skipped: null,
        seq: ev.seq,
      };
      break;
    }
    case "TestRunFinished":
      next = {
        id: ev.payload.runId,
        kind: "test",
        status: ev.payload.failed > 0 ? "failed" : "passed",
        projectId: ev.projectId,
        label: ev.payload.summary,
        startedAt: ev.ts,
        finishedAt: ev.ts,
        durationMs: ev.payload.durationMs,
        passed: ev.payload.passed,
        failed: ev.payload.failed,
        skipped: ev.payload.skipped,
        seq: ev.seq,
      };
      break;
    default:
      return runs as QualityRun[];
  }
  const run = next;
  const rest = runs.filter((r) => r.id !== run.id);
  return [run, ...rest].sort((a, b) => newerFirst({ seq: a.seq, ts: a.finishedAt ?? a.startedAt }, { seq: b.seq, ts: b.finishedAt ?? b.startedAt })).slice(0, max);
}

/** Event kinds that matter to the feed. */
export const FEED_EVENT_KINDS: ReadonlySet<Event["kind"]> = new Set<Event["kind"]>([
  "BuildStarted",
  "BuildFailed",
  "BuildSucceeded",
  "TestRunFinished",
  "ReviewFinished",
  "ProviderRateLimited",
  "ProviderStatusChanged",
  "BudgetThreshold",
  "AgentStateChanged",
  "TaskStatusChanged",
  "EmergencyStop",
  "ApprovalRequested",
  "Log",
]);
