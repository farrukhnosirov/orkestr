/**
 * App updates (ADR-0048) — thin wrapper over the shell's `updates_*`
 * commands. Updates concern the installed desktop binary (like window
 * management), so they go through Tauri rather than the core RPC (§67).
 * Outside the desktop shell every call resolves to `null` / no-op.
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

import { isMainWindow } from "../../lib/tauri";

/** Event with the full status (src-tauri `updates::UPDATES_STATUS_EVENT`). */
export const UPDATES_STATUS_EVENT = "orkestr://updates/status";

export type UpdateChannel = "stable" | "beta";
export type UpdatePhase = "idle" | "checking" | "upToDate" | "available" | "downloading" | "ready" | "installing" | "error";
export type SourceKind = "remote" | "localManifest" | "localInstaller";
export type BackupReason = "preUpdate" | "preMigration" | "manual";

export interface UpdateSettings {
  remoteEnabled: boolean;
  endpoint: string;
  channel: UpdateChannel;
  checkOnStartup: boolean;
  checkIntervalHours: number;
  autoDownload: boolean;
  localPath: string | null;
  backupRetention: number;
}

export interface UpdateInfo {
  version: string;
  currentVersion: string;
  notes: string | null;
  notesI18n: Record<string, string>;
  pubDate: string | null;
  platformKey: string;
  location: { kind: "url" | "file"; value: string };
  /** `null` only for an unsigned local installer. */
  signature: string | null;
  source: SourceKind;
  fileName: string;
  size: number | null;
  prerelease: boolean;
}

export interface BackupInfo {
  path: string;
  reason: BackupReason;
  createdAt: string;
  fromVersion: string;
  toVersion: string | null;
  databases: string[];
  files: number;
  sizeBytes: number;
}

export interface ErrorInfo {
  code: string;
  message: string;
}

export type StartKind =
  | { kind: "firstRun" }
  | { kind: "same" }
  | { kind: "upgraded"; from: string }
  | { kind: "downgraded"; from: string };

export interface StartupReport {
  version: string;
  start: StartKind;
  previous: { version: string; previousVersion: string | null; since: string } | null;
  schema: { state: "noDatabase" } | { state: "compatible"; pending: number } | { state: "newer"; unknown: string[] };
  backup: BackupInfo | null;
  backupError: string | null;
}

export interface UpdateStatus {
  currentVersion: string;
  phase: UpdatePhase;
  settings: UpdateSettings;
  available: UpdateInfo | null;
  progress: { downloaded: number; total: number | null } | null;
  lastCheckedAt: string | null;
  latestSeen: string | null;
  lastError: ErrorInfo | null;
  installOnExit: boolean;
  startup: StartupReport | null;
  backupsDir: string;
  backups: BackupInfo[];
}

/** Command errors are `{code, message}` JSON; anything else becomes `{code: "unknown"}`. */
export function parseUpdateError(e: unknown): ErrorInfo {
  const raw = e instanceof Error ? e.message : String(e);
  try {
    const parsed = JSON.parse(raw) as Partial<ErrorInfo>;
    if (typeof parsed.code === "string") return { code: parsed.code, message: parsed.message ?? "" };
  } catch {
    // not JSON
  }
  return { code: "unknown", message: raw };
}

/** Only the main window drives updates (the commands are granted to it alone). */
export function updatesAvailableHere(): boolean {
  return isMainWindow();
}

export const updatesApi = {
  status: (): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_status"),
  setSettings: (settings: UpdateSettings): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_set_settings", { settings }),
  check: (localPath?: string | null): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_check", { localPath: localPath ?? null }),
  download: (allowUnsigned = false): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_download", { allowUnsigned }),
  cancel: (): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_cancel"),
  install: (when: "now" | "onExit" | "cancel"): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_install", { when }),
  backupNow: (): Promise<UpdateStatus> => invoke<UpdateStatus>("updates_backup_now"),
  onStatus: (handler: (s: UpdateStatus) => void): Promise<() => void> =>
    listen<UpdateStatus>(UPDATES_STATUS_EVENT, (e) => handler(e.payload)),
};
