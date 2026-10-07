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

/** Display names the client already knows, by id, for the breakdown dimensions. */
export interface BreakdownNames {
  projects: ReadonlyMap<string, string>;
  providers: ReadonlyMap<string, string>;
  agents: ReadonlyMap<string, string>;
  tasks: ReadonlyMap<string, string>;
}

/**
 * Row label of the cost breakdown: the project / provider / agent name or
 * task title when it is known, the model name for the Model dimension, and a
 * short id (`…f45a08a5`) only when the lookup fails (the full id is the
 * row's tooltip).
 */
export function breakdownLabel(
  dimension: "Provider" | "Model" | "Agent" | "Project" | "Task",
  row: { key: string; model: string | null },
  names: BreakdownNames,
): string {
  if (row.key === "none") return t("costs:breakdown.unattributed");
  const fallback = `…${row.key.slice(-8)}`;
  switch (dimension) {
    case "Model":
      return row.model ?? row.key;
    case "Project":
      return names.projects.get(row.key) ?? fallback;
    case "Provider":
      return names.providers.get(row.key) ?? fallback;
    case "Agent":
      return names.agents.get(row.key) ?? fallback;
    case "Task":
      return names.tasks.get(row.key) ?? fallback;
    default:
      return fallback;
  }
}

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

const SCOPE_KINDS = ["global", "project", "task", "provider", "agent"] as const;
const PERIODS: readonly BudgetPeriod[] = ["PerTask", "Daily", "Monthly", "Total"];

/**
 * Parses the scope string of a `BudgetThreshold` event (crates/cost
 * `scope_label`: `project:<id>/Monthly`, `global/Daily`; the agent runtime
 * sends `agent:<id>`). `null` when the shape is unknown.
 */
export function parseBudgetScope(raw: string): { scope: BudgetScope; period: BudgetPeriod | null } | null {
  const m = /^([a-z]+)(?::([^/]+))?(?:\/([A-Za-z]+))?$/.exec(raw.trim());
  if (!m) return null;
  const kind = (m[1] ?? "") as (typeof SCOPE_KINDS)[number];
  const id = m[2] ?? null;
  const periodRaw = m[3] ?? null;
  const period = periodRaw && (PERIODS as readonly string[]).includes(periodRaw) ? (periodRaw as BudgetPeriod) : null;
  if (periodRaw && !period) return null;
  let scope: BudgetScope;
  switch (kind) {
    case "global":
      if (id) return null;
      scope = { scope: "Global" };
      break;
    case "project":
      if (!id) return null;
      scope = { scope: "Project", projectId: id };
      break;
    case "task":
      if (!id) return null;
      scope = { scope: "Task", taskId: id };
      break;
    case "provider":
      if (!id) return null;
      scope = { scope: "Provider", providerId: id };
      break;
    case "agent":
      if (!id) return null;
      scope = { scope: "Agent", agentId: id };
      break;
    default:
      return null;
  }
  return { scope, period };
}

/**
 * Localized label of a `BudgetThreshold` scope string: «Проект · Schoolify ·
 * В месяц» instead of `project:<uuid>/Monthly`. Unknown shapes are shown as is.
 */
export function budgetScopeText(raw: string, projects: readonly Pick<Project, "id" | "name">[]): string {
  const parsed = parseBudgetScope(raw);
  // A bare kind ("project") is still named in the UI language.
  if (!parsed) return t(`logs:enums.budgetScope.${raw.toLowerCase()}` as "logs:enums.budgetScope.project", { defaultValue: raw });
  const scope = scopeLabel(parsed.scope, projects as readonly Project[]);
  return parsed.period ? t("costs:scopeWithPeriod", { scope, period: periodLabel(parsed.period) }) : scope;
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
