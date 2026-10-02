/**
 * Wires the platform-shell features once the core connection is ready:
 * settings load + live `SettingsChanged`, theme/DND sync, first-run
 * onboarding, notification events, the tray "Stop All AI" item, Session
 * Resume and global keyboard shortcuts.
 */
import type { Event } from "@orkestr/protocol";
import { TOPIC_GLOBAL, topic } from "@orkestr/protocol";
import { useEffect } from "react";

import { getClient } from "../../core/client";
import { shellControls } from "../../lib/tauri";
import { useAppStore } from "../../store/app";
import { stopAllAi } from "../commands/coreCommands";
import { notificationFromEvent } from "../notifications/fromEvent";
import { useNotificationsStore } from "../notifications/notificationsStore";
import { useOnboardingStore } from "../onboarding/onboardingStore";
import { startResumeWatcher } from "../resume/resumeStore";
import { SEARCH_PREFIX, usePaletteStore } from "../search/paletteStore";
import { useSettingsStore } from "./settingsStore";

function onEvent(ev: Event): void {
  if (ev.kind === "SettingsChanged") {
    useSettingsStore.getState().applyRemote(ev.payload.settings);
    return;
  }
  const n = notificationFromEvent(ev);
  if (n) useNotificationsStore.getState().push(n);
}

/** Topics for notifications: global + every known project. */
export function notificationTopics(projectIds: readonly string[]): string[] {
  return [TOPIC_GLOBAL, ...projectIds.map((id) => topic.project(id))];
}

/** Keyboard shortcuts owned by the shell (palette Ctrl+K lives in App). */
export function shellShortcut(e: Pick<KeyboardEvent, "key" | "ctrlKey" | "metaKey" | "shiftKey" | "altKey">): string | null {
  const mod = e.ctrlKey || e.metaKey;
  if (mod && !e.shiftKey && !e.altKey && e.key === ",") return "settings.open";
  if (mod && !e.shiftKey && !e.altKey && e.key.toLowerCase() === "p") return "project.switch";
  if (mod && e.shiftKey && !e.altKey && e.key === "Backspace") return "ai.stopAll";
  if (!mod && !e.shiftKey && !e.altKey && e.key === "F11") return "window.fullscreen";
  return null;
}

export function useShellBootstrap(ready: boolean): void {
  // Settings, theme, DND, onboarding.
  useEffect(() => {
    if (!ready) return;
    let onboardingChecked = false;
    const applySettings = () => {
      const s = useSettingsStore.getState().settings;
      if (!s) return;
      useNotificationsStore.getState().setDoNotDisturb(s.doNotDisturb);
      const app = useAppStore.getState();
      if (s.themeId && s.themeId !== app.themeId) app.setThemeId(s.themeId);
      if (!onboardingChecked) {
        onboardingChecked = true;
        if (!s.onboardingCompleted) useOnboardingStore.getState().openWizard(0);
      }
    };
    const offSettings = useSettingsStore.subscribe((s, prev) => {
      if (s.settings !== prev.settings) applySettings();
    });
    // Theme picked locally (palette) → persist to settings.
    const offTheme = useAppStore.subscribe((s, prev) => {
      const settings = useSettingsStore.getState().settings;
      if (s.themeId !== prev.themeId && settings && settings.themeId !== s.themeId) {
        void useSettingsStore.getState().update({ themeId: s.themeId });
      }
    });
    void useSettingsStore.getState().load();
    return () => {
      offSettings();
      offTheme();
    };
  }, [ready]);

  // Notification + settings events (global and every project).
  useEffect(() => {
    if (!ready) return;
    let key = "";
    let unsubscribe: (() => void) | null = null;
    const resubscribe = () => {
      const ids = useAppStore.getState().projects.map((p) => p.id);
      const next = ids.join(",");
      if (unsubscribe && next === key) return;
      key = next;
      unsubscribe?.();
      unsubscribe = getClient().subscribe(notificationTopics(ids), onEvent);
    };
    resubscribe();
    const offProjects = useAppStore.subscribe((s, prev) => {
      if (s.projects !== prev.projects) resubscribe();
    });
    return () => {
      offProjects();
      unsubscribe?.();
    };
  }, [ready]);

  // Tray "Stop All AI": stop agents, or hide the window when nothing can be stopped.
  useEffect(() => {
    let off: (() => void) | undefined;
    let disposed = false;
    shellControls
      .onTrayStopAll(() => {
        void stopAllAi().then((stopped) => {
          if (!stopped) void shellControls.hideWindow();
        });
      })
      .then(
        (u) => (disposed ? u() : (off = u)),
        (e: unknown) => console.warn("tray listener", e),
      );
    return () => {
      disposed = true;
      off?.();
    };
  }, []);

  // Session Resume.
  useEffect(() => (ready ? startResumeWatcher() : undefined), [ready]);

  // Shortcuts.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const id = shellShortcut(e);
      if (!id) return;
      e.preventDefault();
      if (id === "settings.open") useSettingsStore.getState().openSettings();
      else if (id === "project.switch") usePaletteStore.getState().openPalette(SEARCH_PREFIX);
      else if (id === "window.fullscreen") void shellControls.toggleFullscreen();
      else if (id === "ai.stopAll") void stopAllAi();
    };
    window.addEventListener("keydown", onKey, { capture: true });
    return () => window.removeEventListener("keydown", onKey, { capture: true });
  }, []);
}
