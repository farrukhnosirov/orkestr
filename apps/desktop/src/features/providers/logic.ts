/**
 * Pure helpers for the provider UI (unit-tested in logic.test.ts).
 */
import type { ModelRef, ProviderKind, ProviderKindInfo, ProviderStatus, ProviderView } from "@orkestr/protocol";
import type { Status } from "@orkestr/ui";
import i18n from "i18next";

import { formatCurrency, formatDuration, formatNumber, intlLocale, t } from "../../i18n";

/** Status dot + short label for a provider. */
export function statusOf(view: Pick<ProviderView, "status" | "provider">): { dot: Status; label: string; pulse: boolean } {
  const s: ProviderStatus = view.status;
  if (!view.provider.enabled) return { dot: "idle", label: t("providers:status.disabled"), pulse: false };
  if (s.circuitOpen) return { dot: "error", label: t("providers:status.circuitOpen"), pulse: false };
  if (s.authState === "Unconfigured") {
    const notInstalled = s.authDetail?.startsWith("not installed");
    return { dot: "idle", label: notInstalled ? t("providers:status.notInstalled") : t("providers:status.notConfigured"), pulse: false };
  }
  if (s.authState === "Invalid") return { dot: "error", label: t("providers:status.authProblem"), pulse: false };
  switch (s.health) {
    case "Healthy":
      return {
        dot: "online",
        label: s.authState === "Authenticated" ? t("providers:status.connected") : t("providers:status.reachable"),
        pulse: false,
      };
    case "RateLimited":
      return { dot: "warning", label: t("providers:status.rateLimited"), pulse: true };
    case "Degraded":
      return {
        dot: "warning",
        label: s.quotaExhaustedUntil ? t("providers:status.quotaExhausted") : t("providers:status.degraded"),
        pulse: false,
      };
    case "Down":
      return { dot: "error", label: t("providers:status.unreachable"), pulse: false };
    default:
      return { dot: "connecting", label: t("providers:status.checking"), pulse: true };
  }
}

/** Localized latency (`320 ms` / `320 мс`, `1.4 sec` / `1,4 с`) or `—`. */
export function formatLatency(ms: number | null | undefined, lng?: string): string {
  if (ms == null) return "—";
  return formatDuration(ms, lng);
}

/** Localized USD amount (`$0.0042` / `0,0042 $`); estimated values are labelled by callers. */
export function formatUsd(v: number | null | undefined, lng?: string): string {
  if (v == null) return "—";
  return formatCurrency(v, "USD", lng);
}

/** Per-1M token price label, e.g. `$3 / $15 per 1M tokens (est.)` / `3 $ / 15 $ за 1 млн токенов (оценка)`. */
export function formatPricing(p: { inputPerMtokUsd: number; outputPerMtokUsd: number } | null, lng?: string): string {
  const opts = lng ? { lng } : {};
  if (!p) return t("providers:models.noPrice", opts);
  const price = (n: number) =>
    formatNumber(n, { style: "currency", currency: "USD", currencyDisplay: "narrowSymbol", minimumFractionDigits: 0, maximumFractionDigits: 3 }, lng);
  return t("providers:models.pricing", { ...opts, input: price(p.inputPerMtokUsd), output: price(p.outputPerMtokUsd) });
}

/** Context window label (`128K`, `1M` / `128 тыс.`, `1 млн`). */
export function formatContext(tokens: number, lng?: string): string {
  return new Intl.NumberFormat(intlLocale(lng), { notation: "compact", maximumFractionDigits: 1 }).format(tokens);
}

/** Localized name of a provider kind; brand names come from the core as is. */
export function kindName(info: Pick<ProviderKindInfo, "kind" | "displayName">): string {
  const key = `providers:kinds.${info.kind}.name`;
  return i18n.exists(key) ? t(key as "providers:kinds.Mock.name") : info.displayName;
}

/**
 * Display name of a provider instance: the core names new instances after
 * their kind in English ("Mock (offline demo)"); such default names are
 * shown localized, names the user chose are shown as is.
 */
export function instanceName(p: { kind: string; name: string }): string {
  const key = `providers:kinds.${p.kind}.name`;
  if (!i18n.exists(key)) return p.name;
  return p.name === t(key as "providers:kinds.Mock.name", { lng: "en" }) ? t(key as "providers:kinds.Mock.name") : p.name;
}

