/**
 * The wake listener must not leave the microphone open when it is paused or
 * disabled while the mic is still opening, and concurrent syncs for the same
 * target must share one start (one segment subscription).
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const h = vi.hoisted(() => {
  const state = { resolveOpen: [] as ((m: unknown) => void)[], closed: 0, subscribes: 0 };
  const makeMic = () => {
    const m = {
      isClosed: false,
      analyser: null,
      subscribe: () => {
        state.subscribes += 1;
        return () => undefined;
      },
      close: () => {
        m.isClosed = true;
        state.closed += 1;
      },
      setBudget: () => undefined,
      setSegmenting: () => undefined,
    };
    return m;
  };
  return { state, makeMic };
});

vi.mock("./audio/mic", () => ({
  MicCapture: {
    open: () =>
      new Promise((resolve) => {
        h.state.resolveOpen.push(resolve);
      }),
  },
}));

import { shutdownVoice, syncWakeListener } from "./controller";
import { useVoiceStore } from "./store";
import { TEMPLATES_KEY } from "./wake/spotter";
import { VOICE_ENGINE_DEFAULTS } from "./testConfig";

const BASE = {
  enabled: true,
  sttEngine: "Disabled",
  sttConsent: null,
  ttsEngine: "System",
  ttsConsentAt: null,
  cloudProviderId: null,
  language: "auto",
  inputDeviceId: null,
  pushToTalkHotkey: "Ctrl+Space",
  spokenReplies: false,
  naturalReplies: false,
  systemVoice: null,
  cloudVoice: "alloy",
  ratePct: 100,
  pitchPct: 100,
  wake: { enabled: true, phrase: "Jarvis", sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: false },
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
  ...VOICE_ENGINE_DEFAULTS,
} satisfies VoiceConfig;

const OFF: VoiceConfig = { ...BASE, wake: { ...BASE.wake, enabled: false } };

async function micRequested(n = 1): Promise<void> {
  await vi.waitFor(() => expect(h.state.resolveOpen.length).toBeGreaterThanOrEqual(n));
}

describe("wake listener vs. a mic that is still opening", () => {
  beforeEach(() => {
    window.localStorage.setItem(TEMPLATES_KEY, JSON.stringify({ phrase: "Jarvis", samples: [[[1, 2]], [[1, 2]], [[1, 2]]] }));
    h.state.resolveOpen = [];
    h.state.closed = 0;
    h.state.subscribes = 0;
    useVoiceStore.setState({ config: BASE, phase: "idle", mic: [], wakeStatus: "off" });
  });
  afterEach(() => {
    shutdownVoice();
    window.localStorage.removeItem(TEMPLATES_KEY);
  });

  it("disabling the wake word during getUserMedia closes the mic once it opens", async () => {
    const a = syncWakeListener();
    await micRequested();
    expect(useVoiceStore.getState().mic).toContain("wake");
    useVoiceStore.setState({ config: OFF });
    await syncWakeListener(); // superseding sync: nothing to stop yet
    h.state.resolveOpen[0]?.(h.makeMic());
    await a;
    expect(useVoiceStore.getState().mic).not.toContain("wake");
    expect(h.state.closed).toBe(1);
    expect(useVoiceStore.getState().wakeStatus).toBe("off");
  });

  it("concurrent syncs for the same target share one start and one segment subscription", async () => {
    const a = syncWakeListener();
    const b = syncWakeListener();
    await micRequested();
    h.state.resolveOpen[0]?.(h.makeMic());
    await Promise.all([a, b]);
    // One level subscription (openMic) + one segment subscription (wake).
    expect(h.state.subscribes).toBe(2);
    expect(useVoiceStore.getState().wakeStatus).toBe("listening");
    expect(useVoiceStore.getState().mic).toEqual(["wake"]);
  });

  it("a restart superseding an in-flight start keeps the mic for the newer listener", async () => {
    const a = syncWakeListener();
    await micRequested();
    useVoiceStore.setState({ config: { ...BASE, wake: { ...BASE.wake, sensitivity: 70 } } });
    const b = syncWakeListener();
    h.state.resolveOpen[0]?.(h.makeMic());
    await Promise.all([a, b]);
    expect(useVoiceStore.getState().mic).toEqual(["wake"]);
    expect(h.state.closed).toBe(0);
    expect(useVoiceStore.getState().wakeStatus).toBe("listening");
    expect(h.state.subscribes).toBe(2);
  });

  it("shutdown while the mic opens does not leave it open", async () => {
    const a = syncWakeListener();
    await micRequested();
    shutdownVoice();
    h.state.resolveOpen[0]?.(h.makeMic());
    await a;
    expect(useVoiceStore.getState().mic).not.toContain("wake");
    expect(h.state.closed).toBe(1);
  });
});
