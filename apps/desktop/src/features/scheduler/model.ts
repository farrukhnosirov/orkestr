/**
 * Pure helpers of the scheduler UI (§11, §68.24–26): localized wait reasons,
 * provider naming and settings-form validation. The core sends structured
 * `WaitReason`s; the UI renders them in the current language.
 */
import type { SchedEntry, SchedulerSettings, SchedulerStatus, WaitReason } from "@orkestr/protocol";

import { formatBytes, formatDate, formatPercent, t } from "../../i18n";

const MB = 1024 * 1024;

/** Short id for a provider without a display name. */
export function shortId(id: string): string {
  return id.length > 8 ? `…${id.slice(-6)}` : id;
}

/** Display name of a provider from the status (falls back to a short id). */
export function providerName(status: SchedulerStatus | null, providerId: string): string {
  return status?.providers.find((p) => p.providerId === providerId)?.name ?? shortId(providerId);
}

/** Model key `<providerId>/<model>` → model name. */
export function modelName(key: string): string {
  const i = key.indexOf("/");
  return i >= 0 ? key.slice(i + 1) : key;
}

/** Localized text of one wait reason (without the "Waiting:" prefix). */
export function reasonText(reason: WaitReason, status: SchedulerStatus | null = null): string {
  switch (reason.kind) {
    case "globalLimit":
      return t("scheduler:reason.globalLimit", { running: reason.running, limit: reason.limit });
    case "projectLimit":
      return t("scheduler:reason.projectLimit", { running: reason.running, limit: reason.limit });
    case "teamLimit":
      return t("scheduler:reason.teamLimit", { team: reason.team, running: reason.running, limit: reason.limit });
    case "rateLimited":
      return t("scheduler:reason.rateLimited", {
        provider: providerName(status, reason.providerId),
        time: formatDate(reason.until, "time"),
      });
    case "providerConcurrency":
      return t("scheduler:reason.providerConcurrency", {
        running: reason.running,
        limit: reason.limit,
        provider: providerName(status, reason.providerId),
      });
    case "modelConcurrency":
      return t("scheduler:reason.modelConcurrency", {
        running: reason.running,
        limit: reason.limit,
        model: modelName(reason.model),
      });
    case "cpu":
      return t("scheduler:reason.cpu", {
        projected: formatPercent(reason.projectedPercent / 100),
        limit: formatPercent(reason.limitPercent / 100),
      });
    case "memory":
      return t("scheduler:reason.memory", {
        free: formatBytes(reason.freeMb * MB),
        needed: formatBytes(reason.neededMb * MB),
      });
    case "disk":
      return t("scheduler:reason.disk", { free: formatBytes(reason.freeMb * MB), min: formatBytes(reason.minMb * MB) });
    case "gpu":
      return t("scheduler:reason.gpu", {
        usage: formatPercent(reason.usagePercent / 100),
        limit: formatPercent(reason.limitPercent / 100),
      });
    case "fileConflict":
      return t("scheduler:reason.fileConflict", { path: reason.path });
    case "agentBudget":
      return t("scheduler:reason.agentBudget", { spawned: reason.spawned, limit: reason.limit });
    case "paused":
      return t("scheduler:reason.paused");
  }
}

/** "Waiting: …" line of an entry, or null when it is not held back. */
export function entryWaitText(entry: SchedEntry, status: SchedulerStatus | null = null): string | null {
  if (!entry.reason) return null;
  return t("scheduler:reason.waiting", { reason: reasonText(entry.reason, status) });
}

/** Most common wait reason across the queue (for the compact bar). */
export function topReason(queue: readonly SchedEntry[]): WaitReason | null {
  const counts = new Map<string, { n: number; reason: WaitReason }>();
  for (const e of queue) {
    if (!e.reason) continue;
    const key = e.reason.kind;
    const cur = counts.get(key);
    counts.set(key, { n: (cur?.n ?? 0) + 1, reason: cur?.reason ?? e.reason });
  }
  let best: { n: number; reason: WaitReason } | null = null;
  for (const v of counts.values()) if (!best || v.n > best.n) best = v;
  return best?.reason ?? null;
}

/** Providers currently in a rate-limit back-off. */
export function rateLimitedProviders(status: SchedulerStatus | null, now = Date.now()) {
  return (status?.providers ?? []).filter((p) => p.rateLimitedUntil && Date.parse(p.rateLimitedUntil) > now);
}

