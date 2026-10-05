/**
 * Pure logic of the HUD / Ambient window (§34): which agents make the strip,
 * the run status line, which approval may be approved with one click, and
 * the remembered window position. Unit-tested in `hud.test.ts`.
 */
import type { AgentInstance, AgentState, Approval, ManagerRun } from "@orkestr/protocol";

/** Agent states that count as "working". */
const ACTIVE: ReadonlySet<AgentState> = new Set(["Listening", "Thinking", "Coding", "Testing", "Reviewing", "Waiting"]);
/** Order in the strip: busiest first. */
const RANK: Record<AgentState, number> = {
  Coding: 0,
  Testing: 1,
  Reviewing: 2,
  Thinking: 3,
  Listening: 4,
  Waiting: 5,
  Error: 6,
  Idle: 7,
  Completed: 8,
  Stopped: 9,
};

export const HUD_STRIP_MAX = 6;

export function isWorking(a: Pick<AgentInstance, "state">): boolean {
  return ACTIVE.has(a.state);
}

/** Agents shown in the strip (working ones + errors), busiest first, and how many did not fit. */
export function agentStrip(agents: readonly AgentInstance[], max = HUD_STRIP_MAX): { shown: AgentInstance[]; more: number; working: number } {
  const relevant = agents.filter((a) => isWorking(a) || a.state === "Error");
  const sorted = [...relevant].sort((a, b) => RANK[a.state] - RANK[b.state] || a.name.localeCompare(b.name));
  return { shown: sorted.slice(0, max), more: Math.max(0, sorted.length - max), working: agents.filter(isWorking).length };
}

export type RunLine =
  | { kind: "idle" }
  | { kind: "run"; status: ManagerRun["status"]; done: number; total: number; goal: string };

const LIVE_STATUSES: ReadonlySet<ManagerRun["status"]> = new Set(["Running", "Paused"]);

/** The status line: the live run if any, else the most recently updated one (≤ 30 min old). */
export function runLine(runs: readonly ManagerRun[], now = Date.now()): RunLine {
  if (runs.length === 0) return { kind: "idle" };
  const byTime = [...runs].sort((a, b) => Date.parse(b.updatedAt) - Date.parse(a.updatedAt));
  const live = byTime.find((r) => LIVE_STATUSES.has(r.status));
  const pick = live ?? byTime[0];
  if (!pick) return { kind: "idle" };
  if (!live && now - Date.parse(pick.updatedAt) > 30 * 60_000) return { kind: "idle" };
  const total = pick.tasks.length;
  const done = pick.tasks.filter((t) => t.status === "Completed").length;
  return { kind: "run", status: pick.status, done, total, goal: pick.goal };
}

/** Risk levels that may be approved with one click from the HUD. */
export function isQuickApprovable(a: Pick<Approval, "risk" | "danger">): boolean {
  return (a.risk === "Low" || a.risk === "Medium") && a.danger === null;
}

/** The oldest pending approval (the one the HUD offers). */
export function nextApproval(pending: readonly Approval[]): Approval | null {
  const open = pending.filter((a) => a.status === "Pending");
  return [...open].sort((a, b) => Date.parse(a.requestedAt) - Date.parse(b.requestedAt))[0] ?? null;
}

// ---------------------------------------------------------------------------
// Position memory (per device)
// ---------------------------------------------------------------------------

export interface HudPosition {
  x: number;
  y: number;
}

export const HUD_POSITION_KEY = "orkestr.hud.position.v1";

/** Parses a stored position; `null` for anything unexpected. */
export function parsePosition(raw: string | null): HudPosition | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw) as unknown;
    if (typeof v !== "object" || v === null) return null;
    const { x, y } = v as { x?: unknown; y?: unknown };
    if (typeof x !== "number" || typeof y !== "number" || !Number.isFinite(x) || !Number.isFinite(y)) return null;
    if (Math.abs(x) > 100_000 || Math.abs(y) > 100_000) return null;
    return { x: Math.round(x), y: Math.round(y) };
  } catch {
    return null;
  }
}

export function serializePosition(p: HudPosition): string {
  return JSON.stringify({ x: Math.round(p.x), y: Math.round(p.y) });
}
