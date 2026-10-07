/**
 * Keeps the file icon theme in sync with the editor preferences (used by the
 * Files and Code panels).
 */
import { useEffect } from "react";

import { useAppStore } from "../../store/app";
import { applyIconTheme } from "./iconTheme";
import { ensureExtensionsLoaded, useExtensionsStore } from "./store";

/** Whether the Orkestr UI currently uses a light background. */
export function isLightUi(): boolean {
  if (typeof document === "undefined") return false;
  const raw = getComputedStyle(document.documentElement).getPropertyValue("--ok-color-bg").trim();
  const m = /^#([0-9a-f]{2})([0-9a-f]{2})([0-9a-f]{2})/i.exec(raw);
  if (!m?.[1] || !m[2] || !m[3]) return false;
  const [r, g, b] = [m[1], m[2], m[3]].map((h) => parseInt(h, 16) / 255) as [number, number, number];
  return 0.2126 * r + 0.7152 * g + 0.0722 * b > 0.5;
}

/** Loads the extension registry and applies the selected file icon theme. */
export function useIconThemeSync(): void {
  const connected = useAppStore((s) => s.connection.state === "open");
  const iconTheme = useExtensionsStore((s) => s.prefs.iconTheme);
  const version = useExtensionsStore((s) => s.version);
  useEffect(() => {
    if (connected) ensureExtensionsLoaded();
  }, [connected]);
  useEffect(() => {
    if (!connected) return;
    applyIconTheme(iconTheme, isLightUi(), version).catch((e: unknown) => console.warn("icon theme failed", e));
  }, [connected, iconTheme, version]);
}
