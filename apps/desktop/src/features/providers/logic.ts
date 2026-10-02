/**
 * Pure helpers for the provider UI (unit-tested in logic.test.ts).
 */
import type { ModelRef, ProviderKind, ProviderKindInfo, ProviderStatus, ProviderView } from "@orkestr/protocol";
import type { Status } from "@orkestr/ui";

/** Status dot + short label for a provider. */
export function statusOf(view: Pick<ProviderView, "status" | "provider">): { dot: Status; label: string; pulse: boolean } {
  const s: ProviderStatus = view.status;
  if (!view.provider.enabled) return { dot: "idle", label: "Disabled", pulse: false };
  if (s.circuitOpen) return { dot: "error", label: "Circuit open", pulse: false };
  if (s.authState === "Unconfigured") {
    return { dot: "idle", label: s.authDetail?.startsWith("not installed") ? "Not installed" : "Not configured", pulse: false };
  }
  if (s.authState === "Invalid") return { dot: "error", label: "Auth problem", pulse: false };
  switch (s.health) {
    case "Healthy":
      return { dot: "online", label: s.authState === "Authenticated" ? "Connected" : "Reachable", pulse: false };
    case "RateLimited":
      return { dot: "warning", label: "Rate limited", pulse: true };
    case "Degraded":
      return { dot: "warning", label: s.quotaExhaustedUntil ? "Quota exhausted" : "Degraded", pulse: false };
    case "Down":
      return { dot: "error", label: "Unreachable", pulse: false };
    default:
      return { dot: "connecting", label: "Checking…", pulse: true };
  }
}

/** `123 ms`, `1.4 s` or `—`. */
export function formatLatency(ms: number | null | undefined): string {
  if (ms == null) return "—";
  if (ms < 1000) return `${Math.round(ms)} ms`;
  return `${(ms / 1000).toFixed(1)} s`;
}

/** `$0.0042` style USD (estimated values are labelled by callers). */
export function formatUsd(v: number | null | undefined): string {
  if (v == null) return "—";
  if (v === 0) return "$0";
  if (v < 0.01) return `$${v.toFixed(4)}`;
  return `$${v.toFixed(2)}`;
}

/** Per-1M token price label, e.g. `$3 / $15 per 1M (est.)`. */
export function formatPricing(p: { inputPerMtokUsd: number; outputPerMtokUsd: number } | null): string {
  if (!p) return "no list price";
  const f = (n: number) => (Number.isInteger(n) ? `$${n}` : `$${n.toFixed(n < 1 ? 3 : 2).replace(/0+$/, "")}`);
  return `${f(p.inputPerMtokUsd)} / ${f(p.outputPerMtokUsd)} per 1M (est.)`;
}

/** Context window label (`128K`, `1M`). */
export function formatContext(tokens: number): string {
  if (tokens >= 1_000_000) return `${+(tokens / 1_000_000).toFixed(1)}M`;
  if (tokens >= 1000) return `${Math.round(tokens / 1000)}K`;
  return String(tokens);
}

/** Moves an item in a list (fallback chain editor). Out-of-range moves are no-ops. */
export function moveItem<T>(items: readonly T[], from: number, to: number): T[] {
  const out = [...items];
  if (from < 0 || from >= out.length || to < 0 || to >= out.length || from === to) return out;
  const [item] = out.splice(from, 1);
  if (item !== undefined) out.splice(to, 0, item);
  return out;
}

/** Stable key for a model ref. */
export function refKey(ref: ModelRef): string {
  return `${ref.providerId}::${ref.model}`;
}

/** Parses a `providerId::model` key back into a ref. */
export function parseRefKey(key: string): ModelRef | null {
  const i = key.indexOf("::");
  if (i <= 0 || i === key.length - 2) return null;
  return { providerId: key.slice(0, i), model: key.slice(i + 2) };
}

/** Header names that carry credentials (must use the API key field instead). */
const SECRET_HEADERS = new Set(["authorization", "proxy-authorization", "x-api-key", "api-key", "x-goog-api-key", "cookie"]);

export interface HeaderRow {
  name: string;
  value: string;
}

/** Validates header rows; returns a record or an error message. */
export function headersFromRows(rows: readonly HeaderRow[]): { headers: Record<string, string> } | { error: string } {
  const headers: Record<string, string> = {};
  for (const row of rows) {
    const name = row.name.trim();
    if (!name && !row.value.trim()) continue;
    if (!/^[A-Za-z0-9_-]+$/.test(name)) return { error: `Invalid header name “${name || "(empty)"}”.` };
    if (SECRET_HEADERS.has(name.toLowerCase())) {
      return { error: `“${name}” carries a credential — use the API key field (stored in the secret vault).` };
    }
    if (/[\r\n]/.test(row.value)) return { error: `Header “${name}” contains a line break.` };
    headers[name] = row.value;
  }
  return { headers };
}

/** Validates an http(s) base URL. */
export function validateBaseUrl(url: string): string | null {
  const u = url.trim();
  if (!u) return null;
  if (!/^https?:\/\/[^/\s]+/.test(u) || /\s/.test(u)) return "Use a full http:// or https:// URL.";
  return null;
}

/** Kind cards in UI order with their configured instances. */
export function groupByKind(
  kinds: readonly ProviderKindInfo[],
  providers: readonly ProviderView[],
): { info: ProviderKindInfo; instances: ProviderView[] }[] {
  return kinds.map((info) => ({ info, instances: providers.filter((p) => p.provider.kind === info.kind) }));
}

/** Human label for a model ref given the known providers. */
export function refLabel(ref: ModelRef, providers: readonly ProviderView[]): string {
  const p = providers.find((v) => v.provider.id === ref.providerId);
  return `${p ? p.provider.name : "Unknown provider"} · ${ref.model}`;
}

/** Whether the kind is an official CLI agent. */
export function isCliKind(kind: ProviderKind): boolean {
  return kind === "ClaudeCode" || kind === "Codex";
}

/** Client-side checks mirroring `providers.setManager` validation. */
export function validateManagerConfig(c: {
  model: ModelRef | null;
  maxParallelAgents: number;
  budgetUsd: number | null;
  timeLimitMins: number | null;
  fallbackChain: readonly ModelRef[];
}): string | null {
  if (c.model && !c.model.model.trim()) return "Choose a model for the Manager.";
  if (!Number.isInteger(c.maxParallelAgents) || c.maxParallelAgents < 1 || c.maxParallelAgents > 64) {
    return "Max parallel agents must be a whole number between 1 and 64.";
  }
  if (c.budgetUsd != null && (!Number.isFinite(c.budgetUsd) || c.budgetUsd < 0)) return "Budget must be a positive amount.";
  if (c.timeLimitMins != null && (!Number.isInteger(c.timeLimitMins) || c.timeLimitMins < 1 || c.timeLimitMins > 1440)) {
    return "Time limit must be between 1 and 1440 minutes.";
  }
  if (c.fallbackChain.some((r) => !r.model.trim())) return "Every fallback entry needs a model.";
  if (c.fallbackChain.length > 16) return "The fallback chain is limited to 16 models.";
  return null;
}

/** Parses an optional numeric input (`""` → null, invalid → NaN). */
export function parseOptionalNumber(text: string): number | null {
  const t = text.trim();
  if (!t) return null;
  return Number(t);
}

/** Default routing purposes offered in the routing table. */
export const ROUTING_PURPOSES = [
  "Manager",
  "Architect",
  "Backend",
  "Frontend",
  "Database",
  "QA",
  "Security",
  "DevOps",
  "Reviewer",
  "Classification",
  "Summarization",
] as const;