/** Editable form of the settings (strings so inputs can be empty mid-edit). */
export interface LimitsForm {
  maxParallelGlobal: string;
  maxParallelPerProject: string;
  projectCap: string;
  maxCpuPercent: string;
  minFreeRamMb: string;
  minFreeDiskMb: string;
  maxGpuPercent: string;
  agingMinutes: string;
  rateLimitBackoffSecs: string;
  maxAutoResumeAttempts: string;
  fairShare: boolean;
  mergeRiskChecks: boolean;
  autoResume: SchedulerSettings["autoResume"];
  providerCaps: Record<string, string>;
}

export function toForm(s: SchedulerSettings, projectId: string | null): LimitsForm {
  const projectCap = projectId ? s.projectCaps.find((c) => c.projectId === projectId)?.max : undefined;
  const providerCaps: Record<string, string> = {};
  for (const c of s.providerCaps) providerCaps[c.providerId] = String(c.max);
  return {
    maxParallelGlobal: String(s.maxParallelGlobal),
    maxParallelPerProject: String(s.maxParallelPerProject),
    projectCap: projectCap === undefined ? "" : String(projectCap),
    maxCpuPercent: String(Math.round(s.maxCpuPercent)),
    minFreeRamMb: String(s.minFreeRamMb),
    minFreeDiskMb: String(s.minFreeDiskMb),
    maxGpuPercent: s.maxGpuPercent === null ? "" : String(Math.round(s.maxGpuPercent)),
    agingMinutes: String(s.agingMinutes),
    rateLimitBackoffSecs: String(s.rateLimitBackoffSecs),
    maxAutoResumeAttempts: String(s.maxAutoResumeAttempts),
    fairShare: s.fairShare,
    mergeRiskChecks: s.mergeRiskChecks,
    autoResume: s.autoResume,
    providerCaps,
  };
}

function int(v: string, min: number, max: number): number | null {
  if (!/^\d+$/.test(v.trim())) return null;
  const n = Number.parseInt(v, 10);
  return n >= min && n <= max ? n : null;
}

/** Form → settings (keeps fields the form does not edit). Null when invalid. */
export function fromForm(base: SchedulerSettings, f: LimitsForm, projectId: string | null): SchedulerSettings | null {
  const global = int(f.maxParallelGlobal, 1, 256);
  const perProject = int(f.maxParallelPerProject, 1, 256);
  const cpu = int(f.maxCpuPercent, 5, 100);
  const ram = int(f.minFreeRamMb, 0, 1_048_576);
  const disk = int(f.minFreeDiskMb, 0, 10_485_760);
  const aging = int(f.agingMinutes, 0, 1440);
  const backoff = int(f.rateLimitBackoffSecs, 1, 3600);
  const attempts = int(f.maxAutoResumeAttempts, 0, 20);
  const gpu = f.maxGpuPercent.trim() === "" ? null : int(f.maxGpuPercent, 5, 100);
  const projectCap = f.projectCap.trim() === "" ? null : int(f.projectCap, 1, 256);
  if (
    global === null ||
    perProject === null ||
    cpu === null ||
    ram === null ||
    disk === null ||
    aging === null ||
    backoff === null ||
    attempts === null ||
    (f.maxGpuPercent.trim() !== "" && gpu === null) ||
    (f.projectCap.trim() !== "" && projectCap === null)
  ) {
    return null;
  }
  const providerCaps = [];
  for (const [providerId, v] of Object.entries(f.providerCaps)) {
    if (v.trim() === "") continue;
    const max = int(v, 1, 256);
    if (max === null) return null;
    providerCaps.push({ providerId, max });
  }
  const projectCaps = base.projectCaps.filter((c) => c.projectId !== projectId);
  if (projectId && projectCap !== null) projectCaps.push({ projectId, max: projectCap });
  return {
    ...base,
    maxParallelGlobal: global,
    maxParallelPerProject: perProject,
    projectCaps,
    providerCaps,
    maxCpuPercent: cpu,
    minFreeRamMb: ram,
    minFreeDiskMb: disk,
    maxGpuPercent: gpu,
    agingMinutes: aging,
    rateLimitBackoffSecs: backoff,
    maxAutoResumeAttempts: attempts,
    fairShare: f.fairShare,
    mergeRiskChecks: f.mergeRiskChecks,
    autoResume: f.autoResume,
  };
}
