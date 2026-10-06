/**
 * Environment of a robot window: connects to the core (same `core_connection`
 * as the main window), mirrors the settings it needs — Performance Mode and
 * language — and keeps them live via `SettingsChanged`. The theme (tokens the
 * robots are drawn with) comes from the shared theme engine (`RobotApp`).
 *
 * Performance: the robots follow the main window's *effective* mode
 * (auto Battery Saver on battery, frame-budget steps — `perfBroadcast`), and
 * the user's choice only until the main window has answered.
 */
import type { AppSettings, Event, PerformanceMode } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { bootstrapCore, maybeClient } from "../../../core/client";
import { applyLanguagePreference, languagePreference } from "../../../i18n";
import { subscribeEffectiveMode } from "../../performance/perfBroadcast";

export interface RobotEnv {
  ready: boolean;
  error: string | null;
  mode: PerformanceMode;
}

function applySettings(s: AppSettings): void {
  if (s.language !== languagePreference()) void applyLanguagePreference(s.language);
}

/** The mode robots render with: the main window's effective mode once known, else the user's choice. */
export function robotMode(chosen: PerformanceMode, effective: PerformanceMode | null): PerformanceMode {
  return effective ?? chosen;
}

export function useRobotEnv(): RobotEnv {
  const [base, setBase] = useState<{ ready: boolean; error: string | null; chosen: PerformanceMode }>({
    ready: false,
    error: null,
    chosen: "Balanced",
  });
  const [effective, setEffective] = useState<PerformanceMode | null>(null);
  useEffect(() => subscribeEffectiveMode(setEffective), []);
  useEffect(() => {
    let off: (() => void) | null = null;
    let cancelled = false;
    bootstrapCore()
      .then(async (c) => {
        const { settings } = await c.call("settings.get", {});
        if (cancelled) return;
        applySettings(settings);
        setBase({ ready: true, error: null, chosen: settings.performanceMode });
        off = c.subscribe([TOPIC_GLOBAL], (ev: Event) => {
          if (ev.kind !== "SettingsChanged") return;
          applySettings(ev.payload.settings);
          setBase((prev) => ({ ...prev, chosen: ev.payload.settings.performanceMode }));
        });
      })
      .catch((e: unknown) => {
        if (!cancelled) setBase((prev) => ({ ...prev, error: e instanceof Error ? e.message : String(e) }));
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);
  return { ready: base.ready, error: base.error, mode: robotMode(base.chosen, effective) };
}

/** The connected client of this window (after `useRobotEnv` is ready). */
export function robotClient() {
  return maybeClient();
}
