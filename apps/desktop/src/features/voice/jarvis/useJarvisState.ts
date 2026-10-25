/**
 * Hooks shared by the Jarvis hosts: the visual state of a voice phase (the
 * wake bloom is timed locally, per window), the debug override used for
 * screenshots, and the transcript split (partial while listening, final after).
 */
import { useEffect, useRef, useState } from "react";

import { type JarvisMode, type JarvisState, jarvisStateOf, WAKE_INTRO_MS } from "./model";

/** {@link jarvisStateOf} with the wake window measured from when listening began here. */
export function useJarvisState(phase: string, speaking: boolean, source: string | null | undefined): JarvisState {
  const since = useRef(0);
  const prev = useRef(phase);
  const [, tick] = useState(0);
  if (phase === "listening" && prev.current !== "listening") since.current = performance.now();
  prev.current = phase;
  const elapsed = phase === "listening" ? performance.now() - since.current : Infinity;
  const state = jarvisStateOf({ phase, speaking, source: source ?? null, sinceListenMs: elapsed });
  useEffect(() => {
    if (state !== "wake") return undefined;
    const t = window.setTimeout(() => tick((n) => n + 1), Math.max(0, WAKE_INTRO_MS - elapsed) + 10);
    return () => window.clearTimeout(t);
  }, [state, elapsed]);
  return state;
}

/** Forced card for screenshots / manual checks (`__orkestrDebug.jarvis.set`). */
export interface JarvisDebugOverride {
  state: JarvisState;
  level?: number;
  partialText?: string;
  finalText?: string;
  replyText?: string;
  confirmText?: string | null;
  mode?: JarvisMode;
  phase?: string;
  speaking?: boolean;
  engine?: string;
  micOpen?: boolean;
}

/**
 * `window.__orkestrDebug.jarvis.set({...} | null)` when the debug-hooks flag
 * is on (`localStorage["orkestr.debugHooks"] = "1"`). Off by default.
 */
export function useJarvisDebug(): JarvisDebugOverride | null {
  const [forced, setForced] = useState<JarvisDebugOverride | null>(null);
  useEffect(() => {
    let on = false;
    try {
      on = window.localStorage.getItem("orkestr.debugHooks") === "1";
    } catch {
      on = false;
    }
    if (!on) return undefined;
    const w = window as unknown as { __orkestrDebug?: Record<string, unknown> };
    w.__orkestrDebug = { ...(w.__orkestrDebug ?? {}), jarvis: { set: (p: JarvisDebugOverride | null) => setForced(p) } };
    return () => {
      if (w.__orkestrDebug) delete w.__orkestrDebug["jarvis"];
    };
  }, []);
  return forced;
}
