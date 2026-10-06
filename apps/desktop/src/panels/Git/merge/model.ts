/**
 * Pure helpers of the Merge Manager tab (no React, no RPC): order
 * validation mirroring the core (`analyze::validate_order`), tones for
 * statuses/gates, and small summaries.
 */
import type {
  MergeBranch,
  MergeGateStatus,
  MergeManagerProposal,
  MergeProposalStatus,
  MergeRecommendation,
} from "@orkestr/protocol";

import type { Tone } from "../model";

/** Statuses during which the proposal has a live background merge. */
export const ACTIVE_STATUSES: ReadonlySet<MergeProposalStatus> = new Set<MergeProposalStatus>([
  "AwaitingApproval",
  "Merging",
  "Conflicted",
  "Testing",
]);

/** Final statuses. */
export const FINAL_STATUSES: ReadonlySet<MergeProposalStatus> = new Set<MergeProposalStatus>([
  "Completed",
  "Failed",
  "Aborted",
  "Rejected",
  "Superseded",
]);

export function isActive(status: MergeProposalStatus): boolean {
  return ACTIVE_STATUSES.has(status);
}

export function isFinal(status: MergeProposalStatus): boolean {
  return FINAL_STATUSES.has(status);
}

export function statusTone(status: MergeProposalStatus): Tone {
  switch (status) {
    case "Completed":
    case "Superseded":
      return "success";
    case "Failed":
    case "Rejected":
      return "danger";
    case "Conflicted":
    case "AwaitingApproval":
      return "warning";
    case "Merging":
    case "Testing":
      return "accent";
    default:
      return "neutral";
  }
}

export function recommendationTone(r: MergeRecommendation): Tone {
  switch (r) {
    case "MergeNow":
      return "success";
    case "ResolveConflicts":
    case "ReviewFirst":
      return "warning";
    case "Blocked":
      return "danger";
    default:
      return "neutral";
  }
}

export function gateTone(status: MergeGateStatus): Tone {
  switch (status) {
    case "Passed":
      return "success";
    case "Failed":
      return "danger";
    case "Pending":
      return "warning";
    default:
      return "neutral";
  }
}

/** Moves `from` to index `to` (returns a new array). */
export function moveItem<T>(list: readonly T[], from: number, to: number): T[] {
  const out = list.slice();
  if (from < 0 || from >= out.length || to < 0 || to >= out.length || from === to) return out;
  const [item] = out.splice(from, 1);
  if (item !== undefined) out.splice(to, 0, item);
  return out;
}

/** A dependency violated by an order: `branch` would merge before `dependsOn`. */
export interface OrderViolation {
  branch: string;
  dependsOn: string;
}

/**
 * Checks an order against the branches' dependencies (same rule as the core):
 * every branch must come after each branch it depends on.
 */
export function orderViolations(order: readonly string[], branches: readonly MergeBranch[]): OrderViolation[] {
  const pos = new Map(order.map((b, i) => [b, i] as const));
  const out: OrderViolation[] = [];
  for (const b of branches) {
    const at = pos.get(b.branch);
    if (at === undefined) continue;
    for (const dep of b.dependsOn) {
      const d = pos.get(dep);
      if (d !== undefined && d > at) out.push({ branch: b.branch, dependsOn: dep });
    }
  }
  return out;
}

/** Branches in proposal order. */
export function orderedBranches(p: MergeManagerProposal): MergeBranch[] {
  const by = new Map(p.branches.map((b) => [b.branch, b] as const));
  const out: MergeBranch[] = [];
  for (const name of p.order) {
    const b = by.get(name);
    if (b) out.push(b);
  }
  for (const b of p.branches) if (!p.order.includes(b.branch)) out.push(b);
  return out;
}

/** Totals over the proposal's branches. */
export function proposalTotals(p: MergeManagerProposal): { files: number; additions: number; deletions: number } {
  let files = 0;
  let additions = 0;
  let deletions = 0;
  for (const b of p.branches) {
    files += b.diff.files.length;
    additions += b.diff.additions;
    deletions += b.diff.deletions;
  }
  return { files, additions, deletions };
}

/** Number of unresolved conflicted files of a stopped merge. */
export function openConflicts(p: MergeManagerProposal): number {
  return p.execution?.conflicts.filter((c) => !c.resolved).length ?? 0;
}

/** Whether `merge.execute` makes sense now (start or continue). */
export function canExecute(p: MergeManagerProposal): boolean {
  if (p.status === "Proposed") return p.blockers.length === 0 && p.recommendation !== "NothingToMerge";
  if (p.status === "Conflicted") return openConflicts(p) === 0;
  return false;
}

/** Whether `merge.abort` is allowed. */
export function canAbort(p: MergeManagerProposal): boolean {
  return p.status === "Proposed" || isActive(p.status);
}

/** Upserts a proposal into a newest-first list. */
export function upsertProposal(list: readonly MergeManagerProposal[], p: MergeManagerProposal): MergeManagerProposal[] {
  const i = list.findIndex((x) => x.id === p.id);
  if (i >= 0) {
    const out = list.slice();
    out[i] = p;
    return out;
  }
  return [p, ...list];
}
