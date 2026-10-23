/**
 * Embedded VS Code: status mirror of the core's `vscode.*` area, install
 * progress, the VS Code window state and the keep-alive heartbeat.
 *
 * All data comes over RPC (`vscode.status`, `VsCodeStatusChanged`,
 * `VsCodeProgress` events). The heartbeat (`vscode.keepAlive`) runs while a
 * VS Code panel is mounted or the VS Code window is open, so the core's
 * idle auto-stop only stops the server when nobody is using it.
 */
import type { OrkestrClient } from "@orkestr/client";
import type {
  Event,
  VsCodeConfigPatch,
  VsCodeExtension,
  VsCodeProgress,
  VsCodeRelease,
  VsCodeStatus,
} from "@orkestr/protocol";
import { TOPIC_GLOBAL } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { documentThemeColors, embedUrlFor } from "./embed";
import { vscodeWindow } from "./windowControls";

/** Heartbeat period (the core's idle grace period is ≥ 30 s). */
export const KEEPALIVE_MS = 20_000;
/** Extension installs run the server CLI (large packages, up to 10 min on the core side). */
export const EXTENSION_INSTALL_TIMEOUT_MS = 11 * 60_000;
/** Listing runs the server CLI too (Node start-up). */
export const EXTENSION_LIST_TIMEOUT_MS = 60_000;

export interface VsCodeStore {
  status: VsCodeStatus | null;
  progress: VsCodeProgress | null;
  release: VsCodeRelease | null;
  releaseError: string | null;
  checkingRelease: boolean;
  logs: string[];
  /** Last action error (start / stop / install…), localized. */
  error: string | null;
  busy: "start" | "stop" | "install" | "uninstall" | "config" | null;
  windowOpen: boolean;
  /** Mounted VS Code panels (heartbeat while > 0). */
  viewers: number;
  /** URL of the last start (contains the token: memory only, never persisted). */
  lastUrl: string | null;
  lastOrigin: string | null;
  /** Section of the VS Code panel to bring into view (quick access: Ctrl+Shift+X). */
  view: "main" | "extensions";
  /** Bumped on every `showView` so the panel re-focuses even if the view is unchanged. */
  viewNonce: number;
  /** Extensions installed in Orkestr's VS Code (`null` = not loaded). */
  extensions: VsCodeExtension[] | null;
  extensionsError: string | null;
  /** Extension id being installed. */
  installingExtension: string | null;
  /** Extension id (or `"*"` for update-all / VSIX) a management action runs for. */
  managingExtension: string | null;
  /** Frame URL of the embedded workbench (contains the token: memory only). */
  embedUrl: string | null;
  /** Project the embedded workbench shows. */
  embedProjectId: string | null;
  /** Bumped to reload the embedded workbench frame. */
  embedKey: number;
  /** The page cannot embed the workbench (e.g. a remote web client). */
  embedUnsupported: boolean;

  load(): Promise<void>;
  showView(view: "main" | "extensions"): void;
  loadExtensions(): Promise<void>;
  /** Installs an Open VSX extension; resolves to whether it succeeded. */
  installExtension(id: string): Promise<boolean>;
  checkRelease(): Promise<VsCodeRelease | null>;
  install(version: string | null): Promise<void>;
  cancelInstall(): Promise<void>;
  uninstall(purge: boolean): Promise<void>;
  /** Starts (or reuses) the server for a project and opens the VS Code window. */
  open(projectId: string | null, title: string): Promise<void>;
  stop(): Promise<void>;
  setConfig(patch: VsCodeConfigPatch): Promise<void>;
  loadLogs(): Promise<void>;
  /** Panel mount / unmount (drives the heartbeat). */
  attach(): () => void;
  setWindowOpen(open: boolean): void;
  applyEvent(event: Event): void;
  /** Starts (or reuses) the server and shows the workbench inside the panel. */
  startEmbedded(projectId: string | null, reload?: boolean): Promise<boolean>;
  /** Reloads the embedded workbench frame. */
  reloadEmbedded(): void;
  /** Stops and starts the server (applies enable/disable of extensions). */
  restart(projectId: string | null): Promise<void>;
  uninstallExtension(id: string): Promise<boolean>;
  installVsix(path: string): Promise<boolean>;
  updateExtensions(): Promise<boolean>;
  setExtensionEnabled(id: string, enabled: boolean): Promise<boolean>;
  /** Pushes the current Orkestr theme into the connected workbench (best effort). */
  syncTheme(waitMs?: number): Promise<void>;
}

/** Window event: the embedded VS Code asked to give the keyboard back to Orkestr. */
export const VSCODE_FOCUS_OUT_EVENT = "orkestr:vscode-focus-out";
/** Window event: the embedded VS Code asked for Orkestr's command palette. */
export const VSCODE_PALETTE_EVENT = "orkestr:vscode-palette";

/** Host name of this page (the frame must be same-site with it). */
function appHost(): string {
  return typeof window === "undefined" ? "" : window.location.hostname;
}

