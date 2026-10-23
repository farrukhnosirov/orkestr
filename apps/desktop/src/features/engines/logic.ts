/**
 * Parallel engines (pure): the two subscription engines — Claude Code
 * (Claude subscription) and Codex (ChatGPT subscription) — their status
 * tones for the title bar, the routing modes and agent roles of
 * `EnginesConfig`, and which engine an agent's provider belongs to.
 */
import type { EngineId, EnginesConfig, EnginesMode, EngineStatus, ProviderKind } from "@orkestr/protocol";

/** Title-bar tone of an engine: signed in / limit above 80 % / not installed / not signed in. */
export type EngineTone = "ok" | "warn" | "off" | "error";

/** Limit share (percent) above which the engine is shown amber. */
export const ENGINE_LIMIT_WARN = 80;

export function engineTone(s: EngineStatus | null | undefined): EngineTone {
  if (!s || !s.installed) return "off";
  if (!s.signedIn) return "error";
  if (s.limitPercent != null && s.limitPercent > ENGINE_LIMIT_WARN) return "warn";
  return "ok";
}

export const ENGINE_MODES: readonly EnginesMode[] = ["off", "single", "split", "balanced", "race", "consensus"];
export const ENGINE_IDS: readonly EngineId[] = ["claudeCode", "codex"];

/**
 * Agent roles that may run on Codex (`EnginesConfig.codexRoles`), as the core
 * names them. The Manager is configured separately.
 */
export const ENGINE_ROLES = [
  "Architect",
  "Backend",
  "Frontend",
  "Database",
  "QA",
  "Security",
  "DevOps",
  "UI/UX",
  "Product Manager",
  "Reviewer",
  "Researcher",
] as const;
export type EngineRole = (typeof ENGINE_ROLES)[number];

/** `agents:roles.*` key of an engine role. */
export function roleKey(role: EngineRole): string {
  switch (role) {
    case "QA":
      return "Qa";
    case "UI/UX":
      return "UiUx";
    case "Product Manager":
      return "ProductManager";
    default:
      return role;
  }
}

/** Normalizes a role name for comparisons ("UiUx" = "UI/UX", "Qa" = "QA"). */
export function normRole(role: string): string {
  return role.toLowerCase().replace(/[^a-z]/g, "");
}

/** Adds / removes a role (comparison ignores spelling variants). */
export function toggleRole(roles: readonly string[], role: string): string[] {
  const n = normRole(role);
  return roles.some((r) => normRole(r) === n) ? roles.filter((r) => normRole(r) !== n) : [...roles, role];
}

export function hasRole(roles: readonly string[], role: string): boolean {
  const n = normRole(role);
  return roles.some((r) => normRole(r) === n);
}

/** Claude Code model aliases offered in the engines settings. */
export const CLAUDE_ENGINE_MODELS = ["sonnet", "opus", "haiku"] as const;

/** Which engine an agent runs on, from its provider kind. */
export type AgentEngine = "claudeCode" | "codex" | "api";

export function providerEngine(kind: ProviderKind | null | undefined): AgentEngine | null {
  if (!kind) return null;
  if (kind === "ClaudeCode") return "claudeCode";
  if (kind === "Codex") return "codex";
  return "api";
}

/** Whether two configs differ (Save button state). */
export function configChanged(a: EnginesConfig, b: EnginesConfig): boolean {
  return (
    a.mode !== b.mode ||
    a.manager !== b.manager ||
    a.primary !== b.primary ||
    a.claudeModel !== b.claudeModel ||
    a.codexModel !== b.codexModel ||
    a.crossFallback !== b.crossFallback ||
    [...a.codexRoles].map(normRole).sort().join(",") !== [...b.codexRoles].map(normRole).sort().join(",")
  );
}

/** Which settings a mode uses (others are dimmed). */
export function modeUses(mode: EnginesMode): { primary: boolean; roles: boolean } {
  return { primary: mode === "single", roles: mode === "split" };
}

/**
 * i18n key (`engines:` namespace) for a known backend `EngineStatus.reason`,
 * or `null` to show the raw text (the core reports reasons in English).
 */
export function engineReasonKey(reason: string): string | null {
  if (/^Claude Code is not installed/i.test(reason)) return "reason.claudeNotInstalled";
  return null;
}

/** The reason as shown to the user: localized when known, else the raw text. */
export function engineReasonText(reason: string, t: (key: string) => string): string {
  const key = engineReasonKey(reason);
  return key ? t(key) : reason;
}
