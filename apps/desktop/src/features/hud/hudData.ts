/**
 * HUD data (RPC + events only): Manager runs of the active project, plus
 * typed slots other areas plug into.
 *
 * - Voice: INTEGRATOR(P3 voice-sound): call `registerHudVoiceSlot()` with
 *   the live voice indicator component; until then the HUD shows a muted mic.
 * - Quality: INTEGRATOR(P2): wire `useHudQuality` to the quality pipeline
 *   (latest build / test result of the active project). Returns `null`
 *   (chip hidden) until then.
 */
import type { Event, ManagerRun } from "@orkestr/protocol";
import { topic } from "@orkestr/protocol";
import { useEffect, useState, useSyncExternalStore, type ComponentType } from "react";

import { maybeClient } from "../../core/client";
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

/** INTEGRATOR(P2): latest build/test status of the project. */
export function useHudQuality(_projectId: string | null): HudQuality | null {
  return null;
}