function client(): OrkestrClient {
  const c = maybeClient();
  if (!c) throw new Error(t("common:errors.notInitialised"));
  return c;
}

let subscribed: OrkestrClient | null = null;
let unsubscribe: (() => void) | null = null;
let heartbeat: ReturnType<typeof setInterval> | null = null;
let windowListener: Promise<() => void> | null = null;

function syncHeartbeat(): void {
  const { viewers, windowOpen } = useVsCodeStore.getState();
  const wanted = viewers > 0 || windowOpen;
  if (wanted && !heartbeat) {
    const beat = () => void maybeClient()?.call("vscode.keepAlive", {}).catch(() => undefined);
    beat();
    heartbeat = setInterval(beat, KEEPALIVE_MS);
  } else if (!wanted && heartbeat) {
    clearInterval(heartbeat);
    heartbeat = null;
  }
}

function ensureSubscribed(): void {
  const c = maybeClient();
  if (!c || subscribed === c) return;
  unsubscribe?.();
  subscribed = c;
  unsubscribe = c.subscribe([TOPIC_GLOBAL], (ev) => useVsCodeStore.getState().applyEvent(ev));
  windowListener ??= vscodeWindow.onChange((open) => useVsCodeStore.getState().setWindowOpen(open));
  void vscodeWindow
    .isOpen()
    .then((open) => useVsCodeStore.getState().setWindowOpen(open))
    .catch(() => undefined);
}

type SetState = (partial: Partial<VsCodeStore> | ((s: VsCodeStore) => Partial<VsCodeStore>)) => void;

/** Runs one extension management call and stores the resulting list. */
async function manage(
  set: SetState,
  id: string,
  call: () => Promise<{ extensions: VsCodeExtension[] }>,
): Promise<boolean> {
  set({ managingExtension: id, extensionsError: null });
  try {
    const { extensions } = await call();
    set({ extensions, managingExtension: null });
    return true;
  } catch (e) {
    set({ extensionsError: describeError(e), managingExtension: null });
    return false;
  }
}

