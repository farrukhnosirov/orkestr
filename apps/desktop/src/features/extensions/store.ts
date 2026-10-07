/**
 * Installed VS Code extensions, editor appearance preferences and install
 * progress (ADR-0025). Business data comes from the core (`extensions.*`);
 * this store only mirrors it and refreshes on `ExtensionsChanged`.
 */
import type {
  EditorPrefs,
  Event,
  ExtensionInstallProgressPayload,
  InstalledExtension,
} from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";

export interface ExtensionsState {
  installed: InstalledExtension[];
  prefs: EditorPrefs;
  loaded: boolean;
  /** Install progress by key (`namespace.name` or `.vsix` file name). */
  progress: Record<string, ExtensionInstallProgressPayload>;
  /** Bumped on every registry / preference change (editor integrations re-apply). */
  version: number;
  load: () => Promise<void>;
  applyProgress: (p: ExtensionInstallProgressPayload) => void;
}

export const useExtensionsStore = create<ExtensionsState>((set, get) => ({
  installed: [],
  prefs: { editorTheme: null, iconTheme: null },
  loaded: false,
  progress: {},
  version: 0,
  load: async () => {
    const client = maybeClient();
    if (!client) return;
    const [list, prefs] = await Promise.all([client.call("extensions.list", {}), client.call("extensions.getPrefs", {})]);
    set({ installed: list.extensions, prefs, loaded: true, version: get().version + 1 });
  },
  applyProgress: (p) => {
    const progress = { ...get().progress, [p.key]: p };
    if (p.phase === "done") delete progress[p.key];
    set({ progress });
  },
}));

let wired = false;
let reloadTimer: ReturnType<typeof setTimeout> | null = null;

function onEvent(ev: Event): void {
  if (ev.kind === "ExtensionInstallProgress") {
    useExtensionsStore.getState().applyProgress(ev.payload);
  } else if (ev.kind === "ExtensionsChanged") {
    if (reloadTimer !== null) clearTimeout(reloadTimer);
    reloadTimer = setTimeout(() => {
      reloadTimer = null;
      useExtensionsStore.getState().load().catch((e: unknown) => console.warn("extensions reload failed", e));
    }, 100);
  }
}

/**
 * Loads the registry once and keeps it fresh (idempotent; call from any
 * consumer — the Extensions panel, the Code panel, the Files panel).
 */
export function ensureExtensionsLoaded(): void {
  const client = maybeClient();
  if (!client) return;
  if (!wired) {
    wired = true;
    client.subscribe([TOPIC_GLOBAL], onEvent);
  }
  if (!useExtensionsStore.getState().loaded) {
    useExtensionsStore.getState().load().catch((e: unknown) => console.warn("extensions load failed", e));
  }
}

/** Enabled extensions. */
export function enabledExtensions(list: readonly InstalledExtension[]): InstalledExtension[] {
  return list.filter((e) => e.enabled);
}