/** Localized description / sign-in note / login instructions of a kind, falling back to the core's text. */
export function kindText(
  kind: ProviderKind,
  field: "description" | "signInNote" | "loginInstructions",
  fallback: string | null,
): string | null {
  if (fallback == null) return null;
  const key = `providers:kinds.${kind}.${field}`;
  return i18n.exists(key) ? t(key as "providers:kinds.Mock.description") : fallback;
}

/**
 * Localized auth detail. The core sends a short English detail; known shapes
 * are translated, anything else (a reason from the provider) is shown as is.
 */
export function authDetailText(status: Pick<ProviderStatus, "authDetail" | "authState">): string {
  const d = status.authDetail;
  if (!d) return t(`providers:auth.state.${status.authState}`);
  let m: RegExpMatchArray | null;
  if (d === "API key") return t("providers:auth.apiKey");
  if (d === "no credentials needed (offline demo)") return t("providers:auth.noCredentials");
  if (d === "no API key (endpoint without auth)") return t("providers:auth.noApiKey");
  if (d === "not configured") return t("providers:auth.notConfigured");
  if ((m = /^local endpoint \(Ollama (.*)\)$/.exec(d))) return t("providers:auth.localEndpoint", { version: m[1] ?? "" });
  if ((m = /^cli session \(managed by (.*)\)$/.exec(d))) return t("providers:auth.cliSession", { cli: m[1] ?? "" });
  if ((m = /^not installed \(`(.*)` not found on PATH\)$/.exec(d))) return t("providers:auth.notInstalled", { binary: m[1] ?? "" });
  if ((m = /^invalid: (.*)$/s.exec(d))) return t("providers:auth.invalid", { reason: m[1] ?? "" });
  if ((m = /^unreachable: (.*)$/s.exec(d))) return t("providers:auth.unreachable", { reason: m[1] ?? "" });
  return d;
}

/** Localized label of a routing purpose; custom purposes are shown as typed. */
export function purposeLabel(purpose: string): string {
  const custom = /^Custom (\d+)$/.exec(purpose);
  if (custom) return t("providers:purposes.custom", { index: Number(custom[1]) });
  const key = `providers:purposes.${purpose}`;
  return i18n.exists(key) ? t(key as "providers:purposes.Manager") : purpose;
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
    if (!/^[A-Za-z0-9_-]+$/.test(name)) {
      return { error: t("providers:validation.headerName", { name: name || t("providers:validation.emptyName") }) };
    }
    if (SECRET_HEADERS.has(name.toLowerCase())) {
      return { error: t("providers:validation.credentialHeader", { name }) };
    }
    if (/[\r\n]/.test(row.value)) return { error: t("providers:validation.headerLineBreak", { name }) };
    headers[name] = row.value;
  }
  return { headers };
}

/** Validates an http(s) base URL. */
export function validateBaseUrl(url: string): string | null {
  const u = url.trim();
  if (!u) return null;
  if (!/^https?:\/\/[^/\s]+/.test(u) || /\s/.test(u)) return t("providers:validation.baseUrl");
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
  return `${p ? instanceName(p.provider) : t("providers:unknownProvider")} · ${ref.model}`;
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
  if (c.model && !c.model.model.trim()) return t("providers:validation.managerModel");
  if (!Number.isInteger(c.maxParallelAgents) || c.maxParallelAgents < 1 || c.maxParallelAgents > 64) {
    return t("providers:validation.parallelAgents", { min: 1, max: 64 });
  }
  if (c.budgetUsd != null && (!Number.isFinite(c.budgetUsd) || c.budgetUsd < 0)) return t("providers:validation.budget");
  if (c.timeLimitMins != null && (!Number.isInteger(c.timeLimitMins) || c.timeLimitMins < 1 || c.timeLimitMins > 1440)) {
    return t("providers:validation.timeLimit");
  }
  if (c.fallbackChain.some((r) => !r.model.trim())) return t("providers:validation.fallbackModel");
  if (c.fallbackChain.length > 16) return t("providers:validation.fallbackLimit", { max: 16 });
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
