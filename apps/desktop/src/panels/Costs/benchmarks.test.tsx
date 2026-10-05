import type { AgentPerformanceRow, ContextReason, ModelBenchmarkRow } from "@orkestr/protocol";
import { cleanup, render, screen } from "@testing-library/react";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../i18n";
import { reasonText } from "../Tasks/ContextPreview";
import { barFraction, benchmarkRoleLabel, sortModelRows } from "./benchmarksModel";

const PROVIDER = "0190f0b0-0000-7000-8000-000000000001";

function row(model: string, over: Partial<ModelBenchmarkRow> = {}): ModelBenchmarkRow {
  return {
    providerId: PROVIDER,
    model,
    roles: ["Backend"],
    executions: 1,
    finished: 1,
    calls: 2,
    callSuccessRate: 1,
    firstPassSuccessRate: null,
    buildSuccessRate: null,
    defectsFoundAvg: null,
    retriesAvg: 0,
    latencyAvgMs: 500,
    latencyP50Ms: 500,
    latencyP95Ms: 900,
    durationAvgMs: 1000,
    inputTokens: 100,
    outputTokens: 50,
    tokensAvg: 150,
    costTotalUsd: 0.1,
    costAvgUsd: 0.1,
    routingScore: null,
    ...over,
  };
}

const mockState: { rows: unknown } = { rows: [] };
vi.mock("../../lib/rpcQuery", () => ({
  useRpcQuery: () => ({ data: mockState.rows, error: null, loading: false, reload: () => undefined }),
  errorMessage: (e: unknown) => String(e),
}));

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(async () => {
  cleanup();
  await i18n.changeLanguage("en");
});

describe("benchmark table helpers", () => {
  it("sorts by a column with missing values last in both directions", () => {
    const rows = [row("b", { firstPassSuccessRate: 0.5 }), row("a", { firstPassSuccessRate: null }), row("c", { firstPassSuccessRate: 0.9 })];
    expect(sortModelRows(rows, "firstPass", "desc").map((r) => r.model)).toEqual(["c", "b", "a"]);
    expect(sortModelRows(rows, "firstPass", "asc").map((r) => r.model)).toEqual(["b", "c", "a"]);
    expect(sortModelRows(rows, "model", "asc").map((r) => r.model)).toEqual(["a", "b", "c"]);
    expect(rows.map((r) => r.model)).toEqual(["b", "a", "c"]);
  });

  it("scales bars and labels roles", async () => {
    expect(barFraction(5, [5, 10, null])).toBe(0.5);
    expect(barFraction(null, [5])).toBe(0);
    expect(barFraction(3, [null])).toBe(0);
    expect(benchmarkRoleLabel("Custom thing")).toBe("Custom thing");
    const en = benchmarkRoleLabel("UI/UX");
    await i18n.changeLanguage("ru");
    expect(benchmarkRoleLabel("UI/UX")).not.toBe("");
    expect(benchmarkRoleLabel("Backend")).not.toBe("Backend");
    expect(en).not.toBe("UiUx");
  });
});

describe("context reasons", () => {
  it("explains reasons in English and Russian", async () => {
    const reasons: ContextReason[] = [
      { kind: "references", symbol: "compute_overtime", file: "src/payroll.rs" },
      { kind: "recentChange", commits: 3 },
      { kind: "rule", origin: "project", overridesGlobal: true },
      { kind: "memory", relevancePct: 80, tags: ["overtime"] },
    ];
    expect(reasons.map(reasonText)).toEqual([
      "uses compute_overtime from src/payroll.rs",
      "changed recently (3 commits)",
      "project rule, overrides a global one",
      "relevance 80% · tags overtime",
    ]);
    await i18n.changeLanguage("ru");
    expect(reasonText({ kind: "recentChange", commits: 5 })).toBe("недавно менялся (5 коммитов)");
    expect(reasonText({ kind: "imports", file: "a.ts" })).toBe("импортирует a.ts");
  });
});

describe("benchmark views", () => {
  it("renders the model table with rates and the agent table", async () => {
    const { ModelBenchmarks, AgentPerformance } = await import("./Benchmarks");
    mockState.rows = [row("mock-smart", { firstPassSuccessRate: 1, routingScore: 0.82 }), row("other", { executions: 0, calls: 1 })];
    render(<ModelBenchmarks projectId={null} />);
    expect(screen.getByText("Model benchmarks")).toBeTruthy();
    expect(screen.getAllByText("mock-smart").length).toBeGreaterThan(0);
    expect(screen.getAllByText("100%").length).toBeGreaterThan(0);
    expect(screen.getByRole("button", { name: "Sort by Runs" })).toBeTruthy();
    cleanup();

    const agentRow: AgentPerformanceRow = {
      key: "t1",
      name: "Backend Engineer",
      role: "Backend",
      templateId: "t1",
      agentId: null,
      executions: 3,
      completed: 2,
      failed: 1,
      cancelled: 0,
      running: 0,
      firstPassSuccessRate: 0.5,
      reviewRejectionRate: 0.25,
      reviewRejections: 1,
      postMergeBugs: 1,
      retries: 1,
      durationAvgMs: 60_000,
      costUsd: 1.5,
      tokens: 1000,
      models: ["mock-smart"],
    };
    mockState.rows = [agentRow];
    render(<AgentPerformance projectId={null} />);
    expect(screen.getByText("Backend Engineer")).toBeTruthy();
    expect(screen.getByText("25%")).toBeTruthy();
    cleanup();

    mockState.rows = [];
    render(<AgentPerformance projectId={null} />);
    expect(screen.getByText("No agent has run in this window yet.")).toBeTruthy();
  });
});
