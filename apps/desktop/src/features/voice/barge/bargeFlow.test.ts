/**
 * Barge-in and continuous conversation through the real controller
 * (ADR-0054): a stop phrase silences speech and (hard) cancels the AI call,
 * nothing is said, a short window listens for the next command; after a
 * reply the assistant listens again without the wake word; silence or
 * «спасибо, всё» end the conversation quietly. Microphone, speech engines,
 * TTS and the core are fakes.
 */
import type { BargeHit, VoiceConfig } from "@orkestr/protocol";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import { i18n } from "../../../i18n";

const h = vi.hoisted(() => {
  type Listener = (m: { type: string; data?: Float32Array }) => void;
  const listeners = new Set<Listener>();
  const mic = {
    isClosed: false,
    analyser: null,
    subscribe: (l: Listener) => {
      listeners.add(l);
      return () => listeners.delete(l);
    },
    close: () => undefined,
    setBudget: () => undefined,
    setSegmenting: () => undefined,
    setRecording: vi.fn(),
  };
  /** The STT session the controller started last (the test decides what was heard). */
  const stt = {
    text: "" as string | Error,
    opts: [] as Array<{ noSpeechMs?: number; preroll?: readonly Float32Array[]; autoEnd: boolean }>,
    autoEnd: null as null | (() => void),
  };
  const startStt = vi.fn((o: { noSpeechMs?: number; preroll?: readonly Float32Array[]; autoEnd: boolean }) => {
    stt.opts.push(o);
    return {
      onInterim: () => undefined,
      onAutoEnd: (cb: () => void) => {
        stt.autoEnd = cb;
      },
      onSpeech: () => undefined,
      finish: async () => {
        if (stt.text instanceof Error) throw stt.text;
        return stt.text;
      },
      cancel: () => undefined,
    };
  });
  /** TTS: each utterance stays "playing" until stopSpeaking() or `endSpeech()`. */
  const tts = { hold: false, playing: [] as Array<() => void> };
  const speak = vi.fn(
    (_text: string) =>
      new Promise<void>((resolve) => {
        if (!tts.hold) resolve();
        else tts.playing.push(resolve);
      }),
  );
  const stopSpeaking = vi.fn(() => {
    const p = tts.playing;
    tts.playing = [];
    p.forEach((r) => r());
  });
  /** routeUtterance: replies at once, or waits for `release()` (an AI call in flight). */
  const route = {
    calls: [] as string[],
    reply: "Здравствуйте, сэр.",
    hold: false,
    release: null as null | ((reply: string) => void),
  };
  const routeUtterance = vi.fn(async (text: string) => {
    route.calls.push(text);
    const reply = route.hold ? await new Promise<string>((r) => (route.release = r)) : route.reply;
    return { outcome: "ok", reply, tone: "ok", speak: [] };
  });
  const playSound = vi.fn();
  return { listeners, mic, stt, startStt, tts, speak, stopSpeaking, route, routeUtterance, playSound };
});

vi.mock("../audio/mic", () => ({ MicCapture: { open: async () => h.mic } }));
vi.mock("../stt/engines", async (orig) => ({ ...(await orig<typeof import("../stt/engines")>()), startStt: h.startStt }));
vi.mock("../tts/speaker", async (orig) => ({ ...(await orig<typeof import("../tts/speaker")>()), speak: h.speak, stopSpeaking: h.stopSpeaking }));
vi.mock("../agentVoices", () => ({ assistantOptions: () => ({ engine: "System", lang: "ru-RU", ratePct: 100, pitchPct: 100 }) }));
vi.mock("../../assistant/voiceRoute", async (orig) => ({ ...(await orig<typeof import("../../assistant/voiceRoute")>()), routeUtterance: h.routeUtterance }));
vi.mock("../../sound/engine", () => ({ playSound: h.playSound }));
vi.mock("../../jarvis/greeting", () => ({ dailyGreeting: () => null }));

import { SttError } from "../stt/engines";
import { type BargeActions, cancelVoice, FOLLOW_UP_DELAY_MS, interrupt, say, setBargeActions, setBargeTransport, setVoiceDeps, simulateTranscript, startListening, stopListening, syncBarge } from "../controller";
import type { VoiceDeps } from "../pipeline/executor";
import { useVoiceStore } from "../store";
import { VOICE_ENGINE_DEFAULTS } from "../testConfig";

