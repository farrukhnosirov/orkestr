import type { AgentRole, ModelBenchmarkRow } from "@orkestr/protocol";


import { roleLabel } from "../Agents/model";

/** Sortable columns of the model benchmark table. */
export type ModelSortKey =
  | "model"
  | "executions"
  | "firstPass"
  | "build"
  | "defects"
  | "retries"
  | "latency"
  | "tokens"
  | "cost"
  | "total"
  | "score";

export type SortDir = "asc" | "desc";

/** Value of a column; `null` (no data) always sorts last. */
export function sortValue(r: ModelBenchmarkRow, key: ModelSortKey): number | string | null {
  switch (key) {
    case "model":
      return r.model.toLowerCase();
    case "executions":
      return r.executions;
    case "firstPass":
      return r.firstPassSuccessRate;
    case "build":
      return r.buildSuccessRate;
    case "defects":
      return r.defectsFoundAvg;
    case "retries":
      return r.retriesAvg;
    case "latency":
      return r.latencyAvgMs;
    case "tokens":
      return r.tokensAvg;
    case "cost":
      return r.costAvgUsd;
    case "total":
      return r.costTotalUsd;
    case "score":
      return r.routingScore;
  }
}

/** Stable sort by a column; rows without data go last in either direction. */
export function sortModelRows(rows: readonly ModelBenchmarkRow[], key: ModelSortKey, dir: SortDir): ModelBenchmarkRow[] {
  const sign = dir === "asc" ? 1 : -1;
  return rows
    .map((r, i) => ({ r, i }))
    .sort((a, b) => {
      const va = sortValue(a.r, key);
      const vb = sortValue(b.r, key);
      if (va === null && vb === null) return a.i - b.i;
      if (va === null) return 1;
      if (vb === null) return -1;
      const c = typeof va === "string" && typeof vb === "string" ? va.localeCompare(vb) : Number(va) - Number(vb);
      return c === 0 ? a.i - b.i : c * sign;
    })
    .map((x) => x.r);
}

/** Lower is better for these columns (default ascending sort). */
export const LOWER_IS_BETTER: ReadonlySet<ModelSortKey> = new Set<ModelSortKey>(["model", "defects", "retries", "latency", "tokens", "cost", "total"]);

const ROLE_KEYS: Record<string, AgentRole> = {
  "UI/UX": "UiUx",
  QA: "Qa",
  "Product Manager": "ProductManager",
};

/** Localized label of a benchmark role (the core stores the router purpose, e.g. `UI/UX`). */
export function benchmarkRoleLabel(role: string): string {
  const key = ROLE_KEYS[role] ?? (role as AgentRole);
  return roleLabel(key);
}

/** Bar width (0..1) of `value` relative to the max of `values`. */
export function barFraction(value: number | null, values: readonly (number | null)[]): number {
  if (value === null || !Number.isFinite(value) || value <= 0) return 0;
  const max = Math.max(0, ...values.filter((v): v is number => v !== null && Number.isFinite(v)));
  return max > 0 ? Math.min(1, value / max) : 0;
}

