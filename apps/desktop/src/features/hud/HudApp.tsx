/**
 * Root of the HUD window (§34). It is its own small app on the same bundle:
 * connects to the core over RPC (like every window), follows settings
 * (language, performance mode) and the UI configuration, keeps approvals
 * live, and remembers where the user put it. Proposals of UI changes are
 * reviewed in the main window only.
 */
import { BUILTIN_THEMES, applyTheme } from "@orkestr/ui";
import type { Event } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { useEffect, useState } from "react";

import { bootstrapCore, maybeClient } from "../../core/client";
import { applyLanguagePreference, languagePreference } from "../../i18n";
import { useAppStore } from "../../store/app";
import { useAiUiRuntime } from "../aiui/useAiUiRuntime";
import { usePerformanceRuntime } from "../performance/usePerformanceRuntime";
import { initSecurityLive } from "../security/store";
import { useSettingsStore } from "../settings/settingsStore";
import { Hud } from "./Hud";
import { hudControls } from "./hudWindow";
import { useHudShortcut } from "./commands";

/** Key under which the main window stores the active project (`store/app.ts`). */
const ACTIVE_PROJECT_STORAGE_KEY = "orkestr.activeProject";

function useHudTheme() {
  const themeId = useAppStore((s) => s.themeId);
  useEffect(() => {
    const theme = BUILTIN_THEMES[themeId] ?? BUILTIN_THEMES["nebula"];
    if (!theme) return;
    try {
      applyTheme(theme);
    } catch (e) {
      console.error("failed to apply theme", e);
    }
  }, [themeId]);
}

/** Settings (language, theme, performance mode) for the HUD window. */
function useHudSettings(ready: boolean) {
  useEffect(() => {
    if (!ready) return undefined;
    const apply = () => {
      const s = useSettingsStore.getState().settings;
      if (!s) return;
      if (s.language !== languagePreference()) void applyLanguagePreference(s.language);
      if (s.themeId && s.themeId !== useAppStore.getState().themeId) useAppStore.getState().setThemeId(s.themeId);
    };
    const offStore = useSettingsStore.subscribe((s, prev) => {
      if (s.settings !== prev.settings) apply();
    });
    void useSettingsStore.getState().load();
    const c = maybeClient();
    const offEvents = c?.subscribe([TOPIC_GLOBAL], (ev: Event) => {
      if (ev.kind === "SettingsChanged") useSettingsStore.getState().applyRemote(ev.payload.settings);
    });
    return () => {
      offStore();
      offEvents?.();
    };
  }, [ready]);
}

export function HudApp() {
  const [ready, setReady] = useState(false);
  const connected = useAppStore((s) => s.connection.state === "open");
  useHudTheme();
  useHudSettings(ready);
  usePerformanceRuntime(connected);
  useAiUiRuntime(connected, false);
  useHudShortcut();

  useEffect(() => {
    document.documentElement.dataset["okWindow"] = "hud";
    let off: (() => void) | undefined;
    let disposed = false;
    hudControls.trackPosition().then(
      (u) => (disposed ? u() : (off = u)),
      () => {},
    );
    bootstrapCore().then(
      () => setReady(true),
      (e: unknown) => console.error("HUD: core bootstrap failed", e),
    );
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  useEffect(() => (connected ? initSecurityLive() : undefined), [connected]);

  // Follow the project picked in the main window (same-origin local storage).
  useEffect(() => {
    const onStorage = (e: StorageEvent) => {
      if (e.key === ACTIVE_PROJECT_STORAGE_KEY && e.newValue !== useAppStore.getState().activeProjectId) {
        useAppStore.getState().setActiveProject(e.newValue);
      }
    };
    window.addEventListener("storage", onStorage);
    return () => window.removeEventListener("storage", onStorage);
  }, []);

  return (
    <div className="hud-root">
      <Hud />
    </div>
  );
}
