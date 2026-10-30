/**
 * ADR-0057: garbage heard from background noise / music / TV never reaches
 * the AI or the Manager. The core rejects what is not speech
 * (`voice.transcribe` → `rejected`); the router asks unsure free text for more.
 */
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { FOLLOW_UP_MIN_CONFIDENCE, freeTextAllowed } from "../../assistant/voiceRoute";
import { type JarvisRouteDeps, routeJarvis } from "../../jarvis/router";
import { useJarvisStore } from "../../jarvis/store";
import { applyLanguagePreference } from "../../../i18n";
import type { MicCapture } from "../audio/mic";
import type { WorkletOutMessage } from "../wake/workletProtocol";

const call = vi.fn();
vi.mock("../../../core/client", () => ({ maybeClient: () => ({ call }), getClient: () => ({ call }) }));

const { SttError, startStt } = await import("./engines");

function fakeMic(): MicCapture & { emit: (m: WorkletOutMessage) => void } {
  const ls = new Set<(m: WorkletOutMessage) => void>();
  return {
    subscribe: (l: (m: WorkletOutMessage) => void) => {
      ls.add(l);
      return () => ls.delete(l);
    },
    setRecording: () => {},
    emit: (m: WorkletOutMessage) => ls.forEach((l) => l(m)),
  } as unknown as MicCapture & { emit: (m: WorkletOutMessage) => void };
}

function loudPcm(): Float32Array {
  const a = new Float32Array(16_000);
  for (let i = 0; i < a.length; i++) a[i] = 0.3 * Math.sin(i / 5);
  return a;
}

beforeEach(async () => {
  await applyLanguagePreference("ru");
  call.mockReset();
  useJarvisStore.setState({ mode: "commands", entries: [], streamingId: null, lastReply: "", config: null });
});
afterEach(() => vi.useRealTimers());

describe("core noise gates in the PCM session", () => {
  it("a rejected recording is an SttError('rejected'), never a transcript", async () => {
    call.mockResolvedValue({ text: "", language: null, engine: "WhisperCpp", elapsedMs: 120, rejected: "Hallucination" });
    const mic = fakeMic();
    const s = startStt({ engine: "WhisperCpp", lang: "ru-RU", mic, autoEnd: false });
    mic.emit({ type: "pcm", data: loudPcm() } as WorkletOutMessage);
    const err = await s.finish().catch((e: unknown) => e);
    expect(err).toBeInstanceOf(SttError);
    expect((err as InstanceType<typeof SttError>).code).toBe("rejected");
  });

  it("the decoder confidence (0..100) is exposed as 0..1", async () => {
    call.mockResolvedValue({ text: "Открой ютуб", language: null, engine: "WhisperCpp", elapsedMs: 300, confidence: 87 });
    const mic = fakeMic();
    const s = startStt({ engine: "WhisperCpp", lang: "ru-RU", mic, autoEnd: false });
    mic.emit({ type: "pcm", data: loudPcm() } as WorkletOutMessage);
    expect(await s.finish()).toBe("Открой ютуб");
    expect(s.confidence?.()).toBeCloseTo(0.87);
  });
});

describe("free text needs a confident recognizer before it reaches an AI / the Manager", () => {
  it("drops noise turned into a couple of words («смещая музыка»)", () => {
    expect(freeTextAllowed("смещая музыка", 0.62)).toBe(false);
    expect(freeTextAllowed("смещая музыка", null)).toBe(false); // engines without a confidence
    expect(freeTextAllowed("Речь неужен, учитывая состав для зубов", 0.3)).toBe(false);
  });

  it("keeps clear questions and requests", () => {
    expect(freeTextAllowed("Какая погода завтра в Москве?", 0.9)).toBe(true);
    expect(freeTextAllowed("расскажи анекдот", 0.7)).toBe(true);
    expect(freeTextAllowed("Джарвис, что нового", 0.66, ["Джарвис"])).toBe(true);
    expect(freeTextAllowed("напомни купить молоко вечером", 0.7)).toBe(true);
    expect(freeTextAllowed("What time is it?", 0.8)).toBe(true);
  });

  it("typed text is never gated", () => {
    expect(freeTextAllowed("ага", undefined)).toBe(true);
  });

  it("in the follow-up window unaddressed statements need a very clear recognizer (TV nearby)", () => {
    const tv = "Сегодня в студии мы обсудим последние новости экономики";
    expect(freeTextAllowed(tv, 0.8, [], true)).toBe(false);
    expect(freeTextAllowed(tv, FOLLOW_UP_MIN_CONFIDENCE, [], true)).toBe(true);
    expect(freeTextAllowed("а почему так получилось", 0.8, [], true)).toBe(true);
    expect(freeTextAllowed("Джарвис, запиши это в заметки", 0.8, ["Джарвис"], true)).toBe(true);
  });

  it("an unsure turn ends as `unheard`: no AI conversation, no Manager hand-off", async () => {
    const chat = vi.fn();
    const runCommand = vi.fn();
    const deps: JarvisRouteDeps = {
      matchTemplate: vi.fn(async () => null),
      commandScore: vi.fn(async () => 0),
      runCommand,
      chat,
      record: vi.fn(async () => {}),
      forget: vi.fn(async () => {}),
      setAddress: vi.fn(async () => {}),
      stopSpeaking: vi.fn(),
    };
    for (const uiMode of ["orchestra", "assistant"] as const) {
      for (const mode of ["chat", "commands"] as const) {
        const turn = await routeJarvis("смещая музыка", { source: "voice", uiMode, mode, allowFreeText: false }, deps);
        expect(turn?.kind).toBe("unheard");
      }
    }
    expect(chat).not.toHaveBeenCalled();
    expect(runCommand).not.toHaveBeenCalled();
    // a known command still runs even when the recognizer was unsure
    const cmd = await routeJarvis("открой ютуб", { source: "voice", uiMode: "orchestra", allowFreeText: false }, { ...deps, commandScore: vi.fn(async () => 0.9), runCommand: vi.fn(async () => ({ outcome: "ok" as const, reply: "Открываю.", tone: "ok" as const, speak: [] })) });
    expect(cmd?.kind).toBe("command");
  });
});
