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
import { applyPrefsToRoot, applyThemeOverlay, prefsOf } from "./prefs";

export function useAiUiRuntime(connected: boolean, receiveProposals = true): void {
  // Effective document → DOM.
  useEffect(() => {
    let last: ReturnType<typeof effectiveConfig> | undefined;
    const sync = () => {
      const cfg = effectiveConfig(useAiUiStore.getState());
      if (cfg === last) return;
      last = cfg;
      applyPrefsToRoot(prefsOf(cfg));
      applyThemeOverlay(cfg);
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
      else if (ev.kind === "UiConfigPatched") s.onPatched(ev.payload);
    });
    return off;
  }, [connected, receiveProposals]);
}
