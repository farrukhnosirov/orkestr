/**
 * Theme state: the effective UI config of this device (`ui.getConfig`), the
 * theme library (`ui.themes`), per-device selection and the editor draft.
 * Business data comes only from the core over RPC (§67); the core validates
 * every write (ADR-0015). Live `UiConfigChanged` / `UiThemesChanged` events
 * keep every window and client in sync.
 */
import type { ContrastCheck, ThemeConfig, ThemeEntry, UiConfig, UiConfigScope } from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { defaultTheme, exportThemeFile, normalizeTheme, parseThemeFile, validateTheme } from "@orkestr/ui";
import { create } from "zustand";

import { getClient, maybeClient } from "../../core/client";
import { describeError } from "../../i18n/errors";
import { type DraftState, duplicateTheme, editDraft, prepareImport, resetDraft, savesAsNew, startDraft, uniqueThemeId } from "./logic";

const DEVICE_KEY = "orkestr.deviceId";
const CACHE_KEY = "orkestr.theme.cache";

function storage(): Storage | null {
  try {
    return window.localStorage;
  } catch {
    return null;
  }
}

/** Stable id of this device/client (per-device theme selection, §34). */
export function deviceId(): string {
  const s = storage();
  const existing = s?.getItem(DEVICE_KEY);
  if (existing && /^[A-Za-z0-9-]{1,64}$/.test(existing)) return existing;
  const id = typeof crypto !== "undefined" && "randomUUID" in crypto ? crypto.randomUUID() : `d-${Date.now().toString(36)}`;
  s?.setItem(DEVICE_KEY, id);
  return id;
}

/** Last applied theme (applied before the core connects, so the app never flashes the default). */
export function cachedTheme(): ThemeConfig | null {
  try {
    const raw = storage()?.getItem(CACHE_KEY);
    if (!raw) return null;
    const t = normalizeTheme(JSON.parse(raw));
    return validateTheme(t).length === 0 ? t : null;
  } catch {
    return null;
  }
}

function cacheTheme(theme: ThemeConfig): void {
  try {
    storage()?.setItem(CACHE_KEY, JSON.stringify(theme));
  } catch {
    /* storage unavailable */
  }
}

export type ThemeNotice = { kind: "ok" | "error"; text: string } | null;

export interface ThemeState {
  /** Effective config (device override or global); null before the first load. */
  config: UiConfig | null;
  version: number;
  scope: UiConfigScope;
  contrast: ContrastCheck[];
  themes: ThemeEntry[];
  loaded: boolean;
  busy: boolean;
  error: string | null;
  /** Theme applied while nothing is loaded yet (cache or default). */
  fallback: ThemeConfig;

  editorOpen: boolean;
  editor: DraftState | null;

  init: () => () => void;
  refresh: () => Promise<void>;
  refreshThemes: () => Promise<void>;
  selectTheme: (id: string) => Promise<boolean>;
  setPerDevice: (on: boolean) => Promise<boolean>;
  saveTheme: (theme: ThemeConfig, overwrite: boolean) => Promise<ThemeEntry | null>;
  deleteTheme: (id: string) => Promise<boolean>;
  duplicate: (id: string, name: string) => Promise<ThemeEntry | null>;

  openEditor: (theme?: ThemeConfig) => void;
  closeEditor: () => void;
  edit: (fn: (t: ThemeConfig) => ThemeConfig) => void;
  resetEditor: () => void;
  /** Saves the draft (new id for built-ins/new themes) and makes it active. */
  saveEditor: () => Promise<boolean>;
  /** Parses a theme file into the editor (not saved until the user saves). */
  importIntoEditor: (json: string) => { ok: true } | { ok: false; issues: string[] };
  exportJson: (theme: ThemeConfig) => string;
}

/** The theme the app should show now: valid editor preview > config > fallback. */
export function activeTheme(s: Pick<ThemeState, "editorOpen" | "editor" | "config" | "fallback">): ThemeConfig {
  if (s.editorOpen && s.editor) return s.editor.preview;
  return s.config?.theme ?? s.fallback;
}

export function themeById(s: Pick<ThemeState, "themes">, id: string): ThemeConfig | null {
  return s.themes.find((e) => e.theme.id === id)?.theme ?? null;
}

function userIds(themes: readonly ThemeEntry[]): Set<string> {
  return new Set(themes.filter((e) => !e.builtin).map((e) => e.theme.id));
}

