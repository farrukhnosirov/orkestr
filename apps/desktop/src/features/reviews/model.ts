/**
 * Pure helpers for reviews and consensus runs (no React, no RPC): grouping
 * findings by severity, picking the latest review of each kind, applying
 * `ReviewUpdated` / `ConsensusUpdated` events to lists.
 */
import type {
  ConsensusProposal,
  ConsensusRun,
  FindingSeverity,
  ModelRef,
  Review,
  ReviewFinding,
  ReviewKind,
  ReviewVerdict,
} from "@orkestr/protocol";

/** Severities from most to least severe. */
export const SEVERITIES: readonly FindingSeverity[] = ["Critical", "High", "Medium", "Low", "Info"];

/** Design-system tone of a severity. */
export const SEVERITY_TONE: Record<FindingSeverity, "danger" | "warning" | "info" | "neutral" | "violet"> = {
  Critical: "danger",
  High: "warning",
  Medium: "violet",
  Low: "info",
  Info: "neutral",
};

/** Design-system tone of a verdict. */
export const VERDICT_TONE: Record<ReviewVerdict, "success" | "warning" | "danger"> = {
  Approve: "success",
  RequestChanges: "warning",
  Reject: "danger",
};

export interface SeverityGroup {
  severity: FindingSeverity;
  findings: ReviewFinding[];
}

/** Findings grouped by severity (most severe first, empty groups dropped). */
export function groupBySeverity(findings: readonly ReviewFinding[]): SeverityGroup[] {
  return SEVERITIES.map((severity) => ({ severity, findings: findings.filter((f) => f.severity === severity) })).filter(
    (g) => g.findings.length > 0,
  );
}

/** Whether a finding still needs action (mirrors `FindingStatus::is_unresolved`). */
export function isUnresolved(f: ReviewFinding): boolean {
  return f.status === "Open" || f.status === "Confirmed";
}

/** Whether a finding blocks the quality gate (mirrors `ReviewFinding::is_blocking`). */
export function isBlocking(f: ReviewFinding): boolean {
  return f.severity === "Critical" && isUnresolved(f);
}

/** Whether a review is still in progress. */
export function isActive(r: Review): boolean {
  return r.status === "Running" || r.status === "Verifying";
}

/** Newest review of a kind (lists are newest first, but don't rely on it). */
export function latestOf(reviews: readonly Review[], kind: ReviewKind): Review | null {
  let best: Review | null = null;
  for (const r of reviews) {
    if (r.kind !== kind) continue;
    if (!best || r.createdAt > best.createdAt) best = r;
  }
  return best;
}

/** Inserts or replaces a review, keeping newest-first order. */
export function upsertReview(list: readonly Review[], review: Review): Review[] {
  const rest = list.filter((r) => r.id !== review.id);
  return [review, ...rest].sort((a, b) => (a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : 0));
}

/** Inserts or replaces a consensus run, keeping newest-first order. */
export function upsertRun(list: readonly ConsensusRun[], run: ConsensusRun): ConsensusRun[] {
  const rest = list.filter((r) => r.id !== run.id);
  return [run, ...rest].sort((a, b) => (a.createdAt < b.createdAt ? 1 : a.createdAt > b.createdAt ? -1 : 0));
}

/** `provider/model` label. */
export function modelLabel(m: ModelRef | null | undefined): string {
  return m ? m.model : "—";
}

/** Proposals ordered for display: the chosen one first, then by score. */
export function orderedProposals(run: ConsensusRun): ConsensusProposal[] {
  const chosen = run.decision?.chosenIndex ?? null;
  return [...run.proposals].sort((a, b) => {
    if (a.index === chosen) return -1;
    if (b.index === chosen) return 1;
    return (b.score ?? -1) - (a.score ?? -1) || a.index - b.index;
  });
}

/** Unique key of a model ref. */
export function modelKey(m: ModelRef): string {
  return `${m.providerId}\u0000${m.model}`;
}

/** Validation of a consensus request: returns an error key or null. */
export function validateConsensus(question: string, models: readonly ModelRef[], auto: boolean): "needQuestion" | "tooLong" | "needModels" | "tooManyModels" | null {
  const q = question.trim();
  if (!q) return "needQuestion";
  if (q.length > 8000) return "tooLong";
  if (auto) return null;
  const unique = new Set(models.filter((m) => m.model.trim()).map(modelKey));
  if (unique.size < 2) return "needModels";
  if (unique.size > 5) return "tooManyModels";
  return null;
}