export const useVsCodeStore = create<VsCodeStore>((set, get) => ({
  status: null,
  progress: null,
  release: null,
  releaseError: null,
  checkingRelease: false,
  logs: [],
  error: null,
  busy: null,
  windowOpen: false,
  viewers: 0,
  lastUrl: null,
  lastOrigin: null,
  view: "main",
  viewNonce: 0,
  extensions: null,
  extensionsError: null,
  installingExtension: null,
  managingExtension: null,
  embedUrl: null,
  embedProjectId: null,
  embedKey: 0,
  embedUnsupported: false,

  showView(view) {
    set((s) => ({ view, viewNonce: s.viewNonce + 1 }));
  },

  async loadExtensions() {
    try {
      const { extensions } = await client().call("vscode.listExtensions", {}, { timeoutMs: EXTENSION_LIST_TIMEOUT_MS });
      set({ extensions, extensionsError: null });
    } catch (e) {
      set({ extensionsError: describeError(e) });
    }
  },

  async installExtension(id) {
    const trimmed = id.trim();
    if (!trimmed) return false;
    set({ installingExtension: trimmed, extensionsError: null });
    try {
      const { extensions } = await client().call(
        "vscode.installExtension",
        { id: trimmed },
        { timeoutMs: EXTENSION_INSTALL_TIMEOUT_MS },
      );
      set({ extensions, installingExtension: null });
      return true;
    } catch (e) {
      set({ extensionsError: describeError(e), installingExtension: null });
      return false;
    }
  },

  async startEmbedded(projectId, reload = false) {
    set({ busy: "start", error: null });
    try {
      const res = await client().call("vscode.start", projectId ? { projectId } : {});
      const embedUrl = embedUrlFor(res.url, appHost());
      set((s) => ({
        status: res.status,
        lastUrl: res.url,
        lastOrigin: res.origin,
        embedUrl,
        embedProjectId: projectId,
        embedUnsupported: embedUrl === null,
        // Same URL again (e.g. after a crash restart): reload the frame on request.
        embedKey: reload && s.embedUrl === embedUrl ? s.embedKey + 1 : s.embedKey,
        busy: null,
      }));
      return embedUrl !== null;
    } catch (e) {
      set({ error: describeError(e), busy: null });
      void get().loadLogs();
      return false;
    }
  },

  reloadEmbedded() {
    set((s) => ({ embedKey: s.embedKey + 1 }));
  },

  async restart(projectId) {
    await get().stop();
    if (!get().error) await get().startEmbedded(projectId);
    void get().loadExtensions();
  },

  async uninstallExtension(id) {
    return manage(set, id, () => client().call("vscode.uninstallExtension", { id }, { timeoutMs: EXTENSION_LIST_TIMEOUT_MS * 3 }));
  },

  async installVsix(path) {
    return manage(set, "*", () => client().call("vscode.installVsix", { path }, { timeoutMs: EXTENSION_INSTALL_TIMEOUT_MS }));
  },

  async updateExtensions() {
    return manage(set, "*", () => client().call("vscode.updateExtensions", {}, { timeoutMs: EXTENSION_INSTALL_TIMEOUT_MS }));
  },

  async setExtensionEnabled(id, enabled) {
    return manage(set, id, () =>
      client().call("vscode.setExtensionEnabled", { id, enabled }, { timeoutMs: EXTENSION_LIST_TIMEOUT_MS }),
    );
  },

  async syncTheme(waitMs = 0) {
    const status = get().status;
    if (!status?.config.syncTheme || !status.bridgeConnected) return;
    const c = maybeClient();
    if (!c) return;
    const { light, colors } = documentThemeColors();
    if (Object.keys(colors).length === 0) return;
    try {
      await c.call("vscode.syncTheme", { kind: light ? "light" : "dark", colors, waitMs }, { timeoutMs: 30_000 + waitMs });
    } catch {
      // Cosmetic: the next theme change or reconnect retries.
    }
  },

  async load() {
    ensureSubscribed();
    try {
      const status = await client().call("vscode.status", {});
      set({ status, progress: status.progress });
    } catch (e) {
      set({ error: describeError(e) });
    }
  },

  async checkRelease() {
    set({ checkingRelease: true, releaseError: null });
    try {
      const release = await client().call("vscode.checkUpdate", {});
      set({ release, checkingRelease: false });
      return release;
    } catch (e) {
      set({ releaseError: describeError(e), checkingRelease: false });
      return null;
    }
  },

  async install(version) {
    set({ busy: "install", error: null });
    try {
      const status = await client().call("vscode.install", version ? { accept: true, version } : { accept: true });
      set({ status, busy: null });
    } catch (e) {
      set({ error: describeError(e), busy: null });
    }
  },

  async cancelInstall() {
    try {
      set({ status: await client().call("vscode.cancelInstall", {}) });
    } catch (e) {
      set({ error: describeError(e) });
    }
  },

  async uninstall(purge) {
    set({ busy: "uninstall", error: null });
    try {
      await vscodeWindow.close().catch(() => undefined);
      const status = await client().call("vscode.uninstall", { purge });
      set({ status, busy: null, lastUrl: null, lastOrigin: null, embedUrl: null, release: null });
    } catch (e) {
      set({ error: describeError(e), busy: null });
    }
  },

  async open(projectId, title) {
    set({ busy: "start", error: null });
    try {
      const res = await client().call("vscode.start", projectId ? { projectId } : {});
      set({ status: res.status, lastUrl: res.url, lastOrigin: res.origin, busy: null });
      await vscodeWindow.open(res.url, title);
      get().setWindowOpen(await vscodeWindow.isOpen().catch(() => false));
    } catch (e) {
      set({ error: describeError(e), busy: null });
      void get().loadLogs();
    }
  },

  async stop() {
    set({ busy: "stop", error: null });
    try {
      await vscodeWindow.close().catch(() => undefined);
      const status = await client().call("vscode.stop", {});
      set({ status, busy: null, lastUrl: null, lastOrigin: null, embedUrl: null });
    } catch (e) {
      set({ error: describeError(e), busy: null });
    }
  },

  async setConfig(patch) {
    set({ busy: "config", error: null });
    try {
      set({ status: await client().call("vscode.setConfig", patch), busy: null });
    } catch (e) {
      set({ error: describeError(e), busy: null });
    }
  },

  async loadLogs() {
    try {
      const { lines } = await client().call("vscode.logs", {});
      set({ logs: lines });
    } catch {
      // Logs are best effort.
    }
  },

  attach() {
    ensureSubscribed();
    set((s) => ({ viewers: s.viewers + 1 }));
    syncHeartbeat();
    return () => {
      set((s) => ({ viewers: Math.max(0, s.viewers - 1) }));
      syncHeartbeat();
    };
  },

  setWindowOpen(open) {
    set({ windowOpen: open });
    syncHeartbeat();
  },

  applyEvent(event) {
    if (event.kind === "VsCodeStatusChanged") {
      const status = event.payload.status;
      const running = status.state === "running" || status.state === "starting" || status.state === "restarting";
      // Enable/disable from elsewhere (another window, an agent): refresh the list.
      const before = get().status?.config.disabledExtensions.join(",");
      if (get().extensions !== null && before !== undefined && before !== status.config.disabledExtensions.join(",")) {
        void get().loadExtensions();
      }
      set((s) => ({
        status,
        progress: status.state === "installing" ? s.progress : null,
        lastUrl: running ? s.lastUrl : null,
        lastOrigin: running ? s.lastOrigin : null,
        embedUrl: running ? s.embedUrl : null,
      }));
      if (!running && get().windowOpen) void vscodeWindow.close().catch(() => undefined);
      if (status.state === "error") void get().loadLogs();
    } else if (event.kind === "VsCodeProgress") {
      set({ progress: event.payload.progress });
    } else if (event.kind === "VsCodeBridgeSignal") {
      const name = event.payload.signal === "showPalette" ? VSCODE_PALETTE_EVENT : VSCODE_FOCUS_OUT_EVENT;
      if (typeof window !== "undefined") window.dispatchEvent(new Event(name));
    }
  },
}));