const CONFIG = {
  enabled: true,
  sttEngine: "WhisperCpp",
  sttConsent: null,
  ttsEngine: "System",
  ttsConsentAt: null,
  cloudProviderId: null,
  language: "ru",
  inputDeviceId: null,
  pushToTalkHotkey: "Ctrl+Space",
  spokenReplies: true,
  naturalReplies: false,
  systemVoice: null,
  cloudVoice: "alloy",
  ratePct: 100,
  pitchPct: 100,
  wake: { enabled: false, phrase: "Jarvis", sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: true },
  notifications: {
    enabled: false,
    quietHours: { enabled: false, start: "22:00", end: "08:00" },
    taskCompleted: true,
    taskFailed: true,
    approvalNeeded: true,
    buildFailed: true,
    budgetThreshold: true,
    deployFinished: true,
  },
  sound: { enabled: false, volumePct: 55, preset: "Auto", muteInDnd: true },
  agentVoices: {},
  ...VOICE_ENGINE_DEFAULTS,
} satisfies VoiceConfig;

const actions = {
  cancelReply: vi.fn(async () => undefined),
  assistantBusy: vi.fn(() => false),
  stopAssistant: vi.fn(async () => undefined),
  cursorActive: vi.fn(() => false),
  stopCursor: vi.fn(async () => undefined),
} satisfies BargeActions;

/** Fake core spotter: every feed returns `nextHit` (once). */
const transport = {
  nextHit: null as BargeHit | null,
  feeds: 0,
  spoken: [] as Array<string | null>,
  start: vi.fn(async () => ({ sessionId: "barge-1", engine: "VoskGrammar" as const, reason: null })),
  feed: vi.fn(async (_id: string, _pcm: string, spoken: string | null) => {
    transport.feeds += 1;
    transport.spoken.push(spoken);
    const hit = transport.nextHit;
    transport.nextHit = null;
    return { hit };
  }),
  stop: vi.fn(async () => undefined),
};

function pcm(): void {
  for (const l of [...h.listeners]) l({ type: "pcm", data: new Float32Array(1600).fill(0.1) });
}

const tick = () => new Promise((r) => setTimeout(r, 0));

beforeAll(async () => {
  await i18n.changeLanguage("ru");
  setVoiceDeps({ listAgents: async () => [], activeProjectId: () => null } as unknown as VoiceDeps);
  setBargeActions(actions);
  setBargeTransport(transport);
});

beforeEach(() => {
  useVoiceStore.setState({ config: structuredClone(CONFIG), phase: "idle", mic: [], reply: null, transcript: "", speaking: false, followUp: null, interruptedAt: null });
  h.tts.hold = false;
  h.route.hold = false;
  h.route.calls = [];
  h.route.release = null;
  h.route.reply = "Здравствуйте, сэр.";
  h.stt.text = "";
  h.stt.opts = [];
  transport.nextHit = null;
  transport.feeds = 0;
  transport.spoken = [];
});

afterEach(() => {
  cancelVoice();
  vi.clearAllMocks();
});

