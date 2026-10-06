/**
 * Effective Performance Mode across windows (§31). The main window derives
 * the effective mode (auto Battery Saver on battery, frame-budget steps) and
 * broadcasts it; separate webviews that draw expensive visuals — the
 * always-on-top robot windows — follow it instead of the user's raw choice.
 * Same-origin `BroadcastChannel`, plain data only.
 */
import type { PerformanceMode } from "@orkestr/protocol";

import { PERF_MODES } from "./policy";
import { usePerfStore } from "./perfStore";

const CHANNEL = "orkestr.perf";

type Message = { type: "mode"; mode: PerformanceMode } | { type: "hello" };

function channel(): BroadcastChannel | null {
  try {
    return typeof BroadcastChannel === "function" ? new BroadcastChannel(CHANNEL) : null;
  } catch {
    return null;
  }
}

function isMode(v: unknown): v is PerformanceMode {
  return typeof v === "string" && (PERF_MODES as readonly string[]).includes(v);
}

/** Main window: publishes the effective mode on every change and to windows that say hello. */
export function startPerfBroadcast(): () => void {
  const ch = channel();
  if (!ch) return () => undefined;
  const post = () => ch.postMessage({ type: "mode", mode: usePerfStore.getState().mode } satisfies Message);
  ch.onmessage = (e: MessageEvent<Message>) => {
    if (e.data?.type === "hello") post();
  };
  const off = usePerfStore.subscribe((s, prev) => {
    if (s.mode !== prev.mode) post();
  });
  post();
  return () => {
    off();
    ch.close();
  };
}

/** Other windows: calls `onMode` with the main window's effective mode (now and on every change). */
export function subscribeEffectiveMode(onMode: (mode: PerformanceMode) => void): () => void {
  const ch = channel();
  if (!ch) return () => undefined;
  ch.onmessage = (e: MessageEvent<Message>) => {
    const msg = e.data;
    if (msg?.type === "mode" && isMode(msg.mode)) onMode(msg.mode);
  };
  ch.postMessage({ type: "hello" } satisfies Message);
  return () => ch.close();
}
