/** Update state for Settings → Updates and the title-bar badge (main window only). */
import { create } from "zustand";

import { type ErrorInfo, parseUpdateError, type UpdateSettings, type UpdateStatus, updatesApi, updatesAvailableHere } from "./api";

export type InstallWhen = "now" | "onExit" | "cancel";

interface UpdatesState {
  /** `null` until loaded (or outside the desktop shell). */
  status: UpdateStatus | null;
  /** A command is in flight. */
  busy: boolean;
  /** Error of the last command (check/download failures arrive in `status.lastError`). */
  error: ErrorInfo | null;
  apply: (status: UpdateStatus) => void;
  refresh: () => Promise<void>;
  saveSettings: (patch: Partial<UpdateSettings>) => Promise<boolean>;
  check: (localPath?: string | null) => Promise<void>;
  download: (allowUnsigned?: boolean) => Promise<void>;
  cancel: () => Promise<void>;
  install: (when: InstallWhen) => Promise<void>;
  backupNow: () => Promise<void>;
}

export const useUpdatesStore = create<UpdatesState>((set, get) => {
  const run = async (f: () => Promise<UpdateStatus>): Promise<boolean> => {
    set({ busy: true, error: null });
    try {
      get().apply(await f());
      return true;
    } catch (e) {
      set({ error: parseUpdateError(e) });
      return false;
    } finally {
      set({ busy: false });
    }
  };
  return {
    status: null,
    busy: false,
    error: null,
    apply: (status) => set({ status }),
    refresh: async () => {
      await run(() => updatesApi.status());
    },
    saveSettings: (patch) => {
      const current = get().status?.settings;
      if (!current) return Promise.resolve(false);
      return run(() => updatesApi.setSettings({ ...current, ...patch }));
    },
    check: async (localPath) => {
      await run(() => updatesApi.check(localPath));
    },
    download: async (allowUnsigned = false) => {
      await run(() => updatesApi.download(allowUnsigned));
    },
    cancel: async () => {
      await run(() => updatesApi.cancel());
    },
    install: async (when) => {
      await run(() => updatesApi.install(when));
    },
    backupNow: async () => {
      await run(() => updatesApi.backupNow());
    },
  };
});

/** Whether the title-bar badge shows (an update is on offer). */
export function hasUpdateBadge(status: UpdateStatus | null): boolean {
  return !!status?.available && (status.phase === "available" || status.phase === "downloading" || status.phase === "ready");
}

/** Loads the status and follows `orkestr://updates/status`. Returns a disposer. */
export function startUpdatesWatcher(): () => void {
  if (!updatesAvailableHere()) return () => {};
  let off: (() => void) | undefined;
  let disposed = false;
  updatesApi
    .onStatus((s) => useUpdatesStore.getState().apply(s))
    .then(
      (u) => (disposed ? u() : (off = u)),
      (e: unknown) => console.warn("updates listener", e),
    );
  void useUpdatesStore.getState().refresh();
  return () => {
    disposed = true;
    off?.();
  };
}
