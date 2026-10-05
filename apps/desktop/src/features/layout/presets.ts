/**
 * Built-in layout presets (§29): Coding, Mission Control, Monitoring, Focus,
 * Review, Custom. A preset is a recipe (panel slots + proportions + an
 * optional multi-monitor spread), turned into a dockview layout by the
 * controller. Once the user changes a preset, the result is stored per
 * device in the core and the recipe is only used again on "Reset".
 *
 * Slots may name panels other areas are still building (Mission Control,
 * Reviews); they are skipped while the registry does not have them, so a
 * preset never shows a broken tab.
 */
import { Activity, Code2, Focus, GitPullRequest, LayoutDashboard, type LucideIcon, Shapes } from "lucide-react";

import type { BuiltinLayoutId } from "./schema";

export type SlotDirection = "left" | "right" | "above" | "below" | "within";

export interface PresetSlot {
  panel: string;
  /** Anchor: a panel placed earlier, with direction. Omitted = first group. */
  position?: { referencePanel: string; direction: SlotDirection };
}

/** Relative size of the group holding `panel` (fraction of the dock). */
export interface PresetSize {
  panel: string;
  width?: number;
  height?: number;
}

/** "Use all monitors": panel → monitor number (1-based, left-to-right). */
export interface MonitorHint {
  panel: string;
  monitor: number;
}

export interface BuiltinPreset {
  id: BuiltinLayoutId;
  /** English name stored in the core document (the UI shows `layout:presets.<id>.name`). */
  storedName: string;
  icon: LucideIcon;
  /** `Ctrl+1..5`. */
  shortcut?: string;
  slots: readonly PresetSlot[];
  sizes: readonly PresetSize[];
  /** Panel focused after applying. Falls back to the first placed panel. */
  focus: readonly string[];
  /** Spread across monitors (only hints whose monitor exists are applied). */
  spread: readonly MonitorHint[];
}

/** The Mission Control panel id (slots with an unregistered panel are skipped). */
export const MISSION_CONTROL_PANEL = "missionControl";
/**
 * Where Phase 2 review / merge status lives: the Git panel (Merge Manager
 * tab); per-task code and security reviews are in the task drawer.
 */
export const REVIEW_PANEL = "git";

const CODING: BuiltinPreset = {
  id: "coding",
  storedName: "Coding",
  icon: Code2,
  shortcut: "Ctrl+1",
  slots: [
    { panel: "code" },
    { panel: "manager", position: { referencePanel: "code", direction: "within" } },
    { panel: "files", position: { referencePanel: "code", direction: "left" } },
    { panel: "agents", position: { referencePanel: "code", direction: "right" } },
    { panel: "tasks", position: { referencePanel: "agents", direction: "within" } },
    { panel: "logs", position: { referencePanel: "code", direction: "below" } },
    { panel: "terminal", position: { referencePanel: "logs", direction: "within" } },
    { panel: "git", position: { referencePanel: "logs", direction: "within" } },
  ],
  sizes: [
    { panel: "files", width: 0.2 },
    { panel: "agents", width: 0.27 },
    { panel: "logs", height: 0.3 },
  ],
  focus: ["manager"],
  // Code on monitor 1, Mission Control on 2, Browser on 3 (§34).
  spread: [
    { panel: MISSION_CONTROL_PANEL, monitor: 2 },
    { panel: "browser", monitor: 3 },
  ],
};

const MISSION_CONTROL: BuiltinPreset = {
  id: "mission-control",
  storedName: "Mission Control",
  icon: LayoutDashboard,
  shortcut: "Ctrl+2",
  slots: [
    { panel: MISSION_CONTROL_PANEL },
    { panel: "manager", position: { referencePanel: MISSION_CONTROL_PANEL, direction: "within" } },
    { panel: "agents", position: { referencePanel: MISSION_CONTROL_PANEL, direction: "right" } },
    { panel: "tasks", position: { referencePanel: "agents", direction: "below" } },
    { panel: "monitoring", position: { referencePanel: MISSION_CONTROL_PANEL, direction: "below" } },
    { panel: "costs", position: { referencePanel: "monitoring", direction: "right" } },
    { panel: "logs", position: { referencePanel: "monitoring", direction: "within" } },
  ],
  sizes: [
    { panel: "agents", width: 0.3 },
    { panel: "monitoring", height: 0.34 },
  ],
  focus: [MISSION_CONTROL_PANEL, "manager"],
  spread: [
    { panel: "monitoring", monitor: 2 },
    { panel: "browser", monitor: 3 },
  ],
};

