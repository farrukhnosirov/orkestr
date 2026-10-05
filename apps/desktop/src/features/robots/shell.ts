/**
 * Window management for floating robots (src-tauri `robots.rs`). These calls
 * only open/close/position windows and pass window-level signals; agent data
 * always comes from the core over RPC (§67). Outside Tauri they are no-ops.
 */
import { invoke } from "@tauri-apps/api/core";
import { emit, emitTo, listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";

import { isTauri } from "../../lib/tauri";
import type { DropZone, HitShape } from "./model/layout";

export const ROBOT_EVENTS = {
  /** shell → robot: cursor entered/left the interactive shape (`boolean`). */
  hover: "orkestr://robot/hover",
  /** shell → robot: drag started/ended (`boolean`). */
  drag: "orkestr://robot/drag",
  /** shell → robot: settled position after snapping (`{x, y, monitor}`). */
  moved: "orkestr://robot/moved",
  /** shell → main: dropped on a project (`{agentId, projectId}`). */
  drop: "orkestr://robot/drop",
  /** robot → main (via `robot_to_main`). */
  openAgent: "orkestr://robot/open-agent",
  hide: "orkestr://robot/hide",
  assign: "orkestr://robot/assign",
  openCluster: "orkestr://robot/open-cluster",
  voice: "orkestr://robot/voice",
  /** cluster → main: the cluster window loaded; send it the overflow ids. */
  clusterReady: "orkestr://robot/cluster-ready",
  /** main → all robot windows: the robots document changed; refetch it. */
  configChanged: "orkestr://robots/config-changed",
  /** main → cluster window: agent ids in the overflow cluster. */
  cluster: "orkestr://robots/cluster",
  /** robot → main: a robot saved its position (`{key, position}`), keep the cached document fresh. */
  positionSaved: "orkestr://robots/position-saved",
} as const;

export interface PositionSaved {
  key: string;
  position: { monitor: string | null; x: number; y: number };
}

export type MainRequest = "open-agent" | "hide" | "assign" | "open-cluster" | "voice" | "cluster-ready";

export interface RobotOpenArgs {
  agentId: string | null;
  width: number;
  height: number;
  x: number | null;
  y: number | null;
  slot: number;
  snap: boolean;
}

export interface RobotMoved {
  x: number;
  y: number;
  monitor: string | null;
}

export interface RobotDrop {
  agentId: string;
  projectId: string;
}

export const robotShell = {
  available: (): boolean => isTauri(),
  async open(args: RobotOpenArgs): Promise<void> {
    if (isTauri()) await invoke("robot_open", { args });
  },
  async close(agentId: string | null): Promise<void> {
    if (isTauri()) await invoke("robot_close", { agentId });
  },
  async closeAll(): Promise<void> {
    if (isTauri()) await invoke("robot_close_all");
  },
  async setShape(shape: HitShape, passThrough: boolean, snap: boolean): Promise<void> {
    if (isTauri()) await invoke("robot_set_shape", { shape, passThrough, snap });
  },
  async setDropZones(zones: DropZone[]): Promise<void> {
    if (isTauri()) await invoke("robot_set_drop_zones", { zones });
  },
  async toMain(request: MainRequest, payload: Record<string, unknown>): Promise<void> {
    if (isTauri()) await invoke("robot_to_main", { request, payload });
  },
  async startDragging(): Promise<void> {
    if (isTauri()) await getCurrentWindow().startDragging();
  },
  async broadcastConfigChanged(): Promise<void> {
    if (isTauri()) await emit(ROBOT_EVENTS.configChanged);
  },
  async positionSaved(p: PositionSaved): Promise<void> {
    if (isTauri()) await emitTo("main", ROBOT_EVENTS.positionSaved, p);
  },
  async sendCluster(agentIds: string[]): Promise<void> {
    if (isTauri()) await emitTo("robot-cluster", ROBOT_EVENTS.cluster, agentIds);
  },
  async on<T>(event: string, handler: (payload: T) => void): Promise<() => void> {
    if (!isTauri()) return () => {};
    return listen<T>(event, (e) => handler(e.payload));
  },
};
