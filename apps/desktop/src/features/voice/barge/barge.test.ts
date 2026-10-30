/** Stop-phrase classifier, conversation endings, the spotter client and the circle cues (ADR-0054). Pure. */
import type { BargeHit } from "@orkestr/protocol";
import { describe, expect, it, vi } from "vitest";

import { followUpLeft, INTERRUPT_MS, interruptProgress, ReactorDynamics } from "../jarvis/model";
import { parsePhrases } from "../settings/ConversationSettings";
import { VOICE_ENGINE_DEFAULTS } from "../testConfig";
import { classifyStop, isConversationEnd, isEchoOf, splitLeadingStop } from "./phrases";
import { BARGE_CHUNK_SAMPLES, BargeSpotterClient, type BargeTransport, floatToPcm16, PREROLL_SAMPLES } from "./spotter";

const C = VOICE_ENGINE_DEFAULTS.barge;
const kind = (text: string, spoken?: string) => classifyStop(text, C, spoken)?.kind ?? null;

describe("stop-phrase classifier", () => {
  it("knows hard and soft synonyms (ru/en), case and punctuation aside", () => {
    for (const p of ["стоп", "Хватит!", "прекрати", "отмена", "перестань", "достаточно", "не надо", "Stop.", "enough", "cancel"]) expect(kind(p), p).toBe("Hard");
    for (const p of ["окей", "Ок", "тихо", "тишина", "замолчи", "заткнись", "помолчи", "всё", "все", "shut up", "quiet"]) expect(kind(p), p).toBe("Soft");
  });

  it("accepts a short utterance around the phrase; hard wins over soft", () => {
    expect(kind("ну хватит уже")).toBe("Hard");
    expect(kind("Джарвис, стоп")).toBe("Hard");
    expect(kind("окей, стоп")).toBe("Hard");
    expect(classifyStop("не надо", C)?.phrase).toBe("не надо");
  });

  it("matches a truncated last word («хват») but not short fragments", () => {
    expect(kind("хват")).toBe("Hard");
    expect(kind("прекра")).toBe("Hard");
    expect(kind("замол")).toBe("Soft");
    expect(kind("ст")).toBeNull();
    expect(kind("хват уже")).toBeNull();
  });

  it("rejects noise and conversation", () => {
    for (const n of ["", "   ", "...", "открой ютуб", "мне хватит одного яблока на ужин сегодня", "стопка книг"]) expect(kind(n), n).toBeNull();
  });

  it("rejects self-echo: words the assistant is saying right now", () => {
    expect(kind("всё", "Всё готово, сэр.")).toBeNull();
    expect(kind("достаточно", "Места на диске достаточно.")).toBeNull();
    expect(kind("стоп", "Всё готово, сэр.")).toBe("Hard");
  });

  it("splits «стоп, открой ютуб» into the stop and the command", () => {
    expect(splitLeadingStop("Стоп, открой ютуб", C)).toEqual({ stop: { phrase: "стоп", kind: "Hard" }, rest: "открой ютуб" });
    expect(splitLeadingStop("ну хватит. Включи музыку", C)?.rest).toBe("Включи музыку");
    expect(splitLeadingStop("не надо", C)?.rest).toBe("");
    expect(splitLeadingStop("открой ютуб", C)).toBeNull();
  });
});

describe("conversation endings and echo", () => {
  it("recognizes endings", () => {
    for (const p of ["Спасибо, всё.", "пока", "На этом всё", "отбой", "спасибо, Джарвис", "That's all", "thanks", "bye"]) expect(isConversationEnd(p), p).toBe(true);
    for (const p of ["спасибо, а теперь открой почту", "открой ютуб", ""]) expect(isConversationEnd(p), p).toBe(false);
  });

  it("detects the assistant's own reply picked up by the mic", () => {
    expect(isEchoOf("здравствуйте сэр", "Здравствуйте, сэр.")).toBe(true);
    expect(isEchoOf("открываю ютуб сэр", "Открываю YouTube, сэр.")).toBe(false);
    expect(isEchoOf("открой почту", "Здравствуйте, сэр.")).toBe(false);
    expect(isEchoOf("что-нибудь", null)).toBe(false);
  });
});