const MONITORING: BuiltinPreset = {
  id: "monitoring",
  storedName: "Monitoring",
  icon: Activity,
  shortcut: "Ctrl+3",
  slots: [
    { panel: "monitoring" },
    { panel: "costs", position: { referencePanel: "monitoring", direction: "right" } },
    { panel: "agents", position: { referencePanel: "costs", direction: "below" } },
    { panel: "logs", position: { referencePanel: "monitoring", direction: "below" } },
    { panel: "terminal", position: { referencePanel: "logs", direction: "within" } },
    { panel: "deployments", position: { referencePanel: "logs", direction: "within" } },
  ],
  sizes: [
    { panel: "costs", width: 0.32 },
    { panel: "logs", height: 0.36 },
  ],
  focus: ["monitoring"],
  spread: [{ panel: "logs", monitor: 2 }],
};

const FOCUS: BuiltinPreset = {
  id: "focus",
  storedName: "Focus",
  icon: Focus,
  shortcut: "Ctrl+4",
  slots: [
    { panel: "code" },
    { panel: "manager", position: { referencePanel: "code", direction: "right" } },
  ],
  sizes: [{ panel: "manager", width: 0.3 }],
  focus: ["code"],
  spread: [{ panel: "manager", monitor: 2 }],
};

const REVIEW: BuiltinPreset = {
  id: "review",
  storedName: "Review",
  icon: GitPullRequest,
  shortcut: "Ctrl+5",
  slots: [
    { panel: REVIEW_PANEL },
    { panel: "code", position: { referencePanel: REVIEW_PANEL, direction: "right" } },
    { panel: "tasks", position: { referencePanel: REVIEW_PANEL, direction: "left" } },
    { panel: "logs", position: { referencePanel: "code", direction: "below" } },
    { panel: "terminal", position: { referencePanel: "logs", direction: "within" } },
  ],
  sizes: [
    { panel: "tasks", width: 0.2 },
    { panel: "code", width: 0.45 },
    { panel: "logs", height: 0.28 },
  ],
  focus: [REVIEW_PANEL],
  spread: [{ panel: "code", monitor: 2 }],
};

const CUSTOM: BuiltinPreset = {
  ...CODING,
  id: "custom",
  storedName: "Custom",
  icon: Shapes,
  shortcut: undefined,
};

export const BUILTIN_PRESETS: readonly BuiltinPreset[] = [CODING, MISSION_CONTROL, MONITORING, FOCUS, REVIEW, CUSTOM];

export function getBuiltinPreset(id: string): BuiltinPreset | undefined {
  return BUILTIN_PRESETS.find((p) => p.id === id);
}

/**
 * Slots this build can place: unknown panels are skipped and slots anchored
 * on a skipped panel are re-anchored on its own anchor (or the first placed
 * panel), keeping the direction. Pure; tested.
 */
export function resolveSlots(slots: readonly PresetSlot[], isKnownPanel: (id: string) => boolean): PresetSlot[] {
  const placed: PresetSlot[] = [];
  const replacement = new Map<string, PresetSlot["position"] | undefined>();
  const anchorFor = (pos: PresetSlot["position"]): PresetSlot["position"] | undefined => {
    let current = pos;
    const seen = new Set<string>();
    while (current && !placed.some((s) => s.panel === current?.referencePanel)) {
      if (seen.has(current.referencePanel)) return undefined;
      seen.add(current.referencePanel);
      const next = replacement.get(current.referencePanel);
      // The skipped panel was the first one: anchor on whatever came first instead.
      if (next === undefined) {
        const first = placed[0];
        return first ? { referencePanel: first.panel, direction: current.direction } : undefined;
      }
      current = { referencePanel: next.referencePanel, direction: current.direction };
    }
    return current;
  };
  for (const slot of slots) {
    if (!isKnownPanel(slot.panel) || placed.some((s) => s.panel === slot.panel)) {
      replacement.set(slot.panel, slot.position ? anchorFor(slot.position) : undefined);
      continue;
    }
    const position = slot.position ? anchorFor(slot.position) : undefined;
    // The first placed panel has no anchor; later ones without one go "within" the first.
    if (placed.length === 0) placed.push({ panel: slot.panel });
    else {
      const first = placed[0];
      placed.push({
        panel: slot.panel,
        position: position ?? (first ? { referencePanel: first.panel, direction: "within" } : undefined),
      });
    }
  }
  return placed;
}

/** Spread hints applicable with `monitorCount` monitors (monitor 1 is the main window). */
export function applicableSpread(
  hints: readonly MonitorHint[],
  monitorCount: number,
  isKnownPanel: (id: string) => boolean,
): MonitorHint[] {
  const used = new Set<number>();
  const out: MonitorHint[] = [];
  for (const h of hints) {
    if (h.monitor < 2 || h.monitor > monitorCount || used.has(h.monitor) || !isKnownPanel(h.panel)) continue;
    used.add(h.monitor);
    out.push(h);
  }
  return out;
}
