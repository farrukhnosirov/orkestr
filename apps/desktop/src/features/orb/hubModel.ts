/**
 * The Agent Hub's pure model (ADR-0045): Orkestr agents and Claude Code
 * sessions as one list of hub items with a normalized status, the actions
 * each allows, grouping by project and the counts the sphere shows.
 */
import type { AgentInstance, Approval, ClaudeSessionInfo, Project } from "@orkestr/protocol";

import { PAUSED_ACTIVITY } from "../../lib/coreText";

export type HubStatus = "waiting" | "error" | "working" | "paused" | "idle" | "done" | "stopped";

export type HubAction = "message" | "pause" | "resume" | "stop" | "restart" | "transcript";

export interface HubItem {
  kind: "agent" | "claude";
  /** Agent id or Claude Code session id. */
  id: string;
  projectId: string;
  /** Raw name (agent names are localized by the view). */
  name: string;
  status: HubStatus;
  /** Short current activity (raw core text), if any. */
  activity: string | null;
  /** Progress 0..1 (turns used of the turn limit), when known. */
  progress: number | null;
  /** Last activity (ISO), for ordering and the "recent" window. */
  updatedAt: string | null;
  /** Whether the item is still running (not finished / stopped). */
  live: boolean;
  actions: HubAction[];
  /** Template the agent was spawned from (restart). */
  templateId: string | null;
  /** Task the agent works on (restart). */
  taskId: string | null;
  /** The CLI's conversation id of a Claude Code session (opening it). */
  claudeSessionId: string | null;
}

const WORKING_STATES = new Set(["Listening", "Thinking", "Coding", "Testing", "Reviewing"]);

/** Hub status of an Orkestr agent. */
export function agentStatus(a: Pick<AgentInstance, "state" | "stoppedAt" | "activity">): HubStatus {
  if (a.state === "Error") return "error";
  if (a.state === "Completed") return "done";
  if (a.state === "Stopped" || a.stoppedAt) return "stopped";
  if (a.activity === PAUSED_ACTIVITY) return "paused";
  if (a.state === "Waiting") return "waiting";
  if (WORKING_STATES.has(a.state)) return "working";
  return "idle";
}

export function agentItem(a: AgentInstance): HubItem {
  const status = agentStatus(a);
  const live = status !== "done" && status !== "stopped" && status !== "error";
  const actions: HubAction[] = ["message"];
  if (status === "paused") actions.push("resume");
  else if (live) actions.push("pause");
  if (live) actions.push("stop");
  else if (a.templateId) actions.push("restart");
  actions.push("transcript");
  return {
    kind: "agent",
    id: a.id,
    projectId: a.projectId,
    name: a.name,
    status,
    activity: a.activity,
    progress: a.maxTurns > 0 ? Math.min(1, Math.max(0, a.turnsUsed / a.maxTurns)) : null,
    updatedAt: a.lastActivityAt ?? a.stoppedAt ?? a.startedAt ?? a.createdAt,
    live,
    actions,
    templateId: a.templateId,
    taskId: a.taskId,
    claudeSessionId: null,
  };
}

/** Hub status of a Claude Code session. */
export function claudeStatus(state: ClaudeSessionInfo["state"]): HubStatus {
  switch (state) {
    case "running":
      return "working";
    case "waitingPermission":
      return "waiting";
    case "exited":
      return "done";
    case "failed":
      return "error";
    default:
      return "idle";
  }
}

export function claudeItem(s: ClaudeSessionInfo): HubItem {
  const status = claudeStatus(s.state);
  const live = status !== "done" && status !== "error";
  const actions: HubAction[] = live ? ["message", "stop", "transcript"] : ["transcript"];
  return {
    kind: "claude",
    id: s.sessionId,
    projectId: s.projectId,
    name: s.title?.trim() || "Claude Code",
    status,
    activity: s.error,
    progress: null,
    updatedAt: s.startedAt,
    live,
    actions,
    templateId: null,
    taskId: null,
    claudeSessionId: s.claudeSessionId,
  };
}

/** Finished items stay in the hub this long. */
export const RECENT_MS = 30 * 60_000;

/** Live items plus the ones that finished recently. */
export function visibleItems(items: readonly HubItem[], now: number, recentMs = RECENT_MS): HubItem[] {
  return items.filter((i) => {
    if (i.live) return true;
    const at = i.updatedAt ? Date.parse(i.updatedAt) : NaN;
    return Number.isFinite(at) && now - at <= recentMs;
  });
}

const STATUS_ORDER: Record<HubStatus, number> = { waiting: 0, error: 1, working: 2, paused: 3, idle: 4, done: 5, stopped: 6 };

export function compareItems(a: HubItem, b: HubItem): number {
  return STATUS_ORDER[a.status] - STATUS_ORDER[b.status] || (b.updatedAt ?? "").localeCompare(a.updatedAt ?? "") || a.id.localeCompare(b.id);
}

export interface HubGroup {
  projectId: string;
  name: string;
  color: string | null;
  items: HubItem[];
  /** Items of this project that need the user (waiting / error). */
  attention: number;
  working: number;
}

/**
 * Items grouped by project: projects that need attention first, then busy
 * ones, then by name; unknown projects keep their id as name.
 */
export function groupByProject(items: readonly HubItem[], projects: readonly Pick<Project, "id" | "name" | "color">[]): HubGroup[] {
  const byId = new Map(projects.map((p) => [p.id, p]));
  const groups = new Map<string, HubGroup>();
  for (const item of items) {
    let g = groups.get(item.projectId);
    if (!g) {
      const p = byId.get(item.projectId);
      g = { projectId: item.projectId, name: p?.name ?? item.projectId, color: p?.color ?? null, items: [], attention: 0, working: 0 };
      groups.set(item.projectId, g);
    }
    g.items.push(item);
    if (item.status === "waiting" || item.status === "error") g.attention += 1;
    if (item.status === "working") g.working += 1;
  }
  const out = [...groups.values()];
  for (const g of out) g.items.sort(compareItems);
  return out.sort((a, b) => b.attention - a.attention || b.working - a.working || a.name.localeCompare(b.name));
}

export interface HubCounts {
  working: number;
  waiting: number;
  errors: number;
}

export function hubCounts(items: readonly HubItem[], acknowledged: ReadonlySet<string> = new Set()): HubCounts {
  let working = 0;
  let waiting = 0;
  let errors = 0;
  for (const i of items) {
    if (i.status === "working") working += 1;
    else if (i.status === "waiting") waiting += 1;
    else if (i.status === "error" && !acknowledged.has(i.id)) errors += 1;
  }
  return { working, waiting, errors };
}

/** Approvals of one project (for the inline approve / deny under its group). */
export function approvalsFor(pending: readonly Approval[], projectId: string): Approval[] {
  return pending.filter((a) => a.projectId === projectId);
}

/** Text sent to the Manager on behalf of an agent (the core has no per-agent inbox; the Manager relays). */
export function managerRelayText(agentName: string, agentId: string, text: string): string {
  return `[Message for agent "${agentName}" (${agentId})] ${text.trim()}`;
}

/** Moves the keyboard selection through a flat list of row ids. */
export function moveSelection(ids: readonly string[], current: string | null, delta: number): string | null {
  if (ids.length === 0) return null;
  const i = current ? ids.indexOf(current) : -1;
  if (i < 0) return delta >= 0 ? (ids[0] ?? null) : (ids[ids.length - 1] ?? null);
  const next = Math.min(ids.length - 1, Math.max(0, i + delta));
  return ids[next] ?? null;
}