describe("spotter client", () => {
  function fakeMic() {
    const listeners = new Set<(m: { type: "pcm"; data: Float32Array }) => void>();
    return {
      listeners,
      setRecording: vi.fn(),
      subscribe: (l: (m: { type: "pcm"; data: Float32Array }) => void) => {
        listeners.add(l);
        return () => listeners.delete(l);
      },
    };
  }

  it("converts PCM to 16-bit little endian", () => {
    const b = floatToPcm16(new Float32Array([0, 1, -1, 0.5]));
    const v = new DataView(b.buffer);
    expect([v.getInt16(0, true), v.getInt16(2, true), v.getInt16(4, true), v.getInt16(6, true)]).toEqual([0, 32767, -32768, 16384]);
  });

  it("streams ~100 ms chunks, one call at a time, and stops on a hit with the pre-roll", async () => {
    const mic = fakeMic();
    let resolveFeed: ((v: { hit: null | { phrase: string; kind: "Hard"; heard: string; confidence: number } }) => void) | null = null;
    const transport: BargeTransport = {
      start: async () => ({ sessionId: "barge-7", engine: "VoskGrammar", reason: null }),
      feed: vi.fn(() => new Promise<{ hit: BargeHit | null }>((r) => (resolveFeed = r))),
      stop: vi.fn(async () => undefined),
    };
    const onHit = vi.fn();
    const s = await BargeSpotterClient.start({ transport, mic: mic as never, spoken: () => "Открываю YouTube", onHit });
    expect(s).not.toBeNull();
    expect(mic.setRecording).toHaveBeenCalledWith(true);
    const push = (n: number) => mic.listeners.forEach((l) => l({ type: "pcm", data: new Float32Array(n) }));
    push(BARGE_CHUNK_SAMPLES / 2);
    expect(transport.feed).not.toHaveBeenCalled();
    push(BARGE_CHUNK_SAMPLES / 2);
    expect(transport.feed).toHaveBeenCalledTimes(1);
    expect(vi.mocked(transport.feed).mock.calls[0]?.[2]).toBe("Открываю YouTube");
    for (let i = 0; i < 20; i++) push(BARGE_CHUNK_SAMPLES); // arrives while the call is in flight: queued, capped
    expect(transport.feed).toHaveBeenCalledTimes(1);
    resolveFeed!({ hit: null });
    await vi.waitFor(() => expect(transport.feed).toHaveBeenCalledTimes(2));
    resolveFeed!({ hit: { phrase: "стоп", kind: "Hard", heard: "стоп", confidence: 1 } });
    await vi.waitFor(() => expect(onHit).toHaveBeenCalledTimes(1));
    const pre = onHit.mock.calls[0]?.[1] as Float32Array[];
    const preLen = pre.reduce((n, c) => n + c.length, 0);
    expect(preLen).toBeGreaterThan(0);
    expect(preLen).toBeLessThanOrEqual(PREROLL_SAMPLES + BARGE_CHUNK_SAMPLES * 20);
    expect(s!.running).toBe(false);
    expect(transport.stop).toHaveBeenCalledWith("barge-7");
    expect(mic.setRecording).toHaveBeenLastCalledWith(false);
  });

  it("returns null without a session (barge-in off / no offline engine)", async () => {
    const transport: BargeTransport = { start: async () => ({ sessionId: null, engine: null, reason: "noEngine" }), feed: vi.fn(), stop: vi.fn() };
    expect(await BargeSpotterClient.start({ transport, mic: fakeMic() as never, spoken: () => null, onHit: vi.fn() })).toBeNull();
  });
});

describe("circle cues", () => {
  it("interrupted animation and follow-up countdown", () => {
    expect(interruptProgress(null, 1000)).toBeNull();
    expect(interruptProgress(1000, 1000 + INTERRUPT_MS / 2)).toBeCloseTo(0.5);
    expect(interruptProgress(1000, 1000 + INTERRUPT_MS)).toBeNull();
    expect(followUpLeft({ until: 9000, ms: 8000 }, 5000)).toBeCloseTo(0.5);
    expect(followUpLeft({ until: 9000, ms: 8000 }, 10_000)).toBe(0);
    expect(followUpLeft(null, 0)).toBeNull();
    const settled = () => {
      const d = new ReactorDynamics();
      for (let t = 16; t < 3000; t += 16) d.step(t, 16, "listening", 0.3, null, {});
      return d;
    };
    const still = settled().step(3000, 16, "listening", 0.3, null, {});
    const cut = settled().step(3000, 16, "listening", 0.3, null, { interrupt: 0.5, countdown: 0.4 });
    expect(cut.interrupt).toBeCloseTo(1);
    expect(cut.scale).toBeLessThan(still.scale);
    expect(cut.countdown).toBeCloseTo(0.4);
    expect(still.countdown).toBe(-1);
  });
});

describe("settings", () => {
  it("parses edited phrase lists", () => {
    expect(parsePhrases("стоп, Хватит\nхватит;  не   надо ,, ")).toEqual(["стоп", "Хватит", "не надо"]);
    expect(parsePhrases("x".repeat(60))[0]).toHaveLength(40);
  });
});
