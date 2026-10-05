/**
 * Robot visual states (§30) and their mapping from the agent state the core
 * reports (`AgentStateChanged`). The robot never invents a state: what it
 * shows is what the agent is doing.
 */
import type { AgentState, Event } from "@orkestr/protocol";

export const ROBOT_STATES = [
  "Idle",
  "Listening",
  "Thinking",
  "Coding",
  "Testing",
  "Reviewing",
  "Waiting",
  "Error",
  "Completed",
] as const;
export type RobotState = (typeof ROBOT_STATES)[number];

export interface RobotVisual {
  state: RobotState;
  /** The agent was stopped: the robot "sleeps" (dimmed, eyes closed). */
  asleep: boolean;
}

/** Maps the agent state to the robot's visual state. */
export function visualStateFor(agent: AgentState): RobotVisual {
  if (agent === "Stopped") return { state: "Idle", asleep: true };
  return { state: agent, asleep: false };
}

/** Whether the robot shows work in progress. */
export function isBusy(state: RobotState): boolean {
  return state === "Thinking" || state === "Coding" || state === "Testing" || state === "Reviewing" || state === "Listening";
}

/**
 * Who gets a desktop slot first when there are more robots than slots:
 * problems, then results, then work, then idle robots.
 */
export function statePriority(state: RobotState, asleep = false): number {
  if (asleep) return 5;
  switch (state) {
    case "Error":
      return 0;
    case "Waiting":
      return 1;
    case "Completed":
      return 2;
    case "Idle":
      return 4;
    default:
      return 3;
  }
}

/** State colors as theme tokens (with Nebula fallbacks). `Idle` uses the robot's accent. */
export const STATE_COLOR: Readonly<Record<Exclude<RobotState, "Idle">, string>> = {
  Listening: "var(--ok-color-info, #60a5fa)",
  Thinking: "var(--ok-color-violet, #a78bfa)",
  Coding: "var(--ok-color-accent, #22d3ee)",
  Testing: "#a3e635",
  Reviewing: "var(--ok-color-magenta, #f472b6)",
  Waiting: "var(--ok-color-warning, #fbbf24)",
  Error: "var(--ok-color-danger, #fb7185)",
  Completed: "var(--ok-color-success, #34d399)",
};

export function stateColor(state: RobotState, accent: string): string {
  return state === "Idle" ? accent : STATE_COLOR[state];
}

/** Concrete hex colors per state (3D materials and lights cannot use CSS variables). */
export const STATE_HEX: Readonly<Record<Exclude<RobotState, "Idle">, string>> = {
  Listening: "#60a5fa",
  Thinking: "#a78bfa",
  Coding: "#22d3ee",
  Testing: "#a3e635",
  Reviewing: "#f472b6",
  Waiting: "#fbbf24",
  Error: "#fb7185",
  Completed: "#34d399",
};

export function stateHex(state: RobotState, accent: string): string {
  return state === "Idle" ? accent : STATE_HEX[state];
}

/** Designer live preview: the state shown at `t` when cycling every `periodMs`. */
export function previewStateAt(t: number, periodMs = 2600): RobotState {
  const i = Math.floor(Math.max(0, t) / periodMs) % ROBOT_STATES.length;
  return ROBOT_STATES[i] ?? "Idle";
}

/**
 * Robot state implied by a quality-pipeline event of the agent's task
 * (Phase 2): Build / Test stages running → `Testing`, Review / Security →
 * `Reviewing`. `null` clears the override (the stage finished), `undefined`
 * means the event says nothing about it.
 */
export function pipelineStateFor(ev: Event): RobotState | null | undefined {
  if (ev.kind !== "PipelineStageChanged") return undefined;
  const { stage, status } = ev.payload;
  if (status !== "Running") return null;
  if (stage === "Build" || stage === "Test") return "Testing";
  if (stage === "Review" || stage === "Security") return "Reviewing";
  return null;
}

/** The visual state with a pipeline override (only while the agent itself is not in a problem / result state). */
export function withPipelineState(visual: RobotVisual, override: RobotState | null): RobotVisual {
  if (!override || visual.asleep) return visual;
  if (visual.state === "Error" || visual.state === "Waiting") return visual;
  return { state: override, asleep: false };
}
