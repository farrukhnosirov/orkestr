/**
 * Environment of a robot window: connects to the core (same `core_connection`
 * as the main window), mirrors the settings it needs — Performance Mode and
 * language — and keeps them live via `SettingsChanged`. The theme (tokens the
 * robots are drawn with) comes from the shared theme engine (`RobotApp`).
 */
import type { AppSettings, Event, PerformanceMode } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { bootstrapCore, maybeClient } from "../../../core/client";
import { applyLanguagePreference, languagePreference } from "../../../i18n";

export interface RobotEnv {
  ready: boolean;
  error: string | null;
  mode: PerformanceMode;
}

function applySettings(s: AppSettings): void {
  if (s.language !== languagePreference()) void applyLanguagePreference(s.language);
}

export function useRobotEnv(): RobotEnv {
  const [env, setEnv] = useState<RobotEnv>({ ready: false, error: null, mode: "Balanced" });
  useEffect(() => {
    let off: (() => void) | null = null;
    let cancelled = false;
    bootstrapCore()
      .then(async (c) => {
        const { settings } = await c.call("settings.get", {});
        if (cancelled) return;
        applySettings(settings);
        setEnv({ ready: true, error: null, mode: settings.performanceMode });
        off = c.subscribe([TOPIC_GLOBAL], (ev: Event) => {
          if (ev.kind !== "SettingsChanged") return;
          applySettings(ev.payload.settings);
          setEnv((prev) => ({ ...prev, mode: ev.payload.settings.performanceMode }));
        });
      })
      .catch((e: unknown) => {
        if (!cancelled) setEnv((prev) => ({ ...prev, error: e instanceof Error ? e.message : String(e) }));
      });
    return () => {
      cancelled = true;
      off?.();
    };
  }, []);
  return env;
}

/** The connected client of this window (after `useRobotEnv` is ready). */
export function robotClient() {
  return maybeClient();
}
