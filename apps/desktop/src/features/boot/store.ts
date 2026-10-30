/**
 * Startup animation state (ADR-0053): the real startup steps shown in the
 * boot log, whether the sequence finished (Jarvis waits with its greeting
 * of the day), a settings preview request, and the device preferences.
 */
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { useVoiceStore } from "../voice/store";
import { readBootPrefs, writeBootPrefs, type BootPrefs } from "./prefs";
import type { BootVariant } from "./sequence";

export const STEP_IDS = ["core", "providers", "voice", "nodes", "plugins"] as const;
export type StepId = (typeof STEP_IDS)[number];
export type StepState = "pending" | "ok" | "skipped" | "error";

export type BootSteps = Record<StepId, StepState>;

const PENDING: BootSteps = { core: "pending", providers: "pending", voice: "pending", nodes: "pending", plugins: "pending" };

export interface BootState {
  prefs: BootPrefs;
  steps: BootSteps;
  /** The start-up sequence is over (or never played). */
  finished: boolean;
  /** The boot sequence spoke the greeting itself (Jarvis skips its own). */
  spokeGreeting: boolean;
  /** Settings «preview»: replay a version over the running app. */
  preview: Exclude<BootVariant, "none"> | null;

  setPrefs: (patch: Partial<BootPrefs>) => void;
  setStep: (id: StepId, state: StepState) => void;
  finish: (spokeGreeting?: boolean) => void;
  setPreview: (v: Exclude<BootVariant, "none"> | null) => void;
}

export const useBootStore = create<BootState>()((set, get) => ({
  prefs: readBootPrefs(),
  steps: { ...PENDING },
  finished: false,
  spokeGreeting: false,
  preview: null,

  setPrefs: (patch) => {
    const prefs = { ...get().prefs, ...patch };
    writeBootPrefs(prefs);
    set({ prefs });
  },
  setStep: (id, state) => set((s) => (s.steps[id] === state ? s : { steps: { ...s.steps, [id]: state } })),
  finish: (spokeGreeting = false) => set((s) => (s.finished && !spokeGreeting ? s : { finished: true, spokeGreeting: s.spokeGreeting || spokeGreeting })),
  setPreview: (preview) => set({ preview }),
}));

/** All steps reported (any outcome). */
export function stepsSettled(steps: BootSteps): boolean {
  return STEP_IDS.every((id) => steps[id] !== "pending");
}

/** Share of finished steps 0..1. */
export function stepsProgress(steps: BootSteps): number {
  return STEP_IDS.filter((id) => steps[id] !== "pending").length / STEP_IDS.length;
}

const STEP_TIMEOUT_MS = 4000;

function withTimeout<T>(p: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error("timeout")), ms);
    p.then(
      (v) => (clearTimeout(timer), resolve(v)),
      (e: unknown) => (clearTimeout(timer), reject(e instanceof Error ? e : new Error(String(e)))),
    );
  });
}

/**
 * Probes the real startup once the core is connected: AI sources
 * (`providers.list`), voice settings, compute nodes and plugins. Read-only
 * calls; failures only mark the step (the app does not depend on this).
 */
export function runBootSteps(): () => void {
  const { setStep } = useBootStore.getState();
  let alive = true;
  const mark = (id: StepId, state: StepState) => alive && setStep(id, state);
  setStep("core", "ok");
  const c = maybeClient();
  if (!c) {
    for (const id of STEP_IDS) if (id !== "core") mark(id, "skipped");
    return () => undefined;
  }
  const probe = (id: StepId, run: () => Promise<unknown>) =>
    withTimeout(run(), STEP_TIMEOUT_MS).then(
      () => mark(id, "ok"),
      () => mark(id, "error"),
    );
  void probe("providers", () => c.call("providers.list", {}));
  void probe("nodes", () => c.call("nodes.list", {}));
  void probe("plugins", () => c.call("plugins.list", {}));
  // Voice: the voice bootstrap loads the config; wait for it.
  let offVoice: (() => void) | null = null;
  const voiceTimer = setTimeout(() => {
    offVoice?.();
    mark("voice", useVoiceStore.getState().config ? "ok" : "skipped");
  }, STEP_TIMEOUT_MS);
  const checkVoice = () => {
    if (!useVoiceStore.getState().config) return false;
    clearTimeout(voiceTimer);
    offVoice?.();
    mark("voice", "ok");
    return true;
  };
  if (!checkVoice()) offVoice = useVoiceStore.subscribe(() => void checkVoice());
  return () => {
    alive = false;
    clearTimeout(voiceTimer);
    offVoice?.();
  };
}

/** Resets the steps (a new sequence, e.g. a preview). */
export function resetBootSteps(): void {
  useBootStore.setState({ steps: { ...PENDING } });
}
