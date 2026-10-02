/**
 * Session Resume (§57) presentation logic: turns `system.resumeSummary`
 * counts into readable lines. Generic — unknown kinds get a readable label
 * derived from the kind name, so new engines need no changes here.
 */
import type { EventKindCount, ResumeSummary } from "@orkestr/protocol";

const LABELS: Record<string, [string, string]> = {
  ProjectCreated: ["project created", "projects created"],
  ProjectUpdated: ["project update", "project updates"],
  TaskCreated: ["task created", "tasks created"],
  TaskUpdated: ["task update", "task updates"],
  TaskStatusChanged: ["task status change", "task status changes"],
  AgentSpawned: ["agent spawned", "agents spawned"],
  AgentStopped: ["agent stopped", "agents stopped"],
  ToolCallFinished: ["tool call", "tool calls"],
  FileChanged: ["file change", "file changes"],
  BuildFailed: ["failed build", "failed builds"],
  BuildSucceeded: ["successful build", "successful builds"],
  TestRunFinished: ["test run", "test runs"],
  ReviewFinished: ["review", "reviews"],
  ApprovalRequested: ["approval request", "approval requests"],
  ApprovalResolved: ["approval resolved", "approvals resolved"],
  SnapshotCreated: ["snapshot", "snapshots"],
  RollbackPerformed: ["rollback", "rollbacks"],
  DecisionRecorded: ["decision recorded", "decisions recorded"],
  MemoryUpdated: ["memory update", "memory updates"],
  Notification: ["notification", "notifications"],
  EmergencyStop: ["emergency stop", "emergency stops"],
  BudgetThreshold: ["budget alert", "budget alerts"],
};

/** "BuildSucceeded" → "build succeeded". */
export function humanizeKind(kind: string): string {
  return kind
    .replace(/([a-z0-9])([A-Z])/g, "$1 $2")
    .toLowerCase()
    .trim();
}

export function describeCount(c: EventKindCount): string {
  const label = LABELS[c.kind];
  if (label) return `${c.count} ${c.count === 1 ? label[0] : label[1]}`;
  return `${c.count} × ${humanizeKind(c.kind)}`;
}

/** Up to `max` lines, most frequent first; the rest folded into "+N more". */
export function summaryLines(summary: ResumeSummary, max = 6): string[] {
  const lines = summary.counts.slice(0, max).map(describeCount);
  const rest = summary.counts.slice(max).reduce((n, c) => n + c.count, 0);
  if (rest > 0) lines.push(`+${rest} more event${rest === 1 ? "" : "s"}`);
  return lines;
}

/** Headline for the card. */
export function headline(summary: ResumeSummary, projectName: string | null): string {
  const where = projectName ? ` in ${projectName}` : "";
  if (summary.totalEvents === 0) {
    return summary.since ? `All quiet${where} since your last session` : `Welcome${where}`;
  }
  const n = summary.totalEvents;
  return `${n} event${n === 1 ? "" : "s"}${where} since ${summary.since ? "your last session" : "the beginning"}`;
}
