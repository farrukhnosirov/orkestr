/**
 * Application settings (`settings.get` / `settings.update`, `SettingsChanged`
 * events) plus the Settings overlay's view state. Business data comes from
 * the core over RPC only; the native shell merely mirrors window behaviour.
 */
import type { AppSettings, AppSettingsPatch } from "@orkestr/protocol";
import { create } from "zustand";

import { getClient } from "../../core/client";
import { shellControls } from "../../lib/tauri";

export interface SettingsState {
  settings: AppSettings | null;
  autostartSupported: boolean;
  loading: boolean;
  error: string | null;
  /** Field currently being saved (for inline spinners). */
  saving: keyof AppSettings | null;
  open: boolean;
  section: string;

  load: () => Promise<void>;
  update: (patch: AppSettingsPatch) => Promise<boolean>;
  applyRemote: (settings: AppSettings) => void;
  openSettings: (section?: string) => void;
  closeSettings: () => void;
  setSection: (id: string) => void;
}

function errorText(e: unknown): string {
  return e instanceof Error ? e.message : String(e);
}

function mirrorShell(s: AppSettings): void {
  shellControls.applyPrefs({ closeToTray: s.closeToTray }).catch((e: unknown) => console.warn("shell prefs", e));
}

export const useSettingsStore = create<SettingsState>()((set, get) => ({
  settings: null,
  autostartSupported: false,
  loading: false,
  error: null,
  saving: null,
  open: false,
  section: "general",

  load: async () => {
    set({ loading: true, error: null });
    try {
      const r = await getClient().call("settings.get", {});
      set({ settings: r.settings, autostartSupported: r.autostartSupported, loading: false });
      mirrorShell(r.settings);
    } catch (e) {
      set({ loading: false, error: errorText(e) });
    }
  },

  update: async (patch) => {
    const key = (Object.keys(patch)[0] ?? null) as keyof AppSettings | null;
    set({ saving: key, error: null });
    try {
      const r = await getClient().call("settings.update", { patch });
      set({ settings: r.settings, autostartSupported: r.autostartSupported, saving: null });
      mirrorShell(r.settings);
      return true;
    } catch (e) {
      set({ saving: null, error: errorText(e) });
      return false;
    }
  },

  applyRemote: (settings) => {
    const prev = get().settings;
    set({ settings });
    if (!prev || prev.closeToTray !== settings.closeToTray) mirrorShell(settings);
  },

  openSettings: (section) => set((s) => ({ open: true, section: section ?? s.section })),
  closeSettings: () => set({ open: false }),
  setSection: (id) => set({ section: id }),
}));