export const useThemeStore = create<ThemeState>()((set, get) => ({
  config: null,
  version: 0,
  scope: "Global",
  contrast: [],
  themes: [],
  loaded: false,
  busy: false,
  error: null,
  fallback: cachedTheme() ?? defaultTheme(),
  editorOpen: false,
  editor: null,

  init: () => {
    void get().refresh();
    void get().refreshThemes();
    const client = maybeClient();
    if (!client) return () => undefined;
    const me = deviceId();
    return client.subscribe([TOPIC_GLOBAL], (ev) => {
      if (ev.kind === "UiConfigChanged") {
        const d = ev.payload.deviceId;
        if (d === null || d === me) void get().refresh();
      } else if (ev.kind === "UiThemesChanged") {
        void get().refreshThemes();
      }
    });
  },

  refresh: async () => {
    try {
      const r = await getClient().call("ui.getConfig", { deviceId: deviceId() });
      cacheTheme(r.config.theme);
      set({ config: r.config, version: r.version, scope: r.scope, contrast: r.contrast, loaded: true, error: null });
    } catch (e) {
      set({ error: describeError(e), loaded: true });
    }
  },

  refreshThemes: async () => {
    try {
      const r = await getClient().call("ui.themes", {});
      set({ themes: r.themes });
    } catch (e) {
      set({ error: describeError(e) });
    }
  },

  selectTheme: async (id) => {
    const s = get();
    const theme = themeById(s, id);
    if (!theme || !s.config) return false;
    const config: UiConfig = { ...s.config, theme: structuredClone(theme) };
    set({ busy: true, error: null, config });
    try {
      const params = s.scope === "Device" ? { config, deviceId: deviceId() } : { config };
      const r = await getClient().call("ui.setConfig", params);
      cacheTheme(r.config.theme);
      set({ config: r.config, version: r.version, scope: r.scope, contrast: r.contrast, busy: false });
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e), config: s.config });
      return false;
    }
  },

  setPerDevice: async (on) => {
    const s = get();
    if (!s.config) return false;
    set({ busy: true, error: null });
    try {
      const r = await getClient().call("ui.setConfig", { config: on ? s.config : null, deviceId: deviceId() });
      cacheTheme(r.config.theme);
      set({ config: r.config, version: r.version, scope: r.scope, contrast: r.contrast, busy: false });
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return false;
    }
  },

  saveTheme: async (theme, overwrite) => {
    set({ busy: true, error: null });
    try {
      const r = await getClient().call("ui.saveTheme", { theme, overwrite });
      await get().refreshThemes();
      set({ busy: false });
      return r.entry;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return null;
    }
  },

  deleteTheme: async (id) => {
    set({ busy: true, error: null });
    try {
      await getClient().call("ui.deleteTheme", { id });
      await Promise.all([get().refreshThemes(), get().refresh()]);
      set({ busy: false });
      return true;
    } catch (e) {
      set({ busy: false, error: describeError(e) });
      return false;
    }
  },

  duplicate: async (id, name) => {
    const s = get();
    const theme = themeById(s, id);
    if (!theme) return null;
    const ids = new Set(s.themes.map((e) => e.theme.id));
    return get().saveTheme(duplicateTheme(theme, name, ids), false);
  },

  openEditor: (theme) => {
    const s = get();
    const base = theme ?? s.config?.theme ?? s.fallback;
    set({ editorOpen: true, editor: startDraft(base) });
  },

  closeEditor: () => set({ editorOpen: false, editor: null }),

  edit: (fn) => {
    const ed = get().editor;
    if (ed) set({ editor: editDraft(ed, fn) });
  },

  resetEditor: () => {
    const ed = get().editor;
    if (ed) set({ editor: resetDraft(ed) });
  },

  saveEditor: async () => {
    const s = get();
    const ed = s.editor;
    if (!ed || ed.issues.length > 0) return false;
    const users = userIds(s.themes);
    let theme = ed.draft;
    const asNew = savesAsNew(ed, users);
    if (asNew) {
      const taken = new Set(s.themes.map((e) => e.theme.id));
      theme = { ...theme, id: taken.has(theme.id) ? uniqueThemeId(theme.name, taken) : theme.id };
    }
    const entry = await get().saveTheme(theme, !asNew);
    if (!entry) return false;
    const ok = await get().selectTheme(entry.theme.id);
    if (ok) set({ editor: startDraft(entry.theme) });
    return ok;
  },

  importIntoEditor: (json) => {
    const parsed = parseThemeFile(json);
    if (!parsed.ok) return { ok: false, issues: parsed.issues.map((i) => (i.path ? `${i.path}: ${i.message}` : i.message)) };
    const taken = new Set(get().themes.map((e) => e.theme.id));
    const theme = prepareImport(parsed.theme, taken);
    const ed = get().editor;
    if (ed && get().editorOpen) set({ editor: editDraft(ed, () => theme) });
    else set({ editorOpen: true, editor: { ...startDraft(theme), dirty: true } });
    return { ok: true };
  },

  exportJson: (theme) => exportThemeFile(normalizeTheme(theme)),
}));
