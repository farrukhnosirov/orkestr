/** Pure helpers of the Replay view (unit-tested in `replay.test.tsx`). */
import type { ReplayPhase, ReplayStep, ReplayStepStatus } from "@orkestr/protocol";

import { agentActivity, agentName, agentStopDetail, snapshotLabel, taskReason } from "../../../lib/coreText";

const AGENT_TITLED = new Set(["AgentSpawned", "AgentStarted", "AgentStateChanged", "AgentStopped"]);

/** Display title of a step: agent names and automatic snapshot labels written by the core are localized. */
export function stepTitle(step: ReplayStep): string {
  if (AGENT_TITLED.has(step.kind)) return agentName(step.title);
  if (step.kind === "SnapshotCreated") return snapshotLabel(step.title);
  return step.title;
}

/**
 * Display text of a step's detail for kinds whose detail is a fixed core
 * string (agent activity, stop detail, task status reason); null for other kinds.
 */
export function coreStepDetail(step: ReplayStep): string | null {
  if (!step.detail) return null;
  switch (step.kind) {
    case "AgentStateChanged":
      return agentActivity(step.detail);
    case "AgentStopped":
      return agentStopDetail(step.detail);
    case "TaskStatusChanged":
      return taskReason(step.detail);
    default:
      return null;
  }
}

export const PHASES: readonly ReplayPhase[] = [
  "Request",
  "Plan",
  "Agents",
  "Delegation",
  "Tools",
  "Files",
  "Tests",
  "Reviews",
  "Approvals",
  "Merge",
  "Run",
];

export const SPEEDS: readonly number[] = [0.5, 1, 2, 4];

/** Base delay between two steps at 1× (ms). */
export const BASE_STEP_MS = 1200;

/** Delay between two steps at `speed`. */
export function stepDelay(speed: number): number {
  return Math.round(BASE_STEP_MS / (speed > 0 ? speed : 1));
}

/** A run of consecutive steps of the same phase (vertical timeline sections). */
export interface PhaseSegment {
  phase: ReplayPhase;
  steps: ReplayStep[];
}

/** Groups consecutive steps of the same phase; the order of steps is kept. */
export function segments(steps: readonly ReplayStep[]): PhaseSegment[] {
  const out: PhaseSegment[] = [];
  for (const s of steps) {
    const last = out[out.length - 1];
    if (last && last.phase === s.phase) last.steps.push(s);
    else out.push({ phase: s.phase, steps: [s] });
  }
  return out;
}

/** Cursor after a player action, clamped to `[0, total - 1]`. */
export function moveCursor(cursor: number, total: number, action: "first" | "prev" | "next" | "last"): number {
  if (total <= 0) return 0;
  const last = total - 1;
  switch (action) {
    case "first":
      return 0;
    case "last":
      return last;
    case "prev":
      return Math.max(0, Math.min(cursor, last) - 1);
    case "next":
      return Math.min(last, cursor + 1);
  }
}

export type Tone = "neutral" | "success" | "warning" | "danger";

export function statusTone(status: ReplayStepStatus): Tone {
  switch (status) {
    case "Success":
      return "success";
    case "Warning":
      return "warning";
    case "Error":
      return "danger";
    case "Info":
      return "neutral";
  }
}

/** `HH:MM:SS` local time. */
export function clock(ts: string): string {
  const d = new Date(ts);
  if (Number.isNaN(d.getTime())) return ts;
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${pad(d.getHours())}:${pad(d.getMinutes())}:${pad(d.getSeconds())}`;
}

/** Milliseconds since the first step (`+1.2 s` labels). */
export function offsetMs(steps: readonly ReplayStep[], step: ReplayStep): number {
  const first = steps[0];
  if (!first) return 0;
  return Math.max(0, Date.parse(step.ts) - Date.parse(first.ts));
}
