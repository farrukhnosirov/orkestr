import { afterAll, beforeAll, describe, expect, it, vi } from "vitest";

import { i18n } from "../../../i18n";
import { factsForModel, humanProjectName, naturalStatus, phraseNaturally } from "./natural";
import { EMPTY_QUALITY, type ProjectFacts } from "./summary";

const BASE: ProjectFacts = {
  running: 0,
  waiting: 0,
  blocked: 0,
  failed: 0,
  completedToday: 0,
  total: 5,
  activeAgents: 0,
  focus: null,
  approvals: 0,
  quality: EMPTY_QUALITY,
};
const f = (over: Partial<ProjectFacts>): ProjectFacts => ({ ...BASE, ...over });
const FAILED_BUILD = { ...EMPTY_QUALITY, lastBuild: { ok: false, at: "2026-10-05T10:00:00Z", summary: "tsc" } };

afterAll(async () => {
  await i18n.changeLanguage("en");
});

describe("natural templates (ru)", () => {
  beforeAll(async () => {
    await i18n.changeLanguage("ru");
  });
  it("idle + failed build + one blocked task → one flowing reply, no slug prefix", () => {
    const text = naturalStatus(f({ blocked: 1, quality: FAILED_BUILD }), { project: null, variant: 0 });
    expect(text).toBe("Всё спокойно: агенты отдыхают. Правда, последняя сборка упала, и одна задача заблокирована. Показать ошибки?");
  });
  it("plurals and agreement", () => {
    expect(naturalStatus(f({ running: 3, activeAgents: 2 }), { project: null, variant: 0 })).toBe("Работа кипит: в процессе 3 задачи, заняты 2 агента.");
    expect(naturalStatus(f({ running: 5, activeAgents: 1 }), { project: null, variant: 1 })).toBe("Докладываю: занят один агент, в работе 5 задач.");
    expect(naturalStatus(f({ failed: 2, approvals: 5 }), { project: null, variant: 0 })).toBe(
      "Всё спокойно: агенты отдыхают. Правда, 2 задачи завершились сбоем, и 5 запросов ждут вашего подтверждения.",
    );
    expect(naturalStatus(f({ completedToday: 21 }), { project: null, variant: 0 })).toContain("За сегодня закрыта 21 задача.");
  });
  it("names the project only when asked to, by its human name", () => {
    expect(naturalStatus(f({ total: 0 }), { project: humanProjectName("ai_office"), variant: 0 })).toBe("В проекте «ai office» задач пока нет.");
  });
});

describe("natural templates (en)", () => {
  beforeAll(async () => {
    await i18n.changeLanguage("en");
  });
  it("busy with problems", () => {
    expect(naturalStatus(f({ running: 1, activeAgents: 2, failed: 1, quality: { ...EMPTY_QUALITY, lastTests: { passed: 9, failed: 3, at: "" } } }), { project: null, variant: 0 })).toBe(
      "Work is underway: one task in progress and 2 agents are busy. But 3 tests are failing, and one task failed. Want me to show the errors?",
    );
  });
  it("quiet with a passing build", () => {
    const q = { ...EMPTY_QUALITY, lastBuild: { ok: true, at: "", summary: "" } };
    expect(naturalStatus(f({ quality: q }), { project: "Schoolify", variant: 1 })).toBe("In Schoolify, all calm at the moment — none of the agents are busy. The last build passed.");
  });
  it("facts for the model carry no IDs and say whether to name the project", () => {
    const text = factsForModel(f({ running: 2, quality: FAILED_BUILD }), null);
    expect(text).toContain("do not name it");
    expect(text).toContain("Last build: failed");
  });
});

describe("phraseNaturally", () => {
  it("uses the AI phrasing when it answers in time", async () => {
    expect(await phraseNaturally(async () => "  Всё спокойно.  ", "facts", "fallback")).toBe("Всё спокойно.");
  });
  it("falls back without a phraser, on errors and on empty answers", async () => {
    expect(await phraseNaturally(undefined, "facts", "fallback")).toBe("fallback");
    expect(await phraseNaturally(() => Promise.reject(new Error("no model")), "facts", "fallback")).toBe("fallback");
    expect(await phraseNaturally(async () => "   ", "facts", "fallback")).toBe("fallback");
  });
  it("falls back when the model is slower than the limit", async () => {
    vi.useFakeTimers();
    try {
      const slow = () => new Promise<string>((resolve) => setTimeout(() => resolve("late"), 10_000));
      const p = phraseNaturally(slow, "facts", "fallback", 2500);
      await vi.advanceTimersByTimeAsync(2600);
      expect(await p).toBe("fallback");
    } finally {
      vi.useRealTimers();
    }
  });
});
