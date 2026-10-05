/**
 * Spoken status summaries (§63 G: "Jarvis, what's happening with
 * Schoolify?" → state lookup → a concise answer). Facts are computed from
 * Phase 1 data (tasks, agents, approvals); Phase 2 quality data (builds,
 * tests, reviews, merges) plugs in through `QualitySnapshot`. Pure.
 */
import type { AgentInstance, AgentState, Task } from "@orkestr/protocol";

/**
 * Phase 2 quality data for a project (latest build / test run / reviews).
 * INTEGRATOR(P2): fill from the quality pipeline (`quality.*` / builds RPC)
 * once Phase 2 is merged; until then only `BuildFailed` events from the
 * event log feed `lastBuild`.
 */
export interface QualitySnapshot {
  lastBuild: { ok: boolean; at: string; summary: string } | null;
  lastTests: { passed: number; failed: number; at: string } | null;
  openReviews: number | null;
}

export const EMPTY_QUALITY: QualitySnapshot = { lastBuild: null, lastTests: null, openReviews: null };

export interface ProjectFacts {
  running: number;
  waiting: number;
  blocked: number;
  failed: number;
  completedToday: number;
  total: number;
  activeAgents: number;
  /** The busiest agent's name + activity, for colour. */
  focus: { agent: string; activity: string | null; state: AgentState } | null;
  approvals: number;
  quality: QualitySnapshot;
}

const ACTIVE: ReadonlySet<AgentState> = new Set<AgentState>(["Listening", "Thinking", "Coding", "Testing", "Reviewing", "Waiting"]);

function sameLocalDay(iso: string | null, now: Date): boolean {
  if (!iso) return false;
  const d = new Date(iso);
  return d.getFullYear() === now.getFullYear() && d.getMonth() === now.getMonth() && d.getDate() === now.getDate();
}

export function projectFacts(
  tasks: readonly Task[],
  agents: readonly AgentInstance[],
  approvals: number,
  now: Date,
  quality: QualitySnapshot = EMPTY_QUALITY,
): ProjectFacts {
  const count = (pred: (t: Task) => boolean) => tasks.reduce((n, t) => (pred(t) ? n + 1 : n), 0);
  const active = agents.filter((a) => ACTIVE.has(a.state));
  const busiest = [...active].sort((a, b) => (b.lastActivityAt ?? "").localeCompare(a.lastActivityAt ?? ""))[0];
  return {
    running: count((t) => t.status === "Running" || t.status === "Testing" || t.status === "Review"),
    waiting: count((t) => t.status === "Waiting"),
    blocked: count((t) => t.status === "Blocked"),
    failed: count((t) => t.status === "Failed"),
    completedToday: count((t) => t.status === "Completed" && sameLocalDay(t.finishedAt, now)),
    total: tasks.length,
    activeAgents: active.length,
    focus: busiest ? { agent: busiest.name, activity: busiest.activity, state: busiest.state } : null,
    approvals,
    quality,
  };
}

/** Sentence ids (keys under `voice:summary.*`) in speaking order, with counts. */
export type SummaryPart =
  | { id: "idle" }
  | { id: "noTasks" }
  | { id: "running"; count: number }
  | { id: "agents"; count: number }
  | { id: "focus"; agent: string; activity: string }
  | { id: "completedToday"; count: number }
  | { id: "failed"; count: number }
  | { id: "blocked"; count: number }
  | { id: "approvals"; count: number }
  | { id: "buildFailed" }
  | { id: "buildOk" }
  | { id: "testsFailed"; count: number };

/**
 * The few most useful sentences, most urgent first, at most `max` (spoken
 * replies must stay short; the HUD shows the same text).
 */
export function summaryParts(f: ProjectFacts, max = 4): SummaryPart[] {
  const urgent: SummaryPart[] = [];
  if (f.approvals > 0) urgent.push({ id: "approvals", count: f.approvals });
  if (f.quality.lastBuild && !f.quality.lastBuild.ok) urgent.push({ id: "buildFailed" });
  if (f.quality.lastTests && f.quality.lastTests.failed > 0) urgent.push({ id: "testsFailed", count: f.quality.lastTests.failed });
  if (f.failed > 0) urgent.push({ id: "failed", count: f.failed });
  if (f.blocked > 0) urgent.push({ id: "blocked", count: f.blocked });

  const progress: SummaryPart[] = [];
  if (f.total === 0) progress.push({ id: "noTasks" });
  else if (f.running === 0 && f.activeAgents === 0) progress.push({ id: "idle" });
  if (f.running > 0) progress.push({ id: "running", count: f.running });
  if (f.activeAgents > 0) progress.push({ id: "agents", count: f.activeAgents });
  if (f.focus?.activity) progress.push({ id: "focus", agent: f.focus.agent, activity: f.focus.activity });
  if (f.completedToday > 0) progress.push({ id: "completedToday", count: f.completedToday });
  if (f.quality.lastBuild?.ok && urgent.length === 0) progress.push({ id: "buildOk" });

  // Progress first (context), then what needs attention; cap the length but keep urgent items.
  const keepUrgent = urgent.slice(0, max);
  const room = Math.max(1, max - keepUrgent.length);
  return [...progress.slice(0, room), ...keepUrgent].slice(0, Math.max(max, 1));
}

/** First sentence(s) of a long text, ≤ `maxChars`, for speaking. */
export function speakable(text: string, maxChars = 220): string {
  const clean = text
    .replace(/```[\s\S]*?```/g, " ")
    .replace(/`([^`]*)`/g, "$1")
    .replace(/[*_#>|]/g, " ")
    .replace(/\[([^\]]*)\]\([^)]*\)/g, "$1")
    .replace(/\s+/g, " ")
    .trim();
  if (clean.length <= maxChars) return clean;
  const sentences = clean.match(/[^.!?…]+[.!?…]+/g) ?? [];
  let out = "";
  for (const s of sentences) {
    if ((out + s).length > maxChars) break;
    out += s;
  }
  if (out.trim()) return out.trim();
  const cut = clean.slice(0, maxChars);
  const sp = cut.lastIndexOf(" ");
  return `${(sp > maxChars * 0.6 ? cut.slice(0, sp) : cut).trim()}…`;
}
