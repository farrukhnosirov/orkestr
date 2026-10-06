import type { Event } from "@orkestr/protocol";
import { afterEach, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { categoryOf, describeEvent, severityOf } from "./describe";

const AGENT = "0190f0b0-0000-7000-8000-0000000abc12";

function ev(kind: string, payload: unknown): Event {
  return { id: "e1", seq: 1, ts: "2026-10-02T12:00:00Z", kind, projectId: null, payload } as unknown as Event;
}

afterEach(async () => {
  await i18n.changeLanguage("en");
});

describe("describeEvent", () => {
  it("describes events in English", async () => {
    await i18n.changeLanguage("en");
    expect(describeEvent(ev("AgentStopped", { agentId: AGENT, reason: "UserRequested", detail: null }))).toBe(
      "Agent …0abc12 stopped (stopped by user)",
    );
    expect(describeEvent(ev("FileChanged", { path: "src/a.ts", oldPath: null, change: "Modified", actor: "agent", agentId: null }))).toBe(
      "File modified: src/a.ts",
    );
    expect(describeEvent(ev("RollbackPerformed", { snapshotId: "s", scope: "Task", filesRestored: 1, preRollbackSnapshotId: null }))).toBe(
      "Rollback (task): 1 file restored",
    );
  });

  it("describes events in natural Russian with correct plurals and enum labels", async () => {
    await i18n.changeLanguage("ru");
    const rollback = (n: number) =>
      describeEvent(ev("RollbackPerformed", { snapshotId: "s", scope: "Workspace", filesRestored: n, preRollbackSnapshotId: null }));
    expect(rollback(1)).toBe("Откат (рабочая область): восстановлен 1 файл");
    expect(rollback(3)).toBe("Откат (рабочая область): восстановлено 3 файла");
    expect(rollback(5)).toBe("Откат (рабочая область): восстановлено 5 файлов");
    expect(rollback(21)).toBe("Откат (рабочая область): восстановлен 21 файл");
    expect(describeEvent(ev("AgentStopped", { agentId: AGENT, reason: "LimitReached", detail: null }))).toBe(
      "Агент …0abc12 остановлен (достигнут лимит)",
    );
    expect(
      describeEvent(ev("FileChanged", { path: "b.ts", oldPath: "a.ts", change: "Renamed", actor: "agent", agentId: null })),
    ).toBe("Файл a.ts переименован в b.ts");
    expect(describeEvent(ev("EmergencyStop", { scope: { scope: "All" }, agentsStopped: 2, triggeredBy: { type: "User" } }))).toBe(
      "ЭКСТРЕННАЯ ОСТАНОВКА (вся работа AI): остановлено 2 агента",
    );
    expect(describeEvent(ev("BudgetThreshold", { scope: "project", spentUsd: 8, limitUsd: 10, fraction: 0.8 }))).toBe(
      "Бюджет (проект): израсходовано 8,00 $ из 10,00 $",
    );
  });

  it("translates the provider fallback reason code", async () => {
    await i18n.changeLanguage("ru");
    const fallback = (reason: string) =>
      describeEvent(ev("ProviderFallback", { from: { providerId: "p", model: "gpt-a" }, to: { providerId: "p", model: "gpt-b" }, reason }));
    expect(fallback("rate_limited")).toBe("Переход на резервную модель: gpt-a → gpt-b (лимит запросов)");
    expect(fallback("brand_new")).toBe("Переход на резервную модель: gpt-a → gpt-b (brand_new)");
    await i18n.changeLanguage("en");
    expect(fallback("rate_limited")).toBe("Model fallback: gpt-a → gpt-b (rate limit)");
  });

  it("classifies events", () => {
    const e = ev("BuildFailed", { summary: "x" });
    expect(categoryOf(e)).toBe("build");
    expect(severityOf(e)).toBe("error");
  });
});
