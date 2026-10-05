/**
 * The robot's context menu (§30): which items exist and when they are
 * enabled. Pure, so the rules ("Stop agent" is always offered while the
 * agent runs — ADR-0016) are unit-tested.
 */
import type { AgentState } from "@orkestr/protocol";

export type RobotMenuItemId = "open" | "stop" | "hide" | "assign" | "voice";

export interface RobotMenuItem {
  id: RobotMenuItemId;
  enabled: boolean;
  /** Voice: current on/off state (rendered as a check). */
  checked?: boolean;
  danger?: boolean;
  /** Opens a submenu (project list). */
  submenu?: boolean;
}

export interface RobotMenuInput {
  state: AgentState;
  stoppedAt: string | null;
  voiceEnabled: boolean;
  /** Other projects the agent's template could be assigned to. */
  assignableProjects: number;
  /** The agent has a template (assigning spawns the template in the target project). */
  hasTemplate: boolean;
}

const ENDED: ReadonlySet<AgentState> = new Set<AgentState>(["Completed", "Stopped", "Error"]);

export function isRunning(state: AgentState, stoppedAt: string | null): boolean {
  return stoppedAt === null && !ENDED.has(state);
}

export function robotMenu(i: RobotMenuInput): RobotMenuItem[] {
  return [
    { id: "open", enabled: true },
    { id: "assign", enabled: i.hasTemplate && i.assignableProjects > 0, submenu: true },
    { id: "voice", enabled: true, checked: i.voiceEnabled },
    { id: "hide", enabled: true },
    { id: "stop", enabled: isRunning(i.state, i.stoppedAt), danger: true },
  ];
}

/** Keyboard navigation over enabled items (wraps). Returns the next index. */
export function nextEnabled(items: readonly RobotMenuItem[], from: number, dir: 1 | -1): number {
  const n = items.length;
  if (n === 0) return -1;
  for (let step = 1; step <= n; step++) {
    const i = (((from + dir * step) % n) + n) % n;
    if (items[i]?.enabled) return i;
  }
  return -1;
}
