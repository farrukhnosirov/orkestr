import type { BudgetLevel, BudgetPeriod, BudgetScope, Project } from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

import { formatCurrency, formatNumber, formatPercent, intlLocale, t } from "../../i18n";

/**
 * USD in the UI locale: `$1,234.56` / `1 234,56 $`, with more precision for
 * small amounts (`$0.0042`).
 */
export function formatUsd(v: number, lng?: string): string {
  if (!Number.isFinite(v)) return "—";
  return formatCurrency(v, "USD", lng);
}

/** Compact token counts: `1.2M`, `34.5K`, `950` / `1,2 млн`, `34,5 тыс.`. */
export function formatTokens(n: number, lng?: string): string {
  if (n < 1_000) return formatNumber(n, { maximumFractionDigits: 0 }, lng);
  const digits = n >= 10_000 && n < 1_000_000 ? 0 : n >= 10_000_000 ? 0 : 1;
  return formatNumber(n, { notation: "compact", maximumFractionDigits: digits }, lng);
}

/** `42%` / `42 %`. */
export function formatPct(fraction: number | null | undefined, lng?: string): string {
  if (fraction == null || !Number.isFinite(fraction)) return "—";
  return formatPercent(fraction, 0, lng);
}

/** Localized period name for selects (`Per month` / «В месяц»). */
export function periodLabel(period: BudgetPeriod): string {
  return t(`costs:period.${period}`);
}

/** Localized limit with its period (`$100.00 per month` / «100,00 $ в месяц»). */
export function limitLabel(amountUsd: number, period: BudgetPeriod): string {
  return t(`costs:budgets.limit.${period}`, { amount: formatUsd(amountUsd) });
}

/** Localized budget level (`Warning` / «Предупреждение»). */
export function levelLabel(level: BudgetLevel): string {
  return t(`costs:level.${level}`);
}

export const LEVEL_TONE: Record<BudgetLevel, Tone> = {
  Ok: "success",
  Warn: "warning",
  Exceeded: "danger",
  Unrestricted: "violet",
  Disabled: "neutral",
};

const short = (id: string) => id.slice(-6);

/** Human label of a budget scope, in the current UI language. */
export function scopeLabel(scope: BudgetScope, projects: readonly Project[]): string {
  switch (scope.scope) {
    case "Global":
      return t("costs:scopeLabel.Global");
    case "Project": {
      const p = projects.find((x) => x.id === scope.projectId);
      return p ? t("costs:scopeLabel.Project", { name: p.name }) : t("costs:scopeLabel.ProjectId", { id: short(scope.projectId) });
    }
    case "Task":
      return t("costs:scopeLabel.Task", { id: short(scope.taskId) });
    case "Provider":
      return t("costs:scopeLabel.Provider", { id: short(scope.providerId) });
    case "Agent":
      return t("costs:scopeLabel.Agent", { id: short(scope.agentId) });
    default:
      return t("common:state.unknown");
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

/** Short day label from `YYYY-MM-DD` → `Oct 2` / «2 окт.». */
export function dayLabel(key: string, lng?: string): string {
  const [y, m, d] = key.split("-").map(Number);
  if (!y || !m || !d) return key;
  return new Intl.DateTimeFormat(intlLocale(lng), { month: "short", day: "numeric", timeZone: "UTC" }).format(new Date(Date.UTC(y, m - 1, d)));
}