describe("barge-in", () => {
  it("«стоп» while Jarvis speaks: TTS stops, nothing is said, a 5 s window listens for the next command", async () => {
    h.tts.hold = true;
    useVoiceStore.setState({ phase: "result" });
    void say("Сегодня в Москве солнечно, плюс двенадцать, ветер северо-западный.");
    expect(useVoiceStore.getState().speaking).toBe(true);
    await syncBarge();
    expect(useVoiceStore.getState().mic).toContain("barge");
    transport.nextHit = { phrase: "стоп", kind: "Hard", heard: "стоп", confidence: 1 };
    pcm();
    await vi.waitFor(() => expect(useVoiceStore.getState().phase).toBe("listening"));
    const s = useVoiceStore.getState();
    expect(h.stopSpeaking).toHaveBeenCalled();
    expect(s.speaking).toBe(false);
    expect(s.interruptedAt).not.toBeNull();
    expect(s.followUp?.ms).toBe(5000);
    expect(actions.cancelReply).toHaveBeenCalled();
    // The sentence being spoken went along for self-echo protection.
    expect(transport.spoken[0]).toContain("солнечно");
    // «Слушаю, сэр.» is not spoken: only the interrupted sentence ever reached TTS.
    expect(h.speak).toHaveBeenCalledTimes(1);
    expect(h.playSound).not.toHaveBeenCalledWith("listenStart");
    // The follow-up capture got the pre-roll (the stop phrase and what follows it) and the 5 s window.
    const o = h.stt.opts.at(-1);
    expect(o?.noSpeechMs).toBe(5000);
    expect(o?.preroll?.length).toBeGreaterThan(0);
    expect(s.mic).not.toContain("barge");
  });

  it("«стоп, открой ютуб»: the stop phrase is dropped and the command runs", async () => {
    interrupt("Hard");
    await vi.waitFor(() => expect(h.startStt).toHaveBeenCalled());
    h.stt.text = "Стоп, открой ютуб";
    await stopListening();
    expect(h.route.calls).toEqual(["открой ютуб"]);
  });

  it("a stop phrase said after the wake word never reaches the command matcher (stop phrases first)", async () => {
    await startListening("WakeWord");
    h.stt.text = "хватит";
    await stopListening();
    expect(h.route.calls).toEqual([]);
    expect(useVoiceStore.getState().reply?.outcome).toBe("cancelled");
    expect(h.speak).not.toHaveBeenCalled();
  });

  it("a hard stop while the AI answers cancels the call and drops its late reply", async () => {
    h.route.hold = true;
    const turn = simulateTranscript("расскажи длинную историю");
    await vi.waitFor(() => expect(h.route.release).not.toBeNull());
    expect(useVoiceStore.getState().phase).toBe("thinking");
    interrupt("Hard", { listen: false });
    expect(actions.cancelReply).toHaveBeenCalledTimes(1);
    expect(useVoiceStore.getState().phase).toBe("idle");
    h.route.release?.("Жил-был…");
    await turn;
    expect(useVoiceStore.getState().reply).toBeNull();
    expect(h.speak).not.toHaveBeenCalled();
  });

  it("a soft phrase («окей») only silences speech: the running action goes on", async () => {
    actions.assistantBusy.mockReturnValue(true);
    h.route.hold = true;
    const turn = simulateTranscript("открой ютуб и включи музыку");
    await vi.waitFor(() => expect(h.route.release).not.toBeNull());
    interrupt("Soft");
    expect(actions.cancelReply).not.toHaveBeenCalled();
    expect(actions.stopAssistant).not.toHaveBeenCalled();
    expect(useVoiceStore.getState().phase).toBe("thinking");
    h.route.release?.("Открываю YouTube, сэр.");
    await turn;
    expect(useVoiceStore.getState().reply?.text).toBe("Открываю YouTube, сэр.");
    expect(h.speak).not.toHaveBeenCalled(); // silenced for the rest of the turn
    actions.assistantBusy.mockReturnValue(false);
  });

  it("«окей» cancels the action too when configured, and a hard stop stops a running computer command", async () => {
    actions.assistantBusy.mockReturnValue(true);
    useVoiceStore.setState({ config: { ...structuredClone(CONFIG), barge: { ...CONFIG.barge, softCancelsActions: true } }, phase: "thinking" });
    interrupt("Soft", { listen: false });
    expect(actions.cancelReply).toHaveBeenCalled();
    expect(actions.stopAssistant).toHaveBeenCalled();
    actions.assistantBusy.mockReturnValue(false);
  });

  it("cursor control: any stop phrase aborts it", async () => {
    actions.cursorActive.mockReturnValue(true);
    expect(await (async () => (await syncBarge(), useVoiceStore.getState().mic.includes("barge")))()).toBe(true);
    transport.nextHit = { phrase: "тихо", kind: "Soft", heard: "тихо", confidence: 1 };
    pcm();
    await vi.waitFor(() => expect(actions.stopCursor).toHaveBeenCalled());
    actions.cursorActive.mockReturnValue(false);
  });

  it("push-to-talk while the assistant works interrupts it and takes the new command", async () => {
    h.route.hold = true;
    void simulateTranscript("напиши мне эссе");
    await vi.waitFor(() => expect(h.route.release).not.toBeNull());
    await startListening("PushToTalk");
    expect(actions.cancelReply).toHaveBeenCalled();
    expect(useVoiceStore.getState().phase).toBe("listening");
    h.route.release?.("…");
  });

  it("barge-in off: no spotter, push-to-talk while working is ignored", async () => {
    useVoiceStore.setState({ config: { ...structuredClone(CONFIG), barge: { ...CONFIG.barge, enabled: false } }, speaking: true, phase: "result" });
    await syncBarge();
    expect(transport.start).not.toHaveBeenCalled();
    useVoiceStore.setState({ phase: "thinking", speaking: false });
    await startListening("PushToTalk");
    expect(useVoiceStore.getState().phase).toBe("thinking");
  });
});

