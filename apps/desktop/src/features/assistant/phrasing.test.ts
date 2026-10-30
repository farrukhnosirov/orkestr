import type { ComputerAction, ComputerRunReport } from "@orkestr/protocol";
import { describe, expect, it } from "vitest";

import { applyLanguagePreference } from "../../i18n";
import { applyPersona } from "../jarvis/persona";
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
    expect(naturalRunReply(calc, first)).toBe("Открываю Калькулятор.");
    expect(naturalRunReply(calc, { rnd: () => 0.3 })).toBe("Сейчас открою Калькулятор.");
    expect(naturalRunReply(report("Открыть фигму", [{ type: "Launch", target: "figma" }], { commandId: "app.figma.open" }), first)).toBe("Открываю Фигму.");
    expect(naturalRunReply(report("Закрыть проводник", [{ type: "CloseApp", app: "explorer", discard: false }], { commandId: "x" }), first)).toBe("Закрываю Проводник.");
    expect(naturalRunReply(report("Громкость", [{ type: "Volume", op: "Set", value: 30 }], { commandId: "x" }), first)).toBe("Громкость — 30 процентов.");
    expect(naturalRunReply(report("Громкость", [{ type: "Volume", op: "Set", value: "22" }], { commandId: "x" }), second)).toBe("Ставлю громкость на 22 процента.");
    expect(naturalRunReply(report("Таймер", [{ type: "Timer", seconds: 300, message: "" }], { commandId: "x" }), first)).toBe("Ставлю таймер на 5 минут.");
    expect(naturalRunReply(report("Прокрутить вниз", [{ type: "Scroll", direction: "Down", unit: "Lines" }], { commandId: "x" }), first)).toBe("Прокручиваю вниз.");
    expect(naturalRunReply(report("Поиск", [{ type: "WebSearch", query: "погода" }], { commandId: "x" }), first)).toBe("Ищу в интернете «погода».");
    expect(naturalRunReply(report("Настройки: экрана", [{ type: "OpenSettings", page: "display" }], { commandId: "x" }), first)).toBe("Открываю настройки экрана.");
    // unknown app: as said
    expect(naturalRunReply(report("Открыть приложение", [{ type: "Launch", target: "мойредактор" }], { commandId: "app.any.open" }), first)).toBe("Открываю мойредактор.");
    // no rich template: the title's verb in the present
    expect(naturalRunReply(report("Копировать", [{ type: "Keys", combo: "ctrl+c" }], { commandId: "keys.copy" }), first)).toBe("Копирую.");
    expect(naturalRunReply(report("Новая вкладка", [{ type: "Keys", combo: "ctrl+t" }], { commandId: "keys.tab" }), first)).toBe("Сделано: новая вкладка.");
    // a multi-step catalogue command keeps its title's verb
    expect(
      naturalRunReply(report("Перезапустить блокнот", [{ type: "CloseApp", app: "notepad", discard: false }, { type: "Wait", ms: 1500 }, { type: "Launch", target: "notepad" }], { commandId: "app.notepad.restart" }), first),
    ).toBe("Перезапускаю блокнот.");
  });

  it("summarises AI plans in the past tense and phrases errors", async () => {
    await applyLanguagePreference("ru");
    const plan = report("write a list", [{ type: "Launch", target: "notepad" }, { type: "Wait", ms: 1500 }, { type: "TypeText", text: "milk" }]);
    expect(naturalRunReply(plan, { ...first, planned: true })).toBe("Сделано: открыл Блокнот и напечатал текст.");
    expect(naturalRunReply(plan, { ...second, planned: true })).toBe("Выполнено: открыл Блокнот и напечатал текст.");
    const failed = report("Открыть блокнот", [{ type: "Launch", target: "notepad" }], { commandId: "x", fail: 0, error: "not found" });
    expect(naturalRunReply(failed, first)).toBe("Боюсь, не получилось открыть Блокнот: not found");
    expect(naturalRunReply(failed, second)).toBe("Увы, не смог открыть Блокнот: not found");
    // no step details (older core): the generic error line
    const old = { ...failed, steps: failed.steps.map(({ step: _s, ...rest }) => rest) };
    expect(naturalRunReply(old, first)).toBe("Боюсь, «Открыть блокнот» выполнить не удалось: not found");
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
    expect(naturalRunReply(report("Open calculator", [{ type: "Launch", target: "calc.exe" }], { commandId: "x" }), first)).toBe("Opening Calculator.");
    expect(naturalRunReply(report("Volume", [{ type: "Volume", op: "Set", value: 30 }], { commandId: "x" }), first)).toBe("Volume — 30 percent.");
    expect(naturalRunReply(report("Timer", [{ type: "Timer", seconds: 300, message: "" }], { commandId: "x" }), first)).toBe("Setting a timer for 5 minutes.");
    expect(naturalRunReply(report("Display settings", [{ type: "OpenSettings", page: "display" }], { commandId: "x" }), first)).toBe("Opening display settings.");
    expect(naturalRunReply(report("Copy", [{ type: "Keys", combo: "ctrl+c" }], { commandId: "x" }), first)).toBe("Copying.");
    const plan = report("list", [{ type: "Launch", target: "notepad" }, { type: "TypeText", text: "milk" }, { type: "Screenshot", scope: "Full", clipboard: false }]);
    expect(naturalRunReply(plan, { ...first, planned: true })).toBe("Done — opened Notepad, typed the text and took a screenshot.");
    expect(naturalRunReply(report("Close Word", [{ type: "CloseApp", app: "word", discard: false }], { commandId: "x", fail: 0, error: "no window" }), first)).toBe(
      "I’m afraid I couldn’t close Word: no window",
    );
  });
});

