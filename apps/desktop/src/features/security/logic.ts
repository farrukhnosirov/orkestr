/**
 * Pure helpers of the security feature (unit-tested in `security.test.ts`).
 */
import type {
  Approval,
  ApprovalMode,
  Capability,
  DangerCategory,
  EmergencyFlagsView,
  PermissionEffect,
  PermissionRuleRecord,
  PermissionScope,
  RiskLevel,
  SecretScope,
} from "@orkestr/protocol";
import type { Tone } from "@orkestr/ui";

import { formatRelativeTime, t } from "../../i18n";

export const CAPABILITIES: readonly Capability[] = [
  "Read",
  "Write",
  "Delete",
  "Terminal",
  "Git",
  "Network",
  "Browser",
  "DbRead",
  "DbWrite",
  "Docker",
  "Ssh",
  "Deploy",
  "ComputerControl",
  "Secrets",
];

export const EFFECTS: readonly PermissionEffect[] = ["Allow", "Ask", "Deny"];

export const APPROVAL_MODE_IDS: readonly ApprovalMode[] = ["Ask", "SemiAuto", "FullAuto"];

/** Localized name of an approval mode («Спрашивать», «Полуавтомат», «Полный автомат»). */
export function approvalModeLabel(mode: ApprovalMode): string {
  return t(`common:status.approvalMode.${mode}`);
}

/** Localized one-line description of an approval mode. */
export function approvalModeDescription(mode: ApprovalMode): string {
  return t(`security:modes.descriptions.${mode}`);
}

/** Localized name of a capability. */
export function capabilityLabel(capability: Capability): string {
  return t(`security:capability.${capability}`);
}

/** Localized name of a rule effect (select options, rule badges). */
export function effectLabel(effect: PermissionEffect): string {
  return t(`security:effect.${effect}`);
}

/** Localized label of a risk level ("High risk"). */
export function riskLabel(risk: RiskLevel): string {
  return t(`security:risk.${risk}`);
}

export const EMPTY_FLAGS: EmergencyFlagsView = {
  allStopped: false,
  fullAutoDisabled: false,
  networkToolsDisabled: false,
  vaultLocked: false,
};

/** Badge tone of a risk level. */
export function riskTone(risk: RiskLevel): Tone {
  switch (risk) {
    case "Low":
      return "success";
    case "Medium":
      return "info";
    case "High":
      return "warning";
    case "Critical":
      return "danger";
  }
}

/** Human label of a dangerous-action category. */
export function dangerLabel(danger: DangerCategory): string {
  return t(`security:danger.${danger}`);
}

/** Badge tone of a rule effect. */
export function effectTone(effect: PermissionEffect): Tone {
  return effect === "Allow" ? "success" : effect === "Ask" ? "warning" : "danger";
}

/** Human label of a permission scope. */
export function scopeLabel(scope: PermissionScope, projectName?: (id: string) => string | undefined): string {
  switch (scope.scope) {
    case "Global":
      return t("security:scope.global");
    case "Project":
      return t("security:scope.project", { name: projectName?.(scope.projectId) ?? shortId(scope.projectId) });
    case "Agent":
      return t("security:scope.agent", { id: shortId(scope.agentId) });
    case "Task":
      return t("security:scope.task", { id: shortId(scope.taskId) });
  }
}

/** Human label of a secret scope. */
export function secretScopeLabel(scope: SecretScope, projectName?: (id: string) => string | undefined): string {
  switch (scope.scope) {
    case "Global":
      return t("security:scope.global");
    case "Project":
      return projectName?.(scope.projectId) ?? t("security:scope.projectId", { id: shortId(scope.projectId) });
    case "Environment":
      return t("security:scope.environment", {
        project: projectName?.(scope.projectId) ?? shortId(scope.projectId),
        env: shortId(scope.environmentId),
      });
  }
}

/** Last 6 characters of an id (UUID v7 prefixes are time-ordered and collide visually). */
export function shortId(id: string): string {
  return id.length > 8 ? `…${id.slice(-6)}` : id;
}

/** Narrowest scope first, then capability, then creation time (mirrors the engine's precedence). */
export function sortRules(rules: readonly PermissionRuleRecord[]): PermissionRuleRecord[] {
  const rank: Record<PermissionScope["scope"], number> = { Task: 0, Agent: 1, Project: 2, Global: 3 };
  return [...rules].sort(
    (a, b) =>
      rank[a.rule.scope.scope] - rank[b.rule.scope.scope] ||
      CAPABILITIES.indexOf(a.rule.capability) - CAPABILITIES.indexOf(b.rule.capability) ||
      a.createdAt.localeCompare(b.createdAt),
  );
}

/** Pending approvals, newest first, after applying an approval event. */
export function upsertApproval(list: readonly Approval[], approval: Approval): Approval[] {
  const rest = list.filter((a) => a.id !== approval.id);
  const next = approval.status === "Pending" ? [approval, ...rest] : rest;
  return next.sort((a, b) => b.requestedAt.localeCompare(a.requestedAt));
}

/** Removes an approval that left the Pending state. */
export function removeApproval(list: readonly Approval[], approvalId: string): Approval[] {
  return list.filter((a) => a.id !== approvalId);
}

const SECRET_NAME_MAX = 128;

/** Secret names: `[A-Za-z_][A-Za-z0-9_.-]{0,127}` (mirrors the core). */
export function validateSecretName(name: string): string | null {
  const n = name.trim();
  if (!n) return t("security:secrets.validation.nameRequired");
  if (n.length > SECRET_NAME_MAX) return t("security:secrets.validation.nameTooLong", { max: SECRET_NAME_MAX });
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(n)) return t("security:secrets.validation.nameInvalid");
  return null;
}

/** Localized relative time («5 минут назад» / "5 minutes ago"); empty for an invalid timestamp. */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const ts = Date.parse(iso);
  if (Number.isNaN(ts)) return "";
  return formatRelativeTime(Math.min(ts, now), now);
}

/** Short summary of active emergency flags (empty when nothing is active). */
export function activeFlags(flags: EmergencyFlagsView): string[] {
  const out: string[] = [];
  if (flags.allStopped) out.push(t("security:flags.allStopped"));
  if (flags.fullAutoDisabled) out.push(t("security:flags.fullAutoDisabled"));
  if (flags.networkToolsDisabled) out.push(t("security:flags.networkToolsDisabled"));
  if (flags.vaultLocked) out.push(t("security:flags.vaultLocked"));
  return out;
}
