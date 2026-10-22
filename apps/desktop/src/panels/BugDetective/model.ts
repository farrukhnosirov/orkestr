/** Pure helpers of the Bug Detective panel (ADR-0042). */
import type {
  Evidence,
  EvidenceKind,
  Incident,
  IncidentStage,
} from "@orkestr/protocol";

/** Happy-path stages shown on the timeline, in order. */
export const FLOW: IncidentStage[] = [
  "Opened",
  "Collecting",
  "Hypothesizing",
  "Reproducing",
  "AwaitingFixApproval",
  "Fixing",
  "Testing",
  "Reviewing",
  "AwaitingMergeApproval",
  "Resolved",
];

export const TERMINAL: ReadonlySet<IncidentStage> = new Set([
  "Resolved",
  "Failed",
  "Cancelled",
]);

export function isTerminal(stage: IncidentStage): boolean {
  return TERMINAL.has(stage);
}

export function isGate(stage: IncidentStage): boolean {
  return stage === "AwaitingFixApproval" || stage === "AwaitingMergeApproval";
}

export type StepState = "done" | "current" | "pending" | "failed" | "cancelled";

export interface TimelineStep {
  stage: IncidentStage;
  state: StepState;
}

/**
 * Timeline of an incident. A failed / cancelled incident has no record of the
 * stage it stopped in (the wire type keeps only the current stage), so the
 * caller passes the last non-terminal stage it saw (`lastActive`, from
 * `IncidentStageChanged` events); without it the stop is inferred from the
 * fields the run filled in.
 */
export function timeline(
  inc: Incident,
  lastActive?: IncidentStage,
): TimelineStep[] {
  if (inc.stage === "Failed" || inc.stage === "Cancelled") {
    const stop = FLOW.indexOf(lastActive ?? inferStop(inc));
    const end: StepState = inc.stage === "Failed" ? "failed" : "cancelled";
    return FLOW.slice(0, -1).map((stage, i) => ({
      stage,
      state: i < stop ? "done" : i === stop ? end : "pending",
    }));
  }
  const at = FLOW.indexOf(inc.stage);
  return FLOW.map((stage, i) => ({
    stage,
    state:
      i < at || (inc.stage === "Resolved" && i === at)
        ? "done"
        : i === at
          ? "current"
          : "pending",
  }));
}

/** Last stage a closed incident reached, inferred from its fields. */
export function inferStop(inc: Incident): IncidentStage {
  if (inc.reviewId) return "Reviewing";
  if (inc.fixTests) return "Testing";
  if (inc.fixBranch) return "Fixing";
  if (inc.reproduction) return "Reproducing";
  if (inc.hypothesis) return "Reproducing";
  if (inc.evidence.length) return "Hypothesizing";
  return "Collecting";
}

export type Tone =
  "neutral" | "accent" | "violet" | "success" | "warning" | "danger" | "info";

export function stageTone(stage: IncidentStage): Tone {
  switch (stage) {
    case "Resolved":
      return "success";
    case "Failed":
      return "danger";
    case "Cancelled":
      return "neutral";
    case "AwaitingFixApproval":
    case "AwaitingMergeApproval":
      return "warning";
    default:
      return "info";
  }
}

/** Confidence 0..1 → integer percent (clamped). */
export function confidencePct(c: number): number {
  if (!Number.isFinite(c)) return 0;
  return Math.round(Math.min(1, Math.max(0, c)) * 100);
}

export function confidenceTone(c: number): Tone {
  const p = confidencePct(c);
  return p >= 75 ? "success" : p >= 45 ? "warning" : "danger";
}

/** Budget usage as percent (0..100). */
export function budgetPct(inc: Incident): number {
  if (inc.budgetUsd <= 0) return 100;
  return Math.min(100, Math.round((inc.spentUsd / inc.budgetUsd) * 100));
}

export function usd(n: number): string {
  return `$${n.toFixed(n < 1 ? 3 : 2)}`;
}

/** Evidence grouped by kind, in a stable display order. */
const KIND_ORDER: EvidenceKind[] = [
  "Alert",
  "StackTrace",
  "CodeSnippet",
  "Logs",
  "Metrics",
  "Deployments",
  "Commits",
  "Diff",
  "QualityFailures",
];

export function groupEvidence(
  items: Evidence[],
): { kind: EvidenceKind; items: Evidence[] }[] {
  return KIND_ORDER.map((kind) => ({
    kind,
    items: items.filter((e) => e.kind === kind),
  })).filter((g) => g.items.length);
}

/** Incidents newest first; open ones before closed ones. */
export function sortIncidents(list: Incident[]): Incident[] {
  return [...list].sort((a, b) => {
    const t = Number(isTerminal(a.stage)) - Number(isTerminal(b.stage));
    return t !== 0 ? t : b.createdAt.localeCompare(a.createdAt);
  });
}

/** Short trigger label key + values. */
export type TriggerKey =
  "trigger.alert" | "trigger.log" | "trigger.test" | "trigger.manual";

export function triggerLabel(inc: Incident): {
  key: TriggerKey;
  values: Record<string, string>;
} {
  const tr = inc.trigger;
  switch (tr.type) {
    case "Alert":
      return { key: "trigger.alert", values: { id: tr.alertId.slice(0, 8) } };
    case "LogError":
      return { key: "trigger.log", values: { source: tr.sourceId } };
    case "TestFailure":
      return { key: "trigger.test", values: { run: tr.runId.slice(0, 8) } };
    default:
      return { key: "trigger.manual", values: {} };
  }
}
