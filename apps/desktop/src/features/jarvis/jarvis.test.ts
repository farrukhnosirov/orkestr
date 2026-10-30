import type { JarvisTemplateReply } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyLanguagePreference, t } from "../../i18n";
import type { AssistantRun } from "../assistant/api";
import type { ChatReply } from "./api";
import {
  addressSuffix,
  applyPersona,
  DEFAULT_PERSONA,
  firstGreetingOfDay,
  greeting,
  jarvisSay,
  pickVariant,
  resetVariants,
  timeOfDay,
} from "./persona";
import { type JarvisRouteDeps, routeJarvis } from "./router";
import { SentenceSpeaker, splitSentences, visibleText } from "./speech";
import { useJarvisStore } from "./store";

const run = (reply: string, outcome: AssistantRun["outcome"] = "ok"): AssistantRun => ({ outcome, reply, tone: outcome === "ok" ? "ok" : "error", speak: [] });
const tpl = (intent: string, text: string, o: Partial<JarvisTemplateReply> = {}): JarvisTemplateReply => ({
  intent,
  category: "test",
  text,
  score: 0.95,
  phrase: intent,
  ...o,
});

function deps(o: Partial<JarvisRouteDeps> = {}): JarvisRouteDeps & { calls: string[] } {
  const calls: string[] = [];
  return {
    calls,
    matchTemplate: vi.fn(async () => null),
    commandScore: vi.fn(async () => 0),
    runCommand: vi.fn(async (text: string, plan: boolean) => {
      calls.push(`run:${text}:${plan}`);
      return run(`ran ${text}`);
    }),
    chat: vi.fn(async (): Promise<ChatReply> => ({ messageId: "m", text: "AI.", model: "mock", command: null })),
    record: vi.fn(async (u: string, _r: string, s: string) => {
      calls.push(`record:${s}:${u}`);
    }),
    forget: vi.fn(async () => {
      calls.push("forget");
    }),
    setAddress: vi.fn(async (a) => {
      calls.push(`address:${a}`);
    }),
    stopSpeaking: vi.fn(() => {
      calls.push("silence");
    }),
    ...o,
  };
}

beforeEach(async () => {
  await applyLanguagePreference("ru");
  useJarvisStore.setState({ mode: "commands", entries: [], streamingId: null, lastReply: "", config: null });
  resetVariants();
});
afterEach(() => applyPersona({ ...DEFAULT_PERSONA, persona: false }));

describe("persona", () => {
  it("puts the form of address into every {{addr}} string", () => {
    applyPersona({ persona: true, address: "Sir", userName: "" });
    expect(addressSuffix()).toBe(", сэр");
    expect(t("voice:reply.cancelled")).toBe("Отменено, сэр.");
    applyPersona({ persona: true, address: "Boss", userName: "" });
    expect(t("assistant:reply.stopped")).toBe("Остановлено, босс.");
    applyPersona({ persona: true, address: "None", userName: "" });
    expect(t("voice:reply.cancelled")).toBe("Отменено.");
    applyPersona({ persona: false, address: "Sir", userName: "" });
    expect(t("voice:reply.cancelled")).toBe("Отменено.");
  });

  it("follows the UI language", async () => {
    applyPersona({ persona: true, address: "Sir", userName: "" });
    await applyLanguagePreference("en");
    applyPersona();
    expect(t("voice:reply.cancelled")).toBe("Cancelled, sir.");
  });

  it("rotates variants without repeating the previous one", () => {
    const seen: string[] = [];
    for (let i = 0; i < 6; i++) seen.push(pickVariant("k", ["a", "b", "c"], () => 0)!);
    for (let i = 1; i < seen.length; i++) expect(seen[i]).not.toBe(seen[i - 1]);
    applyPersona({ persona: true, address: "Sir", userName: "" });
    const a = jarvisSay("modeChat", {}, () => 0);
    const b = jarvisSay("modeChat", {}, () => 0);
    expect(a).toContain("сэр");
    expect(a).not.toBe(b);
  });

  it("greets by the time of day, once per day", () => {
    applyPersona({ persona: true, address: "Sir", userName: "" });
    expect(timeOfDay(new Date(2026, 9, 26, 7))).toBe("morning");
    expect(timeOfDay(new Date(2026, 9, 26, 14))).toBe("day");
    expect(timeOfDay(new Date(2026, 9, 26, 20))).toBe("evening");
    expect(timeOfDay(new Date(2026, 9, 26, 2))).toBe("night");
    expect(greeting(new Date(2026, 9, 26, 7), () => 0)).toBe("Доброе утро, сэр.");
    window.localStorage.removeItem("orkestr.jarvis.greetedOn");
    expect(firstGreetingOfDay(new Date(2026, 9, 26, 19), () => 0)).toBe("Добрый вечер, сэр.");
    expect(firstGreetingOfDay(new Date(2026, 9, 26, 21))).toBeNull();
    expect(firstGreetingOfDay(new Date(2026, 9, 27, 8), () => 0)).toContain("Доброе утро");
  });
});

