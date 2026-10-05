/**
 * How the main window's robot host turns the selection into window calls:
 * open arguments (size from the design scale, remembered position per
 * template — used by one robot at a time — or a default slot) and when an
 * open window must be recreated (its scale changed).
 */
import type { RobotDesign, RobotsConfig } from "@orkestr/protocol";

import { CLUSTER_SIZE, robotWindowLayout } from "./layout";
import type { RobotSlot } from "./visibility";

/** Key the cluster window's position is stored under. */
export const CLUSTER_POSITION_KEY = "cluster";

export interface OpenArgs {
  agentId: string | null;
  width: number;
  height: number;
  x: number | null;
  y: number | null;
  slot: number;
  snap: boolean;
}

export interface OpenWindow {
  scale: number;
  /** Remembered-position key this window took (template id), if any. */
  positionKey: string | null;
}

/** The design a slot is drawn with. */
export function slotDesign(slot: RobotSlot, config: RobotsConfig): RobotDesign {
  return slot.design ?? (slot.templateId ? config.designs[slot.templateId] : undefined) ?? config.defaultDesign;
}

/**
 * Open arguments for a robot. `taken` = position keys already used by open
 * robots (a second robot of the same template gets a default slot).
 */
export function robotOpenArgs(
  slot: RobotSlot,
  config: RobotsConfig,
  slotIndex: number,
  taken: ReadonlySet<string>,
): { args: OpenArgs; window: OpenWindow } {
  const design = slotDesign(slot, config);
  const layout = robotWindowLayout(design.scale);
  const key = slot.templateId && !taken.has(slot.templateId) ? slot.templateId : null;
  const pos = key ? config.positions[key] : undefined;
  return {
    args: {
      agentId: slot.agentId,
      width: layout.width,
      height: layout.height,
      x: pos ? pos.x : null,
      y: pos ? pos.y : null,
      slot: slotIndex,
      snap: design.behaviour.snap,
    },
    window: { scale: design.scale, positionKey: pos ? key : null },
  };
}

export function clusterOpenArgs(config: RobotsConfig, slotIndex: number): OpenArgs {
  const pos = config.positions[CLUSTER_POSITION_KEY];
  return {
    agentId: null,
    width: CLUSTER_SIZE.width,
    height: CLUSTER_SIZE.height,
    x: pos ? pos.x : null,
    y: pos ? pos.y : null,
    slot: slotIndex,
    snap: true,
  };
}

/** Open robots whose window must be recreated because the design scale changed. */
export function staleWindows(
  open: ReadonlyMap<string, OpenWindow>,
  slots: ReadonlyMap<string, RobotSlot>,
  config: RobotsConfig,
): string[] {
  const out: string[] = [];
  for (const [id, w] of open) {
    const slot = slots.get(id);
    if (slot && Math.abs(slotDesign(slot, config).scale - w.scale) > 1e-3) out.push(id);
  }
  return out;
}
