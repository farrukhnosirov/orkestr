/**
 * Which robots are on the desktop (ADR-0016): one robot per real agent,
 * spawned with `AgentSpawned`, kept while the agent works, kept for
 * `lingerSeconds` after it finished (celebration + result bubble, Scenario H),
 * at most `maxVisible` at a time — the rest collapse into the cluster.
 *
 * Pure reducer over agent events + a selector; the main window's robot host
 * diffs the selection against open windows.
 */
import type { AgentInstance, AgentState, Event, RobotDesign } from "@orkestr/protocol";

import { statePriority, visualStateFor } from "./states";

export interface RobotSlot {
  agentId: string;
  projectId: string;
  templateId: string | null;
  state: AgentState;
  /** When the run ended (ms epoch), null while active. */
  finishedAt: number | null;
  /** Template wants a floating robot. */
  floating: boolean;
  /** Resolved design (null until `robots.list` answered). */
  design: RobotDesign | null;
}

export type RobotSlots = ReadonlyMap<string, RobotSlot>;

const FINISHED: ReadonlySet<AgentState> = new Set<AgentState>(["Completed", "Stopped", "Error"]);

export function slotFromAgent(agent: AgentInstance, now: number, prev?: RobotSlot): RobotSlot {
  const finished = FINISHED.has(agent.state) || agent.stoppedAt !== null;
  return {
    agentId: agent.id,
    projectId: agent.projectId,
    templateId: agent.templateId,
    state: agent.state,
    finishedAt: finished ? (prev?.finishedAt ?? now) : null,
    floating: prev?.floating ?? true,
    design: prev?.design ?? null,
  };
}

/** An agent with its resolved robot design (`robots.list` entry). */
export interface RobotEntryLike {
  agent: AgentInstance;
  design: RobotDesign;
  floating: boolean;
}

/** Merges `robots.list` entries (fresh agent + resolved design) into the slots. */
export function mergeRobotEntries(slots: RobotSlots, entries: readonly RobotEntryLike[], now: number): RobotSlots {
  if (entries.length === 0) return slots;
  const next = new Map(slots);
  for (const e of entries) {
    const prev = next.get(e.agent.id);
    next.set(e.agent.id, { ...slotFromAgent(e.agent, now, prev), floating: e.floating, design: e.design });
  }
  return next;
}

/** Agent event kinds the robots react to. */
export const ROBOT_AGENT_EVENTS: ReadonlySet<string> = new Set(["AgentSpawned", "AgentUpdated", "AgentStateChanged", "AgentStopped"]);

/** Applies one agent event. Returns the same map when nothing changed. */
export function applyRobotEvent(slots: RobotSlots, ev: Event, now: number): RobotSlots {
  switch (ev.kind) {
    case "AgentSpawned":
    case "AgentUpdated": {
      const agent = ev.payload.agent;
      const prev = slots.get(agent.id);
      // Updates of agents we never saw active (old runs) do not create robots.
      if (!prev && ev.kind === "AgentUpdated" && FINISHED.has(agent.state)) return slots;
      const next = new Map(slots);
      next.set(agent.id, slotFromAgent(agent, now, prev));
      return next;
    }
    case "AgentStateChanged": {
      const prev = slots.get(ev.payload.agentId);
      if (!prev || prev.state === ev.payload.to) return slots;
      const next = new Map(slots);
      const finished = FINISHED.has(ev.payload.to);
      next.set(prev.agentId, { ...prev, state: ev.payload.to, finishedAt: finished ? (prev.finishedAt ?? now) : null });
      return next;
    }
    case "AgentStopped": {
      const prev = slots.get(ev.payload.agentId);
      if (!prev || prev.finishedAt !== null) return slots;
      const next = new Map(slots);
      next.set(prev.agentId, { ...prev, finishedAt: now });
      return next;
    }
    default:
      return slots;
  }
}

export interface VisibilityOptions {
  enabled: boolean;
  maxVisible: number;
  lingerMs: number;
  /** Robots the user hid this session. */
  hidden: ReadonlySet<string>;
}

export interface RobotSelection {
  visible: string[];
  overflow: string[];
}

/** Whether a slot is still eligible for the desktop at `now`. */
export function isAlive(slot: RobotSlot, now: number, lingerMs: number): boolean {
  return slot.finishedAt === null || now - slot.finishedAt < lingerMs;
}

/** Robots to show and robots in the overflow cluster. */
export function selectRobots(slots: RobotSlots, opts: VisibilityOptions, now: number): RobotSelection {
  if (!opts.enabled) return { visible: [], overflow: [] };
  const eligible = [...slots.values()].filter((s) => {
    if (!s.floating || opts.hidden.has(s.agentId) || !isAlive(s, now, opts.lingerMs)) return false;
    if (s.design?.behaviour.autoHideIdle && (s.state === "Idle" || s.state === "Stopped")) return false;
    return true;
  });
  const prio = (s: RobotSlot) => {
    const v = visualStateFor(s.state);
    return statePriority(v.state, v.asleep);
  };
  // Stable order: priority, then agent id (UUID v7 = creation order).
  eligible.sort((a, b) => prio(a) - prio(b) || a.agentId.localeCompare(b.agentId));
  const max = Math.max(1, Math.floor(opts.maxVisible));
  const ids = eligible.map((s) => s.agentId);
  return { visible: ids.slice(0, max), overflow: ids.slice(max) };
}

/** Drops slots whose linger time is over (keeps memory bounded). */
export function pruneSlots(slots: RobotSlots, now: number, lingerMs: number): RobotSlots {
  let changed = false;
  const next = new Map<string, RobotSlot>();
  for (const [id, s] of slots) {
    if (isAlive(s, now, lingerMs)) next.set(id, s);
    else changed = true;
  }
  return changed ? next : slots;
}

/** The earliest time a lingering robot expires (to schedule the next prune). */
export function nextExpiry(slots: RobotSlots, lingerMs: number): number | null {
  let min: number | null = null;
  for (const s of slots.values()) {
    if (s.finishedAt === null) continue;
    const at = s.finishedAt + lingerMs;
    if (min === null || at < min) min = at;
  }
  return min;
}

/** Diff between open windows and the wanted set. */
export function diffWindows(open: ReadonlySet<string>, wanted: readonly string[]): { toOpen: string[]; toClose: string[] } {
  const want = new Set(wanted);
  return {
    toOpen: wanted.filter((id) => !open.has(id)),
    toClose: [...open].filter((id) => !want.has(id)),
  };
}
