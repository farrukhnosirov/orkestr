/**
 * HUD data (RPC + events only): Manager runs of the active project, plus
 * typed slots other areas plug into.
 *
 * - Voice: the voice area registers its indicator (`registerHudVoiceSlot`,
 *   `HudApp`); it mirrors the main window's microphone over a channel.
 * - Quality: `useHudQuality` — latest build / test result of the active
 *   project from the Phase 2 quality runner (`quality.runs` + live events).
 */
import type { Event, ManagerRun } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useMemo, useState, useSyncExternalStore, type ComponentType } from "react";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { hudStateOf, qualityFactsOf } from "../quality/signals";
import { useQualityLive } from "../quality/store";
import { useAppStore } from "../../store/app";

/** Manager runs of a project, kept live by `ManagerRunUpdated`. */
export function useHudRuns(projectId: string | null): ManagerRun[] {
  const connected = useAppStore((s) => s.connection.state === "open");
  const [runs, setRuns] = useState<ManagerRun[]>([]);
  useEffect(() => {
    setRuns([]);
    if (!projectId || !connected) return undefined;
    const c = maybeClient();
    if (!c) return undefined;
    let cancelled = false;
    c.call("manager.status", { projectId }).then(
      (r) => {
        if (!cancelled) setRuns(r.runs);
      },
      () => {
        /* the HUD simply shows "idle" */
      },
    );
    const off = c.subscribe([topic.project(projectId)], (ev: Event) => {
      if (ev.kind !== "ManagerRunUpdated") return;
      const run = ev.payload.run;
      setRuns((prev) => [run, ...prev.filter((r) => r.id !== run.id)]);
    });
    return () => {
      cancelled = true;
      off();
    };
  }, [projectId, connected]);
  return runs;
}

// ── Voice slot ────────────────────────────────────────────

export interface HudVoiceSlotProps {
  /** Compact size in px of the indicator. */
  size: number;
}

let voiceSlot: ComponentType<HudVoiceSlotProps> | null = null;
const voiceListeners = new Set<() => void>();

/** Lets the voice feature render its live indicator in the HUD. */
export function registerHudVoiceSlot(component: ComponentType<HudVoiceSlotProps> | null): void {
  voiceSlot = component;
  voiceListeners.forEach((l) => l());
}

export function useHudVoiceSlot(): ComponentType<HudVoiceSlotProps> | null {
  return useSyncExternalStore(
    (l) => {
      voiceListeners.add(l);
      return () => voiceListeners.delete(l);
    },
    () => voiceSlot,
  );
}

// ── Quality signal (Phase 2) ──────────────────────────────

export interface HudQuality {
  state: "passing" | "failing" | "running";
  /** Short label, already localized by the provider. */
  label: string;
}

/** Latest build / test status of the project (quality runner, live). */
export function useHudQuality(projectId: string | null): HudQuality | null {
  const { runs } = useQualityLive(projectId);
  return useMemo(() => {
    const facts = qualityFactsOf(runs);
    const state = hudStateOf(facts);
    if (!state) return null;
    const tests = facts.lastTests;
    const label =
      state === "running"
        ? t("hud:quality.running")
        : state === "failing"
          ? facts.lastBuild && !facts.lastBuild.ok
            ? t("hud:quality.buildFailed")
            : t("hud:quality.testsFailed", { count: tests?.failed ?? 0 })
          : tests
            ? t("hud:quality.testsPassed", { count: tests.passed })
            : t("hud:quality.buildOk");
    return { state, label };
  }, [runs]);
}
