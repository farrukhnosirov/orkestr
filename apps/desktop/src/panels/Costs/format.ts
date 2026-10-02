import type { BudgetLevel, BudgetPeriod, BudgetScope, Project } from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

/** `$1,234.56`, with more precision for small amounts (`$0.0042`). */
export function formatUsd(v: number): string {
  if (!Number.isFinite(v)) return "—";
  const abs = Math.abs(v);
  const digits = abs > 0 && abs < 0.01 ? 4 : 2;
  return v.toLocaleString("en-US", { style: "currency", currency: "USD", minimumFractionDigits: digits, maximumFractionDigits: digits });
}

/** `1.2M`, `34.5k`, `950`. */
export function formatTokens(n: number): string {
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(n >= 10_000_000 ? 0 : 1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(n >= 10_000 ? 0 : 1)}k`;
  return String(n);
}

/** `42%`. */
export function formatPct(fraction: number | null | undefined): string {
  if (fraction == null || !Number.isFinite(fraction)) return "—";
  return `${Math.round(fraction * 100)}%`;
}

export const PERIOD_LABEL: Record<BudgetPeriod, string> = {
  Daily: "per day",
  Monthly: "per month",
  Total: "total",
  PerTask: "per task",
};

export const LEVEL_TONE: Record<BudgetLevel, Tone> = {
  Ok: "success",
  Warn: "warning",
  Exceeded: "danger",
  Unrestricted: "violet",
  Disabled: "neutral",
};

const short = (id: string) => id.slice(-6);

/** Human label of a budget scope. */
export function scopeLabel(scope: BudgetScope, projects: readonly Project[]): string {
  switch (scope.scope) {
    case "Global":
      return "All usage";
    case "Project": {
      const p = projects.find((x) => x.id === scope.projectId);
      return p ? `Project · ${p.name}` : `Project · …${short(scope.projectId)}`;
    }
    case "Task":
      return `Task · …${short(scope.taskId)}`;
    case "Provider":
      return `Provider · …${short(scope.providerId)}`;
    case "Agent":
      return `Agent · …${short(scope.agentId)}`;
    default:
      return "Unknown";
  }
}

/** "Nice" upper bound for a chart axis (1, 2, 2.5, 5 × 10^n). */
export function niceMax(v: number): number {
  if (!Number.isFinite(v) || v <= 0) return 1;
  const exp = Math.floor(Math.log10(v));
  const base = 10 ** exp;
  for (const m of [1, 2, 2.5, 5, 10]) {
    if (v <= m * base) return m * base;
  }
  return 10 * base;
}

/** Short day label from `YYYY-MM-DD` → `Oct 2`. */
export function dayLabel(key: string): string {
  const [y, m, d] = key.split("-").map(Number);
  if (!y || !m || !d) return key;
  return new Date(Date.UTC(y, m - 1, d)).toLocaleDateString("en-US", { month: "short", day: "numeric", timeZone: "UTC" });
}
