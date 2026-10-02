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

export const APPROVAL_MODES: readonly { id: ApprovalMode; label: string; description: string }[] = [
  { id: "Ask", label: "Ask", description: "Every write, command and git action asks first." },
  { id: "SemiAuto", label: "Semi-auto", description: "Routine work inside the worktree runs; risky actions ask." },
  { id: "FullAuto", label: "Full auto", description: "Everything policy allows runs. Dangerous actions still ask." },
];

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
  const labels: Record<DangerCategory, string> = {
    RecursiveDelete: "Recursive delete",
    ProtectedPathDelete: "Protected path delete",
    DiskDestruction: "Disk destruction",
    BackupDestruction: "Backup destruction",
    ResourceExhaustion: "Fork bomb",
    SecurityToolingDisable: "Disables security tooling",
    LogWipe: "Log wipe",
    CredentialAccess: "Credential access",
    SelfTamper: "Orkestr self-tamper",
    UnsafeAgentMode: "Unsafe agent mode",
    PortExposure: "Port exposure",
    GitDestructive: "Destructive git",
    GitInternals: ".git internals",
    DataDestruction: "Data destruction",
    ProductionChange: "Production change",
    SecretChange: "Secret change",
    PrivilegeEscalation: "Privilege escalation",
    SystemConfig: "System configuration",
    NetworkConfig: "Network configuration",
    RemoteCodeExecution: "Download & execute",
    ContainerDestruction: "Container destruction",
    Obfuscation: "Obfuscated command",
    Unparseable: "Unparseable command",
    OutsideWorktree: "Outside worktree",
  };
  return labels[danger];
}

/** Badge tone of a rule effect. */
export function effectTone(effect: PermissionEffect): Tone {
  return effect === "Allow" ? "success" : effect === "Ask" ? "warning" : "danger";
}

/** Human label of a permission scope. */
export function scopeLabel(scope: PermissionScope, projectName?: (id: string) => string | undefined): string {
  switch (scope.scope) {
    case "Global":
      return "Global";
    case "Project":
      return `Project · ${projectName?.(scope.projectId) ?? shortId(scope.projectId)}`;
    case "Agent":
      return `Agent · ${shortId(scope.agentId)}`;
    case "Task":
      return `Task · ${shortId(scope.taskId)}`;
  }
}

/** Human label of a secret scope. */
export function secretScopeLabel(scope: SecretScope, projectName?: (id: string) => string | undefined): string {
  switch (scope.scope) {
    case "Global":
      return "Global";
    case "Project":
      return projectName?.(scope.projectId) ?? `Project ${shortId(scope.projectId)}`;
    case "Environment":
      return `${projectName?.(scope.projectId) ?? shortId(scope.projectId)} · env ${shortId(scope.environmentId)}`;
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

/** Secret names: `[A-Za-z_][A-Za-z0-9_.-]{0,127}` (mirrors the core). */
export function validateSecretName(name: string): string | null {
  const n = name.trim();
  if (!n) return "Give the secret a name.";
  if (n.length > 128) return "At most 128 characters.";
  if (!/^[A-Za-z_][A-Za-z0-9_.-]*$/.test(n)) return "Use letters, digits, _ . - and start with a letter or _.";
  return null;
}

/** Relative time ("just now", "5 min ago", "3 h ago", date). */
export function relativeTime(iso: string, now: number = Date.now()): string {
  const t = Date.parse(iso);
  if (Number.isNaN(t)) return "";
  const s = Math.max(0, Math.round((now - t) / 1000));
  if (s < 45) return "just now";
  const m = Math.round(s / 60);
  if (m < 60) return `${m} min ago`;
  const h = Math.round(m / 60);
  if (h < 24) return `${h} h ago`;
  return new Date(t).toLocaleDateString();
}

/** Short summary of active emergency flags (empty when nothing is active). */
export function activeFlags(flags: EmergencyFlagsView): string[] {
  const out: string[] = [];
  if (flags.allStopped) out.push("All AI stopped");
  if (flags.fullAutoDisabled) out.push("Full Auto disabled");
  if (flags.networkToolsDisabled) out.push("Network tools disabled");
  if (flags.vaultLocked) out.push("Vault locked");
  return out;
}