describe("streaming speech", () => {
  it("splits complete sentences and hides the hand-off line", () => {
    expect(splitSentences("Первое предложение. Второе тоже! И хвост")).toEqual([["Первое предложение.", "Второе тоже!"], "И хвост"]);
    expect(splitSentences("Да. Конечно, сэр, сейчас. Далее")[0]).toEqual(["Да. Конечно, сэр, сейчас."]);
    expect(visibleText("Открываю YouTube, сэр.\n<<run: открой youtube>>")).toBe("Открываю YouTube, сэр.");
    expect(visibleText("**Жирный** текст <<ru")).toBe("Жирный текст");
  });

  it("speaks sentence by sentence, in order", async () => {
    const said: string[] = [];
    const sp = new SentenceSpeaker(async (s) => {
      await new Promise((r) => setTimeout(r, 1));
      said.push(s);
    });
    for (const d of ["Добрый ве", "чер, сэр. Сегодня ", "всё спокойно. Ещё", " что-нибудь?", "\n<<run: отк", "рой ютуб>>"]) sp.push(d);
    await sp.finish();
    expect(said).toEqual(["Добрый вечер, сэр.", "Сегодня всё спокойно.", "Ещё что-нибудь?"]);
  });
});

describe("routing order", () => {
  it("Jarvis actions come first: «режим разговора» switches the mode", async () => {
    const d = deps({ matchTemplate: async () => tpl("mode.chat", "Режим разговора, сэр.", { action: "mode.chat" }), commandScore: async () => 0.8 });
    const turn = await routeJarvis("режим разговора", { source: "voice", uiMode: "orchestra" }, d);
    expect(turn?.kind).toBe("action");
    expect(useJarvisStore.getState().mode).toBe("chat");
    expect(d.calls).toEqual([]);
    await routeJarvis("забудь разговор", { source: "voice", uiMode: "orchestra" }, deps({ matchTemplate: async () => tpl("memory.forget", "Забыто.", { action: "memory.forget" }) }));
    const silent = await routeJarvis("замолчи", { source: "voice", uiMode: "orchestra" }, deps({ matchTemplate: async () => tpl("speech.stop", "Умолкаю.", { action: "speech.stop" }) }));
    expect(silent?.spoken).toBe(true);
    const boss = deps({ matchTemplate: async () => tpl("address.boss", "Как скажете, босс.", { action: "address.boss" }) });
    await routeJarvis("называй меня босс", { source: "voice", uiMode: "orchestra" }, boss);
    expect(boss.calls).toEqual(["address:Boss"]);
  });

  it("a matched computer command beats small talk and is executed", async () => {
    const d = deps({ commandScore: async () => 0.93, matchTemplate: async () => tpl("weather", "Боюсь, датчиков нет.") });
    useJarvisStore.getState().setMode("chat");
    const turn = await routeJarvis("открой ютуб", { source: "voice", uiMode: "orchestra" }, d);
    expect(turn?.kind).toBe("command");
    expect(d.calls[0]).toBe("run:открой ютуб:false");
    expect(d.calls).toContain("record:command:открой ютуб");
    expect(d.chat).not.toHaveBeenCalled();
  });

  it("a near miss of a command asks «Вы имели в виду» instead of chatting", async () => {
    const d = deps({ commandScore: async () => 0.65 });
    useJarvisStore.getState().setMode("chat");
    const turn = await routeJarvis("громкость десять ну", { source: "voice", uiMode: "orchestra" }, d);
    expect(turn?.kind).toBe("command");
    expect(d.calls[0]).toBe("run:громкость десять ну:false");
    expect(d.chat).not.toHaveBeenCalled();
  });

  it("small talk is answered locally (no tokens) and remembered", async () => {
    const d = deps({ matchTemplate: async () => tpl("greet.hello", "Здравствуйте, сэр.") });
    useJarvisStore.getState().setMode("chat");
    const turn = await routeJarvis("привет", { source: "text", uiMode: "assistant" }, d);
    expect(turn).toMatchObject({ kind: "template", reply: "Здравствуйте, сэр." });
    expect(d.chat).not.toHaveBeenCalled();
    expect(d.calls).toEqual(["record:template:привет"]);
    expect(useJarvisStore.getState().lastReply).toBe("Здравствуйте, сэр.");
  });

  it("conversation mode: the AI answers, streamed and spoken sentence by sentence", async () => {
    useJarvisStore.getState().setMode("chat");
    const said: string[] = [];
    const partial: string[] = [];
    const d = deps({
      chat: async (_text, o) => {
        o.onDelta("Чёрная дыра — область, откуда не выходит даже свет", false);
        o.onDelta(", сэр. Её граница — горизонт событий.", false);
        return { messageId: "m1", text: "Чёрная дыра — область, откуда не выходит даже свет, сэр. Её граница — горизонт событий.", model: "mock", command: null };
      },
    });
    const turn = await routeJarvis("что такое чёрная дыра", { source: "voice", uiMode: "orchestra", speak: async (s) => void said.push(s), onPartial: (p) => partial.push(p) }, d);
    expect(turn?.kind).toBe("ai");
    expect(turn?.spoken).toBe(true);
    expect(said).toEqual(["Чёрная дыра — область, откуда не выходит даже свет, сэр.", "Её граница — горизонт событий."]);
    expect(partial[0]).toContain("Чёрная дыра");
    const last = useJarvisStore.getState().entries.at(-1);
    expect(last).toMatchObject({ role: "assistant", source: "ai", streaming: false });
  });

  it("a command handed off by the AI still runs («открой ютуб» inside a chat)", async () => {
    useJarvisStore.getState().setMode("chat");
    const d = deps({ chat: async () => ({ messageId: "m", text: "Открываю YouTube, сэр.", model: "mock", command: "открой youtube" }) });
    const turn = await routeJarvis("мне бы видео посмотреть", { source: "text", uiMode: "assistant" }, d);
    expect(turn?.command).toBe("открой youtube");
    expect(d.calls).toContain("run:открой youtube:true");
  });

  it("no model: a polite Jarvis explanation instead of an error code", async () => {
    applyPersona({ persona: true, address: "Sir", userName: "" });
    useJarvisStore.getState().setMode("chat");
    const d = deps({ chat: async () => Promise.reject(new Error("no chat model is available (connect a provider)")) });
    const turn = await routeJarvis("расскажи о квантах", { source: "voice", uiMode: "orchestra" }, d);
    expect(turn?.tone).toBe("error");
    expect(turn?.reply).toMatch(/сэр/);
    expect(turn?.reply).toMatch(/провайдер/);
  });

  it("command mode: unknown text gets the AI planner (Assistant UI) or goes back to Orkestr (Orchestra UI)", async () => {
    const d = deps();
    const planned = await routeJarvis("собери отчёт по продажам", { source: "text", uiMode: "assistant" }, d);
    expect(planned?.kind).toBe("plan");
    expect(d.calls).toEqual(["run:собери отчёт по продажам:true"]);
    const d2 = deps();
    expect(await routeJarvis("собери отчёт по продажам", { source: "voice", uiMode: "orchestra" }, d2)).toBeNull();
    expect(d2.chat).not.toHaveBeenCalled();
  });
});
