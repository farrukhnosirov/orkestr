/**
 * Pure helpers of the Teams tab (unit-tested): display names of the built-in
 * teams, editor drafts and their validation (mirrors the core's bounds in
 * `orchestrator::teams`), member summaries and the hierarchy tree
 * (Manager → Team Lead → Executors → Reviewer) of a team's lead runs.
 */
import type {
  AgentRole,
  AgentTemplate,
  Event,
  TeamKind,
  TeamLeadRun,
  TeamLeadStatus,
  TeamMember,
  TeamPolicies,
  TeamView,
  TaskStatus,
} from "@orkestr/protocol";

import { t } from "../../i18n";

/** Bounds (same numbers as `orkestr_protocol::methods::teams`). */
export const TEAM_LIMITS = {
  nameLen: 80,
  descriptionLen: 2000,
  members: 16,
  memberCount: 8,
  fanOut: 12,
  parallel: 8,
  attempts: 5,
  timeLimitMins: 1440,
} as const;

export const DEFAULT_POLICIES: TeamPolicies = {
  hierarchy: false,
  maxFanOut: 4,
  maxParallel: 2,
  maxAttempts: 2,
  requireReview: true,
  approvalMode: null,
  budgetUsd: null,
  timeLimitMins: 60,
};

type BuiltinKind = Exclude<TeamKind, { Custom: string }>;
const BUILTIN_NAMES: Record<BuiltinKind, string> = {
  Erp: "ERP Team",
  Web: "Web Team",
  Mobile: "Mobile Team",
  Security: "Security Team",
  RapidPrototype: "Rapid Prototype Team",
};
const BUILTIN_KEYS: Record<BuiltinKind, "erp" | "web" | "mobile" | "security" | "rapid"> = {
  Erp: "erp",
  Web: "web",
  Mobile: "mobile",
  Security: "security",
  RapidPrototype: "rapid",
};

function builtinKind(kind: TeamKind): BuiltinKind | null {
  return typeof kind === "string" ? kind : null;
}

/** Display name: the English names of the seeded teams are translated; user names stay as typed. */
export function teamName(team: { name: string; kind: TeamKind; builtin: boolean }): string {
  const k = builtinKind(team.kind);
  if (team.builtin && k && BUILTIN_NAMES[k] === team.name) return t(`teams:builtin.${BUILTIN_KEYS[k]}.name`);
  return team.name;
}

/** Description: only an unchanged built-in description is translated. */
export function teamDescription(team: { name: string; kind: TeamKind; builtin: boolean; description: string }): string {
  const k = builtinKind(team.kind);
  if (team.builtin && k && BUILTIN_NAMES[k] === team.name) {
    const key = `teams:builtin.${BUILTIN_KEYS[k]}.description` as const;
    if (team.description.trim() === t(key, { lng: "en" })) return t(key);
  }
  return team.description;
}

/** Editor draft of a team (create or edit). */
export interface TeamDraft {
  name: string;
  description: string;
  members: TeamMember[];
  policies: TeamPolicies;
  /** Budget field as typed (empty = none). */
  budgetText: string;
}

export function blankTeamDraft(templates: readonly AgentTemplate[]): TeamDraft {
  const first = templates[0];
  return {
    name: "",
    description: "",
    members: first ? [{ templateId: first.id, isLead: false, count: 1, model: null }] : [],
    policies: { ...DEFAULT_POLICIES },
    budgetText: "",
  };
}

export function draftOfTeam(team: TeamView["team"]): TeamDraft {
  return {
    name: team.name,
    description: team.description,
    members: team.members.map((m) => ({ ...m })),
    policies: { ...DEFAULT_POLICIES, ...team.policies },
    budgetText: team.policies.budgetUsd == null ? "" : String(team.policies.budgetUsd),
  };
}

const inRange = (n: number, lo: number, hi: number) => Number.isInteger(n) && n >= lo && n <= hi;

/**
 * Validates a draft; returns the localized problem or `null`. `roles`
 * resolves a member's role (from its template) to check the hierarchy.
 */
export function validateTeamDraft(d: TeamDraft, roleOf: (templateId: string) => AgentRole | null): string | null {
  const name = d.name.trim();
  if (!name || name.length > TEAM_LIMITS.nameLen) return t("teams:validation.name", { max: TEAM_LIMITS.nameLen });
  if (d.description.length > TEAM_LIMITS.descriptionLen) return t("teams:validation.description", { max: TEAM_LIMITS.descriptionLen });
  if (d.members.length === 0 || d.members.length > TEAM_LIMITS.members) return t("teams:validation.members", { max: TEAM_LIMITS.members });
  if (d.members.some((m) => !inRange(m.count, 1, TEAM_LIMITS.memberCount))) return t("teams:validation.count", { max: TEAM_LIMITS.memberCount });
  const leads = d.members.filter((m) => m.isLead);
  if (leads.length > 1) return t("teams:validation.oneLead");
  if (leads.some((m) => m.count !== 1)) return t("teams:validation.leadCount");
  const p = d.policies;
  if (!inRange(p.maxFanOut, 1, TEAM_LIMITS.fanOut)) return t("teams:validation.fanOut", { max: TEAM_LIMITS.fanOut });
  if (!inRange(p.maxParallel, 1, TEAM_LIMITS.parallel)) return t("teams:validation.parallel", { max: TEAM_LIMITS.parallel });
  if (!inRange(p.maxAttempts, 1, TEAM_LIMITS.attempts)) return t("teams:validation.attempts", { max: TEAM_LIMITS.attempts });
  if (!inRange(p.timeLimitMins, 1, TEAM_LIMITS.timeLimitMins)) return t("teams:validation.timeLimit", { max: TEAM_LIMITS.timeLimitMins });
  if (parseBudget(d.budgetText) === undefined) return t("teams:validation.budget");
  if (p.hierarchy) {
    const executors = d.members.filter((m) => !m.isLead && roleOf(m.templateId) !== "Reviewer");
    if (leads.length === 0 || executors.length === 0) return t("teams:validation.hierarchy");
  }
  return null;
}

