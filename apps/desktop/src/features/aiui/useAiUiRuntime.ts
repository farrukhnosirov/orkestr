/**
 * Wires the AI-editable UI for one window: loads the stored UI
 * configuration, listens to `UiPatchProposed` / `UiConfigPatched` on the
 * `global` topic and keeps `<html>` / `<body>` in sync with the effective
 * document (the live preview while a proposal is shown).
 *
 * `receiveProposals` is false in the HUD window: proposals are reviewed in
 * the main window only (one preview card, one decision).
 */
import type { Event } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { useEffect } from "react";

import { maybeClient } from "../../core/client";
import { effectiveConfig, useAiUiStore } from "./aiuiStore";
import { switchLayout } from "../layout/controller";
import { applyPrefsToRoot, applyThemeOverlay, prefsOf } from "./prefs";

/** `UiConfig.layout.preset` → built-in layout id of the layout area (`Custom` keeps the current one). */
export function layoutIdForPreset(preset: string): string | null {
  switch (preset) {
    case "Coding":
      return "coding";
    case "MissionControl":
      return "mission-control";
    case "Monitoring":
      return "monitoring";
    case "Focus":
      return "focus";
    default:
      return null;
  }
}

export function useAiUiRuntime(connected: boolean, receiveProposals = true): void {
  // Effective document → DOM.
  useEffect(() => {
    let last: ReturnType<typeof effectiveConfig> | undefined;
    const sync = () => {
      const cfg = effectiveConfig(useAiUiStore.getState());
      if (cfg === last) return;
      last = cfg;
      applyPrefsToRoot(prefsOf(cfg));
      // Stored theme changes reach the theme engine (UiConfigChanged); only a
      // proposal preview is layered here.
      const s = useAiUiStore.getState();
      applyThemeOverlay(cfg !== s.config ? cfg : null);
    };
    sync();
    return useAiUiStore.subscribe(sync);
  }, []);

  // Data: initial load + live events.
  useEffect(() => {
    if (!connected) return undefined;
    const c = maybeClient();
    if (!c) return undefined;
    void useAiUiStore.getState().load();
    const off = c.subscribe([TOPIC_GLOBAL], (ev: Event) => {
      const s = useAiUiStore.getState();
      if (ev.kind === "UiPatchProposed" && receiveProposals) void s.receiveProposal(ev.payload);
      else if (ev.kind === "UiConfigPatched") {
        const before = s.config?.layout.preset ?? null;
        s.onPatched(ev.payload);
        // A patched layout preset switches this device's layout (main window).
        const after = ev.payload.config.layout.preset;
        if (receiveProposals && after !== before) {
          const id = layoutIdForPreset(after);
          if (id) void switchLayout(id);
        }
      }
    });
    return off;
  }, [connected, receiveProposals]);
}
