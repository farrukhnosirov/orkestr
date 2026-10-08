/**
 * VS Code extension launcher: the commands of every extension installed in
 * Orkestr's VS Code (`vscode.commands`), run through the Orkestr bridge
 * extension (`vscode.execute` / `vscode.openView`). When no VS Code window is
 * connected, the window is opened on the active project first and the call
 * waits for the bridge. Dangerous-looking commands come back as
 * `approval_required`; the confirmation dialog re-sends them with
 * `confirmed: true`. Pins live in the core's VS Code config.
 */
import { OrkestrError } from "@orkestr/client";
import type { VsCodeCommandInfo, VsCodeExtensionCommands } from "@orkestr/protocol";
import { create } from "zustand";

import { maybeClient } from "../../core/client";
import { currentLanguage, t } from "../../i18n";
import { describeError } from "../../i18n/errors";
import { useAppStore } from "../../store/app";
import { openProjectInVsCode } from "./quickAccess";
import { useVsCodeStore } from "./store";

/** How long a call waits for a freshly opened VS Code window to connect. */
export const BRIDGE_WAIT_MS = 60_000;
/** Client-side timeout of a bridge call (wait + run). */
export const EXECUTE_TIMEOUT_MS = 180_000;
/** Commands pinned at most (mirrors the core limit). */
export const MAX_PINS = 24;

/** One request to run something through the bridge. */
export type LaunchRequest =
  | { kind: "command"; commandId: string; title: string; args?: unknown[] }
  | { kind: "view"; extensionId: string; title: string; containerId?: string }
  | { kind: "settings"; extensionId: string; title: string };

export interface LaunchOutcome {
  title: string;
  ok: boolean;
  message: string | null;
  at: number;
}

export interface LauncherStore {
  extensions: VsCodeExtensionCommands[] | null;
  loading: boolean;
  error: string | null;
  /** Key (`command id` / `view:<ext>` / `settings:<ext>`) currently running. */
  running: string | null;
  /** Waiting for the VS Code window to open and connect. */
  opening: boolean;
  /** A dangerous command waiting for the user's confirmation. */
  confirm: LaunchRequest | null;
  last: LaunchOutcome | null;
  load(force?: boolean): Promise<void>;
  launch(req: LaunchRequest, confirmed?: boolean): Promise<boolean>;
  resolveConfirm(accept: boolean): void;
  togglePin(commandId: string): Promise<void>;
  clearLast(): void;
}

function requestKey(req: LaunchRequest): string {
  return req.kind === "command" ? req.commandId : `${req.kind}:${req.extensionId}`;
}

let loadedFor: string | null = null;

/** All commands of all extensions, flattened with their extension. */
export function flattenCommands(
  extensions: readonly VsCodeExtensionCommands[],
): Array<{ ext: VsCodeExtensionCommands; cmd: VsCodeCommandInfo }> {
  return extensions.flatMap((ext) => ext.commands.map((cmd) => ({ ext, cmd })));
}

/** Case-insensitive match of a command against a search query (title, category, id, extension name). */
export function commandMatches(ext: VsCodeExtensionCommands, cmd: VsCodeCommandInfo, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (!q) return true;
  const hay = `${cmd.title} ${cmd.category ?? ""} ${cmd.id} ${ext.displayName} ${ext.id}`.toLowerCase();
  return q.split(/\s+/).every((part) => hay.includes(part));
}

/** Title shown on a command button (`Category: Title`, like VS Code's palette). */
export function commandLabel(cmd: VsCodeCommandInfo): string {
  return cmd.category ? `${cmd.category}: ${cmd.title}` : cmd.title;
}

/** Finds a command by id in the loaded catalog. */
export function findCommand(
  extensions: readonly VsCodeExtensionCommands[] | null,
  commandId: string,
): { ext: VsCodeExtensionCommands; cmd: VsCodeCommandInfo } | null {
  for (const ext of extensions ?? []) {
    const cmd = ext.commands.find((c) => c.id === commandId);
    if (cmd) return { ext, cmd };
  }
  return null;
}

/** Opens the VS Code window on the active project when no window is connected. */
async function ensureWindow(): Promise<boolean> {
  const vs = useVsCodeStore.getState();
  if (!vs.status) await vs.load();
  const status = useVsCodeStore.getState().status;
  if (status?.bridgeConnected) return false;
  if (!status?.installed) {
    await openProjectInVsCode(null);
    throw new Error(t("vscodeLauncher:errors.notInstalled"));
  }
  await openProjectInVsCode(useAppStore.getState().activeProjectId);
  const failed = useVsCodeStore.getState().error;
  if (failed) throw new Error(failed);
  return true;
}

export const useLauncherStore = create<LauncherStore>((set, get) => ({
  extensions: null,
  loading: false,
  error: null,
  running: null,
  opening: false,
  confirm: null,
  last: null,

  async load(force = false) {
    const client = maybeClient();
    if (!client) return;
    const locale = currentLanguage();
    if (!force && get().extensions && loadedFor === locale) return;
    set({ loading: true, error: null });
    try {
      const res = await client.call("vscode.commands", { locale }, { timeoutMs: 30_000 });
      loadedFor = locale;
      set({ extensions: res.extensions, loading: false });
    } catch (e) {
      set({ error: describeError(e), loading: false });
    }
  },

  async launch(req, confirmed = false) {
    const client = maybeClient();
    if (!client) return false;
    const key = requestKey(req);
    set({ running: key, error: null });
    try {
      const opened = await ensureWindow();
      if (opened) set({ opening: true });
      const waitMs = BRIDGE_WAIT_MS;
      const opts = { timeoutMs: EXECUTE_TIMEOUT_MS };
      if (req.kind === "command") {
        const params = { commandId: req.commandId, waitMs, ...(req.args ? { args: req.args } : {}), ...(confirmed ? { confirmed } : {}) };
        await client.call("vscode.execute", params, opts);
      } else {
        await client.call(
          "vscode.openView",
          {
            extensionId: req.extensionId,
            waitMs,
            ...(req.kind === "settings" ? { settings: true } : {}),
            ...(req.kind === "view" && req.containerId ? { containerId: req.containerId } : {}),
          },
          opts,
        );
      }
      set({ running: null, opening: false, last: { title: req.title, ok: true, message: null, at: Date.now() } });
      return true;
    } catch (e) {
      if (e instanceof OrkestrError && e.code === "approval_required" && !confirmed) {
        set({ running: null, opening: false, confirm: req });
        return false;
      }
      const message =
        e instanceof OrkestrError && e.code === "conflict" ? t("vscodeLauncher:errors.noWindow") : describeError(e);
      set({ running: null, opening: false, last: { title: req.title, ok: false, message, at: Date.now() } });
      return false;
    }
  },

  resolveConfirm(accept) {
    const req = get().confirm;
    set({ confirm: null });
    if (accept && req) void get().launch(req, true);
  },

  async togglePin(commandId) {
    const status = useVsCodeStore.getState().status;
    const pins = status?.config.pinnedCommands ?? [];
    const next = pins.includes(commandId) ? pins.filter((p) => p !== commandId) : [...pins, commandId].slice(-MAX_PINS);
    await useVsCodeStore.getState().setConfig({ pinnedCommands: next });
  },

  clearLast() {
    set({ last: null });
  },
}));
