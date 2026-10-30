/**
 * §63 G without microphone hardware: a recognised transcript runs through the
 * same path as speech-to-text output (listen → transcribing → thinking →
 * result), and the debug hook is installed only behind its local flag.
 */
import type { VoiceConfig } from "@orkestr/protocol";
import { afterEach, beforeAll, describe, expect, it } from "vitest";

import { i18n } from "../../i18n";
import { useAppStore } from "../../store/app";
import { installVoiceDebugHook, setVoiceDeps, simulateTranscript } from "./controller";
import type { VoiceDeps } from "./pipeline/executor";
import { useVoiceStore } from "./store";
import { VOICE_ENGINE_DEFAULTS } from "./testConfig";

const CONFIG = {
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
  wake: { enabled: false, phrase: "Jarvis", phrases: ["Jarvis"], sensitivity: 50, cpuBudgetPct: 2, pauseOnBatterySaver: true },
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

const deps: VoiceDeps = {
  activeProjectId: () => null,
  projectName: (id) => (id === "p1" ? "Schoolify" : null),
  listTasks: async () => [],
  listAgents: async () => [],
  pendingApprovals: () => [],
  resolveApproval: async () => undefined,
  stopProject: async () => 0,
  stopAll: async () => true,
  openPanel: () => undefined,
  switchProject: async () => undefined,
  setDnd: async () => true,
  askManager: async () => ({ replyMessageId: "m1" }),
  buildErrors: async () => [],
  quality: async () => ({ lastBuild: null, lastTests: null, openReviews: null }),
};

beforeAll(async () => {
  await i18n.changeLanguage("en");
});

afterEach(() => {
  setVoiceDeps(null);
  window.localStorage.removeItem("orkestr.debugHooks");
  delete (window as unknown as { __orkestrDebug?: unknown }).__orkestrDebug;
});

describe("simulated transcript (acceptance seam)", () => {
  it("runs the recognised-speech path to a project status reply", async () => {
    setVoiceDeps(deps);
    useAppStore.setState({ projects: [{ id: "p1", name: "Schoolify" }] as never, activeProjectId: null });
    useVoiceStore.setState({ config: CONFIG, phase: "idle", reply: null, transcript: "" });
    const phases: string[] = [];
    const off = useVoiceStore.subscribe((s, prev) => {
      if (s.phase !== prev.phase) phases.push(s.phase);
    });
    await simulateTranscript("Jarvis, что происходит со Schoolify?");
    off();
    expect(phases).toEqual(["listening", "transcribing", "thinking", "result"]);
    const s = useVoiceStore.getState();
    expect(s.source).toBe("PushToTalk");
    expect(s.transcript).toBe("Jarvis, что происходит со Schoolify?");
    expect(s.reply?.text).toMatch(/^In Schoolify, /);
  });

  it("exposes the hook only behind the local debug flag", () => {
    const w = window as unknown as { __orkestrDebug?: { voice?: unknown } };
    installVoiceDebugHook()();
    expect(w.__orkestrDebug).toBeUndefined();
    window.localStorage.setItem("orkestr.debugHooks", "1");
    const off = installVoiceDebugHook();
    expect(typeof (w.__orkestrDebug?.voice as { simulateTranscript?: unknown })?.simulateTranscript).toBe("function");
    off();
    expect(w.__orkestrDebug?.voice).toBeUndefined();
  });
});
