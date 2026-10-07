/**
 * Central Performance Mode store (§31). Every visual feature reads the
 * effective policy from here — React through `usePerfPolicy()`, plain code
 * through `getPerfPolicy()` / `subscribePerf()` — instead of deciding on its
 * own. The user's choice lives in the core (`AppSettings.performanceMode`);
 * the auto-switch preferences are per device (local storage).
 *
 * Root contract (CSS): `data-ok-performance`, `data-ok-motion`
 * (`full|reduced|off`), `data-ok-blur`, `data-ok-glow`, `data-ok-bg-anim`,
 * `data-ok-gpu`, `data-ok-paused` (`on|off`) and `--ok-motion-scale`,
 * `--ok-blur-scale`, `--ok-glow-scale` on `<html>`.
 */
import type { PerformanceMode, PowerStatusView } from "@orkestr/protocol";
import { create } from "zustand";

import {
  canDegrade,
  effectiveMode,
  resolvePolicy,
  type ModeReason,
  type PerfPolicy,
} from "./policy";

const AUTO_BATTERY_KEY = "orkestr.perf.autoBattery";
const AUTO_DEGRADE_KEY = "orkestr.perf.autoDegrade";

function readFlag(key: string, fallback: boolean): boolean {
  try {
    const v = window.localStorage.getItem(key);
    return v === null ? fallback : v === "1";
  } catch {
    return fallback;
  }
}

function writeFlag(key: string, value: boolean): void {
  try {
    window.localStorage.setItem(key, value ? "1" : "0");
  } catch {
    /* storage unavailable: the preference lasts for this session */
  }
}

function prefersReducedMotion(): boolean {
  try {
    return typeof window.matchMedia === "function" && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
  } catch {
    return false;
  }
}

export interface PerfState {
  chosen: PerformanceMode;
  autoBattery: boolean;
  autoDegrade: boolean;
  power: PowerStatusView | null;
  degradeSteps: number;
  hidden: boolean;
  /** Visible but not focused (another app is in front). */
  inactive: boolean;
  reducedMotion: boolean;
  mode: PerformanceMode;
  reason: ModeReason;
  policy: PerfPolicy;

  setChosen: (mode: PerformanceMode) => void;
  setAutoBattery: (on: boolean) => void;
  setAutoDegrade: (on: boolean) => void;
  setPower: (power: PowerStatusView | null) => void;
  /** One step down because frames missed their budget; false when already at the floor. */
  degrade: () => boolean;
  setHidden: (hidden: boolean) => void;
  setInactive: (inactive: boolean) => void;
  setReducedMotion: (reduced: boolean) => void;
}

type Inputs = Pick<
  PerfState,
  "chosen" | "autoBattery" | "autoDegrade" | "power" | "degradeSteps" | "hidden" | "inactive" | "reducedMotion"
>;

function derive(s: Inputs): Pick<PerfState, "mode" | "reason" | "policy"> {
  const onBattery = s.power?.supported && s.power.hasBattery ? s.power.onAcPower === false : null;
  const { mode, reason } = effectiveMode({
    chosen: s.chosen,
    autoBattery: s.autoBattery,
    onBattery,
    autoDegrade: s.autoDegrade,
    degradeSteps: s.degradeSteps,
  });
  return { mode, reason, policy: resolvePolicy(mode, { reducedMotion: s.reducedMotion, hidden: s.hidden, inactive: s.inactive }) };
}

const initial: Inputs = {
  chosen: "Balanced",
  autoBattery: readFlag(AUTO_BATTERY_KEY, true),
  autoDegrade: readFlag(AUTO_DEGRADE_KEY, true),
  power: null,
  degradeSteps: 0,
  hidden: typeof document !== "undefined" ? document.hidden : false,
  // Only the windows that opt in (see `usePerformanceRuntime`) ever set this.
  inactive: false,
  reducedMotion: prefersReducedMotion(),
};

function samePower(a: PowerStatusView | null, b: PowerStatusView | null): boolean {
  if (a === b) return true;
  if (!a || !b) return false;
  return JSON.stringify(a) === JSON.stringify(b);
}

export const usePerfStore = create<PerfState>()((set, get) => {
  const update = (patch: Partial<Inputs>) => {
    const next = { ...get(), ...patch };
    set({ ...patch, ...derive(next) });
  };
  return {
    ...initial,
    ...derive(initial),
    // A new explicit choice forgets earlier automatic degrade steps.
    setChosen: (chosen) => {
      if (chosen !== get().chosen) update({ chosen, degradeSteps: 0 });
    },
    setAutoBattery: (autoBattery) => {
      writeFlag(AUTO_BATTERY_KEY, autoBattery);
      update({ autoBattery });
    },
    setAutoDegrade: (autoDegrade) => {
      writeFlag(AUTO_DEGRADE_KEY, autoDegrade);
      update({ autoDegrade, degradeSteps: autoDegrade ? get().degradeSteps : 0 });
    },
    // Polled every minute and on every focus: an unchanged reading must not
    // produce a new policy object (every visual subscriber would re-render).
    setPower: (power) => {
      if (!samePower(power, get().power)) update({ power });
    },
    degrade: () => {
      const s = get();
      const inputs = {
        chosen: s.chosen,
        autoBattery: s.autoBattery,
        onBattery: null,
        autoDegrade: s.autoDegrade,
        degradeSteps: s.degradeSteps,
      };
      if (!s.autoDegrade || !canDegrade(inputs)) return false;
      update({ degradeSteps: s.degradeSteps + 1 });
      return true;
    },
    setHidden: (hidden) => {
      if (hidden !== get().hidden) update({ hidden });
    },
    setInactive: (inactive) => {
      if (inactive !== get().inactive) update({ inactive });
    },
    setReducedMotion: (reducedMotion) => update({ reducedMotion }),
  };
});

/** The effective policy (React). */
export function usePerfPolicy(): PerfPolicy {
  return usePerfStore((s) => s.policy);
}

/** The effective policy (non-React code: canvas loops, chart timers, robots). */
export function getPerfPolicy(): PerfPolicy {
  return usePerfStore.getState().policy;
}

/** Subscribes to policy changes; returns the unsubscribe function. */
export function subscribePerf(listener: (policy: PerfPolicy) => void): () => void {
  return usePerfStore.subscribe((s, prev) => {
    if (s.policy !== prev.policy) listener(s.policy);
  });
}
