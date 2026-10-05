import type { MergeBranch, MergeManagerProposal } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { hasConflictMarkers } from "./ConflictResolver";
import {
  canAbort,
  canExecute,
  moveItem,
  openConflicts,
  orderViolations,
  orderedBranches,
  proposalTotals,
  upsertProposal,
} from "./model";

function branch(name: string, dependsOn: string[] = [], additions = 1): MergeBranch {
  return {
    branch: name,
    commit: `c-${name}`,
    title: name,
    dirty: false,
    dependsOn,
    commitsAhead: 1,
    diff: {
      files: [{ path: `${name}.rs`, status: "Added", additions, deletions: 0, hunks: 1, hunkHeaders: [], binary: false }],
      additions,
      deletions: 0,
      symbols: [],
      risks: [],
      truncated: false,
    },
    gates: {
      task: { status: "NotRequired" },
      review: { status: "Unknown" },
      tests: { status: "Unknown" },
      acceptance: { status: "NotRequired" },
    },
    predictedConflicts: [],
    state: "Pending",
  };
}

function proposal(over: Partial<MergeManagerProposal> = {}): MergeManagerProposal {
  return {
    id: "p1",
    projectId: "proj",
    target: "main",
    targetCommit: "t",
    order: ["a", "b"],
    branches: [branch("b", ["a"], 2), branch("a", [], 3)],
    conflicts: [],
    blockers: [],
    recommendation: "ReviewFirst",
    status: "Proposed",
    policy: {
      mode: "Manual",
      requireReview: true,
      requireTests: true,
      requireAcceptance: false,
      runQuality: true,
      cleanup: "RemoveWorktrees",
    },
    autoMergeable: false,
    createdAt: "2026-10-05T00:00:00Z",
    updatedAt: "2026-10-05T00:00:00Z",
    ...over,
  };
}

describe("merge model", () => {
  it("moves items without mutating", () => {
    const list = ["a", "b", "c"];
    expect(moveItem(list, 0, 2)).toEqual(["b", "c", "a"]);
    expect(moveItem(list, 2, 0)).toEqual(["c", "a", "b"]);
    expect(moveItem(list, 5, 0)).toEqual(list);
    expect(list).toEqual(["a", "b", "c"]);
  });

  it("validates the order against dependencies", () => {
    const p = proposal();
    expect(orderViolations(["a", "b"], p.branches)).toEqual([]);
    expect(orderViolations(["b", "a"], p.branches)).toEqual([{ branch: "b", dependsOn: "a" }]);
  });

  it("orders branches and totals the diff", () => {
    const p = proposal();
    expect(orderedBranches(p).map((b) => b.branch)).toEqual(["a", "b"]);
    expect(proposalTotals(p)).toEqual({ files: 2, additions: 5, deletions: 0 });
  });

  it("gates execute and abort by status", () => {
    expect(canExecute(proposal())).toBe(true);
    expect(canExecute(proposal({ blockers: [{ kind: "TargetDirty" }] }))).toBe(false);
    expect(canExecute(proposal({ recommendation: "NothingToMerge" }))).toBe(false);
    const conflicted = proposal({
      status: "Conflicted",
      execution: {
        autoApproved: false,
        nextIndex: 0,
        conflicts: [{ path: "x", binary: false, resolved: false }],
        cleaned: [],
      },
    });
    expect(openConflicts(conflicted)).toBe(1);
    expect(canExecute(conflicted)).toBe(false);
    expect(canAbort(conflicted)).toBe(true);
    expect(canAbort(proposal({ status: "Completed" }))).toBe(false);
  });

  it("upserts proposals newest first", () => {
    const a = proposal({ id: "a" });
    const b = proposal({ id: "b" });
    expect(upsertProposal([a], b).map((p) => p.id)).toEqual(["b", "a"]);
    expect(upsertProposal([a, b], { ...a, status: "Merging" })[0]?.status).toBe("Merging");
  });

  it("detects conflict markers", () => {
    expect(hasConflictMarkers("a\n<<<<<<< HEAD\nb\n=======\nc\n>>>>>>> x\n")).toBe(true);
    expect(hasConflictMarkers("const a = '=====';\n")).toBe(false);
  });
});