describe("Jarvis persona in command replies (ADR-0049)", () => {
  it("adds the form of address: сэр / босс / name / sir", async () => {
    await applyLanguagePreference("ru");
    const calc = report("Открыть калькулятор", [{ type: "Launch", target: "calculator" }], { commandId: "app.calculator.open" });
    const scroll = report("Прокрутить вниз", [{ type: "Scroll", direction: "Down", unit: "Lines" }], { commandId: "x" });
    const failed = report("Открыть блокнот", [{ type: "Launch", target: "notepad" }], { commandId: "x", fail: 0, error: "not found" });
    try {
      applyPersona({ persona: true, address: "Sir", userName: "" });
      expect(naturalRunReply(calc, first)).toBe("Открываю Калькулятор, сэр.");
      const yt = report("Открыть YouTube", [{ type: "OpenUrl", url: "https://www.youtube.com" }], { commandId: "site.youtube" });
      expect(naturalRunReply(yt, first)).toBe("Открываю YouTube, сэр.");
      expect(naturalRunReply(failed, first)).toBe("Боюсь, не получилось открыть Блокнот, сэр: not found");
      applyPersona({ persona: true, address: "Boss", userName: "" });
      expect(naturalRunReply(scroll, first)).toBe("Прокручиваю вниз, босс.");
      applyPersona({ persona: true, address: "Name", userName: "Фаррух" });
      expect(naturalRunReply(calc, first)).toBe("Открываю Калькулятор, Фаррух.");
      applyPersona({ persona: false, address: "Sir", userName: "" });
      expect(naturalRunReply(calc, first)).toBe("Открываю Калькулятор.");
      await applyLanguagePreference("en");
      applyPersona({ persona: true, address: "Sir", userName: "" });
      expect(naturalRunReply(report("Open calculator", [{ type: "Launch", target: "calc.exe" }], { commandId: "x" }), first)).toBe("Opening Calculator, sir.");
    } finally {
      applyPersona({ persona: false, address: "Sir", userName: "" });
    }
  });
});

describe("full PC control replies (ADR-0055)", () => {
  const withDetail = (title: string, step: ComputerAction, detail: string): ComputerRunReport => ({
    ...report(title, [step], { commandId: "x" }),
    steps: [{ index: 0, action: step.type, ok: true, detail, step }],
  });

  it("speaks every new action family in the Jarvis manner", async () => {
    await applyLanguagePreference("ru");
    try {
      applyPersona({ persona: true, address: "Sir", userName: "" });
      const say = (s: ComputerAction, detail = "ok") => naturalRunReply(withDetail("x", s, detail), { rnd: () => 0.5 });
      expect(say({ type: "Volume", op: "Set", value: 10 })).toBe("Громкость 10 процентов, сэр.");
      expect(say({ type: "Volume", op: "MuteOn" })).toMatch(/сэр\.$/);
      expect(say({ type: "Volume", op: "Down", value: 20, app: "spotify" })).toBe("Делаю Спотифай тише, сэр.");
      expect(say({ type: "Microphone", op: "MuteOn" })).toMatch(/^(Микрофон выключен|Вас больше не слышно), сэр\.$/);
      expect(say({ type: "AudioDevice", kind: "Output", name: "наушники" }, "Наушники (USB)")).toMatch(/Наушники \(USB\), сэр\.$/);
      expect(say({ type: "Window", op: "SnapTopRight" })).toBe("Окно в правом верхнем углу, сэр.");
      expect(naturalRunReply(withDetail("x", { type: "Window", op: "SnapLeft" }, "ok"), second)).toBe("Окно слева, сэр.");
      expect(say({ type: "MouseTo", point: "Center" }, '{"x":960,"y":540}')).toMatch(/^Курсор в центре( экрана)?, сэр\.$/);
      expect(say({ type: "KeyboardLayout", lang: "en" }, "en")).toMatch(/английский, сэр\.$/);
      expect(say({ type: "Query", what: "Time" }, '{"time":"14:05","iso":"2026-10-30T14:05:00+05:00"}')).toMatch(/14:05, сэр\.$/);
      expect(say({ type: "Query", what: "Battery" }, '{"percent":76,"charging":false,"plugged":false}')).toBe("Заряд 76 процентов, сэр.");
      expect(say({ type: "Query", what: "Battery" }, '{"percent":null}')).toMatch(/^У этого компьютера нет батареи, сэр/);
      expect(say({ type: "Query", what: "NowPlaying" }, '{"title":"Back in Black","artist":"AC/DC"}')).toBe("Сейчас играет AC/DC — Back in Black, сэр.");
      expect(say({ type: "Query", what: "NowPlaying" }, '{"none":true}')).toBe("Сейчас ничего не играет, сэр.");
      expect(say({ type: "Calculate", expression: "25*4" }, '{"expression":"25*4","result":"100"}')).toMatch(/100, сэр\.$/);
      expect(say({ type: "Radio", kind: "Wifi", on: false })).toBe("Выключаю Wi-Fi, сэр.");
      expect(say({ type: "FileSearch", name: "отчет" }, '{"count":0}')).toBe("Боюсь, файлов «отчет» не нашлось, сэр.");
      applyPersona({ persona: true, address: "Boss", userName: "" });
      expect(say({ type: "MouseTo", point: "Center" })).toMatch(/^Курсор в центре( экрана)?, босс\.$/);
      await applyLanguagePreference("en");
      applyPersona({ persona: true, address: "Sir", userName: "" });
      expect(say({ type: "Window", op: "Center" })).toBe("Window centered, sir.");
      expect(say({ type: "Query", what: "Volume" }, '{"percent":40,"muted":false}')).toBe("The volume is 40 percent, sir.");
    } finally {
      applyPersona({ persona: false, address: "Sir", userName: "" });
    }
  });
});
