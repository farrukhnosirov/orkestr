/**
 * AI source priority (ADR-0051): pure helpers for the «Приоритет ИИ-источников»
 * settings section — status chips and list edits.
 */
import type { AiSource, ProviderView, RoutingConfig, SourceCapabilities } from "@orkestr/protocol";

export type SourceStatusKind = "ok" | "limit" | "auth" | "offline" | "cooldown" | "disabled" | "unknown";

export interface SourceStatus {
  kind: SourceStatusKind;
  /** ISO time the limit / cooldown ends. */
  until?: string;
}

const future = (iso: string | null | undefined, now: number): string | undefined =>
  iso && Date.parse(iso) > now ? iso : undefined;

/** Status chip of a source: works / limited until HH:MM / sign-in error / offline / cooling down. */
export function sourceStatus(view: ProviderView | undefined, source: AiSource, now = Date.now()): SourceStatus {
  if (!view) return { kind: "unknown" };
  if (!source.enabled || !view.provider.enabled) return { kind: "disabled" };
  const s = view.status;
  if (s.authState === "Invalid" || s.authState === "Unconfigured") return { kind: "auth" };
  const limit = future(s.quotaExhaustedUntil, now) ?? future(s.rateLimitedUntil, now);
  if (limit) return { kind: "limit", until: limit };
  const cool = future(s.cooldownUntil, now);
  if (s.health === "Down") return { kind: "offline", until: cool };
  if (cool) return { kind: "cooldown", until: cool };
  if (s.health === "Healthy" || s.health === "Degraded" || s.health === "RateLimited") return { kind: "ok" };
  return { kind: "unknown" };
}

/** Moves `from` to `to` (drag and drop / arrow buttons). */
export function reorder<T>(list: readonly T[], from: number, to: number): T[] {
  if (from === to || from < 0 || to < 0 || from >= list.length || to >= list.length) return [...list];
  const out = [...list];
  const [item] = out.splice(from, 1);
  out.splice(to, 0, item as T);
  return out;
}

export const CAPABILITIES: readonly (keyof SourceCapabilities)[] = ["chat", "tools", "vision", "computerUse", "embeddings"];

/** Toggles one capability of a source. */
export function toggleCapability(source: AiSource, cap: keyof SourceCapabilities): AiSource {
  const caps: SourceCapabilities = source.capabilities ?? {
    chat: true,
    tools: true,
    vision: true,
    computerUse: false,
    embeddings: false,
  };
  return { ...source, capabilities: { ...caps, [cap]: !caps[cap] } };
}

/** Writes the edited order / overrides into a routing config. */
export function withSources(
  config: RoutingConfig,
  sources: AiSource[],
  overrides: Record<string, string[] | undefined>,
  failover: boolean,
): RoutingConfig {
  const purposeSources = Object.entries(overrides)
    .filter((e): e is [string, string[]] => Array.isArray(e[1]))
    .map(([purpose, list]) => ({ purpose, sources: sources.map((s) => s.providerId).filter((id) => list.includes(id)) }));
  return { ...config, sourcePriority: sources, purposeSources, failover };
}
