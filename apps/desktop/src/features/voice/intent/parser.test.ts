import { describe, expect, it } from "vitest";

import { type Intent, type IntentContext, needsConfirmation, parseIntent } from "./parser";
import { bestMatch, normalize, phoneticKey, similarity, stripWakePhrase, transliterate } from "./text";

const SCHOOLIFY = { id: "p1", name: "Schoolify" };
const THOMPSON = { id: "p2", name: "Thompson Finance" };
const ORKESTR = { id: "p3", name: "Оркестр Сайт" };

const CTX: IntentContext = {
  projects: [SCHOOLIFY, THOMPSON, ORKESTR],
  agents: [
    { id: "a1", name: "Backend Agent #1", projectId: "p1" },
    { id: "a2", name: "QA Engineer #1", projectId: "p1" },
    { id: "a3", name: "Frontend Agent #2", projectId: "p2" },
  ],
  activeProjectId: "p2",
  wakePhrases: ["Jarvis"],
};

function brief(i: Intent): string {
  switch (i.kind) {
    case "stopProject":
    case "runQa":
    case "projectStatus":
    case "switchProject":
      return `${i.kind}:${i.project?.id ?? "?"}`;
    case "showBuildErrors":
      return `${i.kind}:${i.project?.id ?? "?"}`;
    case "agentStatus":
      return `${i.kind}:${i.agent?.id ?? "?"}`;
    case "openPanel":
      return `${i.kind}:${i.panel}`;
    case "dnd":
      return `${i.kind}:${i.on ? "on" : "off"}`;
    default:
      return i.kind;
  }
}

describe("parseIntent — table (RU + EN)", () => {
  const table: ReadonlyArray<readonly [string, string]> = [
    // §33 examples
    ["Запусти QA по Schoolify", "runQa:p1"],
    ["Что делает Backend Agent?", "agentStatus:a1"],
    ["Останови Thompson Finance", "stopProject:p2"],
    ["Покажи ошибки сборки", "showBuildErrors:p2"],
    // §63 G
    ["Jarvis, что происходит со Schoolify?", "projectStatus:p1"],
    ["Джарвис, что происходит со Скулифай?", "projectStatus:p1"],
    ["Hey Jarvis, what's happening with Schoolify?", "projectStatus:p1"],
    // English
    ["run QA on Schoolify", "runQa:p1"],
    ["run the tests for thompson finance", "runQa:p2"],
    ["run tests", "runQa:p2"],
    ["what is the backend agent doing", "agentStatus:a1"],
    ["what's QA doing?", "agentStatus:a2"],
    ["stop Thompson Finance", "stopProject:p2"],
    ["stop all", "stopAll"],
    ["stop all agents", "stopAll"],
    ["emergency stop", "emergencyStop"],
    ["show build errors", "showBuildErrors:p2"],
    ["show build errors for Schoolify", "showBuildErrors:p1"],
    ["open terminal", "openPanel:terminal"],
    ["open mission control", "openPanel:monitoring"],
    ["show me the tasks", "openPanel:tasks"],
    ["switch to Schoolify", "switchProject:p1"],
    ["open Schoolify", "switchProject:p1"],
    ["approve", "approve"],
    ["reject it", "reject"],
    ["turn on do not disturb", "dnd:on"],
    ["status", "projectStatus:p2"],
    ["how is schoolify going", "projectStatus:p1"],
    // Russian
    ["запусти тесты для Thompson Finance", "runQa:p2"],
    ["протестируй скулифай", "runQa:p1"],
    ["чем занят фронтенд агент", "agentStatus:a3"],
    ["останови всё", "stopAll"],
    ["остановите всех агентов", "stopAll"],
    ["экстренная остановка", "emergencyStop"],
    ["открой терминал", "openPanel:terminal"],
    ["покажи задачи", "openPanel:tasks"],
    ["открой центр управления", "openPanel:monitoring"],
    ["открой настройки", "openPanel:settings"],
    ["переключись на Schoolify", "switchProject:p1"],
    ["перейди в проект Оркестр Сайт", "switchProject:p3"],
    ["подтверди", "approve"],
    ["отклони запрос", "reject"],
    ["выключи не беспокоить", "dnd:off"],
    ["как дела у Thompson Finance", "projectStatus:p2"],
    ["что происходит", "projectStatus:p2"],
    ["Пожалуйста, покажи ошибки сборки по Schoolify", "showBuildErrors:p1"],
    // Unknown → Manager
    ["Напиши миграцию для таблицы платежей", "askManager"],
    ["refactor the payroll module", "askManager"],
    ["", "empty"],
    ["Jarvis", "empty"],
  ];
  for (const [text, want] of table) {
    it(`${JSON.stringify(text)} → ${want}`, () => {
      expect(brief(parseIntent(text, CTX))).toBe(want);
    });
  }

  it("keeps the spoken name when no project matches", () => {
    const i = parseIntent("stop Zebra Bank", CTX);
    expect(i).toMatchObject({ kind: "stopProject", project: null, query: "zebra bank" });
  });

  it("uses the active project when none is named, and marks it", () => {
    const i = parseIntent("запусти тесты", CTX);
    expect(i).toMatchObject({ kind: "runQa", project: { id: "p2", via: "active" } });
  });

  it("asks the Manager with the original text", () => {
    expect(parseIntent("Jarvis, write a haiku", CTX)).toEqual({ kind: "askManager", text: "Jarvis, write a haiku" });
  });

  it("destructive intents need confirmation", () => {
    expect(needsConfirmation(parseIntent("stop Schoolify", CTX))).toBe(true);
    expect(needsConfirmation(parseIntent("reject", CTX))).toBe(true);
    expect(needsConfirmation(parseIntent("approve", CTX))).toBe(false);
    expect(needsConfirmation(parseIntent("emergency stop", CTX))).toBe(false);
  });
});

describe("text helpers", () => {
  it("normalizes punctuation, case and ё", () => {
    expect(normalize("  Jarvis,  ЧТО-то   ещё?! ")).toBe("jarvis что то еще");
  });

  it("transliterates and builds phonetic keys", () => {
    expect(transliterate("джарвис")).toBe("dzharvis");
    expect(phoneticKey("Jarvis")).toBe(phoneticKey("Джарвис"));
    expect(similarity("Schoolify", "Скулифай")).toBeGreaterThan(0.8);
    expect(similarity("Schoolify", "Thompson")).toBeLessThan(0.4);
  });

  it("finds entities inside longer phrases", () => {
    expect(bestMatch("qa по schoolify пожалуйста", CTX.projects)?.item.id).toBe("p1");
    expect(bestMatch("томпсон финанс", CTX.projects)?.item.id).toBe("p2");
    expect(bestMatch("zebra", CTX.projects)).toBeNull();
  });

  it("strips wake phrases", () => {
    expect(stripWakePhrase("Jarvis, stop all")).toBe("stop all");
    expect(stripWakePhrase("Эй, Джарвис, статус")).toBe("статус");
    expect(stripWakePhrase("Компьютер, статус", ["Компьютер"])).toBe("статус");
    expect(stripWakePhrase("jarvisville rocks")).toBe("jarvisville rocks");
  });
});
