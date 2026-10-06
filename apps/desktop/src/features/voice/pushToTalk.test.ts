/**
 * Push-to-talk released (or cancelled) while the microphone is still opening
 * must not start a recording: nothing may be captured after the key is up.
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { afterEach, beforeAll, describe, expect, it, vi } from "vitest";

import { i18n } from "../../i18n";

const h = vi.hoisted(() => {
  const state = { resolveOpen: null as null | ((m: unknown) => void), closed: 0 };
  const fakeMic = {
    isClosed: false,
    analyser: null,
    subscribe: () => () => undefined,
    close: () => {
      state.closed += 1;
    },
    setBudget: () => undefined,
    setSegmenting: () => undefined,
  };
  const startStt = vi.fn(() => ({
    onInterim: () => undefined,
    onAutoEnd: () => undefined,
    finish: async () => "",
    cancel: () => undefined,
  }));
  return { state, fakeMic, startStt };
});

vi.mock("./audio/mic", () => ({
  MicCapture: {
    open: () =>
      new Promise((resolve) => {
        h.state.resolveOpen = resolve;
      }),
  },
}));

vi.mock("./stt/engines", async (orig) => ({
  ...(await orig<typeof import("./stt/engines")>()),
  startStt: h.startStt,
}));

import { usePerfStore } from "../performance/perfStore";
import { useSettingsStore } from "../settings/settingsStore";
import { cancelVoice, startListening, stopListening, wakeWanted } from "./controller";
import { useVoiceStore } from "./store";
import { TEMPLATES_KEY } from "./wake/spotter";

const CONFIG = {
  enabled: true,
  sttEngine: "OpenAiWhisper",
  sttConsent: null,
  ttsEngine: "System",
  ttsConsentAt: null,
  cloudProviderId: null,
  language: "auto",
  inputDeviceId: null,
  pushToTalkHotkey: "Ctrl+Space",
  spokenReplies: false,
  systemVoice: null,
  cloudVoice: "alloy",
  ratePct: 100,
  pitchPct: 100,
  wake: { enabled: false, phrase: "Jarvis", sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: true },
  notifications: {
    enabled: true,
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
} satisfies VoiceConfig;

/** Waits until the controller is inside `getUserMedia` (mic open pending). */
async function micOpening(): Promise<void> {
  await vi.waitFor(() => expect(h.state.resolveOpen).not.toBeNull());
}

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  cancelVoice();
  h.startStt.mockClear();
  h.state.resolveOpen = null;
});

describe("push-to-talk while the microphone opens", () => {
  it("a release before the mic is ready records nothing and closes the mic", async () => {
    useVoiceStore.setState({ config: CONFIG, phase: "idle", mic: [], reply: null, transcript: "" });
    const started = startListening("PushToTalk");
    await micOpening();
    expect(useVoiceStore.getState().phase).toBe("listening");
    await stopListening(); // key released during getUserMedia
    expect(h.state.resolveOpen).not.toBeNull();
    const closedBefore = h.state.closed;
    h.state.resolveOpen?.(h.fakeMic);
    await started;
    expect(h.startStt).not.toHaveBeenCalled();
    expect(useVoiceStore.getState().phase).toBe("idle");
    expect(useVoiceStore.getState().mic).not.toContain("command");
    expect(h.state.closed).toBeGreaterThan(closedBefore);
  });

  it("a held key starts recognition once the mic is ready", async () => {
    useVoiceStore.setState({ config: CONFIG, phase: "idle", mic: [], reply: null, transcript: "" });
    const started = startListening("PushToTalk");
    await micOpening();
    h.state.resolveOpen?.(h.fakeMic);
    await started;
    expect(h.startStt).toHaveBeenCalledTimes(1);
    expect(useVoiceStore.getState().mic).toContain("command");
  });

  it("Esc while the mic opens also abandons the capture", async () => {
    useVoiceStore.setState({ config: CONFIG, phase: "idle", mic: [], reply: null, transcript: "" });
    const started = startListening("PushToTalk");
    await micOpening();
    cancelVoice();
    h.state.resolveOpen?.(h.fakeMic);
    await started;
    expect(h.startStt).not.toHaveBeenCalled();
    expect(useVoiceStore.getState().phase).toBe("idle");
  });
});

describe("wake word and Battery Saver", () => {
  it("pauses when the effective mode is Battery Saver (auto switch on battery)", () => {
    window.localStorage.setItem(TEMPLATES_KEY, JSON.stringify({ phrase: "Jarvis", samples: [[[1]], [[1]], [[1]]] }));
    useVoiceStore.setState({ config: { ...CONFIG, wake: { ...CONFIG.wake, enabled: true } } });
    useSettingsStore.setState({ settings: { performanceMode: "Balanced" } as never });
    usePerfStore.getState().setChosen("Balanced");
    usePerfStore.getState().setAutoBattery(true);
    usePerfStore.getState().setPower({ supported: true, hasBattery: true, onAcPower: true } as never);
    expect(wakeWanted().status).toBe("ok");
    usePerfStore.getState().setPower({ supported: true, hasBattery: true, onAcPower: false } as never);
    expect(usePerfStore.getState().mode).toBe("BatterySaver");
    expect(wakeWanted()).toEqual({ run: false, status: "pausedBattery" });
    window.localStorage.removeItem(TEMPLATES_KEY);
  });
});
