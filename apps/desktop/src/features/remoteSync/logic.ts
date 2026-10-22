/**
 * Remote & Sync helpers: error classification and Seamless Continue UI state
 * (capture on this device, restore on the other one; ADR-0031).
 */
import { OrkestrError } from "@orkestr/client";

import { useEditorStore } from "../../store/editor";

function errorData(e: unknown): Record<string, unknown> | null {
  if (!(e instanceof OrkestrError)) return null;
  const d = e.data;
  return d && typeof d === "object" ? (d as Record<string, unknown>) : null;
}

/** True when the core answered `unimplementedFeature` (RR-G14). */
export function isUnimplemented(e: unknown): boolean {
  return typeof errorData(e)?.unimplementedFeature === "string";
}

/** `data.syncError` kind of a `sync.*` failure (`policyDenied`, `network`, `integrity`, …). */
export function syncErrorKind(e: unknown): string | null {
  const k = errorData(e)?.syncError;
  return typeof k === "string" ? k : null;
}

/** UI state handed over by `sync.continueOn` (§9). */
export type HandoffUiState = {
  openEditors: { path: string }[];
  activePath: string | null;
};

/** Captures the editor tabs of the active project. */
export function captureUiState(): HandoffUiState {
  const { tabs, activePath } = useEditorStore.getState();
  return { openEditors: tabs.map((t) => ({ path: t.path })), activePath };
}

/** Re-opens the editors of a handed-over UI state; returns how many were opened. */
export function restoreUiState(state: unknown): number {
  if (!state || typeof state !== "object") return 0;
  const s = state as Partial<HandoffUiState>;
  const editors = Array.isArray(s.openEditors) ? s.openEditors : [];
  const open = useEditorStore.getState().openFile;
  let n = 0;
  for (const e of editors) {
    if (e && typeof e.path === "string" && e.path.length > 0) {
      open(e.path);
      n += 1;
    }
  }
  if (typeof s.activePath === "string" && s.activePath) open(s.activePath);
  return n;
}