describe("continuous conversation", () => {
  async function replied(): Promise<void> {
    const before = h.startStt.mock.calls.length;
    await simulateTranscript("привет");
    await vi.waitFor(() => expect(h.startStt.mock.calls.length).toBe(before + 1), { timeout: FOLLOW_UP_DELAY_MS + 1000 });
    expect(useVoiceStore.getState().phase).toBe("listening");
  }

  it("re-opens listening after the reply without the wake word, for the configured window", async () => {
    await replied();
    expect(h.speak).toHaveBeenCalledWith("Здравствуйте, сэр.", expect.anything());
    const s = useVoiceStore.getState();
    expect(s.followUp?.ms).toBe(8000);
    expect(h.stt.opts.at(-1)?.noSpeechMs).toBe(8000);
    // The next utterance continues the conversation, and listening re-opens again.
    h.route.reply = "Сейчас 14:05, сэр.";
    h.stt.text = "а сколько времени";
    await stopListening();
    expect(h.route.calls).toEqual(["привет", "а сколько времени"]);
    await vi.waitFor(() => expect(useVoiceStore.getState().phase).toBe("listening"), { timeout: FOLLOW_UP_DELAY_MS + 1000 });
    expect(h.startStt).toHaveBeenCalledTimes(2);
  });

  it("silence for the whole window closes quietly (soft chime, no error, nothing said)", async () => {
    await replied();
    h.stt.text = new SttError("noSpeech", "no speech captured");
    const spoken = h.speak.mock.calls.length;
    await stopListening();
    const s = useVoiceStore.getState();
    expect(s.phase).toBe("idle");
    expect(s.reply).toBeNull();
    expect(s.followUp).toBeNull();
    expect(h.playSound).toHaveBeenCalledWith("listenEnd");
    expect(h.playSound).not.toHaveBeenCalledWith("error");
    expect(h.speak.mock.calls.length).toBe(spoken);
  });

  it("«спасибо, всё» ends the conversation at once", async () => {
    await replied();
    h.stt.text = "Спасибо, всё.";
    await stopListening();
    expect(useVoiceStore.getState().phase).toBe("idle");
    expect(h.route.calls).toEqual(["привет"]);
    await tick();
    expect(h.startStt).toHaveBeenCalledTimes(1);
  });

  it("a stop phrase in the window and the assistant's own echo end it too", async () => {
    await replied();
    h.stt.text = "хватит";
    await stopListening();
    expect(useVoiceStore.getState().phase).toBe("idle");
    await replied();
    h.stt.text = "здравствуйте сэр";
    await stopListening();
    expect(useVoiceStore.getState().phase).toBe("idle");
    expect(h.route.calls).toEqual(["привет", "привет"]);
  });

  it("interrupt, then follow-up: the reply after the interruption re-opens listening", async () => {
    h.tts.hold = true;
    void simulateTranscript("расскажи про погоду");
    await vi.waitFor(() => expect(useVoiceStore.getState().speaking).toBe(true));
    interrupt("Hard");
    await vi.waitFor(() => expect(h.startStt).toHaveBeenCalled());
    h.tts.hold = false;
    h.route.reply = "Открываю YouTube, сэр.";
    h.stt.text = "открой ютуб";
    await stopListening();
    expect(h.route.calls).toEqual(["расскажи про погоду", "открой ютуб"]);
    await vi.waitFor(() => expect(useVoiceStore.getState().followUp?.ms).toBe(8000), { timeout: FOLLOW_UP_DELAY_MS + 1000 });
  });

  it("is off when disabled, and never after a typed command", async () => {
    useVoiceStore.setState({ config: { ...structuredClone(CONFIG), followUp: { ...CONFIG.followUp, enabled: false } } });
    await simulateTranscript("привет");
    await new Promise((r) => setTimeout(r, FOLLOW_UP_DELAY_MS + 50));
    expect(useVoiceStore.getState().phase).toBe("result");
    expect(h.startStt).not.toHaveBeenCalled();
    useVoiceStore.setState({ config: structuredClone(CONFIG) });
    await simulateTranscript("привет", "Typed");
    await new Promise((r) => setTimeout(r, FOLLOW_UP_DELAY_MS + 50));
    expect(h.startStt).not.toHaveBeenCalled();
  });
});
