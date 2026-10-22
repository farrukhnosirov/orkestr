/**
 * Deployments panel helpers (ADR-0041): adapter ids of targets, status tones,
 * KEY=VALUE parsing, adapter settings schema fields.
 */
import type { AdapterAvailability, AdapterInfo, DeploymentStatus, DeploymentTarget, EnvironmentKind } from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

export const KNOWN_ADAPTERS = ["staticSite", "localProcess", "docker", "vercel", "netlify", "fly", "kubernetes"] as const;
export type KnownAdapter = (typeof KNOWN_ADAPTERS)[number];

export const KINDS = ["Local", "Dev", "Test", "Staging", "Production"] as const;
export type KindKey = (typeof KINDS)[number] | "Custom";

/** Deployments still in flight. */
export const ACTIVE: ReadonlySet<DeploymentStatus> = new Set(["Pending", "AwaitingApproval", "Running"]);

export function isActive(s: DeploymentStatus): boolean {
  return ACTIVE.has(s);
}

export function isKnownAdapter(id: string): id is KnownAdapter {
  return (KNOWN_ADAPTERS as readonly string[]).includes(id);
}

/** Wire-stable key of a target (`"StaticSite"`, `"Plugin:acme/x"`). */
export function targetKey(t: DeploymentTarget): string {
  if (typeof t === "string") return t;
  if ("Plugin" in t) return `Plugin:${t.Plugin}`;
  return `CustomScript:${t.CustomScript}`;
}

/** The adapter serving `target`, if registered. */
export function adapterFor(adapters: readonly AdapterInfo[], target: DeploymentTarget | undefined): AdapterInfo | undefined {
  if (!target) return undefined;
  const key = targetKey(target);
  return adapters.find((a) => targetKey(a.target) === key);
}

export function kindKey(k: EnvironmentKind): KindKey {
  return typeof k === "string" ? k : "Custom";
}

export function statusTone(s: DeploymentStatus): Tone {
  switch (s) {
    case "Succeeded":
      return "success";
    case "Failed":
      return "danger";
    case "AwaitingApproval":
      return "warning";
    case "Running":
    case "Pending":
      return "info";
    case "RolledBack":
      return "violet";
    default:
      return "neutral";
  }
}

export function availabilityTone(a: AdapterAvailability): Tone {
  return a.state === "Ready" ? "success" : a.state === "NeedsCredential" ? "warning" : "neutral";
}

/** `KEY=VALUE` lines → record (blank lines and `#` comments skipped). */
export function parseKv(text: string): Record<string, string> {
  const out: Record<string, string> = {};
  for (const raw of text.split(/\r?\n/)) {
    const line = raw.trim();
    if (!line || line.startsWith("#")) continue;
    const i = line.indexOf("=");
    if (i <= 0) continue;
    out[line.slice(0, i).trim()] = line.slice(i + 1).trim();
  }
  return out;
}

export function formatKv(rec: Record<string, string> | undefined): string {
  return Object.entries(rec ?? {})
    .map(([k, v]) => `${k}=${v}`)
    .join("\n");
}

export function lines(text: string): string[] {
  return text
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);
}

export interface SettingField {
  key: string;
  description: string | null;
  defaultValue: string | null;
  required: boolean;
  options: string[] | null;
}

/** Fields of an adapter `settingsSchema` (JSON schema object with string properties). */
export function schemaFields(schema: unknown): SettingField[] {
  if (!schema || typeof schema !== "object") return [];
  const s = schema as { properties?: Record<string, { description?: unknown; default?: unknown; enum?: unknown }>; required?: unknown };
  const required = Array.isArray(s.required) ? (s.required as unknown[]).filter((r): r is string => typeof r === "string") : [];
  return Object.entries(s.properties ?? {}).map(([key, p]) => ({
    key,
    description: typeof p.description === "string" ? p.description : null,
    defaultValue: typeof p.default === "string" ? p.default : null,
    required: required.includes(key),
    options: Array.isArray(p.enum) ? (p.enum as unknown[]).map(String) : null,
  }));
}

/** Drops empty values (the core treats absent settings as defaults). */
export function compactSettings(rec: Record<string, string>): Record<string, string> {
  return Object.fromEntries(Object.entries(rec).filter(([, v]) => v.trim() !== ""));
}

export function shortTime(iso: string | null | undefined, lng: string): string {
  if (!iso) return "";
  const d = new Date(iso);
  return Number.isNaN(d.getTime()) ? "" : d.toLocaleString(lng, { dateStyle: "short", timeStyle: "medium" });
}