/** Budget text → number, `null` for empty, `undefined` when invalid. */
export function parseBudget(text: string): number | null | undefined {
  const v = text.trim().replace(",", ".");
  if (!v) return null;
  const n = Number(v);
  return Number.isFinite(n) && n >= 0 ? n : undefined;
}

/** Policies to send (budget parsed). Call after `validateTeamDraft`. */
export function policiesOf(d: TeamDraft): TeamPolicies {
  return { ...d.policies, budgetUsd: parseBudget(d.budgetText) ?? null };
}

/** One avatar per member slot (lead first), with the slot's role. */
export interface MemberAvatar {
  key: string;
  role: AgentRole;
  name: string;
  count: number;
  isLead: boolean;
  hasModel: boolean;
}

export function memberAvatars(members: readonly TeamMember[], templates: readonly AgentTemplate[]): MemberAvatar[] {
  const out = members.map((m, i) => {
    const tpl = templates.find((x) => x.id === m.templateId);
    return {
      key: `${m.templateId}-${i}`,
      role: tpl?.role ?? ({ Custom: "?" } as AgentRole),
      name: tpl?.name ?? "?",
      count: m.count,
      isLead: m.isLead,
      hasModel: m.model != null,
    };
  });
  return out.sort((a, b) => Number(b.isLead) - Number(a.isLead));
}

/** Total number of agents a team can run (sum of slot counts). */
export function headcount(members: readonly TeamMember[]): number {
  return members.reduce((n, m) => n + m.count, 0);
}

export function isAssigned(view: TeamView, projectId: string): boolean {
  return view.assignments.some((a) => a.projectId === projectId);
}

/** Node of the hierarchy tree view. */
export interface TreeNode {
  id: string;
  level: "manager" | "lead" | "executor" | "reviewer";
  title: string;
  role: AgentRole | null;
  status: TaskStatus | TeamLeadStatus | null;
  attempts: number;
  agentId: string | null;
  note: string | null;
  children: TreeNode[];
}

/** Manager → lead runs → executors (and the reviewer) of one team. */
export function hierarchyTree(view: TeamView, leadRole: AgentRole | null, runs: readonly TeamLeadRun[] = view.leadRuns): TreeNode {
  return {
    id: "manager",
    level: "manager",
    title: t("teams:tree.manager"),
    role: "Manager",
    status: null,
    attempts: 0,
    agentId: null,
    note: null,
    children: runs.map((run) => ({
      id: run.id,
      level: "lead",
      title: run.title,
      role: leadRole,
      status: run.status,
      attempts: 0,
      agentId: run.leadAgentId ?? null,
      note: run.statusReason ?? null,
      children: run.subtasks.map((s) => ({
        id: s.taskId,
        level: s.isReview ? "reviewer" : "executor",
        title: s.title,
        role: s.role,
        status: s.status,
        attempts: s.attempts,
        agentId: s.agentId ?? null,
        note: s.note ?? null,
        children: [],
      })),
    })),
  };
}

export const LEAD_STATUS_TONE: Record<TeamLeadStatus, "info" | "violet" | "success" | "danger" | "neutral" | "accent"> = {
  Planning: "violet",
  Executing: "accent",
  Reviewing: "info",
  Completed: "success",
  Failed: "danger",
  Cancelled: "neutral",
};

export function isLeadActive(status: TeamLeadStatus): boolean {
  return status === "Planning" || status === "Executing" || status === "Reviewing";
}

/** Applies a team event to the list. Returns the same array when nothing changed. */
export function applyTeamEvent(list: TeamView[], ev: Event): TeamView[] {
  switch (ev.kind) {
    case "TeamUpdated": {
      const view = ev.payload.team;
      const i = list.findIndex((v) => v.team.id === view.team.id);
      if (i < 0) return [...list, view];
      const next = [...list];
      next[i] = view;
      return next;
    }
    case "TeamDeleted":
      return list.some((v) => v.team.id === ev.payload.teamId) ? list.filter((v) => v.team.id !== ev.payload.teamId) : list;
    case "TeamLeadRunUpdated": {
      const lead = ev.payload.lead;
      const i = list.findIndex((v) => v.team.id === lead.teamId);
      const cur = list[i];
      if (!cur) return list;
      const runs = cur.leadRuns.some((r) => r.id === lead.id)
        ? cur.leadRuns.map((r) => (r.id === lead.id ? lead : r))
        : [lead, ...cur.leadRuns].slice(0, 10);
      const next = [...list];
      next[i] = { ...cur, leadRuns: runs };
      return next;
    }
    default:
      return list;
  }
}
