/** Merge Manager tab over a fake RPC client: list, detail, reorder validation, Russian rendering. */
import type { MergeBranch, MergeManagerProposal } from "@orkestr/protocol";
import { act, cleanup, fireEvent, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference } from "../../../i18n";

const calls: { method: string; params: unknown }[] = [];
let proposals: MergeManagerProposal[] = [];

vi.mock("../../../core/client", () => ({
  maybeClient: () => null,
  getClient: () => ({
    call: (method: string, params: unknown) => {
      calls.push({ method, params });
      switch (method) {
        case "merge.list":
          return Promise.resolve({ proposals });
        case "merge.policy":
          return Promise.resolve({ projectId: "proj", policy: proposals[0]?.policy });
        case "merge.reorder": {
          const p = proposals[0] as MergeManagerProposal;
          return Promise.resolve({ proposal: { ...p, order: (params as { order: string[] }).order } });
        }
        default:
          return Promise.reject(new Error(`unexpected ${method}`));
      }
    },
  }),
}));

const { MergeTab } = await import("./MergeTab");

function branch(name: string, dependsOn: string[] = []): MergeBranch {
  return {
    branch: name,
    commit: `c-${name}`,
    title: `Task ${name}`,
    dirty: false,
    dependsOn,
    commitsAhead: 2,
    diff: {
      files: [{ path: `src/${name}.rs`, status: "Added", additions: 4, deletions: 1, hunks: 2, hunkHeaders: ["@@ -0,0 +1,4 @@"], binary: false }],
      additions: 4,
      deletions: 1,
      symbols: [{ path: `src/${name}.rs`, name, kind: "fn", change: "Added", public: true, signatureChanged: false }],
      risks: name === "agent/api" ? [{ flag: "Migration", items: ["migrations/1.sql"] }] : [],
      truncated: false,
    },
    gates: {
      task: { status: "Passed" },
      review: { status: "Unknown" },
      tests: { status: "Failed", detail: "npm test" },
      acceptance: { status: "NotRequired" },
    },
    predictedConflicts: [],
    state: "Pending",
  };
}

const PROPOSAL: MergeManagerProposal = {
  id: "p1",
  projectId: "proj",
  target: "main",
  targetCommit: "abc",
  order: ["agent/api", "agent/ui"],
  branches: [branch("agent/api"), branch("agent/ui", ["agent/api"])],
  conflicts: [{ scope: "Pair", branch: "agent/api", other: "agent/ui", paths: ["src/shared.rs"] }],
  blockers: [],
  recommendation: "ResolveConflicts",
  status: "Proposed",
  policy: { mode: "Manual", requireReview: true, requireTests: true, requireAcceptance: false, runQuality: true, cleanup: "RemoveWorktrees" },
  autoMergeable: false,
  createdAt: "2026-10-05T10:00:00Z",
  updatedAt: "2026-10-05T10:00:00Z",
};

beforeEach(() => {
  calls.length = 0;
  proposals = [PROPOSAL];
});

afterEach(async () => {
  cleanup();
  await act(async () => {
    await applyLanguagePreference("en");
  });
});

describe("MergeTab", () => {
  it("shows the empty state", async () => {
    proposals = [];
    render(<MergeTab projectId="proj" currentBranch="main" />);
    expect(await screen.findByText("No merge proposals yet")).toBeTruthy();
  });

  it("renders a proposal and validates reordering against dependencies", async () => {
    render(<MergeTab projectId="proj" currentBranch="main" />);
    expect(await screen.findByText("Target: main")).toBeTruthy();
    expect(screen.getByText("Conflicts expected")).toBeTruthy();
    expect(screen.getByText("agent/api conflicts with agent/ui")).toBeTruthy();
    expect(screen.getAllByText("Tests: failed").length).toBe(2);
    expect(screen.getByText("Migration")).toBeTruthy();

    // ui depends on api → moving ui up is flagged and cannot be applied.
    const downs = screen.getAllByRole("button", { name: "Move down" });
    fireEvent.click(downs[0] as HTMLElement);
    expect(screen.getByText("agent/ui must merge after agent/api")).toBeTruthy();
    const apply = screen.getByRole("button", { name: "Apply order" }) as HTMLButtonElement;
    expect(apply.disabled).toBe(true);
    fireEvent.click(screen.getByRole("button", { name: "Reset" }));
    expect(screen.queryByText("agent/ui must merge after agent/api")).toBeNull();
    expect(calls.some((c) => c.method === "merge.reorder")).toBe(false);
  });

  it("renders in Russian", async () => {
    await act(async () => {
      await applyLanguagePreference("ru");
    });
    render(<MergeTab projectId="proj" currentBranch="main" />);
    expect(await screen.findByText("Целевая ветка: main")).toBeTruthy();
    expect(screen.getByText("Ожидаются конфликты")).toBeTruthy();
    expect(screen.getAllByText("Тесты: не пройдено").length).toBe(2);
    expect(screen.getAllByText("Впереди на 2 коммита").length).toBe(2);
    expect(screen.getByRole("button", { name: "Выполнить слияние…" })).toBeTruthy();
  });
});
