import type { CostPoint, Project } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { buildScope } from "./BudgetEditor";
import { breakdownLabel, dayLabel, formatPct, formatTokens, formatUsd, levelLabel, limitLabel, niceMax, scopeLabel } from "./format";

const NBSP = "\u00a0";

beforeEach(async () => {
  await i18n.changeLanguage("en");
});

afterEach(async () => {
  await i18n.changeLanguage("en");
});
import { chartWidth, layoutBars } from "./SpendChart";

const ID = "0190f0b0-0000-7000-8000-000000000001";

function point(bucket: string, cost: number, est: number): CostPoint {
  return {
    bucket,
    totals: { costUsd: cost, estimatedCostUsd: est, inputTokens: 0, outputTokens: 0, cachedInputTokens: 0, requests: 1, failures: 0 },
  };
}

describe("cost formatting", () => {
  it("formats money, tokens and percent", () => {
    expect(formatUsd(1234.5)).toBe("$1,234.50");
    expect(formatUsd(0.0042)).toBe("$0.0042");
    expect(formatUsd(0)).toBe("$0.00");
    expect(formatUsd(Number.NaN)).toBe("—");
    expect(formatTokens(950)).toBe("950");
    expect(formatTokens(1500)).toBe("1.5K");
    expect(formatTokens(2_500_000)).toBe("2.5M");
    expect(formatPct(0.856)).toBe("86%");
    expect(formatPct(null)).toBe("—");
  });

  it("formats money, tokens and percent in Russian", () => {
    expect(formatUsd(1234.5, "ru")).toBe(`1${NBSP}234,50${NBSP}$`);
    expect(formatTokens(1500, "ru")).toBe(`1,5${NBSP}тыс.`);
    expect(formatTokens(2_500_000, "ru")).toBe(`2,5${NBSP}млн`);
    expect(formatPct(0.856, "ru")).toBe(`86${NBSP}%`);
    expect(dayLabel("2026-10-02", "ru")).toBe("2 окт.");
  });

  it("computes nice axis maxima", () => {
    expect(niceMax(0)).toBe(1);
    expect(niceMax(0.7)).toBe(1);
    expect(niceMax(1.3)).toBe(2);
    expect(niceMax(2.2)).toBe(2.5);
    expect(niceMax(42)).toBe(50);
    expect(niceMax(100)).toBe(100);
  });

  it("labels days and scopes", () => {
    expect(dayLabel("2026-10-02")).toBe("Oct 2");
    expect(dayLabel("bad")).toBe("bad");
    const projects = [{ id: ID, name: "Payroll" } as Project];
    expect(scopeLabel({ scope: "Global" }, projects)).toBe("All usage");
    expect(scopeLabel({ scope: "Project", projectId: ID }, projects)).toBe("Project · Payroll");
    expect(scopeLabel({ scope: "Provider", providerId: ID }, projects)).toBe("Provider · …000001");
  });
});

describe("budget scope form", () => {
  it("validates ids", () => {
    expect(buildScope("Global", "")).toEqual({ scope: "Global" });
    expect(buildScope("Project", ID)).toEqual({ scope: "Project", projectId: ID });
    expect(buildScope("Agent", ` ${ID} `)).toEqual({ scope: "Agent", agentId: ID });
    expect(buildScope("Task", "nope")).toBe("Enter a valid task id.");
  });

  it("localizes budget labels in Russian", async () => {
    await i18n.changeLanguage("ru");
    expect(buildScope("Provider", "nope")).toBe("Введите корректный ID провайдера.");
    expect(scopeLabel({ scope: "Global" }, [])).toBe("Весь расход");
    expect(scopeLabel({ scope: "Task", taskId: ID }, [])).toBe("Задача · …000001");
    expect(levelLabel("Warn")).toBe("Предупреждение");
    expect(limitLabel(100, "Monthly")).toBe(`100,00${NBSP}$ в месяц`);
    expect(i18n.t("costs:chart.total", { count: 21, total: "x" })).toBe("21 день · x");
    expect(i18n.t("costs:chart.total", { count: 3, total: "x" })).toBe("3 дня · x");
    expect(i18n.t("costs:chart.total", { count: 30, total: "x" })).toBe("30 дней · x");
  });
});

describe("spend chart layout", () => {
  it("stacks estimated on reported, scaled to the max", () => {
    const bars = layoutBars([point("2026-10-01", 10, 4), point("2026-10-02", 0, 0), point("2026-10-03", 5, 5)], 10);
    expect(bars).toHaveLength(3);
    const [a, b, c] = bars;
    expect(a && b && c).toBeTruthy();
    if (!a || !b || !c) return;
    // Bar heights are proportional (reported 6, estimated 4 of max 10).
    expect(a.reportedH / (a.reportedH + a.estimatedH + 2)).toBeCloseTo(0.6, 1);
    expect(a.estimatedY + a.estimatedH).toBeLessThanOrEqual(a.reportedY);
    expect(b.reportedH).toBe(0);
    expect(b.estimatedH).toBe(0);
    expect(c.reportedH).toBe(0);
    expect(c.estimatedH).toBeGreaterThan(0);
    expect(a.x).toBeLessThan(b.x);
  });

  it("draws at the measured pixel width so axis text does not scale with the panel", () => {
    // 1 SVG unit = 1 CSS px: the viewBox follows the card width (falls back to 640, never below 280).
    expect(chartWidth(1460)).toBe(1460);
    expect(chartWidth(null)).toBe(640);
    expect(chartWidth(120)).toBe(280);
    const pts = Array.from({ length: 30 }, (_, i) => point(`2026-09-${String(i + 1).padStart(2, "0")}`, 1, 0));
    const wide = layoutBars(pts, 1, 1460);
    const last = wide[wide.length - 1];
    expect(last && last.x + last.width).toBeLessThanOrEqual(1460 - 8);
    expect(last && last.x + last.width).toBeGreaterThan(1300);
  });
});

describe("breakdown labels", () => {
  const PROV = "0190f0b0-0000-7000-8000-0000f45a08a5";
  const names = {
    projects: new Map([[ID, "Shop"]]),
    providers: new Map([[PROV, "OpenAI work"]]),
    agents: new Map([["a1", "QA Engineer #1"]]),
    tasks: new Map([["t1", "Implement the module"]]),
  };
  it("resolves provider, agent, task and project names instead of id tails", () => {
    expect(breakdownLabel("Provider", { key: PROV, model: null }, names)).toBe("OpenAI work");
    expect(breakdownLabel("Agent", { key: "a1", model: null }, names)).toBe("QA Engineer #1");
    expect(breakdownLabel("Task", { key: "t1", model: null }, names)).toBe("Implement the module");
    expect(breakdownLabel("Project", { key: ID, model: null }, names)).toBe("Shop");
    expect(breakdownLabel("Model", { key: `${PROV}/gpt`, model: "gpt" }, names)).toBe("gpt");
  });
  it("falls back to a short id and labels unattributed usage", () => {
    expect(breakdownLabel("Provider", { key: "0190f0b0-0000-7000-8000-00000000dead", model: null }, names)).toBe("…0000dead");
    expect(breakdownLabel("Task", { key: "none", model: null }, names)).toBe("Unattributed");
  });
});
