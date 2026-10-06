/**
 * Runs the Performance Mode machinery for one window (main or HUD):
 *
 * - follows the user's choice from the settings store (`SettingsChanged`);
 * - `prefers-reduced-motion` and window visibility (hidden / minimized →
 *   everything pauses, so visuals never compete with the AI workload);
 * - power: polls `nodes.power` (RPC) every minute and on focus — Battery
 *   Saver kicks in on battery when the user allows it;
 * - frame budget: short rAF sampling bursts (4 s every 30 s, only while the
 *   window is visible and not already at the floor) feed
 *   `FrameBudgetMonitor`; sustained jank steps the mode down once;
 * - writes the effective policy to `<html>` (see `policy.ts`).
 */
import { useEffect } from "react";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { useSettingsStore } from "../settings/settingsStore";
import { usePerfStore } from "./perfStore";
import { FrameBudgetMonitor, applyPolicyToRoot } from "./policy";

const POWER_POLL_MS = 60_000;
const SAMPLE_EVERY_MS = 30_000;
const SAMPLE_FOR_MS = 4_000;

async function pollPower(): Promise<void> {
  const c = maybeClient();
  if (!c || c.state !== "open") return;
  try {
    usePerfStore.getState().setPower(await c.call("nodes.power", {}));
  } catch (e) {
    console.debug("nodes.power unavailable", e);
  }
}

function notifyDegraded(): void {
  try {
    const mode = usePerfStore.getState().mode;
    useNotificationsStore
      .getState()
      .notify("Info", t("performance:degraded.title"), t("performance:degraded.body", { mode: t(`performance:modes.${mode}.name`) }));
  } catch (e) {
    console.debug("notification failed", e);
  }
}

export function usePerformanceRuntime(connected: boolean): void {
  // Policy → <html>, and the user's choice from settings.
  useEffect(() => {
    applyPolicyToRoot(usePerfStore.getState().policy);
    const offPolicy = usePerfStore.subscribe((s, prev) => {
      if (s.policy !== prev.policy) applyPolicyToRoot(s.policy);
    });
    const sync = () => {
      const mode = useSettingsStore.getState().settings?.performanceMode;
      if (mode) usePerfStore.getState().setChosen(mode);
    };
    sync();
    const offSettings = useSettingsStore.subscribe((s, prev) => {
      if (s.settings !== prev.settings) sync();
    });
    return () => {
      offPolicy();
      offSettings();
    };
  }, []);

  // Visibility + reduced motion.
  useEffect(() => {
    const onVisibility = () => usePerfStore.getState().setHidden(document.hidden);
    document.addEventListener("visibilitychange", onVisibility);
    let mql: MediaQueryList | null = null;
    const onMotion = (e: MediaQueryListEvent) => usePerfStore.getState().setReducedMotion(e.matches);
    try {
      if (typeof window.matchMedia === "function") {
        mql = window.matchMedia("(prefers-reduced-motion: reduce)");
        mql.addEventListener("change", onMotion);
      }
    } catch {
      mql = null;
    }
    return () => {
      document.removeEventListener("visibilitychange", onVisibility);
      mql?.removeEventListener("change", onMotion);
    };
  }, []);

  // Power (RPC).
  useEffect(() => {
    if (!connected) return undefined;
    void pollPower();
    const timer = window.setInterval(() => void pollPower(), POWER_POLL_MS);
    const onFocus = () => void pollPower();
    window.addEventListener("focus", onFocus);
    return () => {
      window.clearInterval(timer);
      window.removeEventListener("focus", onFocus);
    };
  }, [connected]);

  // Frame budget sampling.
  useEffect(() => {
    if (typeof window.requestAnimationFrame !== "function") return undefined;
    // Each burst is one time-boxed window; two bad bursts in a row step the mode
    // down. (Frame-count windows never filled up within a burst on 60 Hz panels.)
    const monitor = new FrameBudgetMonitor();
    let raf = 0;
    let last = 0;
    let until = 0;
    let mode = usePerfStore.getState().mode;
    const frame = (now: number) => {
      if (last > 0) monitor.sample(now - last);
      last = now;
      if (now < until) {
        raf = window.requestAnimationFrame(frame);
        return;
      }
      raf = 0;
      if (monitor.endWindow() === "degrade" && usePerfStore.getState().degrade()) notifyDegraded();
    };
    const burst = () => {
      const s = usePerfStore.getState();
      if (raf || s.hidden || !s.autoDegrade || s.mode === "Performance" || s.mode === "BatterySaver") return;
      // A different mode renders differently: its bad-burst streak starts over.
      if (s.mode !== mode) {
        mode = s.mode;
        monitor.reset();
      }
      monitor.beginWindow();
      last = 0;
      until = performance.now() + SAMPLE_FOR_MS;
      raf = window.requestAnimationFrame(frame);
    };
    const first = window.setTimeout(burst, 5_000);
    const timer = window.setInterval(burst, SAMPLE_EVERY_MS);
    return () => {
      window.clearTimeout(first);
      window.clearInterval(timer);
      if (raf) window.cancelAnimationFrame(raf);
    };
  }, []);
}
