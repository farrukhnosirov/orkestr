import type { ComputerAction, ComputerRunReport } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { appName, duration, naturalRunReply, progressiveTitle } from "./phrasing";

const report = (title: string, steps: ComputerAction[], o: { commandId?: string; fail?: number; error?: string } = {}): ComputerRunReport => ({
  runId: "r",
  ...(o.commandId ? { commandId: o.commandId } : {}),
  title,
  outcome: o.fail !== undefined ? "Failed" : "Succeeded",
  steps: steps.map((step, index) => ({ index, action: step.type, ok: index !== o.fail, detail: index === o.fail ? (o.error ?? "boom") : "ok", step })),
  effects: [],
  startedAt: "2026-10-24T00:00:00Z",
  durationMs: 5,
});
const first = { rnd: () => 0 };
const second = { rnd: () => 0.99 };

describe("natural replies (ru)", () => {
  it("speaks single actions in the present, with catalogue app names", async () => {
    await applyLanguagePreference("ru");
    const calc = report("Открыть калькулятор", [{ type: "Launch", target: "calculator" }], { commandId: "app.calculator.open" });
    expect(naturalRunReply(calc, first)).toBe("Открываю Калькулятор");
    expect(naturalRunReply(calc, { rnd: () => 0.5 })).toBe("Сейчас открою Калькулятор");
    expect(naturalRunReply(report("Открыть фигму", [{ type: "Launch", target: "figma" }], { commandId: "app.figma.open" }), first)).toBe("Открываю Фигму");
    expect(naturalRunReply(report("Закрыть проводник", [{ type: "CloseApp", app: "explorer", discard: false }], { commandId: "x" }), first)).toBe("Закрываю Проводник");
    expect(naturalRunReply(report("Громкость", [{ type: "Volume", op: "Set", value: 30 }], { commandId: "x" }), first)).toBe("Громкость — 30 процентов");
    expect(naturalRunReply(report("Громкость", [{ type: "Volume", op: "Set", value: "22" }], { commandId: "x" }), second)).toBe("Ставлю громкость на 22 процента");
    expect(naturalRunReply(report("Таймер", [{ type: "Timer", seconds: 300, message: "" }], { commandId: "x" }), first)).toBe("Ставлю таймер на 5 минут");
    expect(naturalRunReply(report("Прокрутить вниз", [{ type: "Scroll", direction: "Down", unit: "Lines" }], { commandId: "x" }), first)).toBe("Прокручиваю вниз");
    expect(naturalRunReply(report("Поиск", [{ type: "WebSearch", query: "погода" }], { commandId: "x" }), first)).toBe("Ищу в интернете «погода»");
    expect(naturalRunReply(report("Настройки: экрана", [{ type: "OpenSettings", page: "display" }], { commandId: "x" }), first)).toBe("Открываю настройки экрана");
    // unknown app: as said
    expect(naturalRunReply(report("Открыть приложение", [{ type: "Launch", target: "мойредактор" }], { commandId: "app.any.open" }), first)).toBe("Открываю мойредактор");
    // no rich template: the title's verb in the present
    expect(naturalRunReply(report("Копировать", [{ type: "Keys", combo: "ctrl+c" }], { commandId: "keys.copy" }), first)).toBe("Копирую");
    expect(naturalRunReply(report("Новая вкладка", [{ type: "Keys", combo: "ctrl+t" }], { commandId: "keys.tab" }), first)).toBe("Готово — новая вкладка");
    // a multi-step catalogue command keeps its title's verb
    expect(
      naturalRunReply(report("Перезапустить блокнот", [{ type: "CloseApp", app: "notepad", discard: false }, { type: "Wait", ms: 1500 }, { type: "Launch", target: "notepad" }], { commandId: "app.notepad.restart" }), first),
    ).toBe("Перезапускаю блокнот");
  });

  it("summarises AI plans in the past tense and phrases errors", async () => {
    await applyLanguagePreference("ru");
    const plan = report("write a list", [{ type: "Launch", target: "notepad" }, { type: "Wait", ms: 1500 }, { type: "TypeText", text: "milk" }]);
    expect(naturalRunReply(plan, { ...first, planned: true })).toBe("Готово — открыл Блокнот и напечатал текст");
    expect(naturalRunReply(plan, { ...second, planned: true })).toBe("Сделано: открыл Блокнот и напечатал текст");
    const failed = report("Открыть блокнот", [{ type: "Launch", target: "notepad" }], { commandId: "x", fail: 0, error: "not found" });
    expect(naturalRunReply(failed, first)).toBe("Не получилось открыть Блокнот: not found");
    expect(naturalRunReply(failed, second)).toBe("Не смог открыть Блокнот: not found");
    // no step details (older core): the generic error line
    const old = { ...failed, steps: failed.steps.map(({ step: _s, ...rest }) => rest) };
    expect(naturalRunReply(old, first)).toBe("Не удалось выполнить «Открыть блокнот»: not found");
  });

  it("units and verbs", async () => {
    await applyLanguagePreference("ru");
    expect(duration(60)).toBe("1 минуту");
    expect(duration(5400)).toBe("1 час 30 минут");
    expect(duration(45)).toBe("45 секунд");
    expect(progressiveTitle("Свернуть хром")).toBe("Сворачиваю хром");
    expect(progressiveTitle("Окно влево")).toBeNull();
  });
});

describe("natural replies (en)", () => {
  it("present, summary and error forms", async () => {
    await applyLanguagePreference("en");
    expect(appName("explorer")).toBe("File Explorer");
    expect(naturalRunReply(report("Open calculator", [{ type: "Launch", target: "calc.exe" }], { commandId: "x" }), first)).toBe("Opening Calculator");
    expect(naturalRunReply(report("Volume", [{ type: "Volume", op: "Set", value: 30 }], { commandId: "x" }), first)).toBe("Volume — 30 percent");
    expect(naturalRunReply(report("Timer", [{ type: "Timer", seconds: 300, message: "" }], { commandId: "x" }), first)).toBe("Setting a timer for 5 minutes");
    expect(naturalRunReply(report("Display settings", [{ type: "OpenSettings", page: "display" }], { commandId: "x" }), first)).toBe("Opening display settings");
    expect(naturalRunReply(report("Copy", [{ type: "Keys", combo: "ctrl+c" }], { commandId: "x" }), first)).toBe("Copying");
    const plan = report("list", [{ type: "Launch", target: "notepad" }, { type: "TypeText", text: "milk" }, { type: "Screenshot" }]);
    expect(naturalRunReply(plan, { ...first, planned: true })).toBe("Done — opened Notepad, typed the text and took a screenshot");
    expect(naturalRunReply(report("Close Word", [{ type: "CloseApp", app: "word", discard: false }], { commandId: "x", fail: 0, error: "no window" }), first)).toBe(
      "Couldn’t close Word: no window",
    );
  });
});
